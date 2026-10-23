"""Existing return-via reuse must be geometric, bounded, and independently checked."""
from dataclasses import replace
from pathlib import Path
import shutil
import pytest
from pcbir import CopperLayer, PhysicalNet, Point, TrackSegment, Via, nm_from_mm, route_global
from pcbir.physical import NetRoutingRule, RouteKind, CopperKeepout, PolygonWithHoles, PolygonRing
from pcbir.drc import run_physical_drc
from pcbir.unified_router import UnifiedRouterOptions, route_unified
from test_unified_router import _board


def crossing(required=True, returns=True):
    board = _board({'S': ((2, 10), (18, 10)), 'W': ((10, 1), (10, 20)), 'RETURN': ((2, 2),)})
    wall = TrackSegment('W', Point.mm(13, 4), Point.mm(13, 23), nm_from_mm('0.2'), CopperLayer.FRONT)
    vias = tuple(Via('RETURN', Point.mm(x, 12), nm_from_mm('0.6'), nm_from_mm('0.3')) for x in (11, 15)) if returns else ()
    rule = NetRoutingRule('S', RouteKind.GENERAL, width_nm=nm_from_mm('0.2'),
        max_length_nm=nm_from_mm(18), max_vias=2, allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK),
        require_return_vias=required, return_via_net='RETURN' if required else None,
        maximum_return_via_distance_nm=nm_from_mm(1) if required else None)
    # Close the wall's end detours without moving its pad or metal geometry.
    def rectangle(y0, y1):
        return PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
            ((12.8, y0), (13.2, y0), (13.2, y1), (12.8, y1)))))
    keepouts = tuple(CopperKeepout(f'wall-end-{i}', (CopperLayer.FRONT,), rectangle(y0, y1),
        block_tracks=True, block_vias=False, block_zones=False, block_pads=False,
        block_footprints=False) for i, (y0, y1) in enumerate(((0, 3.5), (23.5, 26))))
    return replace(board, tracks=(wall,), vias=vias, net_routing_rules=(rule,), copper_keepouts=keepouts)


@pytest.mark.parametrize('native', (False, True))
def test_negotiation_uses_existing_nearby_returns_without_adding_or_dropping_them(monkeypatch, native):
    if not native:
        import pcbir.native
        monkeypatch.setattr(pcbir.native, 'astar', lambda: None)
    board = crossing()
    result = route_unified(board, route_global(replace(board, tracks=(), vias=())), UnifiedRouterOptions(
        pitch_nm=nm_from_mm('0.25'), maximum_iterations=4, commit_rounds=2), only_nets=frozenset({'S'}))
    assert next(r for r in result.nets if r.net == 'S').connected
    signals = [via for via in result.board.vias if via.net == 'S']
    assert len(signals) == 2
    assert all(any((via.position.x_nm-r.position.x_nm)**2+(via.position.y_nm-r.position.y_nm)**2
                       <= nm_from_mm(1)**2 for r in board.vias) for via in signals)
    assert tuple(v for v in result.board.vias if v.net == 'RETURN') == board.vias
    assert board.tracks[0] in result.board.tracks
    assert not [f for f in run_physical_drc(result.board).findings if f.code == 'DRC-RETURN-VIA']


def test_missing_returns_do_not_silently_allow_signal_transitions():
    board = crossing(returns=False)
    result = route_unified(board, route_global(replace(board, tracks=(), vias=())), UnifiedRouterOptions(
        pitch_nm=nm_from_mm('0.25'), maximum_iterations=3, commit_rounds=1), only_nets=frozenset({'S'}))
    assert not next(r for r in result.nets if r.net == 'S').connected
    assert not [v for v in result.board.vias if v.net == 'S']
    assert result.board.tracks == board.tracks


def test_independent_drc_detects_uncovered_single_ended_via_and_preserves_pair_scope():
    board = crossing(returns=False)
    signal = Via('S', Point.mm(11, 13), nm_from_mm('0.6'), nm_from_mm('0.3'))
    report = run_physical_drc(replace(board, vias=(signal,)))
    missing = [f for f in report.findings if f.code == 'DRC-RETURN-VIA']
    assert len(missing) == 1 and missing[0].objects == ('via:0',)
    assert any(c.check == 'single_ended_return_vias' and c.status.value == 'executed' for c in report.coverage)
    plain = replace(board, net_routing_rules=(replace(board.net_routing_rules[0], require_return_vias=False),))
    assert not [f for f in run_physical_drc(replace(plain, vias=(signal,))).findings if f.code == 'DRC-RETURN-VIA']


def test_exact_distance_boundary_and_actual_barrel_span_are_required():
    from pcbir.return_paths import covering_return_vias
    board = crossing()
    signal = Via('S', Point.mm(5, 5), nm_from_mm('0.6'), nm_from_mm('0.3'))
    return_via = Via('RETURN', Point.mm(8, 9), nm_from_mm('0.6'), nm_from_mm('0.3'))
    rule = replace(board.net_routing_rules[0], maximum_return_via_distance_nm=nm_from_mm(5))
    assert covering_return_vias(replace(board, vias=(return_via,)), rule, signal) == (return_via,)
    moved = replace(return_via, position=Point(return_via.position.x_nm+1, return_via.position.y_nm))
    assert covering_return_vias(replace(board, vias=(moved,)), rule, signal) == ()
    stack = replace(board.stackup, copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK))
    short = replace(return_via, to_layer=CopperLayer.INTERNAL_1)
    assert covering_return_vias(replace(board, stackup=stack, vias=(short,)), rule, signal) == ()


def test_exact_commit_cannot_be_bypassed_by_a_permissive_raster(monkeypatch):
    from pcbir.unified_router import _Router
    board = crossing()
    monkeypatch.setattr(_Router, '_via_owners', lambda self, spec, **kw:
        self.rasters.via_array(spec.via_size, spec.via_drill, spec.clearance))
    result = route_unified(board, route_global(replace(board, tracks=(), vias=())),
        UnifiedRouterOptions(pitch_nm=nm_from_mm('0.25'), maximum_iterations=1, commit_rounds=1),
        only_nets=frozenset({'S'}))
    assert not next(r for r in result.nets if r.net == 'S').connected
    assert not [via for via in result.board.vias if via.net == 'S']
    assert result.board.vias == board.vias


def test_native_refill_confirms_existing_returns_join_both_reference_faces(tmp_path):
    from pcbir import CopperZone
    from pcbir.physical import IslandPolicy, ZoneConnection
    from pcbir.plane_verify import verify_filled_planes
    native = Path(shutil.which('kicad-cli') or 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not native.is_file():
        pytest.skip('native refill unavailable')
    board = crossing()
    outline = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((0.5, 0.5), (25.5, 0.5), (25.5, 25.5), (0.5, 25.5)))))
    board = replace(board, zones=(CopperZone('reference', 'RETURN',
        (CopperLayer.FRONT, CopperLayer.BACK), outline, pad_connection=ZoneConnection.SOLID,
        island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None),))
    result = route_unified(board, route_global(replace(board, tracks=(), vias=())),
        UnifiedRouterOptions(pitch_nm=nm_from_mm('0.25'), maximum_iterations=4, commit_rounds=2),
        only_nets=frozenset({'S'}))
    assert next(r for r in result.nets if r.net == 'S').connected
    checked = verify_filled_planes(result.board, kicad_cli=native,
                                  artifact_directory=tmp_path/'native-return-check')
    assert checked.passed, checked.findings


def test_early_escape_candidates_and_reservations_obey_existing_return_requirement():
    from pcbir.fanout import _legal_choices, _reserve, _maze_choices, FanoutOptions
    from pcbir.routing_clearance import RoutingClearanceIndex
    from pcbir.access_diagnostics import AccessRecorder
    board = crossing()
    pose = next(p for p in board.placements if p.reference == 'S_0')
    pad = board.footprints[pose.footprint].pads[0]
    index = RoutingClearanceIndex(board)
    recorder = AccessRecorder()
    choices = _legal_choices(board, index, 'S', pose.position, pose, pad,
                             (Point.mm(6, 13), Point.mm(11, 13)), recorder=recorder)
    assert choices and all(via.position == Point.mm(11, 13) for path, via in choices)
    assert any(reason == 'required_return_via' for phase, reason in recorder.counts)
    path = (TrackSegment('S', pose.position, Point.mm(6, 13), nm_from_mm('0.2'), CopperLayer.FRONT),)
    via = Via('S', Point.mm(6, 13), nm_from_mm('0.6'), nm_from_mm('0.3'))
    tracks, vias = [], []
    assert not _reserve(board, index, tracks, vias, (path, via))
    assert not tracks and not vias
    pins = ((board.nets[0].pads[0], 'S', pose.position, pose, pad),)
    assert not _maze_choices(board, index, 'S', pins,
        FanoutOptions(maximum_radius_nm=nm_from_mm(2), maze_state_budget=200))

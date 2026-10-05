"""A legal dogbone is not proof of a usable, jointly allocated package exit."""
from dataclasses import replace
from pathlib import Path
import shutil
from types import MappingProxyType

import pytest

import pcbir.boundary_access as boundary
from pcbir.boundary_access import BoundaryAccessOptions, analyze_boundary_access, package_collar
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutResult
from pcbir.physical import (BoardOutline, BoardSide, CopperKeepout, CopperLayer, CopperZone,
    FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size,
    Stackup, TrackSegment, Via, nm_from_mm)


def rectangle(name, layers, left, top, right, bottom):
    return CopperKeepout(name, layers, PolygonWithHoles(PolygonRing((
        Point.mm(left, top), Point.mm(right, top), Point.mm(right, bottom), Point.mm(left, bottom)))),
        block_tracks=True, block_vias=False, block_zones=False)


def fixture(two=False, layers=None):
    lands = (FootprintPad("1", Point.mm(0, -.5 if two else 0), Size.mm(.3, .3)),)
    if two:
        lands += (FootprintPad("2", Point.mm(0, .5), Size.mm(.3, .3)),)
    package = PhysicalFootprint("package", lands, Size.mm(8, 8))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    poses = (Placement("U", "package", Point.mm(6, 6)), Placement("J1", "one", Point.mm(15, 4)))
    nets = (PhysicalNet("A", (PadReference("U", "1"), PadReference("J1", "1"))),)
    if two:
        poses += (Placement("J2", "one", Point.mm(15, 8)),)
        nets += (PhysicalNet("B", (PadReference("U", "2"), PadReference("J2", "1"))),)
    board = PhysicalBoard("boundary", BoardOutline.rectangle(20, 16),
        {"package": package, "one": one}, poses, nets,
        stackup=Stackup(copper_layers=layers) if layers else Stackup())
    tracks, vias, accesses = [], [], {}
    for land, net in zip(lands, nets):
        start = Point(nm_from_mm(6), nm_from_mm(6) + land.position.y_nm)
        end = Point(nm_from_mm(6.8), start.y_nm)
        tracks.append(TrackSegment(net.name, start, end, board.rules.default_track_width_nm, CopperLayer.FRONT))
        vias.append(Via(net.name, end, board.rules.default_via_size_nm, board.rules.default_via_drill_nm))
        accesses[PadReference("U", land.number)] = end
    routed = replace(board, tracks=tuple(tracks), vias=tuple(vias))
    return FanoutResult(routed, MappingProxyType(accesses), (), len(tracks), len(vias),
                         tuple(vias), created_tracks=tuple(tracks))


def ring(layers):
    return (rectangle("left", layers, 2, 2, 2.5, 10),
            rectangle("right", layers, 9.5, 2, 10, 10),
            rectangle("top", layers, 2, 2, 10, 2.5),
            rectangle("bottom", layers, 2, 9.5, 10, 10))


def hard_errors(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def test_boundary_witness_is_connected_octilinear_beyond_collar_and_not_committed():
    fan = fixture()
    result = analyze_boundary_access(fan.board, fan)
    assert result.ready and len(result.ports) == 1
    port = result.ports[0]
    assert port.path[0].start == fan.accesses[port.pad]
    assert port.position == port.path[-1].end
    area = result.collars[0].bounds
    assert not (area.min_x <= port.position.x_nm <= area.max_x
                and area.min_y <= port.position.y_nm <= area.max_y)
    assert all(t.start.x_nm == t.end.x_nm or t.start.y_nm == t.end.y_nm or
               abs(t.start.x_nm-t.end.x_nm) == abs(t.start.y_nm-t.end.y_nm) for t in port.path)
    assert len(fan.board.tracks) == 1  # Witness is diagnostic; no board mutation.
    assert not hard_errors(replace(fan.board, tracks=(*fan.board.tracks, *port.path)))


def test_dogbone_inside_closed_channel_does_not_certify_package_exit():
    fan = fixture()
    routed = replace(fan.board, copper_keepouts=ring(fan.board.stackup.copper_layers))
    fan = replace(fan, board=routed)
    assert not hard_errors(routed)  # The launch/via is genuinely legal.
    result = analyze_boundary_access(routed, fan)
    assert not result.ready and result.pending_pads == (PadReference("U", "1"),)
    assert result.pin_analysis[0].diagnostic == "bounded boundary domain exhausted"


def test_individually_legal_ports_cannot_share_a_single_channel(monkeypatch):
    fan = fixture(two=True)
    fan = replace(fan, board=replace(fan.board, copper_keepouts=ring((CopperLayer.FRONT,))))
    monkeypatch.setattr(boundary, "_ports", lambda *args: (("right", Point.mm(10.5, 6)),))
    result = analyze_boundary_access(fan.board, fan, BoundaryAccessOptions(maze_escapes=False))
    assert all(p.candidate_count > 0 for p in result.pin_analysis)
    assert len(result.ports) == 1 and len(result.pending_pads) == 1 and not result.ready
    assert result.assignment.trials and not result.assignment.trials[0].solution_found
    assert not hard_errors(replace(fan.board, tracks=(*fan.board.tracks, *result.ports[0].path)))


def test_same_boundary_position_can_use_distinct_permitted_layers(monkeypatch):
    fan = fixture(two=True)
    monkeypatch.setattr(boundary, "_ports", lambda *args: (("right", Point.mm(10.5, 6)),))
    result = analyze_boundary_access(fan.board, fan)
    assert result.ready and len(result.ports) == 2
    assert len({p.layer for p in result.ports}) == 2


def test_signal_ports_cannot_use_dedicated_plane_even_if_through_via_crosses_it():
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    fan = fixture(layers=layers)
    board = replace(fan.board, nets=(*fan.board.nets, PhysicalNet("GND", ())),
        zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(
            BoardOutline.rectangle(20, 16).vertices))),),
        copper_keepouts=ring((CopperLayer.FRONT, CopperLayer.BACK, CopperLayer.INTERNAL_2)))
    fan = replace(fan, board=board)
    result = analyze_boundary_access(board, fan)
    assert not result.ready and not result.ports
    assert result.pin_analysis[0].candidate_count == 0
    # With the legal inner signal layer opened, the same via is a valid launch.
    board = replace(board, copper_keepouts=ring((CopperLayer.FRONT, CopperLayer.BACK)))
    result = analyze_boundary_access(board, replace(fan, board=board))
    assert result.ready and result.ports[0].layer is CopperLayer.INTERNAL_2


def test_explicit_signal_layer_restrictions_are_retained():
    fan = fixture(layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK))
    board = replace(fan.board, net_routing_rules=(NetRoutingRule("A", allowed_layers=(CopperLayer.FRONT,)),),
                    copper_keepouts=ring((CopperLayer.FRONT,)))
    result = analyze_boundary_access(board, replace(fan, board=board))
    assert not result.ready and not result.ports


def test_empty_coarse_domain_is_refined_without_relaxing_narrow_channel():
    fan = fixture()
    layers = fan.board.stackup.copper_layers
    # Coarse y=6/6.5 miss the 0.6 mm opening after accounting for the track
    # capsule. Anchor-local fine sampling reaches it with exact 45-degree legs.
    walls = (*ring(layers)[:1], *ring(layers)[2:],
             rectangle("east-above", layers, 9.5, 2, 10.7, 6),
             rectangle("east-below", layers, 9.5, 6.6, 10.7, 10))
    board = replace(fan.board, copper_keepouts=walls)
    fan = replace(fan, board=board)
    assert not hard_errors(board)
    coarse = analyze_boundary_access(board, fan, BoundaryAccessOptions(refinement_step_nm=nm_from_mm(.5), maze_escapes=False))
    fine = analyze_boundary_access(board, fan)
    assert not coarse.ready and fine.ready
    assert nm_from_mm(6.125) <= fine.ports[0].position.y_nm <= nm_from_mm(6.475)
    assert not hard_errors(replace(board, tracks=(*board.tracks, *fine.ports[0].path)))


def test_maze_witness_passes_staggered_walls_that_need_multiple_bends():
    fan = fixture()
    layers = fan.board.stackup.copper_layers
    walls = (*ring(layers)[:1], *ring(layers)[2:],
             rectangle("first-wall", layers, 7.4, 2, 7.6, 6.4),
             rectangle("second-wall", layers, 9, 6, 9.2, 10))
    board = replace(fan.board, copper_keepouts=walls)
    fan = replace(fan, board=board)
    simple = analyze_boundary_access(board, fan, BoundaryAccessOptions(maze_escapes=False))
    routed = analyze_boundary_access(board, fan)
    assert not simple.ready and routed.ready
    assert routed.pin_analysis[0].maze_candidate_count and routed.pin_analysis[0].maze_search_states
    assert len(routed.ports[0].path) > 2
    assert not hard_errors(replace(board, tracks=(*board.tracks, *routed.ports[0].path)))
    for first, second in zip(routed.ports[0].path, routed.ports[0].path[1:]):
        dx,dy = first.end.x_nm-first.start.x_nm, first.end.y_nm-first.start.y_nm
        xx,yy = second.end.x_nm-second.start.x_nm, second.end.y_nm-second.start.y_nm
        assert dx*xx + dy*yy > 0  # No 90-degree or sharper turns.


@pytest.mark.parametrize("defect", ["moved", "missing_owned_track", "missing_owned_via"])
def test_stale_or_missing_launch_ownership_is_rejected(defect):
    fan = fixture()
    board = fan.board
    if defect == "moved":
        board = replace(board, placements=(replace(board.placements[0], position=Point.mm(7, 6)), *board.placements[1:]))
    elif defect == "missing_owned_track":
        board = replace(board, tracks=())
    else:
        board = replace(board, vias=())
    with pytest.raises(ValueError):
        analyze_boundary_access(board, fan)


def test_claimed_disconnected_anchor_is_not_a_port():
    fan = fixture()
    fan = replace(fan, accesses={PadReference("U", "1"): Point.mm(8, 6)})
    result = analyze_boundary_access(fan.board, fan)
    assert not result.ready and not result.ports
    assert result.pin_analysis[0].diagnostic == "unverified connected launch"


def test_specialized_critical_launch_cannot_be_certified_as_ordinary():
    from pcbir.physical import RouteKind
    fan = fixture()
    board = replace(fan.board, net_routing_rules=(NetRoutingRule("A", RouteKind.CRITICAL),))
    with pytest.raises(ValueError, match="ordinary"):
        analyze_boundary_access(board, replace(fan, board=board))


def test_native_rejection_rolls_back_all_witnesses(monkeypatch):
    fan = fixture(two=True)
    real = boundary.run_physical_drc
    def reject(board):
        report = real(board)
        finding = next(f for f in report.findings if f.code == "DRC-OPEN-NET")
        return replace(report, findings=(replace(finding, code="DRC-CLEARANCE"),))
    monkeypatch.setattr(boundary, "run_physical_drc", reject)
    result = analyze_boundary_access(fan.board, fan)
    assert not result.native_accepted and not result.ready and not result.ports
    assert set(result.pending_pads) == set(fan.accesses)


def test_collar_transforms_courtyard_and_includes_outlying_pad():
    fan = fixture()
    fp = replace(fan.board.footprints["package"], courtyard=(
        Point.mm(-2, -1), Point.mm(2, -1), Point.mm(2, 1), Point.mm(-2, 1)))
    fp = replace(fp, pads=(*fp.pads, FootprintPad("9", Point.mm(4, 0), Size.mm(1, 1))))
    board = replace(fan.board, footprints={**fan.board.footprints, "package": fp},
                    placements=(replace(fan.board.placements[0], rotation_degrees=45, side=BoardSide.BACK),
                                *fan.board.placements[1:]))
    collar = package_collar(board, "U", 0)
    assert collar.bounds.min_x < nm_from_mm(3.6) and collar.bounds.min_y < nm_from_mm(4)
    assert collar.bounds.max_x > nm_from_mm(8) and collar.bounds.max_y > nm_from_mm(8)


@pytest.mark.parametrize("kwargs", [{"port_step_nm": 0}, {"maximum_detour_nm": -1},
                                   {"refinement_step_nm": nm_from_mm(1)}, {"maximum_ports_per_edge": 0}])
def test_invalid_bounds(kwargs):
    with pytest.raises(ValueError):
        BoundaryAccessOptions(**kwargs)


def test_installed_kicad_accepts_boundary_witness_geometry_without_claiming_connectivity():
    from pcbir.plane_verify import verify_filled_planes
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("KiCad not installed")
    fan = fixture()
    result = analyze_boundary_access(fan.board, fan)
    witness = replace(fan.board, tracks=(*fan.board.tracks, *result.ports[0].path))
    evidence = verify_filled_planes(witness, kicad_cli=Path(cli))
    # Partial witnesses intentionally have free ends, and a via used only on
    # one side is dangling. None of these is a geometry/clearance violation.
    assert all(f.split(":", 1)[0] in {"unconnected_items", "track_dangling", "via_dangling"}
               for f in evidence.findings), evidence.findings
    assert evidence.unconnected_count == 1 and not evidence.passed

"""Real native regressions for reserved escapes during area dependency repair."""
from collections import Counter
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir import (
    BoardOutline,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    PadReference,
    PhysicalBoard,
    PhysicalNet,
    Placement,
    Point,
    TrackSegment,
    Via,
    nm_from_mm,
)
from pcbir.fanout import FanoutResult
from pcbir.filled_feedback import FilledCopperFeedbackOptions, improve_filled_copper
from pcbir.pin_escape import RoutingAccess
from pcbir.physical import BoardSide, IslandPolicy, ZoneConnection
from pcbir.hard_macros import materialize_hard_macros
from pcbir.pad_via_arrays import via_in_pad_array_vias
from pcbir.plane_verify import verify_filled_planes, remove_native_dangling_copper
from pcbir.filled_feedback import _proposals
from routing_fixtures import pipeline, rectangle, terminal
from test_filled_feedback import fragmented
from test_autonomous_plane_access import with_test_owners


@pytest.fixture
def native_cli():
    path = Path(shutil.which('kicad-cli') or 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not path.is_file():
        pytest.skip('real native verification required')
    return path


def dependency_case(renamed=False, settled_witness=False, with_fanout=False):
    source = fragmented(renamed=renamed)
    signal = 'CONTROL_28' if renamed else 'SIGNAL'
    refs = ('DEVICE_A', 'DEVICE_B') if renamed else ('S1', 'S2')
    source = replace(source, copper_keepouts=(*source.copper_keepouts,
        CopperKeepout('back-barrier', (CopperLayer.BACK,), rectangle(0, 5, 20, 7),
                      block_tracks=True, block_vias=False, block_zones=False)),
        placements=(*source.placements,
        *(Placement(ref, 'terminal', Point.mm(10, y)) for ref, y in zip(refs, (3, 9)))),
        nets=(*source.nets, PhysicalNet(signal, tuple(PadReference(ref, '1') for ref in refs))))
    def track(a, b, layer):
        return TrackSegment(signal, Point.mm(*a), Point.mm(*b), nm_from_mm('0.25'), layer)
    lands = tuple(track((10, y), (9, y), CopperLayer.FRONT) for y in (3, 9))
    witnesses = tuple(track((9, y), (8, y), CopperLayer.BACK) for y in (3, 9))
    launches = tuple(Via(signal, Point.mm(9, y), nm_from_mm('0.6'), nm_from_mm('0.3'))
                     for y in (3, 9))
    prefix = replace(source, tracks=(*lands, *witnesses), vias=launches)
    area = (track((8, 3), (12, 3), CopperLayer.BACK),
            track((12, 3), (12, 9), CopperLayer.FRONT),
            track((12, 9), (8, 9), CopperLayer.BACK))
    area_vias = tuple(Via(signal, Point.mm(12, y), nm_from_mm('0.6'), nm_from_mm('0.3'))
                      for y in (3, 9))
    if settled_witness:
        # Main routing legitimately replaced the original collar witnesses.
        # The retained launches are still used on two layers by actual area
        # copper, not by the stale descriptor's now-absent segments.
        area = (track((9, 3), (12, 3), CopperLayer.BACK),
                area[1], track((12, 9), (9, 9), CopperLayer.BACK))
        settled = replace(prefix, tracks=lands)
    else:
        settled = prefix
    routed = replace(settled, tracks=(*settled.tracks, *area), vias=(*launches, *area_vias))
    original = pipeline(source)
    fanout = FanoutResult(prefix,
        {PadReference(ref, '1'): Point.mm(9, y) for ref, y in zip(refs, (3, 9))},
        (), len(prefix.tracks), len(launches), created_vias=launches,
        created_tracks=prefix.tracks,
        boundary_accesses={PadReference(ref, '1'): RoutingAccess(
            Point.mm(8, y), CopperLayer.BACK, Point.mm(9, y), (witness,))
            for ref, y, witness in zip(refs, (3, 9), witnesses)})
    original = replace(original, detailed=replace(original.detailed, board=routed))
    bridge = TrackSegment(source.nets[0].name, Point.mm(3, 6), Point.mm(17, 6),
                          nm_from_mm('0.25'), CopperLayer.FRONT)
    fixed = replace(settled, tracks=(*settled.tracks, bridge))
    case = original, fixed, area, area_vias, signal
    return (*case, fanout) if with_fanout else case


def test_native_evidence_relative_directory_survives_changed_subprocess_cwd(native_cli, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    initial, fixed, _, _, _ = dependency_case()
    checked = verify_filled_planes(initial.board, kicad_cli=native_cli,
                                  artifact_directory=Path('review/native'))
    assert checked.unconnected_count == 1
    assert checked.other_violation_count == 0
    assert Path(checked.artifact_directory) == tmp_path / 'review/native'
    assert (tmp_path / 'review/native/drc.json').is_file()


@pytest.mark.parametrize('renamed', (False, True))
def test_trial_budget_reaches_second_open_net_before_retrying_first(native_cli, renamed):
    names = ('RAIL_71', 'RAIL_19') if renamed else ('FIRST', 'SECOND')
    land = terminal()
    poses = tuple(Placement(f'T{row}_{col}', land.name, Point.mm(x, y))
                  for row, y in enumerate((5, 15)) for col, x in enumerate((3, 17)))
    nets = tuple(PhysicalNet(name, tuple(PadReference(f'T{row}_{col}', '1') for col in (0, 1)))
                 for row, name in enumerate(names))
    zones = tuple(CopperZone(f'fill-{row}', name, (CopperLayer.FRONT,),
        rectangle(1, 1+row*10, 19, 9+row*10), pad_connection=ZoneConnection.SOLID,
        island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None)
        for row, name in enumerate(names))
    board = PhysicalBoard('fair-native-trials', BoardOutline.rectangle(20, 20),
        {land.name: land}, poses, nets, zones=zones,
        copper_keepouts=(CopperKeepout('fill-split', (CopperLayer.FRONT,),
            rectangle('9.5', 0, '10.5', 20), block_tracks=False, block_vias=False, block_zones=True),
            CopperKeepout('first-rail-track-barrier', (CopperLayer.FRONT,),
            rectangle(8, 1, 12, 9), block_tracks=True, block_vias=False, block_zones=False)))
    initial = pipeline(board)
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=2, maximum_search_states=100,
                                           step_nm=nm_from_mm('0.5')))
    assert {trial.net for trial in result.trials} == set(names)
    assert any(trial.net == names[1] and trial.outcome == 'accepted' for trial in result.trials)
    assert result.verification.unconnected_count == 1
    assert result.verification.other_violation_count == result.verification.island_count == 0
    assert result.board.zones == initial.board.zones
    assert result.board.placements == initial.board.placements


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('protected', (False, True))
def test_native_via_cleanup_preserves_unrelated_open_and_owned_occurrences(native_cli, renamed, protected):
    board = fragmented(renamed=renamed)
    signal = 'LINK_26' if renamed else 'SIGNAL'
    refs = ('HOST_45', 'DEVICE_19') if renamed else ('S1', 'S2')
    copper = TrackSegment(signal, Point.mm(3, 3), Point.mm(17, 3),
                          nm_from_mm('0.25'), CopperLayer.FRONT)
    via = Via(signal, Point.mm(12, 3), nm_from_mm('0.6'), nm_from_mm('0.3'))
    board = replace(board, placements=(*board.placements,
        *(Placement(ref, 'terminal', Point.mm(x, 3)) for ref, x in zip(refs, (3, 17)))),
        nets=(*board.nets, PhysicalNet(signal, tuple(PadReference(ref, '1') for ref in refs))),
        tracks=(copper,), vias=(via,))
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    assert evidence.unconnected_count == 1
    assert any(finding.type == 'via_dangling' for finding in evidence.native_findings)
    guarded = Counter((via,)) if protected else Counter()
    cleaned, checked = remove_native_dangling_copper(board, evidence, Counter((copper,)),
        guarded, eligible_nets=frozenset({signal}), kicad_cli=native_cli)
    assert cleaned.tracks == board.tracks
    assert cleaned.placements == board.placements and cleaned.zones == board.zones
    assert checked.matches(cleaned) and checked.unconnected_count == 1
    if protected:
        assert cleaned is board and checked is evidence
    else:
        assert not cleaned.vias and checked.other_violation_count == 0
    stale = replace(board, name='changed-source')
    assert remove_native_dangling_copper(stale, evidence, Counter(), Counter(),
        eligible_nets=frozenset({signal}), kicad_cli=native_cli) == (stale, evidence)


@pytest.mark.parametrize('required_array', (False, True))
def test_cleanup_cannot_remove_macro_or_required_array_vias(native_cli, tmp_path, required_array):
    board = with_test_owners(fragmented(), tmp_path, required_array=required_array)
    # Keep the required array's inner return valid. The macro-owned ordinary
    # via is genuinely dangling, but cleanup must not delete owned copper;
    # required array occurrences must also survive with empty caller guards.
    board = materialize_hard_macros(board)
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    assert any(f.type == 'via_dangling' for f in evidence.native_findings)
    result, checked = remove_native_dangling_copper(board, evidence, Counter(), Counter(),
        eligible_nets=frozenset(net.name for net in board.nets), kicad_cli=native_cli)
    assert result is board and checked is evidence
    assert Counter(via_in_pad_array_vias(board)) <= Counter(result.vias)


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('existing_transition', (False, True))
def test_native_opposite_layer_items_use_only_existing_copper_transitions(
        native_cli, renamed, existing_transition):
    net = 'SUPPLY_46' if renamed else 'POWER'
    refs = ('HOST_13', 'LOAD_72', 'ENTRY_38') if renamed else ('SOURCE', 'LOAD', 'ENTRY')
    land = terminal()
    left = TrackSegment(net, Point.mm(3, 6), Point.mm(7, 6), nm_from_mm('0.25'), CopperLayer.FRONT)
    right = TrackSegment(net, Point.mm(13, 6), Point.mm(17, 6), nm_from_mm('0.25'), CopperLayer.BACK)
    board = PhysicalBoard('native-connected-anchor', BoardOutline.rectangle(20, 12),
        {land.name: land},
        (Placement(refs[0], land.name, Point.mm(3, 6)),
         Placement(refs[1], land.name, Point.mm(17, 6), side=BoardSide.BACK),
         Placement(refs[2], land.name, Point.mm(13, 6), side=BoardSide.BACK)),
        (PhysicalNet(net, tuple(PadReference(ref, '1') for ref in refs)),),
        tracks=(left, right),
        vias=(Via(net, Point.mm(16, 6), nm_from_mm('0.6'), nm_from_mm('0.3')),)
             if existing_transition else (),
        # This declared distribution zone is wholly excluded from native fill.
        # Its outline is not connectivity/transition evidence.
        zones=(CopperZone('empty-distribution', net, (CopperLayer.FRONT,),
            rectangle(1, 1, 4, 2), island_policy=IslandPolicy.REMOVE_ALL,
            minimum_island_area_nm2=None),),
        copper_keepouts=(CopperKeepout('empty-fill', (CopperLayer.FRONT,), rectangle(1, 1, 4, 2),
            block_tracks=False, block_vias=False, block_zones=True),))
    initial = pipeline(replace(board, tracks=(), vias=()))
    initial = replace(initial, detailed=replace(initial.detailed, board=board))
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=1, maximum_search_states=500))
    before = verify_filled_planes(initial.board, kicad_cli=native_cli)
    assert before.unconnected_count == 1 and before.island_count == 0, before.findings
    assert all(f.type in {'track_dangling', 'via_dangling'}
               for f in before.native_findings if f.category == 'violation'), before.findings
    items = next(f.items for f in before.native_findings if f.category == 'unconnected')
    assert all(item.kind in {'segment', 'pad'} for item in items)
    assert not set(items[0].layers) & set(items[1].layers)
    assert Counter(board.tracks) <= Counter(result.board.tracks)
    assert Counter(board.vias) <= Counter(result.board.vias)
    assert result.board.placements == board.placements and result.board.zones == board.zones
    if existing_transition:
        assert tuple(_proposals(initial.board, before, 64))
        assert result.stop_reason == 'verified' and result.verification.passed
        assert result.verification.matches(result.board)
    else:
        assert not tuple(_proposals(initial.board, before, 64))
        assert result.board == initial.board and not result.trials
        assert result.stop_reason == 'no_eligible_surface_bridge'


def test_required_unified_ports_survive_replay_and_reject_stale_witnesses(tmp_path):
    from pcbir.unified_router import UnifiedRouterOptions, route_unified, replay_unified
    from pcbir.state_codec import load_state, save_state
    initial, fixed, _, _, signal, fanout = dependency_case(with_fanout=True)
    path = tmp_path / 'required.state.json'
    options = UnifiedRouterOptions(pitch_nm=nm_from_mm('0.25'), maximum_iterations=4,
        commit_rounds=2, time_budget_s=10, save_inputs_path=str(path))
    routed = route_unified(fixed, initial.placement_and_global.global_route, options,
        only_nets=frozenset({signal}), required_accesses=fanout.boundary_accesses)
    replayed = replay_unified(path, save_inputs_path=None)
    assert replayed.board.tracks == routed.board.tracks
    assert replayed.board.vias == routed.board.vias
    saved = load_state(path)
    witness = next(iter(saved['required_accesses'].values())).path[0]
    tracks = list(saved['board'].tracks)
    tracks.remove(witness)
    saved['board'] = replace(saved['board'], tracks=tuple(tracks))
    stale = tmp_path / 'stale.state.json'
    save_state(stale, saved)
    with pytest.raises(ValueError, match='required routing access is stale or invalid'):
        replay_unified(stale, save_inputs_path=None)

"""Bounded native repair and rollback; real geometry, renamed selectors."""
from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path
import shutil

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone, NetRoutingRule,
    PadReference, PhysicalBoard, PhysicalNet, Placement, Point, RouteKind, TrackSegment, nm_from_mm,
    DetailedRouterOptions)
from pcbir.filled_feedback import FilledCopperFeedbackOptions, improve_filled_copper
from pcbir.filled_feedback import _proposals, _rejection_context
from pcbir.filled_connectivity import filled_component_roots
from pcbir.plane_verify import NativeFilledRegion, native_object_map, verify_filled_planes
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.manufacturing import CommandResult
from pcbir.physical import BoardSide, IslandPolicy, ZoneConnection
from pcbir.physical import Via
from test_autonomous_routing_quality import terminal, rectangle, pipeline, hard_findings
from test_autonomous_plane_access import with_test_owners
from pcbir.hard_macros import materialize_hard_macros, validate_hard_macros
from pcbir.pad_via_arrays import require_via_in_pad_arrays, via_in_pad_array_vias
from collections import Counter


@pytest.fixture
def native_cli():
    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        pytest.skip("real KiCad required")
    return cli


def fragmented(*, renamed=False, block_tracks=False):
    land = terminal()
    net = "SUPPLY_Z" if renamed else "POWER"
    refs = ("SOURCE_42", "IC_90") if renamed else ("SOURCE", "LOAD")
    return PhysicalBoard("native-repair", BoardOutline.rectangle(20, 12), {land.name: land},
        tuple(Placement(ref, land.name, Point.mm(x, 6)) for ref, x in zip(refs, (3, 17))),
        (PhysicalNet(net, tuple(PadReference(ref, "1") for ref in refs)),),
        zones=(CopperZone("regional-supply", net, (CopperLayer.FRONT,), rectangle(1, 1, 19, 11),
            pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL,
            minimum_island_area_nm2=None),),
        copper_keepouts=(CopperKeepout("fill-divider", (CopperLayer.FRONT,),
            rectangle("9.5", 0, "10.5", 12), block_tracks=block_tracks,
            block_vias=False, block_zones=True),))


@pytest.mark.parametrize("budget", (0, 1))
def test_unrepairable_source_preserves_original_and_reports_bounds(native_cli, budget):
    initial = pipeline(fragmented(block_tracks=True))
    options = FilledCopperFeedbackOptions(maximum_trials=budget, maximum_search_states=60)
    result = improve_filled_copper(initial, kicad_cli=native_cli, options=options)
    assert result.board == initial.board and result.pipeline == initial
    assert not result.verification.passed and result.verification.matches(result.board)
    assert result.stop_reason == ("disabled" if budget == 0 else "trial_budget_exhausted")
    assert len(result.trials) == budget
    if budget:
        assert result.trials[0].expanded_states <= 60
        assert result.trials[0].outcome == "no_legal_surface_bridge"


@pytest.mark.parametrize("renamed", (False, True))
def test_exact_blocker_records_are_stable_and_preserve_lock_authority(renamed):
    board = fragmented(renamed=renamed)
    moving = "SIGNAL_X" if renamed else "SIGNAL"
    board = replace(board, nets=(*board.nets, PhysicalNet(moving, ())))
    blocker = TrackSegment(moving, Point.mm(10, 3), Point.mm(10, 9), nm_from_mm("0.25"), CopperLayer.FRONT)
    query = TrackSegment(board.nets[0].name, Point.mm(3, 6), Point.mm(17, 6), nm_from_mm("0.25"), CopperLayer.FRONT)
    index = RoutingClearanceIndex(board)
    index.add_track(blocker)
    records = index.track_blocker_records(query)
    assert len(records) == 1 and records[0].net == moving and not records[0].immutable
    assert records[0].positions_nm == ((10000000,3000000),(10000000,9000000))
    assert records == index.track_blocker_records(query)
    locked = RoutingClearanceIndex(replace(board, tracks=(blocker,))).track_blocker_records(query)
    assert len(locked) == 1 and locked[0].immutable and locked[0].reason == "copper clearance"
    assert index.blocking_track_nets(query) == (frozenset({moving}), False)


def runner(report, *, version="10.0.6"):
    def run(command, cwd):
        if command[1:] == ("--version",):
            return CommandResult(0, version)
        Path(command[command.index("--output")+1]).write_text(json.dumps(report), encoding="utf-8")
        return CommandResult(0)
    return run


@pytest.mark.parametrize("state", ("missing_tool", "unsupported_tool", "invalid_native_result", "unsupported_native_objects"))
def test_tool_schema_and_unknown_object_failures_are_explicit(state, tmp_path):
    initial = pipeline(fragmented())
    def missing(*args):
        raise FileNotFoundError("missing native executable")
    native_runner = (missing if state == "missing_tool" else
        runner({"violations": [], "unconnected_items": []}, version="9.0.0") if state == "unsupported_tool" else
        runner({"violations": []}) if state == "invalid_native_result" else
        runner({"violations": [], "unconnected_items": [{"type":"unconnected_items",
            "items":[{"uuid":"unknown-external-object"}]}]}))
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=native_runner,
        artifact_directory=tmp_path / "evidence")
    assert result.board == initial.board and result.stop_reason == state
    assert not result.trials
    assert (tmp_path / "evidence" / "verification-001").is_dir()
    assert not (result.verification and result.verification.passed)


def test_retained_artifacts_and_exact_uuid_mapping(native_cli, tmp_path):
    board = fragmented()
    evidence = verify_filled_planes(board, kicad_cli=native_cli, artifact_directory=tmp_path / "evidence")
    root = Path(evidence.artifact_directory)
    assert sha256((root / "drc.json").read_bytes()).hexdigest() == evidence.report_digest
    assert sha256(next(root.glob("*.kicad_pcb")).read_bytes()).hexdigest() == evidence.filled_board_digest
    assert next((root / "input").glob("*.kicad_pcb")).is_file()
    assert next((root / "input").glob("*.kicad_pro")).is_file()
    assert evidence.filled_regions and len(evidence.filled_regions) == 2
    assert len({region.fragment_digest for region in evidence.filled_regions}) == 2
    assert all(item.kind == "zone" and item.region == "regional-supply"
               for finding in evidence.native_findings for item in finding.items)
    mapping = native_object_map(board)
    assert {(item.component, item.pad, item.net) for item in mapping.values() if item.kind == "pad"} == {
        ("SOURCE", "1", "POWER"), ("LOAD", "1", "POWER")}
    with pytest.raises(FileExistsError):
        verify_filled_planes(board, kicad_cli=native_cli, artifact_directory=root)


def test_anchor_cap_represents_both_fragments_when_one_has_many_lands(native_cli):
    board = fragmented()
    left = tuple(Placement(f"LEFT_{index}", "terminal",
                           Point(2_000_000 + (index % 10) * 600_000,
                                 2_000_000 + (index // 10) * 600_000))
                 for index in range(64))
    poses = (*left, Placement("RIGHT", "terminal", Point.mm(17, 6)))
    board = replace(board, placements=poses,
        nets=(PhysicalNet("POWER", tuple(PadReference(pose.reference, "1") for pose in poses)),))
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    assert not evidence.passed and evidence.unconnected_count == 1
    proposals = tuple(_proposals(board, evidence, 2))
    assert proposals and all({start.x_nm >= nm_from_mm(10), end.x_nm >= nm_from_mm(10)} == {False, True}
                             for _, _, _, start, end in proposals)


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("filled_return", (False, True))
def test_three_fragments_skip_already_connected_components(native_cli, renamed, filled_return):
    """Nearest polygons already share a plated return; bridge the third instead."""
    pytest.importorskip("shapely")
    net = "RAIL_83" if renamed else "POWER"
    refs = ("PART_A", "PART_B", "PART_C") if renamed else ("A", "B", "C")
    land = terminal()
    board = PhysicalBoard("three-fragments", BoardOutline.rectangle(30, 12), {land.name: land},
        tuple(Placement(ref, land.name, Point.mm(x, 6)) for ref, x in zip(refs, (3, 12, 27))),
        (PhysicalNet(net, tuple(PadReference(ref, "1") for ref in refs)),),
        zones=(CopperZone("front", net, (CopperLayer.FRONT,), rectangle(1, 1, 29, 11),
            pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL,
            minimum_island_area_nm2=None),),
        copper_keepouts=tuple(CopperKeepout(f"divider-{x}", (CopperLayer.FRONT,),
            rectangle(x, 0, x+1, 12), block_tracks=False, block_vias=False, block_zones=True)
            for x in (9, 19)),
        vias=tuple(Via(net, Point.mm(x, 6), nm_from_mm("0.6"), nm_from_mm("0.3"),
                       CopperLayer.FRONT, CopperLayer.BACK) for x in (4, 13)),
        tracks=tuple(TrackSegment(net, Point.mm(x, 6), Point.mm(x+1, 6),
            nm_from_mm("0.25"), CopperLayer.FRONT) for x in (3, 12)))
    if filled_return:
        board = replace(board, zones=(*board.zones, CopperZone("return", net,
            (CopperLayer.BACK,), rectangle(2, 5, 14, 7), pad_connection=ZoneConnection.SOLID,
            island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None)))
    else:
        board = replace(board, tracks=(*board.tracks, TrackSegment(net, Point.mm(4, 6), Point.mm(13, 6),
            nm_from_mm("0.25"), CopperLayer.BACK),))
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    assert evidence.unconnected_count == 1 and evidence.other_violation_count == 0
    proposals = tuple(_proposals(board, evidence, 2))
    assert proposals, "Anchor budget must represent distinct electrical components"
    assert all(27_000_000 in (start.x_nm, end.x_nm) for _, _, _, start, end in proposals)
    # Model a completed immutable prefix, not an illicit import of routed copper
    # into the source-only planning API.
    initial = pipeline(replace(board, tracks=(), vias=()))
    initial = replace(initial, detailed=replace(initial.detailed, board=board),
        fanout=replace(initial.fanout, board=board), critical=replace(initial.critical, board=board))
    assert not hard_findings(board), [(f.code, f.message) for f in hard_findings(board)]
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=1))
    assert result.stop_reason == "verified", result.to_json()
    assert result.verification.matches(result.board) and result.verification.passed
    assert Counter(board.tracks) <= Counter(result.board.tracks)
    assert Counter(board.vias) <= Counter(result.board.vias)


def test_native_rejection_rolls_back_real_candidate(tmp_path):
    initial = pipeline(fragmented())
    mapping = native_object_map(initial.board)
    pads = [item.uuid for item in mapping.values() if item.kind == "pad"]
    report = {"violations":[], "unconnected_items":[{"type":"unconnected_items",
        "items":[{"uuid":uuid} for uuid in pads]}]}
    # Adapter fixture deliberately refuses to certify a real geometric bridge.
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=runner(report),
        options=FilledCopperFeedbackOptions(maximum_trials=1), artifact_directory=tmp_path / "evidence")
    assert result.board == initial.board and result.pipeline == initial
    assert result.trials[0].outcome == "native_not_improved"
    assert result.trials[0].verification and not result.trials[0].verification.passed
    assert result.trials[0].candidate_digest != result.verification.board_digest


def test_stale_fill_is_not_proposal_authority(native_cli):
    board = fragmented()
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    changed = replace(board, name="different-export")
    assert not tuple(_proposals(changed, evidence, 64))
    roots = filled_component_roots(changed, evidence, frozenset({"POWER"}))
    assert not any(identity.startswith("fill:") for identity in roots)


def test_fill_grouping_does_not_join_a_drilled_void_or_another_layer(native_cli):
    pytest.importorskip("shapely")
    board = fragmented()
    board = replace(board, vias=(Via("POWER", Point.mm(10, 6), nm_from_mm("0.6"),
        nm_from_mm("0.3"), CopperLayer.FRONT, CopperLayer.BACK),))
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    zone = next(i for i in native_object_map(board).values() if i.kind == "zone")
    # Synthetic geometry exercises the grouping filter only; it is not a
    # claimed native certificate or a manufacturing approval.
    void = NativeFilledRegion(zone.uuid, "void", "POWER", "F.Cu",
        ((9_900_000,5_900_000),(10_100_000,5_900_000),(10_100_000,6_100_000),(9_900_000,6_100_000)))
    other_layer = NativeFilledRegion(zone.uuid, "back-only", "POWER", "B.Cu",
        ((2_000_000,5_000_000),(4_000_000,5_000_000),(4_000_000,7_000_000),(2_000_000,7_000_000)))
    roots = filled_component_roots(board, replace(evidence, filled_regions=(void, other_layer)),
                                   frozenset({"POWER"}))
    assert roots["fill:void"] != roots["via:0"]
    assert roots["fill:back-only"] != roots["pad:SOURCE.1:0"]


def test_native_rejection_context_invalidates_relevant_changes(native_cli):
    board = fragmented()
    evidence = verify_filled_planes(board, kicad_cli=native_cli)
    base = _rejection_context(board, evidence, "POWER", frozenset())
    other = TrackSegment("OTHER", Point.mm(2, 2), Point.mm(4, 2), nm_from_mm("0.25"), CopperLayer.BACK)
    unrelated = replace(board, nets=(*board.nets, PhysicalNet("OTHER", ())), tracks=(other,))
    assert _rejection_context(unrelated, evidence, "POWER", frozenset()) == base
    assert _rejection_context(replace(unrelated, tracks=(replace(other, layer=CopperLayer.FRONT),)),
        evidence, "POWER", frozenset()) != base
    assert _rejection_context(unrelated, evidence, "POWER", frozenset({"OTHER"})) != base
    assert _rejection_context(replace(board, vias=(Via("POWER", Point.mm(3, 6),
        nm_from_mm("0.6"), nm_from_mm("0.3"), CopperLayer.FRONT, CopperLayer.BACK),)),
        evidence, "POWER", frozenset()) != base
    assert _rejection_context(board, replace(evidence, filled_regions=()), "POWER", frozenset()) != base


def test_unrelated_accepted_bridge_does_not_repeat_rejected_native_pair():
    board = fragmented()
    board = replace(board, placements=(*board.placements,
        Placement("BS", "terminal", Point.mm(3, 9), side=BoardSide.BACK),
        Placement("BL", "terminal", Point.mm(17, 9), side=BoardSide.BACK)),
        nets=(*board.nets, PhysicalNet("OTHER", (PadReference("BS", "1"), PadReference("BL", "1")))),
        zones=(*board.zones, CopperZone("back-rail", "OTHER", (CopperLayer.BACK,),
            rectangle(1, 7, 19, 11), pad_connection=ZoneConnection.SOLID,
            island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None)))
    initial = pipeline(board)
    pads = [i for i in native_object_map(initial.board).values() if i.kind == "pad"]
    findings = [{"type":"unconnected_items", "items":[{"uuid":i.uuid} for i in pads if i.net == net]}
                for net in ("POWER", "OTHER")]
    calls = []
    def native_runner(command, cwd):
        if command[1:] == ("--version",):
            return CommandResult(0, "10.0.6")
        calls.append(command)
        # Initial + rejected front bridge keep both opens; the accepted back
        # bridge closes only OTHER. A fourth verification would be redundant.
        report = {"violations":[], "unconnected_items":findings if len(calls) < 3 else findings[:1]}
        Path(command[command.index("--output")+1]).write_text(json.dumps(report), encoding="utf-8")
        return CommandResult(0)
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=native_runner,
        options=FilledCopperFeedbackOptions(maximum_trials=4))
    assert [t.outcome for t in result.trials] == ["native_not_improved", "accepted"]
    assert len(calls) == 3 and result.stop_reason == "stagnation"
    assert result.verification.unconnected_count == 1 and not result.verification.passed


def test_repeated_candidate_is_not_reverified_and_exhaustion_is_not_impossibility():
    initial = pipeline(fragmented())
    pads = [item.uuid for item in native_object_map(initial.board).values() if item.kind == "pad"]
    finding = {"type":"unconnected_items", "items":[{"uuid":uuid} for uuid in pads]}
    report = {"violations":[], "unconnected_items":[finding, finding]}
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=runner(report),
        options=FilledCopperFeedbackOptions(maximum_trials=4))
    # Identical terminal-pair proposals are deduplicated before searching;
    # different proposals reaching an identical state still use the digest guard.
    assert [item.outcome for item in result.trials] == ["native_not_improved"]
    assert result.stop_reason == "stagnation" and result.board == initial.board


def test_source_zone_design_and_specialized_net_are_not_silently_changed():
    initial = pipeline(fragmented())
    report = {"violations":[{"type":"isolated_copper"}], "unconnected_items":[]}
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=runner(report))
    assert result.stop_reason == "source_zone_design" and result.board == initial.board
    critical = replace(initial.board, net_routing_rules=(NetRoutingRule("POWER", RouteKind.CRITICAL),))
    specialized = pipeline(critical)
    pads = [item.uuid for item in native_object_map(specialized.board).values() if item.kind == "pad"]
    report = {"violations":[], "unconnected_items":[{"type":"unconnected_items",
        "items":[{"uuid":uuid} for uuid in pads]}]}
    result = improve_filled_copper(specialized, kicad_cli=Path("fake"), runner=runner(report))
    assert not result.trials and result.board == specialized.board


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("protected", (False, True))
def test_bridge_cleans_unrelated_completed_area_end_but_preserves_prefix(native_cli, renamed, protected):
    board = fragmented(renamed=renamed)
    signal = "CONTROL_91" if renamed else "DATA"
    refs = ("HOST_8", "SENSOR_6") if renamed else ("S1", "S2")
    board = replace(board, placements=(*board.placements,
        *(Placement(ref, "terminal", Point.mm(x, 3)) for ref, x in zip(refs, (3, 7)))),
        nets=(*board.nets, PhysicalNet(signal, tuple(PadReference(ref, "1") for ref in refs))))
    initial = pipeline(board)
    assert next(item for item in initial.detailed.nets if item.net == signal).connected
    spur = TrackSegment(signal, Point.mm(3, 3), Point.mm(3, 2),
                        nm_from_mm("0.25"), CopperLayer.FRONT)
    candidate = replace(initial.board, tracks=(*initial.board.tracks, spur))
    initial = replace(initial, detailed=replace(initial.detailed, board=candidate))
    if protected:
        initial = replace(initial, fanout=replace(initial.fanout,
            board=replace(initial.fanout.board, tracks=(*initial.fanout.board.tracks, spur))))
    assert not hard_findings(candidate)
    evidence = verify_filled_planes(candidate, kicad_cli=native_cli)
    assert evidence.unconnected_count == 1 and evidence.other_violation_count == 1
    assert evidence.native_findings[-1].type == "track_dangling"
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=1))
    assert result.board.placements == candidate.placements and result.board.zones == candidate.zones
    if protected:
        assert result.board == candidate and not result.verification.passed
        assert spur in result.board.tracks
    else:
        assert result.verification.passed and result.verification.matches(result.board), result.to_json()
        assert spur not in result.board.tracks
        assert result.trials[0].displaced_nets == ()
        assert result.trials[0].cleanup_removed_tracks == 1


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("immutable", (False, True))
def test_native_bridge_dependency_transaction_preserves_signal_and_owners(native_cli, renamed, immutable):
    board = fragmented(renamed=renamed)
    signal = "CONTROL_72" if renamed else "DATA"
    refs = ("HOST_5", "SENSOR_7") if renamed else ("S1", "S2")
    board = replace(board, placements=(*board.placements,
        *(Placement(ref, "terminal", Point.mm(10, y)) for ref,y in zip(refs,(3,9)))),
        nets=(*board.nets, PhysicalNet(signal, tuple(PadReference(ref,"1") for ref in refs))))
    if immutable:
        board = replace(board, net_routing_rules=(NetRoutingRule(signal, RouteKind.CRITICAL,
            allowed_layers=(CopperLayer.FRONT,), max_vias=0),))
    initial = pipeline(board)
    if immutable:
        assert initial.critical.nets[0].connected
    else:
        assert next(item for item in initial.detailed.nets if item.net == signal).connected
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=2, step_nm=nm_from_mm("0.5")),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
            maximum_search_states=20000))
    assert result.board.placements == board.placements and result.board.zones == board.zones
    assert not hard_findings(result.board)
    if immutable:
        assert result.board == initial.board and not result.verification.passed
        assert any(item.immutable and item.net == signal for trial in result.trials for item in trial.blockers)
    else:
        assert result.stop_reason == "verified", result.to_json()
        assert result.verification.matches(result.board) and result.verification.passed
        trial = next(item for item in result.trials if item.outcome == "accepted")
        assert trial.displaced_nets == (signal,) and trial.dependency_searches >= 1
        assert any(item.net == signal and not item.immutable for item in trial.blockers)
        assert next(item for item in result.pipeline.detailed.nets if item.net == signal).connected


@pytest.mark.parametrize("value", (-1, True, "8", 1.5))
def test_feedback_bounds_reject_invalid_values(value):
    with pytest.raises(ValueError):
        FilledCopperFeedbackOptions(maximum_trials=value)


@pytest.mark.parametrize("required_array", (False, True))
@pytest.mark.parametrize("blocking_macro", (False, True))
def test_native_feedback_preserves_macro_and_required_array_copper(native_cli, tmp_path, required_array, blocking_macro):
    board = with_test_owners(fragmented(), tmp_path, required_array=required_array,
                             blocking_macro=blocking_macro)
    # The shared owner fixture's via is deliberately single-layer-connected.
    # Give it a real back-layer return for this strict native certificate; do
    # not waive the otherwise correct native via_dangling finding.
    x, y = (4, 6) if blocking_macro else (11, 11)
    board = replace(board, zones=(*board.zones, CopperZone("owned-via-return", "OWNER_NET",
        (CopperLayer.BACK,), rectangle(x-0.5,y-0.5,x+0.5,y+0.5),
        pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL,
        minimum_island_area_nm2=None)))
    owned = materialize_hard_macros(board)
    initial = pipeline(board)
    assert initial.package_access.ready and not hard_findings(initial.board)
    result = improve_filled_copper(initial, kicad_cli=native_cli,
        options=FilledCopperFeedbackOptions(maximum_trials=2))
    assert result.board.placements == board.placements and not hard_findings(result.board)
    assert Counter(owned.tracks) <= Counter(result.board.tracks)
    assert Counter(owned.vias) <= Counter(result.board.vias)
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "native feedback regression")
    if required_array:
        expected = Counter(via_in_pad_array_vias(board))
        actual = Counter(result.board.vias)
        assert sum(expected.values()) == 4
        assert all(actual[via] == count for via, count in expected.items())
    if blocking_macro:
        assert result.board == initial.board and not result.verification.passed
    else:
        assert result.verification.passed, (result.stop_reason, result.verification.findings,
                                           [trial.outcome for trial in result.trials])
        assert result.verification.matches(result.board)


def _scripted(reports):
    """Native runner returning the first report, then the second for every later check."""
    calls = []
    def run(command, cwd):
        if command[1:] == ("--version",):
            return CommandResult(0, "10.0.6")
        calls.append(command)
        report = reports[0] if len(calls) == 1 else reports[1]
        Path(command[command.index("--output")+1]).write_text(json.dumps(report), encoding="utf-8")
        return CommandResult(0)
    return run


@pytest.mark.parametrize("new_violation", (False, True))
def test_repair_is_judged_on_violations_it_adds(new_violation):
    initial = pipeline(fragmented())
    items = [i for i in native_object_map(initial.board).values() if i.kind == "pad"]
    open_fill = {"type": "unconnected_items", "items": [{"uuid": i.uuid} for i in items]}
    # Board artwork over a pad's mask opening: present before and after the
    # bridge, on the same objects, so it does not block a bridge that only
    # removes the open. A violation on other objects is new and does.
    artwork = {"type": "silk_over_copper", "severity": "warning", "items": [{"uuid": items[0].uuid}]}
    added = {"type": "clearance", "severity": "error", "items": [{"uuid": items[1].uuid}]}
    after = [artwork, added] if new_violation else [artwork]
    result = improve_filled_copper(initial, kicad_cli=Path("fake"), runner=_scripted((
        {"violations": [artwork], "unconnected_items": [open_fill]},
        {"violations": after, "unconnected_items": []})),
        options=FilledCopperFeedbackOptions(maximum_trials=1))
    assert [t.outcome for t in result.trials] == (["native_not_improved"] if new_violation else ["accepted"])
    assert result.verification.unconnected_count == (1 if new_violation else 0)

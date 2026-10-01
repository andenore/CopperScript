from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import shutil

import pytest

from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.plane_verify import verify_filled_planes
from pcbir.route_closure import reconcile_zone_lands
from pcbir.detailed import DetailedRouterOptions, route_detailed
from pcbir.manufacturing import CommandResult
from pcbir.routing import GlobalRouterOptions, route_global
from pcbir.routeflow import detailed_failure_trials
from pcbir.placement import PlacementPlannerOptions
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.drc import run_physical_drc
from pcbir.physical import (
    BoardOutline, CopperLayer, CopperZone, FootprintPad, NetRoutingRule, PadKind, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size,
    PolygonRing, PolygonWithHoles, Via, ZoneConnection, nm_from_mm,
)


def _dense_board() -> PhysicalBoard:
    pads = tuple(FootprintPad(str(index + 1), Point.mm(-2.75 + 0.5 * index, 0),
                              Size.mm("0.25", "0.5")) for index in range(12))
    dense = PhysicalFootprint("dense", pads, Size.mm(6, 2))
    terminal = PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),
    ), Size.mm(1, 1))
    return PhysicalBoard(
        "Fanout", BoardOutline.rectangle(20, 20),
        {dense.name: dense, terminal.name: terminal},
        (Placement("U1", dense.name, Point.mm(8, 10)),
         Placement("J1", terminal.name, Point.mm(16, 10))),
        (PhysicalNet("SIGNAL", (PadReference("U1", "1"), PadReference("J1", "1"))),),
    )


def test_dense_fanout_is_legal_deterministic_and_exposes_anchor() -> None:
    board = _dense_board()
    first = route_fanout(board)
    second = route_fanout(board)
    assert first == second
    assert first.added_track_count == first.added_via_count == 1
    assert first.accesses[PadReference("U1", "1")] == first.board.vias[0].position
    assert first.board.tracks[0].end == first.board.vias[0].position
    assert not first.pending_pads
    assert first.board.vias[0].position != first.board.tracks[0].start


def test_detailed_router_uses_fanout_via_as_access() -> None:
    board = _dense_board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    fanout = route_fanout(board)
    routed = route_detailed(fanout.board, guide,
                            DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"),
                                                  maximum_passes=2),
                            fanout_accesses=fanout.accesses,
                            fanout_created_vias=frozenset((v.net, v.position) for v in fanout.created_vias),
                            fanout_created_tracks=fanout.created_tracks)
    assert routed.metrics.routed_net_count == 1
    assert routed.locked_via_count == 1
    assert routed.board.tracks[0] == fanout.board.tracks[0]
    assert not routed.board.vias  # The front-only route does not need a drill.


def test_same_net_fanout_drills_respect_hole_spacing() -> None:
    board = _dense_board()
    board = replace(board, nets=(PhysicalNet("SIGNAL", (
        PadReference("U1", "1"), PadReference("U1", "2"),
        PadReference("J1", "1"),
    )),))
    result = route_fanout(board)
    assert len(result.board.vias) == 2
    for first in result.board.vias:
        for second in result.board.vias:
            if first is second:
                continue
            distance_sq = ((first.position.x_nm - second.position.x_nm) ** 2
                           + (first.position.y_nm - second.position.y_nm) ** 2)
            required = (first.drill_nm // 2 + second.drill_nm // 2
                        + board.rules.minimum_hole_clearance_nm)
            assert distance_sq >= required * required


def test_fanout_reuses_existing_matching_via() -> None:
    board = _dense_board()
    initial = route_fanout(board)
    seeded = replace(board, vias=(initial.board.vias[0],))
    result = route_fanout(seeded)
    assert result.added_track_count == 1
    assert result.added_via_count == 0
    assert len(result.board.vias) == 1
    assert result.accesses[PadReference("U1", "1")] == seeded.vias[0].position
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    routed = route_detailed(
        result.board, guide, DetailedRouterOptions(pitch_nm=nm_from_mm("0.5")),
        fanout_accesses=result.accesses,
        fanout_created_vias=frozenset((item.net, item.position)
                                    for item in result.created_vias),
    )
    assert seeded.vias[0] in routed.board.vias


def test_same_net_drill_collision_is_rejected_by_router_and_native_drc() -> None:
    board = _dense_board()
    via = Via("SIGNAL", Point.mm(4, 4), nm_from_mm("0.8"), nm_from_mm("0.4"),
              CopperLayer.FRONT, CopperLayer.BACK)
    board = replace(board, vias=(via,))
    too_close = Point.mm(4.5, 4)
    index = RoutingClearanceIndex(board)
    assert not index.can_via("SIGNAL", too_close, via.size_nm,
                             CopperLayer.FRONT, CopperLayer.BACK)
    assert index.can_via("SIGNAL", via.position, via.size_nm,
                         CopperLayer.FRONT, CopperLayer.BACK)
    duplicate = replace(via, position=too_close)
    bad = replace(board, vias=(via, duplicate))
    assert any(item.code == "DRC-DRILL-SPACING"
               for item in run_physical_drc(bad).findings)


def test_via_near_plated_pad_drill_is_checked_without_invalid_pad_copy() -> None:
    pad = FootprintPad("1", Point(0, 0), Size.mm("1", "1"),
                       kind=PadKind.THROUGH_HOLE, drill=Size.mm("0.5", "0.5"))
    footprint = PhysicalFootprint("plated", (pad,), Size.mm(2, 2))
    via = Via("SIGNAL", Point.mm(6.6, 6), nm_from_mm("0.8"),
              nm_from_mm("0.4"), CopperLayer.FRONT, CopperLayer.BACK)
    board = PhysicalBoard(
        "Plated", BoardOutline.rectangle(12, 12), {footprint.name: footprint},
        (Placement("J1", footprint.name, Point.mm(6, 6)),),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"),)),),
        vias=(via,),
    )
    assert any(item.code == "DRC-DRILL-SPACING"
               for item in run_physical_drc(board).findings)


def test_detailed_router_rejects_forged_fanout_anchor() -> None:
    board = _dense_board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(
        board, guide, DetailedRouterOptions(maximum_passes=1),
        fanout_accesses={PadReference("U1", "1"): Point.mm(6, 8)},
    )
    assert result.metrics.unrouted_net_count == 1
    assert "unverified fanout anchor" in result.nets[0].diagnostics[0]


def test_detailed_failure_trials_respect_fixed_parts_and_existing_copper() -> None:
    board = _dense_board()
    fixed = PlacementPlannerOptions(fixed_references=("U1",))
    trials = detailed_failure_trials(board, frozenset({"SIGNAL"}), fixed,
                                     nm_from_mm("0.5"), 4)
    assert trials
    assert all(trial.placements[0] == board.placements[0] for trial in trials)
    assert all(trial.placements[1] != board.placements[1] for trial in trials)
    with_copper = replace(board, tracks=(route_fanout(board).board.tracks[0],))
    assert detailed_failure_trials(with_copper, frozenset({"SIGNAL"}), fixed,
                                   nm_from_mm("0.5"), 4) == ()


def test_fanout_reports_unavailable_layer_without_mutating_board() -> None:
    board = replace(_dense_board(), net_routing_rules=(
        NetRoutingRule("SIGNAL", allowed_layers=(CopperLayer.FRONT,)),
    ))
    result = route_fanout(board, FanoutOptions())
    assert result.board is board
    assert result.pending_pads == (PadReference("U1", "1"),)


def _fake_runner(report: dict, *, status: int = 0):
    def run(command: tuple[str, ...], cwd: Path) -> CommandResult:
        if command[1:] == ("--version",):
            return CommandResult(0, "10.0.6\n")
        if command[1:3] == ("pcb", "drc"):
            output = Path(command[command.index("--output") + 1])
            output.write_text(json.dumps(report), encoding="utf-8")
            return CommandResult(status, stderr="failed" if status else "")
        raise AssertionError(command)
    return run


def test_plane_verification_detects_open_island_and_stale_geometry() -> None:
    board = _dense_board()
    evidence = verify_filled_planes(
        board, kicad_cli=Path("kicad-cli"), runner=_fake_runner({
            "violations": [{"type": "isolated_copper", "description": "island"}],
            "unconnected_items": [{"type": "unconnected_items", "description": "open"}],
        }),
    )
    assert not evidence.passed
    assert (evidence.unconnected_count, evidence.island_count) == (1, 1)
    assert evidence.matches(board)
    assert not evidence.matches(replace(board, name="Changed"))


def test_plane_verification_fails_closed_on_tool_and_schema_errors() -> None:
    board = _dense_board()
    with pytest.raises(RuntimeError, match="refill/DRC failed"):
        verify_filled_planes(board, kicad_cli=Path("kicad-cli"),
                             runner=_fake_runner({"violations": [],
                                                  "unconnected_items": []}, status=2))
    with pytest.raises(RuntimeError, match="lacks violations"):
        verify_filled_planes(board, kicad_cli=Path("kicad-cli"),
                             runner=_fake_runner({"violations": []}))


def test_library_findings_do_not_hide_verified_connectivity_or_pass_signoff() -> None:
    board = _dense_board()
    evidence = verify_filled_planes(board, kicad_cli=Path("kicad-cli"), runner=_fake_runner({
        "violations": [{"type": "lib_footprint_mismatch", "description": "library copy"}],
        "unconnected_items": [],
    }))
    assert not evidence.passed
    assert evidence.zone_connectivity_verified(board)
    assert not evidence.zone_connectivity_verified(replace(board, name="Stale"))
    assert not replace(evidence, other_violation_count=0).zone_connectivity_verified(board)


@pytest.mark.parametrize("kind", ["clearance", "shorting_items", "isolated_copper", "unknown"])
def test_other_violations_prevent_zone_land_reconciliation(kind: str) -> None:
    board = _dense_board()
    evidence = verify_filled_planes(board, kicad_cli=Path("kicad-cli"), runner=_fake_runner({
        "violations": [{"type": kind, "description": "unresolved"}],
        "unconnected_items": [],
    }))
    assert not evidence.zone_connectivity_verified(board)


def test_reconciliation_only_resolves_zone_lands_with_fresh_matching_evidence() -> None:
    board = _dense_board()
    board = replace(board, zones=(CopperZone(
        "signal-plane", "SIGNAL", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
            (1, 1), (19, 1), (19, 19), (1, 19))))),
    ),))
    pending = (PadReference("U1", "1"), PadReference("unknown", "1"))
    evidence = verify_filled_planes(board, kicad_cli=Path("kicad-cli"), runner=_fake_runner({
        "violations": [], "unconnected_items": [],
    }))
    assert reconcile_zone_lands(board, pending, evidence) == (pending[1:], pending[:1])
    assert reconcile_zone_lands(board, pending, None) == (pending, ())
    assert reconcile_zone_lands(replace(board, name="Changed"), pending, evidence) == (pending, ())
    assert reconcile_zone_lands(board, pending, replace(evidence, unconnected_count=1)) == (pending, ())


def test_installed_kicad_can_verify_a_filled_connected_plane() -> None:
    executable = shutil.which("kicad-cli")
    if executable is None:
        installed = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
        if not installed.is_file():
            pytest.skip("KiCad CLI not installed")
        executable = str(installed)
    outline = PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(19, 1),
        Point.mm(19, 19), Point.mm(1, 19),
    )))
    board = replace(_dense_board(), zones=(
        CopperZone("signal", "SIGNAL", (CopperLayer.FRONT,), outline,
                   pad_connection=ZoneConnection.SOLID),
    ))
    evidence = verify_filled_planes(board, kicad_cli=Path(executable))
    assert evidence.passed
    assert evidence.unconnected_count == evidence.island_count == 0
    restricted = replace(board, zones=(replace(board.zones[0], outline=PolygonWithHoles(
        PolygonRing((Point.mm(1, 1), Point.mm(10, 1),
                     Point.mm(10, 19), Point.mm(1, 19))),
    )),))
    incomplete = verify_filled_planes(restricted, kicad_cli=Path(executable))
    assert not incomplete.passed
    assert incomplete.unconnected_count > 0


def test_installed_kicad_accepts_pruned_fanout_route() -> None:
    executable = shutil.which("kicad-cli")
    if executable is None:
        installed = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
        if not installed.is_file():
            pytest.skip("KiCad CLI not installed")
        executable = str(installed)
    board = _dense_board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    fanout = route_fanout(board)
    detailed = route_detailed(
        fanout.board, guide, DetailedRouterOptions(pitch_nm=nm_from_mm("0.5")),
        fanout_accesses=fanout.accesses,
        fanout_created_vias=frozenset((v.net, v.position) for v in fanout.created_vias),
        fanout_created_tracks=fanout.created_tracks,
    )
    evidence = verify_filled_planes(detailed.board, kicad_cli=Path(executable))
    assert evidence.passed, evidence.findings

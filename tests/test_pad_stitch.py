from __future__ import annotations

from pcbir import (
    BoardOutline, CopperKeepout, CopperLayer, FootprintPad, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    PolygonRing, PolygonWithHoles, Size, run_physical_drc,
    stitch_duplicate_pads,
)
from dataclasses import replace
from pcbir import (TrackSegment, Via, PadKind, NetRoutingRule, nm_from_mm,
                   run_routing_pipeline, PlacementPlannerOptions,
                   PlacementRoutingFeedbackOptions, DetailedRouterOptions)


def _board(*, blocking_middle: bool) -> PhysicalBoard:
    pads = [
        FootprintPad("1", Point.mm(0, -2), Size.mm(1, 1)),
        FootprintPad("1", Point.mm(0, 2), Size.mm(1, 1)),
    ]
    if blocking_middle:
        pads.append(FootprintPad("2", Point.mm(0, 0), Size.mm(1, 1)))
    footprint = PhysicalFootprint("test/duplicate-land", tuple(pads), Size.mm(4, 6))
    nets = [PhysicalNet("A", (PadReference("U1", "1"),))]
    if blocking_middle:
        nets.append(PhysicalNet("B", (PadReference("U1", "2"),)))
    return PhysicalBoard(
        "DuplicateLand", BoardOutline.rectangle(20, 20),
        {footprint.name: footprint},
        (Placement("U1", footprint.name, Point.mm(10, 10)),),
        tuple(nets),
    )


def test_stitches_separate_same_number_lands() -> None:
    board = _board(blocking_middle=False)
    result = stitch_duplicate_pads(board)

    assert result.stitched == (PadReference("U1", "1"),)
    assert result.pending == ()
    assert result.added_track_count == 1
    assert {result.board.tracks[0].start, result.board.tracks[0].end} == {
        Point.mm(10, 8), Point.mm(10, 12),
    }
    assert stitch_duplicate_pads(board) == result
    repeated = stitch_duplicate_pads(result.board)
    assert repeated.stitched == result.stitched
    assert repeated.added_track_count == 0
    assert repeated.board == result.board


def test_detours_around_foreign_pad_between_same_number_lands() -> None:
    board = _board(blocking_middle=True)
    result = stitch_duplicate_pads(board)

    assert result.stitched == (PadReference("U1", "1"),)
    assert result.pending == ()
    assert result.added_track_count == 3
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-BOARD-EDGE",
    }
    repeated = stitch_duplicate_pads(result.board)
    assert repeated.added_track_count == 0


def test_unavoidable_keepout_leaves_duplicate_lands_pending() -> None:
    board = _board(blocking_middle=False)
    wall = CopperKeepout(
        "wall", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 9), Point.mm(19, 9),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    result = stitch_duplicate_pads(replace(board, copper_keepouts=(wall,)))

    assert result.stitched == ()
    assert result.pending == (PadReference("U1", "1"),)
    assert result.added_track_count == 0


def test_single_logical_pin_with_disconnected_physical_lands_is_open() -> None:
    board = _board(blocking_middle=False)
    assert any(item.code == "DRC-OPEN-NET" for item in run_physical_drc(board).findings)
    assert not any(item.code == "DRC-OPEN-NET"
                   for item in run_physical_drc(stitch_duplicate_pads(board).board).findings)


def test_existing_pad_edge_via_and_back_layer_path_needs_no_surface_bridge() -> None:
    board = _board(blocking_middle=False)
    width = nm_from_mm("0.2")
    board = replace(board, tracks=(
        TrackSegment("A", Point.mm(10, "8.4"), Point.mm(9, "8.4"), width, CopperLayer.FRONT),
        TrackSegment("A", Point.mm(9, "8.4"), Point.mm(9, "12.4"), width, CopperLayer.BACK),
        TrackSegment("A", Point.mm(9, "12.4"), Point.mm(10, "12.4"), width, CopperLayer.FRONT),
    ), vias=tuple(Via("A", Point.mm(9, y), nm_from_mm("0.6"), nm_from_mm("0.3"))
                  for y in ("8.4", "12.4")))
    result = stitch_duplicate_pads(board)
    assert result.board is board
    assert result.added_track_count == 0
    assert result.pending == ()
    assert result.already_connected == (PadReference("U1", "1"),)


def test_partial_existing_tree_only_bridges_disconnected_land_groups() -> None:
    board = _board(blocking_middle=False)
    footprint = board.footprints["test/duplicate-land"]
    footprint = replace(footprint, pads=(*footprint.pads,
                        replace(footprint.pads[0], position=Point(0, 0))))
    existing = TrackSegment("A", Point.mm(10, 8), Point.mm(10, 10),
                            nm_from_mm("0.2"), CopperLayer.FRONT)
    board = replace(board, footprints={footprint.name: footprint}, tracks=(existing,))
    result = stitch_duplicate_pads(board)
    assert result.added_track_count == 1
    assert result.board.tracks[0] == existing
    assert {result.board.tracks[1].start, result.board.tracks[1].end} == {
        Point.mm(10, 10), Point.mm(10, 12)}
    assert stitch_duplicate_pads(result.board).added_track_count == 0


def test_explicit_layer_restriction_prevents_new_surface_bridge() -> None:
    board = replace(_board(blocking_middle=False), net_routing_rules=(
        NetRoutingRule("A", allowed_layers=(CopperLayer.BACK,)),))
    result = stitch_duplicate_pads(board)
    assert result.pending == (PadReference("U1", "1"),)
    assert result.board is board


def test_existing_plated_land_path_is_connected_not_pending() -> None:
    board = _board(blocking_middle=False)
    footprint = board.footprints["test/duplicate-land"]
    footprint = replace(footprint, pads=tuple(replace(
        pad, kind=PadKind.THROUGH_HOLE, drill=Size.mm("0.4", "0.4"))
        for pad in footprint.pads))
    board = replace(board, footprints={footprint.name: footprint}, tracks=(
        TrackSegment("A", Point.mm(10, 8), Point.mm(10, 12),
                     nm_from_mm("0.2"), CopperLayer.BACK),))
    result = stitch_duplicate_pads(board)
    assert result.pending == ()
    assert result.already_connected == (PadReference("U1", "1"),)
    assert result.board is board


def test_pipeline_closes_lands_before_scoring_and_measures_added_copper() -> None:
    board = _board(blocking_middle=False)
    terminal = PhysicalFootprint("test/terminal", (
        FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(1, 1))
    board = replace(board, footprints={**board.footprints, terminal.name: terminal},
                    placements=(*board.placements, Placement("J1", terminal.name, Point.mm(17, 8))),
                    nets=(PhysicalNet("A", (PadReference("U1", "1"), PadReference("J1", "1"))),))
    result = run_routing_pipeline(
        board,
        placement_options=PlacementPlannerOptions(
            candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=frozenset({"U1", "J1"})),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(maximum_passes=1),
        detailed_feedback_trials=2,
    )
    assert result.duplicate_pad_stitch.added_track_count == 1
    assert result.detailed_feedback_trials == 0
    assert result.status.value == "pass"
    assert result.detailed.metrics.track_count == len(result.board.tracks)
    from math import hypot
    assert result.detailed.metrics.total_length_nm == sum(round(hypot(
        track.end.x_nm-track.start.x_nm, track.end.y_nm-track.start.y_nm))
        for track in result.board.tracks)
    assert result.drc == run_physical_drc(result.board)
    assert stitch_duplicate_pads(result.board).added_track_count == 0


def test_installed_kicad_agrees_on_single_logical_pin_land_closure() -> None:
    from pathlib import Path
    import shutil
    import pytest
    from pcbir.plane_verify import verify_filled_planes
    executable = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(executable).is_file():
        pytest.skip("KiCad CLI not installed")
    board = _board(blocking_middle=False)
    before = verify_filled_planes(board, kicad_cli=Path(executable))
    after = verify_filled_planes(stitch_duplicate_pads(board).board, kicad_cli=Path(executable))
    assert before.unconnected_count > 0
    assert after.unconnected_count == 0
    assert after.passed, after.findings

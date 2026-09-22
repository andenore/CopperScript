from __future__ import annotations

from dataclasses import replace
import json

from pcbir import (
    BoardOutline,
    DrcCompleteness,
    DrcDecision,
    DrcDisposition,
    DrcWaiver,
    FootprintPad,
    PadReference,
    PadShape,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    TrackSegment,
    CopperLayer,
    nm_from_mm,
    physical_board_digest,
    run_physical_drc,
    run_incremental_physical_drc,
)


def _routed_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "DrcBoard",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        tracks=(
            TrackSegment(
                "SIGNAL",
                Point.mm(3, 6),
                Point.mm(17, 6),
                nm_from_mm("0.25"),
                CopperLayer.FRONT,
            ),
        ),
        metadata={"detailed_routing": "complete"},
    )


def test_physical_drc_passes_and_binds_token_to_exact_geometry() -> None:
    board = _routed_board()

    first = run_physical_drc(board)
    second = run_physical_drc(board)

    assert first == second
    assert first.decision is DrcDecision.PASS
    assert first.completeness is DrcCompleteness.COMPLETE
    assert first.token.board_digest == physical_board_digest(board)
    assert len(first.token.token_digest) == 64
    assert json.loads(first.to_json())["schema"] == "copperscript-physical-drc/v0.1"


def test_physical_drc_fails_for_open_net_and_accepts_exact_waiver() -> None:
    board = replace(_routed_board(), tracks=())
    failed = run_physical_drc(board)

    assert failed.decision is DrcDecision.FAIL
    finding = next(item for item in failed.findings if item.code == "DRC-OPEN-NET")

    waived = run_physical_drc(
        board,
        (DrcWaiver(finding.fingerprint, "intentional test fixture", "reviewer"),),
    )

    assert waived.decision is DrcDecision.PASS_WITH_WAIVERS
    assert waived.findings[0].disposition is DrcDisposition.WAIVED


def test_physical_drc_detects_cross_net_short() -> None:
    footprint = _routed_board().footprints["test/one-pad"]
    board = PhysicalBoard(
        "Shorted",
        BoardOutline.rectangle(20, 20),
        {footprint.name: footprint},
        (
            Placement("A1", footprint.name, Point.mm(3, 10)),
            Placement("A2", footprint.name, Point.mm(17, 10)),
            Placement("B1", footprint.name, Point.mm(10, 3)),
            Placement("B2", footprint.name, Point.mm(10, 17)),
        ),
        (
            PhysicalNet("A", (PadReference("A1", "1"), PadReference("A2", "1"))),
            PhysicalNet("B", (PadReference("B1", "1"), PadReference("B2", "1"))),
        ),
        tracks=(
            TrackSegment("A", Point.mm(3, 10), Point.mm(17, 10), nm_from_mm("0.25"), CopperLayer.FRONT),
            TrackSegment("B", Point.mm(10, 3), Point.mm(10, 17), nm_from_mm("0.25"), CopperLayer.FRONT),
        ),
        metadata={"detailed_routing": "complete"},
    )

    report = run_physical_drc(board)

    assert report.decision is DrcDecision.FAIL
    assert any(item.code == "DRC-SHORT" for item in report.findings)


def test_incomplete_route_makes_coverage_incomplete_and_stale_tokens_change() -> None:
    board = _routed_board()
    incomplete = run_physical_drc(replace(board, metadata={}))
    changed = replace(
        board,
        tracks=(replace(board.tracks[0], width_nm=nm_from_mm("0.3")),),
    )

    assert incomplete.decision is DrcDecision.FAIL
    assert incomplete.completeness is DrcCompleteness.INCOMPLETE
    assert run_physical_drc(changed).token.board_digest != run_physical_drc(board).token.board_digest


def test_exact_rectangular_pad_geometry_avoids_bounding_circle_false_positive() -> None:
    footprint = PhysicalFootprint(
        "test/tall-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.5", "3"),
                      shape=PadShape.RECTANGLE),),
        Size.mm(1, 3),
    )
    board = PhysicalBoard(
        "ExactPads", BoardOutline.rectangle(12, 12), {footprint.name: footprint},
        (Placement("A", footprint.name, Point.mm(4, 6)),
         Placement("B", footprint.name, Point.mm(6, 6))),
        (PhysicalNet("A", (PadReference("A", "1"),)),
         PhysicalNet("B", (PadReference("B", "1"),))),
        metadata={"detailed_routing": "complete"},
    )
    report = run_physical_drc(board)
    assert not any(item.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for item in report.findings)


def test_incremental_contract_matches_full_and_concave_edge_crossing_is_found() -> None:
    board = _routed_board()
    assert run_incremental_physical_drc(board, ("track:0",)) == run_physical_drc(board)
    concave = BoardOutline((Point.mm(0, 0), Point.mm(10, 0), Point.mm(10, 10),
                            Point.mm(6, 10), Point.mm(6, 4), Point.mm(4, 4),
                            Point.mm(4, 10), Point.mm(0, 10)))
    net = PhysicalNet("N", ())
    crossing = PhysicalBoard("Concave", concave, {}, (), (net,),
                             tracks=(TrackSegment("N", Point.mm(2, 8), Point.mm(8, 8),
                                                  nm_from_mm("0.2"), CopperLayer.FRONT),),
                             metadata={"detailed_routing": "complete"})
    report = run_physical_drc(crossing)
    assert any(item.code == "DRC-BOARD-EDGE" for item in report.findings)

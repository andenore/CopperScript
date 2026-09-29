from __future__ import annotations

from dataclasses import replace

from pcbir import (
    BoardOutline,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    FootprintPad,
    PadReference,
    PlaneStitchOptions,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    PolygonRing,
    PolygonWithHoles,
    Size,
    Stackup,
    Via,
    nm_from_mm,
    run_physical_drc,
)
from pcbir.plane import stitch_zone_pads


def _plane_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    zone = CopperZone(
        "ground-plane", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(19, 1),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    return PhysicalBoard(
        "PlaneStitch", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("GND", (
            PadReference("J1", "1"), PadReference("J2", "1"),
        )),),
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        zones=(zone,),
    )


def test_stitches_surface_pads_without_claiming_plane_fill() -> None:
    board = _plane_board()
    result = stitch_zone_pads(board)

    assert result.complete
    assert len(result.stitched_pads) == 2
    assert result.added_track_count == 2
    assert result.added_via_count == 2
    assert result.board.metadata["fabrication_ready"] == "false"
    findings = {item.code for item in run_physical_drc(result.board).findings}
    assert "DRC-SHORT" not in findings
    assert "DRC-CLEARANCE" not in findings
    assert "DRC-OPEN-NET" in findings


def test_multiple_zones_on_same_net_do_not_duplicate_stitches() -> None:
    board = _plane_board()
    board = replace(board, zones=(*board.zones, replace(
        board.zones[0], id="second-plane",
    )))

    result = stitch_zone_pads(board)

    assert result.added_via_count == 2
    assert len(result.stitched_pads) == 2


def test_existing_same_net_via_is_reused_not_drilled_twice() -> None:
    board = _plane_board()
    first = stitch_zone_pads(board)
    seeded = replace(board, vias=(first.board.vias[0],))
    result = stitch_zone_pads(seeded)
    assert result.complete
    assert result.added_track_count == 2
    assert result.added_via_count == 1
    assert len(result.board.vias) == 2


def test_reuses_off_grid_same_net_via_before_adding_drill() -> None:
    base = _plane_board()
    existing = Via(
        "GND", Point.mm("4.1", "6.3"), nm_from_mm("0.8"),
        nm_from_mm("0.4"), CopperLayer.FRONT, CopperLayer.BACK,
    )
    board = replace(base, vias=(existing,))
    result = stitch_zone_pads(board)

    assert result.complete
    assert any(track.start == Point.mm(3, 6)
               and track.end == existing.position for track in result.board.tracks)
    assert len(result.board.vias) == 2  # Only J2 needs a new drill.
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING",
    }


def test_selected_early_escape_is_not_duplicated_by_late_pass() -> None:
    board = _plane_board()
    early = stitch_zone_pads(board, PlaneStitchOptions(
        only_pads=frozenset({PadReference("J1", "1")}),
    ))
    late = stitch_zone_pads(early.board)

    assert early.stitched_pads == (PadReference("J1", "1"),)
    assert early.added_via_count == 1
    assert late.complete
    assert late.added_track_count == 1
    assert late.added_via_count == 1


def test_blocked_stitches_remain_explicitly_pending() -> None:
    board = _plane_board()
    wall = CopperKeepout(
        "all-vias", tuple(board.stackup.copper_layers),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(19, 1),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    result = stitch_zone_pads(replace(board, copper_keepouts=(wall,)))

    assert not result.complete
    assert len(result.pending_pads) == 2
    assert not result.board.tracks
    assert not result.board.vias

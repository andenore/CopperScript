from __future__ import annotations

from dataclasses import replace
import pytest

from pcbir import (
    BoardOutline,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    DesignRules,
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


def test_existing_via_can_be_reached_with_legal_two_segment_escape() -> None:
    base = _plane_board()
    blocker = PhysicalFootprint(
        "test/blocker",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    existing = Via(
        "GND", Point.mm(5, 8), nm_from_mm("0.8"),
        nm_from_mm("0.4"), CopperLayer.FRONT, CopperLayer.BACK,
    )
    board = replace(
        base,
        footprints={**base.footprints, blocker.name: blocker},
        placements=(*base.placements, Placement("U1", blocker.name, Point.mm(4, 7))),
        nets=(*base.nets, PhysicalNet("SIGNAL", (PadReference("U1", "1"),))),
        vias=(existing,),
    )

    result = stitch_zone_pads(board)

    assert result.complete
    assert sum(existing.position in (track.start, track.end)
               for track in result.board.tracks) == 1
    assert any(Point.mm(3, 6) in (track.start, track.end)
               for track in result.board.tracks)
    assert len(result.board.tracks) == 3  # Two for J1, one for J2.
    assert len(result.board.vias) == 2
    repeated = stitch_zone_pads(result.board)
    assert repeated.added_track_count == 0
    assert repeated.added_via_count == 0
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE",
    }


def test_blocked_elbows_can_use_bounded_three_segment_escape() -> None:
    base = _plane_board()
    blocker = PhysicalFootprint(
        "test/three-blockers",
        (
            FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),
            FootprintPad("2", Point.mm(-1, 0), Size.mm("0.6", "0.6")),
            FootprintPad("3", Point.mm(0, -1), Size.mm("0.6", "0.6")),
        ),
        Size.mm(2, 2),
    )
    existing = Via(
        "GND", Point.mm(5, 8), nm_from_mm("0.8"),
        nm_from_mm("0.4"), CopperLayer.FRONT, CopperLayer.BACK,
    )
    board = replace(
        base,
        footprints={**base.footprints, blocker.name: blocker},
        placements=(*base.placements, Placement("U1", blocker.name, Point.mm(4, 7))),
        nets=(*base.nets, PhysicalNet("SIGNAL", tuple(
            PadReference("U1", number) for number in ("1", "2", "3")
        ))),
        vias=(existing,),
    )

    result = stitch_zone_pads(board, PlaneStitchOptions(
        maximum_detour_nm=nm_from_mm("1.5"),
    ))

    first_escape = [track for track in result.board.tracks
                    if track.net == "GND" and max(
                        track.start.x_nm, track.end.x_nm,
                    ) < nm_from_mm(10)]
    assert result.complete
    assert len(first_escape) == 3
    assert any(Point.mm(3, 6) in (track.start, track.end)
               for track in first_escape)
    assert any(existing.position in (track.start, track.end)
               for track in first_escape)
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE",
    }


def test_every_separate_land_of_one_logical_ground_pad_is_escaped() -> None:
    base = _plane_board()
    connector = PhysicalFootprint(
        "test/two-ground-lands",
        (
            FootprintPad("G", Point.mm(-2, 0), Size.mm("0.6", "0.6")),
            FootprintPad("S", Point(0, 0), Size.mm("0.6", "0.6")),
            FootprintPad("G", Point.mm(2, 0), Size.mm("0.6", "0.6")),
        ),
        Size.mm(6, 2),
    )
    board = replace(
        base,
        footprints={connector.name: connector},
        placements=(Placement("J1", connector.name, Point.mm(10, 6)),),
        nets=(
            PhysicalNet("GND", (PadReference("J1", "G"),)),
            PhysicalNet("SIGNAL", (PadReference("J1", "S"),)),
        ),
    )

    result = stitch_zone_pads(board)

    assert result.stitched_pads == (PadReference("J1", "G"),)
    assert result.pending_pads == ()
    assert any(Point.mm(8, 6) in (track.start, track.end)
               for track in result.board.tracks)
    assert any(Point.mm(12, 6) in (track.start, track.end)
               for track in result.board.tracks)
    assert len(result.board.vias) == 2
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE",
    }


def test_explicit_local_escape_width_preserves_clearance_checks() -> None:
    base = _plane_board()
    board = replace(base, rules=DesignRules(
        minimum_clearance_nm=nm_from_mm("0.09"),
        minimum_track_width_nm=nm_from_mm("0.09"),
        default_track_width_nm=nm_from_mm("0.2"),
    ))

    result = stitch_zone_pads(board, PlaneStitchOptions(
        escape_width_nm=nm_from_mm("0.12"),
    ))

    assert result.complete
    assert all(track.width_nm == nm_from_mm("0.12")
               for track in result.board.tracks)
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE",
    }


def test_local_escape_width_cannot_undercut_board_rule_floor() -> None:
    with pytest.raises(ValueError, match="below the board rule floor"):
        stitch_zone_pads(_plane_board(), PlaneStitchOptions(
            escape_width_nm=nm_from_mm("0.12"),
        ))


def test_blocked_land_can_reach_plane_through_escaped_neighbor() -> None:
    base = _plane_board()
    wall = CopperKeepout(
        "local-via-exclusion", tuple(base.stackup.copper_layers),
        PolygonWithHoles(PolygonRing((
            Point.mm(4, 1), Point.mm(10, 1),
            Point.mm(10, 11), Point.mm(4, 11),
        ))),
        block_tracks=False, block_zones=False,
    )
    board = replace(
        base,
        placements=(base.placements[0], replace(
            base.placements[1], position=Point.mm(7, 6),
        )),
        copper_keepouts=(wall,),
    )

    limited = stitch_zone_pads(board, PlaneStitchOptions(
        maximum_contact_radius_nm=nm_from_mm(3),
    ))
    rescued = stitch_zone_pads(board, PlaneStitchOptions(
        maximum_contact_radius_nm=nm_from_mm(5),
    ))

    assert limited.pending_pads == (PadReference("J2", "1"),)
    assert rescued.complete
    assert rescued.added_via_count == 1
    assert any({track.start, track.end} == {Point.mm(3, 6), Point.mm(7, 6)}
               for track in rescued.board.tracks)
    assert not {finding.code for finding in run_physical_drc(rescued.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING",
        "DRC-BOARD-EDGE", "DRC-COPPER-KEEPOUT",
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

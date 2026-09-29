from __future__ import annotations

from pcbir import (
    BoardOutline, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, Size, stitch_duplicate_pads,
)


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


def test_blocked_same_number_lands_remain_pending() -> None:
    board = _board(blocking_middle=True)
    result = stitch_duplicate_pads(board)

    assert result.stitched == ()
    assert result.pending == (PadReference("U1", "1"),)
    assert result.board == board

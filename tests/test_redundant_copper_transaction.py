from collections import Counter
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, Via, nm_from_mm)
from pcbir.route_cleanup import remove_redundant_ordinary_copper


def fixture():
    fp = PhysicalFootprint('one', (FootprintPad('1', Point(0, 0), Size.mm('.6', '.6')),), Size.mm(1, 1))
    tracks = (TrackSegment('X', Point.mm(3, 3), Point.mm(7, 3), nm_from_mm('.2'), CopperLayer.FRONT),
              TrackSegment('X', Point.mm(4, 3), Point.mm(4, 5), nm_from_mm('.2'), CopperLayer.FRONT))
    via = Via('X', Point.mm(4, 5), nm_from_mm('.6'), nm_from_mm('.3'), CopperLayer.FRONT, CopperLayer.BACK)
    return PhysicalBoard('redundant', BoardOutline.rectangle(10, 10), {fp.name: fp},
        (Placement('U', fp.name, Point.mm(3, 3)), Placement('J', fp.name, Point.mm(7, 3))),
        (PhysicalNet('X', (PadReference('U', '1'), PadReference('J', '1'))),), tracks=tracks, vias=(via,))


def test_redundant_branch_removed_without_changing_input():
    board = fixture()
    result = remove_redundant_ordinary_copper(board, (board.tracks[1],), board.vias,
        mutable_tracks=Counter(board.tracks), mutable_vias=Counter(board.vias))
    assert result is not None and result.tracks == (board.tracks[0],) and not result.vias
    assert len(board.tracks) == 2 and len(board.vias) == 1


def test_required_connection_is_retained_if_proof_fails():
    board = fixture()
    assert remove_redundant_ordinary_copper(board, (board.tracks[0],),
        mutable_tracks=Counter(board.tracks), mutable_vias=Counter()) is None


def test_cleanup_refuses_immutable_or_nonexistent_copper():
    board = fixture()
    with pytest.raises(ValueError, match='exact mutable'):
        remove_redundant_ordinary_copper(board, (board.tracks[1],),
            mutable_tracks=Counter(), mutable_vias=Counter())
    changed = replace(board.tracks[1], width_nm=nm_from_mm('.25'))
    with pytest.raises(ValueError, match='exact mutable'):
        remove_redundant_ordinary_copper(board, (changed,),
            mutable_tracks=Counter({changed: 1}), mutable_vias=Counter())

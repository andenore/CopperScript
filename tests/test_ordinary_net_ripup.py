from collections import Counter
from dataclasses import replace
import pytest
from pcbir import (BoardOutline, CopperLayer, NetRoutingRule, PhysicalBoard,
    PhysicalNet, Point, RouteKind, TrackSegment, Via, nm_from_mm,
    CopperZone, PolygonRing, PolygonWithHoles)
from pcbir.route_cleanup import ripup_mutable_nets


def fixture():
    tracks = tuple(TrackSegment(n, Point.mm(2, y), Point.mm(4, y),
        nm_from_mm('.2'), CopperLayer.FRONT) for n, y in [('X', 3), ('Y', 6)])
    via = Via('X', Point.mm(4, 3), nm_from_mm('.6'), nm_from_mm('.3'),
              CopperLayer.FRONT, CopperLayer.BACK)
    return PhysicalBoard('ripup', BoardOutline.rectangle(10, 10), {}, (),
        (PhysicalNet('X', ()), PhysicalNet('Y', ())), tracks=tracks, vias=(via,))


def test_ripup_only_explicit_mutable_net_and_retains_input():
    board = replace(fixture(), metadata={'detailed_routing': 'complete', 'fabrication_ready': 'true'})
    result = ripup_mutable_nets(board, frozenset({'X'}),
        mutable_tracks=Counter(board.tracks), mutable_vias=Counter(board.vias))
    assert result.tracks == (board.tracks[1],) and not result.vias
    assert len(board.tracks) == 2 and len(board.vias) == 1
    assert result.metadata['detailed_routing'] == 'partial'
    assert result.metadata['fabrication_ready'] == 'false'
    assert board.metadata['detailed_routing'] == 'complete'


@pytest.mark.parametrize('tracks,vias', [(False, True), (True, False)])
def test_ripup_rejects_any_immutable_object(tracks, vias):
    board = fixture()
    with pytest.raises(ValueError, match='immutable'):
        ripup_mutable_nets(board, frozenset({'X'}),
            mutable_tracks=Counter(board.tracks) if tracks else Counter(),
            mutable_vias=Counter(board.vias) if vias else Counter())


def test_ripup_rejects_unknown_and_critical_nets():
    board = fixture()
    with pytest.raises(ValueError, match='known'):
        ripup_mutable_nets(board, frozenset({'unknown'}), mutable_tracks=Counter(), mutable_vias=Counter())
    board = replace(board, net_routing_rules=(NetRoutingRule('X', RouteKind.CLOCK),))
    with pytest.raises(ValueError, match='ordinary'):
        ripup_mutable_nets(board, frozenset({'X'}),
            mutable_tracks=Counter(board.tracks), mutable_vias=Counter(board.vias))


def test_ripup_rejects_distribution_plane_contacts():
    board = fixture()
    board = replace(board, zones=(CopperZone('plane', 'X', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(board.outline.vertices))),))
    with pytest.raises(ValueError, match='non-plane'):
        ripup_mutable_nets(board, frozenset({'X'}),
            mutable_tracks=Counter(board.tracks), mutable_vias=Counter(board.vias))

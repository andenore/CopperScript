"""UNEXECUTED exact selection contracts: full source inventory stays present."""
from dataclasses import replace
import pytest
from pcbir.decoupling import route_decouplers
from pcbir.package_access import _verified_bypass_links
from pcbir.physical import PadReference, Point
from surface_bypass_fixture import build


def two_links():
    source,first,_=build();cap=source.placements[-1]
    other=replace(cap,reference="OTHER",position=Point.mm(6,3))
    second=replace(first,capacitor=PadReference(other.reference,"1"),return_pad=PadReference(other.reference,"2"))
    source=replace(source,placements=(*source.placements,other),decoupling_links=(first,second),
        nets=tuple(replace(n,pads=(*n.pads,second.capacitor)) if n.name==first.net else
                   replace(n,pads=(*n.pads,second.return_pad)) if n.name==first.return_net else n for n in source.nets))
    return source,first,second


def test_exact_link_filter_preserves_same_rail_sibling_inventory():
    source,first,second=two_links()
    result=route_decouplers(source,only_links=frozenset((first,)))
    assert not result.pending and result.board.decoupling_links is source.decoupling_links
    assert result.board.nets is source.nets and result.board.placements is source.placements
    assert first in _verified_bypass_links(result.board,{first.net})
    assert second not in _verified_bypass_links(result.board,{first.net})
    assert second.capacitor not in result.connected_pads
    assert route_decouplers(source)==route_decouplers(source,only_links=None)


def test_unselected_previously_verified_sibling_remains_fresh():
    source,first,second=two_links()
    held=route_decouplers(source,only_links=frozenset((first,)))
    before=held.board
    result=route_decouplers(before,only_links=frozenset((second,)))
    assert not result.pending
    assert {first,second} <= _verified_bypass_links(result.board,{first.net})
    assert all(a is b for a,b in zip(before.tracks,result.board.tracks))
    assert source.decoupling_links==result.board.decoupling_links


def test_empty_and_net_intersection_are_noop_with_full_source():
    source,first,_=two_links()
    for kwargs in ({"only_links":frozenset()},
                   {"only_links":frozenset((first,)),"only_nets":frozenset((first.return_net,))}):
        result=route_decouplers(source,**kwargs)
        assert result.board is source and not result.created_tracks and not result.pending


def test_unknown_or_mutable_exact_selection_rejected_without_source_change():
    source,first,_=two_links();before=repr(source)
    unknown=replace(first,capacitor=PadReference("UNKNOWN","1"),return_pad=PadReference("UNKNOWN","2"))
    with pytest.raises(ValueError,match="undeclared source association"):
        route_decouplers(source,only_links=frozenset((unknown,)))
    with pytest.raises(TypeError,match="frozenset"):
        route_decouplers(source,only_links={first})
    assert repr(source)==before

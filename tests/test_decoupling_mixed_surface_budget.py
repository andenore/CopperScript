"""Later local feeds cannot invalidate previously accepted complete owners."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling import route_decouplers
from pcbir.decoupling_paths import validate_crosslayer_witness
from pcbir.drc import physical_board_digest
from pcbir.fanout import route_fanout
from pcbir.package_access import _verified_bypass_links
from pcbir.physical import BoardSide, PadReference, Placement, Point
from test_decoupling_crosslayer_paths import fixture


def mixed(*,renamed=False,reversed_order=False):
    board,back=fixture(renamed=renamed)
    ref="FRONT_BRANCH" if renamed else "CF"
    front=replace(back,capacitor=PadReference(ref,"1"),return_pad=PadReference(ref,"2"))
    board=replace(board,
        placements=(*board.placements,Placement(ref,"cap",Point.mm(17,8),side=BoardSide.FRONT)),
        nets=tuple(replace(n,pads=(*n.pads,front.capacitor if n.name==back.net else front.return_pad))
                   for n in board.nets),
        decoupling_links=(front,back) if reversed_order else (back,front),
        net_routing_rules=tuple(replace(r,max_length_nm=10_000_000) if r.net==back.net else r
                                for r in board.net_routing_rules))
    return board,back,front


@pytest.mark.parametrize("renamed",[False,True])
@pytest.mark.parametrize("reversed_order",[False,True])
def test_mixed_surface_batch_rejects_only_later_budget_overflow(renamed,reversed_order):
    board,back,front=mixed(renamed=renamed,reversed_order=reversed_order)
    first,later=board.decoupling_links
    result=route_decouplers(board)
    assert len(result.pending)==1 and result.pending[0][0]==later
    assert "budget" in result.pending[0][1] or "candidates_exhausted" in result.pending[0][1]
    assert _verified_bypass_links(result.board,{back.net})=={first}
    assert first.capacitor in result.connected_pads and later.capacitor not in result.connected_pads
    if first==back:
        assert not any(t.net==back.net and Point.mm(16.5,8) in (t.start,t.end)
                       for t in result.created_tracks)
    assert all(validate_crosslayer_witness(result.board,w)=="" for w in result.supply_return_witnesses)
    assert all(w.board_digest==physical_board_digest(result.board) for w in result.supply_return_witnesses)
    fanout=route_fanout(board)
    assert len(fanout.decoupling_pending)==1
    assert verified_bypass_reservations(board,fanout)[2] is None
    assert board.tracks==board.vias==()


def test_existing_complete_source_occurrences_survive_later_legacy_rejection():
    board,back,front=mixed()
    original=route_decouplers(replace(board,decoupling_links=(back,))).board
    source=replace(original,decoupling_links=board.decoupling_links)
    result=route_decouplers(source)
    assert len(result.pending)==1 and result.pending[0][0]==front
    assert not result.created_tracks and not result.created_vias
    assert result.board is source
    assert Counter(source.tracks)==Counter(result.board.tracks)
    assert Counter(source.vias)==Counter(result.board.vias)
    assert _verified_bypass_links(result.board,{back.net})=={back}
    assert all(w.board_digest==physical_board_digest(source) for w in result.supply_return_witnesses)


def test_saved_crosslayer_witnesses_share_the_fresh_final_snapshot():
    board,first=fixture()
    other,second=fixture(renamed=True)
    board=replace(board,
        placements=(*board.placements,*(replace(p,position=Point(p.position.x_nm,p.position.y_nm+6_000_000))
                                       for p in other.placements)),
        nets=(*board.nets,*other.nets),decoupling_links=(first,second),
        net_routing_rules=(*board.net_routing_rules,*other.net_routing_rules))
    result=route_decouplers(board)
    assert not result.pending and len(result.supply_return_witnesses)==2
    digest=physical_board_digest(result.board)
    assert all(w.board_digest==digest and validate_crosslayer_witness(result.board,w)==""
               for w in result.supply_return_witnesses)


def test_no_associations_is_an_unchanged_typed_noop_without_index(monkeypatch):
    board,_=fixture();board=replace(board,decoupling_links=())
    def forbidden(*args,**kwargs):
        raise AssertionError("empty decoupling work must not build a clearance index")
    monkeypatch.setattr("pcbir.routing_clearance.RoutingClearanceIndex",forbidden)
    result=route_decouplers(board)
    assert result.board is board and not result.created_tracks and not result.created_vias
    assert not result.connected_pads and not result.pending and not result.supply_return_witnesses
    assert not result.accesses

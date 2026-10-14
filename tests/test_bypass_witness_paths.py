"""Source association witnesses return exact owned inputs and never stale paths."""
from dataclasses import replace
from pcbir.fanout import route_fanout
from pcbir.package_access import _verified_bypass_links
from test_bypass_owner_regeneration import bypass_fixture

def test_witness_ledger_returns_original_segments_and_clears_stale_associations():
    board, _, _, settings, names = bypass_fixture()
    net=names['B']
    reserved=route_fanout(board,settings,only_nets=frozenset({net}))
    ledger={}
    expected=_verified_bypass_links(reserved.board,{net})
    assert expected
    assert _verified_bypass_links(reserved.board,{net},witness_paths=ledger)==expected
    assert set(ledger)==set(expected)
    for path in ledger.values():
        assert path and all(any(track is original for original in reserved.board.tracks) for track in path)
    empty=replace(reserved.board,tracks=board.tracks,vias=board.vias)
    assert not _verified_bypass_links(empty,{net},witness_paths=ledger)
    assert ledger=={}
    assert reserved.board.tracks==(*board.tracks,*reserved.created_tracks)

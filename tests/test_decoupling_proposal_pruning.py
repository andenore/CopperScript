"""A proposal may discard its own unused branches, never pre-existing copper."""
from dataclasses import replace
import pytest
import pcbir.decoupling_paths as paths
from pcbir.physical import TrackSegment, Via, Point, CopperLayer
from test_decoupling_crosslayer_paths import fixture

@pytest.mark.parametrize("renamed", [False, True])
def test_unused_proposal_suffix_is_pruned_and_source_objects_survive(monkeypatch, renamed):
    board,link=fixture(renamed=renamed)
    old=TrackSegment(link.net,Point.mm(18,15),Point.mm(19,15),150000,CopperLayer.FRONT)
    original_via=Via(link.net,Point.mm(19,15),450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
    board=replace(board,tracks=(old,),vias=(original_via,),
                  net_routing_rules=tuple(replace(r,max_vias=8) for r in board.net_routing_rules))
    branch=TrackSegment(link.net,Point.mm(19,15),Point.mm(20,15),150000,CopperLayer.FRONT)
    extra=Via(link.net,Point.mm(20,15),450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
    original=paths._connections
    def candidates(current,first,second,net,*args,**kwargs):
        for candidate in original(current,first,second,net,*args,**kwargs):
            if net==link.return_net:
                candidate=replace(candidate,tracks=(*candidate.tracks,branch),vias=(*candidate.vias,extra))
            yield candidate
    monkeypatch.setattr(paths,"_connections",candidates)
    result=paths.propose_crosslayer_decoupling(board,link)
    assert result.witness is not None,result.reason
    assert paths.validate_crosslayer_witness(result.board,result.witness)==""
    assert result.board.tracks[0] is old and result.board.vias[0] is original_via
    assert branch not in result.board.tracks and extra not in result.board.vias
    assert old not in result.created_tracks and original_via not in result.created_vias
    used={id(step.edge) for path in (result.supply_path,result.return_path) for step in path.steps}
    assert all(id(edge) in used for edge in (*result.created_tracks,*result.created_vias))
    assert result.board.placements is board.placements

def test_failed_return_does_not_adopt_or_prune_any_source_geometry():
    board,link=fixture()
    board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0) if r.net==link.return_net else r
                                              for r in board.net_routing_rules))
    result=paths.propose_crosslayer_decoupling(board,link)
    assert result.board is board and not result.witness
    assert not result.created_tracks and not result.created_vias

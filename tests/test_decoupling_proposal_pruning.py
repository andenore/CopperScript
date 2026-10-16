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


@pytest.mark.parametrize("blocked_half", ("supply", "return"))
def test_incomplete_proposal_needs_no_whole_board_drc(monkeypatch, blocked_half):
    board, link = fixture()
    blocked = link.net if blocked_half == "supply" else link.return_net
    board = replace(board, net_routing_rules=tuple(
        replace(rule, max_vias=0) if rule.net == blocked else rule
        for rule in board.net_routing_rules))
    calls = []
    original = paths.run_physical_drc
    def check(candidate):
        calls.append(candidate)
        return original(candidate)
    monkeypatch.setattr(paths, "run_physical_drc", check)
    result = paths.propose_crosslayer_decoupling(board, link)
    assert result.board is board and result.witness is None
    assert not result.created_tracks and not result.created_vias
    assert calls == []


def test_complete_proposal_keeps_fresh_baseline_and_candidate_drc(monkeypatch):
    board, link = fixture()
    calls = []
    original = paths.run_physical_drc
    def check(candidate):
        calls.append(candidate)
        return original(candidate)
    monkeypatch.setattr(paths, "run_physical_drc", check)
    result = paths.propose_crosslayer_decoupling(board, link)
    assert result.witness is not None
    assert len(calls) >= 2 and calls[0] is board and calls[-1] is result.board
    assert sum(candidate is board for candidate in calls) == 1


@pytest.mark.parametrize("reverse_edge", (False, True))
def test_only_used_span_of_new_witness_track_is_adopted(monkeypatch, reverse_edge):
    board, link = fixture()
    original = paths._connections
    extended = []
    def candidates(current, first, second, net, *args, **kwargs):
        for candidate in original(current, first, second, net, *args, **kwargs):
            if net == link.return_net:
                endpoint = paths._land(candidate, link.return_pad)[2].position
                target = next(t for t in candidate.tracks[len(current.tracks):]
                              if t.end == endpoint and t.layer is CopperLayer.BACK)
                dx = (target.end.x_nm > target.start.x_nm)-(target.end.x_nm < target.start.x_nm)
                dy = (target.end.y_nm > target.start.y_nm)-(target.end.y_nm < target.start.y_nm)
                beyond = Point(endpoint.x_nm+500000*dx, endpoint.y_nm+500000*dy)
                extra = replace(target, end=beyond)
                if reverse_edge:
                    extra = replace(extra, start=extra.end, end=extra.start)
                extended.append(extra)
                candidate = replace(candidate, tracks=tuple(extra if t is target else t
                                                             for t in candidate.tracks))
            yield candidate
    monkeypatch.setattr(paths, "_connections", candidates)
    result = paths.propose_crosslayer_decoupling(board, link)
    assert result.witness is not None, result.reason
    assert extended and all(t not in result.board.tracks for t in extended)
    for track in result.created_tracks:
        points = {node.position for path in (result.supply_path, result.return_path)
                  for step in path.steps if step.edge is track for node in (step.start, step.end)}
        assert track.start in points and track.end in points
    assert paths.validate_crosslayer_witness(result.board, result.witness) == ""
    assert result.board.placements is board.placements

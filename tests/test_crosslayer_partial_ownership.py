"""Actual six-layer bypass ownership in bounded ordinary repair."""
from collections import Counter
from dataclasses import replace
import pytest
from pcbir.fanout import route_fanout, FanoutOptions
from pcbir.package_access import _partial_ordinary_pattern, _verified_bypass_links
from pcbir.physical import Point, TrackSegment, CopperLayer, PhysicalNet
from test_decoupling_crosslayer_paths import fixture

def completed(renamed=False):
    source,link=fixture(renamed=renamed)
    settings=FanoutOptions()
    result=route_fanout(source,settings)
    assert result.decoupling_vias and not result.decoupling_pending
    assert _verified_bypass_links(result.board,{link.net})==frozenset({link})
    return source,link,settings,result

@pytest.mark.parametrize("renamed",[False,True])
def test_unselected_supply_and_return_generated_occurrences_are_retained(renamed):
    source,link,settings,original=completed(renamed)
    proposed,displaced,tracks,vias,reason=_partial_ordinary_pattern(original,source,source,settings,8)
    assert reason=="proposed" and not displaced
    assert proposed.decoupling_tracks==original.decoupling_tracks
    assert proposed.decoupling_vias==original.decoupling_vias
    assert Counter(original.created_tracks)==Counter(proposed.created_tracks)
    assert Counter(original.created_vias)==Counter(proposed.created_vias)
    assert tracks==len(original.created_tracks) and vias==len(original.created_vias)
    assert _verified_bypass_links(proposed.board,{link.net})==frozenset({link})

@pytest.mark.parametrize("regenerate",[False,True])
def test_return_via_conflict_cannot_release_unselected_supply_cohort(regenerate):
    source,link,settings,original=completed()
    via=next(v for v in original.decoupling_vias if v.net==link.return_net)
    obstacle=replace(source,nets=(*source.nets,PhysicalNet("FOREIGN",())),tracks=(TrackSegment("FOREIGN",Point(via.position.x_nm-100000,via.position.y_nm),
        Point(via.position.x_nm+100000,via.position.y_nm),100000,CopperLayer.FRONT),))
    proposed,displaced,_,_,reason=_partial_ordinary_pattern(original,source,obstacle,settings,8,regenerate)
    assert proposed is None and reason=="protected_ordinary_owner"
    assert link.return_net in displaced and link.net not in displaced
    assert _verified_bypass_links(original.board,{link.net})==frozenset({link})

def test_forged_semantic_via_ownership_is_rejected_before_trial():
    source,link,settings,original=completed()
    forged=replace(original,decoupling_vias=(*original.decoupling_vias,original.decoupling_vias[0]))
    with pytest.raises(ValueError,match="semantic feed ownership is stale"):
        _partial_ordinary_pattern(forged,source,source,settings,8)

@pytest.mark.parametrize("renamed",[False,True])
def test_supply_via_can_be_regenerated_without_releasing_return(renamed):
    source,link,settings,original=completed(renamed)
    via=next(v for v in original.decoupling_vias if v.net==link.net)
    obstacle=replace(source,nets=(*source.nets,PhysicalNet("FOREIGN",())),
        tracks=(TrackSegment("FOREIGN",Point(via.position.x_nm-100000,via.position.y_nm),
          Point(via.position.x_nm+100000,via.position.y_nm),100000,CopperLayer.FRONT),))
    proposed,displaced,_,_,reason=_partial_ordinary_pattern(original,source,obstacle,settings,1,True)
    assert reason=="proposed" and proposed is not None
    assert displaced==(link.net,)
    assert Counter(v for v in original.decoupling_vias if v.net==link.return_net) <= Counter(proposed.decoupling_vias)
    assert Counter(t for t in original.decoupling_tracks if t.net==link.return_net) <= Counter(proposed.decoupling_tracks)
    assert via not in proposed.created_vias
    assert _verified_bypass_links(proposed.board,{link.net})==frozenset({link})

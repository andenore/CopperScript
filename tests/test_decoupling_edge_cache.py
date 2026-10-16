"""Edge observations cannot survive changed copper, index state or rules."""
from dataclasses import replace
import pytest
from pcbir import decoupling_paths as paths
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.physical import TrackSegment,Point
from test_decoupling_crosslayer_paths import fixture

def completed():
    b,l=fixture()
    r=paths.propose_crosslayer_decoupling(b,l)
    assert r.witness is not None
    return r.board,l

def test_repeated_observation_preserves_original_edges_but_new_board_is_fresh(monkeypatch):
    b,l=completed();index=RoutingClearanceIndex(b);paths._cached_path_edges.cache_clear()
    original=paths._track_ok;calls=[]
    def count(*a):calls.append(a[2]);return original(*a)
    monkeypatch.setattr(paths,'_track_ok',count)
    first=paths.verify_crosslayer_decoupling(b,l,index=index)
    assert first.witness is not None
    n=len(calls)
    again=paths.verify_crosslayer_decoupling(b,l,index=index)
    assert again.witness==first.witness and len(calls)==n
    clone=replace(b)
    assert paths.verify_crosslayer_decoupling(clone,l).witness is not None
    assert len(calls)>n
    assert all(any(t is original for original in b.tracks) for t in again.supply_path.tracks)

def test_appended_foreign_index_obstacle_invalidates_cached_path():
    b,l=completed();index=RoutingClearanceIndex(b)
    before=paths.verify_crosslayer_decoupling(b,l,index=index);assert before.witness is not None
    t=before.supply_path.tracks[0]
    # An obstacle over the exact source feed invalidates its next observation.
    index.add_track(replace(t,net='FOREIGN'),locked=True)
    after=paths.verify_crosslayer_decoupling(b,l,index=index)
    assert after.witness is None
    assert paths.validate_crosslayer_witness(b,before.witness,index=index)

def test_exact_rule_change_invalidates_observed_edges():
    b,l=completed();index=RoutingClearanceIndex(b)
    first=paths._path_edges(b,l.net,index)
    assert first[0]
    # Clearance is read from the index; its changed rule must not reuse a hit.
    old=paths._cached_path_edges.cache_info().misses
    index.rules[l.net]=replace(index.rules[l.net],clearance_nm=9000000)
    paths._path_edges(b,l.net,index)
    assert paths._cached_path_edges.cache_info().misses==old+1

def test_cache_is_bounded():
    b,l=completed();paths._cached_path_edges.cache_clear()
    for _ in range(40):paths._path_edges(b,l.net,RoutingClearanceIndex(b))
    assert paths._cached_path_edges.cache_info().currsize==32

"""Finite-width held contacts preserve exact candidate and source occurrences."""
from dataclasses import replace
import pytest
from pcbir.copper_connectivity import CopperContact, copper_contacts_overlap
from pcbir.drc import _explicit_copper_contacts
from pcbir.geometry import RoundedConvexShape, segment_distance_squared
from pcbir.physical import CopperLayer, Point, TrackSegment
from pcbir.critical import _tune_pair
from pcbir.pair_tuning_spans import tune_virtual_pair_spans
from test_pair_tuning_spans import fixture, fragment, assert_unchanged, SKEW, AMP, F


def target_contact(i,t):
    return CopperContact(f"candidate:{i}",t.net,(t.layer,),
                        RoundedConvexShape((t.start,t.end),t.width_nm//2))


@pytest.mark.parametrize("shape",["parallel","endpoint_disks"])
@pytest.mark.parametrize("source_owned",[False,True])
def test_finite_width_held_contacts_lock_exact_candidate_occurrences(shape,source_owned):
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    if shape=="parallel":
        # Offset centrelines never intersect, but finite same-net copper joins
        # every shorter-lane fragment. Duplicate held occurrences stay explicit.
        rail=TrackSegment("A",Point.mm(5,4.95),Point.mm(15,4.95),100000,F)
        edges=(rail,rail)
    else:
        # Each held stub starts beyond a fragment end; endpoint disks contact
        # without collinear overlap or a centreline intersection.
        edges=tuple(TrackSegment("A",Point(t.end.x_nm+10000,t.end.y_nm-50000),
                        Point(t.end.x_nm+20000,t.end.y_nm-50000),100000,F)
                    for t in parts[0])
    source=replace(board,tracks=edges) if source_owned else board
    held=source if source_owned else replace(board,tracks=edges)
    contacts,_,_=_explicit_copper_contacts(held,only_nets=frozenset({"A"}),
                                          include_internal_connections=False)
    track_contacts=[c for c in contacts if c.identity.startswith("track:")]
    assert {c.identity for c in track_contacts}=={f"track:{i}" for i in range(len(edges))}
    # This is the exact gap in the original draft's centreline-only guard.
    for i,t in enumerate(parts[0]):
        assert all(segment_distance_squared(t.start,t.end,h.start,h.end)>0 for h in edges)
        assert any(copper_contacts_overlap(target_contact(i,t),c) for c in track_contacts)
    a,b=map(list,parts)
    assert _tune_pair(list(a),list(b),SKEW,AMP,32)==0
    assert tune_virtual_pair_spans(a,b,SKEW,AMP,32,source=source,held=held,
                                    tuner=_tune_pair)==0
    assert_unchanged(parts[0],a);assert_unchanged(parts[1],b)
    assert len(held.tracks)==len(edges)
    assert all(a is b for a,b in zip(held.tracks,edges))
    assert source.placements==board.placements and source.net_routing_rules==board.net_routing_rules


def test_identical_back_copper_is_not_a_front_contact():
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    rail=TrackSegment("A",Point.mm(5,4.95),Point.mm(15,4.95),100000,CopperLayer.BACK)
    held=replace(board,tracks=(rail,))
    contacts,_,_=_explicit_copper_contacts(held,only_nets=frozenset({"A"}),
                                          include_internal_connections=False)
    assert not any(copper_contacts_overlap(target_contact(i,t),c)
                   for i,t in enumerate(parts[0]) for c in contacts if c.identity.startswith("track:"))
    a,b=map(list,parts)
    assert _tune_pair(list(a),list(b),SKEW,AMP,32)==0
    assert tune_virtual_pair_spans(a,b,SKEW,AMP,32,source=board,held=held,
                                    tuner=_tune_pair)>0
    assert held.tracks==(rail,) and held.tracks[0] is rail


def test_pad_contact_ids_are_actual_footprint_indices():
    board,_=fixture()
    contacts,pads,_=_explicit_copper_contacts(board,only_nets=frozenset({"A"}),
                                             include_internal_connections=False)
    assert {c.identity for c in contacts}=={"pad:J1.1:0","pad:J2.1:0"}
    assert {identity for nodes in pads.values() for identity in nodes}=={
        "pad:J1.1:0","pad:J2.1:0"}

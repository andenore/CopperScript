"""Disposable routing metadata cannot become a new bypass source context."""
from dataclasses import replace

import pytest

import pcbir.package_access as access
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling_paths import validate_crosslayer_witness
from pcbir.fanout import FanoutOptions, FanoutResult, route_fanout
from pcbir.physical import CopperLayer, PhysicalNet, Point, TrackSegment
from test_crosslayer_partial_ownership import completed


@pytest.mark.parametrize('metadata',(
    {'critical_routing':'complete','detailed_routing':'partial','global_routing_fingerprint':'probe'},
    {'plane_stitching':'pad-escapes-only','fabrication_ready':'false'},
    {'critical_routing':'complete','plane_stitching':'partial','via_in_pad_count':'0'},
))
def test_metadata_only_probe_generates_fresh_witness_under_original_source(monkeypatch,metadata):
    source,link,settings,_=completed()
    source=replace(source,metadata={'source_note':'immutable author intent'})
    original=route_fanout(source,settings)
    via=next(v for v in original.decoupling_vias if v.net==link.net)
    obstacle=TrackSegment('FOREIGN',Point(via.position.x_nm-100000,via.position.y_nm),
                          Point(via.position.x_nm+100000,via.position.y_nm),100000,CopperLayer.FRONT)
    probe=replace(source,tracks=(*source.tracks,obstacle),metadata={**source.metadata,**metadata})
    # The validator remains strict; we never rebind this stale context.
    assert validate_crosslayer_witness(probe,original.decoupling_witnesses[0])=='stale_witness_context'
    real=access.route_fanout
    trials=[]
    def inspect(frame,*args,**kwargs):
        assert frame.metadata==source.metadata and obstacle in frame.tracks
        trials.append(frame)
        return real(frame,*args,**kwargs)
    monkeypatch.setattr(access,'route_fanout',inspect)
    before=original.board
    proposed,displaced,_,_,reason=access._partial_ordinary_pattern(original,source,probe,settings,1,True)
    assert reason=='proposed' and proposed is not None and len(trials)==1
    assert displaced==(link.net,)
    assert proposed.board.metadata==source.metadata and obstacle not in proposed.board.tracks
    assert all(validate_crosslayer_witness(proposed.board,w)=='' for w in proposed.decoupling_witnesses)
    assert verified_bypass_reservations(source,proposed)[2] is None
    assert original.board is before and probe.metadata!={**source.metadata}


@pytest.mark.parametrize('fault',('net','rule','pose','name'))
def test_changed_nonmetadata_source_intent_rolls_back_before_generation(monkeypatch,fault):
    source,_,settings,original=completed()
    if fault=='net':probe=replace(source,nets=(*source.nets,PhysicalNet('UNDECLARED_SOURCE',())))
    elif fault=='rule':probe=replace(source,rules=replace(source.rules,minimum_clearance_nm=110000))
    elif fault=='pose':probe=replace(source,placements=(replace(source.placements[0],position=Point.mm(7,8)),*source.placements[1:]))
    else:probe=replace(source,name='changed source identity')
    def forbidden(*args,**kwargs):raise AssertionError('malformed source intent must not route')
    monkeypatch.setattr(access,'route_fanout',forbidden)
    before=original.board
    answer=access._partial_ordinary_pattern(original,source,probe,settings,1,True)
    assert answer[0] is None and answer[-1]=='stale_probe_source_intent'
    assert original.board is before


def test_normalization_retains_actual_disposable_copper_and_fill_geometry():
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles,ZoneFillResult
    source,link,_,_=completed()
    polygon=PolygonWithHoles(PolygonRing((Point.mm(20,14),Point.mm(22,14),Point.mm(22,16),Point.mm(20,16))))
    source=replace(source,zones=(CopperZone('fixture-zone',link.return_net,(CopperLayer.INTERNAL_1,),polygon),),
                   metadata={'source_note':'owner'})
    track=TrackSegment('FOREIGN',Point.mm(21,3),Point.mm(22,3),100000,CopperLayer.FRONT)
    # Synthetic fill geometry is an obstacle, never a connectivity witness.
    fill=ZoneFillResult('fixture-zone',CopperLayer.INTERNAL_1,'synthetic-input','fixture','1',(polygon,))
    probe=replace(source,tracks=(*source.tracks,track),zone_fills=(fill,),metadata={'plane_stitching':'partial'})
    normalized=access._ordinary_probe_frame(source,probe)
    assert normalized is not None and normalized.metadata==source.metadata
    assert normalized.tracks==probe.tracks and normalized.vias==probe.vias and normalized.zone_fills==probe.zone_fills
    assert normalized.tracks[-1] is track and normalized.zone_fills[0] is fill


@pytest.mark.parametrize('fault',('missing_via','duplicate_via','missing_track','reordered_vias'))
def test_source_macro_array_occurrences_are_required_in_disposable_prefix(tmp_path,monkeypatch,fault):
    from test_autonomous_plane_access import constrained_contact_board,with_test_owners
    from pcbir.hard_macros import materialize_hard_macros,validate_hard_macros
    from pcbir.pad_via_arrays import require_via_in_pad_arrays
    source=materialize_hard_macros(with_test_owners(constrained_contact_board(),tmp_path,required_array=True))
    assert source.tracks and len(source.vias)==5
    original=FanoutResult(source,{},(),0,0)
    fresh=access._ordinary_probe_frame(source,replace(source,metadata={**source.metadata,'critical_routing':'complete'}))
    assert fresh.tracks==source.tracks and fresh.vias==source.vias and fresh.metadata==source.metadata
    assert all(a is b for a,b in zip(fresh.tracks,source.tracks))
    assert all(a is b for a,b in zip(fresh.vias,source.vias))
    if fault=='missing_via':probe=replace(source,vias=source.vias[:-1])
    elif fault=='duplicate_via':probe=replace(source,vias=(source.vias[0],source.vias[1],source.vias[1],*source.vias[3:]))
    elif fault=='missing_track':
        # Immutable macro removal is already rejected at construction, before
        # a disposable board can reach the partial owner gate.
        with pytest.raises(ValueError,match='immutable hard-macro copper'):
            replace(source,tracks=source.tracks[1:])
        validate_hard_macros(source)
        require_via_in_pad_arrays(source,'rejected malformed macro')
        return
    else:probe=replace(source,vias=tuple(reversed(source.vias)))
    def forbidden(*args,**kwargs):raise AssertionError('immutable prefix cannot be repaired away')
    monkeypatch.setattr(access,'route_fanout',forbidden)
    answer=access._partial_ordinary_pattern(original,source,probe,FanoutOptions(),8,True)
    assert answer[0] is None and answer[-1]=='stale_probe_source_intent'
    assert original.board is source
    validate_hard_macros(source)
    require_via_in_pad_arrays(source,'rejected malformed probe')

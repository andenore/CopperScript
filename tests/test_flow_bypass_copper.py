"""Real bypass reservations retain net-aware geometry and decoded ownership."""
from collections import Counter
from dataclasses import replace
import pytest

from pcbir import CopperLayer, Point, route_global
from pcbir.flow import _bypass_copper_reservations, _copper_keepouts
from pcbir.decoupling import route_decouplers
from pcbir.decoupling_paths import validate_crosslayer_witness
from pcbir.bypass_reservations import restore_bypass_reservation_edges, verified_bypass_reservations
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.workbench import save_state, load_state, scene_of
from pcbir.critical import replay_critical
from test_decoupling_crosslayer_paths import fixture


@pytest.mark.parametrize('renamed', (False, True))
def test_actual_bypass_return_allows_same_net_join_but_never_foreign_copper(renamed):
    source,link=fixture(renamed=renamed)
    planned=route_decouplers(source)
    held,reason=_bypass_copper_reservations(source,planned.created_tracks,planned.created_vias,
                                           planned.admitted_path_witnesses)
    assert held is not None and reason is None
    assert verified_bypass_reservations(source,held)==(held.decoupling_tracks,held.decoupling_vias,None)
    assert Counter(held.created_tracks)==Counter(planned.created_tracks)
    assert Counter(held.created_vias)==Counter(planned.created_vias)
    assert all(validate_crosslayer_witness(held.board,w)=='' for w in held.decoupling_witnesses)
    via=next(v for v in held.created_vias if v.net==link.return_net)
    a,b=via.position,Point(via.position.x_nm+100000,via.position.y_nm)
    actual=RoutingClearanceIndex(held.board)
    assert actual.can_track(link.return_net,a,b,100000,CopperLayer.FRONT)
    assert not actual.can_track(link.net,a,b,100000,CopperLayer.FRONT)
    proxy=replace(source,copper_keepouts=(*source.copper_keepouts,
        *_copper_keepouts(source,held.created_tracks,held.created_vias,'planned')))
    assert not RoutingClearanceIndex(proxy).can_track(link.return_net,a,b,100000,CopperLayer.FRONT)
    assert source.tracks==source.vias==()


def test_bypass_checkpoint_replays_only_after_unique_edge_restore_and_fresh_admission(tmp_path):
    source,_=fixture()
    planned=route_decouplers(source)
    held,reason=_bypass_copper_reservations(source,planned.created_tracks,planned.created_vias,
                                           planned.admitted_path_witnesses)
    assert held is not None,reason
    path=save_state(tmp_path/'inputs.state.json',{'board':source,'global_route':route_global(source),
                                               'reserved_accesses':held})
    decoded=load_state(path)['reserved_accesses']
    restored=restore_bypass_reservation_edges(decoded)
    assert restored==held
    assert verified_bypass_reservations(source,restored)[2] is None
    result=replay_critical(path)
    assert Counter(result.board.tracks)>=Counter(held.board.tracks)
    assert Counter(result.board.vias)>=Counter(held.board.vias)
    assert len(scene_of(path).vias)==len(held.board.vias)
    for changed in (replace(decoded,created_tracks=()),
                    replace(decoded,board=replace(decoded.board,tracks=(*decoded.board.tracks,decoded.created_tracks[0])))):
        with pytest.raises(ValueError):
            restore_bypass_reservation_edges(changed)


def test_unwitnessed_or_stale_bypass_proposal_is_not_filtered_into_acceptance():
    source,_=fixture()
    planned=route_decouplers(source)
    assert _bypass_copper_reservations(source,planned.created_tracks,(),planned.admitted_path_witnesses)[0] is None
    changed=replace(source,rules=replace(source.rules,minimum_clearance_nm=110000))
    assert _bypass_copper_reservations(changed,planned.created_tracks,planned.created_vias,
                                      planned.admitted_path_witnesses)[0] is None


def test_ordinary_copper_binding_preserves_whole_plan_and_rejects_invalid_trial(tmp_path):
    from pcbir.flow import bind_ordinary_escape_copper
    from test_unified_router import _board
    from pcbir import TrackSegment, Via, nm_from_mm
    source = _board({"A": ((3, 10), (17, 10)), "B": ((10, 3), (10, 17))})
    track = TrackSegment("A", Point.mm(6,13), Point.mm(7,13), nm_from_mm("0.2"), CopperLayer.FRONT)
    via = Via("A", Point.mm(7,13), nm_from_mm("0.6"), nm_from_mm("0.3"), CopperLayer.FRONT, CopperLayer.BACK)
    masks = tuple(_copper_keepouts(source, (track,), (via,), "escape-reserve"))
    masked = replace(source, copper_keepouts=masks)
    clean, held, reason = bind_ordinary_escape_copper(masked, None, ((track,), (via,), masks))
    assert reason is None and clean == source
    assert held.created_tracks == (track,) and held.created_vias == (via,)
    assert held.board.tracks == (track,) and held.board.vias == (via,)
    path = save_state(tmp_path/'ordinary-prefix.state.json',
        {'board':clean, 'global_route':route_global(clean), 'reserved_accesses':held})
    replay = replay_critical(path)
    assert Counter(held.board.tracks) <= Counter(replay.board.tracks)
    assert Counter(held.board.vias) <= Counter(replay.board.vias)
    # A crossing is rejected atomically, with no accepted subset or mask loss.
    crossing = TrackSegment("B", Point.mm(6.5,12), Point.mm(6.5,14), nm_from_mm("0.2"), CopperLayer.FRONT)
    rejected, prior, reason = bind_ordinary_escape_copper(masked, None, ((track,crossing), (via,), masks))
    assert reason and rejected is masked and prior is None
    assert source.tracks == source.vias == () and masked.copper_keepouts == masks


def test_mixed_escape_prefix_keeps_fresh_bypass_witnesses_after_checkpoint(tmp_path):
    from pcbir.flow import bind_ordinary_escape_copper
    from pcbir import PhysicalFootprint, FootprintPad, Size, Placement, PhysicalNet, PadReference, TrackSegment
    source, link = fixture()
    fp = PhysicalFootprint('signal', (FootprintPad('1',Point(0,0),Size.mm(.4,.4)),),Size.mm(1,1))
    source = replace(source, footprints={**source.footprints, fp.name:fp},
        placements=(*source.placements,Placement('J1',fp.name,Point.mm(18,4)),Placement('J2',fp.name,Point.mm(20,4))),
        nets=(*source.nets,PhysicalNet('DATA',(PadReference('J1','1'),PadReference('J2','1')))))
    edge = TrackSegment('DATA',Point.mm(18,4),Point.mm(19,4),150000,CopperLayer.FRONT)
    masks = tuple(_copper_keepouts(source,(edge,),(),'escape-reserve'))
    masked = replace(source,copper_keepouts=masks)
    bypass = route_decouplers(masked)
    held,reason = _bypass_copper_reservations(masked,bypass.created_tracks,bypass.created_vias,
        bypass.admitted_path_witnesses)
    assert reason is None
    clean,mixed,reason = bind_ordinary_escape_copper(masked,held,((edge,),(),masks))
    assert reason is None
    assert verified_bypass_reservations(clean,mixed)[2] is None
    assert Counter(held.created_tracks) <= Counter(mixed.created_tracks)
    assert Counter(held.created_vias) == Counter(mixed.created_vias)
    path = save_state(tmp_path/'mixed.state.json',
        {'board':clean,'global_route':route_global(clean),'reserved_accesses':mixed})
    replay = replay_critical(path)
    assert Counter(mixed.board.tracks) <= Counter(replay.board.tracks)
    assert Counter(mixed.board.vias) <= Counter(replay.board.vias)


@pytest.mark.parametrize("required_array", (False, True))
def test_standard_pipeline_keeps_whole_bypass_prefix_and_emits_replayable_input(tmp_path, required_array):
    from pcbir.flow import run_routing_pipeline
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    from pcbir.unified_router import UnifiedRouterOptions
    from pcbir.decoupling_paths import verify_crosslayer_decoupling
    if required_array:
        from test_decoupling_source_arrays import array_fixture
        source,link=array_fixture(3)
    else:
        source,link=fixture()
    stages,events={},[]
    result=run_routing_pipeline(source,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
            fixed_references=tuple(p.reference for p in source.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(time_budget_s=5,maximum_iterations=5),
        on_stage=lambda name,value:stages.update({name:value}),
        on_progress=lambda phase,event,details:events.append((phase,event,details)))
    assert any(phase=='bypass_reservation' and details.get('mode')=='copper' for phase,_,details in events)
    held=stages['critical-inputs']['reserved_accesses']
    assert held is not None
    assert Counter(held.board.tracks)<=Counter(result.board.tracks)
    assert Counter(held.board.vias)<=Counter(result.board.vias)
    assert verify_crosslayer_decoupling(result.board,link).witness is not None
    path=save_state(tmp_path/'critical-inputs.state.json',stages['critical-inputs'])
    replay=replay_critical(path)
    assert Counter(held.board.tracks)<=Counter(replay.board.tracks)


def test_standard_critical_stage_honors_its_existing_pair_state_bound():
    from pcbir.flow import run_routing_pipeline
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    from pcbir.package_access import PackageAccessOptions
    from pcbir.unified_router import UnifiedRouterOptions
    from test_local_pair_branches import support_fixture
    source=support_fixture()
    result=run_routing_pipeline(source,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0,
            fixed_references=tuple(p.reference for p in source.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        package_access_options=PackageAccessOptions(initial_pair_state_limit=1),
        unified_options=UnifiedRouterOptions(time_budget_s=2,maximum_iterations=1))
    group=result.critical.nets[0]
    assert group.pair_state_limit==1 and group.search_states<=1
    assert not group.connected and len(group.local_preparation.pending_contacts)==2


def test_rematerialized_required_array_keeps_exact_bypass_witness():
    from test_decoupling_source_arrays import array_fixture
    from pcbir import route_critical_nets
    from pcbir.decoupling_paths import verify_crosslayer_decoupling
    source, link = array_fixture(3)
    planned = route_decouplers(source)
    assert planned.admitted_path_witnesses
    held, reason = _bypass_copper_reservations(source,planned.created_tracks,
        planned.created_vias,planned.admitted_path_witnesses)
    assert held is not None, reason
    assert Counter(held.board.vias) >= Counter(source.vias)
    assert not any(v.finish == "filled-capped" for v in held.created_vias)
    assert all(validate_crosslayer_witness(held.board,w)=="" for w in held.decoupling_witnesses)
    routed = route_critical_nets(source,route_global(source),reserved_accesses=held)
    assert Counter(routed.board.vias) >= Counter(source.vias)
    assert verify_crosslayer_decoupling(routed.board,link).witness is not None


def keepout_fixture(*, required_array=False, renamed=False):
    from pcbir.physical import CopperKeepout, PolygonWithHoles, PolygonRing
    if required_array:
        from test_decoupling_source_arrays import array_fixture
        source, link = array_fixture(3)
    else:
        source, link = fixture(renamed=renamed)
    keepout = CopperKeepout("temporary", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
            ((1,1),(2,1),(2,2),(1,2))))))
    source = replace(source, copper_keepouts=(keepout, replace(keepout,id="retained")))
    planned = route_decouplers(source)
    held, reason = _bypass_copper_reservations(source, planned.created_tracks,
        planned.created_vias, planned.admitted_path_witnesses)
    assert held is not None, reason
    return source, link, held


@pytest.mark.parametrize("required_array,renamed", ((False,False),(False,True),(True,False)))
def test_keepout_removal_rederives_complete_paths_without_changing_copper(required_array, renamed):
    from pcbir.bypass_reservations import rebind_bypass_keepout_removal
    source, link, held = keepout_fixture(required_array=required_array,renamed=renamed)
    after = replace(source,copper_keepouts=source.copper_keepouts[1:])
    stale, reason = _bypass_copper_reservations(after, held.created_tracks,
        held.created_vias, held.decoupling_witnesses)
    assert stale is None and "stale_witness_context" in reason
    rebound, reason = rebind_bypass_keepout_removal(source, held, after)
    assert rebound is not None, reason
    assert rebound.board.tracks == held.board.tracks and rebound.board.vias == held.board.vias
    assert rebound.created_tracks == held.created_tracks and rebound.created_vias == held.created_vias
    assert rebound.board.copper_keepouts == after.copper_keepouts
    assert all(validate_crosslayer_witness(rebound.board,w)=="" for w in rebound.decoupling_witnesses)
    assert verified_bypass_reservations(after,rebound)[2] is None
    assert held.board.copper_keepouts == source.copper_keepouts
    assert verified_bypass_reservations(source,held)[2] is None
    assert Counter(rebound.board.vias) >= Counter(source.vias)


def test_keepout_removal_rejects_other_changes_and_unproven_originals():
    from pcbir.bypass_reservations import rebind_bypass_keepout_removal
    source, _, held = keepout_fixture()
    after = replace(source,copper_keepouts=source.copper_keepouts[1:])
    changes = (
        replace(after,rules=replace(after.rules,minimum_clearance_nm=110000)),
        replace(after,placements=(replace(after.placements[0],position=Point.mm(7,8)),*after.placements[1:])),
        replace(after,decoupling_links=()),
        replace(after,copper_keepouts=(*after.copper_keepouts,replace(after.copper_keepouts[0],id="new"))),
        replace(after,copper_keepouts=tuple(reversed(source.copper_keepouts))),
    )
    for changed in changes:
        candidate, reason = rebind_bypass_keepout_removal(source,held,changed)
        assert candidate is None and reason
    invalid = (
        replace(held,created_vias=()),
        replace(held,board=replace(held.board,vias=())),
        replace(held,board=replace(held.board,copper_keepouts=())),
        replace(held,added_track_count=held.added_track_count+1),
    )
    for stale in invalid:
        candidate, reason = rebind_bypass_keepout_removal(source,stale,after)
        assert candidate is None and reason
    assert verified_bypass_reservations(source,held)[2] is None

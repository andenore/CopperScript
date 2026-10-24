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


def test_standard_pipeline_keeps_whole_bypass_prefix_and_emits_replayable_input(tmp_path):
    from pcbir.flow import run_routing_pipeline
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    from pcbir.unified_router import UnifiedRouterOptions
    from pcbir.decoupling_paths import verify_crosslayer_decoupling
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

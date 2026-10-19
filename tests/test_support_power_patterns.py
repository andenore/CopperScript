"""Native power preparation participates in mutable support-bank selection."""
from dataclasses import replace

from pcbir import CopperLayer, PhysicalPowerDomain, PhysicalNet, PadReference, Placement, Point
from pcbir.placement import PlacementPlannerOptions
from pcbir.support_patterns import SupportPatternOptions, improve_support_placement
from pcbir.drc import physical_board_digest
from test_support_patterns import scene
from test_power_preparation import verifier
from pcbir.power_geometry import PowerAreaOptions


def fixture():
    board,links=scene()
    source=PadReference('SOURCE','S')
    board=replace(board,placements=(*board.placements,Placement('SOURCE','chip',Point.mm(20,5))),
        nets=tuple(replace(n,pads=(*n.pads,source)) if n.name==links[0].net else n for n in board.nets),
        net_routing_rules=tuple(replace(r,allowed_layers=board.stackup.copper_layers,max_vias=12) for r in board.net_routing_rules))
    board=replace(board,power_domains=(PhysicalPowerDomain(links[0].net,board.nets[0].pads,(source,)),))
    options=PowerAreaOptions(nets=(links[0].net,),candidates_per_domain=2,maximum_layer_alternatives=1,
        allowed_layers=(CopperLayer.INTERNAL_2,),maximum_contact_alternatives=2)
    return board,options


def run(board,options,verify=None):
    return improve_support_placement(board,{p.reference:p for p in board.placements},PlacementPlannerOptions(),
        options=SupportPatternOptions(maximum_complete_checks=3),
        power_area_options=options,power_native_verifier=verify)


def test_combined_patterns_need_native_evidence_and_keep_exact_source():
    board,options=fixture()
    result=run(board,options)
    assert result.status=='native_tool_required'
    assert result.board==board and not result.power_trials and result.routing_passes==0


def test_combined_bank_rebuilds_contacts_areas_and_native_evidence(tmp_path):
    board,options=fixture();verify,calls=verifier(tmp_path)
    result=run(board,options,verify)
    assert result.status=='geometry_improved',result.report()
    assert result.board.tracks==result.board.vias==() and result.board.zones==board.zones
    prep=result.power_preparation
    assert prep.accepted and prep.evidence.matches(result.signal_scene)
    assert result.report()['power_native_acceptance']
    assert len(result.power_trials)==result.routing_passes<=5
    assert len({t['source_digest'] for t in result.power_trials})>=2
    assert all(t['plan'] for t in result.power_trials)
    assert result.power_trials[-1]['source_digest']==physical_board_digest(prep.source)
    assert prep.plan.selected and prep.board.zones
    assert not result.proposal  # Evidence binds the complete reserved scene.
    assert all(p==next(q for q in result.board.placements if q.reference==p.reference)
               for p in board.placements if p.reference not in {l.capacitor.component for l in board.decoupling_links})


def test_native_rejection_prevents_adopting_a_compact_bank(tmp_path):
    board,options=fixture();verify,calls=verifier(tmp_path)
    def reject(candidate,stage):
        evidence=verify(candidate,stage)
        return replace(evidence,other_violation_count=evidence.other_violation_count+1)
    result=run(board,options,reject)
    assert result.board==board and not result.selected and result.power_preparation is None
    assert result.power_trials
    assert all(t['status']!='native_connected' for t in result.power_trials)
    assert not result.report()['power_native_acceptance']


def test_final_combined_regeneration_failure_rolls_back_all_poses(tmp_path,monkeypatch):
    import pcbir.power_preparation as preparation
    board,options=fixture();verify,calls=verifier(tmp_path)
    original=preparation.prepare_power_distribution
    count=0
    def fail_final(*args,**kwargs):
        nonlocal count
        count+=1
        result=original(*args,**kwargs)
        # Baseline, three bounded complete trials, then mandatory fresh rebuild.
        return replace(result,status='native_power_return_rejected') if count==5 else result
    monkeypatch.setattr(preparation,'prepare_power_distribution',fail_final)
    result=run(board,options,verify)
    assert count==5 and result.status=='final_regeneration_rejected',result.report()
    assert result.board==board and not result.selected and result.power_preparation is None


def test_external_routed_scene_is_explicitly_unsupported_without_mutation():
    from pcbir import TrackSegment
    board,options=fixture()
    options=replace(options,reserved_tracks=(TrackSegment('HELD',Point.mm(2,2),Point.mm(3,2),150000,CopperLayer.FRONT),))
    result=run(board,options,lambda *_: (_ for _ in ()).throw(AssertionError('must not route')))
    assert result.status=='external_support_reservations_unsupported'
    assert result.board==board and result.routing_passes==0


def test_standard_pipeline_passes_power_settings_into_support_search(tmp_path):
    from test_power_preparation import fixture as power_fixture
    from pcbir.physical import ComponentPlacementRule
    from pcbir.flow import run_routing_pipeline
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    from pcbir.package_access import PackageAccessOptions
    from pcbir.fanout import FanoutOptions
    board,options=power_fixture(data=True)
    board=replace(board,placement_rules=tuple(ComponentPlacementRule(p.reference,side=p.side,
        fixed_position=p.position if p.reference!='C' else None) for p in board.placements))
    options=replace(options,reserve_ordinary_before_contacts=True,maximum_contact_alternatives=2)
    verify,calls=verifier(tmp_path)
    result=run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(analytical_iterations=0,refinement_passes=0,
            candidate_count=1,escape_spacing_passes=0,support_pattern_trials=1),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        package_access_options=PackageAccessOptions(maximum_trials=0,maximum_pattern_trials=0),
        fanout_options=FanoutOptions(),power_area_options=options,power_native_verifier=verify)
    assert any(c.startswith('support-') for c in calls)
    assert result.power_preparation.accepted,result.power_preparation.report()
    assert result.power_preparation_validation['status']=='native_connected'
    assert all(next(q for q in result.board.placements if q.reference==p.reference)==p
               for p in board.placements if p.reference!='C')

"""Joint prospective contacts retain actual process geometry and source intent."""
from dataclasses import replace

import pytest

from pcbir import CopperLayer, PhysicalNet, Point, Stackup, TrackSegment, Via
from pcbir.drc import explicit_copper_connectivity, physical_board_digest
from pcbir.power_geometry import PowerAreaOptions, apply_power_area_plan, plan_power_areas
from test_power_geometry import board as feed_board


def fixture():
    board = feed_board()
    board = replace(board, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.BACK)), net_routing_rules=(replace(board.net_routing_rules[0],
        allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_2)),))
    options = PowerAreaOptions(candidates_per_domain=2, maximum_layer_alternatives=1,
        allowed_layers=(CopperLayer.INTERNAL_2,), maximum_contact_alternatives=2)
    return board, options


def test_area_tree_can_choose_second_contact_arrangement():
    board, options = fixture()
    wall = TrackSegment('WALL', Point.mm(9, '4.75'), Point.mm(9, 10), 250000, CopperLayer.INTERNAL_2)
    board = replace(board, nets=(*board.nets, PhysicalNet('WALL', ())), tracks=(wall,))
    options = replace(options, search_states=1)
    first = plan_power_areas(board, replace(options, candidates_per_domain=1))
    assert not first.selected and first.search_failures
    plan = plan_power_areas(board, options)
    assert plan.selected and plan.selected[0].contact_variant == 1
    assert plan.selected[0].contact_identity
    routed, status = apply_power_area_plan(board, plan)
    assert status == 'contact_routed_native_pending'
    assert routed.tracks[0] == wall and routed.placements == board.placements
    assert explicit_copper_connectivity(routed).net_connected(board.nets[0])
    assert plan.report() == plan_power_areas(board, options).report()
    assert physical_board_digest(board) == plan.snapshot
    assert all(not z.id.startswith('power-area-contact:') for z in routed.zones)


def test_joint_assignment_avoids_individually_legal_but_colliding_drills(monkeypatch):
    from pcbir.plane import PlaneContactProposals
    from pcbir.routing_clearance import RoutingClearanceIndex
    import pcbir.power_contacts as contacts
    board, options = fixture()
    board = replace(board, placements=(board.placements[0], replace(board.placements[1], position=Point.mm('5.2', 5))))
    generated = []
    def proposals(temporary, reference, settings, **kwargs):
        start = next(p.position for p in board.placements if p.reference == reference.component)
        # Both first choices are individually legal; their drills overlap.
        targets = ((Point.mm('4.05', 5) if reference.component == 'SRC' else Point.mm('4.15', 5)),
                   Point(start.x_nm, start.y_nm-1000000))
        choices = tuple(((TrackSegment('RAIL', start, p, 250000, CopperLayer.FRONT),),
            Via('RAIL', p, board.rules.default_via_size_nm, board.rules.default_via_drill_nm,
                CopperLayer.FRONT, CopperLayer.BACK)) for p in targets)
        index = RoutingClearanceIndex(temporary)
        for path, via in choices:
            assert index.can_track('RAIL',path[0].start,path[0].end,path[0].width_nm,path[0].layer)
            assert index.can_via(via.net,via.position,via.size_nm,via.from_layer,via.to_layer,via.drill_nm)
        generated.append(choices)
        return PlaneContactProposals(physical_board_digest(temporary), choices, 2, 'finite_pool_generated', targets)
    monkeypatch.setattr(contacts, 'propose_plane_contacts', proposals)
    plan = plan_power_areas(board, options)
    assert plan.selected and len(generated) == 2
    assert all(not {choices[0][1] for choices in generated} <= set(c.vias) for c in plan.candidates)
    assert apply_power_area_plan(board, plan)[1] == 'contact_routed_native_pending'
    assert plan.contact_trials[0]['pair_checks'] > 0


@pytest.mark.parametrize('bridge',[0,1])
def test_conflicting_package_contacts_can_share_one_checked_transition(monkeypatch,tmp_path,bridge):
    from pcbir import BoardSide
    from pcbir.plane import PlaneContactProposals
    from pcbir.plane_verify import verify_filled_planes
    import pcbir.power_contacts as contacts
    import shutil
    board,options=fixture()
    board=replace(board,placements=(board.placements[0],replace(board.placements[1],
        position=Point.mm('5.2',5),side=BoardSide.BACK)),net_routing_rules=(replace(board.net_routing_rules[0],
        allowed_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_2,CopperLayer.BACK)),))
    options=replace(options,maximum_branch_layers=2*bridge,
        allowed_layers=(CopperLayer.INTERNAL_2,CopperLayer.FRONT,CopperLayer.BACK))
    def proposals(temporary,reference,settings,**kwargs):
        start=next(p.position for p in board.placements if p.reference==reference.component)
        target=Point.mm('4.05' if reference.component=='SRC' else '4.15',5)
        choice=((TrackSegment('RAIL',start,target,250000,
            CopperLayer.FRONT if reference.component=='SRC' else CopperLayer.BACK),),
            Via('RAIL',target,board.rules.default_via_size_nm,board.rules.default_via_drill_nm,
                CopperLayer.FRONT,CopperLayer.BACK))
        return PlaneContactProposals(physical_board_digest(temporary),(choice,),1,'finite_pool_generated',(target,))
    monkeypatch.setattr(contacts,'propose_plane_contacts',proposals)
    plan=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2)
    if not bridge:
        assert not plan.selected and apply_power_area_plan(board,plan)[0] is board
        return
    assert plan.selected,plan.report()
    routed,status=apply_power_area_plan(board,plan)
    assert status=='contact_routed_native_pending' and len(routed.vias)==1
    assert explicit_copper_connectivity(routed).net_connected(board.nets[0])
    assert any(o.status=='joined_existing_component' for o in plan.contact_observations)
    cli=shutil.which('kicad-cli')
    if not cli:pytest.skip('KiCad CLI unavailable')
    evidence=verify_filled_planes(routed,kicad_cli=cli,artifact_directory=tmp_path/'native')
    assert evidence.matches(routed)
    assert evidence.unconnected_count==evidence.other_violation_count==evidence.island_count==0


@pytest.mark.parametrize('field,value', [('contact_assignment_states', 1), ('contact_pair_queries', 1),
                                        ('contact_site_queries', 1)])
def test_exhausted_work_has_exact_rollback_and_visible_counters(field, value):
    board, options = fixture()
    options = replace(options, **{field: value})
    plan = plan_power_areas(board, options)
    assert not plan.selected
    assert apply_power_area_plan(board, plan)[0] is board
    report = plan.contact_trials[0]
    assert report['site_queries'] <= options.contact_site_queries
    assert report['assignment_states'] <= options.contact_assignment_states
    assert report['pair_queries'] <= options.contact_pair_queries
    assert board.tracks == board.vias == ()


def test_existing_transition_is_retained_and_forbidden_surface_layer_rejected():
    board, options = fixture()
    via = Via('RAIL', Point.mm(3, 4), 600000, 300000, CopperLayer.FRONT, CopperLayer.BACK)
    track = TrackSegment('RAIL', Point.mm(3, 5), via.position, 250000, CopperLayer.FRONT)
    board = replace(board, tracks=(track,), vias=(via,))
    plan = plan_power_areas(board, options)
    routed, status = apply_power_area_plan(board, plan)
    assert status == 'contact_routed_native_pending'
    assert routed.tracks[0] == track and routed.vias[0] == via
    assert len(routed.vias) == 2
    forbidden = replace(board, net_routing_rules=(replace(board.net_routing_rules[0],
                        allowed_layers=(CopperLayer.INTERNAL_2,)),))
    rejected = plan_power_areas(forbidden, options)
    assert not rejected.selected and apply_power_area_plan(forbidden, rejected)[0] is forbidden
    with pytest.raises(ValueError, match='stale'):
        apply_power_area_plan(replace(board, vias=()), plan)


def test_native_fill_of_alternative_contacts(tmp_path):
    from pcbir.plane_verify import verify_filled_planes
    import shutil
    cli = shutil.which('kicad-cli')
    if not cli:
        pytest.skip('KiCad CLI unavailable')
    board, options = fixture()
    plan = plan_power_areas(board, options)
    candidate, status = apply_power_area_plan(board, plan)
    assert status == 'contact_routed_native_pending'
    evidence = verify_filled_planes(candidate, kicad_cli=cli, artifact_directory=tmp_path/'native')
    assert evidence.matches(candidate)
    assert evidence.unconnected_count == evidence.other_violation_count == evidence.island_count == 0


def local_capacitor_fixture():
    from pcbir import BoardSide, PadReference, PhysicalFootprint, FootprintPad, Size
    from pcbir.physical import DecouplingLink
    board, options = fixture()
    cap_fp = PhysicalFootprint('capacitor', (FootprintPad('1',Point(0,0),Size.mm(1,1)),
        FootprintPad('2',Point.mm(2,0),Size.mm(1,1))), Size.mm(4,2))
    cap = replace(board.placements[1], reference='CAP', footprint='capacitor',
                  position=Point.mm(15, '6.8'), side=BoardSide.BACK)
    net = replace(board.nets[0], pads=(*board.nets[0].pads, PadReference('CAP','1')))
    board = replace(board, placements=(*board.placements, cap),
        footprints={**board.footprints,'capacitor':cap_fp},
        nets=(net, PhysicalNet('GND',(PadReference('CAP','2'),))),
        power_domains=(replace(board.power_domains[0], members=net.pads),),
        decoupling_links=(DecouplingLink(net='RAIL', target=PadReference('LOAD','1'),
            capacitor=PadReference('CAP','1'), return_net='GND', return_pad=PadReference('CAP','2')),),
        net_routing_rules=(replace(board.net_routing_rules[0], max_vias=2,
            allowed_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_2,CopperLayer.BACK)),))
    options = replace(options, maximum_branch_layers=1,
        allowed_layers=(CopperLayer.INTERNAL_2,CopperLayer.BACK))
    return board, options


def test_local_capacitor_can_share_its_target_contact_without_an_extra_via():
    from pcbir import PadReference
    board, options = local_capacitor_fixture()
    plan = plan_power_areas(board, options, _seed_layer=CopperLayer.INTERNAL_2)
    assert plan.selected, plan.report()
    assert len(plan.selected[0].vias) == 2
    assert any(o.status=='joined_existing_component' and PadReference('CAP','1') in o.terminals
               for o in plan.contact_observations)
    # This fixture isolates supply connectivity; the return land is retained
    # but its native connection is a separate preparation-stage obligation.
    assert plan.source_board is board and board.decoupling_links == plan.source_board.decoupling_links


def test_complete_direct_contacts_precede_unreachable_shared_contacts_with_one_tree_slot():
    board, options = local_capacitor_fixture()
    board = replace(board, net_routing_rules=(replace(board.net_routing_rules[0], max_vias=3),))
    # Surface bridging is unavailable, while a direct plated transition is legal.
    options = replace(options, allowed_layers=(CopperLayer.INTERNAL_2,), candidates_per_domain=1)
    plan = plan_power_areas(board, options, _seed_layer=CopperLayer.INTERNAL_2)
    assert plan.selected, plan.report()
    candidate = plan.selected[0]
    assert candidate.contact_variant == 1 and len(candidate.vias) == 3
    assert len(plan.candidates) == 1 and not plan.conflict_trials
    routed, status = apply_power_area_plan(board, plan)
    assert status == 'contact_routed_native_pending'
    assert explicit_copper_connectivity(routed).net_connected(board.nets[0])
    # Explicit regeneration keeps the original pool identity after ranking.
    seeded = plan_power_areas(board, options, _seed_layer=CopperLayer.INTERNAL_2, _seed_contact=1)
    assert seeded.selected[0].contact_identity == candidate.contact_identity
    rejected = plan_power_areas(board, options, _seed_layer=CopperLayer.INTERNAL_2, _seed_contact=0)
    assert not rejected.selected and apply_power_area_plan(board, rejected)[0] is board


def test_short_direct_contact_precedes_a_complete_long_surface_bridge():
    board,options=local_capacitor_fixture()
    board=replace(board,net_routing_rules=(replace(board.net_routing_rules[0],max_vias=3),))
    options=replace(options,candidates_per_domain=1)
    direct=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2,_seed_contact=1)
    bridge=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2,_seed_contact=0)
    assert direct.selected and bridge.selected
    assert len(direct.selected[0].vias)==3 and len(bridge.selected[0].vias)==2
    selected=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2)
    assert selected.selected and selected.selected[0].contact_identity==direct.selected[0].contact_identity
    assert len(selected.candidates)==1 and not selected.conflict_trials
    routed,status=apply_power_area_plan(board,selected)
    assert status=='contact_routed_native_pending'
    assert explicit_copper_connectivity(routed).net_connected(board.nets[0])


@pytest.mark.parametrize('bridge_available',[False,True])
def test_repair_contact_preference_is_revalidated_without_forcing_a_failed_bank(bridge_available):
    board,options=local_capacitor_fixture()
    board=replace(board,net_routing_rules=(replace(board.net_routing_rules[0],max_vias=3),))
    options=replace(options,candidates_per_domain=1)
    if not bridge_available:options=replace(options,allowed_layers=(CopperLayer.INTERNAL_2,))
    strict=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2,_seed_contact=0)
    assert bool(strict.selected)==bridge_available  # Strict diagnostic replay remains exact.
    preferred=plan_power_areas(board,options,_seed_layer=CopperLayer.INTERNAL_2,_preferred_contact=0)
    assert preferred.selected and preferred.selected[0].contact_variant==(0 if bridge_available else 1)
    routed,status=apply_power_area_plan(board,preferred)
    assert status=='contact_routed_native_pending' and explicit_copper_connectivity(routed).net_connected(board.nets[0])
    assert preferred.contact_trials==strict.contact_trials  # The existing finite pool is sufficient.
    assert board.tracks==board.vias==() and preferred.source_board is board
    with pytest.raises(ValueError,match='contact seed and preference'):
        plan_power_areas(board,options,_seed_contact=0,_preferred_contact=1)


def test_failed_deferred_contact_does_not_rotate_successful_bank_contacts(monkeypatch):
    from pcbir import BoardSide, PadReference
    from pcbir.plane import PlaneContactProposals
    from pcbir.power_geometry import power_obligations
    import pcbir.power_contacts as contacts
    board,options=local_capacitor_fixture()
    board=replace(board,net_routing_rules=(replace(board.net_routing_rules[0],max_vias=3),))
    def proposals(temporary,reference,settings,**kwargs):
        pose=next(p for p in board.placements if p.reference==reference.component)
        targets=tuple(Point(pose.position.x_nm+dx,pose.position.y_nm) for dx in (-1000000,1000000))
        layer=CopperLayer.BACK if pose.side==BoardSide.BACK else CopperLayer.FRONT
        choices=tuple(((TrackSegment('RAIL',pose.position,p,250000,layer),),
            Via('RAIL',p,600000,300000,CopperLayer.FRONT,CopperLayer.BACK)) for p in targets)
        return PlaneContactProposals(physical_board_digest(temporary),choices,2,'finite_pool_generated',targets)
    monkeypatch.setattr(contacts,'propose_plane_contacts',proposals)
    calls=[]
    def prepare(candidate,representatives):
        # The bounded bridge evaluator reports this one deferred component.
        calls.append(candidate)
        return candidate,frozenset({PadReference('CAP','1')})
    pool=contacts.propose_contact_arrangements(board,board,power_obligations(board,options)[0],
        CopperLayer.INTERNAL_2,board.nets[0].pads,options,prepare_candidate=prepare)
    assert len(calls)==len(pool.boards)==options.maximum_contact_alternatives==2
    assert len(pool.boards[0].vias)==2 and len(pool.boards[1].vias)==3
    assert set(pool.boards[0].vias)<=set(pool.boards[1].vias)
    assert pool.observation['assignment_states']<=options.contact_assignment_states


def test_through_contacts_may_cross_an_ordinary_ground_plane_with_antipads():
    from pcbir import CopperZone, PolygonRing, PolygonWithHoles
    board, options = fixture()
    ground = CopperZone('ground', 'GND', (CopperLayer.INTERNAL_1,),
                        PolygonWithHoles(PolygonRing(board.outline.vertices)))
    board = replace(board, zones=(ground,), nets=(*board.nets,PhysicalNet('GND',())))
    plan = plan_power_areas(board, options)
    assert plan.selected
    assert apply_power_area_plan(board, plan)[1] == 'contact_routed_native_pending'
    assert board.zones == (ground,)


def test_required_array_multiplicity_and_fixed_source_copper_survive():
    from collections import Counter
    from pcbir import PhysicalPowerDomain
    from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
    from test_pad_via_arrays import _array_board, EP
    board = materialize_via_in_pad_arrays(_array_board(rows=2,columns=2,pitch_nm=800000))
    rail = next(n for n in board.nets if n.name=='GND')
    board = replace(board,power_domains=(PhysicalPowerDomain('GND',rail.pads,(EP,)),))
    options = PowerAreaOptions(nets=('GND',),allowed_layers=(CopperLayer.INTERNAL_2,),
        candidates_per_domain=2,maximum_layer_alternatives=1,maximum_contact_alternatives=2)
    plan = plan_power_areas(board,options)
    assert plan.selected, plan.report()
    routed,status = apply_power_area_plan(board,plan)
    assert status=='contact_routed_native_pending'
    assert Counter(board.vias) <= Counter(routed.vias)
    assert len(board.vias)==4
    assert routed.via_in_pad_rules==board.via_in_pad_rules
    assert routed.placements==board.placements


@pytest.mark.parametrize('field', ['maximum_contact_alternatives', 'contact_candidates_per_terminal',
    'contact_site_queries', 'contact_assignment_states', 'contact_pair_queries'])
def test_contact_work_bounds_are_explicit_positive_integers(field):
    with pytest.raises(ValueError):
        PowerAreaOptions(**{field: 0})

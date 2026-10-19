"""Reference-motivated surface channels retain source rules and ownership."""
from dataclasses import replace

import pytest

from pcbir.boundary_access import BoundaryAccessOptions, _maze_path, package_collar
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.physical import CopperLayer, NetRoutingRule, Point, nm_from_mm
from pcbir.pin_escape import verified_routing_access
from pcbir.routing_clearance import RoutingClearanceIndex
from test_boundary_access import fixture, rectangle, ring
from test_surface_package_escape import board, PIN, FOUR


def corridor(length):
    source = replace(fixture().board, tracks=(), vias=())
    return replace(source, rules=replace(source.rules,
        minimum_clearance_nm=100000, minimum_track_width_nm=100000,
        default_track_width_nm=150000),
        net_routing_rules=(NetRoutingRule('A', width_nm=150000,
            breakout_width_nm=100000, breakout_length_nm=length),),
        copper_keepouts=(
            rectangle('upper', (CopperLayer.FRONT,), 5.8, 0, 20, 5.94),
            rectangle('lower', (CopperLayer.FRONT,), 5.8, 6.06, 20, 16),
            rectangle('left', (CopperLayer.FRONT,), 5.7, 0, 5.8, 16)))


def test_surface_maze_searches_the_authorized_neckdown_pieces():
    source = corridor(nm_from_mm('10'))
    settings = BoundaryAccessOptions()
    area = package_collar(source, 'U', settings.collar_margin_nm).bounds
    index = RoutingClearanceIndex(source)
    result, states = _maze_path(source, index, 'A', Point.mm(6, 6), 150000,
        CopperLayer.FRONT, area, 250000, settings)
    assert result is not None and 0 < states <= settings.maze_state_budget
    pieces = tuple(piece for t in result[1] for piece in index.route_pieces(
        'A', t.start, t.end, 150000, t.layer))
    assert any(t.width_nm == 100000 for t in pieces)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in pieces)
    assert source.tracks == source.vias == ()


def test_neckdown_cannot_be_extended_beyond_its_source_region():
    source = corridor(nm_from_mm('.5'))
    settings = BoundaryAccessOptions()
    result, states = _maze_path(source, RoutingClearanceIndex(source), 'A', Point.mm(6,6),
        150000, CopperLayer.FRONT, package_collar(source,'U',250000).bounds,250000,settings)
    assert result is None and states <= settings.maze_state_budget


def test_maze_does_not_escape_through_a_remote_edge_outside_the_detour_limit():
    fan = fixture()
    layers = fan.board.stackup.copper_layers
    source = replace(fan.board, copper_keepouts=(
        *ring(layers)[:1], *ring(layers)[2:],
        rectangle('first-wall', layers, 7.4, 2, 7.6, 6.4),
        rectangle('second-wall', layers, 9, 6, 9.2, 10)))
    start = Point.mm(6.8, 6)
    area = package_collar(source, 'U', 250000).bounds
    index = RoutingClearanceIndex(source)
    tight, _ = _maze_path(source, index, 'A', start, source.rules.default_track_width_nm,
        CopperLayer.FRONT, area, 250000, BoundaryAccessOptions(maximum_detour_nm=0))
    available, _ = _maze_path(source, index, 'A', start, source.rules.default_track_width_nm,
        CopperLayer.FRONT, area, 250000, BoundaryAccessOptions())
    assert tight is None and available is not None


def test_temporary_copper_preserves_exact_source_occurrences_and_only_transfers_additions():
    from pcbir.copper_reservations import reserve_copper_scene
    source = fixture().board
    reserved = replace(source.tracks[0], start=Point.mm(12,12), end=Point.mm(13,12))
    scene = reserve_copper_scene(source, (source.tracks[0], reserved, reserved))
    assert scene.board.tracks == (*source.tracks, reserved, reserved)
    added = replace(source.tracks[0], start=Point.mm(10,3), end=Point.mm(12,3))
    result = scene.extract(replace(scene.board, tracks=(*scene.board.tracks,added)))
    assert result.tracks == (*source.tracks, added) and result.vias == source.vias
    assert source.tracks == scene.base.tracks
    with pytest.raises(ValueError, match='exact input prefix'):
        scene.extract(replace(scene.board,tracks=scene.board.tracks[1:]))
    with pytest.raises(ValueError, match='physical intent'):
        scene.extract(replace(scene.board,net_routing_rules=(NetRoutingRule('A',width_nm=100000),)))


def test_early_contact_snapshot_preserves_signal_ports_without_ordinary_copper():
    from test_power_preparation import fixture as power_fixture
    from pcbir.critical import route_critical_nets
    from pcbir.routing import route_global
    from pcbir.power_preparation import prepare_power_contacts
    from pcbir.joint_preparation import joint_reservations_preserved
    source, options = power_fixture(data=True)
    critical = route_critical_nets(source, route_global(source))
    result = prepare_power_contacts(source,critical,
        replace(options,reserve_ordinary_before_contacts=True),FanoutOptions(),ordinary_nets={'DATA'})
    assert result.status == 'contacts_prepared'
    snapshot = result.contact_snapshot
    assert snapshot.matches() and snapshot.early_reservations.ports
    assert joint_reservations_preserved(snapshot.early_reservations,snapshot.board)
    assert not any(t.net == 'DATA' for t in snapshot.board.tracks)
    assert not result.accepted and not snapshot.ownership.frozen


def test_stale_critical_input_cannot_authorize_new_contact_preparation():
    from test_power_preparation import fixture as power_fixture
    from pcbir.critical import route_critical_nets
    from pcbir.routing import route_global
    from pcbir.power_preparation import prepare_power_contacts
    source, options = power_fixture(data=True)
    critical = route_critical_nets(source, route_global(source))
    changed = replace(source,placements=(replace(source.placements[0],position=Point.mm(5,10)),*source.placements[1:]))
    result = prepare_power_contacts(changed,critical,
        replace(options,reserve_ordinary_before_contacts=True),FanoutOptions(),ordinary_nets={'DATA'})
    assert result.status == 'stale_critical_snapshot' and result.contact_snapshot is None
    assert result.board is changed


def test_native_power_acceptance_keeps_early_signals_temporary(tmp_path):
    from test_power_preparation import fixture as power_fixture, verifier
    from pcbir.routing import route_global
    from pcbir.power_preparation import prepare_power_distribution
    source, options = power_fixture(data=True)
    native, calls = verifier(tmp_path)
    result = prepare_power_distribution(source,route_global(source),
        replace(options,reserve_ordinary_before_contacts=True),FanoutOptions(),
        native_verifier=native,ordinary_nets={'DATA'})
    assert result.accepted and result.joint_access.ports
    assert result.joint_access.ownership.frozen and result.ownership.frozen
    assert not any(t.net == 'DATA' for t in result.board.tracks)
    assert result.evidence.other_violation_count == 0
    assert calls == ['power-return-baseline','power-return-candidate']


def test_perimeter_policy_keeps_deep_array_control_dogbones_and_is_pose_independent():
    from pcbir.access_sites import circular_array_depth
    from pcbir.physical import FootprintPad, PadShape, PhysicalFootprint, PhysicalNet, PadReference, Size, BoardSide
    from test_interstitial_fanout import fixture as array_fixture
    source,pose,_,_ = array_fixture()
    pads = tuple(FootprintPad(str(y*7+x),Point.mm((x-3)*.75,(y-3)*.75),Size.mm('.25','.25'),shape=PadShape.CIRCLE)
                 for y in range(7) for x in range(7))
    fp = PhysicalFootprint('array7',pads,Size.mm('5.25','5.25'))
    pose=replace(pose,footprint='array7')
    terminal=PhysicalFootprint('one',(FootprintPad('1',Point(0,0),Size.mm('.3','.3')),),Size.mm('.5','.5'))
    source=replace(source,footprints={'array7':fp,'one':terminal},placements=(pose,
        replace(pose,reference='J_E',footprint='one',position=Point.mm(10,3)),
        replace(pose,reference='J_C',footprint='one',position=Point.mm(10,8))),
        nets=(PhysicalNet('EDGE',(PadReference('U','10'),PadReference('J_E','1'))),
              PhysicalNet('CORE',(PadReference('U','24'),PadReference('J_C','1')))))
    for rotation in (0,90,180,270):
        for side in (BoardSide.FRONT,BoardSide.BACK):
            transformed=replace(pose,rotation_degrees=rotation,side=side)
            assert circular_array_depth(source,transformed,pads[10],1500000)==1
            assert circular_array_depth(source,transformed,pads[24],1500000)==3
    result=route_fanout(source,FanoutOptions(prefer_surface_escapes=True,surface_preference_depth=1))
    assert PadReference('U','10') in result.surface_accesses
    assert PadReference('U','24') in result.accesses
    assert not result.pending_pads
    partial=replace(source,footprints={'array7':replace(fp,pads=pads[:-1]),'one':terminal})
    assert circular_array_depth(partial,pose,pads[10],1500000) is None


@pytest.mark.parametrize('renamed',(False,True))
def test_late_local_completion_is_bounded_and_keeps_exact_source_ownership(renamed):
    from test_decoupling_crosslayer_paths import fixture as bypass_fixture
    from pcbir.power_preparation import complete_preparation_bypasses
    from pcbir.power_geometry import PowerAreaOptions
    from pcbir.copper_ownership import begin_copper_ownership
    from pcbir.package_access import _verified_bypass_links
    source,link=bypass_fixture(renamed=renamed)
    ledger=begin_copper_ownership(source)
    zero=PowerAreaOptions(nets=(link.net,),joint_contact_trials=0)
    unchanged,held,trials=complete_preparation_bypasses(source,ledger,zero,FanoutOptions())
    assert unchanged is source and held is ledger and trials==()
    result,ownership,trials=complete_preparation_bypasses(source,ledger,
        replace(zero,joint_contact_trials=1),FanoutOptions())
    assert len(trials)==1 and trials[0]['status']=='local_link_restored'
    assert ownership.matches(result) and link in _verified_bypass_links(result,{link.net})
    assert result.placements==source.placements and result.net_routing_rules==source.net_routing_rules
    with pytest.raises(ValueError,match='disposable'):
        complete_preparation_bypasses(result,replace(ownership,frozen=True),zero,FanoutOptions())


def test_bypass_roots_can_negotiate_a_contact_when_no_ordinary_pin_is_pending(monkeypatch,tmp_path):
    import pcbir.fanout as fanout
    from test_decoupling_crosslayer_paths import fixture as bypass_fixture
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles,TrackSegment,Via,Size
    from pcbir.copper_ownership import begin_copper_ownership,append_copper_ownership,prospective_contact_uses
    from pcbir.joint_preparation import negotiate_preparation_contacts
    from pcbir.package_access import _verified_bypass_links
    source,link=bypass_fixture()
    # This fixture also exercises native DRC. Keep its synthetic body outline
    # clear of the lands rather than suppressing unmapped silkscreen findings.
    source=replace(source,footprints={name:replace(fp,body_size=Size.mm(6,6))
                                      for name,fp in source.footprints.items()})
    zone=CopperZone('return',link.return_net,(CopperLayer.BACK,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((1,1),(24,1),(24,19),(1,19))))))
    source=replace(source,zones=(zone,))
    via=Via(link.return_net,Point.mm(9.5,8.5),450000,200000)
    tracks=(TrackSegment(link.return_net,Point.mm(9,10),Point.mm(9.5,9.5),150000,CopperLayer.FRONT),
            TrackSegment(link.return_net,Point.mm(9.5,9.5),via.position,150000,CopperLayer.FRONT))
    original=replace(source,tracks=tracks,vias=(via,))
    from pcbir.physical import PadReference
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',
        uses=prospective_contact_uses(original,(PadReference('U','G'),)),trial_mutable=True)
    old=fanout._candidates
    monkeypatch.setattr(fanout,'_candidates',lambda start,*a,**k:
        (Point.mm(9.5,8.5),) if start==Point.mm(9,8) else old(start,*a,**k))
    result=negotiate_preparation_contacts(original,ledger,FanoutOptions(),only_nets=frozenset(),
        bypass_links=frozenset({link}))
    assert not result.baseline.pending_pads
    assert result.status=='geometry_proposal',result.report()
    assert link in _verified_bypass_links(result.board,{link.net}) and result.ownership.matches(result.board)
    assert original.tracks==tracks and original.vias==(via,)
    import shutil
    from pathlib import Path
    from pcbir.plane_verify import verify_filled_planes
    from pcbir.power_extract import compare_power_fill_connectivity
    cli=shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable for native contact preservation')
    before=verify_filled_planes(original,kicad_cli=Path(cli),artifact_directory=tmp_path/'before')
    after=verify_filled_planes(result.board,kicad_cli=Path(cli),artifact_directory=tmp_path/'after')
    assert compare_power_fill_connectivity(original,before,result.board,after)['status']=='preserved'
    assert after.other_violation_count==0


def test_local_bypass_geometry_sites_share_the_original_site_allowance():
    from test_geometry_access_sites import offset_scene
    from pcbir.decoupling_paths import _local_launch_sites
    from pcbir.fanout import _legal_choices
    source,pose,pad,start,seed=offset_scene()
    ordinary=FanoutOptions(step_nm=125000,refinement_step_nm=125000)
    original=tuple(_local_launch_sites(source,pose,pad,ordinary,24))
    index=RoutingClearanceIndex(source)
    sites=tuple(_local_launch_sites(source,pose,pad,replace(ordinary,maximum_geometry_candidates=24),24,
                                    clearance=index,net='N5'))
    novel=tuple(p for p in sites if p not in original)
    assert len(sites)<=24 and novel
    assert _legal_choices(source,index,'N5',start,pose,pad,novel,multiple_orders=True)
    assert tuple(_local_launch_sites(source,pose,pad,ordinary,24))==original


@pytest.mark.parametrize('spent',(0,1,4))
def test_early_bypass_negotiation_shares_completion_trial_budget(monkeypatch,spent):
    from test_power_preparation import fixture as power_fixture
    from pcbir.critical import route_critical_nets
    from pcbir.routing import route_global
    from pcbir.power_preparation import prepare_power_contacts,prepare_power_from_contacts
    import pcbir.joint_preparation as joint
    import pcbir.package_access as access
    import pcbir.power_geometry as geometry
    source,options=power_fixture(data=True)
    options=replace(options,plane_return_nets=(),reserve_ordinary_before_contacts=True,joint_contact_trials=4)
    critical=route_critical_nets(source,route_global(source))
    prep=prepare_power_contacts(source,critical,options,FanoutOptions(),ordinary_nets={'DATA'})
    snapshot=replace(prep.contact_snapshot,contact_completion_trials=({'status':'finite_local_pool_unresolved'},)*spent)
    assert snapshot.matches()
    monkeypatch.setattr(access,'_verified_bypass_links',lambda *a,**k:frozenset())
    calls=[]
    class ReachedStage(Exception):
        pass
    def negotiate(board,ownership,fanout,**kwargs):
        calls.append(kwargs)
        assert kwargs['fixed_reservations'] is snapshot.early_reservations
        assert kwargs['bypass_links']==frozenset(source.decoupling_links)
        assert kwargs['options'].maximum_trials==4-spent
        assert kwargs['only_nets']==frozenset()
        raise ReachedStage
    def areas(*a,**k):
        raise ReachedStage
    monkeypatch.setattr(joint,'negotiate_preparation_contacts',negotiate)
    monkeypatch.setattr(geometry,'plan_power_areas',areas)
    with pytest.raises(ReachedStage):
        prepare_power_from_contacts(snapshot,options,FanoutOptions(),native_verifier=lambda *a:object())
    assert len(calls)==(spent<4)


def test_early_policy_prefers_a_verified_surface_without_changing_the_net_layers():
    source = board(allowed_layers=FOUR)
    result = route_fanout(source, FanoutOptions(prefer_surface_escapes=True))
    access = result.surface_accesses[PIN]
    assert PIN not in result.pending_pads and PIN not in result.accesses
    assert not any(v.net == 'X1' for v in result.board.vias)
    assert verified_routing_access(result.board, PIN, 'X1', access, RoutingClearanceIndex(result.board))
    assert result.board.net_routing_rules == source.net_routing_rules


@pytest.mark.parametrize('value', [1, 'yes', None])
def test_surface_preference_has_an_explicit_runtime_policy(value):
    with pytest.raises(ValueError):
        FanoutOptions(prefer_surface_escapes=value)
    with pytest.raises(ValueError):
        FanoutOptions(prefer_surface_escapes=True, surface_fallback=False)

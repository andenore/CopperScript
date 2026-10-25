from dataclasses import replace

import pytest

from pcbir import (CopperLayer, CopperZone, PhysicalPowerDomain, Point,
                   PolygonRing, PolygonWithHoles, TrackSegment, PhysicalNet, PadReference)
from pcbir.drc import physical_board_digest
from pcbir.power_geometry import PowerAreaOptions, plan_power_areas, apply_power_area_plan
from pcbir.zone_geometry import deferred_zone_nets
from test_power_feed import board as feed_board


def board():
    b = feed_board()
    return replace(b, power_domains=(PhysicalPowerDomain('RAIL', b.nets[0].pads, b.nets[0].pads[:1]),))


def test_tree_area_has_exact_contacts_and_atomic_stale_rejection():
    b = board()
    events = []
    plan = plan_power_areas(b, PowerAreaOptions(candidates_per_domain=2),
                           on_progress=lambda phase, event, data: events.append((phase, event, data)))
    assert plan.status == 'geometry_proposal'
    assert len(plan.selected) == 1 and plan.selected[0].zones
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    assert routed.placements == b.placements and routed.nets == b.nets
    assert routed.tracks[:len(b.tracks)] == b.tracks
    assert physical_board_digest(b) == plan.snapshot
    with pytest.raises(ValueError, match='stale'):
        apply_power_area_plan(routed, plan)
    with pytest.raises(ValueError, match='source contracts'):
        apply_power_area_plan(replace(b, power_domains=(replace(b.power_domains[0], sources=()),)), plan)
    assert plan.report()['domains'][0]['status'] == 'boundary_known'
    assert events[-1][0:2] == ('power_area_search', 'finished')
    assert events[-1][2]['status'] == plan.status
    assert sum(phase == 'power_area_candidate' for phase, _, _ in events) == 2


def test_macro_boundary_port_survives_anchor_limit(tmp_path):
    from test_hard_macros import fixture
    from pcbir.hard_macros import materialize_hard_macros
    from pcbir.drc import explicit_copper_connectivity, _explicit_copper_contacts
    from pcbir.power_geometry import _anchors
    original, asset, bind = fixture(tmp_path)
    # The declared public port is lexicographically after private copper.
    asset['protected_regions'] = []
    asset['keepouts'] = []
    asset['ports'][0]['point'] = {'pad': ['passive', '1']}
    b = materialize_hard_macros(bind(asset))
    graph = explicit_copper_connectivity(b, include_internal_connections=False)
    contacts, _, _ = _explicit_copper_contacts(b, include_internal_connections=False)
    groups = _anchors(b, 'N', graph, contacts, b.nets[0].pads, CopperLayer.FRONT, 1)
    assert tuple(groups.values()) == ((Point.mm(12, 10),),)


def test_unknown_boundary_and_budget_exhaustion_never_invent_copper():
    b = board()
    unknown = replace(b, power_domains=(replace(b.power_domains[0], sources=()),))
    plan = plan_power_areas(unknown)
    assert plan.status == 'unresolved' and not plan.selected
    assert apply_power_area_plan(unknown, plan)[0] is unknown
    wall = TrackSegment('WALL', Point.mm(9, 0), Point.mm(9, 10), 250000, CopperLayer.FRONT)
    obstructed = replace(b, nets=(*b.nets, PhysicalNet('WALL', ())), tracks=(wall,))
    bounded = plan_power_areas(obstructed, PowerAreaOptions(search_states=1, candidates_per_domain=1))
    assert not bounded.selected
    assert ('RAIL', 'no_tree_within_budget') in bounded.diagnostics
    assert bounded.search_failures and bounded.search_failures[0].expanded_states <= 1
    assert bounded.report()['search_failures'][0]['start_nm'] != bounded.report()['search_failures'][0]['end_nm']


def test_other_contact_edge_can_avoid_maze_search_on_blocked_nearest_pair():
    b = board()
    wall = TrackSegment('WALL', Point.mm(6, '4.75'), Point.mm(12, '4.75'), 250000, CopperLayer.FRONT)
    b = replace(b, nets=(*b.nets, PhysicalNet('WALL', ())), tracks=(wall,))
    options = PowerAreaOptions(search_states=1, candidates_per_domain=1, maximum_anchors=8)
    plan = plan_power_areas(b, options)
    assert plan.selected and not plan.search_failures
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending' and routed.tracks[0] == wall
    restricted = plan_power_areas(b, replace(options, maximum_anchors=1))
    assert not restricted.selected


def test_tree_defers_blocked_load_until_another_branch_supplies_a_better_parent():
    from pcbir.physical import CopperKeepout
    b = board()
    net = replace(b.nets[0], pads=(*b.nets[0].pads, PadReference('FAR', '1')))
    b = replace(b, placements=(b.placements[0], replace(b.placements[1], position=Point.mm(6, 5)),
        replace(b.placements[1], reference='FAR', position=Point.mm(15, 8))), nets=(net,),
        power_domains=(PhysicalPowerDomain(net.name, net.pads, net.pads[:1]),),
        copper_keepouts=(CopperKeepout('wall', (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing(
            tuple(Point.mm(x, y) for x, y in (('4.4', '3.5'), ('4.6', '3.5'), ('4.6', '6'), ('4.4', '6')))))),))
    options = PowerAreaOptions(candidates_per_domain=1, allowed_layers=(CopperLayer.FRONT,), search_states=1)
    forward = plan_power_areas(b, options)
    assert len(forward.selected) == 1 and not forward.selected[0].tree_reverse
    assert apply_power_area_plan(b, forward)[1] == 'contact_routed_native_pending'
    plan = plan_power_areas(b, options, _seed_layer=CopperLayer.FRONT, _seed_tree=(250000, True, 0))
    assert len(plan.selected) == 1
    assert plan.selected[0].tree_reverse and plan.selected[0].tree_width_nm == 250000
    assert apply_power_area_plan(b, plan)[1] == 'contact_routed_native_pending'
    with pytest.raises(ValueError, match='minimum width'):
        plan_power_areas(b, options, _seed_tree=(1, True, 0))


def test_source_connected_remote_anchor_survives_dense_package_contacts():
    from pcbir.physical import CopperKeepout
    b = board()
    points = tuple(Point.mm(3, y) for y in (5, '5.1', '5.2', 8))
    original = tuple(TrackSegment('RAIL', a, z, 250000, CopperLayer.FRONT)
                     for a, z in zip(points, points[1:]))
    b = replace(b, tracks=original, copper_keepouts=(CopperKeepout('wall', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
            (('4.4', 0), ('4.6', 0), ('4.6', 7), ('4.4', 7)))))),))
    options = PowerAreaOptions(maximum_anchors=2, candidates_per_domain=1, search_states=1)
    plan = plan_power_areas(b, options)
    assert plan.selected and not plan.search_failures
    assert any(t.start == points[-1] or t.end == points[-1] for t in plan.selected[0].tracks)
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending' and routed.tracks[:len(original)] == original
    restricted = plan_power_areas(b, replace(options, maximum_anchors=1))
    assert not restricted.selected and apply_power_area_plan(b, restricted)[0] is b


def test_alternate_parents_share_the_original_maze_allowance(monkeypatch):
    import pcbir.power_geometry as geometry
    b = board()
    options = PowerAreaOptions(search_states=31, candidates_per_domain=1)
    attempts = []
    def blocked(board, clearance, net, start, end, width, layer, **kwargs):
        allowance = kwargs['state_budget']
        attempts.append(allowance)
        kwargs['on_progress']('surface_bridge_search', 'finished', dict(expanded_states=allowance))
        return None
    monkeypatch.setattr(geometry, 'surface_path_between', blocked)
    import pcbir.pin_escape as escape
    monkeypatch.setattr(escape, 'checked_access_paths', lambda *args, **kwargs: iter(()))
    a, z = Point.mm(3, 5), Point.mm(15, 5)
    pairs = [(0, a, z), (1, Point.mm(3, 8), z)]
    failures, work = [], []
    assert geometry._attachment_branch(b, 'RAIL', CopperLayer.FRONT, pairs, 250000,
        geometry._AreaClearance(b, options), options, options.search_states, 0, failures, work=work) is None
    assert len(attempts) > 1 and sum(attempts) == sum(work) <= options.search_states
    assert {f.start for f in failures} == {p[-2] for p in pairs}


def zone(name, net, x1, x2):
    return CopperZone(name, net, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(x1, 1), Point.mm(x2, 1), Point.mm(x2, 9), Point.mm(x1, 9)))))


def test_source_outside_local_zone_and_separate_islands_are_not_deferred():
    b = board()
    assert deferred_zone_nets(replace(b, zones=(zone('local', 'RAIL', 12, 18),))) == set()
    assert deferred_zone_nets(replace(b, zones=(zone('src', 'RAIL', 1, 5), zone('load', 'RAIL', 12, 18)))) == set()
    assert deferred_zone_nets(replace(b, zones=(zone('shared', 'RAIL', 1, 18),))) == {'RAIL'}


def test_protected_corridor_and_foreign_reference_plane_block_area_generation():
    from pcbir.physical import CopperKeepout
    b = board()
    corridor = CopperKeepout('signal', (CopperLayer.FRONT,), zone('x', 'RAIL', 8, 10).outline,
                            block_tracks=False, block_vias=False, block_zones=True)
    plan = plan_power_areas(b, PowerAreaOptions(protected_corridors=(corridor,), search_states=300, candidates_per_domain=1))
    assert not plan.selected
    b = replace(b, nets=(*b.nets, PhysicalNet('RETURN', ())), zones=(zone('reference', 'RETURN', 1, 18),))
    assert not plan_power_areas(b, PowerAreaOptions(search_states=20, candidates_per_domain=1)).selected


def test_protected_area_broad_phase_preserves_exact_corner_and_clearance_checks(monkeypatch):
    import pcbir.power_geometry as geometry
    from pcbir.geometry import RoundedConvexShape
    b = board()
    b = replace(b, nets=(*b.nets, PhysicalNet('OTHER', ())), zones=(zone('protected', 'OTHER', 8, 10),))
    index = geometry._AreaClearance(b, PowerAreaOptions())
    original = geometry._conflict_geometry
    calls = []
    def counted(shape):
        calls.append(shape)
        return original(shape)
    monkeypatch.setattr(geometry, '_conflict_geometry', counted)
    assert index.can_track('RAIL', Point.mm(3, 5), Point.mm(4, 5), 250000, CopperLayer.FRONT)
    assert not calls  # A distant area needs no polygon construction.
    for x in (7000000, 7600000, 7770000, 7875000, 8000000, 10230000):
        for y in (500000, 775000, 1000000, 2000000, 9000000, 9225000):
            start, end = Point(x, y), Point(x+125000, y+125000)
            expected = index.base.can_track('RAIL', start, end, 250001, CopperLayer.FRONT) and index.area_clear(
                'RAIL', original(RoundedConvexShape((start, end), 125001)), CopperLayer.FRONT)
            assert index.can_track('RAIL', start, end, 250001, CopperLayer.FRONT) == expected
    assert calls


def test_equivalent_translation_keeps_track_geometry_and_status():
    b = board()
    options = PowerAreaOptions(candidates_per_domain=1)
    first = plan_power_areas(b, options)
    dx, dy = 170003, 890001
    shift = lambda p: Point(p.x_nm+dx, p.y_nm+dy)
    moved = replace(b, outline=replace(b.outline, vertices=tuple(shift(p) for p in b.outline.vertices)),
                    placements=tuple(replace(p, position=shift(p.position)) for p in b.placements))
    second = plan_power_areas(moved, options)
    assert first.status == second.status
    assert tuple(replace(t, start=shift(t.start), end=shift(t.end)) for t in first.selected[0].tracks) == second.selected[0].tracks


def test_two_domains_joint_geometry_does_not_silently_overlap():
    b = board()
    # Crossing endpoints on one layer expose contention; the bounded proposal
    # pool may fail, but cannot accept two intersecting distribution trees.
    p1, p2 = replace(b.placements[0], reference='S2', position=Point.mm(9, 2)), replace(b.placements[1], reference='L2', position=Point.mm(9, 8))
    pads = (PadReference('S2', '1'), PadReference('L2', '1'))
    b = replace(b, placements=(*b.placements, p1, p2), nets=(*b.nets, PhysicalNet('OTHER', pads)),
        power_domains=(*b.power_domains, PhysicalPowerDomain('OTHER', pads, pads[:1])))
    plan = plan_power_areas(b, PowerAreaOptions(candidates_per_domain=2, allowed_layers=(CopperLayer.FRONT,)))
    assert plan.status == 'unresolved' and not plan.selected
    assert any(any(r.startswith('conflict:') for r in c.allocation.resources) for c in plan.candidates)


@pytest.mark.parametrize('plane_returns', [(), ('RAIL',)])
def test_conflict_trial_regrows_tree_without_committing_temporary_obstacle(plane_returns):
    b = board()
    pads = (PadReference('S2', '1'), PadReference('L2', '1'))
    b = replace(b, placements=(*b.placements,
        replace(b.placements[0], reference='S2', position=Point.mm(8, 4)),
        replace(b.placements[1], reference='L2', position=Point.mm(12, 6))),
        nets=(*b.nets, PhysicalNet('OTHER', pads)),
        power_domains=(*b.power_domains, PhysicalPowerDomain('OTHER', pads, pads[:1])))
    options = PowerAreaOptions(candidates_per_domain=1, allowed_layers=(CopperLayer.FRONT,),
                               detour_nm=4000000, maximum_conflict_trials=4,
                               plane_return_nets=plane_returns)
    original = plan_power_areas(b, replace(options, maximum_conflict_trials=0))
    assert not original.selected
    plan = plan_power_areas(b, options)
    assert plan.status == 'geometry_proposal' and len(plan.selected) == 2
    assert 0 < len(plan.conflict_trials) <= 4
    assert plan.allocation_states <= options.allocation_states
    assert len(plan.candidates) <= 2+options.maximum_conflict_trials
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    assert len(routed.tracks) == sum(len(c.tracks) for c in plan.selected)
    assert routed.placements == b.placements and routed.nets == b.nets
    assert plan.report()['candidates'][0]['conflicts_with']
    assert physical_board_digest(b) == plan.snapshot
    limited = plan_power_areas(b, replace(options, allocation_states=1))
    assert not limited.selected and limited.exhausted and limited.allocation_states <= 1
    assert apply_power_area_plan(b, limited)[0] is b
    limited = plan_power_areas(b, replace(options, allocation_states=original.allocation_states+1))
    assert len(limited.conflict_trials) == 1 and limited.exhausted and not limited.selected
    assert limited.allocation_states <= original.allocation_states+1
    assert apply_power_area_plan(b, limited)[0] is b


def test_existing_contacts_and_copper_occurrences_are_held():
    b = board()
    track = TrackSegment('RAIL', Point.mm(3, 5), Point.mm(15, 5), 250000, CopperLayer.FRONT)
    b = replace(b, tracks=(track, track))
    plan = plan_power_areas(b, PowerAreaOptions(candidates_per_domain=1))
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'already_contact_routed' and routed is b


@pytest.mark.parametrize('unrelated', [False, True])
def test_three_rails_regrow_against_compatible_owners_without_committing_trials(unrelated):
    b = board()
    extra = (('AA', (3, 9), (15, 9)),) if unrelated else ()
    cases = (('B', (8, 4), (12, 6)), ('C', (6, 6), (10, 4))) + extra
    for name, start, end in cases:
        pads = (PadReference('S'+name, '1'), PadReference('L'+name, '1'))
        b = replace(b, placements=(*b.placements,
            replace(b.placements[0], reference='S'+name, position=Point.mm(*start)),
            replace(b.placements[1], reference='L'+name, position=Point.mm(*end))),
            nets=(*b.nets, PhysicalNet(name, pads)),
            power_domains=(*b.power_domains, PhysicalPowerDomain(name, pads, pads[:1])))
    options = PowerAreaOptions(candidates_per_domain=1, allowed_layers=(CopperLayer.FRONT,),
                               detour_nm=4000000, maximum_conflict_trials=8)
    plan = plan_power_areas(b, options)
    assert len(plan.selected) == 3+int(unrelated) and any(len(owners) > 1 for _, owners in plan.conflict_contexts)
    assert plan.conflict_core_observations
    assert all('AA' not in row['domains'] for row in plan.conflict_core_observations)
    assert plan.allocation_states <= options.allocation_states and len(plan.conflict_trials) <= 8
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    assert len(routed.tracks) == sum(len(c.tracks) for c in plan.selected)
    assert len(routed.vias) == sum(len(c.vias) for c in plan.selected)
    assert routed.placements == b.placements and physical_board_digest(b) == plan.snapshot


def test_pool_uses_drill_clearance_at_drills_not_distant_tracks():
    from pcbir import Via
    from pcbir.power_areas import PowerAreaCandidate
    from pcbir.power_geometry import PhysicalAreaCandidate, _AreaClearance, _candidate_shapes, _allocate_pool
    base = board()
    b = replace(base, placements=(), power_domains=(), net_routing_rules=(),
        nets=(PhysicalNet('A', ()), PhysicalNet('B', ())),
        rules=replace(base.rules, minimum_clearance_nm=100000, minimum_hole_clearance_nm=200000))
    snapshot = physical_board_digest(b)
    def candidate(name, y, vias=()):
        return PhysicalAreaCandidate(PowerAreaCandidate(name, name, snapshot, frozenset({name}), None, (), 1),
            (TrackSegment(name, Point.mm(2, y), Point.mm(10, y), 100000, CopperLayer.FRONT),),
            (), CopperLayer.FRONT, vias=vias)
    via = Via('A', Point.mm(15, 8), 600000, 500000, CopperLayer.FRONT, CopperLayer.BACK)
    a, b_candidate = candidate('A', 2, (via,)), candidate('B', '2.25')
    options = PowerAreaOptions(maximum_domains=2, candidates_per_domain=1)
    def allocate(c):
        pool = (a, c)
        return _allocate_pool(b, pool, {p.allocation.id:_candidate_shapes(b,p) for p in pool},
            _AreaClearance(b, options), {'A','B'}, snapshot, options, 100)[1]
    assert allocate(b_candidate).status == 'geometry_proposal'  # 150 µm copper gap, remote drill.
    near_drill = replace(b_candidate, tracks=(TrackSegment('B', Point.mm(14, '8.46'),
        Point.mm(16, '8.46'), 100000, CopperLayer.FRONT),))
    # 110 µm copper gap passes, but 160 µm drill-to-copper gap fails 200 µm.
    assert not allocate(near_drill).selected


def test_enclosed_tree_void_exports_without_filling_the_hole():
    from shapely.ops import unary_union
    from pcbir.power_geometry import _zones, _zone_geometry
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.drc import run_physical_drc
    points = tuple(Point.mm(x, y) for x, y in ((5, 3), (11, 3), (11, 7), (5, 7)))
    tracks = tuple(TrackSegment('RAIL', p, points[(i+1) % 4], 250000, CopperLayer.FRONT)
                   for i, p in enumerate(points))
    zones, geometry = _zones(tracks, 'loop', 250000)
    assert geometry.interiors and not any(z.outline.holes for z in zones)
    assert unary_union([_zone_geometry(z) for z in zones]).symmetric_difference(geometry).area < 1000000
    candidate = replace(board(), tracks=tracks, zones=zones)
    assert not any(f.code == 'DRC-ZONE-PRIORITY' for f in run_physical_drc(candidate).findings)
    assert KiCadPcbBackend().generate(candidate)


def test_joint_physical_trees_choose_detour_and_preserve_signal_corridor():
    from pcbir.physical import CopperKeepout
    b = board()
    pads = (PadReference('S2', '1'), PadReference('L2', '1'))
    b = replace(b, placements=(*b.placements,
        replace(b.placements[0], reference='S2', position=Point.mm(8, 4)),
        replace(b.placements[1], reference='L2', position=Point.mm(12, 6))),
        nets=(*b.nets, PhysicalNet('OTHER', pads)),
        power_domains=(*b.power_domains, PhysicalPowerDomain('OTHER', pads, pads[:1])))
    corridor = CopperKeepout('signal', (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(2, 1), Point.mm(17, 1), Point.mm(17, 2), Point.mm(2, 2)))))
    options = PowerAreaOptions(candidates_per_domain=3, allowed_layers=(CopperLayer.FRONT,),
                               detour_nm=4000000, protected_corridors=(corridor,))
    plan = plan_power_areas(b, options)
    assert plan.status == 'geometry_proposal' and len(plan.selected) == 2
    assert any(r.startswith('conflict:') for c in plan.candidates for r in c.allocation.resources)
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    from pcbir.power_geometry import _zone_geometry
    assert all(_zone_geometry(z).distance(_zone_geometry(corridor)) >= b.rules.minimum_clearance_nm for z in routed.zones)


def test_inner_distribution_adds_bounded_contacts_without_replacing_source():
    from pcbir import Stackup
    from pcbir.physical import CopperKeepout
    b = board()
    inner = CopperLayer.INTERNAL_2
    b = replace(b, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, inner, CopperLayer.BACK)),
        net_routing_rules=(replace(b.net_routing_rules[0], allowed_layers=(CopperLayer.FRONT, inner), max_vias=2),),
        copper_keepouts=(CopperKeepout('blocked-surface', (CopperLayer.FRONT,), zone('wall', 'RAIL', 8, 10).outline),))
    plan = plan_power_areas(b, PowerAreaOptions(candidates_per_domain=2, search_states=200))
    assert plan.selected and plan.selected[0].layer == inner
    assert len(plan.selected[0].vias) == 2
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    assert routed.copper_keepouts == b.copper_keepouts and routed.placements == b.placements
    assert all(z.layers == (inner,) for z in routed.zones)
    bounded = plan_power_areas(b, PowerAreaOptions(candidates_per_domain=2, search_states=200, maximum_new_contacts=1))
    assert not bounded.selected and ('RAIL', 'contact_budget_exhausted') in bounded.diagnostics


def test_ground_distribution_requires_explicit_external_return_boundary():
    from pcbir.power_geometry import ExternalPowerBoundary
    b = replace(board(), power_domains=())
    options = PowerAreaOptions(nets=('RAIL',), candidates_per_domain=1)
    assert not plan_power_areas(b, options).selected
    options = replace(options, external_boundaries=(ExternalPowerBoundary('RAIL', PadReference('SRC', '1'),
        'return', 'Fixture external supply return connector'),))
    plan = plan_power_areas(b, options)
    assert plan.selected and plan.obligations[0].external_boundaries == options.external_boundaries


def test_blocked_branch_can_change_layer_only_at_existing_contacts():
    from pcbir import Via
    base = board()
    wall = TrackSegment('WALL', Point.mm(9, 0), Point.mm(9, 10), 250000, CopperLayer.FRONT)
    b = replace(base, nets=(*base.nets, PhysicalNet('WALL', ())), tracks=(wall,),
        net_routing_rules=(replace(base.net_routing_rules[0], allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK)),),
        vias=tuple(Via('RAIL', p.position, 600000, 300000, CopperLayer.FRONT, CopperLayer.BACK) for p in base.placements))
    options = PowerAreaOptions(candidates_per_domain=1, maximum_layer_alternatives=1,
        search_states=100, maximum_branch_layers=1, branch_search_states=100)
    assert not plan_power_areas(b, replace(options, maximum_branch_layers=0)).selected
    plan = plan_power_areas(b, options)
    assert plan.selected and {t.layer for t in plan.selected[0].tracks} == {CopperLayer.BACK}
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending' and routed.vias == b.vias
    assert plan.report()['candidates'][0]['layers'] == ['B.Cu']
    assert not plan_power_areas(replace(b, vias=()), options).selected
    forbidden = replace(b, net_routing_rules=base.net_routing_rules)
    assert not plan_power_areas(forbidden, options).selected
    protected = replace(b, zones=(replace(zone('reference', 'WALL', 1, 18), layers=(CopperLayer.BACK,)),))
    assert not plan_power_areas(protected, options).selected


def test_multilayer_tree_zones_keep_layers_and_count_both_areas():
    from pcbir.power_geometry import _zones
    first = TrackSegment('RAIL', Point.mm(3, 5), Point.mm(15, 5), 250000, CopperLayer.FRONT)
    second = replace(first, layer=CopperLayer.BACK)
    _, single = _zones((first,), 'one', 100000)
    zones, geometry = _zones((first,second), 'two', 100000)
    assert {z.layers for z in zones} == {(CopperLayer.FRONT,), (CopperLayer.BACK,)}
    assert len({z.id for z in zones}) == len(zones)
    assert geometry.area == pytest.approx(2*single.area)


def test_passive_feed_lowering_keeps_upstream_obligations_and_rejects_cycle():
    from test_passive_dc_power import compile_source, SOURCE
    from pcbir import prototype_physicalize
    source_with_footprints = SOURCE.replace('component F1: Feed;', 'component F1: Feed { footprint = "feed"; }').replace(
        'component F2: Feed;', 'component F2: Feed { footprint = "feed"; }').replace(
        'component U: Load;', 'component U: Load { footprint = "load"; }')
    source_with_footprints = source_with_footprints.replace(': Feed {', ': p.Feed {').replace(': Load {', ': p.Load {')
    physical = prototype_physicalize(compile_source(source_with_footprints))
    feeds = {f.net: f for f in physical.power_feeds}
    assert feeds['FILTERED'].terminal == PadReference('F2', '2')
    assert feeds['FILTERED'].upstream_net == 'MID'
    assert feeds['MID'].upstream_net == 'SOURCE'
    assert all(not d.sources for d in physical.power_domains)
    source = source_with_footprints.replace('supply SOURCE { voltage = 1.8 V; external = true; }', '')
    assert not prototype_physicalize(compile_source(source)).power_feeds

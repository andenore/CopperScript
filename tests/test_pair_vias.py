from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, Stackup,
    nm_from_mm, route_global, route_critical_nets, run_physical_drc)
from pcbir.critical import _route_pair, _validate_candidate
from pcbir.pair_search import PairSearchStats, paired_candidates
from pcbir.pair_vias import paired_via_candidates, _signal_drills_outside_smd
from pcbir.physical import Via, ComponentPlacementRule


def board_fixture(returns=False):
    fp = PhysicalFootprint('pair', tuple(FootprintPad(str(i+1),Point.mm(0,y),Size.mm(.4,.4))
        for i,y in enumerate((-.5,.5))),Size.mm(1,2))
    rules = tuple(NetRoutingRule(name,RouteKind.DIFFERENTIAL,
        differential_partner=partner,width_nm=nm_from_mm(.25),pair_gap_nm=nm_from_mm(.2),
        allowed_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_2),max_vias=2,
        require_return_vias=returns,return_via_net='GND' if returns else None,
        maximum_return_via_distance_nm=nm_from_mm(1.5) if returns else None)
        for name,partner in (('A','B'),('B','A')))
    wall=CopperKeepout('top-wall',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((
        Point.mm(19,0),Point.mm(21,0),Point.mm(21,25),Point.mm(19,25)))))
    return PhysicalBoard('PairVias',BoardOutline.rectangle(40,25),{fp.name:fp},
        (Placement('J1',fp.name,Point.mm(5,12)),Placement('J2',fp.name,Point.mm(35,12))),
        (*tuple(PhysicalNet(name,(PadReference('J1',str(i+1)),PadReference('J2',str(i+1))))
                for i,name in enumerate(('A','B'))),PhysicalNet('GND',())),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,
                                      CopperLayer.INTERNAL_2,CopperLayer.BACK)),
        net_routing_rules=rules,copper_keepouts=(wall,))


@pytest.mark.parametrize('returns',(False,True))
def test_terminal_vias_cross_top_wall_jointly_and_deterministically(returns):
    board=board_fixture(returns)
    guides=route_global(board)
    rules=board.net_routing_rules
    routes={r.net:r for r in guides.routes}
    stats=PairSearchStats()
    def propose():
        return next(paired_via_candidates(board,*rules,routes['A'],routes['B'],stats=stats))
    candidate=propose()
    assert candidate==propose()
    assert len(candidate.via_pairs)==2
    assert len(candidate.return_vias)==(2 if returns else 0)
    assert all(t.layer in rules[0].allowed_layers for t in (*candidate.first,*candidate.second))
    assert any(t.layer is CopperLayer.INTERNAL_2 for t in candidate.first)
    assert any(t.layer is CopperLayer.FRONT for t in candidate.first)
    assert all(v.from_layer is CopperLayer.FRONT and v.to_layer is CopperLayer.BACK
               for pair in candidate.via_pairs for v in pair)
    proposed,tracks,vias=_route_pair(board,*rules,routes,
        exact_tracks=(candidate.first,candidate.second),exact_via_pairs=candidate.via_pairs,
        exact_return_vias=candidate.return_vias)
    accepted,tracks,vias=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.paired_via_transitions==2
    assert accepted.strategy=='joint_pair_via_search'
    assert accepted.via_count==(6 if returns else 4)
    routed=replace(board,tracks=tracks,vias=vias)
    assert not [f for f in run_physical_drc(routed).findings
                if f.severity.value=='error' and f.code!='DRC-ROUTE-INCOMPLETE']
    assert board.tracks==board.vias==()


@pytest.mark.parametrize('failure',('budget','surface_only','all_layer_wall','return_distance'))
def test_paired_via_search_fails_closed(failure):
    board=board_fixture(returns=failure=='return_distance')
    if failure=='budget':
        board=replace(board,net_routing_rules=tuple(replace(r,max_vias=1) for r in board.net_routing_rules))
    elif failure=='surface_only':
        board=replace(board,net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,))
            for r in board.net_routing_rules))
    elif failure=='all_layer_wall':
        board=replace(board,copper_keepouts=tuple(replace(k,layers=board.stackup.copper_layers)
            for k in board.copper_keepouts))
    else:
        board=replace(board,net_routing_rules=tuple(replace(r,maximum_return_via_distance_nm=nm_from_mm(.1))
            for r in board.net_routing_rules))
    routes={r.net:r for r in route_global(board).routes}
    assert not list(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B'],
                                         maximum_searches=2,maximum_states=100))
    assert board.tracks==board.vias==()


def test_critical_owner_accepts_via_pair_without_surface_refinement(monkeypatch):
    # The surface engine has its own wall/clearance tests. Isolate the owner
    # integration rather than exhaust three surface maze budgets here.
    monkeypatch.setattr('pcbir.critical.paired_candidates',lambda *a,**k:iter(()))
    board=board_fixture(True)
    result=route_critical_nets(board,route_global(board))
    pair=result.nets[0]
    assert pair.connected and pair.strategy=='joint_pair_via_search'
    assert pair.paired_via_transitions==2 and pair.return_via_count==2
    assert pair.pair_refinement_attempts==0


@pytest.mark.parametrize('distance,legal',((0,False),(.35,False),(.5,True)))
def test_signal_drills_cannot_overlap_their_own_smd_land(distance,legal):
    board=board_fixture()
    pair=tuple(Via(net,Point.mm(5+distance,y),nm_from_mm(.8),nm_from_mm(.4))
               for net,y in (('A',11.5),('B',12.5)))
    assert _signal_drills_outside_smd(board,pair) is legal


def test_diagonal_terminal_rows_keep_matched_transition_geometry():
    board=board_fixture()
    board=replace(board,placements=tuple(replace(p,rotation_degrees=45) for p in board.placements),
        placement_rules=tuple(ComponentPlacementRule(p.reference,allowed_orientations=(45,))
                              for p in board.placements))
    routes={r.net:r for r in route_global(board).routes}
    candidate=next(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
        exact_tracks=(candidate.first,candidate.second),exact_via_pairs=candidate.via_pairs)
    accepted,_,_=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.paired_via_transitions==2


def test_surface_alternative_uses_actual_pad_layer_not_tentative_guide_access():
    board=replace(board_fixture(),copper_keepouts=())
    routes={r.net:replace(r,accesses=tuple(replace(a,layer=CopperLayer.INTERNAL_2)
                                         for a in r.accesses)) for r in route_global(board).routes}
    candidate=next(paired_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    assert all(t.layer is CopperLayer.FRONT for t in (*candidate.first,*candidate.second))
    proposed,tracks,vias=_route_pair(board,*board.net_routing_rules,routes,
                                   exact_tracks=(candidate.first,candidate.second))
    accepted,_,_=_validate_candidate(board,proposed,tracks,vias,[],[])
    assert accepted.connected and accepted.via_count==0


@pytest.mark.parametrize('fault',('missing_return','asymmetric_span','floating_track','clearance','budget'))
def test_critical_owner_rejects_corrupted_paired_transition_atomically(fault):
    board=board_fixture(True)
    routes={r.net:r for r in route_global(board).routes}
    candidate=next(paired_via_candidates(board,*board.net_routing_rules,routes['A'],routes['B']))
    pairs=candidate.via_pairs
    returns=candidate.return_vias
    tracks=(candidate.first,candidate.second)
    if fault=='missing_return':
        returns=()
    elif fault=='asymmetric_span':
        pairs=((pairs[0][0],replace(pairs[0][1],to_layer=CopperLayer.INTERNAL_2)),pairs[1])
    elif fault=='floating_track':
        tracks=(tuple(t for t in tracks[0] if t.layer is not CopperLayer.INTERNAL_2),tracks[1])
    elif fault=='clearance':
        pairs=((pairs[0][0],replace(pairs[0][1],position=pairs[0][0].position)),pairs[1])
    else:
        board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0) for r in board.net_routing_rules))
    proposed,created,vias=_route_pair(board,*board.net_routing_rules,routes,exact_tracks=tracks,
                                    exact_via_pairs=pairs,exact_return_vias=returns)
    rejected,created,vias=_validate_candidate(board,proposed,created,vias,[],[])
    assert not rejected.connected and rejected.candidate_rejected
    assert created==vias==()


@pytest.mark.parametrize("engine", (paired_candidates, paired_via_candidates))
def test_aggregate_state_cap_is_shared_across_candidate_calls(monkeypatch, engine):
    board = replace(board_fixture(), copper_keepouts=())
    routes = {r.net: r for r in route_global(board).routes}
    stats, budgets = PairSearchStats(), []
    def exhausted(*args):
        budgets.append(args[-2])
        args[-1].expanded_states += args[-2]
        return None
    module = "pcbir.pair_search" if engine is paired_candidates else "pcbir.pair_vias"
    monkeypatch.setattr(module + "._search", exhausted)
    assert not list(engine(board, *board.net_routing_rules, routes['A'], routes['B'],
        stats=stats, maximum_states=4, maximum_total_states=7))
    assert budgets == [4, 3] and stats.expanded_states == 7
    # Reusing the telemetry shares the cap, rather than silently resetting it.
    assert not list(engine(board, *board.net_routing_rules, routes['A'], routes['B'],
        stats=stats, pitch_nm=500_000, maximum_total_states=7))
    assert budgets == [4, 3] and board.tracks == board.vias == ()
    with pytest.raises(ValueError, match="aggregate"):
        list(engine(board, *board.net_routing_rules, routes['A'], routes['B'], maximum_total_states=0))


def test_real_pair_search_honors_tiny_budget_without_committing_copper():
    board = replace(board_fixture(), copper_keepouts=())
    routes = {r.net: r for r in route_global(board).routes}
    stats = PairSearchStats()
    assert not list(paired_candidates(board, *board.net_routing_rules, routes['A'], routes['B'],
        maximum_total_states=1, stats=stats))
    assert stats.expanded_states == 1 and board.tracks == board.vias == ()


def test_critical_limited_tier_samples_both_families_and_all_pitches(monkeypatch):
    budgets = []
    def exhausted(*args, **kwargs):
        stats = kwargs['stats']
        budgets.append((kwargs['pitch_nm'], kwargs['maximum_total_states'] - stats.expanded_states))
        stats.expanded_states = kwargs['maximum_total_states']
        stats.searches += 1
        return iter(())
    monkeypatch.setattr('pcbir.critical.paired_candidates', exhausted)
    monkeypatch.setattr('pcbir.critical.paired_via_candidates', exhausted)
    board = board_fixture()
    result = route_critical_nets(board, route_global(board), pair_state_limit=60)
    assert budgets == [(pitch, 10) for _ in range(2) for pitch in (1_000_000, 500_000, 250_000)]
    pair = result.nets[0]
    assert not pair.connected and pair.search_states == 60
    assert pair.pair_state_limit == 60 and pair.pair_budget_exhausted
    assert result.board.tracks == result.board.vias == ()
    with pytest.raises(ValueError, match="aggregate"):
        route_critical_nets(board, route_global(board), pair_state_limit=0)


def test_easy_via_pair_is_still_exactly_accepted_with_initial_cap(monkeypatch):
    monkeypatch.setattr('pcbir.critical.paired_candidates', lambda *a, **k: iter(()))
    board = board_fixture(True)
    # A short real pair fits the exploratory slices. The longer retained
    # corridor needs 1,574 states for its first candidate and exercises the
    # historical full-budget route in the existing owner test instead.
    wall = CopperKeepout('short-wall', (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(8, 0), Point.mm(10, 0), Point.mm(10, 25), Point.mm(8, 25)))))
    board = replace(board, placements=(board.placements[0],
        replace(board.placements[1], position=Point.mm(12, 12))), copper_keepouts=(wall,))
    result = route_critical_nets(board, route_global(board), pair_state_limit=6000)
    pair = result.nets[0]
    assert pair.connected and pair.paired_via_transitions == 2 and pair.return_via_count == 2
    assert pair.search_states <= 6000 and not pair.pair_budget_exhausted
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value == 'error' and f.code != 'DRC-ROUTE-INCOMPLETE']


def test_actual_package_preflight_full_fallback_recovers_long_pair(monkeypatch):
    from pcbir import PackageAccessOptions, preflight_package_access
    from pcbir.fanout import FanoutOptions
    # Suppress futile surface search only in this retained top-wall fixture;
    # vias, actual joint maze search, coupling, DRC and acceptance remain real.
    monkeypatch.setattr('pcbir.critical.paired_candidates', lambda *a, **k: iter(()))
    board = board_fixture(True)
    result = preflight_package_access(board, route_global(board), FanoutOptions(),
                                     options=PackageAccessOptions(maximum_trials=0))
    assert result.ready and not result.hard_findings
    assert len(result.search_tiers) == 2
    assert not result.search_tiers[0].ready and result.search_tiers[1].ready
    assert result.search_tiers[1].selected and result.search_tiers[1].pair_state_limit is None
    pair = result.critical.nets[0]
    assert pair.connected and pair.paired_via_transitions == 2 and pair.return_via_count == 2
    assert board.tracks == board.vias == ()


@pytest.mark.parametrize("initial,patterns", ((0, 2), (6000, 0)))
def test_paired_preflight_disable_modes_keep_historical_budget(monkeypatch, initial, patterns):
    from pcbir import PackageAccessOptions, preflight_package_access
    from pcbir.fanout import FanoutOptions
    monkeypatch.setattr('pcbir.critical.paired_candidates', lambda *a, **k: iter(()))
    board = board_fixture(True)
    result = preflight_package_access(board, route_global(board), FanoutOptions(),
        options=PackageAccessOptions(maximum_trials=0, initial_pair_state_limit=initial,
                                     maximum_pattern_trials=patterns))
    assert result.ready and len(result.search_tiers) == 1
    assert result.search_tiers[0].name == "full" and result.search_tiers[0].selected
    assert result.critical.nets[0].pair_state_limit is None

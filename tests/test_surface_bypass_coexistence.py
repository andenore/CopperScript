"""Bounded surface/bypass coexistence with real geometry."""
from collections import Counter
from dataclasses import fields, replace
from decimal import Decimal
from math import hypot

import pytest

from pcbir.boundary_access import BoundaryAccessOptions, reserve_boundary_access
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling import route_decouplers
from pcbir.detailed import route_detailed
from pcbir.drc import explicit_copper_connectivity, run_physical_drc
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import (PackageAccessOptions, preflight_package_access,
    _verified_bypass_links)
from pcbir.physical import Point
from pcbir.pin_escape import verified_routing_access
from pcbir.routing import route_global
from pcbir.routing_clearance import RoutingClearanceIndex
import pcbir.surface_bypass_coexistence as proposal
from surface_bypass_fixture import build


def scene(renamed=False):
    source, link, signals = build(rotated_renamed=renamed)
    if renamed:
        inverse = lambda p: Point(p.y_nm, 25_000_000-p.x_nm)
        source = replace(source,
            placements=tuple(replace(p,position=inverse(p.position),rotation_degrees=Decimal(0))
                             for p in source.placements),
            placement_rules=tuple(replace(r,fixed_position=inverse(r.fixed_position)
                if r.fixed_position is not None else None) for r in source.placement_rules))
    return source, link, signals


def hard(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


@pytest.mark.parametrize("renamed", (False, True))
def test_public_pattern_preserves_both_full_nets_and_actual_return(renamed, monkeypatch):
    source, link, signals = scene(renamed)
    guides = route_global(source)
    calls = []
    import pcbir.package_access as access
    real_fanout = access.route_fanout
    def observe(*args, **kwargs):
        calls.append((args,kwargs))
        return real_fanout(*args, **kwargs)
    monkeypatch.setattr(access,"route_fanout",observe)
    result = preflight_package_access(source,guides,FanoutOptions(),
        options=PackageAccessOptions(maximum_trials=0,maximum_pattern_trials=1))
    assert len(calls) == 1  # Initial production fanout only; none per alternative.
    assert result.ready and len(result.pattern_trials) == 1
    assert result.pattern_trials[0].strategy == "surface_bypass_first"
    assert result.pattern_trials[0].outcome == "accepted"
    assert not result.accepted_moves and result.source is source
    assert result.board.placements == source.placements
    assert link in _verified_bypass_links(result.fanout.board,{link.net})
    assert not result.fanout.decoupling_witnesses  # No invented same-face return certificate.
    fanout = reserve_boundary_access(result.board,result.fanout,result.boundary)
    selected=frozenset((*(n.name for n in signals),link.return_net))
    detailed=route_detailed(fanout.board,guides,
        fanout_accesses=fanout.routing_accesses,
        fanout_created_tracks=fanout.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in fanout.created_vias),
        only_nets=selected)
    assert detailed.status.value == "success" and not hard(detailed.board)
    graph=explicit_copper_connectivity(detailed.board,include_internal_connections=False)
    assert all(len(n.pads)==3 and graph.net_connected(n) for n in signals)
    returning=next(n for n in source.nets if n.name==link.return_net)
    assert {p.pad for p in returning.pads if p.component==link.target.component} == {"12","25"}
    assert link.return_pad in returning.pads and graph.net_connected(returning)
    assert all(graph.net_connected(n) for n in source.nets)
    assert not any(v.net in {n.name for n in signals} for v in detailed.board.vias)
    assert tuple(t for t in detailed.board.tracks if t.net==link.net) == tuple(
        t for t in result.fanout.board.tracks if t.net==link.net)
    assert all(getattr(source,f.name)==getattr(detailed.board,f.name) for f in fields(source)
               if f.name not in {"tracks","vias","metadata"})


def test_one_domain_batch_one_assignment_one_feed_call_no_full_fanout(monkeypatch):
    source,link,signals=scene();original=route_fanout(source)
    real_domain=proposal.surface_escapes;real_joint=proposal.improve_escape_assignment
    real_feed=proposal.route_decouplers;counts={"domains":[],"joint":0,"feed":0}
    def domain(*args,**kwargs):
        counts["domains"].append((args[2],kwargs.get("refined",False)))
        return real_domain(*args,**kwargs)
    def joint(*args,**kwargs):
        counts["joint"]+=1
        return real_joint(*args,**kwargs)
    def feed(*args,**kwargs):
        counts["feed"]+=1
        assert kwargs["only_links"]==frozenset((link,))
        assert args[0].decoupling_links == source.decoupling_links
        assert args[0].metadata==source.metadata and args[0].placements==source.placements
        return real_feed(*args,**kwargs)
    monkeypatch.setattr(proposal,"surface_escapes",domain)
    monkeypatch.setattr(proposal,"improve_escape_assignment",joint)
    monkeypatch.setattr(proposal,"route_decouplers",feed)
    monkeypatch.setattr("pcbir.fanout.route_fanout",lambda *a,**k:pytest.fail("hidden full fanout"))
    before=repr(original);answer=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions())
    assert answer.fanout is not None and answer.reason=="proposed"
    assert counts["joint"]==counts["feed"]==1
    assert Counter(counts["domains"])==Counter((n.name,refined) for n in signals for refined in (False,True))
    assert repr(original)==before and not source.tracks and not source.vias
    assert verified_bypass_reservations(source,answer.fanout)[2] is None
    index=RoutingClearanceIndex(answer.fanout.board)
    assert all(verified_routing_access(answer.fanout.board,n.pads[0],n.name,
        answer.fanout.surface_accesses[n.pads[0]],index) is not None for n in signals)
    held=tuple(t for t in original.created_tracks if all(t is not r for r in answer.released_tracks))
    assert all(any(t is found for found in answer.fanout.created_tracks) for t in held)
    assert all(any(v is found for found in answer.fanout.created_vias) for v in original.created_vias)


def test_real_whole_net_length_rejects_regeneration_and_rolls_back():
    source,link,_=scene()
    source=replace(source,net_routing_rules=tuple(replace(r,max_length_nm=6_400_000)
        if r.net==link.net else r for r in source.net_routing_rules))
    original=route_fanout(source)
    assert link in _verified_bypass_links(original.board,{link.net})
    old_length=sum(round(hypot(t.end.x_nm-t.start.x_nm,t.end.y_nm-t.start.y_nm))
                   for t in original.board.tracks if t.net==link.net)
    assert old_length < 6_400_000
    before=repr(original);board=original.board
    answer=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions())
    assert answer.fanout is None and answer.reason=="selected_surface_feed_regeneration_failed"
    assert original.board is board and repr(original)==before
    assert link in _verified_bypass_links(original.board,{link.net})
    assert not hard(original.board)


@pytest.mark.parametrize("fault",("pose","rule","source_metadata","count","equal_nonoriginal","duplicate"))
def test_stale_or_ambiguous_owner_rejected_before_generation(fault,monkeypatch):
    source,_,_=scene();original=route_fanout(source)
    if fault=="pose":
        bad=replace(original,board=replace(original.board,placements=(
            replace(source.placements[0],position=Point.mm(11,10)),*source.placements[1:])))
    elif fault=="rule":bad=replace(original,board=replace(original.board,net_routing_rules=()))
    elif fault=="source_metadata":bad=replace(original,board=replace(original.board,metadata={"changed":"true"}))
    elif fault=="count":bad=replace(original,added_track_count=original.added_track_count+1)
    elif fault=="equal_nonoriginal":bad=replace(original,decoupling_tracks=tuple(replace(t) for t in original.decoupling_tracks))
    else:
        edge=original.decoupling_tracks[0]
        bad=replace(original,created_tracks=(*original.created_tracks,edge),
            board=replace(original.board,tracks=(*original.board.tracks,edge)),
            added_track_count=original.added_track_count+1)
    monkeypatch.setattr(proposal,"route_decouplers",lambda *a,**k:pytest.fail("generation before admission"))
    before=repr(bad)
    assert proposal.propose_surface_bypass(source,bad,BoundaryAccessOptions()).fanout is None
    assert repr(bad)==before


def test_probe_metadata_normalized_before_generation_but_source_intent_not_rebound():
    source,_,_=scene();original=route_fanout(source)
    obstacles=replace(original.board,metadata={**source.metadata,"critical_routing":"output"})
    accepted=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions(),obstacles=obstacles)
    assert accepted.fanout is not None and accepted.fanout.board.metadata==source.metadata
    wrong=replace(obstacles,net_routing_rules=())
    before=repr(original)
    rejected=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions(),obstacles=wrong)
    assert rejected.fanout is None and rejected.reason=="stale_surface_obstacle_context"
    assert repr(original)==before


def test_zero_pattern_budget_does_not_run_new_domains_or_hide_pending(monkeypatch):
    source,_,signals=scene();guides=route_global(source)
    monkeypatch.setattr(proposal,"surface_escapes",lambda *a,**k:pytest.fail("zero pattern budget"))
    result=preflight_package_access(source,guides,FanoutOptions(),
        options=PackageAccessOptions(maximum_trials=0,maximum_pattern_trials=0))
    assert not result.ready and not result.pattern_trials
    assert {n.pads[0] for n in signals} <= set(result.fanout.pending_pads)


@pytest.mark.parametrize("renamed",(False,True))
def test_original_shared_path_ledger_limits_exclusive_release(renamed):
    # Actual established fixture: later canonical paths omit original shared
    # copper. The release predicate must use the canonical+ORIGINAL union.
    from test_original_bypass_shared_ownership import scene as shared_scene
    source,original,old,inverse,_,_=shared_scene(renamed)
    nets={l.net for l in source.decoupling_links};canonical,canonical_vias={},{}
    _verified_bypass_links(original.board,nets,witness_paths=canonical,witness_vias=canonical_vias)
    paths,vias={},{}
    required=_verified_bypass_links(original.board,nets,witness_paths=paths,witness_vias=vias,
        admitted_witnesses=original.decoupling_witnesses)
    assert all(v is not old for vs in canonical_vias.values() for v in vs)
    assert any(v is old for v in vias[inverse])
    selected=original.decoupling_witnesses[0].link
    released=proposal._exclusive_feed_tracks(original,selected,required,paths,vias)
    protected={id(t) for other in required if other!=selected for t in paths.get(other,())}
    assert not {id(t) for t in released} & protected
    assert all(any(t is source_t for source_t in original.decoupling_tracks) for t in released)
    before=repr(original)
    answer=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions())
    assert answer.fanout is None  # No eligible same-face constrained cohort here.
    assert repr(original)==before and any(v is old for v in original.board.vias)


@pytest.mark.parametrize("required_array",(False,True))
def test_coexistence_preserves_materialized_macro_and_exact_required_array(tmp_path,required_array):
    from pcbir.physical import BoardOutline,FootprintPad,PhysicalFootprint,Size
    from pcbir.hard_macros import materialize_hard_macros,validate_hard_macros
    from pcbir.pad_via_arrays import require_via_in_pad_arrays,via_in_pad_array_vias
    from test_autonomous_plane_access import with_test_owners
    source,_,signals=scene()
    # This independent source variant translates the entire fixture BEFORE any
    # operator. It leaves room for the existing real synthetic macro/array owner
    # fixture at its authored positions; no candidate pose is changed.
    move=lambda p:Point(p.x_nm+20_000_000,p.y_nm)
    terminal=PhysicalFootprint("terminal",(FootprintPad("1",Point(0,0),Size.mm(.3,.3)),),Size.mm(.8,.8))
    source=replace(source,outline=BoardOutline.rectangle(50,25),
        footprints={**source.footprints,"terminal":terminal},
        placements=tuple(replace(p,position=move(p.position)) for p in source.placements),
        placement_rules=tuple(replace(r,fixed_position=move(r.fixed_position)
            if r.fixed_position is not None else None) for r in source.placement_rules))
    owner=materialize_hard_macros(with_test_owners(source,tmp_path,required_array=required_array))
    assert not hard(owner)
    original=route_fanout(owner)
    assert {n.pads[0] for n in signals}<=set(original.pending_pads)
    answer=proposal.propose_surface_bypass(owner,original,BoundaryAccessOptions())
    assert answer.fanout is not None and not hard(answer.fanout.board)
    final=answer.fanout.board
    assert all(a is b for a,b in zip(owner.tracks,final.tracks))
    assert all(a is b for a,b in zip(owner.vias,final.vias))
    assert owner.placements==final.placements and owner.hard_macros==final.hard_macros
    validate_hard_macros(final)
    require_via_in_pad_arrays(final,"surface coexistence regression")
    if required_array:
        arrays=Counter(via_in_pad_array_vias(owner));assert sum(arrays.values())==4
        actual=Counter(final.vias)
        assert all(actual[v]==count for v,count in arrays.items())
    assert verified_bypass_reservations(owner,answer.fanout)[2] is None


def test_lost_unrelated_real_critical_owner_rejects_entire_pattern(monkeypatch):
    """Actual baseline owner, followed by an adversarial missing-edge result."""
    import pcbir.package_access as access
    from pcbir.physical import (FootprintPad,PhysicalFootprint,PhysicalNet,
        PadReference,Placement,Size,NetRoutingRule,RouteKind,CopperLayer)
    source,_,_=scene()
    fp=PhysicalFootprint("quiet-terminal",(FootprintPad("1",Point(0,0),Size.mm(.5,.5)),),Size.mm(1,1))
    source=replace(source,footprints={**source.footprints,fp.name:fp},
        placements=(*source.placements,Placement("Q0",fp.name,Point.mm(20,20)),
                    Placement("Q1",fp.name,Point.mm(22,20))),
        nets=(*source.nets,PhysicalNet("QUIET_CRITICAL",(PadReference("Q0","1"),PadReference("Q1","1")))),
        net_routing_rules=(*source.net_routing_rules,NetRoutingRule("QUIET_CRITICAL",RouteKind.CRITICAL,
            width_nm=200000,allowed_layers=(CopperLayer.FRONT,),max_vias=0)))
    guides=route_global(source)
    baseline=preflight_package_access(source,guides,FanoutOptions(),
        options=PackageAccessOptions(maximum_trials=0,maximum_pattern_trials=0))
    assert any(n.connected and n.nets==("QUIET_CRITICAL",) for n in baseline.critical.nets)
    old=tuple(t for t in baseline.board.tracks if t.net=="QUIET_CRITICAL");assert old
    assert not baseline.hard_findings and baseline.fanout.pending_pads
    real=access._critical_search
    def missing_edge(*args,**kwargs):
        result=real(*args,**kwargs)
        # This is deliberate adversarial output corruption, never positive
        # connectivity evidence or a solver-policy replacement.
        return replace(result,board=replace(result.board,
            tracks=tuple(t for t in result.board.tracks if t.net!="QUIET_CRITICAL")))
    monkeypatch.setattr(access,"_critical_search",missing_edge)
    before=repr(baseline)
    answer=access._surface_bypass_pattern_trial(baseline,source,FanoutOptions(),
        PackageAccessOptions(maximum_trials=0,maximum_pattern_trials=1),None,None,None,None,"full")
    assert answer.pattern_trials[0].outcome=="unrelated_owner_not_retained"
    assert answer.fanout is baseline.fanout and answer.critical is baseline.critical
    assert repr(baseline)==before
    assert all(any(t is edge for edge in answer.board.tracks) for t in old)


def test_current_fanout_and_boundary_computational_bounds_are_not_increased(monkeypatch):
    from pcbir.escape_assignment import EscapeAssignmentOptions
    source,_,_=scene();original=route_fanout(source)
    limits=EscapeAssignmentOptions(maximum_trials=1,maximum_cluster_pins=2,
        maximum_search_states=17,maximum_pair_checks=19,maximum_pair_queries=23)
    settings=FanoutOptions(assignment_options=limits,maze_state_budget=29)
    real_joint=proposal.improve_escape_assignment;real_domain=proposal.surface_escapes
    seen=[]
    def joint(*args,**kwargs):
        seen.append(args[5]);assert args[5]==limits
        return real_joint(*args,**kwargs)
    def domain(*args,**kwargs):
        assert args[6].maze_state_budget==29
        return real_domain(*args,**kwargs)
    monkeypatch.setattr(proposal,"improve_escape_assignment",joint)
    monkeypatch.setattr(proposal,"surface_escapes",domain)
    answer=proposal.propose_surface_bypass(source,original,BoundaryAccessOptions(),fanout_options=settings)
    assert seen==[limits]
    assert answer.fanout is not None  # Greedy direct ports require no maze/CSP expansion.

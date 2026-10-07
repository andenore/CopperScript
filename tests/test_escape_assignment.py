"""Conflict-graph exactness and bounded compatible allocation/rollback."""
from dataclasses import replace
from pathlib import Path
import shutil

import pytest
import pcbir.fanout as fanout
from pcbir.drc import run_physical_drc
from pcbir.escape_assignment import EscapeAssignmentOptions, EscapeConflicts, improve_escape_assignment
from pcbir.physical import (BoardOutline, CopperLayer, FootprintPad, NetRoutingRule,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    Size, Stackup, TrackSegment, Via, nm_from_mm)


def board():
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0,-1), Size.mm(.3,.3)),
        FootprintPad("2", Point.mm(0,1), Size.mm(.3,.3))), Size.mm(2,3))
    one = PhysicalFootprint("one", (FootprintPad("1",Point(0,0),Size.mm(.3,.3)),),Size.mm(1,1))
    return PhysicalBoard("assignment", BoardOutline.rectangle(14,14),
        {"two":package,"one":one}, (Placement("U","two",Point.mm(5,7)),
        Placement("JA","one",Point.mm(11,6)),Placement("JB","one",Point.mm(11,8))),
        (PhysicalNet("A",(PadReference("U","1"),PadReference("JA","1"))),
         PhysicalNet("B",(PadReference("U","2"),PadReference("JB","1")))))


def opts(**kw):
    return fanout.FanoutOptions(minimum_component_pads=2,
        maximum_neighbor_distance_nm=nm_from_mm(3), **kw)


def trap_candidates(position, center, settings):
    # Both domains have two choices: static scarcity ordering cannot fix this.
    return (Point.mm(3,7),Point.mm(3,4)) if position.y_nm == nm_from_mm(6) else (
        Point.mm(3,7),Point.mm(3,7.5))


def test_joint_assignment_repairs_equal_slack_trap_without_losing_incumbent(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout,"_candidates",trap_candidates)
    greedy = fanout.route_fanout(base,opts(two_leg_escapes=False,joint_escapes=False))
    joint = fanout.route_fanout(base,opts(two_leg_escapes=False))
    assert greedy.pending_pads == (PadReference("U","2"),)
    assert not joint.pending_pads
    assert set(joint.accesses).issuperset(greedy.accesses)
    assert joint.accesses[PadReference("U","1")] == Point.mm(3,4)
    assert joint.assignment.trials[0].solution_found and joint.assignment.native_accepted
    assert joint == fanout.route_fanout(base,opts(two_leg_escapes=False))
    assert not any(f.code in {"DRC-SHORT","DRC-CLEARANCE","DRC-DRILL-SPACING"}
                   for f in run_physical_drc(joint.board).findings)


def test_competing_pin_with_legal_radial_choice_gets_off_ray_alternative(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout,"_candidates",lambda p,*args:(Point.mm(3,7),))
    # Each radial choice exists individually; A's first choice blocks B.
    monkeypatch.setattr(fanout,"_two_leg_candidates",lambda p,*args:
                        (Point.mm(3,4),) if p.y_nm == nm_from_mm(6) else ())
    greedy = fanout.route_fanout(base,opts(joint_escapes=False))
    joint = fanout.route_fanout(base,opts())
    assert greedy.pending_pads and not joint.pending_pads
    assert PadReference("U","1") in joint.assignment.expanded_pads
    assert joint.accesses[PadReference("U","1")] == Point.mm(3,4)


@pytest.mark.parametrize("budget", ["maximum_cluster_pins","maximum_search_states","maximum_pair_checks", "maximum_pair_queries"])
def test_budget_exhaustion_preserves_greedy_geometry(monkeypatch,budget):
    base = board()
    monkeypatch.setattr(fanout,"_candidates",trap_candidates)
    greedy = fanout.route_fanout(base,opts(two_leg_escapes=False,joint_escapes=False))
    joint = fanout.route_fanout(base,opts(two_leg_escapes=False,
        assignment_options=EscapeAssignmentOptions(**{budget:1})))
    assert joint.board == greedy.board and joint.accesses == greedy.accesses
    assert not any(t.solution_found for t in joint.assignment.trials)
    assert "budget" in joint.assignment.trials[0].diagnostic


def test_locked_subset_copper_is_not_a_replaceable_escape(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout,"_candidates",trap_candidates)
    installed = fanout.route_fanout(base,opts(two_leg_escapes=False),only_nets=frozenset({"A"}))
    trial = fanout.route_fanout(installed.board,opts(two_leg_escapes=False),only_nets=frozenset({"B"}))
    assert trial.board is installed.board and trial.pending_pads == (PadReference("U","2"),)
    assert not trial.created_tracks and not trial.created_vias and not trial.assignment.trials


def test_exact_whole_board_gate_rolls_failed_assignment_back_to_greedy(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout,"_candidates",trap_candidates)
    greedy = fanout.route_fanout(base,opts(two_leg_escapes=False,joint_escapes=False))
    original = fanout.run_physical_drc
    from types import SimpleNamespace
    def gate(b):
        if any(t.net == "B" for t in b.tracks):
            return SimpleNamespace(findings=(SimpleNamespace(code="DRC-CLEARANCE"),))
        return original(b)
    monkeypatch.setattr(fanout,"run_physical_drc",gate)
    joint = fanout.route_fanout(base,opts(two_leg_escapes=False))
    assert joint.board == greedy.board and joint.accesses == greedy.accesses
    assert joint.assignment.trials[0].solution_found and not joint.assignment.native_accepted


def candidate(net,a,b,layer=CopperLayer.FRONT,via=False):
    path = (TrackSegment(net,Point.mm(*a),Point.mm(*b),nm_from_mm(.25),layer),)
    v = Via(net,path[0].end,nm_from_mm(.8),nm_from_mm(.4)) if via else None
    return path,v


def test_conflicts_respect_layers_through_vias_same_net_and_drill_spacing():
    conflicts = EscapeConflicts(board(),100)
    a = candidate("A",(2,2),(4,2))
    b = candidate("B",(3,1),(3,3),CopperLayer.BACK)
    assert conflicts.compatible(a,b)
    assert not conflicts.compatible(a,candidate("B",(3,1),(3,3)))
    assert not conflicts.compatible(candidate("A",(2,2),(4,2),via=True),
                                    candidate("B",(4,1),(4,3),CopperLayer.BACK))
    shared = candidate("A",(2,2),(4,2),via=True)
    assert conflicts.compatible(shared,shared)  # one reused hole, not two drills
    assert not conflicts.compatible(shared,candidate("A",(2,2),(4.6,2),via=True))
    previous = conflicts.checks
    assert conflicts.compatible(a,b)
    assert conflicts.checks == previous


def test_remote_candidates_do_not_starve_exact_compatibility_budget():
    conflicts = EscapeConflicts(board(), maximum_checks=1, maximum_queries=100)
    local = candidate("A", (2, 2), (4, 2))
    for i in range(30):
        assert conflicts.compatible(local, candidate("B", (8, 8 + i / 100), (9, 8 + i / 100)))
    assert conflicts.queries == conflicts.broad_phase_accepts == 30
    assert conflicts.checks == 0 and not conflicts._cache
    assert not conflicts.compatible(local, candidate("B", (3, 1), (3, 3)))
    assert conflicts.checks == 1
    # Cached exact answers also count toward the total CPU/query bound.
    assert not conflicts.compatible(local, candidate("B", (3, 1), (3, 3)))
    assert conflicts.checks == 1 and conflicts.queries == 32


def test_broad_phase_has_its_own_query_limit():
    conflicts = EscapeConflicts(board(), maximum_checks=1, maximum_queries=1)
    first, remote = candidate("A", (2, 2), (4, 2)), candidate("B", (8, 8), (9, 8))
    assert conflicts.compatible(first, remote)
    from pcbir.escape_assignment import _BudgetExceeded
    with pytest.raises(_BudgetExceeded, match="query budget"):
        conflicts.compatible(first, remote)
    assert conflicts.checks == 0 and conflicts.queries == 1


def test_foreign_clearance_overrides_enter_candidate_conflicts():
    base = replace(board(),net_routing_rules=(NetRoutingRule("B",clearance_nm=nm_from_mm(1)),))
    a,b = candidate("A",(2,2),(4,2)),candidate("B",(2,2.8),(4,2.8))
    assert EscapeConflicts(board(),100).compatible(a,b)
    assert not EscapeConflicts(base,100).compatible(a,b)
    assert not EscapeConflicts(base,100).compatible(b,a)


def test_trial_and_invalid_option_budgets_are_explicit():
    for invalid in (0, -1, 1.5, "8"):
        with pytest.raises(ValueError, match="positive integer"):
            EscapeAssignmentOptions(maximum_trials=invalid)
    base = board()
    pads = (PadReference("U","1"),PadReference("U","2"))
    choices = {pads[0]:(candidate("A",(5,6),(3,6),via=True),),
               pads[1]:(candidate("B",(5,8),(3,8),via=True),)}
    selected,report = improve_escape_assignment(base,pads,choices,{},lambda p:(),
                                               EscapeAssignmentOptions(maximum_trials=1))
    assert set(selected) == {pads[0]} and len(report.trials) == 1


def independent_assignment_traps():
    # Ten separated two-pin conflicts, each requiring its incumbent to move.
    # Expansion exposes the alternative; each missing root needs its own trial.
    base = PhysicalBoard("independent-traps", BoardOutline.rectangle(110, 14), {}, (),
        tuple(PhysicalNet(f"{role}{i}", ()) for i in range(10) for role in ("A", "B")))
    ordered, domains, incumbent, additions = [], {}, {}, {}
    for i in range(10):
        first, second = PadReference(f"U{i}", "1"), PadReference(f"U{i}", "2")
        x = 4 + 10 * i
        ordered.extend((first, second))
        domains[first] = (candidate(f"A{i}", (x, 3), (x, 4), via=True),)
        domains[second] = (candidate(f"B{i}", (x, 3), (x, 4), via=True),)
        additions[first] = (candidate(f"A{i}", (x + 2, 3), (x + 2, 4), via=True),)
        incumbent[first] = 0
    return base, tuple(ordered), domains, incumbent, lambda pad: additions.get(pad, ())


def test_default_repairs_more_than_eight_roots_and_explicit_cap_preserves_successes():
    base, ordered, domains, incumbent, expand = independent_assignment_traps()
    options = fanout.FanoutOptions().assignment_options
    selected, report = improve_escape_assignment(base, ordered, dict(domains), incumbent, expand, options)
    assert selected == {pad: 1 if pad in incumbent else 0 for pad in ordered}
    assert len(report.trials) == 10 and all(t.solution_found for t in report.trials)
    assert (selected, report) == improve_escape_assignment(
        base, ordered, dict(domains), incumbent, expand, options)
    capped, limited = improve_escape_assignment(base, ordered, dict(domains), incumbent, expand,
        replace(options, maximum_trials=8))
    assert len(limited.trials) == 8 and all(t.solution_found for t in limited.trials)
    assert set(incumbent) <= set(capped)
    assert capped == {**incumbent, **{pad: selected[pad] for pad in ordered[:16]}}
    assert limited.trials == report.trials[:8]


@pytest.mark.parametrize("budget, counter", [
    ("maximum_pair_checks", "pair_checks"), ("maximum_pair_queries", "pair_queries")])
def test_default_root_trials_remain_bounded_by_shared_conflict_budgets(budget, counter):
    base, ordered, domains, incumbent, expand = independent_assignment_traps()
    prefix, prefix_report = improve_escape_assignment(base, ordered, dict(domains), incumbent, expand,
        EscapeAssignmentOptions(maximum_trials=3))
    limit = getattr(prefix_report, counter)
    options = EscapeAssignmentOptions(**{budget: limit})
    selected, report = improve_escape_assignment(base, ordered, dict(domains), incumbent, expand, options)
    assert selected == prefix  # Exhausting a shared budget cannot lose earlier repairs.
    assert report.trials[:-1] == prefix_report.trials
    assert not report.trials[-1].solution_found and "budget exhausted" in report.trials[-1].diagnostic
    assert getattr(report, counter) == limit
    assert (selected, report) == improve_escape_assignment(
        base, ordered, dict(domains), incumbent, expand, options)


def test_conflict_driven_cluster_growth_repairs_a_three_pin_chain():
    base = PhysicalBoard("chain",BoardOutline.rectangle(14,14),{},(),
                         tuple(PhysicalNet(n,()) for n in ("A","B","C")))
    pads = tuple(PadReference("U",str(n)) for n in (1,2,3))
    def site(net,x):
        return candidate(net,(x,3.5),(x,4),via=True)
    domains = {pads[0]:(site("A",4),),pads[1]:(site("B",4),site("B",6)),
               pads[2]:(site("C",6),site("C",8))}
    incumbent = {pads[1]:0,pads[2]:0}
    selected,report = improve_escape_assignment(base,pads,dict(domains),incumbent,
                                               lambda p:(),EscapeAssignmentOptions())
    assert selected == {pads[0]:0,pads[1]:1,pads[2]:1}
    assert report.trials[0].cluster == pads and report.trials[0].solution_found
    failed,limited = improve_escape_assignment(base,pads,dict(domains),incumbent,
        lambda p:(),EscapeAssignmentOptions(maximum_cluster_pins=2))
    assert failed == incumbent and "pin budget" in limited.trials[0].diagnostic


def test_installed_kicad_accepts_joint_escape_route(monkeypatch):
    from pcbir.detailed import DetailedRouterOptions, route_detailed
    from pcbir.routing import GlobalRouterOptions, route_global
    from pcbir.plane_verify import verify_filled_planes
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("KiCad not installed")
    base = board()
    monkeypatch.setattr(fanout,"_candidates",trap_candidates)
    joint = fanout.route_fanout(base,opts(two_leg_escapes=False))
    result = route_detailed(joint.board,route_global(base,GlobalRouterOptions()),DetailedRouterOptions(),
        fanout_accesses=joint.accesses,fanout_created_tracks=joint.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in joint.created_vias))
    assert all(n.connected for n in result.nets)
    evidence = verify_filled_planes(result.board,kicad_cli=Path(cli))
    assert evidence.passed,evidence.findings

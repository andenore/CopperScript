from dataclasses import replace
import pytest
from pcbir import ComponentPlacementRule
from pcbir.placement import PlacementPlannerOptions
from pcbir.decoupling import improve_decoupling_placement,route_decouplers
from test_routing_quality import bypass_board


@pytest.mark.parametrize("lock",("source","session"))
def test_no_routing_work_when_every_capacitor_pose_is_fixed(monkeypatch,lock):
    b=bypass_board();poses={p.reference:p for p in b.placements}
    cap=b.decoupling_links[0].capacitor.component
    options=PlacementPlannerOptions()
    if lock=="source":
        b=replace(b,placement_rules=(*b.placement_rules,ComponentPlacementRule(
            cap,fixed_position=poses[cap].position,fixed_rotation_degrees=poses[cap].rotation_degrees)))
    else:options=replace(options,fixed_references=frozenset((cap,)))
    def unexpected(*args,**kwargs):
        raise AssertionError("fixed-only pose refinement cannot use disposable routes")
    monkeypatch.setattr("pcbir.decoupling.route_decouplers",unexpected)
    result,moved=improve_decoupling_placement(b,poses,options)
    assert result is poses and moved==0
    assert b.tracks==b.vias==()


def test_movable_capacitor_still_gets_complete_baseline_reservation(monkeypatch):
    b=bypass_board();poses={p.reference:p for p in b.placements};calls=[]
    def observe(*args,**kwargs):
        calls.append(args[0]);return route_decouplers(*args,**kwargs)
    monkeypatch.setattr("pcbir.decoupling.route_decouplers",observe)
    improve_decoupling_placement(b,poses,PlacementPlannerOptions())
    assert calls and calls[0].decoupling_links==b.decoupling_links


def test_fixed_only_shortcut_does_not_allow_routed_input():
    b=route_decouplers(bypass_board()).board;poses={p.reference:p for p in b.placements}
    with pytest.raises(ValueError,match="unrouted"):
        improve_decoupling_placement(b,poses,PlacementPlannerOptions(fixed_references=frozenset(poses)))


def test_zero_pose_budget_skips_routes_but_still_validates_fixed_copper(monkeypatch):
    b=bypass_board();poses={p.reference:p for p in b.placements}
    options=PlacementPlannerOptions(decoupling_placement_trials=0)
    def unexpected(*args,**kwargs):
        raise AssertionError('disabled pose search must not route disposable feeds')
    monkeypatch.setattr('pcbir.decoupling.route_decouplers',unexpected)
    assert improve_decoupling_placement(b,poses,options)==(poses,0)
    def rejected(*args,**kwargs):
        raise ValueError('fixed copper rejected')
    monkeypatch.setattr('pcbir.hard_macros.materialize_hard_macros',rejected)
    with pytest.raises(ValueError,match='fixed copper rejected'):
        improve_decoupling_placement(b,poses,options)


@pytest.mark.parametrize('value',[-1,True,1.5])
def test_invalid_pose_budget(value):
    with pytest.raises(ValueError,match='nonnegative integer'):
        PlacementPlannerOptions(decoupling_placement_trials=value)

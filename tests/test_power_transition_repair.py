"""Exact native power-via faults permit bounded, additive transition repair."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import CopperLayer, Point, TrackSegment, Via
from pcbir.power_geometry import PowerAreaOptions, apply_power_area_plan, plan_power_areas
from pcbir.power_transition_repair import repair_native_power_transitions


def fixture():
    from test_power_geometry import board
    source = board()
    front, back = CopperLayer.FRONT, CopperLayer.BACK
    source = replace(source, net_routing_rules=(replace(source.net_routing_rules[0],
        allowed_layers=(front, back)),), tracks=(
        TrackSegment('RAIL', Point.mm(3, 5), Point.mm(15, 5), 250000, front),
        TrackSegment('RAIL', Point.mm(12, 5), Point.mm(14, 5), 250000, back)),
        vias=tuple(Via('RAIL', Point.mm(x, 5), 600000, 300000, front, back) for x in (6, 12, 14)))
    options = PowerAreaOptions(candidates_per_domain=1, maximum_layer_alternatives=1,
        maximum_conflict_trials=1, maximum_branch_layers=2, maximum_native_trials=2)
    return source, options


@pytest.fixture(scope='module')
def held_transition(tmp_path_factory):
    from test_power_preparation import verifier
    source, options = fixture()
    plan = plan_power_areas(source, options)
    board, status = apply_power_area_plan(source, plan)
    assert status in {'already_contact_routed', 'contact_routed_native_pending'}
    verify, _ = verifier(tmp_path_factory.mktemp('transition'))
    evidence = verify(board, 'before')
    assert evidence.unconnected_count == evidence.island_count == 0
    assert evidence.other_violation_count == 1
    assert [f.type for f in evidence.native_findings] == ['via_dangling']
    return source, options, plan, board, evidence


def test_held_transition_completion_keeps_source_via_array_and_requires_new_native_evidence(held_transition, tmp_path):
    from test_power_preparation import verifier
    source, options, plan, board, evidence = held_transition
    repaired = repair_native_power_transitions(plan, options, evidence)
    assert repaired.selected_ids != plan.selected_ids
    assert repaired.status == 'geometry_proposal'
    candidate, status = apply_power_area_plan(source, repaired)
    assert status == 'contact_routed_native_pending'
    assert candidate.vias == source.vias and len(candidate.vias) == 3
    assert not Counter(board.tracks) - Counter(candidate.tracks)
    assert candidate.zones == board.zones and candidate.placements == board.placements
    assert all(t.layer == CopperLayer.BACK for t in candidate.tracks if t not in board.tracks)
    assert not evidence.matches(candidate)
    assert len(repaired.conflict_trials) == 1
    assert plan.allocation_states < repaired.allocation_states <= options.allocation_states
    assert repaired.native_transition_trials[0]['status'] == 'geometry_proposal'
    verify, _ = verifier(tmp_path)
    fresh = verify(candidate, 'after')
    assert fresh.passed and fresh.matches(candidate)
    assert fresh.unconnected_count == fresh.other_violation_count == fresh.island_count == 0


@pytest.mark.parametrize('change', ['evidence', 'source'])
def test_stale_native_evidence_or_source_cannot_authorize_repair(held_transition, change):
    source, options, plan, _, evidence = held_transition
    if change == 'evidence':
        evidence = replace(evidence, board_digest='stale')
    else:
        plan = replace(plan, source_board=replace(source, tracks=source.tracks[:-1]))
    with pytest.raises(ValueError, match='stale power transition'):
        repair_native_power_transitions(plan, options, evidence)


@pytest.mark.parametrize('change', ['uuid', 'index', 'net', 'type'])
def test_native_binding_must_match_exact_object(held_transition, change):
    _, options, plan, _, evidence = held_transition
    finding = evidence.native_findings[0]
    if change == 'type':
        finding = replace(finding, type='unconnected_items')
    else:
        item = replace(finding.items[0], **{change: {'uuid':'foreign', 'index':1, 'net':'other'}[change]})
        finding = replace(finding, items=(item,))
    assert repair_native_power_transitions(plan, options,
        replace(evidence, native_findings=(finding,))) is plan


@pytest.mark.parametrize('budget', ['repair', 'allocator', 'layers'])
def test_exhausted_existing_budgets_return_exact_plan(held_transition, budget):
    _, options, plan, _, evidence = held_transition
    if budget == 'repair':
        options = replace(options, maximum_conflict_trials=0)
    elif budget == 'allocator':
        plan = replace(plan, allocation_states=options.allocation_states)
    else:
        options = replace(options, maximum_branch_layers=0)
    assert repair_native_power_transitions(plan, options, evidence) is plan


def test_blocked_or_forbidden_layer_keeps_every_original_candidate(held_transition, monkeypatch):
    import pcbir.power_geometry as geometry
    source, options, plan, board, evidence = held_transition
    for blocked in (False, True):
        settings = options if blocked else replace(options, allowed_layers=(CopperLayer.FRONT,))
        if blocked:
            monkeypatch.setattr(geometry._AreaClearance, 'can_track', lambda *args: False)
        failed = repair_native_power_transitions(plan, settings, evidence)
        assert failed.selected_ids == plan.selected_ids and failed.candidates == plan.candidates
        assert apply_power_area_plan(source, failed)[0] == board
        assert failed.native_transition_trials[0]['status'] == 'transition_no_path'
        assert len(failed.conflict_trials) == 1
        assert failed.native_transition_trials[0]['expanded_states'] <= options.branch_search_states


@pytest.mark.parametrize('modeled', ['options', 'candidate'])
def test_geometry_repair_cannot_reuse_an_old_electrical_assessment(held_transition, modeled):
    from test_power_extract import fixture as electrical_fixture
    from pcbir.power_extract import screen_power_model
    source, options, plan, board, evidence = held_transition
    modeled_board, model = electrical_fixture()
    if modeled == 'options':
        options = replace(options, electrical_models=(model,))
    else:
        screen = screen_power_model(modeled_board, model)
        assert screen.status == 'dc_mesh_screen_pass'
        old = plan.selected[0]
        assessed = replace(old, allocation=replace(old.allocation, network=screen.network,
            minimum_voltages_v=screen.limits, voltage_references=screen.references))
        plan = replace(plan, candidates=tuple(assessed if c == old else c for c in plan.candidates))
    rejected = repair_native_power_transitions(plan, options, evidence)
    assert rejected.native_transition_trials[0]['status'] == 'transition_electrical_reassessment_required'
    assert rejected.candidates == plan.candidates and rejected.selected_ids == plan.selected_ids
    assert rejected.allocation_states == plan.allocation_states
    assert apply_power_area_plan(source, rejected)[0] == board


@pytest.mark.parametrize('native_trials', [1, 2])
def test_normal_preparation_rechecks_repair_and_freezes_only_native_acceptance(tmp_path, native_trials):
    from test_power_preparation import verifier
    from pcbir.fanout import FanoutOptions
    from pcbir.power_preparation import (ContactPreparationSnapshot, prepare_power_from_contacts,
                                         preparation_preserved)
    from pcbir.copper_ownership import begin_copper_ownership, append_copper_ownership
    from pcbir.critical import route_critical_nets
    from pcbir.routing import route_global
    source, options = fixture()
    options = replace(options, maximum_native_trials=native_trials)
    verify, calls = verifier(tmp_path)
    clean = replace(source, tracks=(), vias=())
    critical = route_critical_nets(clean, route_global(clean))
    ownership = append_copper_ownership(begin_copper_ownership(critical.board),
        critical.board, source, 'package-contacts', trial_mutable=True)
    snapshot = ContactPreparationSnapshot(clean, critical, source, ownership, ('RAIL',), frozenset())
    assert snapshot.matches()
    prepared = prepare_power_from_contacts(snapshot, options, FanoutOptions(), native_verifier=verify)
    assert len(prepared.native_trials) == native_trials
    assert prepared.native_trials[0]['observation']['status'] != 'native_connected'
    if native_trials == 1:
        assert not prepared.accepted and prepared.board is clean
        assert calls == ['power-return-baseline', 'power-return-candidate']
    else:
        assert prepared.accepted, prepared.report()
        assert calls[-1] == 'power-return-candidate-2'
        assert prepared.ownership.frozen and prepared.ownership.matches(prepared.board)
        assert prepared.evidence.matches(prepared.board)
        assert preparation_preserved(prepared, prepared.board)
        assert prepared.board.vias == source.vias
        assert prepared.plan.native_transition_trials[0]['status'] == 'geometry_proposal'

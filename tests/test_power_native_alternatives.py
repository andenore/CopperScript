"""Native fallback reuses a certified pool and never promotes failed copper."""
from dataclasses import replace

import pytest

from pcbir.power_areas import PowerAreaCandidate, allocate_power_areas
from pcbir.power_geometry import (PowerAreaOptions, plan_power_areas,
                                  reselect_power_area_plan)


def candidate(identity, domain, area):
    return PowerAreaCandidate(identity, domain, 'snapshot', frozenset({identity}), None, (), area)


def test_rejected_joint_assignment_does_not_ban_its_individual_candidates():
    pool = (candidate('a0', 'A', 1), candidate('a1', 'A', 2),
            candidate('b0', 'B', 1), candidate('b1', 'B', 2))
    args = dict(domains=('A', 'B'), snapshot='snapshot', allow_unassessed_geometry=True)
    first = allocate_power_areas(pool, **args)
    ids = tuple(c.id for c in first.selected)
    assert ids == ('a0', 'b0')
    second = allocate_power_areas(pool, **args, excluded_assignments=(ids,))
    assert tuple(c.id for c in second.selected) == ('a0', 'b1')
    all_rejected = [('a'+str(a), 'b'+str(b)) for a in (0, 1) for b in (0, 1)]
    none = allocate_power_areas(pool, **args, excluded_assignments=all_rejected)
    assert not none.selected and not none.exhausted
    limited = allocate_power_areas(pool, **args, excluded_assignments=all_rejected, maximum_states=1)
    assert not limited.selected and limited.exhausted and limited.states == 1


@pytest.mark.parametrize('row', [('missing', 'b0'), ('a0',), ('a0', 'a1'), ('a0', 'a0')])
def test_rejection_must_identify_one_known_candidate_per_domain(row):
    pool = (candidate('a0', 'A', 1), candidate('a1', 'A', 2), candidate('b0', 'B', 1))
    with pytest.raises(ValueError, match='excluded assignments'):
        allocate_power_areas(pool, domains=('A', 'B'), snapshot='snapshot', excluded_assignments=(row,))


def test_reselection_spends_the_original_cumulative_allocator_allowance():
    from test_power_geometry import board
    options = PowerAreaOptions(candidates_per_domain=3, maximum_layer_alternatives=1)
    plan = plan_power_areas(board(), options)
    assert len(plan.candidates) == 3
    next_plan = reselect_power_area_plan(plan, options, (plan.selected_ids,))
    assert next_plan.selected_ids != plan.selected_ids
    assert next_plan.candidates == plan.candidates and next_plan.snapshot == plan.snapshot
    assert plan.allocation_states < next_plan.allocation_states <= options.allocation_states
    used_up = replace(plan, allocation_states=options.allocation_states)
    exhausted = reselect_power_area_plan(used_up, options, (plan.selected_ids,))
    assert not exhausted.selected and exhausted.exhausted
    assert exhausted.allocation_states == options.allocation_states


@pytest.mark.parametrize('limit', [1, 2])
@pytest.mark.parametrize('contact_alternatives', [1, 2])
def test_native_rejection_uses_fresh_alternative_or_rolls_back_exactly(tmp_path, limit, contact_alternatives):
    from test_power_preparation import fixture, verifier
    from pcbir.fanout import FanoutOptions
    from pcbir.power_preparation import prepare_power_distribution, preparation_preserved
    from pcbir.routing import route_global
    board, options = fixture()
    from pcbir import CopperLayer
    options = replace(options, candidates_per_domain=2, maximum_layer_alternatives=2,
                      maximum_native_trials=limit,
                      maximum_contact_alternatives=contact_alternatives,
                      allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_2, CopperLayer.BACK))
    verify, calls = verifier(tmp_path)
    seen, baseline = [], []
    def reject_first(candidate, stage):
        evidence = verify(candidate, stage)
        if stage == 'power-return-baseline':
            baseline.append(candidate)
        if stage.startswith('power-return-candidate'):
            seen.append(candidate)
            if len(seen) == 1:
                # Controlled native rejection; all later candidates still need
                # their own genuine, source-bound native verification.
                return replace(evidence, island_count=evidence.island_count+1)
        return evidence
    prep = prepare_power_distribution(board, route_global(board), options, FanoutOptions(),
                                      native_verifier=reject_first)
    assert len(prep.native_trials) == limit, prep.report()
    assert calls[:2] == ['power-return-baseline', 'power-return-candidate']
    assert prep.native_trials[0]['observation']['status'] == 'unresolved'
    if limit == 1:
        assert not prep.accepted and prep.board is board
        assert prep.board.tracks == prep.board.vias == ()
    else:
        assert prep.accepted, prep.report()
        assert calls[-1] == 'power-return-candidate-2'
        assert prep.native_trials[0]['selected_ids'] != prep.native_trials[1]['selected_ids']
        assert prep.evidence.matches(prep.board) and preparation_preserved(prep, prep.board)
        assert prep.board is seen[1] and prep.board is not seen[0]
        assert prep.plan.allocation_states <= options.allocation_states
        from pcbir.power_geometry import apply_power_area_plan
        expected, _ = apply_power_area_plan(baseline[0], prep.plan)
        assert prep.board == expected  # No copper from the rejected assignment.


def test_exhausted_candidate_pool_does_not_repeat_native_or_adopt_failed_trial(tmp_path):
    from test_power_preparation import fixture, verifier
    from pcbir.fanout import FanoutOptions
    from pcbir.power_preparation import prepare_power_distribution
    from pcbir.routing import route_global
    board, options = fixture()
    options = replace(options, maximum_native_trials=3)
    verify, calls = verifier(tmp_path)
    def reject(candidate, stage):
        evidence = verify(candidate, stage)
        return replace(evidence, island_count=evidence.island_count+1) if stage.endswith('candidate') else evidence
    prep = prepare_power_distribution(board, route_global(board), options, FanoutOptions(), native_verifier=reject)
    assert not prep.accepted and prep.board is board
    assert len(prep.native_trials) == 1
    assert calls == ['power-return-baseline', 'power-return-candidate']

from dataclasses import replace
from fractions import Fraction as F

import pytest

from pcbir.power_network import DCNetwork, ResistorEdge
from pcbir.power_areas import (PowerAreaCandidate, allocate_power_areas, reduce_power_conflict,
                               power_repair_contexts)


def candidate(name, domain, resources, area, resistance=F(1, 10), current=F(1)):
    return PowerAreaCandidate(name, domain, 'snapshot', frozenset(resources),
        DCNetwork((ResistorEdge('conductor', 'source', 'load', resistance),),
                  {'source': F(33, 10)}, {'load': current}), (('load', F(3)),), area)


def test_joint_assignment_saves_corridor_needed_by_competing_rail():
    # Greedy A takes the shared slot and strands B. Joint selection gives A its
    # detour, preserving B's only corridor and the unrelated USB reservation.
    options = (candidate('a-short', 'A', {'shared'}, 10),
               candidate('a-detour', 'A', {'around'}, 20),
               candidate('a-usb', 'A', {'usb'}, 1),
               candidate('b-only', 'B', {'shared'}, 12))
    result = allocate_power_areas(options, domains=('A', 'B'), snapshot='snapshot', reserved_resources={'usb'})
    assert result.status == 'feasible_screen' and not result.exhausted
    assert {c.id for c in result.selected} == {'a-detour', 'b-only'}
    assert dict((i, s) for i, s, _ in result.assessments)['a-usb'] == 'reserved_resource_conflict'
    assert result == allocate_power_areas(reversed(options), domains=('B', 'A'), snapshot='snapshot', reserved_resources={'usb'})


def test_pairwise_feasible_pool_reports_joint_dead_end_without_partial_acceptance():
    # Three domains competing for two slots: every pair can coexist, while
    # every complete assignment conflicts. A third C corridor closes the gap.
    options = tuple(candidate(f'{net}-{slot}', net, {slot}, 10)
                    for net in ('A', 'B', 'C') for slot in ('left', 'right'))
    for omitted in ('A', 'B', 'C'):
        subset = tuple(c for c in options if c.domain != omitted)
        assert allocate_power_areas(subset, domains={c.domain for c in subset}, snapshot='snapshot').selected
    failed = allocate_power_areas(options, domains=('A', 'B', 'C'), snapshot='snapshot')
    assert not failed.selected and not failed.exhausted and failed.blocked_prefixes
    by_id = {c.id:c for c in options}
    for net, owners in failed.blocked_prefixes:
        assert net == 'C' and len(owners) == 2
        a, b = (by_id[i] for i in owners)
        assert not a.resources & b.resources
        assert all(c.resources & (a.resources | b.resources) for c in options if c.domain == net)
    complete = allocate_power_areas((*options, candidate('c-third', 'C', {'third'}, 20)),
                                   domains=('A', 'B', 'C'), snapshot='snapshot')
    assert len(complete.selected) == 3 and not complete.blocked_prefixes
    bounded = allocate_power_areas(options, domains=('A', 'B', 'C'), snapshot='snapshot', maximum_states=1)
    assert bounded.states == 1 and bounded.exhausted and not bounded.selected


def test_multi_owner_repair_can_move_any_domain_not_only_the_last_blocked_one():
    pool=tuple(candidate(f'{net}-{slot}',net,{slot},10)
        for net in ('A','B','C') for slot in ('left','right'))
    original=allocate_power_areas(pool,domains=('A','B','C'),snapshot='snapshot')
    assert {net for net,_ in original.blocked_prefixes}=={'C'}
    contexts=power_repair_contexts(pool,domains=('A','B','C'),snapshot='snapshot',maximum_states=128)
    assert not contexts.exhausted and {net for net,_ in contexts.contexts}=={'A','B','C'}
    by_id={c.id:c for c in pool}
    for net,ids in contexts.contexts:
        held=[by_id[i] for i in ids]
        assert {c.domain for c in held}=={'A','B','C'}-{net}
        assert not held[0].resources&held[1].resources
    assert contexts==power_repair_contexts(reversed(pool),domains=('C','B','A'),snapshot='snapshot',maximum_states=128)
    bounded=power_repair_contexts(pool,domains=('A','B','C'),snapshot='snapshot',maximum_states=1)
    assert bounded.exhausted and bounded.states==1 and not bounded.contexts
    assert not allocate_power_areas(pool,domains=('A','B','C'),snapshot='snapshot').selected


def test_repair_contexts_do_not_hide_stale_candidates_or_exceed_pool_bound():
    pool=(candidate('a','A',{'one'},1),candidate('b','B',{'two'},1))
    with pytest.raises(ValueError):power_repair_contexts(pool,domains=('A','B'),snapshot='changed')
    with pytest.raises(ValueError):power_repair_contexts(pool,domains=('A','B'),snapshot='snapshot',maximum_states=0)
    limited=power_repair_contexts(pool,domains=('A','B'),snapshot='snapshot',maximum_candidates=1)
    assert limited.exhausted and not limited.contexts and limited.states==0


def test_repair_context_extension_keeps_compatible_neighbours_and_core_fallbacks():
    core=tuple(candidate(f'{net}-{slot}',net,{slot},10)
        for net in ('A','B','C') for slot in ('left','right'))
    pool=(*core,candidate('d-crossing','D',{'left'},1),
        candidate('d-outside','D',{'outside'},2),
        candidate('e-electrically-failed','E',{'free'},1,resistance=F(1)))
    result=power_repair_contexts(pool,domains=('A','B','C','D','E'),
        repair_domains=('A','B','C'),snapshot='snapshot')
    by_id={c.id:c for c in pool}
    assert not result.exhausted and len(result.contexts)==6
    for moving in ('A','B','C'):
        held=[ids for net,ids in result.contexts if net==moving]
        assert {len(ids) for ids in held}=={2,3}
        assert 'd-outside' in next(ids for ids in held if len(ids)==3)
        assert all('e-electrically-failed' not in ids and 'd-crossing' not in ids for ids in held)
        for ids in held:
            assert all(not by_id[a].resources&by_id[b].resources for a in ids for b in ids if a!=b)
    assert result==power_repair_contexts(reversed(pool),domains=('E','D','C','B','A'),
        repair_domains=('C','A','B'),snapshot='snapshot')
    limited=power_repair_contexts(pool,domains=('A','B','C','D','E'),
        repair_domains=('A','B','C'),snapshot='snapshot',maximum_states=result.states-1)
    assert limited.exhausted and limited.states==result.states-1
    with pytest.raises(ValueError,match='repair domains'):
        power_repair_contexts(pool,domains=('A','B','C','D','E'),repair_domains=('outside',),snapshot='snapshot')


def test_overlapping_conflicts_require_repairing_a_shared_owner():
    # ABC compete for two horizontal slots; BCD compete for two vertical slots.
    # Moving only A cannot fix BCD, nor can moving only D fix ABC. Joint held
    # choices identify B/C as the useful repair directions without adopting them.
    pool=tuple(candidate('a-'+x,'A',{x},1) for x in ('left','right'))
    pool+=tuple(candidate(n+'-'+x+'-'+y,n,{x,y},2)
        for n in ('B','C') for x in ('left','right') for y in ('top','bottom'))
    pool+=tuple(candidate('d-'+y,'D',{y},1) for y in ('top','bottom'))
    for names in (('A','B','C'),('B','C','D')):
        assert not allocate_power_areas(tuple(c for c in pool if c.domain in names),
            domains=names,snapshot='snapshot').selected
    joint=power_repair_contexts(pool,domains=('A','B','C','D'),snapshot='snapshot')
    assert not joint.exhausted and {n for n,_ in joint.contexts}=={'B','C'}
    by_id={c.id:c for c in pool}
    for moving,ids in joint.contexts:
        held=tuple(by_id[i] for i in ids)
        assert {c.domain for c in held}=={'A','B','C','D'}-{moving}
        assert allocate_power_areas(held,domains=tuple(c.domain for c in held),snapshot='snapshot').selected
    assert not allocate_power_areas(pool,domains=('A','B','C','D'),snapshot='snapshot').selected


def test_rejected_physical_context_can_change_neighbours_without_banning_candidates():
    pool=tuple(candidate(f'{net}-{slot}',net,{slot},10)
        for net in ('A','B','C') for slot in ('left','right'))
    initial=power_repair_contexts(pool,domains=('A','B','C'),snapshot='snapshot')
    rejected=next(row for row in initial.contexts if row[0]=='C')
    result=power_repair_contexts(pool,domains=('A','B','C'),snapshot='snapshot',
        rejected_contexts=(rejected,))
    choices=[ids for net,ids in result.contexts if net=='C']
    assert len(choices)==2 and rejected[1] in choices
    changed=next(ids for ids in choices if ids!=rejected[1])
    assert set(changed)=={'A-right','B-left'}
    by_id={c.id:c for c in pool}
    assert not by_id[changed[0]].resources&by_id[changed[1]].resources
    assert result==power_repair_contexts(reversed(pool),domains=('C','B','A'),snapshot='snapshot',
        rejected_contexts=(rejected,))
    # Exclusions select trial obstacles only. The original pool remains
    # unchanged and the full three-domain problem still has no assignment.
    assert not allocate_power_areas(pool,domains=('A','B','C'),snapshot='snapshot').selected
    bounded=power_repair_contexts(pool,domains=('A','B','C'),snapshot='snapshot',
        maximum_states=result.states-1,rejected_contexts=(rejected,))
    assert bounded.exhausted and bounded.states==result.states-1


def test_rejected_context_preserves_exact_surrounding_scope_and_source_validation():
    core=tuple(candidate(f'{net}-{slot}',net,{slot},10)
        for net in ('A','B','C') for slot in ('left','right'))
    pool=(*core,candidate('d-near','D',{'near'},1),candidate('d-far','D',{'far'},2))
    initial=power_repair_contexts(pool,domains=('A','B','C','D'),repair_domains=('A','B','C'),snapshot='snapshot')
    rejected=next(row for row in initial.contexts if row[0]=='C' and len(row[1])==3)
    second=('C',('A-right','B-left','d-near'))
    result=power_repair_contexts(pool,domains=('A','B','C','D'),repair_domains=('A','B','C'),
        snapshot='snapshot',rejected_contexts=(rejected,second))
    # Both held assignments using near-D failed. Changing only D can retain
    # A/B; projecting the failures onto A/B would discard every useful choice.
    changed=next(ids for net,ids in result.contexts if net=='C' and len(ids)==3 and ids!=rejected[1])
    assert set(changed)==(set(rejected[1])-{'d-near'})|{'d-far'}
    assert any(net=='C' and len(ids)==2 for net,ids in result.contexts)
    for invalid in (('C',('unknown',)),('C',('C-left',)),('C',('A-left','A-right'))):
        with pytest.raises(ValueError,match='rejected repair contexts'):
            power_repair_contexts(pool,domains=('A','B','C','D'),snapshot='snapshot',rejected_contexts=(invalid,))


def test_conflict_reduction_removes_irrelevant_domains_but_never_adopts_a_pair():
    pool = tuple(candidate(f'{net}-{slot}', net, {slot}, 10)
                 for net in ('B', 'C', 'D') for slot in ('left', 'right'))
    pool += (candidate('unrelated-first', 'A', {'first'}, 1),
             candidate('unrelated-last', 'Z', {'last'}, 1))
    result = reduce_power_conflict(pool, domains=('A','B','C','D','Z'), snapshot='snapshot')
    assert result.domains == ('B','C','D') and not result.exhausted
    assert not result.allocation.selected and result.allocation.blocked_prefixes
    assert result.states == sum(row[2] for row in result.checks)
    for net, ids in result.allocation.blocked_prefixes:
        assert net in result.domains
        assert not {'unrelated-first','unrelated-last'}.intersection(ids)
    for omitted in result.domains:
        names = set(result.domains)-{omitted}
        assert allocate_power_areas(tuple(c for c in pool if c.domain in names),
            domains=names, snapshot='snapshot').selected
    assert result == reduce_power_conflict(reversed(pool), domains=('Z','D','C','B','A'), snapshot='snapshot')


def test_conflict_reduction_uses_one_budget_and_never_calls_exhaustion_infeasibility():
    pool = tuple(candidate(f'{net}-{slot}', net, {slot}, 10)
                 for net in ('A','B','C') for slot in ('left','right'))
    full = allocate_power_areas(pool, domains=('A','B','C'), snapshot='snapshot')
    budget = full.states+1
    bounded = reduce_power_conflict(pool, domains=('A','B','C'), snapshot='snapshot', maximum_states=budget)
    assert bounded.exhausted and bounded.states == budget
    assert bounded.domains == ('A','B','C') and bounded.allocation == full
    assert bounded.checks[-1][3] and not bounded.allocation.exhausted
    one = reduce_power_conflict(pool, domains=('A','B','C'), snapshot='snapshot', maximum_states=1)
    assert one.exhausted and one.states == 1 and one.allocation.exhausted


def test_conflict_reduction_rechecks_a_repaired_core_and_rejects_foreign_candidates():
    pool = tuple(candidate(f'{net}-{slot}', net, {slot}, 10)
                 for net in ('A','B','C') for slot in ('left','right'))
    repaired = reduce_power_conflict((*pool,candidate('c-third','C',{'third'},20)),
        domains=('A','B','C'),snapshot='snapshot')
    assert len(repaired.allocation.selected)==3 and len(repaired.checks)==1
    with pytest.raises(ValueError, match='foreign domain'):
        reduce_power_conflict(pool,domains=('A','B'),snapshot='snapshot')
    with pytest.raises(ValueError, match='stale snapshot'):
        reduce_power_conflict(pool,domains=('A','B','C'),snapshot='changed')


def test_electrical_demand_changes_selected_area_before_routing():
    narrow = candidate('narrow', 'A', {'lane'}, 10, resistance=F(1, 5))
    wide = candidate('wide', 'A', {'lane', 'adjacent'}, 20, resistance=F(1, 10))
    first = allocate_power_areas((narrow, wide), domains=('A',), snapshot='snapshot')
    assert first.selected[0].id == 'narrow'
    high = tuple(replace(c, network=replace(c.network, withdrawals_a={'load': F(2)})) for c in (narrow, wide))
    second = allocate_power_areas(high, domains=('A',), snapshot='snapshot')
    assert second.selected[0].id == 'wide'
    assert second.assessments[0][1] == 'voltage_limit_failed'


def test_unassessed_geometry_never_outranks_an_electrically_screened_choice():
    assessed = candidate('assessed', 'A', {'space'}, 100)
    unknown = replace(candidate('unknown', 'A', {'space'}, 1), network=None)
    result = allocate_power_areas((unknown, assessed), domains=('A',), snapshot='snapshot', allow_unassessed_geometry=True)
    assert result.status == 'feasible_screen' and result.selected == (assessed,)
    result = allocate_power_areas((unknown,), domains=('A',), snapshot='snapshot', allow_unassessed_geometry=True)
    assert result.status == 'geometry_proposal' and result.assessments[0][1] == 'electrical_inputs_missing'


def test_missing_inputs_or_unmodeled_loads_do_not_pass():
    c = candidate('c', 'A', {'lane'}, 1)
    for bad in (replace(c, network=None), replace(c, minimum_voltages_v=()),
                replace(c, network=replace(c.network, withdrawals_a={})),
                replace(c, minimum_voltages_v=(('other', F(3)),))):
        result = allocate_power_areas((bad,), domains=('A',), snapshot='snapshot')
        assert result.status == 'unresolved' and not result.selected


def test_disconnected_source_and_budget_exhaustion_remain_unresolved():
    c = candidate('c', 'A', {'lane'}, 1)
    disconnected = replace(c, network=replace(c.network, fixed_voltages_v={'outside': F(33, 10)}))
    result = allocate_power_areas((disconnected,), domains=('A',), snapshot='snapshot')
    assert result.assessments[0][1] == 'missing_voltage_boundary'
    result = allocate_power_areas((c,), domains=('A',), snapshot='snapshot', maximum_states=1)
    assert result.exhausted and result.status == 'unresolved'
    assert not result.selected


def test_stale_snapshots_and_missing_domain_are_not_silently_accepted():
    c = candidate('c', 'A', {'lane'}, 1)
    with pytest.raises(ValueError, match='stale'):
        allocate_power_areas((c,), domains=('A',), snapshot='different')
    result = allocate_power_areas((c,), domains=('A', 'B'), snapshot='snapshot')
    assert result.unresolved_domains == ('B',)


def test_candidate_bound_and_source_immutability():
    a, b = candidate('a', 'A', {'a'}, 1), candidate('b', 'A', {'b'}, 2)
    before = repr((a, b))
    result = allocate_power_areas((a, b), domains=('A',), snapshot='snapshot', maximum_candidates=1)
    assert result.status == 'candidate_budget_exhausted'
    assert repr((a, b)) == before


def test_resource_renaming_preserves_unique_solution():
    candidates = (candidate('x', 'A', {'shared'}, 1), candidate('y', 'A', {'detour'}, 2),
                  candidate('z', 'B', {'shared'}, 1))
    original = allocate_power_areas(candidates, domains=('A', 'B'), snapshot='snapshot')
    renamed = tuple(replace(c, domain=c.domain+'renamed', resources=frozenset(r+'new' for r in c.resources)) for c in candidates)
    result = allocate_power_areas(renamed, domains=('Arenamed', 'Brenamed'), snapshot='snapshot')
    assert {c.id for c in original.selected} == {c.id for c in result.selected}


def test_local_return_drop_cannot_hide_behind_absolute_supply_voltage():
    c = candidate('c', 'A', {'lane'}, 1, current=F(2))
    network = DCNetwork((*c.network.edges, ResistorEdge('return', 'gnd', 'local_return', F(1, 10))),
        {'source': F(33, 10), 'gnd': 0}, {'load': 2, 'local_return': -2})
    c = replace(c, network=network, voltage_references=(('load', 'local_return'),))
    result = allocate_power_areas((c,), domains=('A',), snapshot='snapshot')
    assert result.status == 'unresolved'
    _, status, solved = result.assessments[0]
    assert solved.voltages_v['load'] == pytest.approx(3.1)
    assert solved.voltages_v['local_return'] == pytest.approx(.2)
    assert status == 'voltage_limit_failed'

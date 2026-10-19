"""Bounded joint selection of explicit power-region candidates.

Resources represent caller-certified spatial conflicts, not connectivity.
The physical planner supplies exact pairwise geometric conflict certificates.
This allocator never mutates a board or authorizes net deferral.
"""
from dataclasses import dataclass
from fractions import Fraction

from .power_network import DCNetwork, DCSolution, _exact, solve_dc


@dataclass(frozen=True)
class PowerAreaCandidate:
    id: str
    domain: str
    snapshot: str
    resources: frozenset[str]
    network: DCNetwork | None
    # node -> minimum acceptable voltage for every required modeled load.
    minimum_voltages_v: tuple[tuple[str, Fraction], ...]
    area_nm2: int
    via_count: int = 0
    # Optional local return for each constrained load; otherwise global 0 V.
    voltage_references: tuple[tuple[str, str], ...] = ()

    def __post_init__(self):
        if not all((self.id, self.domain, self.snapshot)):
            raise ValueError('candidate requires identity, domain and input snapshot')
        if type(self.area_nm2) is not int or self.area_nm2 <= 0 or type(self.via_count) is not int or self.via_count < 0:
            raise ValueError('invalid area or via count')
        object.__setattr__(self, 'resources', frozenset(self.resources))
        if not self.resources or any(not isinstance(r, str) or not r for r in self.resources):
            raise ValueError('invalid spatial resource')
        limits = tuple((n, _exact(v, 'minimum voltage')) for n, v in self.minimum_voltages_v)
        if len({n for n, _ in limits}) != len(limits) or any(not n for n, _ in limits):
            raise ValueError('voltage limits require unique named nodes')
        object.__setattr__(self, 'minimum_voltages_v', limits)
        references = tuple(self.voltage_references)
        if (len({n for n, _ in references}) != len(references)
                or any(n not in dict(limits) or not isinstance(r, str) or not r for n, r in references)):
            raise ValueError('local voltage references must identify constrained loads')
        object.__setattr__(self, 'voltage_references', references)


@dataclass(frozen=True)
class AreaAllocation:
    status: str
    selected: tuple[PowerAreaCandidate, ...]
    assessments: tuple[tuple[str, str, DCSolution | None], ...]
    states: int
    exhausted: bool
    unresolved_domains: tuple[str, ...]
    # Bounded dead-end witnesses, never a partial accepted assignment.
    # Each names the next domain and a mutually compatible prefix of owners.
    blocked_prefixes: tuple[tuple[str, tuple[str, ...]], ...] = ()


@dataclass(frozen=True)
class AreaConflictCore:
    domains: tuple[str, ...]
    allocation: AreaAllocation
    states: int
    exhausted: bool
    checks: tuple = ()


@dataclass(frozen=True)
class AreaRepairContexts:
    contexts: tuple[tuple[str, tuple[str, ...]], ...]
    states: int
    exhausted: bool


def power_repair_contexts(candidates, *, domains, snapshot, maximum_states=4096,
                          maximum_candidates=128, repair_domains=None, rejected_contexts=()):
    """Prove compatible held choices for each direction of a conflict repair.

    An allocator's last blocked domain is an ordering effect, not authority to
    keep every earlier owner fixed. Each context holds one complete compatible
    assignment for the other repair domains. Extend it with compatible choices
    from surrounding domains so a local repair need not rediscover conflicts
    outside its core. Retain the smaller context as a fallback. These are
    disposable obstacles, never an accepted partial board. Every query shares
    one state allowance. A rejected physical repair can request another exact
    held assignment. This excludes only that combination from the context
    query, never individual candidates or the complete-board allocation.
    """
    candidates,domains=tuple(candidates),tuple(sorted(domains))
    if not snapshot or not domains or len(set(domains))!=len(domains):
        raise ValueError('unique nonempty domains and snapshot required')
    if any(type(v) is not int or v<=0 for v in (maximum_states,maximum_candidates)):
        raise ValueError('repair context bounds must be positive integers')
    if len({c.id for c in candidates})!=len(candidates) or any(
            c.domain not in domains or c.snapshot!=snapshot for c in candidates):
        raise ValueError('repair candidates require unique identities and the same source')
    repair_domains=domains if repair_domains is None else tuple(sorted(repair_domains))
    if not repair_domains or len(set(repair_domains))!=len(repair_domains) or not set(repair_domains)<=set(domains):
        raise ValueError('repair domains must be a nonempty unique subset')
    identities={c.id:c.domain for c in candidates}
    rejected_contexts=tuple((net,tuple(sorted(ids))) for net,ids in rejected_contexts)
    for net,ids in rejected_contexts:
        if (net not in domains or not ids or any(i not in identities for i in ids)
                or net in {identities[i] for i in ids}
                or len({identities[i] for i in ids})!=len(ids)):
            raise ValueError('rejected repair contexts require one known candidate per held domain')
    if len(candidates)>maximum_candidates:return AreaRepairContexts((),0,True)
    contexts=[];states=0
    surrounding=sorted(set(domains)-set(repair_domains),
        key=lambda n:(sum(c.domain==n for c in candidates),n))
    for net in repair_domains:
        others=tuple(n for n in repair_domains if n!=net)
        if not others:continue
        if states>=maximum_states:return AreaRepairContexts(tuple(contexts),states,True)
        allocation=allocate_power_areas(tuple(c for c in candidates if c.domain in others),
            domains=others,snapshot=snapshot,maximum_candidates=maximum_candidates,
            maximum_states=maximum_states-states,allow_unassessed_geometry=True)
        states+=allocation.states
        if allocation.selected:
            contexts.append((net,tuple(c.id for c in allocation.selected)))
        if allocation.exhausted:return AreaRepairContexts(tuple(contexts),states,True)
        if not allocation.selected:continue
        held=allocation.selected
        for other in surrounding:
            if states>=maximum_states:return AreaRepairContexts(tuple(contexts),states,True)
            extension=allocate_power_areas(tuple(c for c in candidates if c.domain==other),
                domains=(other,),snapshot=snapshot,maximum_candidates=maximum_candidates,
                maximum_states=maximum_states-states,allow_unassessed_geometry=True,
                reserved_resources=frozenset(r for c in held for r in c.resources))
            states+=extension.states
            if extension.exhausted:return AreaRepairContexts(tuple(contexts),states,True)
            held=(*held,*extension.selected)
        extended=(net,tuple(c.id for c in held))
        if extended not in contexts:contexts.append(extended)
    scopes={}
    for net,ids in rejected_contexts:
        if net in repair_domains:
            scope=tuple(sorted(identities[i] for i in ids))
            scopes.setdefault((net,scope),set()).add(ids)
    for (net,scope),excluded in sorted(scopes.items()):
        if states>=maximum_states:return AreaRepairContexts(tuple(contexts),states,True)
        alternative=allocate_power_areas(tuple(c for c in candidates if c.domain in scope),
            domains=scope,snapshot=snapshot,maximum_candidates=maximum_candidates,
            maximum_states=maximum_states-states,allow_unassessed_geometry=True,
            excluded_assignments=excluded)
        states+=alternative.states
        if alternative.selected:
            context=(net,tuple(c.id for c in alternative.selected))
            if context not in contexts:contexts.append(context)
        if alternative.exhausted:return AreaRepairContexts(tuple(contexts),states,True)
    return AreaRepairContexts(tuple(contexts),states,False)


def reduce_power_conflict(candidates, *, domains, snapshot, maximum_states=4096,
                          maximum_candidates=128, allow_unassessed_geometry=False):
    """Find an inclusion-minimal conflicting subset within one shared budget.

    Only a fully searched failed subset may replace the incumbent. A feasible
    subset is diagnostic, never a complete-board assignment. Exhaustion retains
    the last established conflict and stops all further queries. Removing an
    irrelevant domain cannot introduce conflicts into a previously feasible
    subset, so one deterministic deletion pass suffices for a finite pool.
    """
    candidates, core = tuple(candidates), tuple(sorted(domains))
    states, checks = 0, []
    def check(names):
        nonlocal states
        offered = candidates if not checks else tuple(c for c in candidates if c.domain in names)
        result = allocate_power_areas(offered,
            domains=names, snapshot=snapshot, maximum_candidates=maximum_candidates,
            maximum_states=maximum_states-states, allow_unassessed_geometry=allow_unassessed_geometry)
        states += result.states
        checks.append((names, result.status, result.states, result.exhausted))
        return result
    incumbent = check(core)
    if incumbent.selected or incumbent.exhausted:
        return AreaConflictCore(core, incumbent, states, incumbent.exhausted, tuple(checks))
    for domain in core:
        if len(core) == 1:
            break
        if states >= maximum_states:
            return AreaConflictCore(core, incumbent, states, True, tuple(checks))
        subset = tuple(n for n in core if n != domain)
        trial = check(subset)
        if trial.exhausted:
            return AreaConflictCore(core, incumbent, states, True, tuple(checks))
        if not trial.selected:
            core, incumbent = subset, trial
    return AreaConflictCore(core, incumbent, states, False, tuple(checks))


def allocate_power_areas(candidates, *, domains, snapshot, reserved_resources=frozenset(),
                         maximum_candidates=128, maximum_states=4096,
                         maximum_dc_nodes=4096, maximum_dc_iterations=2048,
                         allow_unassessed_geometry=False, excluded_assignments=()) -> AreaAllocation:
    """Choose one electrically screened candidate per domain without conflicts.

    Exhaustive within an explicit finite candidate/state budget, with pruning.
    Area then via count rank feasible assignments. Incomplete electrical models
    cannot outrank assessed candidates or be promoted as a feasible allocation.
    No solution within the budget is not proof of physical infeasibility.
    """
    candidates, domains = tuple(candidates), tuple(sorted(domains))
    if not snapshot or not domains or len(set(domains)) != len(domains):
        raise ValueError('unique nonempty domains and snapshot required')
    if any(type(v) is not int or v <= 0 for v in (maximum_candidates, maximum_states, maximum_dc_nodes, maximum_dc_iterations)):
        raise ValueError('allocation bounds must be positive integers')
    if len({c.id for c in candidates}) != len(candidates):
        raise ValueError('candidate IDs must be unique')
    if any(c.domain not in domains or c.snapshot != snapshot for c in candidates):
        raise ValueError('candidate has foreign domain or stale snapshot')
    exclusions = {tuple(sorted(row)) for row in excluded_assignments}
    identities = {c.id: c.domain for c in candidates}
    if any(len(row) != len(domains) or any(i not in identities for i in row)
           or {identities[i] for i in row} != set(domains) for row in exclusions):
        raise ValueError('excluded assignments must identify one known candidate per domain')
    if len(candidates) > maximum_candidates:
        return AreaAllocation('candidate_budget_exhausted', (), (), 0, True, domains)
    assessments, eligible = [], {d: [] for d in domains}
    for c in sorted(candidates, key=lambda c: (c.domain, c.area_nm2, c.via_count, c.id)):
        solution = None
        if c.resources & reserved_resources:
            status = 'reserved_resource_conflict'
        elif c.network is None or not c.minimum_voltages_v:
            status = 'electrical_inputs_missing'
        elif (not {n for n, i in c.network.withdrawals_a.items() if i > 0} <= {n for n, _ in c.minimum_voltages_v}
              or not {n for n, _ in c.minimum_voltages_v} <= c.network.withdrawals_a.keys()):
            status = 'load_limits_missing'
        else:
            solution = solve_dc(c.network, maximum_nodes=maximum_dc_nodes, maximum_iterations=maximum_dc_iterations)
            status = solution.status
            if status == 'solved':
                references = dict(c.voltage_references)
                if (any(n not in solution.voltages_v for n, _ in c.minimum_voltages_v)
                        or any(n not in solution.voltages_v for n in references.values())):
                    status = 'unknown_load_terminal'
                elif any(solution.voltages_v[n] - (solution.voltages_v[references[n]] if n in references else 0.) < float(v)
                         for n, v in c.minimum_voltages_v):
                    status = 'voltage_limit_failed'
                else:
                    status = 'dc_screen_pass'
                    eligible[c.domain].append(c)
        assessments.append((c.id, status, solution))
        if allow_unassessed_geometry and status == 'electrical_inputs_missing':
            eligible[c.domain].append(c)
    missing = tuple(d for d in domains if not eligible[d])
    if missing:
        return AreaAllocation('unresolved', (), tuple(assessments), 0, False, missing)
    # Most constrained first; order influences only budget coverage and ties.
    order = sorted(domains, key=lambda d: (len(eligible[d]), d))
    unassessed = {identity for identity, status, _ in assessments if status != 'dc_screen_pass'}
    best, best_score, states, exhausted = (), None, 0, False
    blocked = {}
    stack = [(0, frozenset(reserved_resources), (), 0, 0, 0)]
    while stack:
        if states >= maximum_states:
            exhausted = True
            break
        depth, used, selected, area, vias, unknown = stack.pop()
        states += 1
        if best_score is not None and (unknown, area, vias) > best_score[:3]:
            continue
        if depth == len(order):
            if tuple(sorted(c.id for c in selected)) in exclusions:
                continue
            score = (unknown, area, vias, tuple(sorted(c.id for c in selected)))
            if best_score is None or score < best_score:
                best, best_score = selected, score
            continue
        available = [c for c in eligible[order[depth]] if not c.resources & used]
        if not available and selected:
            witness = order[depth], tuple(sorted(c.id for c in selected))
            blocked[witness] = (-depth, unknown, area, vias, witness)
            # Retain only sixteen measured dead ends, without another search
            # or a new state allowance. Deep compatible prefixes come first.
            if len(blocked) > 16:
                del blocked[max(blocked, key=blocked.get)]
        for c in reversed(available):
            stack.append((depth+1, used | c.resources, (*selected, c), area+c.area_nm2, vias+c.via_count,
                          unknown+int(c.id in unassessed)))
    status = ('geometry_proposal' if any(c.id in unassessed for c in best) else 'feasible_screen') if best else 'unresolved'
    return AreaAllocation(status, tuple(sorted(best, key=lambda c: c.domain)),
                          tuple(assessments), states, exhausted, () if best else domains,
                          tuple(sorted(blocked, key=blocked.get)) if not best else ())

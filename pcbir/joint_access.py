"""Bounded signal/contact bundle selection on a disposable ownership frame.

Results are physical proposals. Native supply/return preservation and final
routing gates remain separate; this module never adopts a board or waives DRC.
"""
from dataclasses import dataclass, replace

from .copper_ownership import CopperTrial
from .drc import explicit_copper_connectivity, physical_board_digest, run_physical_drc
from .escape_assignment import EscapeAssignmentOptions, EscapeConflicts, _BudgetExceeded, _finite_assignment
from .physical import CopperLayer, DecouplingLink, PadReference, Point, RouteKind, TrackSegment, Via
from .pin_escape import RoutingAccess, verified_fanout_path, verified_routing_access
from .routing_clearance import RoutingClearanceIndex


@dataclass(frozen=True, slots=True)
class AccessRequirement:
    id: str
    kind: str
    net: str
    terminals: tuple[PadReference, ...]
    target_layers: tuple[CopperLayer, ...] = ()
    link: DecouplingLink | None = None

    def __post_init__(self):
        object.__setattr__(self, 'terminals', tuple(self.terminals))
        object.__setattr__(self, 'target_layers', tuple(self.target_layers))
        if (not self.id or not self.net or not self.terminals
                or self.kind not in {'signal', 'contact', 'connection', 'bypass'}):
            raise ValueError('named source access requirement required')
        if self.kind in {'signal', 'contact'} and len(self.terminals) != 1:
            raise ValueError('a port requirement must name one actual terminal')
        if self.kind == 'connection' and len(set(self.terminals)) < 2:
            raise ValueError('a connection requirement must name distinct terminals')
        if self.kind == 'contact' and not self.target_layers:
            raise ValueError('contact requirement must declare prospective zone layers')
        if self.kind == 'bypass' and self.link is None:
            raise ValueError('bypass requirement needs its original source association')


@dataclass(frozen=True, slots=True)
class AccessBundle:
    id: str
    family: str
    snapshot: str
    covers: frozenset[str]
    tracks: tuple[TrackSegment, ...] = ()
    vias: tuple[Via, ...] = ()
    ports: tuple[tuple[PadReference, Point | RoutingAccess], ...] = ()
    shared_vias: frozenset[Via] = frozenset()

    def __post_init__(self):
        for name in ('tracks', 'vias', 'ports'):
            object.__setattr__(self, name, tuple(getattr(self, name)))
        object.__setattr__(self, 'covers', frozenset(self.covers))
        object.__setattr__(self, 'shared_vias', frozenset(self.shared_vias))
        if not self.id or not self.family or not self.snapshot or not self.covers:
            raise ValueError('bundle requires identity, family, snapshot and coverage')
        if not self.shared_vias <= set(self.vias):
            raise ValueError('shared via must belong to the bundle')
        if len({p for p, _ in self.ports}) != len(self.ports):
            raise ValueError('bundle ports must retain unique terminal identities')


@dataclass(frozen=True, slots=True)
class JointAccessOptions:
    maximum_candidates: int = 128
    maximum_complete_checks: int = 4
    assignment: EscapeAssignmentOptions = EscapeAssignmentOptions()

    def __post_init__(self):
        if any(type(v) is not int or v <= 0 for v in (self.maximum_candidates, self.maximum_complete_checks)):
            raise ValueError('joint access work bounds must be positive integers')


@dataclass(frozen=True, slots=True)
class JointAccessResult:
    status: str
    board: object
    selected: tuple[AccessBundle, ...] = ()
    assessments: tuple[tuple[str, str], ...] = ()
    search_states: int = 0
    pair_checks: int = 0
    pair_queries: int = 0
    complete_checks: int = 0
    rejected_assignments: tuple = ()


def _materialize(board, bundles):
    tracks, vias = list(board.tracks), list(board.vias)
    seen_tracks, seen_vias = set(tracks), set(vias)
    for bundle in bundles:
        for target, seen, items in ((tracks, seen_tracks, bundle.tracks), (vias, seen_vias, bundle.vias)):
            for item in items:
                if item not in seen:
                    target.append(item)
                    seen.add(item)
    return replace(board, tracks=tuple(tracks), vias=tuple(vias))


def _hard_findings(board):
    return tuple(f.code for f in run_physical_drc(board).findings if f.severity.value == 'error'
                 and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'})


def _prove_requirement(board, requirement, ports, index):
    if requirement.kind == 'bypass':
        from .package_access import _verified_bypass_links
        return requirement.link in _verified_bypass_links(board, {requirement.net})
    if requirement.kind == 'connection':
        graph = explicit_copper_connectivity(board, only_nets=frozenset({requirement.net}),
                                             include_internal_connections=False)
        nodes = [graph.pad_nodes.get(p, ()) for p in requirement.terminals]
        return all(nodes) and len({graph.roots[n] for group in nodes for n in group}) == 1
    pad = requirement.terminals[0]
    anchor = ports.get(pad)
    if anchor is None:
        return False
    verify = verified_routing_access if isinstance(anchor, RoutingAccess) else verified_fanout_path
    if verify(board, pad, requirement.net, anchor, index) is None:
        return False
    if requirement.kind == 'signal':
        return not requirement.target_layers or (isinstance(anchor, RoutingAccess)
            and anchor.layer in requirement.target_layers) or any(
                v.net == requirement.net and v.position == anchor and any(
                    l in board.stackup.copper_layers[
                        board.stackup.copper_layers.index(v.from_layer):
                        board.stackup.copper_layers.index(v.to_layer)+1] for l in requirement.target_layers)
                for v in board.vias)
    # A declared zone is a prospective target, never a native fill witness.
    from .plane import _via_contacts_zone
    from .placement import resolved_copper_keepouts
    point = anchor.position if isinstance(anchor, RoutingAccess) else anchor
    layers = ({anchor.layer} if isinstance(anchor, RoutingAccess) else
              {l for v in board.vias if v.net == requirement.net and v.position == point
               for l in board.stackup.copper_layers[board.stackup.copper_layers.index(v.from_layer):
                                                    board.stackup.copper_layers.index(v.to_layer)+1]})
    reachable = layers.intersection(requirement.target_layers)
    keepouts = resolved_copper_keepouts(board)
    return any(z.net == requirement.net and _via_contacts_zone(point, z, reachable, keepouts)
               for z in board.zones)


def allocate_joint_access(frame: CopperTrial, requirements, candidates, options=None, *, excluded_assignments=()):
    """One complete alternative per family, preserving all released obligations.

    Candidate validation is finite, followed by shared MRV/forward checking and
    lazy exact multi-via conflicts. Failed or over-budget trials return the exact
    original board. No partial assignment or native-clean flag is returned.
    """
    options = options or JointAccessOptions()
    if not frame.matches():
        raise ValueError('fresh complete disposable ownership frame required')
    from .pad_via_arrays import require_via_in_pad_arrays
    require_via_in_pad_arrays(frame.held, 'joint package access')
    requirements, candidates = tuple(requirements), tuple(candidates)
    required = {r.id: r for r in requirements}
    if len(required) != len(requirements) or not required or not set(frame.requirements) <= required.keys():
        raise ValueError('joint access cannot omit or duplicate a required source obligation')
    uses = {u.id: u for u in frame.ownership.uses}
    membership = {p: n.name for n in frame.held.nets for p in n.pads}
    for r in requirements:
        if r.id in uses and (uses[r.id].kind != r.kind or uses[r.id].terminals != r.terminals):
            raise ValueError('joint requirement changed its source identity')
        if any(p not in membership for p in r.terminals) or any(l not in frame.held.stackup.copper_layers for l in r.target_layers):
            raise ValueError('unknown access terminal or layer')
        if r.kind == 'bypass':
            if r.link not in frame.held.decoupling_links or r.net != r.link.net or not {
                    r.link.target, r.link.capacitor, r.link.return_pad} <= set(r.terminals):
                raise ValueError('bypass obligation differs from source')
        elif any(membership[p] != r.net for p in r.terminals):
            raise ValueError('access requirement has foreign terminal net')
    snapshot = physical_board_digest(frame.held)
    if len({c.id for c in candidates}) != len(candidates) or any(c.snapshot != snapshot for c in candidates):
        raise ValueError('unique candidates from the current held snapshot required')
    if any(not c.covers <= required.keys() for c in candidates):
        raise ValueError('bundle claims an unknown obligation')
    if len(candidates) > options.maximum_candidates:
        return JointAccessResult('candidate_budget_exhausted', frame.original)
    families = sorted({c.family for c in candidates})
    if len(families) > options.assignment.maximum_cluster_pins:
        return JointAccessResult('family_budget_exhausted', frame.original)
    domains, assessments = {f: [] for f in families}, []
    rules = {r.net: r for r in frame.held.net_routing_rules}
    for bundle in sorted(candidates, key=lambda c: (c.family, c.id)):
        reason = ''
        if len(set(bundle.vias)) != len(bundle.vias) or len(set(bundle.tracks)) != len(bundle.tracks):
            reason = 'duplicate_copper_occurrences'
        elif any(rules[t.net].kind is not RouteKind.GENERAL for t in (*bundle.tracks, *bundle.vias) if t.net in rules):
            reason = 'specialized_copper_is_protected'
        if not reason:
            candidate = _materialize(frame.held, (bundle,))
            index = RoutingClearanceIndex(candidate)
            if _hard_findings(candidate):
                reason = 'physical_geometry_rejected'
            elif any(p not in membership for p, _ in bundle.ports):
                reason = 'unknown_port'
            elif not all(_prove_requirement(candidate, required[r], dict(bundle.ports), index) for r in bundle.covers):
                reason = 'unverified_obligation'
        assessments.append((bundle.id, reason or 'eligible_geometry'))
        if not reason:
            domains[bundle.family].append(bundle)
    if not families or any(not values for values in domains.values()) or not set(required) <= {
            r for values in domains.values() for c in values for r in c.covers}:
        return JointAccessResult('unresolved_obligations', frame.original, assessments=tuple(assessments))
    excluded = {tuple(sorted(row)) for row in excluded_assignments}
    known = {c.id: c.family for c in candidates}
    if any(len(row) != len(families) or any(i not in known for i in row)
           or {known[i] for i in row} != set(families) for row in excluded):
        raise ValueError('exclusion must identify one complete known assignment')
    conflicts = EscapeConflicts(frame.held, options.assignment.maximum_pair_checks, options.assignment.maximum_pair_queries)
    states, checks, rejected = 0, 0, []
    def compatible(f, i, g, j):
        a, b = domains[f][i], domains[g][j]
        shared = (set(a.vias) & set(b.vias)) - set(frame.held.vias)
        # Undeclared aliases still consume the query bound, even when geometry
        # would allow reusing the same-net hole.
        geometry = conflicts.compatible((a.tracks, a.vias), (b.tracks, b.vias),
            ordered_lanes=any(required[r].kind == 'signal' for r in a.covers)
                          and any(required[r].kind == 'signal' for r in b.covers))
        return geometry and shared <= (a.shared_vias & b.shared_vias)
    def consume():
        nonlocal states
        if states >= options.assignment.maximum_search_states:
            raise _BudgetExceeded('assignment_state_budget_exhausted')
        states += 1
    def complete(selection):
        nonlocal checks
        bundles = tuple(domains[f][i] for f, i in sorted(selection.items()))
        ids = tuple(sorted(b.id for b in bundles))
        if ids in excluded:
            return False
        if checks >= options.maximum_complete_checks:
            raise _BudgetExceeded('complete_validation_budget_exhausted')
        checks += 1
        candidate = _materialize(frame.held, bundles)
        ports = {p: a for b in bundles for p, a in b.ports}
        if len(ports) != sum(len(b.ports) for b in bundles):
            rejected.append((ids, 'duplicate_terminal_ports'))
            return False
        index = RoutingClearanceIndex(candidate)
        valid = (set(required) <= {r for b in bundles for r in b.covers} and not _hard_findings(candidate)
                 and all(_prove_requirement(candidate, r, ports, index) for r in requirements))
        if not valid:
            rejected.append((ids, 'complete_geometry_or_obligation_rejected'))
        return valid
    try:
        selected = _finite_assignment({f: tuple(range(len(domains[f]))) for f in families}, {},
            preferred={}, rank={f: i for i, f in enumerate(families)}, compatible=compatible,
            consume_state=consume, accept_complete=complete)
        status = 'geometry_proposal' if selected is not None else 'no_compatible_assignment'
    except _BudgetExceeded as error:
        selected, status = None, str(error)
    bundles = tuple(domains[f][i] for f, i in sorted(selected.items())) if selected is not None else ()
    return JointAccessResult(status, _materialize(frame.held, bundles) if bundles else frame.original,
        bundles, tuple(assessments), states, conflicts.checks, conflicts.queries, checks, tuple(rejected))

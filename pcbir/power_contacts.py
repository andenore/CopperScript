"""Finite additive contact arrangements for a prospective power-area layer.

Every arrangement retains source copper. Candidate contacts and the chosen
distribution area are disposable until the caller's complete native check.
"""
from dataclasses import dataclass, replace
from hashlib import sha256
from math import isqrt

from .drc import explicit_copper_connectivity, _explicit_copper_contacts
from .escape_assignment import EscapeConflicts, _BudgetExceeded, _finite_assignment
from .physical import PadKind, PadShape
from .placement import transformed_local_point
from .plane import PlaneStitchOptions, propose_plane_contacts


@dataclass(frozen=True)
class ContactArrangements:
    boards: tuple
    identities: tuple[str, ...]
    observation: dict


def _contact_rank(board, reference, choice, target):
    """Local length first, then outward motion for circular land arrays.

    This only orders a finite pool; inward contacts remain alternatives. An
    outward preference never permits a longer escape beyond source limits.
    """
    pose = next(p for p in board.placements if p.reference == reference.component)
    pad = next(p for p in board.footprints[pose.footprint].pads if p.number == reference.pad)
    start = transformed_local_point(pose, pad.position)
    length = sum(isqrt((t.end.x_nm-t.start.x_nm)**2 + (t.end.y_nm-t.start.y_nm)**2)
                 for t in choice[0])
    lands = tuple(p for p in board.footprints[pose.footprint].pads if p.kind is PadKind.SMD)
    outward = 0
    if target is not None and len(lands) >= 9 and all(p.shape is PadShape.CIRCLE for p in lands):
        centres = tuple(transformed_local_point(pose, p.position) for p in lands)
        dx = len(centres)*start.x_nm-sum(p.x_nm for p in centres)
        dy = len(centres)*start.y_nm-sum(p.y_nm for p in centres)
        outward = (target.x_nm-start.x_nm)*dx + (target.y_nm-start.y_nm)*dy
    return length, -outward, choice[1] is not None, repr(choice)


def propose_contact_arrangements(board, temporary, obligation, layer, selected, options, *, prepare_candidate=None):
    """Enumerate mutually compatible contacts, sharing all finite work bounds.

    ``temporary`` contains only held reservations and a prospective target zone.
    Its appended copper and zone are never transferred to the returned boards.
    A terminal with no direct candidate remains an explicit missing contact;
    the existing bounded layer-bridge producer may subsequently connect it.
    """
    from .power_geometry import _anchors, _AreaClearance
    selected = tuple(sorted(selected))
    pools, terminals, sites, states = {}, [], 0, 0
    stop = 'finite_contact_pool_exhausted'
    rule = next((r for r in board.net_routing_rules if r.net == obligation.net), None)
    clearance = _AreaClearance(temporary, options)
    source_graph = explicit_copper_connectivity(board, only_nets=frozenset({obligation.net}),
                                               include_internal_connections=False)
    root = lambda p: source_graph.roots[source_graph.pad_nodes[p][0]]
    # A source-associated capacitor may reach its target ball's plated access
    # through a local surface bridge. Retain a no-new-via alternative for that
    # component. All terminals still must join the final area tree, and native
    # validation separately checks the original capacitor associations.
    deferred_roots = {root(link.capacitor) for link in obligation.local_links
                      if root(link.capacitor) != root(link.target)}
    stitch = PlaneStitchOptions(preferred_ground_pads=frozenset(), include_surface_zones=True,
        maximum_radius_nm=options.detour_nm, step_nm=min(500000, options.detour_nm),
        maze_state_budget=options.search_states, maze_step_nm=options.step_nm)
    for index, reference in enumerate(selected):
        allowance = (options.contact_site_queries-sites)//(len(selected)-index)
        if allowance <= 0:
            terminals.append(dict(terminal=str(reference), candidates=0, status='contact_site_budget_exhausted'))
            pools[reference] = (((), None),)
            continue
        proposal = propose_plane_contacts(temporary, reference, stitch,
            maximum_alternatives=options.contact_candidates_per_terminal, maximum_sites=allowance)
        sites += proposal.site_queries
        choices = []
        for choice, target in zip(proposal.choices, proposal.targets):
            tracks, via = choice
            if (rule and rule.allowed_layers and any(t.layer not in rule.allowed_layers for t in tracks)
                    or not all(clearance.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in tracks)):
                continue
            # Ordinary plane outlines permit an antipad around a through via.
            # The normal contact proposer checks actual drill/annulus obstacles
            # and reserved regions; a polygon outline is not filled copper.
            # Preservation of existing plane connections needs native refill.
            choices.append((_contact_rank(board, reference, choice, target), choice))
        values = tuple(dict.fromkeys(c for _, c in sorted(choices, key=lambda c: c[0])))
        deferred = bool(options.maximum_branch_layers) and root(reference) in deferred_roots
        shared = bool(options.maximum_branch_layers) and not deferred
        # A package terminal may share another real plated component through
        # the same bounded layer-bridge producer. Prefer its own short contact,
        # but do not make a colliding new drill mandatory. Existing contacts
        # and required arrays remain untouched; every terminal must still join
        # the complete area tree and pass native association/return checks.
        pools[reference] = ((((), None), *values) if deferred else
            (*values, ((), None)) if shared and values else values or (((), None),))
        terminals.append(dict(terminal=str(reference), candidates=len(values), status=proposal.stop_reason,
            site_queries=proposal.site_queries, local_bridge_alternative=deferred,shared_contact_alternative=shared,
            choices=tuple(dict(index=i, via_nm=(via.position.x_nm, via.position.y_nm) if via else None,
                tracks=tuple(dict(start_nm=(t.start.x_nm,t.start.y_nm), end_nm=(t.end.x_nm,t.end.y_nm),
                                  layer=t.layer.value, width_nm=t.width_nm) for t in path))
                for i,(path,via) in enumerate(pools[reference]))))
    conflicts = EscapeConflicts(temporary, options.contact_pair_queries, options.contact_pair_queries)
    boards, identities, seen = [], [], set()
    blocked_pairs = set()
    previous, pending = {}, frozenset()

    def consume():
        nonlocal states
        if states >= options.contact_assignment_states:
            raise _BudgetExceeded('contact_assignment_budget_exhausted')
        states += 1

    def compatible(a, i, b, j):
        clear = conflicts.compatible(pools[a][i], pools[b][j], ordered_lanes=False)
        if not clear and len(blocked_pairs) < 16:
            blocked_pairs.add(tuple(sorted(((str(a),i),(str(b),j)))))
        return clear

    def complete(assignment):
        nonlocal previous, pending
        tracks, vias = list(board.tracks), list(board.vias)
        for reference in selected:
            path, via = pools[reference][assignment[reference]]
            tracks.extend(t for t in path if t not in tracks)
            if via is not None and via not in vias:
                vias.append(via)
        if rule and rule.max_vias is not None and sum(v.net == obligation.net for v in vias) > rule.max_vias:
            return False
        added = tuple(tracks[len(board.tracks):]), tuple(vias[len(board.vias):])
        if added in seen:
            return False
        seen.add(added)
        candidate = replace(board, tracks=tuple(tracks), vias=tuple(vias))
        # All proposed contacts have real explicit copper access to the target
        # layer. Unavailable terminals are left for the separate bridge stage.
        graph = explicit_copper_connectivity(candidate, only_nets=frozenset({obligation.net}),
                                             include_internal_connections=False)
        contacts, _, _ = _explicit_copper_contacts(candidate, only_nets=frozenset({obligation.net}),
                                                   include_internal_connections=False)
        groups = _anchors(candidate, obligation.net, graph, contacts, obligation.terminals,
                          layer, options.maximum_anchors)
        if groups is None or any(not groups[graph.roots[graph.pad_nodes[p][0]]]
                                 for p in selected if pools[p][assignment[p]] != ((), None)):
            return False
        if prepare_candidate is not None:
            candidate, pending = prepare_candidate(candidate, selected)
        previous = dict(assignment)
        boards.append(candidate)
        identities.append(sha256(repr((candidate.tracks[len(board.tracks):],
                                      candidate.vias[len(board.vias):])).encode()).hexdigest()[:20])
        return True

    try:
        for variant in range(options.maximum_contact_alternatives):
            # Vary the entire bank preference, rather than spending every
            # arrangement on choices for only the last terminal in a DFS.
            # Counters, exact conflict cache and rejected geometries persist.
            preferred = {p: variant % len(pools[p]) for p in selected}
            repair = {p: next((i for i,c in enumerate(pools[p]) if c != ((), None)), None)
                      for p in pending if p in pools and pools[p][previous[p]] == ((), None)}
            repair = {p:i for p,i in repair.items() if i is not None}
            if repair:
                # A failed deferred bridge needs a direct contact, not a new
                # via preference for every successful member of the bank.
                # Exact compatibility can still change conflicting neighbors.
                preferred = {**previous, **repair}
            found = _finite_assignment({p: tuple(range(len(pools[p]))) for p in selected}, {},
                preferred=preferred,
                rank={p: i for i, p in enumerate(selected)}, compatible=compatible,
                consume_state=consume, accept_complete=complete)
            if found is None:
                break
        if len(boards) == options.maximum_contact_alternatives:
            stop = 'contact_alternative_limit_reached'
    except _BudgetExceeded as error:
        stop = str(error)
    return ContactArrangements(tuple(boards), tuple(identities), dict(net=obligation.net,
        layer=layer.value, status=stop, arrangements=len(boards), site_queries=sites,
        assignment_states=states, pair_queries=conflicts.queries, pair_checks=conflicts.checks,
        blocked_pairs=tuple(sorted(blocked_pairs)), terminals=tuple(terminals)))

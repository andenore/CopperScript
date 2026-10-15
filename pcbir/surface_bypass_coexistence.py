"""One measured no-pose ordinary-surface/bypass pattern proposal.

One caller-owned pattern slot pays for one domain batch, one joint assignment
and one exact-link feed regeneration. It never reruns fanout or changes source
inventory/metadata/poses. Same-face bypass evidence remains feed-only.
"""
from collections import Counter
from dataclasses import dataclass, fields, replace
from types import MappingProxyType

from .boundary_access import BoundaryAccessOptions, package_collar, surface_escapes
from .bypass_reservations import verified_bypass_reservations
from .contact_owner_regeneration import _hard_findings, _valid_exits
from .decoupling import route_decouplers
from .decoupling_paths import refresh_crosslayer_witnesses
from .escape_assignment import improve_escape_assignment
from .fanout import FanoutOptions, FanoutPinAnalysis, FanoutResult, _surface_layer
from .hard_macros import macro_owned_pads, validate_hard_macros
from .pad_via_arrays import require_via_in_pad_arrays
from .physical import DecouplingLink, PhysicalBoard, RouteKind
from .pin_escape import RoutingAccess, verified_routing_access
from .placement import transformed_pad_position
from .routing_clearance import RoutingClearanceIndex


@dataclass(frozen=True, slots=True)
class SurfaceBypassProposal:
    fanout: FanoutResult | None
    reason: str
    link: DecouplingLink | None = None
    roots: tuple = ()
    released_tracks: tuple = ()
    retained_track_count: int = 0
    retained_via_count: int = 0


def constrained_surface_roots(owner, original):
    """Cheap source-policy eligibility only; no routing or connected flags."""
    poses = {p.reference: p for p in owner.placements}
    names = {pad: net.name for net in owner.nets for pad in net.pads}
    rules = {r.net: r for r in owner.net_routing_rules}
    owned = macro_owned_pads(owner)
    return tuple(sorted(pad for pad in original.pending_pads
        if pad in names and pad.component in poses and pad not in owned
        and (names[pad] not in rules or rules[names[pad]].kind is RouteKind.GENERAL)
        and _surface_layer(owner, names[pad], rules.get(names[pad]), poses[pad.component]) is not None))


def _same_objects(actual, expected):
    return len(actual) == len(expected) and all(a is b for a, b in zip(actual, expected))


def _identity_subtract(created, released):
    """Actual original occurrences, never a net or equal-geometry selector."""
    ids = {id(edge) for edge in released}
    return tuple(edge for edge in created if id(edge) not in ids)


def _legal(index, path):
    return all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in path)


def _exclusive_feed_tracks(original, link, required, paths, via_paths):
    protected_ids = {id(edge) for other in required if other != link
                     for edge in (*paths.get(other, ()), *via_paths.get(other, ()))}
    return tuple(t for t in original.decoupling_tracks
                 if any(t is edge for edge in paths.get(link, ())) and id(t) not in protected_ids)


def propose_surface_bypass(owner: PhysicalBoard, original: FanoutResult,
                           options: BoundaryAccessOptions, *, obstacles=None,
                           fanout_options: FanoutOptions | None = None) -> SurfaceBypassProposal:
    """Return a fully owned proposal or no contribution, with exact rollback.

    Discovery removes only declared generated semantic copper. All domains are
    generated ONCE, then filtered freshly against the actual held frame for ONE
    measured association. Unselected semantic copper is restored before joint
    assignment and before feed generation. Shared ORIGINAL/canonical path edges
    remain protected. No search is rerun per alternative or failed regeneration.
    """
    def reject(reason, link=None, roots=(), released=(), tracks=0, vias=0):
        return SurfaceBypassProposal(None, reason, link, roots, released, tracks, vias)

    settings = fanout_options or FanoutOptions()
    a, b = options.assignment_options, settings.assignment_options
    limits = [value for value in (a.maximum_trials, b.maximum_trials) if value is not None]
    options = replace(options, maze_state_budget=min(options.maze_state_budget, settings.maze_state_budget),
        assignment_options=replace(a, maximum_trials=min(limits) if limits else None,
            maximum_cluster_pins=min(a.maximum_cluster_pins,b.maximum_cluster_pins),
            maximum_search_states=min(a.maximum_search_states,b.maximum_search_states),
            maximum_pair_checks=min(a.maximum_pair_checks,b.maximum_pair_checks),
            maximum_pair_queries=min(a.maximum_pair_queries,b.maximum_pair_queries)))

    if original.boundary_accesses:
        return reject("late_boundary_ownership_unsupported")
    if (not _same_objects(original.board.tracks, (*owner.tracks, *original.created_tracks))
            or not _same_objects(original.board.vias, (*owner.vias, *original.created_vias))
            or any(getattr(original.board, f.name) != getattr(owner, f.name)
                   for f in fields(owner) if f.name not in {"tracks", "vias"})):
        return reject("stale_surface_owner_context")
    from .package_access import _ordinary_probe_frame
    context = _ordinary_probe_frame(owner, obstacles if obstacles is not None else original.board)
    if (context is None or context.zone_fills != owner.zone_fills
            or not _same_objects(context.tracks[:len(original.board.tracks)], original.board.tracks)
            or not _same_objects(context.vias[:len(original.board.vias)], original.board.vias)):
        return reject("stale_surface_obstacle_context")
    extra_tracks = context.tracks[len(original.board.tracks):]
    extra_vias = context.vias[len(original.board.vias):]
    if verified_bypass_reservations(owner, original)[2] is not None:
        return reject("invalid_bypass_reservations")
    if refresh_crosslayer_witnesses(original.board, original.decoupling_witnesses)[1]:
        return reject("stale_original_bypass_witness")
    try:
        validate_hard_macros(owner)
        require_via_in_pad_arrays(owner, "surface-bypass proposal")
    except ValueError:
        return reject("immutable_source_ownership_invalid")
    if not _valid_exits(original.board, original):
        return reject("stale_ordinary_exit")
    pending = constrained_surface_roots(owner, original)
    if not pending or not original.decoupling_tracks:
        return reject("no_constrained_surface_bypass_case")

    # One component cohort, selected by the first actual pending source pad.
    # Overlarge cohorts are unsupported rather than silently dropping members.
    roots = tuple(p for p in pending if p.component == pending[0].component)
    if len(roots) > options.assignment_options.maximum_cluster_pins:
        return reject("surface_cluster_budget_exhausted", roots=roots)
    poses = {p.reference: p for p in owner.placements}
    names = {p: n.name for n in owner.nets for p in n.pads}
    rules = {r.net: r for r in owner.net_routing_rules}
    from .package_access import _verified_bypass_links
    paths, via_paths = {}, {}
    bypass_nets = frozenset(link.net for link in owner.decoupling_links)
    required = _verified_bypass_links(original.board, bypass_nets,
        witness_paths=paths, witness_vias=via_paths,
        admitted_witnesses=original.decoupling_witnesses)
    if not required:
        return reject("no_verified_generated_surface_feed", roots=roots)

    # Actual declared generated occurrences only; the admission gate has already
    # rejected equal/duplicate source or generated semantic geometry ambiguity.
    semantic_ids = {id(t) for t in original.decoupling_tracks}
    semantic_via_ids = {id(v) for v in original.decoupling_vias}
    discovery = replace(owner,
        tracks=(*owner.tracks, *(t for t in original.created_tracks if id(t) not in semantic_ids), *extra_tracks),
        vias=(*owner.vias, *(v for v in original.created_vias if id(v) not in semantic_via_ids), *extra_vias))
    index = RoutingClearanceIndex(discovery)
    area = package_collar(owner, roots[0].component, options.collar_margin_nm).bounds
    domains, refined_domains = {}, {}
    for pad in roots:
        pose = poses[pad.component]
        layer = _surface_layer(owner, names[pad], rules.get(names[pad]), pose)
        start = transformed_pad_position(owner, pose, pad.pad)
        coarse = surface_escapes(discovery, index, names[pad], start, layer, area, options)
        refined = surface_escapes(discovery, index, names[pad], start, layer, area, options, refined=True)
        domains[pad] = tuple(dict.fromkeys((path, None) for path in (*coarse, *refined)))
        refined_domains[pad] = frozenset((path, None) for path in refined)
    baseline_index = RoutingClearanceIndex(context)
    blocked = tuple(p for p in roots if domains[p]
                    and not any(_legal(baseline_index, path) for path, _ in domains[p]))
    if not blocked:
        return reject("no_measured_semantic_surface_blockage", roots=roots)

    selected = None
    for link in owner.decoupling_links:
        if (link not in required or poses[link.capacitor.component].side is not poses[link.target.component].side
                or link.target in macro_owned_pads(owner) or link.capacitor in macro_owned_pads(owner)):
            continue
        release = _exclusive_feed_tracks(original, link, required, paths, via_paths)
        # This narrow producer owns same-surface SUPPLY only. Any other rail or
        # semantic via requirement needs a separate reviewed transaction.
        if not release or any(t.net != link.net for t in release):
            continue
        retained = _identity_subtract(original.created_tracks, release)
        held = replace(owner, tracks=(*owner.tracks, *retained, *extra_tracks),
                       vias=(*owner.vias, *original.created_vias, *extra_vias))
        fresh = RoutingClearanceIndex(held)
        # The only changed physical inputs are these exact selected original
        # occurrences. A previously blocked path legal now is measured evidence
        # for their ownership, without guessing from net names or lock flags.
        if any(any(_legal(fresh, path) for path, _ in domains[p]) for p in blocked):
            selected = link, release, retained, held, fresh
            break
    if selected is None:
        return reject("no_exclusive_single_association_owner", roots=roots)
    link, released, retained, held, held_index = selected
    domains = {p: tuple(c for c in choices if _legal(held_index, c[0])) for p, choices in domains.items()}
    ordered = tuple(sorted(roots, key=lambda p: (len(domains[p]), p)))
    incumbent, greedy = {}, RoutingClearanceIndex(held)
    for pad in ordered:
        for choice, (path, _) in enumerate(domains[pad]):
            if _legal(greedy, path):
                incumbent[pad] = choice
                for track in path:
                    greedy.add_track(track, locked=True)
                break
    selection, assignment = improve_escape_assignment(held, ordered, domains, incumbent,
        lambda pad: domains[pad], options.assignment_options)
    if set(selection) != set(roots):
        return reject("surface_joint_assignment_incomplete", link, roots, released,
                      len(retained), len(original.created_vias))
    port_tracks = tuple(t for p in ordered for t in domains[p][selection[p]][0])
    if (len(set(port_tracks)) != len(port_tracks)
            or Counter(port_tracks) & Counter(held.tracks)):
        return reject("ambiguous_generated_surface_occurrence", link, roots, released)
    ports = {p: RoutingAccess(domains[p][selection[p]][0][-1].end,
        domains[p][selection[p]][0][0].layer,
        transformed_pad_position(owner, poses[p.component], p.pad), domains[p][selection[p]][0])
        for p in ordered}
    frame = replace(held, tracks=(*held.tracks, *port_tracks))
    if _hard_findings(frame) - _hard_findings(original.board):
        return reject("surface_materialization_hard_drc", link, roots, released)
    # Full source declarations/metadata/poses stay present. only_links is an
    # exact production selector, never a board clone with sibling links removed.
    rebuilt = route_decouplers(frame, only_links=frozenset((link,)))
    if (rebuilt.pending or not _same_objects(rebuilt.board.tracks, (*frame.tracks, *rebuilt.created_tracks))
            or not _same_objects(rebuilt.board.vias, (*frame.vias, *rebuilt.created_vias))
            or replace(rebuilt.board, tracks=frame.tracks, vias=frame.vias) != frame
            or any(t.net != link.net for t in rebuilt.created_tracks) or rebuilt.created_vias):
        return reject("selected_surface_feed_regeneration_failed", link, roots, released)
    combined_tracks = (*retained, *port_tracks, *rebuilt.created_tracks)
    combined_vias = original.created_vias
    proposal_board = replace(owner, tracks=(*owner.tracks, *combined_tracks),
                             vias=(*owner.vias, *combined_vias))
    inherited, reason = refresh_crosslayer_witnesses(proposal_board, original.decoupling_witnesses)
    if reason:
        return reject("stale_original_bypass_witness", link, roots, released)
    fresh = RoutingClearanceIndex(proposal_board)
    if any(verified_routing_access(proposal_board, p, names[p], access, fresh) is None
           for p, access in ports.items()):
        return reject("surface_port_lost_by_regeneration", link, roots, released)
    proposed = replace(original, board=proposal_board,
        created_tracks=combined_tracks, created_vias=combined_vias,
        added_track_count=len(combined_tracks), added_via_count=len(combined_vias),
        accesses=MappingProxyType(dict(original.accesses)),
        surface_accesses=MappingProxyType({**(original.surface_accesses or {}), **ports}),
        pending_pads=tuple(p for p in original.pending_pads if p not in ports),
        pin_analysis=tuple(a for a in original.pin_analysis if a.pad not in ports) + tuple(
            FanoutPinAnalysis(p, len(domains[p]), selection[p], "",
                sum(len(path)==2 for path,_ in domains[p]),
                sum(choice in refined_domains[p] for choice in domains[p])) for p in ordered),
        assignment=assignment,
        decoupling_tracks=(*_identity_subtract(original.decoupling_tracks, released), *rebuilt.created_tracks),
        decoupling_witnesses=inherited)
    current = _verified_bypass_links(proposal_board, bypass_nets,
        admitted_witnesses=proposed.decoupling_witnesses)
    if (not required <= current or verified_bypass_reservations(owner, proposed)[2] is not None
            or not original.escaped_pads <= proposed.escaped_pads or not _valid_exits(proposal_board, proposed)
            or _hard_findings(proposal_board) - _hard_findings(original.board)
            or Counter(retained) - Counter(proposed.created_tracks)
            or Counter(original.created_vias) - Counter(proposed.created_vias)):
        return reject("surface_bypass_final_owner_gate", link, roots, released)
    # All returned data is local. The incumbent, index, source and witnesses
    # were never mutated, and every failed branch returned no contribution.
    return SurfaceBypassProposal(proposed, "proposed", link, roots, released,
                                 len(retained), len(original.created_vias))

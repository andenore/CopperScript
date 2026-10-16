"""Pin-associated bypass placement and local surface routing preferences.

No capacitance, maximum distance, impedance or thermal limits are invented.
Fixed source poses, macro ownership, layer intent and clearances remain binding.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from math import hypot
from types import MappingProxyType
from typing import Mapping, TYPE_CHECKING

from .physical import (BoardSide, CopperLayer, DecouplingLink, PadKind, PadReference,
                       PhysicalBoard, Point, RouteKind, TrackSegment, Via, nm_from_mm,
                       component_pair_courtyard_gap_nm)

if TYPE_CHECKING:
    from .decoupling_paths import DecouplingCrossLayerWitness


@dataclass(frozen=True, slots=True)
class DecouplingRouteResult:
    board: PhysicalBoard
    created_tracks: tuple[TrackSegment, ...]
    connected_pads: frozenset[PadReference]
    pending: tuple[tuple[DecouplingLink, str], ...]
    created_vias: tuple[Via, ...] = ()
    supply_return_witnesses: tuple[DecouplingCrossLayerWitness, ...] = ()
    accesses: Mapping[PadReference, Point] | None = None
    # Original complete paths at adoption, separately freshly bound at return.
    admitted_path_witnesses: tuple[DecouplingCrossLayerWitness, ...] = ()


def _surface_candidates(board, index, link, target, cap, width, layer):
    """Cheap exact paths, including a straight lead beyond the protected land.

    A bend at the land center can hit an adjacent pin even when an outward
    lead and the remaining elbow are clear. Derive the lead from real land
    dimensions and applicable clearance; this does not relax either rule.
    """
    from .placement import transformed_pad_position
    from .pin_escape import checked_access_paths
    start = transformed_pad_position(board, target, link.target.pad)
    end = transformed_pad_position(board, cap, link.capacitor.pad)
    yield from checked_access_paths(board, index, link.net, start, end, width, layer)
    land = next(p for p in board.footprints[target.footprint].pads if p.number == link.target.pad)
    nx, ny = start.x_nm-target.position.x_nm, start.y_nm-target.position.y_nm
    if not (nx or ny):
        return
    dx, dy = (1 if nx > 0 else -1 if nx else 0), (1 if ny > 0 else -1 if ny else 0)
    if abs(nx) > 2*abs(ny):
        dy = 0
    elif abs(ny) > 2*abs(nx):
        dx = 0
    rule = next((r for r in board.net_routing_rules if r.net == link.net), None)
    clearance = max(board.rules.minimum_clearance_nm, rule.clearance_nm or 0 if rule else 0)
    length = (max(land.size.width_nm, land.size.height_nm)+width+1)//2+clearance
    for multiplier in (1, 2):
        lead = Point(start.x_nm+dx*length*multiplier, start.y_nm+dy*length*multiplier)
        prefix = next(checked_access_paths(board, index, link.net, start, lead, width, layer), None)
        if prefix is not None:
            for tail in checked_access_paths(board, index, link.net, lead, end, width, layer):
                yield (*prefix, *tail)


def route_decouplers(board, *, only_nets=None, fanout_options=None, only_links=None):
    """Reserve explicit bypass feeds before ordinary package exits.

    Existing same-surface supply paths retain their original semantics.
    Opposite-surface parts require both actual supply and explicit component-
    return paths, through permitted normal vias, before any copper is adopted.
    This proves physical joins, not electrical return/loop qualification.
    Critical-profile nets remain under their specialized router's ownership.
    Unsupported or blocked associations are returned explicitly, never hidden.

    Optional only_links is an exact immutable subset of the ORIGINAL source
    DecouplingLinks. It intersects only_nets without deleting sibling source
    declarations or weakening the all-original-association admission ledger.
    Higher authored supply-net priority reserves its bypasses first; equal
    priorities retain source declaration order. Filters do not alter inventory.
    """
    if only_links is not None:
        if not isinstance(only_links, frozenset) or any(not isinstance(link, DecouplingLink)
                                                       for link in only_links):
            raise TypeError("decoupling link selection must be a frozenset of DecouplingLinks")
        if not only_links <= frozenset(board.decoupling_links):
            raise ValueError("decoupling link selection contains an undeclared source association")
        if not only_links:
            return DecouplingRouteResult(board, (), frozenset(), (), accesses=MappingProxyType({}))
    if not board.decoupling_links:
        return DecouplingRouteResult(board, (), frozenset(), (), accesses=MappingProxyType({}))
    from .placement import transformed_pad_position
    from .routing_clearance import RoutingClearanceIndex
    from .pin_escape import _verified_land_path
    from .surface_path import surface_path_between
    from .hard_macros import macro_owned_pads
    from .routing_layers import routing_layers
    from .route_quality import escape_paths_cross
    from .package_access import _verified_bypass_links
    from .decoupling_paths import (_budget_reason, verify_crosslayer_decoupling,
                                  refresh_crosslayer_witnesses)
    poses = {p.reference: p for p in board.placements}
    owned = macro_owned_pads(board)
    rules = {r.net: r for r in board.net_routing_rules}
    index = RoutingClearanceIndex(board)
    tracks, vias, successful, pending, witnesses, accesses = [], [], set(), [], [], {}
    admitted_witnesses = []
    all_bypass_nets = {link.net for link in board.decoupling_links}
    verified = _verified_bypass_links(board, all_bypass_nets)
    def admission(candidate, link):
        reason = _budget_reason(candidate, link.net)
        if reason:
            return None, reason
        current = _verified_bypass_links(candidate, all_bypass_nets)
        if not (verified | {link}) <= current:
            return None, "would_lose_verified_bypass_association"
        if admitted_witnesses and refresh_crosslayer_witnesses(candidate,tuple(admitted_witnesses))[1]:
            return None, "would_lose_admitted_bypass_path"
        return current, ""
    ordered_links = sorted(board.decoupling_links,
                           key=lambda link: -(rules[link.net].priority if link.net in rules else 0))
    for link in ordered_links:
        if only_links is not None and link not in only_links:
            continue
        if only_nets is not None and link.net not in only_nets:
            continue
        if link.target in owned or link.capacitor in owned:
            continue  # Prescribed local copper is never regenerated.
        target, cap = poses[link.target.component], poses[link.capacitor.component]
        rule = rules.get(link.net)
        if rule is not None and rule.kind is not RouteKind.GENERAL:
            continue  # Explicit RF/critical topology wins, not generic bypass.
        layer = CopperLayer.FRONT if target.side is BoardSide.FRONT else CopperLayer.BACK
        lands = [p for ref, pose in ((link.target, target), (link.capacitor, cap))
                 for p in board.footprints[pose.footprint].pads if p.number == ref.pad]
        if cap.side is not target.side:
            from .decoupling_paths import propose_crosslayer_decoupling
            from .pin_escape import verified_fanout_path
            current = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))
            measured = propose_crosslayer_decoupling(current, link, fanout_options=fanout_options)
            if measured.witness is None:
                pending.append((link, measured.reason))
                continue
            admitted, reason = admission(measured.board, link)
            if reason:
                pending.append((link, reason))
                continue
            verified = admitted
            witness = measured.witness
            witnesses.append((link, witness.return_target))
            admitted_witnesses.append(witness)
            successful.update((link.target, link.capacitor, link.return_pad, witness.return_target))
            for track in measured.created_tracks:
                tracks.append(track)
                index.add_track(track, locked=True)
            for via in measured.created_vias:
                vias.append(via)
                index.add_via(via, locked=True)
            # Actual witnessed through-vias provide onward distribution ports.
            # They remain generated semantic owners, never immutable macros.
            fresh = RoutingClearanceIndex(measured.board)
            for path in (witness.supply_path, witness.return_path):
                for endpoint, choices in ((path.start, path.vias), (path.end, tuple(reversed(path.vias)))):
                    for via in choices:
                        if verified_fanout_path(measured.board, endpoint, path.net, via.position, fresh) is not None:
                            accesses[endpoint] = via.position
                            break
            continue
        if (len(lands) != 2
                or any(p.kind is not PadKind.SMD for p in lands)
                or layer not in routing_layers(board, link.net, rule)):
            pending.append((link, "same-surface SMD path unavailable"))
            continue
        start = transformed_pad_position(board, target, link.target.pad)
        end = transformed_pad_position(board, cap, link.capacitor.pad)
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        # Preserve a verified multi-leg surface chain rather than adding a
        # parallel shortcut on a subsequent call. No via grants connectivity.
        current_board = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias)) if tracks or vias else board
        if _verified_land_path(current_board, lands[0], target, link.net, end, index,
                               require_via=False, target_layer=layer, required_width_nm=width) is not None:
            reason = _budget_reason(current_board, link.net)
            if reason:
                pending.append((link, reason))
                continue
            successful.update((link.target, link.capacitor))
            continue
        candidates = sorted(_surface_candidates(board, index, link, target, cap, width, layer),
                            key=lambda path: (sum(hypot(t.end.x_nm-t.start.x_nm, t.end.y_nm-t.start.y_nm)
                                                      for t in path), len(path)))
        path = next((p for p in candidates if not escape_paths_cross(p, tracks)), None)
        if path is None:
            path = surface_path_between(board, index, link.net, start, end, width, layer)
            if path is not None and escape_paths_cross(path, tracks):
                path = None
        if path is None:
            pending.append((link, "bounded surface candidates blocked"))
            continue
        created = tuple(track for track in path if track not in board.tracks and track not in tracks)
        candidate = replace(board, tracks=(*board.tracks, *tracks, *created),
                            vias=(*board.vias, *vias))
        admitted, reason = admission(candidate, link)
        if reason:
            pending.append((link, reason))
            continue
        verified = admitted
        successful.update((link.target, link.capacitor))
        for track in created:
            tracks.append(track)
            index.add_track(track, locked=True)
    result_board = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias)) if tracks or vias else board
    final_witnesses = ()
    if witnesses or admitted_witnesses:
        fresh_index = RoutingClearanceIndex(result_board)
        final_witnesses = tuple(verify_crosslayer_decoupling(result_board, link,
                                return_target=target, index=fresh_index).witness
                                for link, target in witnesses)
    if any(witness is None for witness in final_witnesses):
        raise ValueError("decoupling batch lost an admitted complete association")
    original_witnesses, reason = refresh_crosslayer_witnesses(result_board,tuple(admitted_witnesses),
        index=fresh_index if admitted_witnesses else None)
    if reason:
        raise ValueError("decoupling batch lost an original admitted path: " + reason)
    return DecouplingRouteResult(result_board, tuple(tracks), frozenset(successful), tuple(pending),
                                 created_vias=tuple(vias), supply_return_witnesses=final_witnesses,
                                 accesses=MappingProxyType(accesses), admitted_path_witnesses=original_witnesses)


def improve_decoupling_placement(board, placements, options):
    """Bounded legal pad-relative relocation after ordinary placement refinement.

    Same side and a legal direct surface path win, then pad-centre distance.
    Opposite-surface poses require fresh complete supply and component-return
    joins through permitted ordinary vias; a prior pose's witness is not reused.
    The search radius derives
    from the two footprints, not an electrical maximum-distance requirement.
    Fixed, attached, macro and required-array members are not moved. A candidate
    may replace only its own exclusive generated feed; every other exact
    source-associated witness remains reserved, including later source links.
    An obstructed layout keeps its existing pose; explicit limits need source
    intent. No count improvement permits losing an already verified identity.
    At most 64 locally legal poses receive a copper-access check per capacitor.
    """
    from .placement import (_fixed_placements, _allowed_orientations, _legal,
                            placement_solution_is_legal, transformed_pad_position)
    from .hard_macros import materialize_hard_macros, macro_owned_pads
    from .routing_clearance import RoutingClearanceIndex
    from .routing_layers import routing_layers
    from .package_access import _verified_bypass_links
    from .pad_via_arrays import array_rules
    from .decoupling_paths import propose_crosslayer_decoupling
    from .drc import DrcSeverity, run_physical_drc
    from .route_quality import escape_paths_cross
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("decoupling placement requires unrouted, unfilled source")
    if not board.decoupling_links:
        return placements, 0
    poses = dict(placements)
    fixed = _fixed_placements(board, poses, options)
    fixed.update({r: poses[r] for c in board.rigid_clusters for r in (m.reference for m in c.members)})
    fixed.update({r.pad.component: poses[r.pad.component] for r in array_rules(board)})
    moved = 0
    # Later-declared valid feeds are just as binding as earlier ones. Generate
    # this complete baseline once, then retain its exact generated prefix while
    # incrementally adding pending feeds. Successful labels alone are not proof.
    frame = materialize_hard_macros(replace(board, placements=tuple(poses.values())))
    if all(link.capacitor.component in fixed for link in board.decoupling_links):
        # This phase only proposes poses. With no movable capacitor, a full
        # disposable routing pass cannot change its result. Source/macro
        # validation above and subsequent package routing remain mandatory.
        return placements, 0
    reservation = route_decouplers(frame)
    baseline_nets = {link.net for link in board.decoupling_links}
    witnesses, via_witnesses = {}, {}
    verified = _verified_bypass_links(reservation.board, baseline_nets,
                                     witness_paths=witnesses, witness_vias=via_witnesses)
    baseline_verified = verified
    pending_links = {link for link, _ in reservation.pending}
    macro_owned = macro_owned_pads(reservation.board)
    owned_tracks = reservation.created_tracks
    owned_vias = reservation.created_vias
    # Physical DRC uses default planner options for its placement finding.
    # This heuristic instead enforces the caller's explicit options below;
    # retain every copper/process finding without substituting that default.
    omitted_hard = {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE", "DRC-PLACEMENT"}
    baseline_hard = {finding.fingerprint for finding in run_physical_drc(reservation.board).findings
                     if finding.severity is DrcSeverity.ERROR
                     and finding.code not in omitted_hard}
    reserved = reservation.board
    for link in board.decoupling_links:
        reference = link.capacitor.component
        if ((link not in pending_links and link not in verified) or reference in fixed
                or link.target in macro_owned or link.capacitor in macro_owned):
            continue
        current, target = poses[reference], poses[link.target.component]
        point = transformed_pad_position(board, target, link.target.pad)
        cap_fp, target_fp = board.footprints[current.footprint], board.footprints[target.footprint]
        # A small off-grid search spans local courtyard edges while allowing
        # orientation of the feed land toward the protected IC pad.
        radius = max(target_fp.body_size.width_nm, target_fp.body_size.height_nm) + \
                 max(cap_fp.body_size.width_nm, cap_fp.body_size.height_nm) + \
                 component_pair_courtyard_gap_nm(board, reference, target.reference,
                                                fallback_nm=options.component_clearance_nm)
        step = max(nm_from_mm("0.1"), radius // 24)
        candidates = [current]
        rule = next((r for r in board.placement_rules if r.reference == reference), None)
        sides = ((rule.side,) if rule and rule.side is not None
                 else tuple(dict.fromkeys((target.side, current.side, BoardSide.FRONT, BoardSide.BACK))))
        for side in sides:
            for angle in _allowed_orientations(board, reference):
                origin = replace(current, position=Point(0, 0), rotation_degrees=angle, side=side)
                offset = transformed_pad_position(board, origin, link.capacitor.pad)
                for distance in range(step, radius+1, step):
                    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)):
                        candidates.append(replace(origin, position=Point(point.x_nm+dx*distance-offset.x_nm,
                                                                         point.y_nm+dy*distance-offset.y_nm)))
        def rank(pose):
            terminal = transformed_pad_position(board, pose, link.capacitor.pad)
            dx, dy = terminal.x_nm-point.x_nm, terminal.y_nm-point.y_nm
            nx, ny = point.x_nm-target.position.x_nm, point.y_nm-target.position.y_nm
            outward = dx*nx+dy*ny
            # Prefer the pin's outward half-cone over across-package placement.
            # This is a ranking hint: obstructed/central pins may still fall back.
            outside = bool(nx or ny) and (outward <= 0 or
                2*outward*outward < (dx*dx+dy*dy)*(nx*nx+ny*ny))
            # Among equally short feeds, point the supply end toward the IC
            # and the return end away. Tangential two-terminal bodies otherwise
            # win arbitrary XY/angle ties and occupy neighbouring pad exits.
            end_on = (False, 0)
            if pose.side is target.side and link.return_pad is not None:
                end = transformed_pad_position(board, pose, link.return_pad.pad)
                ax, ay = end.x_nm-terminal.x_nm, end.y_nm-terminal.y_nm
                end_on = (ax*dx+ay*dy <= 0, abs(ax*dy-ay*dx))
            return (pose.side is not target.side,
                    outside, dx*dx+dy*dy, end_on,
                    pose.position.x_nm, pose.position.y_nm, pose.rotation_degrees)
        routing_rule = next((r for r in board.net_routing_rules if r.net == link.net), None)
        # All initially verified feeds and earlier pending feeds are physical
        # reservations. Source order cannot grant permission to lose a future
        # association that already had an exact valid witness.
        own_path = set(witnesses.get(link, ()))
        other_paths = {track for other, path in witnesses.items() if other != link
                       for track in path}
        # Source/macro tracks never acquire generated ownership. Shared original
        # segments stay in the prefix even if the moving link also used them.
        releasable = own_path - other_paths - set(frame.tracks)
        retained = tuple(track for track in owned_tracks if track not in releasable)
        own_vias = set(via_witnesses.get(link, ()))
        other_vias = {via for other, path in via_witnesses.items() if other != link for via in path}
        releasable_vias = own_vias - other_vias - set(frame.vias)
        retained_vias = tuple(via for via in owned_vias if via not in releasable_vias)
        def direct_access(candidate):
            layer = CopperLayer.FRONT if target.side is BoardSide.FRONT else CopperLayer.BACK
            if layer not in routing_layers(board, link.net, routing_rule):
                return None
            scratch = replace(reserved, placements=tuple(
                candidate if p.reference == reference else poses[p.reference] for p in board.placements),
                tracks=(*frame.tracks, *retained), vias=(*frame.vias, *retained_vias))
            width = routing_rule.width_nm if routing_rule and routing_rule.width_nm else board.rules.default_track_width_nm
            index = RoutingClearanceIndex(scratch)
            # Moving this capacitor's return land must not cut any retained
            # bypass, even if its own feed path is individually clear.
            # Immutable source geometry may intentionally occupy its own macro
            # region. Cheap ordinary checks exclude that prescribed prefix;
            # fresh complete DRC below still detects moved pads cutting it.
            if any(not index.can_track(t.net, t.start, t.end, t.width_nm, t.layer)
                   for t in retained):
                return None
            if any(not index.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer,
                                     drill_nm=v.drill_nm)
                   for v in retained_vias):
                return None
            if candidate.side is not target.side:
                proposed = propose_crosslayer_decoupling(scratch, link)
                if proposed.witness is None:
                    return None
                trial, created, created_vias = proposed.board, proposed.created_tracks, proposed.created_vias
            else:
                path = next((path for path in _surface_candidates(scratch, index, link,
                             target, candidate, width, layer)
                             if not escape_paths_cross(path, scratch.tracks)), None)
                if path is None:
                    return None
                created = tuple(t for t in path if t not in scratch.tracks)
                created_vias = ()
                trial = replace(scratch, tracks=(*scratch.tracks, *created))
            trial_witnesses, trial_via_witnesses = {}, {}
            trial_verified = _verified_bypass_links(trial, baseline_nets,
                                                    witness_paths=trial_witnesses,
                                                    witness_vias=trial_via_witnesses)
            if not (verified | {link}) <= trial_verified:
                return None
            trial_hard = {finding.fingerprint for finding in run_physical_drc(trial).findings
                          if finding.severity is DrcSeverity.ERROR
                          and finding.code not in omitted_hard}
            if trial_hard - baseline_hard:
                return None
            return (trial, (*retained, *created), (*retained_vias, *created_vias),
                    trial_verified, trial_witnesses, trial_via_witnesses)
        current_access = direct_access(current)
        others = {r: p for r, p in poses.items() if r != reference}
        access_trials = 0
        ordered = sorted(candidates, key=rank)
        if not current_access and len(sides) > 1:
            # An unrestricted pending association must not spend its entire
            # 64-pose allowance on the target face. Interleave the existing
            # ranked domains; explicit side rules still have one domain.
            domains = [[p for p in ordered if p.side is side] for side in sides]
            ordered = [domain[i] for i in range(max(map(len, domains)))
                       for domain in domains if i < len(domain)]
        for candidate in ordered:
            if current_access and rank(candidate) >= rank(current):
                break
            if _legal(candidate, others, board, options):
                access_trials += 1
                trial = direct_access(candidate)
                if trial is None:
                    if access_trials >= 64:
                        break
                    continue
                if not placement_solution_is_legal(board, {**poses, reference: candidate}, options):
                    if access_trials >= 64:
                        break
                    continue
                poses[reference] = candidate
                moved += candidate != current
                reserved, owned_tracks, owned_vias, verified, witnesses, via_witnesses = trial
                break
    if moved:
        # The public output is unrouted poses, not a relabelled feed macro.
        # Fresh normal regeneration must preserve identities as well as the
        # internal proposal ledger; otherwise roll back this bounded heuristic.
        regenerated = route_decouplers(materialize_hard_macros(replace(board,
            placements=tuple(poses.values()))))
        if not (baseline_verified | verified) <= _verified_bypass_links(regenerated.board, baseline_nets):
            return dict(placements), 0
        regenerated_hard = {finding.fingerprint for finding in run_physical_drc(regenerated.board).findings
                            if finding.severity is DrcSeverity.ERROR
                            and finding.code not in omitted_hard}
        if regenerated_hard - baseline_hard:
            return dict(placements), 0
    return poses, moved

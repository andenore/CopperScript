"""Pin-associated bypass placement and local surface routing preferences.

No capacitance, maximum distance, impedance or thermal limits are invented.
Fixed source poses, macro ownership, layer intent and clearances remain binding.
"""
from dataclasses import dataclass, replace
from math import hypot

from .physical import (BoardSide, CopperLayer, DecouplingLink, PadKind, PadReference,
                       PhysicalBoard, Point, RouteKind, TrackSegment, nm_from_mm)


@dataclass(frozen=True, slots=True)
class DecouplingRouteResult:
    board: PhysicalBoard
    created_tracks: tuple[TrackSegment, ...]
    connected_pads: frozenset[PadReference]
    pending: tuple[tuple[DecouplingLink, str], ...]


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


def route_decouplers(board, *, only_nets=None):
    """Reserve a short pad-to-pad surface path before ordinary package exits.

    Ground-return vias and remote distribution layer changes are NOT banned.
    Critical-profile nets remain under their specialized router's ownership.
    Unsupported or blocked associations are returned explicitly, never hidden.
    """
    from .placement import transformed_pad_position
    from .routing_clearance import RoutingClearanceIndex
    from .pin_escape import _verified_land_path
    from .surface_path import surface_path_between
    from .hard_macros import macro_owned_pads
    from .routing_layers import routing_layers
    from .route_quality import escape_paths_cross
    poses = {p.reference: p for p in board.placements}
    owned = macro_owned_pads(board)
    rules = {r.net: r for r in board.net_routing_rules}
    index = RoutingClearanceIndex(board)
    tracks, successful, pending = [], set(), []
    for link in board.decoupling_links:
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
        if (cap.side is not target.side or len(lands) != 2
                or any(p.kind is not PadKind.SMD for p in lands)
                or layer not in routing_layers(board, link.net, rule)):
            pending.append((link, "same-surface SMD path unavailable"))
            continue
        start = transformed_pad_position(board, target, link.target.pad)
        end = transformed_pad_position(board, cap, link.capacitor.pad)
        width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        # Preserve a verified multi-leg surface chain rather than adding a
        # parallel shortcut on a subsequent call. No via grants connectivity.
        current_board = replace(board, tracks=(*board.tracks, *tracks)) if tracks else board
        if _verified_land_path(current_board, lands[0], target, link.net, end, index,
                               require_via=False, target_layer=layer, required_width_nm=width) is not None:
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
        successful.update((link.target, link.capacitor))
        for track in path:
            if track not in board.tracks and track not in tracks:
                tracks.append(track)
                index.add_track(track, locked=True)
    return DecouplingRouteResult(replace(board, tracks=(*board.tracks, *tracks)) if tracks else board,
                                 tuple(tracks), frozenset(successful), tuple(pending))


def improve_decoupling_placement(board, placements, options):
    """Bounded legal pad-relative relocation after ordinary placement refinement.

    Same side and a legal direct surface path win, then pad-centre distance.
    The search radius derives
    from the two footprints, not an electrical maximum-distance requirement.
    Fixed, attached and macro members are not moved. An obstructed layout keeps
    its existing legal pose; explicit limits still need source-backed intent.
    At most 64 locally legal poses receive a copper-access check per capacitor.
    """
    from .placement import (_fixed_placements, _allowed_orientations, _legal,
                            placement_solution_is_legal, transformed_pad_position)
    from .hard_macros import materialize_hard_macros
    from .routing_clearance import RoutingClearanceIndex
    from .routing_layers import routing_layers
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("decoupling placement requires unrouted, unfilled source")
    if not board.decoupling_links:
        return placements, 0
    poses = dict(placements)
    fixed = _fixed_placements(board, poses, options)
    fixed.update({r: poses[r] for c in board.rigid_clusters for r in (m.reference for m in c.members)})
    moved = 0
    for link_index, link in enumerate(board.decoupling_links):
        reference = link.capacitor.component
        if reference in fixed:
            continue
        current, target = poses[reference], poses[link.target.component]
        point = transformed_pad_position(board, target, link.target.pad)
        cap_fp, target_fp = board.footprints[current.footprint], board.footprints[target.footprint]
        # A small off-grid search spans local courtyard edges while allowing
        # orientation of the feed land toward the protected IC pad.
        radius = max(target_fp.body_size.width_nm, target_fp.body_size.height_nm) + \
                 max(cap_fp.body_size.width_nm, cap_fp.body_size.height_nm) + options.component_clearance_nm
        step = max(nm_from_mm("0.1"), radius // 24)
        candidates = [current]
        rule = next((r for r in board.placement_rules if r.reference == reference), None)
        side = rule.side if rule and rule.side is not None else target.side
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
            return (pose.side is not target.side,
                    outside, dx*dx+dy*dy,
                    pose.position.x_nm, pose.position.y_nm, pose.rotation_degrees)
        routing_rule = next((r for r in board.net_routing_rules if r.net == link.net), None)
        ordinary = routing_rule is None or routing_rule.kind is RouteKind.GENERAL
        # Earlier bypasses are physical reservations, including fixed caps.
        # Independently accessible poses can otherwise compete for the same
        # corridor and fail as soon as the complete set is actually routed.
        reservation = route_decouplers(materialize_hard_macros(replace(board,
            placements=tuple(poses.values()), decoupling_links=board.decoupling_links[:link_index])))
        reserved = reservation.board
        def direct_access(candidate):
            if not ordinary:
                return True  # A stronger topology remains with its own router.
            layer = CopperLayer.FRONT if target.side is BoardSide.FRONT else CopperLayer.BACK
            if candidate.side is not target.side or layer not in routing_layers(board, link.net, routing_rule):
                return False
            scratch = replace(reserved, placements=tuple(
                candidate if p.reference == reference else poses[p.reference] for p in board.placements))
            width = routing_rule.width_nm if routing_rule and routing_rule.width_nm else board.rules.default_track_width_nm
            index = RoutingClearanceIndex(scratch)
            # Moving this capacitor's return land must not cut an earlier
            # bypass, even if its own feed path is individually clear.
            if any(not index.can_track(t.net, t.start, t.end, t.width_nm, t.layer)
                   for t in reservation.created_tracks):
                return False
            return next(_surface_candidates(scratch, index, link,
                                            target, candidate, width, layer), None) is not None
        current_access = direct_access(current)
        others = {r: p for r, p in poses.items() if r != reference}
        access_trials = 0
        for candidate in sorted(candidates, key=rank):
            if current_access and rank(candidate) >= rank(current):
                break
            if _legal(candidate, others, board, options):
                access_trials += 1
                if not direct_access(candidate):
                    if access_trials >= 64:
                        break
                    continue
                if not placement_solution_is_legal(board, {**poses, reference: candidate}, options):
                    if access_trials >= 64:
                        break
                    continue
                poses[reference] = candidate
                moved += candidate != current
                break
    return poses, moved

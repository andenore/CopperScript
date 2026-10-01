"""Bounded macro legalization and whole-cluster local refinement.

Search dependencies are imported inside the helpers to keep pose transforms
independent of placement search. Ordinary components never refine a member alone.
"""

from dataclasses import replace
from decimal import Decimal

from .clusters import cluster_placements, resolved_cluster_keepouts
from .physical import Point


def place_rigid_clusters(board, targets, original, options):
    from .placement import (
        PlacementAlgorithmError, _candidate_positions, _fixed_placements,
        _hpwl, _legal, transformed_local_point,
    )

    if not board.rigid_clusters:
        return targets, original, options
    targets, original = dict(targets), dict(original)
    fixed = _fixed_placements(board, original, options)
    all_members = {item.reference for cluster in board.rigid_clusters for item in cluster.members}
    # Unrelated fixed components are obstacles, not movable cluster companions.
    placed = {ref: item for ref, item in fixed.items() if ref not in all_members}
    clusters = sorted(board.rigid_clusters, key=lambda cluster: (
        not any(item.reference in fixed for item in cluster.members), cluster.name
    ))
    for cluster in clusters:
        local_anchor = next(item for item in cluster.members if item.reference == cluster.anchor.reference)
        target = targets[cluster.anchor.reference]
        fixed_members = [item for item in cluster.members if item.reference in fixed]
        if fixed_members:
            member = fixed_members[0]
            known = fixed[member.reference]
            rotation = ((known.rotation_degrees - member.rotation_degrees) % Decimal(360)
                        + Decimal(360)) % Decimal(360)
            frame = replace(known, rotation_degrees=rotation)
            target = replace(original[cluster.anchor.reference],
                             position=transformed_local_point(frame, Point(
                                 local_anchor.position.x_nm - member.position.x_nm,
                                 local_anchor.position.y_nm - member.position.y_nm)),
                             rotation_degrees=rotation)
            poses = (target,)
        else:
            positions = (target.position, *_candidate_positions(board, target, options))
            # The budget bounds distinct anchor locations, not individual members.
            positions = tuple(dict.fromkeys(positions))[:options.legalization_candidates]
            poses = tuple(replace(target, position=point, rotation_degrees=angle)
                          for point in positions for angle in cluster.allowed_rotations)
        best = None
        for pose in poses:
            try:
                members = cluster_placements(board, cluster, pose, original)
            except ValueError:
                continue
            if any(members[item.reference] != fixed[item.reference] for item in fixed_members):
                continue
            trial = {**placed, **members}
            keepouts = resolved_cluster_keepouts(board, trial)
            if not all(_legal(item, {ref: other for ref, other in trial.items() if ref != item.reference},
                              board, options, cluster_keepouts=keepouts) for item in trial.values()):
                continue
            rank = (_hpwl(board, {**targets, **trial}),
                    abs(pose.position.x_nm - target.position.x_nm)
                    + abs(pose.position.y_nm - target.position.y_nm),
                    pose.position.y_nm, pose.position.x_nm, pose.rotation_degrees)
            if best is None or rank < best[0]:
                best = (rank, members)
        if best is None:
            raise PlacementAlgorithmError(f"cannot legalize rigid cluster {cluster.name!r} within its candidate budget")
        placed.update(best[1])
        original.update(best[1])
        targets.update(best[1])
    return targets, original, replace(options, fixed_references=options.fixed_references | all_members)


def refine_rigid_clusters(board, source, options):
    from .placement import (
        _fast_score, _fixed_placements, _nearby_positions, placement_metrics,
        placement_solution_is_legal,
    )

    placements = dict(source)
    fixed = set(_fixed_placements(board, source, options))
    moves = 0
    for _ in range(options.refinement_passes):
        changed = False
        for cluster in sorted(board.rigid_clusters, key=lambda item: item.name):
            if any(item.reference in fixed for item in cluster.members):
                continue
            anchor = placements[cluster.anchor.reference]
            best = placements
            best_rank = (_fast_score(board, placements), anchor.position.y_nm,
                         anchor.position.x_nm, anchor.rotation_degrees)
            baseline = placement_metrics(board, placements, options)
            for point in (anchor.position, *_nearby_positions(anchor.position, options)):
                for angle in cluster.allowed_rotations:
                    pose = replace(anchor, position=point, rotation_degrees=angle)
                    trial = {**placements, **cluster_placements(board, cluster, pose, placements)}
                    if not placement_solution_is_legal(board, trial, options):
                        continue
                    rank = (_fast_score(board, trial), point.y_nm, point.x_nm, angle)
                    if rank >= best_rank:
                        continue
                    metrics = placement_metrics(board, trial, options)
                    if (metrics.constraint_penalty_nm > baseline.constraint_penalty_nm
                            or metrics.congestion_overflow > baseline.congestion_overflow):
                        continue
                    best, best_rank = trial, rank
            if best is not placements:
                placements = best
                moves += 1
                changed = True
        if not changed:
            break
    return placements, moves

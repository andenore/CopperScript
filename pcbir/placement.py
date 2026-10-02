"""Deterministic, constraint-aware PCB component placement algorithms.

The engine deliberately separates global estimation, legalization, detailed
refinement, and candidate selection. It consumes only physical IR and never
creates routed copper.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from decimal import Decimal
from math import cos, exp, hypot, radians, sin
from typing import Iterable, Mapping

from .clusters import (
    cluster_placement_matches, resolved_cluster_keepouts,
)
from .cluster_placement import (
    place_rigid_clusters as _place_rigid_clusters,
    refine_rigid_clusters as _refine_rigid_clusters,
)

from .physical import (
    AlignmentAxis,
    BoardOutline,
    BoardSide,
    ComponentPlacementRule,
    CopperKeepout,
    CopperLayer,
    Nanometres,
    PhysicalBoard,
    Placement,
    PlacementGroup,
    PlacementTarget,
    Point,
    PolygonRing,
    PolygonWithHoles,
    RelativePlacementKind,
    nm_from_mm,
)


@dataclass(frozen=True, slots=True)
class PlacementPlannerOptions:
    grid_step_nm: Nanometres = nm_from_mm("1")
    edge_clearance_nm: Nanometres = nm_from_mm("2")
    component_clearance_nm: Nanometres = nm_from_mm("0.5")
    congestion_bin_nm: Nanometres = nm_from_mm("5")
    analytical_iterations: int = 40
    analytical_step_mm: float = 0.08
    analytical_momentum: float = 0.85
    refinement_passes: int = 2
    refinement_radius_steps: int = 2
    candidate_count: int = 3
    legalization_candidates: int = 96
    exact_repair_limit: int = 6
    exact_repair_candidates: int = 28
    fixed_references: frozenset[str] = field(default_factory=frozenset)

    def __post_init__(self) -> None:
        positive = (
            self.grid_step_nm,
            self.edge_clearance_nm,
            self.component_clearance_nm,
            self.congestion_bin_nm,
        )
        if any(value <= 0 for value in positive):
            raise ValueError("placement planner dimensions must be positive")
        if self.analytical_iterations < 0 or self.analytical_step_mm <= 0:
            raise ValueError("analytical placement settings are invalid")
        if not 0 <= self.analytical_momentum < 1:
            raise ValueError("analytical momentum must be in [0, 1)")
        if self.refinement_passes < 0 or self.refinement_radius_steps < 1:
            raise ValueError("placement refinement settings are invalid")
        if self.candidate_count < 1:
            raise ValueError("placement candidate count must be positive")
        if (
            self.legalization_candidates < 1
            or self.exact_repair_limit < 2
            or self.exact_repair_candidates < 1
        ):
            raise ValueError("exact repair settings are invalid")
        object.__setattr__(self, "fixed_references", frozenset(self.fixed_references))


@dataclass(frozen=True, slots=True)
class PlacementMetrics:
    component_count: int
    net_count: int
    estimated_connection_count: int
    half_perimeter_wire_length_nm: Nanometres
    crossing_count: int
    estimated_via_count: int
    pin_escape_pressure: int
    high_pin_spacing_penalty_nm: Nanometres
    constraint_penalty_nm: Nanometres
    minimum_constraint_margin_nm: Nanometres
    group_spread_nm: Nanometres
    congestion_bin_count: int
    congestion_capacity_per_bin: int
    congestion_overflow: int
    maximum_congestion_utilization_ppm: int

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.constraint_penalty_nm,
            self.congestion_overflow,
            self.crossing_count,
            self.estimated_via_count,
            self.pin_escape_pressure,
            -self.minimum_constraint_margin_nm,
            self.half_perimeter_wire_length_nm + self.high_pin_spacing_penalty_nm,
            self.half_perimeter_wire_length_nm,
            self.group_spread_nm,
        )


@dataclass(frozen=True, slots=True)
class PlacementCandidate:
    candidate_id: str
    seed: int
    placements: tuple[Placement, ...]
    metrics: PlacementMetrics
    statistics: "PlacementStatistics"


@dataclass(frozen=True, slots=True)
class PlacementStatistics:
    analytical_iterations: int
    exact_repair_count: int
    relative_repair_count: int
    refinement_move_count: int
    refinement_swap_count: int
    routing_feedback_passes: int


class PlacementAlgorithmError(ValueError):
    pass


def generate_placement_candidates(
    board: PhysicalBoard,
    options: PlacementPlannerOptions | None = None,
) -> tuple[PlacementCandidate, ...]:
    """Return a deterministic Pareto frontier of legal placement candidates."""

    options = options or PlacementPlannerOptions()
    if board.tracks or board.vias:
        raise PlacementAlgorithmError(
            "placement planning requires an unrouted board; existing copper cannot be moved safely"
        )
    placements = {item.reference: item for item in board.placements}
    unknown_fixed = sorted(options.fixed_references - placements.keys())
    if unknown_fixed:
        raise PlacementAlgorithmError(
            f"fixed placement references unknown component {unknown_fixed[0]!r}"
        )
    attempts: list[PlacementCandidate] = []
    seeds = max(2, options.candidate_count)
    for seed in range(seeds):
        continuous = _initial_seed(board, placements, options, seed)
        continuous = _analytical_place(board, continuous, options)
        continuous, seeded_original, phase_options = _place_rigid_clusters(
            board, continuous, placements, options
        )
        legalized, exact_repairs = _legalize(
            board, continuous, seeded_original, phase_options, seed
        )
        legalized, relative_before = _repair_relative_constraints(
            board, legalized, phase_options
        )
        refined, moves, swaps, feedback_passes = _detailed_refine(
            board, legalized, phase_options, seed
        )
        refined, relative_after = _repair_relative_constraints(
            board, refined, phase_options
        )
        refined, cluster_moves = _refine_rigid_clusters(board, refined, options)
        moves += cluster_moves
        refined = _spread_high_pin_components(board, refined, options)
        metrics = placement_metrics(board, refined, options)
        if metrics.constraint_penalty_nm or not placement_solution_is_legal(board, refined, options):
            continue
        attempts.append(
            PlacementCandidate(
                candidate_id=f"candidate-{seed:02d}",
                seed=seed,
                placements=tuple(refined[item.reference] for item in board.placements),
                metrics=metrics,
                statistics=PlacementStatistics(
                    options.analytical_iterations,
                    exact_repairs,
                    relative_before + relative_after,
                    moves,
                    swaps,
                    feedback_passes,
                ),
            )
        )
    if not attempts:
        raise PlacementAlgorithmError(
            "no legal placement candidate satisfies all represented physical constraints"
        )
    frontier = [
        candidate
        for candidate in attempts
        if not any(
            _dominates(other.metrics, candidate.metrics)
            for other in attempts
            if other is not candidate
        )
    ]
    frontier.sort(key=lambda item: (*item.metrics.quality_vector, item.seed))
    return tuple(frontier[: options.candidate_count])


def select_placement_candidate(
    candidates: tuple[PlacementCandidate, ...],
) -> PlacementCandidate:
    if not candidates:
        raise PlacementAlgorithmError("cannot select from an empty candidate set")
    return min(candidates, key=lambda item: (*item.metrics.quality_vector, item.seed))


def placement_metrics(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> PlacementMetrics:
    route = _coarse_route(board, placements, options)
    return PlacementMetrics(
        component_count=len(placements),
        net_count=len(board.nets),
        estimated_connection_count=route.connection_count,
        half_perimeter_wire_length_nm=_hpwl(board, placements),
        crossing_count=route.crossing_count,
        estimated_via_count=route.estimated_vias,
        pin_escape_pressure=_pin_escape_pressure(board, placements),
        high_pin_spacing_penalty_nm=_high_pin_spacing_penalty(board, placements),
        constraint_penalty_nm=_relative_penalty(board, placements),
        minimum_constraint_margin_nm=_relative_margin(board, placements),
        group_spread_nm=_group_spread(board, placements),
        congestion_bin_count=route.bin_count,
        congestion_capacity_per_bin=route.capacity,
        congestion_overflow=route.overflow,
        maximum_congestion_utilization_ppm=route.maximum_utilization_ppm,
    )


def transformed_pad_position(
    board: PhysicalBoard,
    placement: Placement,
    pad_number: str,
) -> Point:
    footprint = board.footprints[placement.footprint]
    try:
        pad = next(item for item in footprint.pads if item.number == pad_number)
    except StopIteration as exc:
        raise PlacementAlgorithmError(
            f"component {placement.reference!r} has no pad {pad_number!r}"
        ) from exc
    return transformed_local_point(placement, pad.position)


def transformed_local_point(placement: Placement, point: Point) -> Point:
    """Transform one footprint-local point without requiring a unique pad number."""

    offset = _transform_local(point, placement.rotation_degrees, placement.side)
    return Point(
        placement.position.x_nm + offset.x_nm,
        placement.position.y_nm + offset.y_nm,
    )


def resolved_copper_keepouts(board: PhysicalBoard) -> tuple[CopperKeepout, ...]:
    """Return board keepouts plus footprint-local keepouts at placed locations."""

    result = [*board.copper_keepouts, *resolved_cluster_keepouts(
        board, {item.reference: item for item in board.placements}
    )]
    seen = {keepout.id for keepout in result}
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        for local in footprint.keepouts:
            resolved_id = f"{placement.reference}/{local.id}"
            if resolved_id in seen:
                raise ValueError(f"duplicate resolved copper keepout id {resolved_id!r}")
            seen.add(resolved_id)
            layers = tuple(
                CopperLayer.BACK if placement.side is BoardSide.BACK and layer is CopperLayer.FRONT
                else CopperLayer.FRONT if placement.side is BoardSide.BACK and layer is CopperLayer.BACK
                else layer
                for layer in local.layers
            )
            outer = PolygonRing(tuple(
                transformed_local_point(placement, point)
                for point in local.outline.outer.vertices
            ))
            holes = tuple(
                PolygonRing(tuple(
                    transformed_local_point(placement, point)
                    for point in hole.vertices
                ))
                for hole in local.outline.holes
            )
            result.append(
                CopperKeepout(
                    id=resolved_id,
                    layers=layers,
                    outline=PolygonWithHoles(outer, holes),
                    block_tracks=local.block_tracks,
                    block_vias=local.block_vias,
                    block_pads=local.block_pads,
                    block_zones=local.block_zones,
                    block_footprints=local.block_footprints,
                )
            )
    return tuple(result)


def transformed_footprint_polygon(
    board: PhysicalBoard, placement: Placement
) -> tuple[Point, ...]:
    """Return the placed courtyard, or conservative body polygon."""

    return _placement_polygon(board, placement)


def placement_solution_is_legal(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions | None = None,
) -> bool:
    """Return whether a complete placement satisfies represented hard rules."""

    options = options or PlacementPlannerOptions()
    original = {item.reference: item for item in board.placements}
    if set(placements) != set(original):
        return False
    if any(item.reference != reference or item.footprint != original[reference].footprint
           for reference, item in placements.items()):
        return False
    if not cluster_placement_matches(board, placements):
        return False
    fixed = _fixed_placements(board, original, options)
    if any(placements[reference] != expected for reference, expected in fixed.items()):
        return False
    accepted: dict[str, Placement] = {}
    keepouts = resolved_cluster_keepouts(board, placements)
    for reference in sorted(placements):
        candidate = placements[reference]
        if not _legal(candidate, accepted, board, options, cluster_keepouts=keepouts):
            return False
        accepted[reference] = candidate
    return _relative_penalty(board, placements) == 0


def _dominates(left: PlacementMetrics, right: PlacementMetrics) -> bool:
    a = left.quality_vector
    b = right.quality_vector
    return all(x <= y for x, y in zip(a, b, strict=True)) and any(
        x < y for x, y in zip(a, b, strict=True)
    )


def _rules(board: PhysicalBoard) -> dict[str, ComponentPlacementRule]:
    return {rule.reference: rule for rule in board.placement_rules}


def _fixed_placements(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> dict[str, Placement]:
    result: dict[str, Placement] = {}
    rules = _rules(board)
    for reference in sorted(source):
        rule = rules.get(reference)
        fixed_by_rule = rule is not None and rule.fixed_position is not None
        if reference not in options.fixed_references and not fixed_by_rule:
            continue
        placement = source[reference]
        if fixed_by_rule:
            placement = replace(
                placement,
                position=rule.fixed_position,
                rotation_degrees=(
                    rule.fixed_rotation_degrees
                    if rule.fixed_rotation_degrees is not None
                    else placement.rotation_degrees
                ),
                side=rule.side or placement.side,
            )
        result[reference] = placement
    return result


def _allowed_orientations(board: PhysicalBoard, reference: str) -> tuple[Decimal, ...]:
    rule = _rules(board).get(reference)
    return (
        tuple(rule.allowed_orientations)
        if rule is not None
        else (Decimal(0), Decimal(90), Decimal(180), Decimal(270))
    )


def _initial_seed(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    seed: int,
) -> dict[str, Placement]:
    bounds = _outline_bounds(board.outline)
    fixed = _fixed_placements(board, source, options)
    result = dict(fixed)
    primary_group: dict[str, PlacementGroup] = {}
    for group in sorted(board.placement_groups, key=lambda item: (-item.priority, item.name)):
        for reference in group.references:
            primary_group.setdefault(reference, group)
    groups = sorted(
        {group.name: group for group in primary_group.values()}.values(),
        key=lambda item: (-item.priority, item.name),
    )
    center_x = (bounds[0] + bounds[2]) // 2
    center_y = (bounds[1] + bounds[3]) // 2
    group_centers: dict[str, Point] = {}
    group_step = max(options.congestion_bin_nm * 2, nm_from_mm("10"))
    for index, group in enumerate(groups):
        dx, dy = _spiral_offset(index + seed, max(1, len(groups)))
        group_centers[group.name] = Point(
            center_x + dx * group_step,
            center_y + dy * group_step,
        )
    members_seen: dict[str, int] = {}
    movable = sorted(
        (reference for reference in source if reference not in fixed),
        key=lambda reference: (
            -primary_group.get(reference, PlacementGroup("_", (reference,))).priority,
            -_footprint_area(board, source[reference]),
            reference,
        ),
    )
    for index, reference in enumerate(movable):
        original = source[reference]
        group = primary_group.get(reference)
        if group is not None:
            member_index = members_seen.get(group.name, 0)
            members_seen[group.name] = member_index + 1
            dx, dy = _spiral_offset(member_index + seed, len(group.references))
            target = group_centers[group.name]
            point = Point(
                target.x_nm + dx * options.grid_step_nm * 2,
                target.y_nm + dy * options.grid_step_nm * 2,
            )
        else:
            point = _seed_transform(original.position, bounds, seed, index)
        orientations = _allowed_orientations(board, reference)
        rotation = orientations[(seed + index) % len(orientations)]
        result[reference] = replace(original, position=point, rotation_degrees=rotation)
    return result


def _spiral_offset(index: int, count: int) -> tuple[int, int]:
    if index == 0:
        return 0, 0
    layer = 1
    while (2 * layer + 1) ** 2 <= index:
        layer += 1
    side = 2 * layer
    offset = index - (2 * layer - 1) ** 2
    segment, position = divmod(offset, side)
    if segment == 0:
        return layer, -layer + position
    if segment == 1:
        return layer - position, layer
    if segment == 2:
        return -layer, layer - position
    return -layer + position, -layer


def _seed_transform(
    point: Point,
    bounds: tuple[int, int, int, int],
    seed: int,
    index: int,
) -> Point:
    min_x, min_y, max_x, max_y = bounds
    mode = seed % 4
    if mode == 1:
        point = Point(max_x - (point.x_nm - min_x), point.y_nm)
    elif mode == 2:
        point = Point(point.x_nm, max_y - (point.y_nm - min_y))
    elif mode == 3:
        point = Point(
            min_x + (point.y_nm - min_y) * (max_x - min_x) // max(1, max_y - min_y),
            min_y + (point.x_nm - min_x) * (max_y - min_y) // max(1, max_x - min_x),
        )
    jitter = ((index * 17 + seed * 31) % 5 - 2) * nm_from_mm("0.5")
    return Point(point.x_nm + jitter, point.y_nm - jitter)


def _analytical_place(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> dict[str, Placement]:
    if not source or options.analytical_iterations == 0:
        return dict(source)
    fixed = set(_fixed_placements(board, source, options))
    coordinates = {
        reference: [item.position.x_nm / 1_000_000, item.position.y_nm / 1_000_000]
        for reference, item in source.items()
    }
    velocity = {reference: [0.0, 0.0] for reference in source}
    gamma = max(1.0, options.congestion_bin_nm / 1_000_000)
    bounds = _outline_bounds(board.outline)

    for _ in range(options.analytical_iterations):
        gradient = {reference: [0.0, 0.0] for reference in source}
        for net in board.nets:
            pads = [pad for pad in net.pads if pad.component in source]
            if len(pads) < 2:
                continue
            xs: list[float] = []
            ys: list[float] = []
            for pad in pads:
                offset = _pad_offset_mm(board, source[pad.component], pad.pad)
                xs.append(coordinates[pad.component][0] + offset[0])
                ys.append(coordinates[pad.component][1] + offset[1])
            gx = _smooth_span_gradient(xs, gamma)
            gy = _smooth_span_gradient(ys, gamma)
            for pad, dx, dy in zip(pads, gx, gy, strict=True):
                gradient[pad.component][0] += dx
                gradient[pad.component][1] += dy

        references = sorted(source)
        for index, left in enumerate(references):
            left_width, left_height = _half_extents_mm(board, source[left])
            for right in references[index + 1 :]:
                if source[left].side is not source[right].side:
                    continue
                right_width, right_height = _half_extents_mm(board, source[right])
                dx = coordinates[left][0] - coordinates[right][0]
                dy = coordinates[left][1] - coordinates[right][1]
                dense_gap = _high_pin_target_gap(board, source[left], source[right]) / 1_000_000
                # Keep global repulsion gentle; exact clearance is only a soft
                # refinement score because fixed/proximity rules may need space.
                clearance = max(options.component_clearance_nm / 1_000_000, min(2.0, dense_gap))
                limit_x = left_width + right_width + clearance
                limit_y = left_height + right_height + clearance
                overlap_x = limit_x - abs(dx)
                overlap_y = limit_y - abs(dy)
                if overlap_x <= 0 or overlap_y <= 0:
                    continue
                if overlap_x < overlap_y:
                    force = min(4.0, overlap_x) * (1 if dx >= 0 else -1)
                    gradient[left][0] -= force
                    gradient[right][0] += force
                else:
                    force = min(4.0, overlap_y) * (1 if dy >= 0 else -1)
                    gradient[left][1] -= force
                    gradient[right][1] += force

        for group in board.placement_groups:
            members = [reference for reference in group.references if reference in coordinates]
            if len(members) < 2:
                continue
            center_x = sum(coordinates[item][0] for item in members) / len(members)
            center_y = sum(coordinates[item][1] for item in members) / len(members)
            strength = min(0.25, 0.01 + max(0, group.priority) / 4000)
            for reference in members:
                gradient[reference][0] += (coordinates[reference][0] - center_x) * strength
                gradient[reference][1] += (coordinates[reference][1] - center_y) * strength

        for reference in sorted(source):
            if reference in fixed:
                continue
            velocity[reference][0] = (
                options.analytical_momentum * velocity[reference][0]
                - options.analytical_step_mm * gradient[reference][0]
            )
            velocity[reference][1] = (
                options.analytical_momentum * velocity[reference][1]
                - options.analytical_step_mm * gradient[reference][1]
            )
            coordinates[reference][0] += velocity[reference][0]
            coordinates[reference][1] += velocity[reference][1]
            _project_coordinate(board, source[reference], coordinates[reference], bounds, options)

    result = {
        reference: replace(
            item,
            position=Point(
                round(coordinates[reference][0] * 1_000_000),
                round(coordinates[reference][1] * 1_000_000),
            ),
        )
        for reference, item in source.items()
    }
    return _choose_orientations(board, result, fixed)


def _smooth_span_gradient(values: list[float], gamma: float) -> list[float]:
    maximum = max(values)
    minimum = min(values)
    positive = [exp((value - maximum) / gamma) for value in values]
    negative = [exp((minimum - value) / gamma) for value in values]
    sum_positive = sum(positive)
    sum_negative = sum(negative)
    return [
        pos / sum_positive - neg / sum_negative
        for pos, neg in zip(positive, negative, strict=True)
    ]


def _project_coordinate(
    board: PhysicalBoard,
    placement: Placement,
    coordinate: list[float],
    bounds: tuple[int, int, int, int],
    options: PlacementPlannerOptions,
) -> None:
    half_width, half_height = _half_extents_mm(board, placement)
    min_x, min_y, max_x, max_y = (value / 1_000_000 for value in bounds)
    rule = _rules(board).get(placement.reference)
    if rule is not None and rule.region is not None:
        region = next(item for item in board.regions if item.name == rule.region)
        region_bounds = _outline_bounds(region.outline)
        min_x, min_y, max_x, max_y = (value / 1_000_000 for value in region_bounds)
    edge = options.edge_clearance_nm / 1_000_000
    coordinate[0] = min(max_x - edge - half_width, max(min_x + edge + half_width, coordinate[0]))
    coordinate[1] = min(max_y - edge - half_height, max(min_y + edge + half_height, coordinate[1]))


def _choose_orientations(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    fixed: set[str],
) -> dict[str, Placement]:
    result = dict(placements)
    for reference in sorted(result):
        if reference in fixed:
            continue
        current = result[reference]
        best = current
        best_cost = _fast_score(board, result)
        for orientation in _allowed_orientations(board, reference):
            candidate = replace(current, rotation_degrees=orientation)
            result[reference] = candidate
            cost = _fast_score(board, result)
            if (cost, int(orientation)) < (best_cost, int(best.rotation_degrees)):
                best, best_cost = candidate, cost
        result[reference] = best
    return result


def _legalize(
    board: PhysicalBoard,
    targets: Mapping[str, Placement],
    original: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    seed: int,
) -> tuple[dict[str, Placement], int]:
    fixed = _fixed_placements(board, original, options)
    placed: dict[str, Placement] = {}
    for reference in sorted(fixed):
        candidate = fixed[reference]
        if not _legal(candidate, placed, board, options):
            raise PlacementAlgorithmError(
                f"fixed component {reference!r} violates board, keepout, region, or clearance constraints"
            )
        placed[reference] = candidate

    adjacency = _adjacency(board)
    priorities = _component_priorities(board)
    movable = sorted(
        (reference for reference in targets if reference not in fixed),
        key=lambda reference: (
            -priorities.get(reference, 0),
            -sum(adjacency.get(reference, {}).values()),
            -_footprint_area(board, targets[reference]),
            reference,
        ),
    )
    movable = _relative_cluster_order(board, movable, targets)
    placed_order: list[str] = []
    repair_count = 0
    for reference in movable:
        choice = _best_legal_choice(
            board,
            reference,
            targets[reference],
            placed,
            targets,
            adjacency,
            options,
            seed,
            limit=options.legalization_candidates,
        )
        if choice is None:
            repaired = _exact_repair(
                board,
                reference,
                placed,
                placed_order,
                targets,
                adjacency,
                options,
                seed,
            )
            if repaired is None:
                raise PlacementAlgorithmError(
                    f"cannot legalize component {reference!r}; enlarge the board or relax constraints"
                )
            placed = repaired
            repair_count += 1
        else:
            placed[reference] = choice
        placed_order.append(reference)
    return placed, repair_count


def _relative_cluster_order(
    board: PhysicalBoard,
    movable: list[str],
    targets: Mapping[str, Placement],
) -> list[str]:
    """Reserve space for close-placement groups before unrelated components.

    A decoupler placed after most of the board has been legalized may have no
    vacant position near its IC even on an otherwise roomy board. Keep each
    connected relative-rule group together, starting with its largest anchor.
    The normal priority order is retained for unconstrained components.
    """
    movable_set = set(movable)
    neighbors: dict[str, set[str]] = {reference: set() for reference in movable}
    for rule in board.relative_rules:
        members = [target.reference for target in rule.targets if target.reference in movable_set]
        for reference in members:
            neighbors[reference].update(other for other in members if other != reference)
    visited: set[str] = set()
    clusters: list[list[str]] = []
    for reference in movable:
        if reference in visited or not neighbors[reference]:
            continue
        stack = [reference]
        group: list[str] = []
        while stack:
            item = stack.pop()
            if item in visited:
                continue
            visited.add(item)
            group.append(item)
            stack.extend(sorted(neighbors[item] - visited, reverse=True))
        clusters.append(group)
    order = {reference: index for index, reference in enumerate(movable)}
    clusters.sort(
        key=lambda group: (
            -max(_footprint_area(board, targets[reference]) for reference in group),
            min(order[reference] for reference in group),
        )
    )
    result: list[str] = []
    for group in clusters:
        anchor = max(group, key=lambda reference: (_footprint_area(board, targets[reference]), -order[reference]))
        result.append(anchor)
        companions = [reference for reference in group if reference != anchor]
        distance_limits = {
            reference: min(
                (
                    rule.distance_nm
                    for rule in board.relative_rules
                    if rule.kind is RelativePlacementKind.MAX_DISTANCE
                    and any(target.reference == reference for target in rule.targets)
                    and any(target.reference == anchor for target in rule.targets)
                    and rule.distance_nm is not None
                ),
                default=2**63,
            )
            for reference in companions
        }
        result.extend(sorted(companions, key=lambda reference: (distance_limits[reference], -_footprint_area(board, targets[reference]), order[reference])))
    result.extend(reference for reference in movable if reference not in visited)
    return result


def _best_legal_choice(
    board: PhysicalBoard,
    reference: str,
    target: Placement,
    placed: Mapping[str, Placement],
    all_targets: Mapping[str, Placement],
    adjacency: Mapping[str, Mapping[str, int]],
    options: PlacementPlannerOptions,
    seed: int,
    limit: int | None = None,
) -> Placement | None:
    best: tuple[int, int, int, int, Placement] | None = None
    count = 0
    orientations = _allowed_orientations(board, reference)
    orientations = orientations[seed % len(orientations) :] + orientations[: seed % len(orientations)]
    preferred = _relative_candidate_positions(board, reference, target, placed, options)
    general = _candidate_positions(board, target, options)
    positions = tuple(dict.fromkeys((*preferred, *general)))
    for point in positions:
        for orientation in orientations:
            candidate = replace(target, position=point, rotation_degrees=orientation)
            if not _legal(candidate, placed, board, options):
                continue
            count += 1
            trial = dict(placed)
            trial[reference] = candidate
            cost = _incremental_cost(reference, candidate, placed, adjacency)
            cost += _relative_penalty(board, trial) * 100
            ranked = (cost, point.y_nm, point.x_nm, int(orientation), candidate)
            if best is None or ranked[:4] < best[:4]:
                best = ranked
            if limit is not None and count >= limit:
                return best[4] if best is not None else None
    return best[4] if best is not None else None


def _relative_candidate_positions(
    board: PhysicalBoard,
    reference: str,
    placement: Placement,
    placed: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> tuple[Point, ...]:
    points: list[Point] = []
    for rule in board.relative_rules:
        own = next((target for target in rule.targets if target.reference == reference), None)
        if own is None:
            continue
        own_offsets = (Point(0, 0),)
        if own.pad is not None:
            footprint = board.footprints[placement.footprint]
            pad = next(item for item in footprint.pads if item.number == own.pad)
            own_offsets = tuple(
                _transform_local(pad.position, orientation, placement.side)
                for orientation in _allowed_orientations(board, reference)
            )
        companions = [
            target
            for target in rule.targets
            if target.reference != reference and target.reference in placed
        ]
        for companion in companions:
            anchor = _target_point(board, placed, companion)
            if anchor is None:
                continue
            for own_offset in own_offsets:
                target_origin = Point(
                    anchor.x_nm - own_offset.x_nm,
                    anchor.y_nm - own_offset.y_nm,
                )
                if rule.kind is RelativePlacementKind.MAX_DISTANCE:
                    radius = max(1, (rule.distance_nm or 0) // options.grid_step_nm)
                    for distance in range(radius + 1):
                        for dx, dy in (
                            (distance, 0),
                            (-distance, 0),
                            (0, distance),
                            (0, -distance),
                            (distance, distance),
                            (distance, -distance),
                            (-distance, distance),
                            (-distance, -distance),
                        ):
                            points.append(
                                Point(
                                    target_origin.x_nm + dx * options.grid_step_nm,
                                    target_origin.y_nm + dy * options.grid_step_nm,
                                )
                            )
                elif rule.kind is RelativePlacementKind.MIN_DISTANCE:
                    radius = max(
                        1,
                        ((rule.distance_nm or 0) + options.grid_step_nm - 1)
                        // options.grid_step_nm,
                    )
                    for dx, dy in (
                        (radius, 0),
                        (-radius, 0),
                        (0, radius),
                        (0, -radius),
                        (radius, radius),
                        (radius, -radius),
                        (-radius, radius),
                        (-radius, -radius),
                    ):
                        points.append(
                            Point(
                                target_origin.x_nm + dx * options.grid_step_nm,
                                target_origin.y_nm + dy * options.grid_step_nm,
                            )
                        )
                elif rule.kind is RelativePlacementKind.ALIGN:
                    if rule.axis is AlignmentAxis.X:
                        points.append(
                            Point(target_origin.x_nm, placement.position.y_nm)
                        )
                    else:
                        points.append(
                            Point(placement.position.x_nm, target_origin.y_nm)
                        )
    return tuple(dict.fromkeys(points))


def _exact_repair(
    board: PhysicalBoard,
    failed: str,
    placed: Mapping[str, Placement],
    placed_order: list[str],
    targets: Mapping[str, Placement],
    adjacency: Mapping[str, Mapping[str, int]],
    options: PlacementPlannerOptions,
    seed: int,
) -> dict[str, Placement] | None:
    movable_tail = [
        reference
        for reference in reversed(placed_order)
        if reference not in options.fixed_references
    ][: options.exact_repair_limit - 1]
    repair = [failed, *reversed(movable_tail)]
    base = {reference: item for reference, item in placed.items() if reference not in repair}
    ordered = sorted(
        repair,
        key=lambda reference: (-_footprint_area(board, targets[reference]), reference),
    )

    def solve(index: int, current: dict[str, Placement]) -> dict[str, Placement] | None:
        if index == len(ordered):
            return current
        reference = ordered[index]
        candidates: list[Placement] = []
        orientations = _allowed_orientations(board, reference)
        for point in _candidate_positions(board, targets[reference], options):
            for orientation in orientations:
                candidate = replace(
                    targets[reference], position=point, rotation_degrees=orientation
                )
                if _legal(candidate, current, board, options):
                    candidates.append(candidate)
                    if len(candidates) >= options.exact_repair_candidates:
                        break
            if len(candidates) >= options.exact_repair_candidates:
                break
        candidates.sort(
            key=lambda item: (
                _incremental_cost(reference, item, current, adjacency),
                item.position.y_nm,
                item.position.x_nm,
                int(item.rotation_degrees),
            )
        )
        for candidate in candidates:
            next_current = dict(current)
            next_current[reference] = candidate
            solved = solve(index + 1, next_current)
            if solved is not None:
                return solved
        return None

    return solve(0, dict(base))


def _detailed_refine(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    seed: int,
) -> tuple[dict[str, Placement], int, int, int]:
    placements = dict(source)
    fixed = set(_fixed_placements(board, source, options))
    adjacency = _adjacency(board)
    move_count = 0
    swap_count = 0
    feedback_passes = 0
    for _ in range(options.refinement_passes):
        snapshot = dict(placements)
        before = placement_metrics(board, snapshot, options)
        changed = False
        for reference in sorted(placements):
            if reference in fixed:
                continue
            current = placements[reference]
            without = dict(placements)
            del without[reference]
            best = current
            best_score = _fast_score(board, placements)
            positions = (current.position, *_nearby_positions(current.position, options))
            for point in positions:
                for orientation in _allowed_orientations(board, reference):
                    candidate = replace(current, position=point, rotation_degrees=orientation)
                    if not _legal(candidate, without, board, options):
                        continue
                    trial = dict(without)
                    trial[reference] = candidate
                    score = _fast_score(board, trial)
                    ranked = (score, point.y_nm, point.x_nm, int(orientation))
                    best_ranked = (
                        best_score,
                        best.position.y_nm,
                        best.position.x_nm,
                        int(best.rotation_degrees),
                    )
                    if ranked < best_ranked:
                        best, best_score = candidate, score
            if best != current:
                placements[reference] = best
                changed = True
                move_count += 1

        pairs = sorted(
            {
                tuple(sorted((left, right)))
                for left, neighbors in adjacency.items()
                for right in neighbors
                if left not in fixed and right not in fixed
            }
        )
        for left, right in pairs[: max(8, len(placements))]:
            a, b = placements[left], placements[right]
            trial = dict(placements)
            del trial[left]
            del trial[right]
            swapped_a = replace(a, position=b.position)
            swapped_b = replace(b, position=a.position)
            if not _legal(swapped_a, trial, board, options):
                continue
            trial[left] = swapped_a
            if not _legal(swapped_b, trial, board, options):
                continue
            trial[right] = swapped_b
            if _fast_score(board, trial) < _fast_score(board, placements):
                placements = trial
                changed = True
                swap_count += 1

        after = placement_metrics(board, placements, options)
        feedback_passes += 1
        if (
            after.constraint_penalty_nm > before.constraint_penalty_nm
            or after.congestion_overflow > before.congestion_overflow
        ):
            placements = snapshot
            changed = False
        if not changed:
            break
    return placements, move_count, swap_count, feedback_passes


def _repair_relative_constraints(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> tuple[dict[str, Placement], int]:
    placements = dict(source)
    fixed = set(_fixed_placements(board, source, options))
    repair_count = 0
    for _ in range(max(1, len(board.relative_rules) * 2)):
        baseline = _relative_penalty(board, placements)
        if baseline == 0:
            break
        best_placements: dict[str, Placement] | None = None
        best_rank = (baseline, _fast_score(board, placements))
        for rule in board.relative_rules:
            references = sorted(
                {target.reference for target in rule.targets if target.reference not in fixed},
                key=lambda reference: (_footprint_area(board, placements[reference]), reference),
            )
            for reference in references:
                current = placements[reference]
                without = dict(placements)
                del without[reference]
                positions = _relative_candidate_positions(
                    board, reference, current, without, options
                )
                for point in positions:
                    for orientation in _allowed_orientations(board, reference):
                        candidate = replace(
                            current, position=point, rotation_degrees=orientation
                        )
                        if not _legal(candidate, without, board, options):
                            continue
                        trial = dict(without)
                        trial[reference] = candidate
                        rank = (_relative_penalty(board, trial), _fast_score(board, trial))
                        if rank < best_rank:
                            best_rank = rank
                            best_placements = trial
        if best_placements is None:
            break
        placements = best_placements
        repair_count += 1
    return placements, repair_count


def _nearby_positions(center: Point, options: PlacementPlannerOptions) -> tuple[Point, ...]:
    radius = options.refinement_radius_steps
    offsets = sorted(
        (
            (dx, dy)
            for dy in range(-radius, radius + 1)
            for dx in range(-radius, radius + 1)
            if dx or dy
        ),
        key=lambda item: (abs(item[0]) + abs(item[1]), item[1], item[0]),
    )
    return tuple(
        Point(
            center.x_nm + dx * options.grid_step_nm,
            center.y_nm + dy * options.grid_step_nm,
        )
        for dx, dy in offsets
    )


def _candidate_positions(
    board: PhysicalBoard,
    placement: Placement,
    options: PlacementPlannerOptions,
) -> tuple[Point, ...]:
    bounds = _outline_bounds(board.outline)
    rule = _rules(board).get(placement.reference)
    if rule is not None and rule.region is not None:
        region = next(item for item in board.regions if item.name == rule.region)
        bounds = _outline_bounds(region.outline)
    min_x, min_y, max_x, max_y = bounds
    max_half_width = 0
    max_half_height = 0
    for orientation in _allowed_orientations(board, placement.reference):
        candidate = replace(placement, rotation_degrees=orientation)
        half_width, half_height = _half_extents(board, candidate)
        max_half_width = max(max_half_width, half_width)
        max_half_height = max(max_half_height, half_height)
    first_x = _ceil_grid(
        min_x + options.edge_clearance_nm + max_half_width, options.grid_step_nm
    )
    first_y = _ceil_grid(
        min_y + options.edge_clearance_nm + max_half_height, options.grid_step_nm
    )
    last_x = max_x - options.edge_clearance_nm - max_half_width
    last_y = max_y - options.edge_clearance_nm - max_half_height
    if first_x > last_x or first_y > last_y:
        return ()
    points = (
        Point(x, y)
        for y in range(first_y, last_y + 1, options.grid_step_nm)
        for x in range(first_x, last_x + 1, options.grid_step_nm)
    )
    return tuple(
        sorted(
            points,
            key=lambda point: (
                abs(point.x_nm - placement.position.x_nm)
                + abs(point.y_nm - placement.position.y_nm),
                point.y_nm,
                point.x_nm,
            ),
        )
    )


def _legal(
    candidate: Placement,
    placed: Mapping[str, Placement],
    board: PhysicalBoard,
    options: PlacementPlannerOptions,
    *,
    cluster_keepouts: tuple[CopperKeepout, ...] | None = None,
) -> bool:
    rule = _rules(board).get(candidate.reference)
    if candidate.rotation_degrees not in _allowed_orientations(board, candidate.reference):
        return False
    if rule is not None and rule.side is not None and candidate.side is not rule.side:
        return False
    polygon = _placement_polygon(board, candidate)
    edge_clearance = (rule.edge_clearance_nm if rule and rule.edge_clearance_nm is not None
                      else options.edge_clearance_nm)
    if not _polygon_inside(polygon, board.outline.vertices, edge_clearance):
        return False
    if rule is not None and rule.region is not None:
        region = next(item for item in board.regions if item.name == rule.region)
        if region.side is not None and candidate.side is not region.side:
            return False
        if not _polygon_inside(polygon, region.outline.vertices, 0):
            return False
    footprint = board.footprints[candidate.footprint]
    from .hard_macros import resolved_macro_geometry
    macro_poses = {**placed, candidate.reference: candidate}
    for macro in board.hard_macros:
        cluster = next(c for c in board.rigid_clusters if c.name == macro.cluster)
        if cluster.anchor.reference not in macro_poses:
            continue
        members = {m.reference for m in cluster.members}
        outsiders = ([p for ref,p in placed.items() if ref not in members]
                     if candidate.reference in members else [candidate])
        for region in resolved_macro_geometry(board, macro, macro_poses)[3]:
            for outsider in outsiders:
                layer = CopperLayer.FRONT if outsider.side is BoardSide.FRONT else CopperLayer.BACK
                if layer in region.layers and _polygons_too_close(
                    _placement_polygon(board, outsider), region.outline.outer.vertices, 0
                ):
                    return False
    for keepout in board.keepouts:
        if keepout.side is not None and candidate.side is not keepout.side:
            continue
        if keepout.maximum_component_height_nm is not None:
            if footprint.height_nm is not None and footprint.height_nm <= keepout.maximum_component_height_nm:
                continue
        if _polygons_too_close(polygon, keepout.outline.vertices, 0):
            return False
    if cluster_keepouts is None:
        cluster_keepouts = resolved_cluster_keepouts(board, {**placed, candidate.reference: candidate})
    for keepout in (*board.copper_keepouts, *cluster_keepouts):
        candidate_layer = CopperLayer.FRONT if candidate.side is BoardSide.FRONT else CopperLayer.BACK
        if keepout.block_footprints and candidate_layer in keepout.layers and _polygons_too_close(
            polygon, keepout.outline.outer.vertices, 0
        ):
            return False
    for other in placed.values():
        if other.side is not candidate.side:
            continue
        other_footprint = board.footprints[other.footprint]
        other_polygon = _placement_polygon(board, other)
        for local in other_footprint.keepouts:
            if local.block_footprints and _polygons_too_close(
                polygon,
                tuple(transformed_local_point(other, point) for point in local.outline.outer.vertices),
                0,
            ):
                return False
        for local in footprint.keepouts:
            if local.block_footprints and _polygons_too_close(
                other_polygon,
                tuple(transformed_local_point(candidate, point) for point in local.outline.outer.vertices),
                0,
            ):
                return False
        clearance = options.component_clearance_nm
        for cluster in board.rigid_clusters:
            members = {item.reference for item in cluster.members}
            if (candidate.reference in members and other.reference in members
                    and cluster.internal_clearance_nm is not None):
                clearance = cluster.internal_clearance_nm
                break
        if _polygons_too_close(
            polygon,
            other_polygon,
            clearance,
        ):
            return False
    return True


def _placement_polygon(board: PhysicalBoard, placement: Placement) -> tuple[Point, ...]:
    footprint = board.footprints[placement.footprint]
    if footprint.courtyard:
        local = footprint.courtyard
    else:
        half_width = (footprint.body_size.width_nm + 1) // 2
        half_height = (footprint.body_size.height_nm + 1) // 2
        local = (
            Point(-half_width, -half_height),
            Point(half_width, -half_height),
            Point(half_width, half_height),
            Point(-half_width, half_height),
        )
    return tuple(
        Point(
            placement.position.x_nm + transformed.x_nm,
            placement.position.y_nm + transformed.y_nm,
        )
        for point in local
        for transformed in (_transform_local(point, placement.rotation_degrees, placement.side),)
    )


def _transform_local(point: Point, rotation: Decimal, side: BoardSide) -> Point:
    x = -point.x_nm if side is BoardSide.BACK else point.x_nm
    # KiCad's positive footprint angle rotates counter-clockwise in its
    # Cartesian convention; PCB/IR coordinates have Y increasing downwards.
    angle = radians(-float(rotation))
    return Point(
        round(x * cos(angle) - point.y_nm * sin(angle)),
        round(x * sin(angle) + point.y_nm * cos(angle)),
    )


def _polygons_too_close(
    first: tuple[Point, ...], second: tuple[Point, ...], clearance: int
) -> bool:
    first_box = _point_bounds(first)
    second_box = _point_bounds(second)
    if (
        first_box[2] + clearance <= second_box[0]
        or second_box[2] + clearance <= first_box[0]
        or first_box[3] + clearance <= second_box[1]
        or second_box[3] + clearance <= first_box[1]
    ):
        return False
    if _polygons_intersect(first, second):
        return True
    if clearance <= 0:
        return False
    minimum = min(
        _point_segment_distance(point, start, end)
        for polygon, other in ((first, second), (second, first))
        for point in polygon
        for start, end in _edges(other)
    )
    return minimum < clearance


def _polygon_inside(
    inner: tuple[Point, ...], outer: tuple[Point, ...], clearance: int
) -> bool:
    samples = (
        *inner,
        *(
            Point((start.x_nm + end.x_nm) // 2, (start.y_nm + end.y_nm) // 2)
            for start, end in _edges(inner)
        ),
    )
    if not all(_point_in_polygon(point, outer) for point in samples):
        return False
    if clearance:
        return all(
            min(_point_segment_distance(point, start, end) for start, end in _edges(outer))
            >= clearance
            for point in inner
        )
    return True


def _polygons_intersect(first: tuple[Point, ...], second: tuple[Point, ...]) -> bool:
    if any(_segments_intersect(a, b, c, d) for a, b in _edges(first) for c, d in _edges(second)):
        return True
    return _point_in_polygon(first[0], second) or _point_in_polygon(second[0], first)


def _edges(points: tuple[Point, ...]) -> tuple[tuple[Point, Point], ...]:
    return tuple((points[index], points[(index + 1) % len(points)]) for index in range(len(points)))


def _segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool:
    def orientation(p: Point, q: Point, r: Point) -> int:
        value = (q.y_nm - p.y_nm) * (r.x_nm - q.x_nm) - (q.x_nm - p.x_nm) * (r.y_nm - q.y_nm)
        return 0 if value == 0 else 1 if value > 0 else -1

    def on_segment(p: Point, q: Point, r: Point) -> bool:
        return (
            min(p.x_nm, r.x_nm) <= q.x_nm <= max(p.x_nm, r.x_nm)
            and min(p.y_nm, r.y_nm) <= q.y_nm <= max(p.y_nm, r.y_nm)
        )

    values = (orientation(a, b, c), orientation(a, b, d), orientation(c, d, a), orientation(c, d, b))
    if values[0] != values[1] and values[2] != values[3]:
        return True
    return (
        (values[0] == 0 and on_segment(a, c, b))
        or (values[1] == 0 and on_segment(a, d, b))
        or (values[2] == 0 and on_segment(c, a, d))
        or (values[3] == 0 and on_segment(c, b, d))
    )


def _point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for start, end in _edges(polygon):
        if _point_segment_distance(point, start, end) == 0:
            return True
        if (start.y_nm > point.y_nm) != (end.y_nm > point.y_nm):
            crossing = (end.x_nm - start.x_nm) * (point.y_nm - start.y_nm) / (end.y_nm - start.y_nm) + start.x_nm
            if point.x_nm < crossing:
                inside = not inside
    return inside


def _point_segment_distance(point: Point, start: Point, end: Point) -> float:
    dx = end.x_nm - start.x_nm
    dy = end.y_nm - start.y_nm
    if dx == 0 and dy == 0:
        return hypot(point.x_nm - start.x_nm, point.y_nm - start.y_nm)
    fraction = max(
        0.0,
        min(
            1.0,
            ((point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy)
            / (dx * dx + dy * dy),
        ),
    )
    return hypot(
        point.x_nm - (start.x_nm + fraction * dx),
        point.y_nm - (start.y_nm + fraction * dy),
    )


def _outline_bounds(outline: BoardOutline) -> tuple[int, int, int, int]:
    return _point_bounds(outline.vertices)


def _point_bounds(points: Iterable[Point]) -> tuple[int, int, int, int]:
    values = tuple(points)
    return (
        min(point.x_nm for point in values),
        min(point.y_nm for point in values),
        max(point.x_nm for point in values),
        max(point.y_nm for point in values),
    )


def _half_extents(board: PhysicalBoard, placement: Placement) -> tuple[int, int]:
    bounds = _point_bounds(_placement_polygon(board, replace(placement, position=Point(0, 0))))
    return max(abs(bounds[0]), abs(bounds[2])), max(abs(bounds[1]), abs(bounds[3]))


def _half_extents_mm(board: PhysicalBoard, placement: Placement) -> tuple[float, float]:
    width, height = _half_extents(board, placement)
    return width / 1_000_000, height / 1_000_000


def _footprint_area(board: PhysicalBoard, placement: Placement) -> int:
    footprint = board.footprints[placement.footprint]
    return footprint.body_size.width_nm * footprint.body_size.height_nm


def _ceil_grid(value: int, step: int) -> int:
    return ((value + step - 1) // step) * step


def _nearest_grid(value: int, step: int) -> int:
    return ((value + step // 2) // step) * step


def _adjacency(board: PhysicalBoard) -> dict[str, dict[str, int]]:
    result = {placement.reference: {} for placement in board.placements}
    for net in board.nets:
        references = sorted({pad.component for pad in net.pads if pad.component in result})
        if len(references) < 2:
            continue
        weight = max(1, 1000 // ((len(references) - 1) ** 2))
        for index, left in enumerate(references):
            for right in references[index + 1 :]:
                result[left][right] = result[left].get(right, 0) + weight
                result[right][left] = result[right].get(left, 0) + weight
    for group in board.placement_groups:
        for reference in group.references:
            for other in group.references:
                if reference != other and reference in result and other in result:
                    result[reference][other] = result[reference].get(other, 0) + max(1, group.priority)
    return result


def _component_priorities(board: PhysicalBoard) -> dict[str, int]:
    result = {rule.reference: rule.priority for rule in board.placement_rules}
    for group in board.placement_groups:
        for reference in group.references:
            result[reference] = max(result.get(reference, 0), group.priority)
    return result


def _incremental_cost(
    reference: str,
    candidate: Placement,
    placed: Mapping[str, Placement],
    adjacency: Mapping[str, Mapping[str, int]],
) -> int:
    return sum(
        (
            abs(candidate.position.x_nm - placed[other].position.x_nm)
            + abs(candidate.position.y_nm - placed[other].position.y_nm)
        )
        * weight
        for other, weight in adjacency.get(reference, {}).items()
        if other in placed
    )


def _pad_offset_mm(
    board: PhysicalBoard, placement: Placement, pad_number: str
) -> tuple[float, float]:
    footprint = board.footprints[placement.footprint]
    pad = next(item for item in footprint.pads if item.number == pad_number)
    transformed = _transform_local(pad.position, placement.rotation_degrees, placement.side)
    return transformed.x_nm / 1_000_000, transformed.y_nm / 1_000_000


def _net_points(
    board: PhysicalBoard, placements: Mapping[str, Placement], net_index: int
) -> dict[str, Point]:
    result: dict[str, Point] = {}
    net = board.nets[net_index]
    for index, pad in enumerate(net.pads):
        if pad.component in placements:
            result[f"{pad.component}.{pad.pad}:{index}"] = transformed_pad_position(
                board, placements[pad.component], pad.pad
            )
    return result


def _hpwl(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    total = 0
    for index in range(len(board.nets)):
        points = tuple(_net_points(board, placements, index).values())
        if len(points) >= 2:
            total += max(point.x_nm for point in points) - min(point.x_nm for point in points)
            total += max(point.y_nm for point in points) - min(point.y_nm for point in points)
    return total


def _target_point(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    target: PlacementTarget,
) -> Point | None:
    placement = placements.get(target.reference)
    if placement is None:
        return None
    return (
        transformed_pad_position(board, placement, target.pad)
        if target.pad is not None
        else placement.position
    )


def _relative_penalty(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    total = 0
    for rule in board.relative_rules:
        points = [_target_point(board, placements, target) for target in rule.targets]
        if any(point is None for point in points):
            continue
        concrete = [point for point in points if point is not None]
        if rule.kind is RelativePlacementKind.MAX_DISTANCE:
            for left, right in zip(concrete, concrete[1:]):
                distance = round(hypot(left.x_nm - right.x_nm, left.y_nm - right.y_nm))
                total += max(0, distance - (rule.distance_nm or 0)) * rule.weight
        elif rule.kind is RelativePlacementKind.MIN_DISTANCE:
            for left, right in zip(concrete, concrete[1:]):
                distance = round(hypot(left.x_nm - right.x_nm, left.y_nm - right.y_nm))
                total += max(0, (rule.distance_nm or 0) - distance) * rule.weight
        elif rule.kind is RelativePlacementKind.ALIGN:
            values = [
                point.x_nm if rule.axis is AlignmentAxis.X else point.y_nm
                for point in concrete
            ]
            total += max(0, max(values) - min(values) - rule.tolerance_nm) * rule.weight
    return total


def _relative_margin(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    margins: list[int] = []
    for rule in board.relative_rules:
        points = [_target_point(board, placements, target) for target in rule.targets]
        if any(point is None for point in points):
            continue
        concrete = [point for point in points if point is not None]
        if rule.kind in {
            RelativePlacementKind.MAX_DISTANCE,
            RelativePlacementKind.MIN_DISTANCE,
        }:
            for left, right in zip(concrete, concrete[1:]):
                distance = round(
                    hypot(left.x_nm - right.x_nm, left.y_nm - right.y_nm)
                )
                if rule.kind is RelativePlacementKind.MAX_DISTANCE:
                    margins.append((rule.distance_nm or 0) - distance)
                else:
                    margins.append(distance - (rule.distance_nm or 0))
        elif rule.kind is RelativePlacementKind.ALIGN:
            values = [
                point.x_nm if rule.axis is AlignmentAxis.X else point.y_nm
                for point in concrete
            ]
            margins.append(rule.tolerance_nm - (max(values) - min(values)))
    return min(margins, default=0)


def _pin_escape_pressure(
    board: PhysicalBoard, placements: Mapping[str, Placement]
) -> int:
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    layers = max(1, len(board.stackup.copper_layers))
    pressure = 0
    for placement in placements.values():
        footprint = board.footprints[placement.footprint]
        electrical_pads = [pad for pad in footprint.pads if pad.number]
        if not electrical_pads:
            continue
        min_x, min_y, max_x, max_y = _point_bounds(
            _placement_polygon(board, placement)
        )
        perimeter = 2 * ((max_x - min_x) + (max_y - min_y))
        escape_slots = max(1, perimeter // pitch) * layers
        pressure += max(0, len(electrical_pads) - escape_slots)
    return pressure


def _group_spread(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    total = 0
    for group in board.placement_groups:
        members = [placements[item] for item in group.references if item in placements]
        if len(members) < 2:
            continue
        anchor = (
            placements[group.anchor].position
            if group.anchor is not None and group.anchor in placements
            else Point(
                sum(item.position.x_nm for item in members) // len(members),
                sum(item.position.y_nm for item in members) // len(members),
            )
        )
        total += sum(
            abs(item.position.x_nm - anchor.x_nm) + abs(item.position.y_nm - anchor.y_nm)
            for item in members
        ) * max(1, group.priority)
    return total


def _high_pin_target_gap(board: PhysicalBoard, left: Placement, right: Placement) -> int:
    """Soft courtyard channel reserved between two dense, same-side packages."""

    if left.side is not right.side:
        return 0
    left_pads = len({pad.number for pad in board.footprints[left.footprint].pads if pad.number})
    right_pads = len({pad.number for pad in board.footprints[right.footprint].pads if pad.number})
    smaller = min(left_pads, right_pads)
    if smaller < 32:
        return 0
    return nm_from_mm("10" if smaller >= 64 else "8")


def _high_pin_spacing_penalty(
    board: PhysicalBoard, placements: Mapping[str, Placement]
) -> int:
    """Bounded soft cost for dense packages without an escape corridor."""

    dense = sorted(
        (item for item in placements.values()
         if len({pad.number for pad in board.footprints[item.footprint].pads if pad.number}) >= 32),
        key=lambda item: item.reference,
    )
    penalty = 0
    for index, left in enumerate(dense):
        left_width, left_height = _half_extents(board, left)
        for right in dense[index + 1 :]:
            gap = _high_pin_target_gap(board, left, right)
            if not gap:
                continue
            right_width, right_height = _half_extents(board, right)
            deficit_x = gap + left_width + right_width - abs(left.position.x_nm - right.position.x_nm)
            deficit_y = gap + left_height + right_height - abs(left.position.y_nm - right.position.y_nm)
            if deficit_x > 0 and deficit_y > 0:
                penalty += min(deficit_x, deficit_y) * 40
    return penalty


def _spread_high_pin_components(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> dict[str, Placement]:
    """Translate dense ICs with close companions into free board space.

    Moving a decoupled IC alone would violate its proximity rule. The small
    MAX_DISTANCE cluster is moved together, and every trial is fully legalized.
    This is a bounded soft optimization; fixed placements always win.
    """

    placements = dict(source)
    dense = sorted(
        reference for reference, item in placements.items()
        if len({pad.number for pad in board.footprints[item.footprint].pads if pad.number}) >= 32
    )
    if len(dense) < 2:
        return placements
    fixed = set(_fixed_placements(board, placements, options))
    companions: dict[str, set[str]] = {reference: {reference} for reference in placements}
    for rigid in board.rigid_clusters:
        members = {item.reference for item in rigid.members}
        for reference in members:
            companions[reference].update(members)
    for rule in board.relative_rules:
        if rule.kind is not RelativePlacementKind.MAX_DISTANCE:
            continue
        members = {target.reference for target in rule.targets if target.reference in placements}
        for reference in members:
            companions[reference].update(members)
    offsets = (
        (dx, dy)
        for radius in (4, 8, 12, 16)
        for dx, dy in (
            (-radius, 0), (radius, 0), (0, -radius), (0, radius),
            (-radius, -radius), (-radius, radius), (radius, -radius), (radius, radius),
        )
    )
    displacements = tuple(offsets)
    for _ in range(2):
        changed = False
        for reference in dense:
            cluster = set(companions[reference])
            # Include transitive companions so no proximity relation is broken.
            while any(not companions[item] <= cluster for item in cluster):
                cluster.update(*(companions[item] for item in tuple(cluster)))
            if cluster & fixed:
                continue
            baseline = _fast_score(board, placements)
            best = placements
            best_rank = (baseline, _hpwl(board, placements))
            for dx, dy in displacements:
                offset_x = nm_from_mm(dx)
                offset_y = nm_from_mm(dy)
                trial = dict(placements)
                for item in cluster:
                    current = trial[item]
                    trial[item] = replace(current, position=Point(
                        current.position.x_nm + offset_x,
                        current.position.y_nm + offset_y,
                    ))
                if not placement_solution_is_legal(board, trial, options):
                    continue
                rank = (_fast_score(board, trial), _hpwl(board, trial))
                if rank < best_rank:
                    best, best_rank = trial, rank
            if (
                best is not placements
                and _coarse_route(board, best, options).overflow
                <= _coarse_route(board, placements, options).overflow
            ):
                placements = best
                changed = True
        if not changed:
            break
    return placements


def _fast_score(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    return (
        _hpwl(board, placements)
        + _high_pin_spacing_penalty(board, placements)
        + _group_spread(board, placements) // 20
        + _relative_penalty(board, placements) * 100
    )


@dataclass(frozen=True, slots=True)
class _CoarseRoute:
    bin_count: int
    capacity: int
    overflow: int
    maximum_utilization_ppm: int
    connection_count: int
    crossing_count: int
    estimated_vias: int


@dataclass(frozen=True, slots=True)
class _RouteSegment:
    net: str
    layer: int
    start: Point
    end: Point


def _coarse_route(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> _CoarseRoute:
    min_x, min_y, max_x, max_y = _outline_bounds(board.outline)
    columns = max(1, (max_x - min_x + options.congestion_bin_nm - 1) // options.congestion_bin_nm)
    rows = max(1, (max_y - min_y + options.congestion_bin_nm - 1) // options.congestion_bin_nm)
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    capacity = max(1, options.congestion_bin_nm // pitch)
    demand: dict[tuple[int, int, int], int] = {}
    segments: list[_RouteSegment] = []
    connections = 0
    cross_side = 0
    layers = max(1, len(board.stackup.copper_layers))
    for net_index, net in enumerate(board.nets):
        points = _net_points(board, placements, net_index)
        for left, right in _manhattan_mst(points):
            connections += 1
            left_ref = left.split(".", 1)[0]
            right_ref = right.split(".", 1)[0]
            if placements[left_ref].side is not placements[right_ref].side:
                cross_side += 1
            start = _bin(points[left], min_x, min_y, columns, rows, options.congestion_bin_nm)
            end = _bin(points[right], min_x, min_y, columns, rows, options.congestion_bin_nm)
            choices: list[tuple[int, int, bool, tuple[tuple[int, int], ...]]] = []
            for layer in range(layers):
                for horizontal_first in (True, False):
                    path = _grid_path(start, end, horizontal_first)
                    cost = sum(demand.get((layer, *cell), 0) for cell in path)
                    overflow_cost = sum(
                        max(0, demand.get((layer, *cell), 0) + 1 - capacity)
                        for cell in path
                    )
                    choices.append((overflow_cost, cost, layer, horizontal_first, path))
            _, _, layer, horizontal_first, path = min(choices)
            for cell in path:
                key = (layer, *cell)
                demand[key] = demand.get(key, 0) + 1
            bend = (
                Point(points[right].x_nm, points[left].y_nm)
                if horizontal_first
                else Point(points[left].x_nm, points[right].y_nm)
            )
            if points[left] != bend:
                segments.append(_RouteSegment(net.name, layer, points[left], bend))
            if bend != points[right]:
                segments.append(_RouteSegment(net.name, layer, bend, points[right]))
    crossings = 0
    for index, left in enumerate(segments):
        for right in segments[index + 1 :]:
            if left.net == right.net or left.layer != right.layer:
                continue
            if _segments_intersect(left.start, left.end, right.start, right.end):
                crossings += 1
    overflow = sum(max(0, value - capacity) for value in demand.values())
    maximum = max((value * 1_000_000 // capacity for value in demand.values()), default=0)
    return _CoarseRoute(
        columns * rows * layers,
        capacity,
        overflow,
        maximum,
        connections,
        crossings,
        cross_side + crossings // layers,
    )


def _manhattan_mst(points: Mapping[str, Point]) -> tuple[tuple[str, str], ...]:
    if len(points) < 2:
        return ()
    remaining = set(points)
    connected = {min(remaining)}
    remaining -= connected
    edges: list[tuple[str, str]] = []
    while remaining:
        _, left, right = min(
            (
                (
                    abs(points[source].x_nm - points[target].x_nm)
                    + abs(points[source].y_nm - points[target].y_nm),
                    source,
                    target,
                )
                for source in connected
                for target in remaining
            )
        )
        edges.append((left, right))
        connected.add(right)
        remaining.remove(right)
    return tuple(edges)


def _bin(
    point: Point,
    min_x: int,
    min_y: int,
    columns: int,
    rows: int,
    size: int,
) -> tuple[int, int]:
    return (
        min(columns - 1, max(0, (point.x_nm - min_x) // size)),
        min(rows - 1, max(0, (point.y_nm - min_y) // size)),
    )


def _grid_path(
    start: tuple[int, int], end: tuple[int, int], horizontal_first: bool
) -> tuple[tuple[int, int], ...]:
    bend = (end[0], start[1]) if horizontal_first else (start[0], end[1])
    first = _axis_path(start, bend)
    second = _axis_path(bend, end)
    return (*first, *second[1:])


def _axis_path(
    start: tuple[int, int], end: tuple[int, int]
) -> tuple[tuple[int, int], ...]:
    if start[0] != end[0]:
        step = 1 if end[0] > start[0] else -1
        return tuple((x, start[1]) for x in range(start[0], end[0] + step, step))
    step = 1 if end[1] > start[1] else -1
    return tuple((start[0], y) for y in range(start[1], end[1] + step, step))

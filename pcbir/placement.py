"""Deterministic, constraint-aware PCB component placement algorithms.

The engine deliberately separates global estimation, legalization, detailed
refinement, and candidate selection. It consumes only physical IR and never
creates routed copper.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from decimal import Decimal
from itertools import chain, count, islice
from functools import lru_cache
from heapq import heappop, heappush
from math import ceil, cos, exp, hypot, isfinite, radians, sin
from typing import Iterable, Mapping

from .clusters import (
    cluster_placement_matches, resolved_cluster_keepouts,
)
from .cluster_placement import (
    place_rigid_clusters as _place_rigid_clusters,
    refine_rigid_clusters as _refine_rigid_clusters,
)
from .placement_escape import EscapeSpacingModel, placement_units
from .power_planning import power_domain_gradient, power_domain_penalty

from .physical import (
    AlignmentAxis,
    BoardOutline,
    BoardSide,
    ComponentPlacementRule,
    CopperKeepout,
    CopperLayer,
    Nanometres,
    PadKind,
    PadReference,
    PhysicalBoard,
    Placement,
    PlacementGroup,
    PlacementKeepout,
    PlacementTarget,
    Point,
    PolygonRing,
    PolygonWithHoles,
    RelativePlacementKind,
    RelativePlacementRule,
    TrackSegment,
    Via,
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
    escape_margin_nm: Nanometres = nm_from_mm("0.5")
    escape_transit_lanes: int = 1
    escape_spacing_passes: int = 2
    escape_max_movement_nm: Nanometres = nm_from_mm("16")
    power_domain_weight: float = 0.25

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
        if not isfinite(self.power_domain_weight) or not 0 <= self.power_domain_weight <= 1:
            raise ValueError("power-domain weight must be finite and in [0, 1]")
        if not 0 <= self.analytical_momentum < 1:
            raise ValueError("analytical momentum must be in [0, 1)")
        if self.refinement_passes < 0 or self.refinement_radius_steps < 1:
            raise ValueError("placement refinement settings are invalid")
        if self.candidate_count < 1:
            raise ValueError("placement candidate count must be positive")
        if (self.escape_margin_nm < 0 or self.escape_transit_lanes < 0
                or self.escape_spacing_passes < 0 or self.escape_max_movement_nm <= 0):
            raise ValueError("escape-spacing settings are invalid")
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
    escape_channel_penalty_nm: Nanometres
    escape_channel_deficit_nm: Nanometres
    escape_channel_pair_count: int
    constraint_penalty_nm: Nanometres
    minimum_constraint_margin_nm: Nanometres
    group_spread_nm: Nanometres
    congestion_bin_count: int
    congestion_capacity_per_bin: int
    congestion_overflow: int
    maximum_congestion_utilization_ppm: int
    power_domain_penalty_nm: Nanometres = 0
    weighted_wire_length_nm: Nanometres | None = None
    local_connection_length_nm: Nanometres = 0

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.constraint_penalty_nm,
            self.escape_channel_deficit_nm,
            self.congestion_overflow,
            self.crossing_count,
            self.estimated_via_count,
            self.pin_escape_pressure,
            self.local_connection_length_nm,
            -self.minimum_constraint_margin_nm,
            (self.weighted_wire_length_nm if self.weighted_wire_length_nm is not None
             else self.half_perimeter_wire_length_nm) + self.power_domain_penalty_nm,
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


@dataclass(frozen=True, slots=True)
class ReservedCorridor:
    """Placement keepout derived from a ``reserve_corridor`` differential pair.

    The keepout is checked exactly like a hand-drawn :class:`PlacementKeepout`,
    except that the pair's own terminal components are exempt from it.
    """

    nets: tuple[str, str]
    keepout: PlacementKeepout
    terminal_references: tuple[str, ...]
    margin_nm: Nanometres

    @property
    def name(self) -> str:
        return self.keepout.name

    @property
    def side(self) -> BoardSide | None:
        return self.keepout.side


@dataclass(frozen=True, slots=True)
class SkippedCorridor:
    """A requested corridor that was not derived, with the reason."""

    nets: tuple[str, ...]
    reason: str


@dataclass(frozen=True, slots=True)
class CorridorReservation:
    corridors: tuple[ReservedCorridor, ...] = ()
    skipped: tuple[SkippedCorridor, ...] = ()


def generate_placement_candidates(
    board: PhysicalBoard,
    options: PlacementPlannerOptions | None = None,
    *, progress=None,
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
    conflicts = reserved_corridor_conflicts(
        board, _fixed_placements(board, placements, options)
    )
    if conflicts:
        raise PlacementAlgorithmError("; ".join(conflicts))
    attempts: list[PlacementCandidate] = []
    closest_rejected = None
    spacing = _escape_model(board, options)
    seeds = max(2, options.candidate_count)
    for seed in range(seeds):
        if progress:
            progress("analytical", seed, seeds)
        continuous = _initial_seed(board, placements, options, seed)
        continuous = _analytical_place(board, continuous, options, spacing)
        if progress:
            progress("legalization", seed, seeds)
        continuous, seeded_original, phase_options = _place_rigid_clusters(
            board, continuous, placements, options
        )
        legalized, exact_repairs = _legalize(
            board, continuous, seeded_original, phase_options, seed
        )
        legalized, relative_before = _repair_relative_constraints(
            board, legalized, phase_options, spacing
        )
        if progress:
            progress("refinement", seed, seeds)
        refined, moves, swaps, feedback_passes = _detailed_refine(
            board, legalized, phase_options, seed, spacing
        )
        refined, relative_after = _repair_relative_constraints(
            board, refined, phase_options, spacing
        )
        refined, cluster_moves = _refine_rigid_clusters(board, refined, options, spacing_model=spacing)
        moves += cluster_moves
        refined, spacing_moves = _spread_escape_components(board, refined, options, spacing)
        moves += spacing_moves
        metrics = placement_metrics(board, refined, options, spacing_model=spacing)
        if metrics.constraint_penalty_nm or not placement_solution_is_legal(board, refined, options):
            if closest_rejected is None or metrics.constraint_penalty_nm < closest_rejected[0]:
                closest_rejected = (metrics.constraint_penalty_nm, refined)
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
        details = _local_failure_details(board, closest_rejected[1], options) if closest_rejected else ""
        raise PlacementAlgorithmError(
            "no legal placement candidate satisfies all represented physical constraints"
            + (f"; {details}" if details else "")
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
    if progress:
        progress("finished", seeds, seeds)
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
    *, spacing_model: EscapeSpacingModel | None = None,
) -> PlacementMetrics:
    route = _coarse_route(board, placements, options)
    channels = (spacing_model or _escape_model(board, options)).channels(placements)
    deficit = sum(c.deficit_nm for c in channels)
    return PlacementMetrics(
        component_count=len(placements),
        net_count=len(board.nets),
        estimated_connection_count=route.connection_count,
        half_perimeter_wire_length_nm=_hpwl(board, placements),
        crossing_count=route.crossing_count,
        estimated_via_count=route.estimated_vias,
        pin_escape_pressure=sum(c.pad_count for c in channels),
        escape_channel_penalty_nm=deficit * 40,
        escape_channel_deficit_nm=deficit,
        escape_channel_pair_count=len(channels),
        constraint_penalty_nm=_relative_penalty(board, placements),
        minimum_constraint_margin_nm=_relative_margin(board, placements),
        group_spread_nm=_group_spread(board, placements),
        congestion_bin_count=route.bin_count,
        congestion_capacity_per_bin=route.capacity,
        congestion_overflow=route.overflow,
        maximum_congestion_utilization_ppm=route.maximum_utilization_ppm,
        power_domain_penalty_nm=power_domain_penalty(board, placements, options.power_domain_weight),
        weighted_wire_length_nm=_planning_wirelength(board, placements),
        local_connection_length_nm=_local_connection_length(board, placements),
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
    """Return whether a complete placement satisfies hard and relative rules.

    Relative placement rules are still hard for the planner and manufacturing
    flow.  The interactive editor deliberately uses
    :func:`placement_solution_is_hard_legal` so a user can inspect a temporary
    pose that needs a relative-constraint adjustment.
    """

    return (
        placement_solution_is_hard_legal(board, placements, options)
        and _relative_penalty(board, placements) == 0
    )


def placement_solution_is_hard_legal(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions | None = None,
) -> bool:
    """Return whether a pose satisfies non-relative physical legality rules.

    This predicate intentionally excludes alignment and distance preferences.
    It is used only by the placement editor's temporary preview path; planners,
    source validation, routing and signoff continue to require
    :func:`placement_solution_is_legal`.
    """

    options = options or PlacementPlannerOptions()
    original = {item.reference: item for item in board.placements}
    if set(placements) != set(original):
        return False
    if any(item.reference != reference or item.footprint != original[reference].footprint
           for reference, item in placements.items()):
        return False
    if not cluster_placement_matches(board, placements):
        return False
    if any(placements[reference] != original[reference] for reference in options.fixed_references):
        return False
    for rule in board.placement_rules:
        pose = placements[rule.reference]
        if rule.fixed_position is not None and pose.position != rule.fixed_position:
            return False
        if rule.fixed_rotation_degrees is not None and pose.rotation_degrees != rule.fixed_rotation_degrees:
            return False
    accepted: dict[str, Placement] = {}
    keepouts = resolved_cluster_keepouts(board, placements)
    for reference in sorted(placements):
        candidate = placements[reference]
        if not _legal(candidate, accepted, board, options, cluster_keepouts=keepouts):
            return False
        accepted[reference] = candidate
    return True


def placement_rejection_reasons(board: PhysicalBoard, placements: Mapping[str, Placement],
                                options: PlacementPlannerOptions | None = None) -> tuple[str, ...]:
    """Explain a rejected pose using the same hard-rule predicates as the planner.

    This runs only on failure; the ordinary boolean hot path remains unchanged.
    Pair diagnostics deliberately use the real footprint/cluster clearances.
    """
    options = options or PlacementPlannerOptions()
    if placement_solution_is_legal(board, placements, options):
        return ()
    original = {p.reference: p for p in board.placements}
    if set(placements) != set(original):
        return ("component inventory differs from physical source",)
    from .geometry import RoundedConvexShape, shapes_clear
    from .mechanical import shape_in_outline, hole_shape
    errors = []
    for ref, pose in sorted(placements.items()):
        rule = _rules(board).get(ref)
        if ref in options.fixed_references and pose != original[ref]:
            errors.append(f"{ref}: temporary session lock fixes the complete pose")
        if rule:
            if rule.fixed_position is not None and pose.position != rule.fixed_position:
                errors.append(f"{ref}: source constraint fixes its position")
            if rule.fixed_rotation_degrees is not None and pose.rotation_degrees != rule.fixed_rotation_degrees:
                errors.append(f"{ref}: source constraint fixes its rotation")
            if rule.side is not None and pose.side is not rule.side:
                errors.append(f"{ref}: source constraint fixes side {rule.side.value}")
        if pose.rotation_degrees not in _allowed_orientations(board, ref):
            errors.append(f"{ref}: rotation {pose.rotation_degrees} is not allowed")
        shape = RoundedConvexShape(_placement_polygon(board, pose))
        clearance = rule.edge_clearance_nm if rule and rule.edge_clearance_nm is not None else options.edge_clearance_nm
        from .mechanical_assembly import body_in_material
        if not body_in_material(board, pose, shape, clearance):
            errors.append(f"{ref}: courtyard violates board material/cutout clearance ({clearance} nm)")
        for hole in board.mechanical_holes:
            if not shapes_clear(shape, hole_shape(hole), 1):
                errors.append(f"{ref}: courtyard intersects hole {hole.id}")
            if hole.head_clearance_radius_nm and not shapes_clear(shape,
                    RoundedConvexShape((hole.position,), hole.head_clearance_radius_nm), 1):
                errors.append(f"{ref}: courtyard intersects screw-head clearance {hole.id}")
        from .mechanical_assembly import component_height,assembly_pose_legal
        if not assembly_pose_legal(board,pose,shape.spine,{k:v for k,v in placements.items() if k!=ref}):
            height=component_height(board,pose)
            limits=[f"{e.id} ≤ {e.maximum_height_nm} nm" for e in board.assembly_envelopes
                    if e.side is pose.side and _polygons_too_close(shape.spine,e.outline.vertices,0)
                    and (height is None or height>e.maximum_height_nm)]
            if limits:errors.append(f"{ref}: component height {'unknown' if height is None else str(height)+' nm'} violates enclosure {', '.join(limits)}")
            else:errors.append(f"{ref}: assembly-access or original-board pad clearance conflict (body overhang never waives copper)")
        if not _legal(pose, {}, board, options):
            if not any(e.startswith(ref + ":") for e in errors):
                names = [k.name for k in board.keepouts if (k.side is None or k.side is pose.side)
                         and _polygons_too_close(shape.spine, k.outline.vertices, 0)]
                names += [c.name for c in reserved_corridors(board).corridors
                          if ref not in c.terminal_references
                          and _keepout_blocks(board, c.keepout, pose, shape.spine)]
                if rule and rule.region:
                    names.append(rule.region)
                names += [k.id for k in board.copper_keepouts
                          if k.block_footprints and _polygons_too_close(shape.spine, k.outline.outer.vertices, 0)]
                errors.append(f"{ref}: pose violates region/keepout {', '.join(names) or 'macro boundary'}")
        else:
            for other_ref, other in sorted(placements.items()):
                if other_ref >= ref or other.side is not pose.side:
                    continue
                if not _legal(pose, {other_ref: other}, board, options):
                    errors.append(f"{ref} / {other_ref}: courtyard or footprint keepout clearance conflict")
    if not cluster_placement_matches(board, placements):
        errors.append("rigid cluster/hard-macro member poses must move together")
    if _relative_penalty(board, placements):
        errors.append("relative placement constraint (alignment/distance) is not satisfied")
    return tuple(dict.fromkeys(errors)) or ("pose violates a represented physical constraint",)


_CORRIDOR_CACHE: dict[int, tuple[PhysicalBoard, CorridorReservation]] = {}
_CORRIDOR_CACHE_SIZE = 8
_EMPTY_RESERVATION = CorridorReservation()
# tan(pi/8): a regular octagon with this half-side per unit inradius
# circumscribes the unit disc, so expanded lands stay conservative.
_OCTAGON_HALF_SIDE = 0.41421356237309515


def reserved_corridors(board: PhysicalBoard) -> CorridorReservation:
    """Derive placement keepouts for ``reserve_corridor`` differential pairs.

    A corridor is derived only when every terminal component of both pair
    nets has a source-fixed position and rotation. It is the convex hull of
    the pair's terminal lands, expanded on every side by the pair's track
    width + pair gap + clearance (falling back to the board's default track
    width and minimum clearance). Pairs that cannot be derived are reported
    as skipped with a reason; nothing is guessed. The result is a pure,
    deterministic function of the board and is cached per board object.
    """

    cached = _CORRIDOR_CACHE.get(id(board))
    if cached is not None and cached[0] is board:
        return cached[1]
    if not any(rule.reserve_corridor for rule in board.net_routing_rules):
        return _EMPTY_RESERVATION
    result = _derive_reserved_corridors(board)
    if len(_CORRIDOR_CACHE) >= _CORRIDOR_CACHE_SIZE:
        _CORRIDOR_CACHE.clear()
    # The entry holds the board itself, so its id cannot be reused while cached.
    _CORRIDOR_CACHE[id(board)] = (board, result)
    return result


def reserved_corridor_conflicts(
    board: PhysicalBoard, poses: Mapping[str, Placement]
) -> tuple[str, ...]:
    """Explain fixed non-terminal components that obstruct a derived corridor."""

    messages: list[str] = []
    for corridor in reserved_corridors(board).corridors:
        for reference in sorted(poses):
            if reference in corridor.terminal_references:
                continue
            pose = poses[reference]
            if _keepout_blocks(board, corridor.keepout, pose, _placement_polygon(board, pose)):
                messages.append(
                    f"reserved corridor {corridor.name!r} for differential pair "
                    f"{corridor.nets[0]}/{corridor.nets[1]} overlaps fixed component "
                    f"{reference!r}; move {reference} out of the corridor or remove "
                    "reserve_corridor from the pair's routing profile"
                )
    return tuple(messages)


def _derive_reserved_corridors(board: PhysicalBoard) -> CorridorReservation:
    pairs: set[tuple[str, ...]] = set()
    for rule in board.net_routing_rules:
        if rule.reserve_corridor:
            pairs.add((rule.net,) if rule.differential_partner is None
                      else tuple(sorted((rule.net, rule.differential_partner))))
    corridors: list[ReservedCorridor] = []
    skipped: list[SkippedCorridor] = []
    for members in sorted(pairs):
        result = _derive_corridor(board, members)
        if isinstance(result, str):
            skipped.append(SkippedCorridor(members, result))
        else:
            corridors.append(result)
    return CorridorReservation(tuple(corridors), tuple(skipped))


def _derive_corridor(board: PhysicalBoard, members: tuple[str, ...]) -> ReservedCorridor | str:
    """Return one pair's corridor, or the reason it cannot be derived."""

    from .drc import placed_pad_shape

    if len(members) != 2:
        return "reserve_corridor requires a differential partner"
    nets = {net.name: net for net in board.nets}
    for net in members:
        if net not in nets:
            return f"pair net {net} has no physical pads"
        if len(nets[net].pads) < 2:
            return f"pair net {net} has fewer than two terminal lands"
    pads = sorted({pad for net in members for pad in nets[net].pads})
    references = tuple(sorted({pad.component for pad in pads}))
    source = {item.reference: item for item in board.placements}
    unplaced = [ref for ref in references if ref not in source]
    if unplaced:
        return f"terminal component(s) {', '.join(unplaced)} are not placed"
    placement_rules = _rules(board)
    movable = [
        ref for ref in references
        if ref not in placement_rules
        or placement_rules[ref].fixed_position is None
        or placement_rules[ref].fixed_rotation_degrees is None
    ]
    if movable:
        return (
            f"terminal component(s) {', '.join(movable)} are not fixed; reserve_corridor "
            "needs fixed_placement with position and rotation on every terminal component"
        )
    poses = {
        ref: replace(
            source[ref],
            position=placement_rules[ref].fixed_position,
            rotation_degrees=placement_rules[ref].fixed_rotation_degrees,
            side=placement_rules[ref].side or source[ref].side,
        )
        for ref in references
    }
    routing_rules = {rule.net: rule for rule in board.net_routing_rules}
    pair_rules = [routing_rules[net] for net in members if net in routing_rules]
    width = max((rule.width_nm for rule in pair_rules if rule.width_nm),
                default=board.rules.default_track_width_nm)
    gap = max((rule.pair_gap_nm for rule in pair_rules if rule.pair_gap_nm),
              default=board.rules.minimum_clearance_nm)
    clearance = max((board.rules.minimum_clearance_nm,
                     *(rule.clearance_nm for rule in pair_rules if rule.clearance_nm)))
    margin = width + gap + clearance
    points: list[tuple[int, int]] = []
    for pad_ref in pads:
        pose = poses[pad_ref.component]
        lands = [pad for pad in board.footprints[pose.footprint].pads if pad.number == pad_ref.pad]
        if not lands:
            return f"terminal pad {pad_ref.component}.{pad_ref.pad} is not in its footprint"
        for land in lands:
            shape = placed_pad_shape(transformed_local_point(pose, land.position), land, pose)
            radius = shape.radius_nm + margin
            half = ceil(radius * _OCTAGON_HALF_SIDE)
            for spine in shape.spine:
                points.extend(
                    (spine.x_nm + dx, spine.y_nm + dy)
                    for dx, dy in ((radius, half), (half, radius), (-half, radius),
                                   (-radius, half), (-radius, -half), (-half, -radius),
                                   (half, -radius), (radius, -half))
                )
    sides = {pose.side for pose in poses.values()}
    name = f"reserved-corridor:{members[0]}/{members[1]}"
    return ReservedCorridor(
        (members[0], members[1]),
        PlacementKeepout(name, BoardOutline(_convex_hull(points)),
                         next(iter(sides)) if len(sides) == 1 else None),
        references,
        margin,
    )


def _convex_hull(points: Iterable[tuple[int, int]]) -> tuple[Point, ...]:
    """Return the deterministic monotone-chain convex hull without collinear points."""

    ordered = sorted(set(points))

    def cross(o: tuple[int, int], a: tuple[int, int], b: tuple[int, int]) -> int:
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lower: list[tuple[int, int]] = []
    for point in ordered:
        while len(lower) >= 2 and cross(lower[-2], lower[-1], point) <= 0:
            lower.pop()
        lower.append(point)
    upper: list[tuple[int, int]] = []
    for point in reversed(ordered):
        while len(upper) >= 2 and cross(upper[-2], upper[-1], point) <= 0:
            upper.pop()
        upper.append(point)
    return tuple(Point(x, y) for x, y in (*lower[:-1], *upper[:-1]))


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
    if rule is not None and rule.fixed_rotation_degrees is not None:
        return (rule.fixed_rotation_degrees,)
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
    spacing: EscapeSpacingModel | None = None,
) -> dict[str, Placement]:
    if not source or options.analytical_iterations == 0:
        return dict(source)
    fixed = set(_fixed_placements(board, source, options))
    spacing = spacing or _escape_model(board, options)
    coordinates = {
        reference: [item.position.x_nm / 1_000_000, item.position.y_nm / 1_000_000]
        for reference, item in source.items()
    }
    velocity = {reference: [0.0, 0.0] for reference in source}
    gamma = max(1.0, options.congestion_bin_nm / 1_000_000)
    bounds = _outline_bounds(board.outline)
    plane_nets = {zone.net for zone in board.zones}

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
            net_weight = 0.25 if net.name in plane_nets else 1.0
            for pad, dx, dy in zip(pads, gx, gy, strict=True):
                gradient[pad.component][0] += dx * net_weight
                gradient[pad.component][1] += dy * net_weight

        references = sorted(source)
        for index, left in enumerate(references):
            left_width, left_height = _half_extents_mm(board, source[left])
            for right in references[index + 1 :]:
                if source[left].side is not source[right].side:
                    continue
                right_width, right_height = _half_extents_mm(board, source[right])
                dx = coordinates[left][0] - coordinates[right][0]
                dy = coordinates[left][1] - coordinates[right][1]
                clearance = options.component_clearance_nm / 1_000_000
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

        current_poses = {reference: replace(source[reference], position=Point(
            round(xy[0] * 1_000_000), round(xy[1] * 1_000_000)))
            for reference, xy in coordinates.items()}
        domain_gradient = power_domain_gradient(board, current_poses, options.power_domain_weight)
        for reference, force in domain_gradient.items():
            gradient[reference][0] += force[0]
            gradient[reference][1] += force[1]
        for channel in spacing.channels(current_poses):
            axis = 0 if channel.axis == "x" else 1
            delta = coordinates[channel.left][axis] - coordinates[channel.right][axis]
            force = min(4.0, channel.deficit_nm / 1_000_000) * (1 if delta >= 0 else -1)
            gradient[channel.left][axis] -= force
            gradient[channel.right][axis] += force

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
    return _choose_orientations(board, result, fixed, spacing, options.power_domain_weight)


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
    edge = options.edge_clearance_nm / 1_000_000
    if rule is not None and rule.region is not None:
        region = next(item for item in board.regions if item.name == rule.region)
        region_bounds = _outline_bounds(region.outline)
        min_x, min_y, max_x, max_y = (value / 1_000_000 for value in region_bounds)
        edge = 0  # Region boundaries contain courtyards; they are not board edges.
    coordinate[0] = min(max_x - edge - half_width, max(min_x + edge + half_width, coordinate[0]))
    coordinate[1] = min(max_y - edge - half_height, max(min_y + edge + half_height, coordinate[1]))


def _choose_orientations(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    fixed: set[str],
    spacing: EscapeSpacingModel | None = None,
    power_domain_weight: float = 0.25,
) -> dict[str, Placement]:
    spacing = spacing or EscapeSpacingModel(board)
    result = dict(placements)
    for reference in sorted(result):
        if reference in fixed:
            continue
        current = result[reference]
        best = current
        best_cost = _fast_score(board, result, spacing, power_domain_weight)
        for orientation in _allowed_orientations(board, reference):
            candidate = replace(current, rotation_degrees=orientation)
            result[reference] = candidate
            cost = _fast_score(board, result, spacing, power_domain_weight)
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
    rigid_members = {m.reference for cluster in board.rigid_clusters for m in cluster.members}
    candidate_rules = (*board.relative_rules, *_anchored_candidate_rules(board, fixed))
    direct = {target.reference for rule in candidate_rules
              if rule.kind is RelativePlacementKind.MAX_DISTANCE
              and any(t.reference in fixed for t in rule.targets)
              for target in rule.targets if target.reference not in fixed and target.reference not in rigid_members}
    units = placement_units(board)
    groups = {units[ref] & direct for ref in direct}
    order = {ref: i for i, ref in enumerate(movable)}
    for group in sorted(groups, key=lambda refs: min(order[ref] for ref in refs)):
        if len(group) < 2:
            continue
        references = sorted(group, key=order.get)
        packed = _pack_local_components(board, references, placed, targets, options, allow_general=False)
        if packed is not None:
            placed = packed
            placed_order.extend(references)
            repair_count += 1
    for reference in movable:
        if reference in placed:
            continue
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
        if choice is None or _relative_penalty(board, {**placed, reference: choice}) > _relative_penalty(board, placed):
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
                if choice is None:
                    raise PlacementAlgorithmError(
                        f"cannot legalize component {reference!r} within the local repair budget; "
                        + _local_failure_details(board, {**placed, reference: targets[reference]}, options)
                    )
                placed[reference] = choice
            else:
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
    """Schedule flexible companions together, including paths through fixed ICs."""
    units = placement_units(board)
    movable_set = set(movable)
    order = {reference: index for index, reference in enumerate(movable)}
    groups = {units[reference] for reference in movable if len(units[reference]) > 1}
    groups = sorted(groups, key=lambda group: (
        not bool(group - movable_set),
        -max(_footprint_area(board, targets[reference]) for reference in group),
        min(order[reference] for reference in group if reference in order),
    ))
    result: list[str] = []
    for group in groups:
        pending = set(group & movable_set)
        anchors = set(group - movable_set)
        if not anchors:
            anchor = min(pending, key=lambda ref: (-_footprint_area(board, targets[ref]), order[ref]))
            result.append(anchor)
            pending.remove(anchor)
            anchors.add(anchor)
        while pending:
            def difficulty(reference):
                limits = [rule.distance_nm for rule in board.relative_rules
                          if rule.kind is RelativePlacementKind.MAX_DISTANCE
                          and any(t.reference == reference for t in rule.targets)
                          and any(t.reference in anchors for t in rule.targets)]
                return (not bool(limits), min(limits, default=2**63), -len(limits),
                        -_footprint_area(board, targets[reference]), reference)
            companion = min(pending, key=difficulty)
            result.append(companion)
            pending.remove(companion)
            anchors.add(companion)
    scheduled = set(result)
    result.extend(reference for reference in movable if reference not in scheduled)
    return result


def _local_obstacles(board, reference, placed):
    """Test a local target's courtyard before distant fixed board obstacles."""
    anchors = {t.reference for rule in board.relative_rules
               if any(t.reference == reference for t in rule.targets) for t in rule.targets}
    return {ref: placed[ref] for ref in sorted(placed, key=lambda ref: ref not in anchors)}


def _bounded_local_candidates(candidates, limit, fallback=()):
    # Keep outer-row poses within the same legality-probe budget. Taking only
    # the closest prefix can discard every pose beyond a large anchor.
    if len(candidates) >= limit:
        # Reserve bounded ordinary-grid fallback so an invalid local domain
        # cannot starve a legal regional grid position.
        fallback = tuple(islice(fallback, limit//4))
        count = limit-len(fallback)
        candidates = tuple(candidates[round(i*(len(candidates)-1)/max(1, count-1))]
                           for i in range(count))
    return chain(candidates, fallback)


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
    spacing = _escape_model(board, options)
    relative_rules = tuple(r for r in board.relative_rules if any(t.reference == reference for t in r.targets))
    local_rules = tuple(r for r in relative_rules if all(t.pad is not None for t in r.targets))
    best = None
    obstacles = _local_obstacles(board, reference, placed)
    orientations = _allowed_orientations(board, reference)
    orientations = orientations[seed % len(orientations):] + orientations[:seed % len(orientations)]
    general = (replace(target, position=point, rotation_degrees=angle)
               for point in _candidate_positions(board, target, options) for angle in orientations)
    for step, fallback in ((nm_from_mm("0.25"), general), (nm_from_mm("0.1"), ())):
        count = 0
        local_target = best[1] if best is not None else target
        relative = _relative_candidates(board, reference, local_target, placed, options, step_nm=step)
        candidates = (_bounded_local_candidates(relative, options.legalization_candidates*64, fallback)
                      if local_rules else chain(relative, fallback))
        for probe, candidate in enumerate(candidates):
            if local_rules and probe >= options.legalization_candidates*64:
                break
            if not _legal(candidate, obstacles, board, options):
                continue
            trial = {**placed, reference: candidate}
            penalty = _relative_penalty(board, trial, rules=relative_rules)
            incremental = _incremental_cost(reference, candidate, placed, adjacency)
            rank = (penalty if local_rules else incremental + penalty*100,
                    sum(c.deficit_nm for c in spacing.channels(trial)) if local_rules else 0,
                    _local_connection_length(board, trial, rules=relative_rules),
                    incremental if local_rules else 0,
                    candidate.position.y_nm, candidate.position.x_nm, candidate.rotation_degrees)
            if best is None or rank < best[0]:
                best = (rank, candidate)
            count += 1
            if limit is not None and count >= limit:
                break
        if best is not None and _relative_penalty(board, {**placed, reference: best[1]}, rules=relative_rules) == 0:
            break
    return best[1] if best is not None else None


def _relative_candidates(
    board: PhysicalBoard,
    reference: str,
    placement: Placement,
    placed: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    *, step_nm: int = nm_from_mm("0.25"),
) -> tuple[Placement, ...]:
    """Bounded local XY search with orientation-specific pad/courtyard origins.

    The broad grid remains unchanged. Edge-derived samples reach the perimeter
    of large packages; a small two-dimensional neighbourhood fills gaps missed
    by rays, including legal positions between off-grid courtyard edges.
    """
    result = {}
    step_nm = min(options.grid_step_nm, step_nm)
    for rule in board.relative_rules:
        own_index = next((i for i, t in enumerate(rule.targets) if t.reference == reference), None)
        if own_index is None:
            continue
        own = rule.targets[own_index]
        companions = (rule.targets if rule.kind is RelativePlacementKind.ALIGN else
                      rule.targets[max(0, own_index-1):own_index] + rule.targets[own_index+1:own_index+2])
        for companion in companions:
            if companion.reference == reference or companion.reference not in placed:
                continue
            anchor = _target_point(board, placed, companion)
            for angle in _allowed_orientations(board, reference):
                pose = replace(placement, position=Point(0, 0), rotation_degrees=angle)
                offset = _target_point(board, {reference: pose}, own)
                origin = Point(anchor.x_nm-offset.x_nm, anchor.y_nm-offset.y_nm)
                points = set()
                if rule.kind is RelativePlacementKind.MAX_DISTANCE and (own.pad is None or companion.pad is None):
                    radius = max(1, (rule.distance_nm or 0)//options.grid_step_nm)
                    for distance in range(radius+1):
                        for dx, dy in ((distance, 0), (-distance, 0), (0, distance), (0, -distance),
                                       (distance, distance), (distance, -distance),
                                       (-distance, distance), (-distance, -distance)):
                            point = Point(origin.x_nm+dx*options.grid_step_nm, origin.y_nm+dy*options.grid_step_nm)
                            result.setdefault(replace(pose, position=point), len(result))
                    continue
                if rule.kind is RelativePlacementKind.MAX_DISTANCE:
                    distance = rule.distance_nm or 0
                    # Bound each neighbourhood independently of board size.
                    radius = min(24, distance // step_nm)
                    points.update(Point(origin.x_nm+x*step_nm, origin.y_nm+y*step_nm)
                                  for x in range(-radius, radius+1) for y in range(-radius, radius+1)
                                  if (x*step_nm)**2 + (y*step_nm)**2 <= distance**2)
                    if step_nm < nm_from_mm("0.25"):
                        points.update(Point(placement.position.x_nm+x*step_nm, placement.position.y_nm+y*step_nm)
                                      for x in range(-2, 3) for y in range(-2, 3))
                    own_bounds = _point_bounds(_placement_polygon(board, pose))
                    xs, ys = {origin.x_nm}, {origin.y_nm}
                    for other in placed.values():
                        if other.side is not placement.side:
                            continue
                        bounds = _point_bounds(_placement_polygon(board, other))
                        gap = options.component_clearance_nm
                        edges_x = (bounds[0]-gap-own_bounds[2], bounds[2]+gap-own_bounds[0])
                        edges_y = (bounds[1]-gap-own_bounds[3], bounds[3]+gap-own_bounds[1])
                        # Distant objects cannot bound this local neighbourhood.
                        # Their projected edges would crowd out nearby samples.
                        if (edges_x[1] < origin.x_nm-distance or edges_x[0] > origin.x_nm+distance
                                or edges_y[1] < origin.y_nm-distance or edges_y[0] > origin.y_nm+distance):
                            continue
                        xs.update(x for edge in edges_x for x in (edge-step_nm, edge, edge+step_nm)
                                  if abs(x-origin.x_nm) <= distance)
                        ys.update(y for edge in edges_y for y in (edge-step_nm, edge, edge+step_nm)
                                  if abs(y-origin.y_nm) <= distance)
                    # Only nearby edge coordinates participate in the Cartesian search.
                    xs = sorted(xs, key=lambda x: (abs(x-origin.x_nm), x))[:16]
                    ys = sorted(ys, key=lambda y: (abs(y-origin.y_nm), y))[:16]
                    points.update(Point(x, y) for x in xs for y in ys
                                  if (x-origin.x_nm)**2+(y-origin.y_nm)**2 <= distance**2)
                    # Retain coarse reach for constraints wider than the local window.
                    steps = max(1, min(64, distance // options.grid_step_nm))
                    for radius in (i*distance//steps for i in range(steps+1)):
                        points.update(Point(origin.x_nm+dx*radius, origin.y_nm+dy*radius)
                                      for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1),
                                                     (1, 1), (1, -1), (-1, 1), (-1, -1)))
                elif rule.kind is RelativePlacementKind.MIN_DISTANCE:
                    distance = max(1, ((rule.distance_nm or 0)+options.grid_step_nm-1)//options.grid_step_nm)*options.grid_step_nm
                    points.update(Point(origin.x_nm+dx*distance, origin.y_nm+dy*distance)
                                  for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1),
                                                 (1, 1), (1, -1), (-1, 1), (-1, -1)))
                elif rule.kind is RelativePlacementKind.ALIGN:
                    points.add(Point(origin.x_nm, placement.position.y_nm) if rule.axis is AlignmentAxis.X
                               else Point(placement.position.x_nm, origin.y_nm))
                for point in sorted(points, key=lambda p: ((p.x_nm-origin.x_nm)**2+(p.y_nm-origin.y_nm)**2,
                                                          p.y_nm, p.x_nm)):
                    candidate = replace(pose, position=point)
                    squared_distance = (point.x_nm-origin.x_nm)**2+(point.y_nm-origin.y_nm)**2
                    result[candidate] = min(result.get(candidate, squared_distance), squared_distance)
    if not any(r.kind is RelativePlacementKind.MAX_DISTANCE and all(t.pad is not None for t in r.targets)
               and any(t.reference == reference for t in r.targets) for r in board.relative_rules):
        point_order = {point: i for i, point in enumerate(dict.fromkeys(p.position for p in result))}
        return tuple(sorted(result, key=lambda p: (point_order[p.position], p.rotation_degrees)))
    local_rules = tuple(r for r in board.relative_rules if any(t.reference == reference for t in r.targets))
    related = {t.reference for r in local_rules for t in r.targets}
    anchors = {ref: pose for ref, pose in placed.items() if ref in related}
    def rank(pose):
        trial = {**anchors, reference: pose}
        return (_relative_penalty(board, trial, rules=local_rules),
                _local_connection_length(board, trial, rules=local_rules), result[pose],
                pose.position.y_nm, pose.position.x_nm, pose.rotation_degrees)
    return tuple(sorted(result, key=rank))


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
    fixed = _fixed_placements(board, placed, options)
    movable = set(placed_order) - fixed.keys()
    # Prefer actual blockers and local companions to an unrelated insertion tail.
    blockers: dict[str, int] = {}
    probes = _relative_candidates(board, failed, targets[failed], placed, options)
    checked = 0
    for candidate in probes:
        if not _legal(candidate, fixed, board, options):
            continue
        for ref in sorted(movable):
            if not _legal(candidate, {ref: placed[ref]}, board, options):
                blockers[ref] = blockers.get(ref, 0) + 1
        checked += 1
        if checked >= options.exact_repair_candidates:
            break
    companions = placement_units(board)[failed] & movable
    order = {ref: i for i, ref in enumerate(placed_order)}
    neighbours = sorted(movable, key=lambda ref: (ref not in blockers, ref not in companions,
                                                  -blockers.get(ref, 0), -order[ref], ref))
    repair = [failed, *neighbours[:options.exact_repair_limit-1]]
    base = {ref: item for ref, item in placed.items() if ref not in repair}
    return _pack_local_components(board, repair, base, targets, options, allow_general=True)


def _anchored_candidate_rules(board, anchors):
    """Safe candidate-only reach bounds through identical intermediate pads.

    If A.1 is within r of B.2, and B.2 is within s of a fixed C.3,
    A.1 is within r+s of C.3. No distance between distinct pads of B is
    inferred. Original rules still determine actual placement feasibility.
    """
    graph = {}
    for rule in board.relative_rules:
        if rule.kind is not RelativePlacementKind.MAX_DISTANCE or any(t.pad is None for t in rule.targets):
            continue
        for left, right in zip(rule.targets, rule.targets[1:]):
            for a, b in ((left, right), (right, left)):
                neighbours = graph.setdefault(a, {})
                neighbours[b] = min(neighbours.get(b, rule.distance_nm), rule.distance_nm)
    key = lambda target: (target.reference, target.pad)
    serial, pending, reached = count(), [], {}
    for anchor in sorted((target for target in graph if target.reference in anchors), key=key):
        heappush(pending, (0, next(serial), anchor, anchor, 0))
    while pending:
        distance, _, target, anchor, hops = heappop(pending)
        if target in reached:
            continue
        reached[target] = (distance, anchor, hops)
        for neighbour, length in sorted(graph[target].items(), key=lambda item: key(item[0])):
            if neighbour not in reached:
                heappush(pending, (distance+length, next(serial), neighbour, anchor, hops+1))
    return tuple(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
                 (target, anchor), distance_nm=distance)
                 for target, (distance, anchor, hops) in sorted(reached.items(), key=lambda item: key(item[0]))
                 if hops > 1 and target.reference not in anchors)


def _pack_local_components(board, references, base, targets, options, *, allow_general, fine=False):
    """One bounded domain search for anchored companions and local repair.

    Domains contain individually legal poses; courtyard forward checking removes
    obvious conflicts, while every selected pose still passes the complete gate.
    This changes flexible positions, never rigid membership or source locks.
    """
    rules = tuple(r for r in board.relative_rules if any(t.reference in references for t in r.targets))
    derived = _anchored_candidate_rules(board, base)
    candidate_board = replace(board, relative_rules=(*board.relative_rules, *derived)) if derived else board
    domains = {}
    limit = max(2, options.legalization_candidates*4)
    refinable = False
    for ref in references:
        target = targets[ref]
        obstacles = _local_obstacles(board, ref, base)
        own_rules = tuple(r for r in rules if any(t.reference == ref for t in r.targets))
        relative = _relative_candidates(candidate_board, ref, target, base, options)
        general = (replace(target, position=point, rotation_degrees=angle)
                   for point in _candidate_positions(board, target, options)
                   for angle in _allowed_orientations(board, ref)) if allow_general else ()
        candidates = _bounded_local_candidates(relative, options.legalization_candidates*64, general)
        legal = []
        seen = set()
        for index, candidate in enumerate(candidates):
            if index >= options.legalization_candidates*64:
                break
            if candidate in seen:
                continue
            seen.add(candidate)
            if (_relative_penalty(board, {**base, ref: candidate}, rules=own_rules) == 0
                    and _legal(candidate, obstacles, board, options)):
                legal.append(candidate)
        if not legal:
            return None
        narrow = (len(legal) <= options.legalization_candidates
                  and any(r.kind is RelativePlacementKind.MAX_DISTANCE
                          and all(t.pad is not None for t in r.targets) for r in own_rules))
        refinable |= narrow
        if fine and narrow:
            # Quarter-grid poses can individually fit a tight row while missing
            # the small shifts needed to fit together. Refine only narrow
            # domains; expanding broad bulk-part domains adds no useful space.
            neighbours = dict.fromkeys(legal)
            step = min(options.grid_step_nm, nm_from_mm("0.1"))
            for pose in legal:
                for dx in (-step, 0, step):
                    for dy in (-step, 0, step):
                        neighbours.setdefault(replace(pose, position=Point(
                            pose.position.x_nm+dx, pose.position.y_nm+dy)), None)
            candidates = tuple(neighbours)
            probe_limit = options.legalization_candidates*64
            if len(candidates) > probe_limit:
                candidates = tuple(candidates[round(i*(len(candidates)-1)/(probe_limit-1))]
                                   for i in range(probe_limit))
            legal = [pose for pose in candidates
                     if _relative_penalty(board, {**base, ref: pose}, rules=own_rules) == 0
                     and _legal(pose, obstacles, board, options)]
        # Preserve candidates across the complete local range, including a
        # second row; taking only the closest positions can erase the solution.
        if len(legal) > limit:
            legal = [legal[round(i*(len(legal)-1)/(limit-1))] for i in range(limit)]
        domains[ref] = tuple(legal)
    polygons = {ref: tuple(_placement_polygon(board, pose) for pose in poses)
                for ref, poses in domains.items()}
    remaining = options.legalization_candidates*options.exact_repair_candidates*64
    heuristic_remaining = remaining//3
    pair_rules = {}
    for rule in rules:
        pair = frozenset(t.reference for t in rule.targets)
        if len(pair) == 2 and pair <= domains.keys():
            pair_rules.setdefault(pair, []).append(rule)

    @lru_cache(maxsize=8192)
    def clash(left, i, right, j):
        return (domains[left][i].side is domains[right][j].side
                and _polygons_too_close(polygons[left][i], polygons[right][j], options.component_clearance_nm)
                or _relative_penalty(board, {left: domains[left][i], right: domains[right][j]},
                                     rules=pair_rules.get(frozenset((left, right)), ())) > 0)

    def solve(available, current):
        nonlocal remaining, heuristic_remaining
        if not available:
            return current if _relative_penalty(board, current, rules=rules) == 0 else None
        ref = min(available, key=lambda item: (len(available[item]), references.index(item), item))
        obstacles = _local_obstacles(board, ref, current)
        # Among equally feasible poses, leave space for the other companions.
        # A small, evenly spread sample keeps this look-ahead bounded even for
        # bulk parts whose allowed placement area is much larger.
        samples = {other: indices if len(indices) <= 16 else
                   tuple(indices[round(i*(len(indices)-1)/15)] for i in range(16))
                   for other, indices in available.items() if other != ref}
        ranked = []
        choices = available[ref]
        if len(choices) > options.exact_repair_candidates:
            choices = tuple(choices[round(i*(len(choices)-1)/max(1, options.exact_repair_candidates-1))]
                            for i in range(options.exact_repair_candidates))
        for index in choices:
            conflicts = 0.0
            for other, indices in samples.items():
                if heuristic_remaining < len(indices):
                    break
                heuristic_remaining -= len(indices)
                conflicts += sum(clash(ref, index, other, j) for j in indices)/len(indices)
            else:
                ranked.append((conflicts, index))
                continue
            break
        # Heuristic exhaustion cannot reject an otherwise feasible group.
        # Reserve the search budget and retain every unranked pose in its
        # deterministic original order.
        ranked_indices = {index for _, index in ranked}
        ordered = [index for _, index in sorted(ranked)]
        ordered.extend(index for index in available[ref] if index not in ranked_indices)
        for index in ordered:
            if remaining <= 0:
                return None
            remaining -= 1
            candidate = domains[ref][index]
            trial = {**current, ref: candidate}
            if _relative_penalty(board, trial, rules=rules) or not _legal(candidate, obstacles, board, options):
                continue
            rest = {}
            for other, indices in sorted(available.items(), key=lambda item: (len(item[1]), item[0])):
                if other == ref:
                    continue
                kept = []
                for j in indices:
                    if remaining <= 0:
                        return None
                    remaining -= 1
                    if not clash(ref, index, other, j):
                        kept.append(j)
                if not kept:
                    break
                rest[other] = tuple(kept)
            else:
                result = solve(rest, trial)
                if result is not None:
                    return result
        return None

    result = solve({ref: tuple(range(len(poses))) for ref, poses in domains.items()}, dict(base))
    if result is None and refinable and not fine:
        return _pack_local_components(board, references, base, targets, options,
                                      allow_general=allow_general, fine=True)
    return result


def _detailed_refine(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    seed: int,
    spacing: EscapeSpacingModel | None = None,
) -> tuple[dict[str, Placement], int, int, int]:
    spacing = spacing or _escape_model(board, options)
    placements = dict(source)
    fixed = set(_fixed_placements(board, source, options))
    adjacency = _adjacency(board)
    move_count = 0
    swap_count = 0
    feedback_passes = 0
    for _ in range(options.refinement_passes):
        snapshot = dict(placements)
        before = placement_metrics(board, snapshot, options, spacing_model=spacing)
        changed = False
        for reference in sorted(placements):
            if reference in fixed:
                continue
            current = placements[reference]
            without = dict(placements)
            del without[reference]
            obstacles = _local_obstacles(board, reference, without)
            best = current
            best_score = _fast_score(board, placements, spacing, options.power_domain_weight)
            positions = (current.position, *_nearby_positions(current.position, options))
            local_candidates = (_relative_candidates(board, reference, current, without, options)
                if any(r.kind is RelativePlacementKind.MAX_DISTANCE and all(t.pad is not None for t in r.targets)
                       and any(t.reference == reference for t in r.targets) for r in board.relative_rules) else ())
            candidates = dict.fromkeys((*local_candidates,
                *(replace(current, position=point, rotation_degrees=angle)
                  for point in positions for angle in _allowed_orientations(board, reference))))
            evaluated = 0
            for candidate in candidates:
                point, orientation = candidate.position, candidate.rotation_degrees
                if not _legal(candidate, obstacles, board, options):
                    continue
                trial = dict(without)
                trial[reference] = candidate
                if _relative_penalty(board, trial) > best_score[0]:
                    continue
                evaluated += 1
                score = _fast_score(board, trial, spacing, options.power_domain_weight)
                ranked = (score, point.y_nm, point.x_nm, int(orientation))
                best_ranked = (
                    best_score,
                    best.position.y_nm,
                    best.position.x_nm,
                    int(best.rotation_degrees),
                )
                if ranked < best_ranked:
                    best, best_score = candidate, score
                if local_candidates and evaluated >= options.legalization_candidates:
                    break
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
            if _fast_score(board, trial, spacing, options.power_domain_weight) < _fast_score(board, placements, spacing, options.power_domain_weight):
                placements = trial
                changed = True
                swap_count += 1

        after = placement_metrics(board, placements, options, spacing_model=spacing)
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
    spacing: EscapeSpacingModel | None = None,
) -> tuple[dict[str, Placement], int]:
    spacing = spacing or _escape_model(board, options)
    placements = dict(source)
    fixed = set(_fixed_placements(board, source, options))
    repair_count = 0
    remaining = options.legalization_candidates * options.exact_repair_limit * 8
    for _ in range(max(1, min(options.exact_repair_limit, len(board.relative_rules)*2))):
        if _relative_penalty(board, placements) == 0:
            break
        violating = {ref for violation in relative_placement_violations(board, placements)
                     for ref in violation["references"] if ref not in fixed}
        changed = False
        for reference in sorted(violating, key=lambda ref: (_footprint_area(board, placements[ref]), ref)):
            baseline = _relative_penalty(board, placements)
            if baseline == 0:
                break
            best_placements = None
            best_rank = (baseline, _fast_score(board, placements, spacing, options.power_domain_weight))
            current = placements[reference]
            without = {ref: pose for ref, pose in placements.items() if ref != reference}
            evaluated = 0
            obstacles = _local_obstacles(board, reference, without)
            local_rules = tuple(r for r in board.relative_rules if any(t.reference == reference for t in r.targets))
            related = {t.reference for r in local_rules for t in r.targets}
            anchors = {ref: pose for ref, pose in without.items() if ref in related}
            constant = baseline - _relative_penalty(board, placements, rules=local_rules)
            candidates = _relative_candidates(board, reference, current, without, options,
                                               step_nm=nm_from_mm("0.1"))
            for candidate in _bounded_local_candidates(candidates, max(1, remaining)):
                if remaining <= 0:
                    if best_placements is not None:
                        placements = best_placements
                        repair_count += 1
                    return placements, repair_count
                remaining -= 1
                penalty = constant + _relative_penalty(board, {**anchors, reference: candidate}, rules=local_rules)
                if penalty >= baseline or penalty > best_rank[0] or not _legal(candidate, obstacles, board, options):
                    continue
                trial = {**without, reference: candidate}
                rank = (penalty, _fast_score(board, trial, spacing, options.power_domain_weight))
                if rank < best_rank:
                    best_rank = rank
                    best_placements = trial
                evaluated += 1
                if evaluated >= options.exact_repair_candidates:
                    break
            if best_placements is not None:
                placements = best_placements
                repair_count += 1
                changed = True
        if not changed:
            break
    return placements, repair_count


def _local_failure_details(board, placements, options):
    """Explain bounded-search failure without claiming a constraint is impossible."""
    violations = relative_placement_violations(board, placements)
    messages = [item["message"] for item in violations[:3]]
    blockers = set()
    for reference in sorted({ref for item in violations[:3] for ref in item["references"]}):
        if reference in _fixed_placements(board, placements, options):
            continue
        without = {ref: pose for ref, pose in placements.items() if ref != reference}
        probes = _relative_candidates(board, reference, placements[reference], without, options,
                                      step_nm=nm_from_mm("0.1"))
        for candidate in probes[:options.exact_repair_candidates]:
            if not _legal(candidate, {}, board, options):
                continue
            for ref, other in without.items():
                if not _legal(candidate, {ref: other}, board, options):
                    blockers.add(ref)
    if blockers:
        messages.append("local candidate blockers: " + ", ".join(sorted(blockers)[:8]))
    return "; ".join(messages) or "no legal pose found in the bounded candidate search"


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
    edge = options.edge_clearance_nm
    if rule is not None and rule.region is not None:
        region = next(item for item in board.regions if item.name == rule.region)
        bounds = _outline_bounds(region.outline)
        edge = 0  # _legal still enforces the independent physical board edge.
    min_x, min_y, max_x, max_y = bounds
    max_half_width = 0
    max_half_height = 0
    for orientation in _allowed_orientations(board, placement.reference):
        candidate = replace(placement, rotation_degrees=orientation)
        half_width, half_height = _half_extents(board, candidate)
        max_half_width = max(max_half_width, half_width)
        max_half_height = max(max_half_height, half_height)
    first_x = _ceil_grid(
        min_x + edge + max_half_width, options.grid_step_nm
    )
    first_y = _ceil_grid(
        min_y + edge + max_half_height, options.grid_step_nm
    )
    last_x = max_x - edge - max_half_width
    last_y = max_y - edge - max_half_height
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


def _lands_meet_macro_copper(
    board: PhysicalBoard, pose: Placement,
    tracks: tuple[TrackSegment, ...], vias: tuple[Via, ...],
) -> bool:
    """Whether a non-member's lands violate a hard macro's copper.

    Foreign-net copper keeps the larger of the board minimum and either net's
    routing clearance; every foreign via drill also keeps the board hole
    clearance. Same-net copper may touch, as in physical DRC.
    """
    from .drc import placed_pad_shape
    from .geometry import RoundedConvexShape, shapes_clear

    lands = [pad for pad in board.footprints[pose.footprint].pads
             if pad.kind is not PadKind.APERTURE]
    if not lands or (not tracks and not vias):
        return False
    rules = {rule.net: rule for rule in board.net_routing_rules}
    reach = max(board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm,
                *(rule.clearance_nm or 0 for rule in rules.values()))
    shapes = [(pad, placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose))
              for pad in lands]
    copper = [*(RoundedConvexShape((track.start, track.end), track.width_nm // 2) for track in tracks),
              *(RoundedConvexShape((via.position,), via.size_nm // 2) for via in vias)]
    if not any(shape.bounds.intersects_expanded(item.bounds, reach)
               for _, shape in shapes for item in copper):
        return False
    owners = {pad: net.name for net in board.nets for pad in net.pads}
    stack = board.stackup.copper_layers
    side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK

    def required(first: str | None, second: str) -> int:
        return max((board.rules.minimum_clearance_nm,
                    *(rules[net].clearance_nm or 0 for net in (first, second) if net in rules)))

    for pad, shape in shapes:
        net = owners.get(PadReference(pose.reference, pad.number))
        layers = {side} if pad.kind is PadKind.SMD else set(stack)
        for track, item in zip(tracks, copper):
            if (track.layer in layers and track.net != net
                    and not shapes_clear(shape, item, required(net, track.net))):
                return True
        for via, item in zip(vias, copper[len(tracks):]):
            low, high = sorted((stack.index(via.from_layer), stack.index(via.to_layer)))
            if via.net == net or not layers & set(stack[low:high + 1]):
                continue
            if (not shapes_clear(shape, item, required(net, via.net))
                    or not shapes_clear(shape, RoundedConvexShape((via.position,), via.drill_nm // 2),
                                        board.rules.minimum_hole_clearance_nm)):
                return True
    return False


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
    # Reject local courtyard collisions before expensive board/cutout checks.
    for other in placed.values():
        if other.side is not candidate.side:
            continue
        other_polygon = _placement_polygon(board, other)
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
    edge_clearance = (rule.edge_clearance_nm if rule and rule.edge_clearance_nm is not None
                      else options.edge_clearance_nm)
    from .mechanical import shape_in_board
    from .geometry import RoundedConvexShape, shapes_clear
    from .mechanical_assembly import body_in_material,assembly_pose_legal
    inside = (body_in_material(board,candidate,RoundedConvexShape(polygon),edge_clearance)
              if board.body_overhangs else (shape_in_board(board, RoundedConvexShape(polygon), edge_clearance)
              if board.outline.circular_boundary or board.outline.boundary_path or board.outline.cutouts or board.mechanical_holes or board.mechanical_slots
              else _polygon_inside(polygon, board.outline.vertices, edge_clearance)))
    if not inside:
        return False
    if (board.body_overhangs or board.assembly_envelopes or board.assembly_access) and not assembly_pose_legal(board,candidate,polygon,placed):
        return False
    for hole in board.mechanical_holes:
        if hole.head_clearance_radius_nm and not shapes_clear(
                RoundedConvexShape(polygon),
                RoundedConvexShape((hole.position,), hole.head_clearance_radius_nm), 1):
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
        tracks, vias, _, regions = resolved_macro_geometry(board, macro, macro_poses)
        for region in regions:
            for outsider in outsiders:
                layer = CopperLayer.FRONT if outsider.side is BoardSide.FRONT else CopperLayer.BACK
                if layer in region.layers and _polygons_too_close(
                    _placement_polygon(board, outsider), region.outline.outer.vertices, 0
                ):
                    return False
        # Port vias and lead-in tracks may lie outside the protected regions.
        if any(_lands_meet_macro_copper(board, outsider, tracks, vias) for outsider in outsiders):
            return False
    for keepout in board.keepouts:
        if _keepout_blocks(board, keepout, candidate, polygon):
            return False
    for corridor in reserved_corridors(board).corridors:
        if (candidate.reference not in corridor.terminal_references
                and _keepout_blocks(board, corridor.keepout, candidate, polygon)):
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
    return True


def _keepout_blocks(
    board: PhysicalBoard,
    keepout: PlacementKeepout,
    candidate: Placement,
    polygon: tuple[Point, ...],
) -> bool:
    """Return whether a placement keepout excludes this placed body."""

    if keepout.side is not None and candidate.side is not keepout.side:
        return False
    if keepout.maximum_component_height_nm is not None:
        from .mechanical_assembly import component_height
        height = component_height(board, candidate)
        if height is not None and height <= keepout.maximum_component_height_nm:
            return False
    return _polygons_too_close(polygon, keepout.outline.vertices, 0)


def _placement_polygon(board: PhysicalBoard, placement: Placement) -> tuple[Point, ...]:
    footprint = board.footprints[placement.footprint]
    local = _rotated_courtyard(footprint.courtyard, footprint.body_size,
                               placement.rotation_degrees, placement.side)
    return _translated_polygon(local, placement.position)


@lru_cache(maxsize=4096)
def _rotated_courtyard(courtyard, body_size, rotation, side) -> tuple[Point, ...]:
    if not courtyard:
        half_width = (body_size.width_nm+1)//2
        half_height = (body_size.height_nm+1)//2
        courtyard = (Point(-half_width, -half_height), Point(half_width, -half_height),
                     Point(half_width, half_height), Point(-half_width, half_height))
    return tuple(_transform_local(point, rotation, side) for point in courtyard)


@lru_cache(maxsize=8192)
def _translated_polygon(polygon: tuple[Point, ...], position: Point) -> tuple[Point, ...]:
    return tuple(Point(point.x_nm+position.x_nm, point.y_nm+position.y_nm) for point in polygon)


@lru_cache(maxsize=4096)
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
    if _is_axis_rectangle(first) and _is_axis_rectangle(second):
        dx = max(first_box[0]-second_box[2], second_box[0]-first_box[2], 0)
        dy = max(first_box[1]-second_box[3], second_box[1]-first_box[3], 0)
        return (dx == 0 and dy == 0) or dx*dx+dy*dy < clearance*clearance
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


@lru_cache(maxsize=8192)
def _is_axis_rectangle(polygon: tuple[Point, ...]) -> bool:
    if len(polygon) != 4:
        return False
    left, top, right, bottom = _point_bounds(polygon)
    return set(polygon) == {Point(left, top), Point(right, top),
                            Point(right, bottom), Point(left, bottom)}


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
    return _cached_point_bounds(tuple(points))


@lru_cache(maxsize=8192)
def _cached_point_bounds(values: tuple[Point, ...]) -> tuple[int, int, int, int]:
    return (
        min(point.x_nm for point in values),
        min(point.y_nm for point in values),
        max(point.x_nm for point in values),
        max(point.y_nm for point in values),
    )


def _half_extents(board: PhysicalBoard, placement: Placement) -> tuple[int, int]:
    footprint = board.footprints[placement.footprint]
    bounds = _point_bounds(_rotated_courtyard(footprint.courtyard, footprint.body_size,
                                             placement.rotation_degrees, placement.side))
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
    plane_nets = {zone.net for zone in board.zones}
    for net in board.nets:
        references = sorted({pad.component for pad in net.pads if pad.component in result})
        if len(references) < 2:
            continue
        weight = max(1, 1000 // ((len(references) - 1) ** 2))
        if net.name in plane_nets:
            weight = max(1, weight // 4)
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


def _planning_wirelength(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    """Plane rails need terminal access, not a shortest full distribution tree."""
    planes = {zone.net for zone in board.zones}
    total = 0
    for index, net in enumerate(board.nets):
        points = tuple(_net_points(board, placements, index).values())
        if len(points) >= 2:
            span = (max(p.x_nm for p in points) - min(p.x_nm for p in points)
                    + max(p.y_nm for p in points) - min(p.y_nm for p in points))
            total += span // 4 if net.name in planes else span
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


def _relative_penalty(board: PhysicalBoard, placements: Mapping[str, Placement], *, rules=None) -> int:
    total = 0
    for rule in board.relative_rules if rules is None else rules:
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


def _local_connection_length(board: PhysicalBoard, placements: Mapping[str, Placement], *, rules=None) -> int:
    """Bounded preference inside explicit pin-to-pin maximum-distance limits."""
    return sum(min(rule.distance_nm or 0, round(hypot(a.x_nm-b.x_nm, a.y_nm-b.y_nm))) * rule.weight
               for rule in (board.relative_rules if rules is None else rules)
               if rule.kind is RelativePlacementKind.MAX_DISTANCE
               for left, right in zip(rule.targets, rule.targets[1:])
               if left.pad is not None and right.pad is not None
               if (a := _target_point(board, placements, left)) is not None
               if (b := _target_point(board, placements, right)) is not None)


def relative_placement_violations(
    board: PhysicalBoard, placements: Mapping[str, Placement]
) -> tuple[dict[str, object], ...]:
    """Return deterministic, UI-friendly diagnostics for violated soft rules.

    The planner keeps relative rules hard when selecting a fabrication
    candidate, but the editor needs to show *which* relation is broken while
    allowing a temporary drag.  Each record contains the participating
    references, actual measurement and requested limit/tolerance in nanometres.
    Missing targets are skipped here because the physical-board validator
    reports inventory errors separately.
    """

    violations: list[dict[str, object]] = []
    for rule_index, rule in enumerate(board.relative_rules):
        points = [_target_point(board, placements, target) for target in rule.targets]
        if any(point is None for point in points):
            continue
        concrete = [point for point in points if point is not None]
        targets = [
            {"reference": target.reference, "pad": target.pad}
            for target in rule.targets
        ]
        if rule.kind in {
            RelativePlacementKind.MAX_DISTANCE,
            RelativePlacementKind.MIN_DISTANCE,
        }:
            for pair_index, (left, right) in enumerate(zip(concrete, concrete[1:])):
                actual = round(hypot(left.x_nm - right.x_nm, left.y_nm - right.y_nm))
                limit = rule.distance_nm or 0
                amount = (
                    max(0, actual - limit)
                    if rule.kind is RelativePlacementKind.MAX_DISTANCE
                    else max(0, limit - actual)
                )
                if not amount:
                    continue
                left_target, right_target = rule.targets[pair_index : pair_index + 2]
                kind_text = "maximum" if rule.kind is RelativePlacementKind.MAX_DISTANCE else "minimum"
                violations.append({
                    "rule_index": rule_index,
                    "kind": rule.kind.value,
                    "targets": targets,
                    "references": [left_target.reference, right_target.reference],
                    "pair_index": pair_index,
                    "actual_distance_nm": actual,
                    "limit_nm": limit,
                    "excess_nm": amount,
                    "message": (
                        f"{left_target.reference} to {right_target.reference}: "
                        f"{kind_text} distance {limit} nm, actual {actual} nm"
                    ),
                })
        elif rule.kind is RelativePlacementKind.ALIGN:
            values = [
                point.x_nm if rule.axis is AlignmentAxis.X else point.y_nm
                for point in concrete
            ]
            actual = max(values) - min(values)
            tolerance = rule.tolerance_nm
            amount = max(0, actual - tolerance)
            if amount:
                violations.append({
                    "rule_index": rule_index,
                    "kind": rule.kind.value,
                    "targets": targets,
                    "references": [target.reference for target in rule.targets],
                    "axis": rule.axis.value if rule.axis is not None else None,
                    "actual_span_nm": actual,
                    "tolerance_nm": tolerance,
                    "excess_nm": amount,
                    "message": (
                        f"{', '.join(target.reference for target in rule.targets)}: "
                        f"{rule.axis.value if rule.axis is not None else 'axis'} alignment "
                        f"tolerance {tolerance} nm, actual span {actual} nm"
                    ),
                })
    return tuple(violations)


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


def _escape_model(board: PhysicalBoard, options: PlacementPlannerOptions) -> EscapeSpacingModel:
    return EscapeSpacingModel(board, margin_nm=options.escape_margin_nm,
                              transit_lanes=options.escape_transit_lanes)


def _spread_escape_components(
    board: PhysicalBoard,
    source: Mapping[str, Placement],
    options: PlacementPlannerOptions,
    spacing: EscapeSpacingModel | None = None,
) -> tuple[dict[str, Placement], int]:
    """Relieve facing channels by translating complete constrained units."""

    placements = dict(source)
    spacing = spacing or _escape_model(board, options)
    fixed = set(_fixed_placements(board, placements, options))
    moves = 0
    for _ in range(options.escape_spacing_passes):
        changed = False
        channels = spacing.channels(placements)
        references = {ref for c in channels for ref in (c.left, c.right)}
        ordered = sorted(references, key=lambda ref: (
            sum(_footprint_area(board, placements[item]) for item in spacing.units[ref]), ref))
        visited = set()
        for reference in ordered:
            cluster = spacing.units[reference]
            if cluster in visited:
                continue
            visited.add(cluster)
            if cluster & fixed:
                continue
            relevant = [c for c in spacing.channels(placements)
                        if (c.left in cluster) != (c.right in cluster)]
            if not relevant:
                continue  # Internal proximity deficits cannot be relieved by translation.
            offsets = set()
            for channel in relevant:
                ref, other = ((channel.left, channel.right) if channel.left in cluster
                              else (channel.right, channel.left))
                axis = "x_nm" if channel.axis == "x" else "y_nm"
                sign = -1 if getattr(placements[ref].position, axis) < getattr(placements[other].position, axis) else 1
                amount = ((channel.deficit_nm + options.grid_step_nm - 1)
                          // options.grid_step_nm) * options.grid_step_nm
                amount = min(options.escape_max_movement_nm, amount) // options.grid_step_nm * options.grid_step_nm
                if amount:
                    offsets.add((sign * amount, 0) if channel.axis == "x" else (0, sign * amount))
            for radius in {options.grid_step_nm, nm_from_mm(2), nm_from_mm(4),
                           nm_from_mm(8), options.escape_max_movement_nm}:
                if radius > options.escape_max_movement_nm:
                    continue
                radius = radius // options.grid_step_nm * options.grid_step_nm
                if not radius:
                    continue
                for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1),
                               (-1, -1), (-1, 1), (1, -1), (1, 1)):
                    if (dx * radius)**2 + (dy * radius)**2 <= options.escape_max_movement_nm**2:
                        offsets.add((dx * radius, dy * radius))
            baseline = _fast_score(board, placements, spacing, options.power_domain_weight)
            best = placements
            best_rank = (baseline, _hpwl(board, placements))
            for offset_x, offset_y in sorted(offsets, key=lambda xy: (abs(xy[0])+abs(xy[1]), xy[1], xy[0])):
                trial = dict(placements)
                for item in cluster:
                    current = trial[item]
                    trial[item] = replace(current, position=Point(
                        current.position.x_nm + offset_x,
                        current.position.y_nm + offset_y,
                    ))
                if not placement_solution_is_legal(board, trial, options):
                    continue
                rank = (_fast_score(board, trial, spacing, options.power_domain_weight), _hpwl(board, trial))
                if rank < best_rank:
                    best, best_rank = trial, rank
            if (
                best is not placements
                and _coarse_route(board, best, options).overflow
                <= _coarse_route(board, placements, options).overflow
            ):
                placements = best
                changed = True
                moves += 1
        if not changed:
            break
    return placements, moves


def _fast_score(board: PhysicalBoard, placements: Mapping[str, Placement],
                spacing: EscapeSpacingModel | None = None,
                power_domain_weight: float = 0.25) -> tuple[int, int, int, int]:
    channels = (spacing or EscapeSpacingModel(board)).channels(placements)
    return (
        _relative_penalty(board, placements),
        sum(c.deficit_nm for c in channels),
        _local_connection_length(board, placements),
        _planning_wirelength(board, placements) + _group_spread(board, placements) // 20
        + power_domain_penalty(board, placements, power_domain_weight),
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

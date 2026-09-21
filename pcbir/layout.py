"""Deterministic placement planning and physical-design readiness gates.

The public workflow is deliberately small: prepare, place, route, verify.
Global placement, legalization, local refinement, and routability estimation
are internal substeps of ``place`` rather than separate user-visible phases.
This module implements the placement stage; it does not claim to autoroute a
board or make a design fabrication-ready.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from enum import Enum
import json
from types import MappingProxyType
from typing import Mapping

from .physical import Nanometres, PhysicalBoard, Placement, Point, nm_from_mm


class LayoutStage(str, Enum):
    PREPARE = "prepare"
    PLACE = "place"
    ROUTE = "route"
    VERIFY = "verify"


class GateStatus(str, Enum):
    PASS = "pass"
    WARNING = "warning"
    BLOCKED = "blocked"
    NOT_RUN = "not_run"


@dataclass(frozen=True, slots=True)
class LayoutFinding:
    code: str
    stage: LayoutStage
    status: GateStatus
    message: str
    subject: str | None = None


@dataclass(frozen=True, slots=True)
class LayoutGate:
    stage: LayoutStage
    status: GateStatus
    summary: str


@dataclass(frozen=True, slots=True)
class PlacementMetrics:
    component_count: int
    net_count: int
    estimated_connection_count: int
    half_perimeter_wire_length_nm: Nanometres
    congestion_bin_count: int
    congestion_capacity_per_bin: int
    congestion_overflow: int
    maximum_congestion_utilization_ppm: int


@dataclass(frozen=True, slots=True)
class LayoutReport:
    board_name: str
    gates: tuple[LayoutGate, ...]
    findings: tuple[LayoutFinding, ...]
    metrics: PlacementMetrics
    algorithm: str = "connectivity-greedy/legalize/congestion-refine-v0.1"

    def to_json(self) -> str:
        return json.dumps(
            {
                "schema": "copperscript-layout-report/v0.1",
                "board": self.board_name,
                "algorithm": self.algorithm,
                "gates": [
                    {
                        "stage": gate.stage.value,
                        "status": gate.status.value,
                        "summary": gate.summary,
                    }
                    for gate in self.gates
                ],
                "findings": [
                    {
                        "code": finding.code,
                        "stage": finding.stage.value,
                        "status": finding.status.value,
                        "message": finding.message,
                        **(
                            {"subject": finding.subject}
                            if finding.subject is not None
                            else {}
                        ),
                    }
                    for finding in self.findings
                ],
                "metrics": {
                    "component_count": self.metrics.component_count,
                    "net_count": self.metrics.net_count,
                    "estimated_connection_count": self.metrics.estimated_connection_count,
                    "half_perimeter_wire_length_nm": self.metrics.half_perimeter_wire_length_nm,
                    "congestion_bin_count": self.metrics.congestion_bin_count,
                    "congestion_capacity_per_bin": self.metrics.congestion_capacity_per_bin,
                    "congestion_overflow": self.metrics.congestion_overflow,
                    "maximum_congestion_utilization_ppm": self.metrics.maximum_congestion_utilization_ppm,
                },
            },
            indent=2,
            sort_keys=True,
        ) + "\n"


@dataclass(frozen=True, slots=True)
class PlacementPlannerOptions:
    grid_step_nm: Nanometres = nm_from_mm("1")
    edge_clearance_nm: Nanometres = nm_from_mm("2")
    component_clearance_nm: Nanometres = nm_from_mm("0.5")
    congestion_bin_nm: Nanometres = nm_from_mm("5")
    refinement_passes: int = 2
    refinement_radius_steps: int = 3
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
        if self.refinement_passes < 0 or self.refinement_radius_steps < 1:
            raise ValueError("placement refinement settings are invalid")
        object.__setattr__(self, "fixed_references", frozenset(self.fixed_references))


@dataclass(frozen=True, slots=True)
class PlacementPlan:
    board: PhysicalBoard
    report: LayoutReport


class PlacementPlanningError(ValueError):
    pass


def plan_placement(
    board: PhysicalBoard, options: PlacementPlannerOptions | None = None
) -> PlacementPlan:
    """Produce one deterministic, legal placement candidate and readiness report.

    The planner uses connectivity-aware greedy global placement, exact
    axis-aligned legalization, and bounded local refinement scored by HPWL and
    a coarse negotiated-capacity routing estimate. Existing placements named
    in ``fixed_references`` are treated as immovable mechanical decisions.
    """

    options = options or PlacementPlannerOptions()
    if board.tracks or board.vias:
        raise PlacementPlanningError(
            "placement planning requires an unrouted board; existing copper cannot be moved safely"
        )
    placements = {placement.reference: placement for placement in board.placements}
    unknown_fixed = sorted(options.fixed_references - placements.keys())
    if unknown_fixed:
        raise PlacementPlanningError(
            f"fixed placement references unknown component {unknown_fixed[0]!r}"
        )
    bounds = _rectangular_bounds(board)
    adjacency = _adjacency(board)
    placed: dict[str, Placement] = {}

    for reference in sorted(options.fixed_references):
        candidate = placements[reference]
        if not _legal(candidate, placed, board, bounds, options):
            raise PlacementPlanningError(
                f"fixed component {reference!r} violates board or component clearance"
            )
        placed[reference] = candidate

    movable = sorted(
        (reference for reference in placements if reference not in placed),
        key=lambda reference: (
            -sum(adjacency.get(reference, {}).values()),
            -_footprint_area(board, placements[reference]),
            reference,
        ),
    )
    for reference in movable:
        original = placements[reference]
        candidates = _candidate_positions(board, original, bounds, options)
        best: tuple[int, int, int, Point] | None = None
        for point in candidates:
            candidate = replace(original, position=point)
            if not _legal(candidate, placed, board, bounds, options):
                continue
            cost = _incremental_cost(reference, point, placed, adjacency, bounds)
            ranked = (cost, point.y_nm, point.x_nm, point)
            if best is None or ranked[:3] < best[:3]:
                best = ranked
        if best is None:
            raise PlacementPlanningError(
                f"cannot legalize component {reference!r}; enlarge the board or reduce clearances"
            )
        placed[reference] = replace(original, position=best[3])

    for _ in range(options.refinement_passes):
        changed = False
        for reference in sorted(placed):
            if reference in options.fixed_references:
                continue
            current = placed[reference]
            current_score = _placement_score(board, placed, options)
            without = dict(placed)
            del without[reference]
            best = current
            best_score = current_score
            for point in _nearby_positions(current.position, options):
                candidate = replace(current, position=point)
                if not _legal(candidate, without, board, bounds, options):
                    continue
                trial = dict(without)
                trial[reference] = candidate
                score = _placement_score(board, trial, options)
                if score < best_score or (
                    score == best_score
                    and (point.y_nm, point.x_nm)
                    < (best.position.y_nm, best.position.x_nm)
                ):
                    best = candidate
                    best_score = score
            if best != current:
                placed[reference] = best
                changed = True
        if not changed:
            break

    ordered = tuple(placed[item.reference] for item in board.placements)
    metrics = _metrics(board, placed, options)
    findings = _findings(board, metrics)
    gates = _gates(findings)
    metadata = dict(board.metadata)
    metadata.update(
        {
            "prototype_placement": "false",
            "planned_placement": "true",
            "layout_stage": LayoutStage.PLACE.value,
            "placement_algorithm": "connectivity-greedy/legalize/congestion-refine-v0.1",
            "fabrication_ready": "false",
        }
    )
    planned = replace(
        board,
        placements=ordered,
        metadata=MappingProxyType(metadata),
    )
    return PlacementPlan(
        planned,
        LayoutReport(board.name, gates, findings, metrics),
    )


def _rectangular_bounds(board: PhysicalBoard) -> tuple[int, int, int, int]:
    xs = {point.x_nm for point in board.outline.vertices}
    ys = {point.y_nm for point in board.outline.vertices}
    if len(board.outline.vertices) != 4 or len(xs) != 2 or len(ys) != 2:
        raise PlacementPlanningError(
            "the initial placement planner supports rectangular board outlines only"
        )
    return min(xs), min(ys), max(xs), max(ys)


def _footprint_area(board: PhysicalBoard, placement: Placement) -> int:
    size = board.footprints[placement.footprint].body_size
    return size.width_nm * size.height_nm


def _half_extents(board: PhysicalBoard, placement: Placement) -> tuple[int, int]:
    size = board.footprints[placement.footprint].body_size
    if placement.rotation_degrees % 90:
        raise PlacementPlanningError(
            f"component {placement.reference!r} uses a non-orthogonal rotation; "
            "exact rotated collision geometry is not implemented"
        )
    quarter_turn = int(placement.rotation_degrees // 90) % 2
    width = size.height_nm if quarter_turn else size.width_nm
    height = size.width_nm if quarter_turn else size.height_nm
    return (width + 1) // 2, (height + 1) // 2


def _candidate_positions(
    board: PhysicalBoard,
    placement: Placement,
    bounds: tuple[int, int, int, int],
    options: PlacementPlannerOptions,
) -> tuple[Point, ...]:
    min_x, min_y, max_x, max_y = bounds
    half_width, half_height = _half_extents(board, placement)
    first_x = _ceil_grid(min_x + options.edge_clearance_nm + half_width, options.grid_step_nm)
    first_y = _ceil_grid(min_y + options.edge_clearance_nm + half_height, options.grid_step_nm)
    last_x = max_x - options.edge_clearance_nm - half_width
    last_y = max_y - options.edge_clearance_nm - half_height
    if first_x > last_x or first_y > last_y:
        return ()
    center_x = (min_x + max_x) // 2
    center_y = (min_y + max_y) // 2
    points = tuple(
        Point(x, y)
        for y in range(first_y, last_y + 1, options.grid_step_nm)
        for x in range(first_x, last_x + 1, options.grid_step_nm)
    )
    return tuple(
        sorted(
            points,
            key=lambda point: (
                abs(point.x_nm - center_x) + abs(point.y_nm - center_y),
                point.y_nm,
                point.x_nm,
            ),
        )
    )


def _ceil_grid(value: int, step: int) -> int:
    return ((value + step - 1) // step) * step


def _legal(
    candidate: Placement,
    placed: Mapping[str, Placement],
    board: PhysicalBoard,
    bounds: tuple[int, int, int, int],
    options: PlacementPlannerOptions,
) -> bool:
    min_x, min_y, max_x, max_y = bounds
    half_width, half_height = _half_extents(board, candidate)
    if (
        candidate.position.x_nm - half_width < min_x + options.edge_clearance_nm
        or candidate.position.x_nm + half_width > max_x - options.edge_clearance_nm
        or candidate.position.y_nm - half_height < min_y + options.edge_clearance_nm
        or candidate.position.y_nm + half_height > max_y - options.edge_clearance_nm
    ):
        return False
    for other in placed.values():
        other_width, other_height = _half_extents(board, other)
        if (
            abs(candidate.position.x_nm - other.position.x_nm)
            < half_width + other_width + options.component_clearance_nm
            and abs(candidate.position.y_nm - other.position.y_nm)
            < half_height + other_height + options.component_clearance_nm
        ):
            return False
    return True


def _adjacency(board: PhysicalBoard) -> dict[str, dict[str, int]]:
    adjacency: dict[str, dict[str, int]] = {
        placement.reference: {} for placement in board.placements
    }
    for net in board.nets:
        references = sorted({pad.component for pad in net.pads if pad.component in adjacency})
        if len(references) < 2:
            continue
        weight = max(1, 1000 // ((len(references) - 1) ** 2))
        for index, left in enumerate(references):
            for right in references[index + 1 :]:
                adjacency[left][right] = adjacency[left].get(right, 0) + weight
                adjacency[right][left] = adjacency[right].get(left, 0) + weight
    return adjacency


def _incremental_cost(
    reference: str,
    point: Point,
    placed: Mapping[str, Placement],
    adjacency: Mapping[str, Mapping[str, int]],
    bounds: tuple[int, int, int, int],
) -> int:
    connected = [
        (placed[other].position, weight)
        for other, weight in adjacency.get(reference, {}).items()
        if other in placed
    ]
    if connected:
        connection_cost = sum(
            (abs(point.x_nm - other.x_nm) + abs(point.y_nm - other.y_nm)) * weight
            for other, weight in connected
        )
    else:
        connection_cost = 0
    center_x = (bounds[0] + bounds[2]) // 2
    center_y = (bounds[1] + bounds[3]) // 2
    compactness = abs(point.x_nm - center_x) + abs(point.y_nm - center_y)
    return connection_cost + compactness


def _nearby_positions(
    center: Point, options: PlacementPlannerOptions
) -> tuple[Point, ...]:
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


def _placement_score(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> int:
    hpwl = _hpwl(board, placements)
    _, _, overflow, maximum, _ = _congestion(board, placements, options)
    return hpwl + overflow * options.congestion_bin_nm * 10 + max(0, maximum - 1_000_000)


def _hpwl(board: PhysicalBoard, placements: Mapping[str, Placement]) -> int:
    total = 0
    for net in board.nets:
        points = [
            placements[pad.component].position
            for pad in net.pads
            if pad.component in placements
        ]
        if len(points) >= 2:
            total += max(point.x_nm for point in points) - min(point.x_nm for point in points)
            total += max(point.y_nm for point in points) - min(point.y_nm for point in points)
    return total


def _metrics(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> PlacementMetrics:
    bins, capacity, overflow, maximum, connections = _congestion(
        board, placements, options
    )
    return PlacementMetrics(
        component_count=len(placements),
        net_count=len(board.nets),
        estimated_connection_count=connections,
        half_perimeter_wire_length_nm=_hpwl(board, placements),
        congestion_bin_count=bins,
        congestion_capacity_per_bin=capacity,
        congestion_overflow=overflow,
        maximum_congestion_utilization_ppm=maximum,
    )


def _congestion(
    board: PhysicalBoard,
    placements: Mapping[str, Placement],
    options: PlacementPlannerOptions,
) -> tuple[int, int, int, int, int]:
    min_x, min_y, max_x, max_y = _rectangular_bounds(board)
    columns = max(1, (max_x - min_x + options.congestion_bin_nm - 1) // options.congestion_bin_nm)
    rows = max(1, (max_y - min_y + options.congestion_bin_nm - 1) // options.congestion_bin_nm)
    pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm
    capacity = max(1, options.congestion_bin_nm // pitch) * len(board.stackup.copper_layers)
    demand: dict[tuple[int, int], int] = {}
    connections = 0
    for net in sorted(board.nets, key=lambda item: item.name):
        references = sorted(
            {pad.component for pad in net.pads if pad.component in placements}
        )
        points = {reference: placements[reference].position for reference in references}
        for left, right in _manhattan_mst(points):
            connections += 1
            start = _bin(points[left], min_x, min_y, columns, rows, options.congestion_bin_nm)
            end = _bin(points[right], min_x, min_y, columns, rows, options.congestion_bin_nm)
            horizontal = _grid_path(start, end, True)
            vertical = _grid_path(start, end, False)
            horizontal_cost = sum(demand.get(cell, 0) for cell in horizontal)
            vertical_cost = sum(demand.get(cell, 0) for cell in vertical)
            path = horizontal if (horizontal_cost, horizontal) <= (vertical_cost, vertical) else vertical
            for cell in path:
                demand[cell] = demand.get(cell, 0) + 1
    overflow = sum(max(0, value - capacity) for value in demand.values())
    maximum = max(
        (value * 1_000_000 // capacity for value in demand.values()),
        default=0,
    )
    return columns * rows, capacity, overflow, maximum, connections


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


def _findings(
    board: PhysicalBoard, metrics: PlacementMetrics
) -> tuple[LayoutFinding, ...]:
    findings: list[LayoutFinding] = []
    if board.metadata.get("prototype_footprints") == "true":
        findings.append(
            LayoutFinding(
                "PROXY_FOOTPRINTS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                "proxy footprints are sufficient for workflow testing but not physical signoff",
            )
        )
    if board.metadata.get("footprint_import_warnings"):
        findings.append(
            LayoutFinding(
                "FOOTPRINT_IMPORT_WARNINGS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                "one or more resolved footprints have lossy-import warnings",
            )
        )
    omitted = board.metadata.get("omitted_components", "")
    if omitted:
        findings.append(
            LayoutFinding(
                "OMITTED_COMPONENTS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                f"components without selected footprints were omitted: {omitted}",
            )
        )
    if metrics.congestion_overflow:
        findings.append(
            LayoutFinding(
                "ESTIMATED_CONGESTION",
                LayoutStage.PLACE,
                GateStatus.WARNING,
                f"coarse global-routing estimate has {metrics.congestion_overflow} overflow units",
            )
        )
    findings.append(
        LayoutFinding(
            "DETAILED_ROUTING_NOT_RUN",
            LayoutStage.ROUTE,
            GateStatus.NOT_RUN,
            "placement includes only a coarse routability estimate; no copper routes were generated",
        )
    )
    findings.append(
        LayoutFinding(
            "PHYSICAL_SIGNOFF_NOT_RUN",
            LayoutStage.VERIFY,
            GateStatus.NOT_RUN,
            "DRC, SI/PI, thermal, RF, DFX, and independent CAM verification remain required",
        )
    )
    return tuple(findings)


def _gates(findings: tuple[LayoutFinding, ...]) -> tuple[LayoutGate, ...]:
    prepare_warning = any(
        finding.stage is LayoutStage.PREPARE
        and finding.status is GateStatus.WARNING
        for finding in findings
    )
    place_warning = any(
        finding.stage is LayoutStage.PLACE
        and finding.status is GateStatus.WARNING
        for finding in findings
    )
    return (
        LayoutGate(
            LayoutStage.PREPARE,
            GateStatus.WARNING if prepare_warning else GateStatus.PASS,
            "inputs accepted for placement exploration"
            if prepare_warning
            else "physical inputs are complete for placement",
        ),
        LayoutGate(
            LayoutStage.PLACE,
            GateStatus.WARNING if place_warning else GateStatus.PASS,
            "legal placement produced with congestion warning"
            if place_warning
            else "legal placement produced with no estimated congestion overflow",
        ),
        LayoutGate(LayoutStage.ROUTE, GateStatus.NOT_RUN, "detailed routing not implemented"),
        LayoutGate(LayoutStage.VERIFY, GateStatus.BLOCKED, "release is blocked until routing and signoff pass"),
    )

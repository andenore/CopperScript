"""Bounded transactional placement repair for exact critical-route failures."""
from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal
from time import perf_counter
from typing import Callable, Iterator

from .clusters import move_placement_unit
from .critical import CriticalNetResult, CriticalRoutingResult, _fingerprint, route_critical_nets
from .drc import run_physical_drc
from .physical import PhysicalBoard, Point, RouteKind, nm_from_mm
from .placement import PlacementPlannerOptions, _allowed_orientations, placement_solution_is_legal
from .routing import GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus, _placement_fingerprint, route_global


@dataclass(frozen=True, slots=True)
class CriticalPlacementTrial:
    index: int
    reference: str
    rotation_degrees: str
    position_nm: tuple[int, int]
    changed_references: tuple[str, ...]
    outcome: str
    failed_nets: tuple[str, ...]
    global_fingerprint: str
    critical_fingerprint: str | None
    seconds: float


@dataclass(frozen=True, slots=True)
class CriticalPlacementFeedbackResult:
    board: PhysicalBoard
    global_route: GlobalRoutingResult
    critical: CriticalRoutingResult
    trials: tuple[CriticalPlacementTrial, ...]
    accepted_moves: int


def _failures(board: PhysicalBoard, result: CriticalRoutingResult) -> tuple[frozenset[str], int]:
    expected = {rule.net for rule in board.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    reported = {net for item in result.nets for net in item.nets if not net.startswith("<")}
    failed = expected - reported
    failed.update(net for item in result.nets if not item.connected or item.diagnostics
                  for net in item.nets if net in expected)
    if any((not item.connected or item.diagnostics) and any(net.startswith("<") for net in item.nets)
           for item in result.nets):
        failed.update(expected)
    drc = run_physical_drc(result.board)
    failed.update(net for finding in drc.findings if finding.code == "DRC-OPEN-NET"
                  for net in finding.nets if net in expected)
    hard = sum(finding.severity.value == "error"
               and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"} for finding in drc.findings)
    return frozenset(failed), hard


def critical_placement_trials(
    board: PhysicalBoard, failed: frozenset[str] | set[str],
    options: PlacementPlannerOptions, movement_nm: int,
) -> Iterator[tuple[str, PhysicalBoard]]:
    """Whole-unit, legal rotations first, then bounded local translations."""
    poses = {pose.reference: pose for pose in board.placements}
    affected = {pad.component for net in board.nets if net.name in failed for pad in net.pads}
    fixed = set(options.fixed_references) | {rule.reference for rule in board.placement_rules
                                            if rule.fixed_position is not None}
    for reference in sorted(affected - fixed, key=lambda ref: (len(board.footprints[poses[ref].footprint].pads), ref)):
        current = poses[reference]
        angles = sorted((angle for angle in _allowed_orientations(board, reference)
                         if angle != current.rotation_degrees), key=lambda angle: (
                             0 if (angle - current.rotation_degrees) % Decimal(360) == 180 else 1,
                             min((angle - current.rotation_degrees) % Decimal(360),
                                 (current.rotation_degrees - angle) % Decimal(360)), angle))
        proposals = [replace(current, rotation_degrees=angle) for angle in angles]
        proposals.extend(replace(current, position=Point(current.position.x_nm + dx,
                                                        current.position.y_nm + dy))
                         for dx, dy in ((movement_nm, 0), (-movement_nm, 0),
                                        (0, movement_nm), (0, -movement_nm)))
        for proposed in proposals:
            try:
                trial = move_placement_unit(board, poses, reference, proposed)
            except ValueError:
                continue
            if placement_solution_is_legal(board, trial, options):
                yield reference, replace(board, placements=tuple(trial[pose.reference] for pose in board.placements))


def improve_critical_placement(
    board: PhysicalBoard, global_route: GlobalRoutingResult, critical: CriticalRoutingResult, *,
    maximum_trials: int = 0, movement_nm: int = nm_from_mm("0.5"),
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    on_trial_started: Callable[[int, str, PhysicalBoard], None] | None = None,
    on_trial: Callable[[CriticalPlacementTrial], None] | None = None,
    on_progress: Callable[[str, tuple[str, ...], CriticalNetResult | None], None] | None = None,
) -> CriticalPlacementFeedbackResult:
    """Rebuild all critical reservations; never move beneath accepted copper.

    A trial must globally succeed, introduce no hard native finding, and strictly
    shrink the identity of failed critical nets. Ordinary copper is not yet built.
    """
    if maximum_trials < 0 or movement_nm <= 0:
        raise ValueError("critical placement feedback bounds are invalid")
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("critical placement feedback requires an unrouted, unfilled source")
    if critical.reserved_track_count or critical.reserved_via_count:
        raise ValueError("critical-only placement feedback cannot rebuild package access; use the package-access controller")
    if (replace(critical.board, tracks=(), vias=(), materialized_macros=(), metadata=board.metadata) != board
            or global_route.placement_fingerprint != _placement_fingerprint(board)
            or critical.global_routing_fingerprint != global_route.routing_fingerprint
            or critical.routing_fingerprint != _fingerprint(global_route.routing_fingerprint,
                list(critical.locked_tracks), list(critical.locked_vias), list(critical.nets))
            or critical.board.tracks != critical.locked_tracks or critical.board.vias != critical.locked_vias):
        raise ValueError("critical placement feedback baseline is stale or inconsistent")
    options = placement_options or PlacementPlannerOptions()
    router = global_options or GlobalRouterOptions()
    records: list[CriticalPlacementTrial] = []
    accepted = 0
    failed, _ = _failures(board, critical)
    seen = set()
    while failed and len(records) < maximum_trials:
        improved = False
        for reference, trial in critical_placement_trials(board, failed, options, movement_nm):
            signature = tuple((pose.reference, pose.position, pose.rotation_degrees, pose.side)
                              for pose in trial.placements)
            if signature in seen:
                continue
            seen.add(signature)
            started = perf_counter()
            if on_trial_started:
                on_trial_started(len(records) + 1, reference, trial)
            proposed_global = route_global(trial, router)
            proposed_critical = None
            remaining = failed
            outcome = "global_failed"
            if proposed_global.status is GlobalRoutingStatus.SUCCESS:
                proposed_critical = route_critical_nets(trial, proposed_global, on_progress=on_progress)
                remaining, hard = _failures(trial, proposed_critical)
                if hard:
                    outcome = "hard_drc"
                elif not remaining < failed:
                    outcome = "critical_not_improved"
                else:
                    outcome = "accepted"
            pose = next(pose for pose in trial.placements if pose.reference == reference)
            old = {pose.reference: pose for pose in board.placements}
            record = CriticalPlacementTrial(len(records) + 1, reference, str(pose.rotation_degrees),
                (pose.position.x_nm, pose.position.y_nm),
                tuple(pose.reference for pose in trial.placements if pose != old[pose.reference]),
                outcome, tuple(sorted(remaining)), proposed_global.routing_fingerprint,
                proposed_critical.routing_fingerprint if proposed_critical else None, perf_counter() - started)
            records.append(record)
            if on_trial:
                on_trial(record)
            if outcome == "accepted":
                board, global_route, critical = trial, proposed_global, proposed_critical
                failed = remaining
                accepted += 1
                improved = True
                break
            if len(records) >= maximum_trials:
                break
        if not improved:
            break
    return CriticalPlacementFeedbackResult(board, global_route, critical, tuple(records), accepted)

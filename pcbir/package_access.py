"""Escape-first preflight and bounded placement repair before area routing.

This first implementation allocates ordinary exits jointly, then routes critical
groups with their existing paired/clock/RF owners around those reservations.
It is not a joint optimizer over all ordinary and critical access domains.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Iterator

from .clusters import move_placement_unit
from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .critical_feedback import _failures
from .detailed import (DetailedNetResult, DetailedRoutingMetrics, DetailedRoutingResult,
                       DetailedRoutingStatus, _fingerprint)
from .fanout import FanoutOptions, FanoutResult, route_fanout
from .physical import PadReference, PhysicalBoard, Point, RouteKind, nm_from_mm
from .placement import PlacementPlannerOptions, _allowed_orientations, placement_solution_is_legal
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .routing import (GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus,
                      _placement_fingerprint, route_global)
from .progress import ProgressCallback, critical_progress, emit


@dataclass(frozen=True, slots=True)
class PackageAccessOptions:
    maximum_trials: int = 8
    movement_nm: int = nm_from_mm("0.5")

    def __post_init__(self):
        if self.maximum_trials < 0 or self.movement_nm <= 0:
            raise ValueError("package-access feedback bounds are invalid")


@dataclass(frozen=True, slots=True)
class PackageAccessTrial:
    index: int
    reference: str
    changed_references: tuple[str, ...]
    position_nm: tuple[int, int]
    rotation_degrees: str
    pending_pads: tuple[PadReference, ...]
    failed_critical_nets: tuple[str, ...]
    outcome: str


@dataclass(frozen=True, slots=True)
class PackageAccessResult:
    source: PhysicalBoard
    global_route: GlobalRoutingResult
    fanout: FanoutResult
    critical: CriticalRoutingResult
    plane_stitch: PlaneStitchResult | None
    failed_critical_nets: frozenset[str]
    hard_findings: int
    trials: tuple[PackageAccessTrial, ...] = ()
    accepted_moves: int = 0

    @property
    def board(self) -> PhysicalBoard:
        return self.plane_stitch.board if self.plane_stitch else self.critical.board

    @property
    def pending_pads(self) -> frozenset[PadReference]:
        return frozenset((*self.fanout.pending_pads,
                          *(self.plane_stitch.pending_pads if self.plane_stitch else ())))

    @property
    def ready(self) -> bool:
        return (self.global_route.status is GlobalRoutingStatus.SUCCESS
                and self.critical.status is not CriticalRoutingStatus.FAILED
                and not self.pending_pads and not self.failed_critical_nets and not self.hard_findings)


def preflight_package_access(
    board: PhysicalBoard, global_route: GlobalRoutingResult, fanout_options: FanoutOptions,
    plane_options: PlaneStitchOptions | None = None,
    *, on_progress: ProgressCallback | None = None,
) -> PackageAccessResult:
    """Reserve compatible ordinary exits before any critical long route.

    Critical pairs keep their specialized access/search/profile checks; early
    plane contacts are built against both signal exits and critical copper.
    No ordinary area search is launched by this function.
    """
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("package-access preflight requires an unrouted, unfilled source")
    if global_route.placement_fingerprint != _placement_fingerprint(board):
        raise ValueError("package-access global guides are stale")
    emit(on_progress, "ordinary_package_exits", "started")
    fanout = route_fanout(board, fanout_options)
    emit(on_progress, "ordinary_package_exits", "finished",
         escaped=len(fanout.accesses), pending=len(fanout.pending_pads))
    critical = route_critical_nets(board, global_route, reserved_accesses=fanout,
                                   on_progress=critical_progress(on_progress))
    plane = stitch_zone_pads(critical.board, plane_options) if plane_options else None
    failed, hard = _failures(board, critical)
    if plane is not None:
        from .drc import run_physical_drc
        hard = sum(finding.severity.value == "error"
                   and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
                   for finding in run_physical_drc(plane.board).findings)
    result = PackageAccessResult(board, global_route, fanout, critical, plane, failed, hard)
    emit(on_progress, "package_access", "finished", ready=result.ready,
         pending_pads=[f"{p.component}.{p.pad}" for p in sorted(result.pending_pads)],
         failed_critical_nets=sorted(failed), hard_findings=hard)
    return result


def package_placement_trials(
    baseline: PackageAccessResult, options: PlacementPlannerOptions, movement_nm: int,
) -> Iterator[tuple[str, PhysicalBoard]]:
    """Move failing owners/critical endpoints as legal whole units, never copper.

    Small translations grow in radius; explicit orientations include 45 degrees
    only when the component/cluster rules allow them. Legality preserves fixed
    companions, rigid templates, keepouts and proximity constraints.
    """
    board = baseline.source
    poses = {pose.reference: pose for pose in board.placements}
    owners = {pad.component for pad in baseline.pending_pads}
    owners.update(pad.component for net in board.nets if net.name in baseline.failed_critical_nets
                  for pad in net.pads)
    fixed = set(options.fixed_references) | {rule.reference for rule in board.placement_rules
                                            if rule.fixed_position is not None}
    ranked = sorted(owners - fixed, key=lambda ref: (
        -sum(pad.component == ref for pad in baseline.pending_pads),
        -len(board.footprints[poses[ref].footprint].pads), ref))
    # Interleave owners so one package does not consume every trial.
    proposals = {}
    for reference in ranked:
        current = poses[reference]
        candidates = [replace(current, position=Point(current.position.x_nm + dx * distance,
                                                       current.position.y_nm + dy * distance))
                      for distance in (movement_nm, movement_nm * 2, movement_nm * 4)
                      for dx, dy in ((1,0), (-1,0), (0,1), (0,-1), (1,1), (-1,1), (1,-1), (-1,-1))]
        angles = sorted(_allowed_orientations(board, reference))
        # Test rotations early, without broadening orientation constraints.
        candidates[4:4] = [replace(current, rotation_degrees=angle) for angle in angles
                           if angle != current.rotation_degrees]
        proposals[reference] = candidates
    for index in range(max((len(items) for items in proposals.values()), default=0)):
        for reference in ranked:
            if index >= len(proposals[reference]):
                continue
            try:
                trial = move_placement_unit(board, poses, reference, proposals[reference][index])
            except ValueError:
                continue
            if placement_solution_is_legal(board, trial, options):
                yield reference, replace(board, placements=tuple(trial[pose.reference] for pose in board.placements))


def improve_package_access(
    baseline: PackageAccessResult, fanout_options: FanoutOptions, *,
    options: PackageAccessOptions | None = None,
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    plane_options: PlaneStitchOptions | None = None,
    on_progress: ProgressCallback | None = None,
) -> PackageAccessResult:
    """Accept only identity-preserving access/critical improvements; rollback others."""
    options = options or PackageAccessOptions()
    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    if (baseline.source.tracks or baseline.source.vias or baseline.source.zone_fills
            or baseline.global_route.placement_fingerprint != _placement_fingerprint(baseline.source)
            or replace(baseline.fanout.board, tracks=(), vias=()) != baseline.source
            or baseline.critical.board.placements != baseline.source.placements):
        raise ValueError("package-access feedback source or global guides are stale")
    accepted = baseline
    records = []
    moves = 0
    seen = {_placement_fingerprint(baseline.source)}
    while not accepted.ready and len(records) < options.maximum_trials:
        improved = False
        for reference, trial in package_placement_trials(accepted, placement_options, options.movement_nm):
            signature = _placement_fingerprint(trial)
            if signature in seen:
                continue
            seen.add(signature)
            emit(on_progress, "package_access_trial", "started",
                 index=len(records) + 1, reference=reference)
            guides = route_global(trial, global_options)
            candidate = None
            outcome = "global_failed"
            if guides.status is GlobalRoutingStatus.SUCCESS:
                candidate = preflight_package_access(trial, guides, fanout_options, plane_options,
                                                      on_progress=on_progress)
                required = {item.pad for item in accepted.fanout.pin_analysis}
                tested = {item.pad for item in candidate.fanout.pin_analysis}
                # Eligibility changing with placement must not hide a lost exit.
                ordinary_kept = set(accepted.fanout.accesses) <= set(candidate.fanout.accesses)
                critical_kept = candidate.failed_critical_nets <= accepted.failed_critical_nets
                no_lost_contact = candidate.pending_pads <= accepted.pending_pads
                strict = (candidate.pending_pads < accepted.pending_pads
                          or candidate.failed_critical_nets < accepted.failed_critical_nets)
                if candidate.hard_findings:
                    outcome = "hard_drc"
                elif not (required <= tested and ordinary_kept and critical_kept and no_lost_contact and strict):
                    outcome = "access_not_improved"
                else:
                    outcome = "accepted"
            pose = next(pose for pose in trial.placements if pose.reference == reference)
            old = {pose.reference: pose for pose in accepted.source.placements}
            records.append(PackageAccessTrial(len(records) + 1, reference,
                tuple(pose.reference for pose in trial.placements if pose != old[pose.reference]),
                (pose.position.x_nm, pose.position.y_nm), str(pose.rotation_degrees),
                tuple(sorted(candidate.pending_pads if candidate else accepted.pending_pads)),
                tuple(sorted(candidate.failed_critical_nets if candidate else accepted.failed_critical_nets)), outcome))
            emit(on_progress, "package_access_trial", "finished", index=len(records),
                 reference=reference, outcome=outcome)
            if outcome == "accepted":
                accepted = candidate
                moves += 1
                improved = True
                break
            if len(records) >= options.maximum_trials:
                break
        if not improved:
            break
    return replace(accepted, trials=tuple(records), accepted_moves=moves)


def blocked_area_result(access: PackageAccessResult) -> DetailedRoutingResult:
    """Explicit zero-pass result: preserve diagnostic copper, do not run a router."""
    reason = "area routing blocked: package-access preflight incomplete"
    rules = {rule.net: rule for rule in access.board.net_routing_rules}
    nets = tuple(DetailedNetResult(net.name, False, 0, 0, 0, 0, (reason,))
                 for net in access.board.nets if len(net.pads) >= 2
                 and (net.name not in rules or rules[net.name].kind is RouteKind.GENERAL))
    metadata = {**access.board.metadata, "package_access": "blocked", "detailed_routing": "partial",
                "fabrication_ready": "false"}
    board = replace(access.board, metadata=metadata)
    metrics = DetailedRoutingMetrics(0, len(nets), 0, 0, 0, 0, 0, 0)
    return DetailedRoutingResult(DetailedRoutingStatus.PARTIAL, board, nets, metrics,
        len(board.tracks), len(board.vias), access.global_route.routing_fingerprint,
        _fingerprint(access.global_route.routing_fingerprint, board, metrics))

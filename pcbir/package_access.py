"""Escape-first preflight and bounded placement repair before area routing.

Ordinary exits are allocated jointly, then specialized critical owners and
selected plane contacts are verified. Bounded alternate-order proposals can
replace an incompatible ordinary pattern before any placement or area search.
This is transactional pattern negotiation, not an exhaustive joint optimizer.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Iterator

from .boundary_access import BoundaryAccessOptions, BoundaryAccessResult, analyze_boundary_access
from .clusters import move_placement_unit
from .critical import (CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets,
                       _fingerprint as _critical_fingerprint)
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
    maximum_pattern_trials: int = 2
    boundary_options: BoundaryAccessOptions = BoundaryAccessOptions()
    initial_pair_state_limit: int = 6_000
    reserve_plane_contacts: bool = True

    def __post_init__(self):
        if self.maximum_trials < 0 or self.movement_nm <= 0:
            raise ValueError("package-access feedback bounds are invalid")
        if not 0 <= self.maximum_pattern_trials <= 2:
            raise ValueError("package-access pattern trial budget must be 0..2")
        if self.initial_pair_state_limit < 0:
            raise ValueError("initial pair state limit must be non-negative (0 disables staging)")


@dataclass(frozen=True, slots=True)
class PackagePatternTrial:
    index: int
    strategy: str
    pending_pads: tuple[PadReference, ...]
    failed_critical_nets: tuple[str, ...]
    outcome: str
    revalidated: bool = False
    search_tier: str = "full"


@dataclass(frozen=True, slots=True)
class PackageSearchTier:
    """Actual work including rejected probes; not an acceptance certificate."""
    name: str
    pair_state_limit: int | None
    critical_passes: int
    pair_searches: int
    expanded_states: int
    ready: bool
    selected: bool = False


@dataclass(slots=True)
class _SearchWork:
    """Counters only: do not retain discarded full-board candidate snapshots."""
    critical_passes: int = 0
    pair_searches: int = 0
    expanded_states: int = 0

    def observe(self, result: CriticalRoutingResult) -> None:
        self.critical_passes += 1
        self.pair_searches += sum(net.pair_searches for net in result.nets)
        self.expanded_states += sum(net.search_states for net in result.nets)


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
    pattern_trials: tuple[PackagePatternTrial, ...] = ()
    boundary: BoundaryAccessResult | None = None
    search_tiers: tuple[PackageSearchTier, ...] = ()

    @property
    def board(self) -> PhysicalBoard:
        return self.plane_stitch.board if self.plane_stitch else self.critical.board

    @property
    def pending_pads(self) -> frozenset[PadReference]:
        return frozenset((*self.fanout.pending_pads,
                          *(self.plane_stitch.pending_pads if self.plane_stitch else ()),
                          *(self.boundary.pending_pads if self.boundary else ())))

    @property
    def ready(self) -> bool:
        return (self.global_route.status is GlobalRoutingStatus.SUCCESS
                and self.critical.status is not CriticalRoutingStatus.FAILED
                and not self.pending_pads and not self.failed_critical_nets and not self.hard_findings
                and self.boundary is not None and self.boundary.ready)


def preflight_package_access(
    board: PhysicalBoard, global_route: GlobalRoutingResult, fanout_options: FanoutOptions,
    plane_options: PlaneStitchOptions | None = None,
    *, options: PackageAccessOptions | None = None,
    on_progress: ProgressCallback | None = None,
) -> PackageAccessResult:
    """Reserve compatible ordinary exits before any critical long route.

    Critical pairs keep their specialized access/search/profile checks. When
    reservations block critical/plane owners, negotiate alternate ordinary
    patterns and re-run every owner from the clean source before accepting one.
    No ordinary area search or placement change is launched here.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    options = options or PackageAccessOptions()
    if plane_options is None and options.reserve_plane_contacts and board.zones:
        plane_options = PlaneStitchOptions()
    board = macro_source(board)
    owner_board = materialize_hard_macros(board)
    if global_route.placement_fingerprint != _placement_fingerprint(board):
        raise ValueError("package-access global guides are stale")
    emit(on_progress, "ordinary_package_exits", "started")
    fanout = route_fanout(owner_board, fanout_options)
    emit(on_progress, "ordinary_package_exits", "finished",
         escaped=len(fanout.accesses), pending=len(fanout.pending_pads),
         pending_pads=[f"{p.component}.{p.pad}" for p in fanout.pending_pads])
    staged = (options.initial_pair_state_limit > 0 and options.maximum_pattern_trials > 0
              and any(rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}
                      for rule in board.net_routing_rules))
    tiers, patterns = [], []

    def run_tier(name: str, limit: int | None) -> PackageAccessResult:
        work = _SearchWork()
        emit(on_progress, "package_search_tier", "started", name=name, pair_state_limit=limit)
        critical = _critical_search(board, global_route, fanout, on_progress, limit, work)
        candidate = _access_result(board, global_route, fanout, critical, plane_options,
                                   options.boundary_options, on_progress)
        candidate = _negotiate_patterns(candidate, owner_board, fanout_options,
            options, plane_options, on_progress, pair_state_limit=limit, search_work=work,
            search_tier=name)
        tier = PackageSearchTier(name, limit, work.critical_passes,
                                 work.pair_searches, work.expanded_states, candidate.ready)
        tiers.append(tier)
        patterns.extend(candidate.pattern_trials)
        emit(on_progress, "package_search_tier", "finished", **{
            "name": name, "pair_state_limit": limit, "critical_passes": tier.critical_passes,
            "pair_searches": tier.pair_searches, "expanded_states": tier.expanded_states,
            "ready": candidate.ready})
        return candidate

    result = run_tier("initial" if staged else "full",
                      options.initial_pair_state_limit if staged else None)
    selected = 0
    if staged and not result.ready:
        # Restart from the ORIGINAL ordinary pattern with historical search
        # bounds. Limited failure is never interpreted as proof of no solution.
        emit(on_progress, "package_search_fallback", "started", reason="initial_access_incomplete")
        full = run_tier("full", None)
        if _improvement_outcome(result, full, allow_equal=True) == "accepted":
            result, selected = full, 1
        emit(on_progress, "package_search_fallback", "finished", selected=tiers[selected].name,
             ready=result.ready)
    result = replace(result,
        pattern_trials=tuple(replace(trial, index=index) for index, trial in enumerate(patterns, 1)),
        search_tiers=tuple(replace(tier, selected=index == selected) for index, tier in enumerate(tiers)))
    emit(on_progress, "package_access", "finished", ready=result.ready,
         pending_pads=[f"{p.component}.{p.pad}" for p in sorted(result.pending_pads)],
         failed_critical_nets=sorted(result.failed_critical_nets), hard_findings=result.hard_findings)
    return result


def _access_result(
    board: PhysicalBoard, guides: GlobalRoutingResult, fanout: FanoutResult,
    critical: CriticalRoutingResult, plane_options: PlaneStitchOptions | None,
    boundary_options: BoundaryAccessOptions | None = None,
    on_progress: ProgressCallback | None = None,
) -> PackageAccessResult:
    plane = stitch_zone_pads(critical.board, plane_options) if plane_options else None
    failed, hard = _failures(board, critical)
    if plane is not None:
        from .drc import run_physical_drc
        hard = sum(finding.severity.value == "error"
                   and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
                   for finding in run_physical_drc(plane.board).findings)
    emit(on_progress, "package_boundary_access", "started")
    boundary = analyze_boundary_access(plane.board if plane else critical.board, fanout, boundary_options)
    emit(on_progress, "package_boundary_access", "finished", allocated=len(boundary.ports),
         pending_pads=[f"{p.component}.{p.pad}" for p in boundary.pending_pads],
         native_accepted=boundary.native_accepted)
    return PackageAccessResult(board, guides, fanout, critical, plane, failed, hard, boundary=boundary)


def _improvement_outcome(baseline: PackageAccessResult, candidate: PackageAccessResult,
                         *, allow_equal: bool = False) -> str:
    required = {item.pad for item in baseline.fanout.pin_analysis}
    tested = {item.pad for item in candidate.fanout.pin_analysis}
    if candidate.hard_findings:
        return "hard_drc"
    if baseline.boundary is not None:
        if (candidate.boundary is None or not candidate.boundary.native_accepted
                or not {p.pad for p in baseline.boundary.pin_analysis} <=
                       {p.pad for p in candidate.boundary.pin_analysis}
                or not {p.pad for p in baseline.boundary.ports} <=
                       {p.pad for p in candidate.boundary.ports}):
            return "access_not_improved"
    # Counts alone cannot hide a lost pin or exchange one failed interface for
    # another. The same identity-preserving gate is used for placement trials.
    if not (required <= tested
            and set(baseline.fanout.accesses) <= set(candidate.fanout.accesses)
            and candidate.failed_critical_nets <= baseline.failed_critical_nets
            and candidate.pending_pads <= baseline.pending_pads
            and (allow_equal or candidate.pending_pads < baseline.pending_pads
                 or candidate.failed_critical_nets < baseline.failed_critical_nets)):
        return "access_not_improved"
    return "accepted"


def _negotiate_patterns(
    baseline: PackageAccessResult, owner_board: PhysicalBoard, fanout_options: FanoutOptions,
    options: PackageAccessOptions, plane_options: PlaneStitchOptions | None,
    on_progress: ProgressCallback | None,
    *, pair_state_limit: int | None = None,
    search_work: _SearchWork | None = None,
    search_tier: str = "full",
) -> PackageAccessResult:
    """Use alternate owners as obstacles only while proposing ordinary exits.

    Probe copper is never committed. Only fanout-owned stubs are rebased onto
    the original macro-materialized source; critical profiles and plane contacts
    are then rebuilt against those stubs. Failed trials retain the full incumbent.
    """
    plane_pending = bool(baseline.plane_stitch and baseline.plane_stitch.pending_pads)
    ordinary_pending = bool(baseline.fanout.pending_pads or baseline.boundary and baseline.boundary.pending_pads)
    other_owners = bool(baseline.critical.nets or plane_options and baseline.source.zones)
    if (not options.maximum_pattern_trials or baseline.hard_findings
            or baseline.global_route.status is not GlobalRoutingStatus.SUCCESS
            or not (baseline.failed_critical_nets or plane_pending or ordinary_pending and other_owners)):
        return baseline
    # Already-successful critical copper can seed the proposal without a second
    # expensive pair search. Strip only the verified fanout prefix; ownership,
    # source copper and all critical geometry remain explicit. This reuse is a
    # proposal optimization, never a substitute for final owner revalidation.
    prefix_tracks = (*owner_board.tracks, *baseline.fanout.created_tracks)
    prefix_vias = (*owner_board.vias, *baseline.fanout.created_vias)
    reuse = (not baseline.failed_critical_nets
             and baseline.critical.status is not CriticalRoutingStatus.FAILED
             and baseline.critical.board.tracks[:len(prefix_tracks)] == prefix_tracks
             and baseline.critical.board.vias[:len(prefix_vias)] == prefix_vias)
    emit(on_progress, "package_pattern_probe", "started", reused_critical=reuse,
         search_tier=search_tier)
    if reuse:
        tracks = (*owner_board.tracks, *baseline.critical.board.tracks[len(prefix_tracks):])
        vias = (*owner_board.vias, *baseline.critical.board.vias[len(prefix_vias):])
        probe = replace(baseline.critical,
            board=replace(baseline.critical.board, tracks=tracks, vias=vias),
            locked_tracks=tracks, locked_vias=vias, reserved_track_count=0, reserved_via_count=0,
            routing_fingerprint=_critical_fingerprint(baseline.global_route.routing_fingerprint,
                                                     list(tracks), list(vias), list(baseline.critical.nets)))
    else:
        probe = _critical_search(baseline.source, baseline.global_route, None,
                                 on_progress, pair_state_limit, search_work)
    probe_failed, probe_hard = _failures(baseline.source, probe)
    emit(on_progress, "package_pattern_probe", "finished",
         failed_critical_nets=sorted(probe_failed), hard_findings=probe_hard, reused_critical=reuse,
         search_tier=search_tier)
    # Spend the bounded budget on clean-source probes with a strictly smaller
    # failure set, or on failed plane access. This is a heuristic, not proof
    # that the skipped joint search has no solution.
    if probe_hard or (not (plane_pending or ordinary_pending)
                      and not probe_failed < baseline.failed_critical_nets):
        return baseline
    include_planes = bool(plane_options and baseline.source.zones and (plane_pending or ordinary_pending))
    strategies = ("critical_and_plane_first", "critical_first") if include_planes else ("critical_first",)
    accepted, records = baseline, []
    seen = {(baseline.fanout.created_tracks, baseline.fanout.created_vias)}
    for strategy in strategies[:options.maximum_pattern_trials]:
        emit(on_progress, "package_pattern_trial", "started", index=len(records) + 1, strategy=strategy,
             search_tier=search_tier)
        obstacles = (stitch_zone_pads(probe.board, plane_options).board
                     if strategy == "critical_and_plane_first" else probe.board)
        proposed = route_fanout(obstacles, fanout_options)
        # Do not leak critical metadata/copper into ordinary ownership. Existing
        # macro copper belongs to the source, not to the proposed fanout pattern.
        proposed = replace(proposed, board=replace(owner_board,
            tracks=(*owner_board.tracks, *proposed.created_tracks),
            vias=(*owner_board.vias, *proposed.created_vias)))
        signature = proposed.created_tracks, proposed.created_vias
        candidate = None
        if signature in seen:
            outcome = "unchanged_pattern"
        elif not set(accepted.fanout.accesses) <= set(proposed.accesses):
            outcome = "lost_ordinary_access"
        else:
            seen.add(signature)
            verified = _critical_search(baseline.source, baseline.global_route, proposed,
                                        on_progress, pair_state_limit, search_work)
            candidate = _access_result(baseline.source, baseline.global_route, proposed, verified, plane_options,
                                       options.boundary_options, on_progress)
            outcome = _improvement_outcome(accepted, candidate)
        evidence = candidate or accepted
        records.append(PackagePatternTrial(len(records) + 1, strategy,
            tuple(sorted(candidate.pending_pads if candidate else
                         accepted.pending_pads | frozenset(proposed.pending_pads))),
            tuple(sorted(evidence.failed_critical_nets)), outcome, candidate is not None, search_tier))
        emit(on_progress, "package_pattern_trial", "finished", index=len(records), strategy=strategy,
             outcome=outcome, search_tier=search_tier)
        if outcome == "accepted":
            accepted = candidate
            if accepted.ready:
                break
    return replace(accepted, pattern_trials=tuple(records))


def _critical_search(board: PhysicalBoard, guides: GlobalRoutingResult,
                     fanout: FanoutResult | None, on_progress: ProgressCallback | None,
                     pair_state_limit: int | None,
                     search_work: _SearchWork | None) -> CriticalRoutingResult:
    kwargs = {"on_progress": critical_progress(on_progress)}
    if fanout is not None:
        kwargs["reserved_accesses"] = fanout
    if pair_state_limit is not None:
        kwargs["pair_state_limit"] = pair_state_limit
    result = route_critical_nets(board, guides, **kwargs)
    if search_work is not None:
        search_work.observe(result)
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
    from .hard_macros import materialize_hard_macros
    owner_board = materialize_hard_macros(baseline.source)
    if (baseline.source.tracks or baseline.source.vias or baseline.source.zone_fills
            or baseline.global_route.placement_fingerprint != _placement_fingerprint(baseline.source)
            or replace(baseline.fanout.board, tracks=owner_board.tracks, vias=owner_board.vias) != owner_board
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
                                                      options=options, on_progress=on_progress)
                outcome = _improvement_outcome(accepted, candidate)
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

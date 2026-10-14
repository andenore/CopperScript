"""Escape-first preflight and bounded placement repair before area routing.

Ordinary exits are allocated jointly, then specialized critical owners and
selected plane contacts are verified. Bounded alternate-order proposals can
replace an incompatible ordinary pattern before any placement or area search.
This is transactional pattern negotiation, not an exhaustive joint optimizer.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from collections import Counter
from types import MappingProxyType
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
from .placement import (PlacementPlannerOptions, _allowed_orientations,
                        placement_metrics, placement_solution_is_legal, placement_rejection_reasons)
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
    maximum_placement_probes: int = 16
    maximum_partial_owner_nets: int = 8

    def __post_init__(self):
        if self.maximum_trials < 0 or self.movement_nm <= 0:
            raise ValueError("package-access feedback bounds are invalid")
        if not 0 <= self.maximum_pattern_trials <= 2:
            raise ValueError("package-access pattern trial budget must be 0..2")
        if self.initial_pair_state_limit < 0:
            raise ValueError("initial pair state limit must be non-negative (0 disables staging)")
        if type(self.maximum_placement_probes) is not int or self.maximum_placement_probes < 0:
            raise ValueError("package placement probe budget must be a non-negative integer")
        if type(self.maximum_partial_owner_nets) is not int or not 0 <= self.maximum_partial_owner_nets <= 8:
            raise ValueError("partial ordinary owner budget must be an integer in 0..8")


@dataclass(frozen=True, slots=True)
class PackagePatternTrial:
    index: int
    strategy: str
    pending_pads: tuple[PadReference, ...]
    failed_critical_nets: tuple[str, ...]
    outcome: str
    revalidated: bool = False
    search_tier: str = "full"
    displaced_nets: tuple[str, ...] = ()
    retained_track_count: int = 0
    retained_via_count: int = 0
    partial_skipped_reason: str = ""
    failed_bypass_capacitors: tuple[PadReference, ...] = ()


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
    local_contact_searches: int = 0
    local_contact_expanded_states: int = 0


@dataclass(slots=True)
class _SearchWork:
    """Counters only: do not retain discarded full-board candidate snapshots."""
    critical_passes: int = 0
    pair_searches: int = 0
    expanded_states: int = 0
    local_contact_searches: int = 0
    local_contact_expanded_states: int = 0

    def observe_local(self, preparation) -> None:
        if preparation is not None:
            self.local_contact_searches += preparation.searches
            self.local_contact_expanded_states += preparation.expanded_states

    def observe(self, result: CriticalRoutingResult) -> None:
        self.critical_passes += 1
        self.pair_searches += sum(net.pair_searches for net in result.nets)
        self.expanded_states += sum(net.search_states for net in result.nets)
        for net in result.nets:
            self.observe_local(net.local_preparation)


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
    placement_probe_index: int | None = None


@dataclass(frozen=True, slots=True)
class PackagePlacementProbe:
    """Local ranking evidence, not critical routing or filled connectivity."""
    index: int
    reference: str
    placement_fingerprint: str
    ordinary_pending_pads: tuple[PadReference, ...]
    plane_pending_pads: tuple[PadReference, ...]
    boundary_pending_pads: tuple[PadReference, ...]
    lost_access_pads: tuple[PadReference, ...]
    hard_findings: int
    placement_quality: tuple[int, ...]
    rank: tuple[int, ...]


@dataclass(frozen=True, slots=True)
class PackagePlacementConstraint:
    """A sampled rejected proposal, not proof that all placements are impossible."""
    reference: str
    position_nm: tuple[int, int]
    rotation_degrees: str
    changed_references: tuple[str, ...]
    reasons: tuple[str, ...]
    kind: str = "rejected_pose"


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
    placement_probes: tuple[PackagePlacementProbe, ...] = ()
    placement_stop_reason: str | None = None
    placement_constraints: tuple[PackagePlacementConstraint, ...] = ()

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
         pending_pads=[f"{p.component}.{p.pad}" for p in fanout.pending_pads],
         **({"surface_escaped_pads": [f"{p.component}.{p.pad}" for p in fanout.surface_accesses]}
            if fanout.surface_accesses else {}))
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
                                 work.pair_searches, work.expanded_states, candidate.ready,
                                 local_contact_searches=work.local_contact_searches,
                                 local_contact_expanded_states=work.local_contact_expanded_states)
        tiers.append(tier)
        patterns.extend(candidate.pattern_trials)
        emit(on_progress, "package_search_tier", "finished", **{
            "name": name, "pair_state_limit": limit, "critical_passes": tier.critical_passes,
            "pair_searches": tier.pair_searches, "expanded_states": tier.expanded_states,
            "local_contact_searches": tier.local_contact_searches,
            "local_contact_expanded_states": tier.local_contact_expanded_states,
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
    plane = None
    if plane_options:
        emit(on_progress, "package_plane_stitch", "started", purpose="contact_validation")
        plane = stitch_zone_pads(critical.board, plane_options)
        emit(on_progress, "package_plane_stitch", "finished", purpose="contact_validation",
             pending=len(plane.pending_pads))
    emit(on_progress, "package_critical_drc", "started")
    failed, hard = _failures(board, critical)
    emit(on_progress, "package_critical_drc", "finished", hard_findings=hard,
         failed_critical_nets=sorted(failed))
    if plane is not None:
        from .drc import run_physical_drc
        emit(on_progress, "package_contact_drc", "started")
        hard = sum(finding.severity.value == "error"
                   and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
                   for finding in run_physical_drc(plane.board).findings)
        emit(on_progress, "package_contact_drc", "finished", hard_findings=hard)
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
            and baseline.fanout.escaped_pads <= candidate.fanout.escaped_pads
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
    allow_contact_trial: bool = True,
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
            or not (baseline.failed_critical_nets or plane_pending or ordinary_pending and other_owners)):
        return baseline
    if allow_contact_trial and options.maximum_partial_owner_nets:
        contact = _contact_pattern_trial(baseline, owner_board, fanout_options, options,
                                         plane_options, on_progress, pair_state_limit,
                                         search_work, search_tier)
        if contact is not None:
            remaining = options.maximum_pattern_trials - 1
            if contact.ready or not remaining:
                return contact
            followup = _negotiate_patterns(replace(contact, pattern_trials=()), owner_board, fanout_options,
                replace(options, maximum_pattern_trials=remaining), plane_options, on_progress,
                pair_state_limit=pair_state_limit, search_work=search_work, search_tier=search_tier,
                allow_contact_trial=False)
            records = (*contact.pattern_trials, *followup.pattern_trials)
            return replace(followup, pattern_trials=tuple(
                replace(record, index=index) for index, record in enumerate(records, 1)))
    # An unreachable coarse planner must not disable exact source-terminal
    # ownership probes. These are disposable proposals, and the existing full
    # owner/pad/DRC revalidation and area-readiness gates still decide adoption.
    # Already-successful critical copper can seed the proposal without a second
    # expensive pair search. Strip only the verified fanout prefix; ownership,
    # source copper and all critical geometry remain explicit. This reuse is a
    # proposal optimization, never a substitute for final owner revalidation.
    prefix_tracks = (*owner_board.tracks, *baseline.fanout.created_tracks)
    prefix_vias = (*owner_board.vias, *baseline.fanout.created_vias)
    reuse = (not baseline.failed_critical_nets
             and not baseline.critical.plane_reservation_track_count
             and not baseline.critical.plane_reservation_via_count
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
                                 on_progress, pair_state_limit, search_work,
                                 retained_critical=baseline.critical, retained_accesses=baseline.fanout)
    emit(on_progress, "package_probe_drc", "started")
    probe_failed, probe_hard = _failures(baseline.source, probe)
    emit(on_progress, "package_probe_drc", "finished", hard_findings=probe_hard,
         failed_critical_nets=sorted(probe_failed))
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
    strategies = (["critical_and_plane_first", "critical_first"] if include_planes else
                  ["critical_first", "partial_ordinary"])
    accepted, records = baseline, []
    seen = {(baseline.fanout.created_tracks, baseline.fanout.created_vias, False)}
    for index in range(min(len(strategies), options.maximum_pattern_trials)):
        strategy = strategies[index]
        if strategy == "partial_ordinary" and (not records or records[-1].outcome != "lost_ordinary_access"
                                                or not options.maximum_partial_owner_nets):
            continue
        partial_skipped_reason = ""
        if strategy == "partial_ordinary":
            _, partial_skipped_reason = _partial_ordinary_release(
                baseline.fanout, owner_board, probe.board, options.maximum_partial_owner_nets)
            if partial_skipped_reason == "protected_ordinary_owner":
                _, bypass_reason = _partial_ordinary_release(
                    baseline.fanout, owner_board, probe.board, options.maximum_partial_owner_nets, True)
                if not bypass_reason:
                    strategy, partial_skipped_reason = "partial_bypass", ""
            if partial_skipped_reason and records[-1].strategy != "critical_first":
                # Cheap eligibility failure must not suppress the established
                # critical-only fallback after a plane-first proposal. No third
                # pattern/search is purchased and protected feeds stay intact.
                strategy = "critical_first"
            elif partial_skipped_reason:
                records.append(PackagePatternTrial(len(records) + 1, strategy,
                    tuple(sorted(accepted.pending_pads)), tuple(sorted(accepted.failed_critical_nets)),
                    partial_skipped_reason, search_tier=search_tier,
                    partial_skipped_reason=partial_skipped_reason))
                emit(on_progress, "package_partial_proposal", "skipped", reason=partial_skipped_reason,
                     search_tier=search_tier)
                continue
        displaced, retained_tracks, retained_vias = (), 0, 0
        emit(on_progress, "package_pattern_trial", "started", index=len(records) + 1, strategy=strategy,
             search_tier=search_tier)
        if strategy == "critical_and_plane_first":
            emit(on_progress, "package_plane_stitch", "started", purpose="proposal_obstacles")
            stitched = stitch_zone_pads(probe.board, plane_options)
            emit(on_progress, "package_plane_stitch", "finished", purpose="proposal_obstacles",
                 pending=len(stitched.pending_pads))
            obstacles = stitched.board
            # When the critical-first probe itself blocks contacts, repeating
            # critical-first cannot reserve them. Spend the second bounded trial
            # on the opposite order, with fresh all-owner validation below.
            if stitched.pending_pads and len(strategies) > 1:
                strategies[1] = "plane_first"
        elif strategy == "plane_first":
            emit(on_progress, "package_plane_stitch", "started", purpose="plane_first_proposal")
            stitched = stitch_zone_pads(owner_board, plane_options)
            emit(on_progress, "package_plane_stitch", "finished", purpose="plane_first_proposal",
                 pending=len(stitched.pending_pads))
            obstacles = stitched.board
        else:
            obstacles = probe.board
        emit(on_progress, "package_pattern_fanout", "started", strategy=strategy)
        failed_bypass_capacitors = []
        if strategy in {"partial_ordinary", "partial_bypass"}:
            proposed, displaced, retained_tracks, retained_vias, partial_outcome = _partial_ordinary_pattern(
                baseline.fanout, owner_board, obstacles, fanout_options, options.maximum_partial_owner_nets,
                strategy == "partial_bypass", failed_bypass_capacitors)
        else:
            proposed = route_fanout(obstacles, fanout_options)
        emit(on_progress, "package_pattern_fanout", "finished", strategy=strategy,
             escaped=len(proposed.accesses) if proposed else 0,
             pending=len(proposed.pending_pads) if proposed else 0,
             displaced_nets=list(displaced), retained_tracks=retained_tracks, retained_vias=retained_vias)
        if proposed is None:
            records.append(PackagePatternTrial(len(records) + 1, strategy,
                tuple(sorted(accepted.pending_pads)), tuple(sorted(accepted.failed_critical_nets)),
                partial_outcome, search_tier=search_tier, displaced_nets=displaced,
                failed_bypass_capacitors=tuple(failed_bypass_capacitors)))
            emit(on_progress, "package_pattern_trial", "finished", index=len(records), strategy=strategy,
                 outcome=partial_outcome, search_tier=search_tier,
                 failed_bypass_capacitors=[f"{p.component}.{p.pad}" for p in failed_bypass_capacitors])
            continue
        # Do not leak critical metadata/copper into ordinary ownership. Existing
        # macro copper belongs to the source, not to the proposed fanout pattern.
        proposed = replace(proposed, board=replace(owner_board,
            tracks=(*owner_board.tracks, *proposed.created_tracks),
            vias=(*owner_board.vias, *proposed.created_vias)))
        signature = proposed.created_tracks, proposed.created_vias, strategy == "plane_first"
        candidate = None
        if signature in seen:
            outcome = "unchanged_pattern"
        elif not accepted.fanout.escaped_pads <= proposed.escaped_pads:
            outcome = "lost_ordinary_access"
        else:
            seen.add(signature)
            verified = _critical_search(baseline.source, baseline.global_route, proposed,
                                        on_progress, pair_state_limit, search_work,
                                        plane_options=plane_options if strategy == "plane_first" else None,
                                        retained_critical=baseline.critical, retained_accesses=baseline.fanout)
            candidate = _access_result(baseline.source, baseline.global_route, proposed, verified, plane_options,
                                       options.boundary_options, on_progress)
            outcome = _improvement_outcome(accepted, candidate)
            if (include_planes and candidate.plane_stitch is not None
                    and candidate.plane_stitch.pending_pads and len(strategies) > 1):
                # A contact may be feasible in the probe yet lost when critical
                # copper is rebuilt around the changed ordinary pattern.
                strategies[1] = "plane_first"
        evidence = candidate or accepted
        records.append(PackagePatternTrial(len(records) + 1, strategy,
            tuple(sorted(candidate.pending_pads if candidate else
                         accepted.pending_pads | frozenset(proposed.pending_pads))),
            tuple(sorted(evidence.failed_critical_nets)), outcome, candidate is not None, search_tier,
            displaced, retained_tracks, retained_vias, partial_skipped_reason))
        emit(on_progress, "package_pattern_trial", "finished", index=len(records), strategy=strategy,
             outcome=outcome, search_tier=search_tier)
        if outcome == "accepted":
            accepted = candidate
            if accepted.ready:
                break
        elif (outcome == "lost_ordinary_access" and len(strategies) > index + 1
              and strategies[index + 1] != "plane_first" and options.maximum_partial_owner_nets):
            strategies[index + 1] = "partial_ordinary"
    return replace(accepted, pattern_trials=tuple(records))


def _contact_pattern_trial(baseline, owner_board, fanout_options, options, plane_options,
                           on_progress, pair_state_limit, search_work, search_tier):
    """Spend one existing pattern slot on measured local contact competition.

    The clean local branch is disposable. Only exact generated ordinary owners
    can be reallocated; full current critical, planes and boundary gates still
    decide adoption. No whole-board clean critical probe is purchased here.
    """
    selected = next((item for item in baseline.critical.nets
        if not item.connected and len(item.nets) == 2 and item.local_preparation is not None
        and item.local_preparation.status == "blocked"
        and item.local_preparation.paired_topology_supported), None)
    if selected is None:
        return None
    from .pair_terminals import PairTerminalPreparationStats, prepare_pair_terminals
    from .contact_owner_regeneration import regenerate_contact_owners
    rules = {rule.net: rule for rule in owner_board.net_routing_rules}
    if any(name not in rules for name in selected.nets):
        return None
    first, second = (rules[name] for name in selected.nets)
    if first.kind is not RouteKind.DIFFERENTIAL or second.kind is not RouteKind.DIFFERENTIAL:
        return None
    emit(on_progress, "package_pattern_trial", "started", index=1, strategy="contact_first",
         search_tier=search_tier, nets=list(selected.nets))
    report = PairTerminalPreparationStats()
    _, a, b, pairs, returns = prepare_pair_terminals(owner_board, first, second, {}, report=report)
    if search_work is not None:
        search_work.observe_local(report)
    proposed = None
    if a and b:
        proposed = regenerate_contact_owners(owner_board, baseline.fanout, selected.nets,
            (*a, *b), (*[via for pair in pairs for via in pair], *returns), fanout_options,
            maximum_owner_nets=options.maximum_partial_owner_nets)
    candidate = None
    outcome = "contact_probe_blocked" if not a or not b else "contact_owner_proposal_rejected"
    if proposed is not None:
        verified = _critical_search(baseline.source, baseline.global_route, proposed,
            on_progress, pair_state_limit, search_work,
            retained_critical=baseline.critical, retained_accesses=baseline.fanout)
        candidate = _access_result(baseline.source, baseline.global_route, proposed, verified,
            plane_options, options.boundary_options, on_progress)
        outcome = _improvement_outcome(baseline, candidate)
    accepted = candidate if outcome == "accepted" else baseline
    record = PackagePatternTrial(1, "contact_first", tuple(sorted(accepted.pending_pads)),
        tuple(sorted(accepted.failed_critical_nets)), outcome, candidate is not None, search_tier)
    emit(on_progress, "package_pattern_trial", "finished", index=1, strategy="contact_first",
         outcome=outcome, search_tier=search_tier,
         local_contact_searches=report.searches, local_contact_expanded_states=report.expanded_states)
    return replace(accepted, pattern_trials=(record,))


def _partial_ordinary_pattern(original, owner, obstacles, settings, maximum_nets,
                              regenerate_bypass=False, failed_bypass_capacitors=None):
    """Keep legal generated ownership exactly; rebuild only intersecting nets.

    The clean critical probe is a disposable obstacle, not committed copper.
    Net-granular release avoids guessing ownership of shared same-net stubs.
    Source/macro/array copper is never in this removable contribution. Semantic
    feeds remain protected by default; the explicit regeneration trial requires
    every previously verified source association to be independently witnessed
    again against the complete proposal. Final critical/contact/boundary checks
    still rebuild every owner against the combined ordinary contribution.
    """
    names = {pad: net.name for net in owner.nets for pad in net.pads}
    ordered, reason = _partial_ordinary_release(original, owner, obstacles, maximum_nets, regenerate_bypass)
    if reason:
        return None, ordered, 0, 0, reason
    displaced = frozenset(ordered)
    feed_counts = Counter(original.decoupling_tracks)
    held_tracks = []
    for track in original.created_tracks:
        if feed_counts[track]:
            if not regenerate_bypass or track.net not in displaced:
                held_tracks.append(track)
            feed_counts[track] -= 1
        elif track.net not in displaced:
            held_tracks.append(track)
    tracks = tuple(held_tracks)
    vias = tuple(v for v in original.created_vias if v.net not in displaced)
    held = replace(obstacles, tracks=(*obstacles.tracks, *tracks), vias=(*obstacles.vias, *vias))
    required_bypass = _verified_bypass_links(original.board, displaced) if regenerate_bypass else frozenset()
    rebuilt = route_fanout(held, settings, only_nets=frozenset(displaced))
    missing_bypass = required_bypass - _verified_bypass_links(rebuilt.board, displaced)
    if missing_bypass:
        if failed_bypass_capacitors is not None:
            failed_bypass_capacitors.extend(sorted({link.capacitor for link in missing_bypass})[:32])
        return None, ordered, len(tracks), len(vias), "lost_bypass_connection"
    # Detect accidental owner/prefix replacement, including duplicate geometry.
    if (rebuilt.board.tracks != (*held.tracks, *rebuilt.created_tracks)
            or rebuilt.board.vias != (*held.vias, *rebuilt.created_vias)):
        raise ValueError("partial ordinary rebuild replaced input ownership")
    combined_tracks, combined_vias = (*tracks, *rebuilt.created_tracks), (*vias, *rebuilt.created_vias)
    if Counter(held.tracks) - Counter(rebuilt.board.tracks) or Counter(held.vias) - Counter(rebuilt.board.vias):
        raise ValueError("partial ordinary rebuild lost input copper multiplicity")
    def retained(mapping):
        return {pad: value for pad, value in (mapping or {}).items() if names[pad] not in displaced}
    proposed = replace(rebuilt,
        board=replace(owner, tracks=(*owner.tracks, *combined_tracks), vias=(*owner.vias, *combined_vias)),
        accesses=MappingProxyType({**retained(original.accesses), **rebuilt.accesses}),
        surface_accesses=MappingProxyType({**retained(original.surface_accesses), **(rebuilt.surface_accesses or {})}),
        boundary_accesses=None, created_tracks=combined_tracks, created_vias=combined_vias,
        added_track_count=len(combined_tracks), added_via_count=len(combined_vias),
        pin_analysis=tuple(a for a in original.pin_analysis if names[a.pad] not in displaced) + rebuilt.pin_analysis,
        decoupling_pending=tuple(p for p in original.decoupling_pending if names[p[0]] not in displaced)
                           + rebuilt.decoupling_pending,
        decoupling_tracks=(*(t for t in original.decoupling_tracks
                             if not regenerate_bypass or t.net not in displaced), *rebuilt.decoupling_tracks))
    return proposed, ordered, len(tracks), len(vias), "proposed"


def _partial_ordinary_release(original, owner, obstacles, maximum_nets, regenerate_bypass=False):
    """Cheap exact eligibility, shared by dispatch and the actual proposal."""
    from .routing_clearance import RoutingClearanceIndex
    if (original.board.tracks != (*owner.tracks, *original.created_tracks)
            or original.board.vias != (*owner.vias, *original.created_vias)
            or replace(original.board, tracks=owner.tracks, vias=owner.vias) != owner):
        raise ValueError("partial ordinary ownership is stale")
    if Counter(original.decoupling_tracks) - Counter(original.created_tracks):
        raise ValueError("partial ordinary semantic feed ownership is stale")
    names = {pad: net.name for net in owner.nets for pad in net.pads}
    rules = {rule.net: rule for rule in owner.net_routing_rules}
    index = RoutingClearanceIndex(obstacles)
    displaced = {track.net for track in original.created_tracks
                 if not index.can_track(track.net, track.start, track.end, track.width_nm, track.layer)}
    displaced.update(via.net for via in original.created_vias if not index.can_via(
        via.net, via.position, via.size_nm, via.from_layer, via.to_layer, via.drill_nm))
    displaced.update(names[pad] for pad in original.pending_pads)
    ordered = tuple(sorted(displaced))
    conflicting_feeds = {t.net for t in original.decoupling_tracks
                         if not index.can_track(t.net, t.start, t.end, t.width_nm, t.layer)}
    if any(net in rules and rules[net].kind is not RouteKind.GENERAL for net in displaced):
        return ordered, "protected_ordinary_owner"
    if conflicting_feeds:
        if not regenerate_bypass:
            return ordered, "protected_ordinary_owner"
        # Source/macro copper is outside created_tracks. Only independently
        # verified source-associated generated feeds are eligible; ambiguous
        # or unverified associations never grant permission to remove copper.
        verified = _verified_bypass_links(original.board, conflicting_feeds)
        if not conflicting_feeds <= {link.net for link in verified}:
            return ordered, "protected_ordinary_owner"
    if len(displaced) > maximum_nets:
        return ordered, "partial_owner_budget_exhausted"
    return ordered, ""


def _verified_bypass_links(board, only_nets, *, witness_paths=None):
    """Read-only exact same-surface witnesses for explicit source associations.

    Optional witness_paths receives original input track identities, never
    reconstructed or synthesized ownership. No routing, via shortcut or connected
    flag is used as evidence. Actual
    preferred/breakout widths, every foreign land and all source copper apply.
    """
    from .routing_clearance import RoutingClearanceIndex
    from .pin_escape import _verified_land_path
    from .placement import transformed_pad_position
    from .physical import BoardSide, CopperLayer, PadKind
    from .routing_layers import routing_layers
    from .hard_macros import macro_owned_pads
    if witness_paths is not None:
        witness_paths.clear()
    poses = {p.reference:p for p in board.placements}
    rules = {r.net:r for r in board.net_routing_rules}
    index, owned, verified = RoutingClearanceIndex(board), macro_owned_pads(board), set()
    for link in board.decoupling_links:
        if link.net not in only_nets or link.target in owned or link.capacitor in owned:
            continue
        rule=rules.get(link.net)
        if rule is not None and rule.kind is not RouteKind.GENERAL:
            continue
        target,cap=poses[link.target.component],poses[link.capacitor.component]
        lands=[p for ref,pose in ((link.target,target),(link.capacitor,cap))
               for p in board.footprints[pose.footprint].pads if p.number==ref.pad]
        if cap.side is not target.side or len(lands)!=2 or any(p.kind is not PadKind.SMD for p in lands):
            continue
        layer=CopperLayer.FRONT if target.side is BoardSide.FRONT else CopperLayer.BACK
        if layer not in routing_layers(board,link.net,rule):
            continue
        end=transformed_pad_position(board,cap,link.capacitor.pad)
        width=rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        path = _verified_land_path(board,lands[0],target,link.net,end,index,require_via=False,
                                   target_layer=layer,required_width_nm=width)
        if path is not None:
            verified.add(link)
            if witness_paths is not None:
                witness_paths[link] = path
    return frozenset(verified)


def _critical_search(board: PhysicalBoard, guides: GlobalRoutingResult,
                     fanout: FanoutResult | None, on_progress: ProgressCallback | None,
                     pair_state_limit: int | None,
                     search_work: _SearchWork | None, *,
                     plane_options: PlaneStitchOptions | None = None,
                     retained_critical: CriticalRoutingResult | None = None,
                     retained_accesses: FanoutResult | None = None) -> CriticalRoutingResult:
    emit(on_progress, "package_critical_search", "started", reserved_accesses=fanout is not None,
         pair_state_limit=pair_state_limit, plane_contacts_first=plane_options is not None)
    kwargs = {"on_progress": critical_progress(on_progress)}
    if fanout is not None:
        kwargs["reserved_accesses"] = fanout
    if pair_state_limit is not None:
        kwargs["pair_state_limit"] = pair_state_limit
    if plane_options is not None:
        kwargs["plane_stitch_options"] = plane_options
    if retained_critical is not None:
        kwargs["retained_critical"] = retained_critical
        kwargs["retained_accesses"] = retained_accesses
    result = route_critical_nets(board, guides, **kwargs)
    if search_work is not None:
        search_work.observe(result)
    emit(on_progress, "package_critical_search", "finished", reserved_accesses=fanout is not None,
         pair_state_limit=pair_state_limit, pair_searches=sum(net.pair_searches for net in result.nets),
         expanded_states=sum(net.search_states for net in result.nets),
         plane_contacts_first=plane_options is not None)
    return result


def package_placement_trials(
    baseline: PackageAccessResult, options: PlacementPlannerOptions, movement_nm: int,
    *, constraints: list[PackagePlacementConstraint] | None = None,
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
    # Fixed package endpoints can be fenced by movable semantic bypass feeds.
    # Use only explicit source pin associations as dependency hints; normal
    # whole-unit legality and complete fresh owner validation still apply.
    dependency_owners = {link.capacitor.component for link in board.decoupling_links
                         if link.target.component in owners}
    owners.update(dependency_owners)
    # Measured lost source-associated feeds are independent placement hints,
    # never current pending pads or accepted copper. Check exact source pad
    # identity again; unknown/ground/renamed non-associated pads add no owner.
    source_bypass_pads = {link.capacitor for link in board.decoupling_links}
    bypass_hints = {pad for trial in baseline.pattern_trials if trial.outcome == "lost_bypass_connection"
                    for pad in trial.failed_bypass_capacitors if pad in source_bypass_pads}
    owners.update(pad.component for pad in bypass_hints)
    # A rejected ordering can expose movable ordinary owners even when every
    # endpoint of the original failure is fixed. Keep its copper discarded,
    # but use the measured lost identities as bounded placement-search hints.
    pattern_pads = {pad for trial in baseline.pattern_trials if trial.outcome == "lost_ordinary_access"
                    for pad in trial.pending_pads if pad not in baseline.pending_pads}
    hinted = {pad.component for pad in pattern_pads} - owners
    owners.update(hinted)
    fixed = set(options.fixed_references) | {rule.reference for rule in board.placement_rules
                                            if rule.fixed_position is not None}
    # Bound both report size and expensive explanation work across all moves.
    def record(reference, pose, trial, reasons, kind="rejected_pose"):
        if constraints is not None and len(constraints) < 32:
            item = PackagePlacementConstraint(reference,
                (pose.position.x_nm, pose.position.y_nm), str(pose.rotation_degrees),
                tuple(sorted(ref for ref in trial if trial[ref] != poses[ref])), reasons, kind)
            if item not in constraints:
                constraints.append(item)
    for ref in sorted(owners & fixed):
        record(ref, poses[ref], poses, (f"{ref}: " + (
            "temporary session lock fixes the complete pose" if ref in options.fixed_references
            else "source constraint fixes its position"),), "fixed_owner")
    for ref in sorted({pad.component for pad in bypass_hints} - fixed):
        record(ref, poses[ref], poses, ("rejected generated ownership lost the verified source bypass at "
            + ", ".join(f"{pad.component}.{pad.pad}" for pad in sorted(bypass_hints)
                        if pad.component == ref),), "bypass_dependency_hint")
    for ref in sorted(hinted - fixed):
        record(ref, poses[ref], poses, ("rejected ownership ordering lost access to "
            + ", ".join(f"{pad.component}.{pad.pad}" for pad in sorted(pattern_pads)
                        if pad.component == ref),), "pattern_hint")
    from .placement import transformed_pad_position
    failed_pins = set(baseline.pending_pads)
    failed_pins.update(p for n in board.nets if n.name in baseline.failed_critical_nets for p in n.pads)
    def pin_point(pad):
        pose=poses[pad.component]
        lands=[p for p in board.footprints[pose.footprint].pads if p.number == pad.pad]
        return transformed_pad_position(board,pose,pad.pad) if len(lands) == 1 else None
    failure_points=[point for pad in sorted(failed_pins) if (point:=pin_point(pad)) is not None]
    def dependency_distance(ref):
        if ref not in dependency_owners:
            return 0
        targets=[point for link in board.decoupling_links if link.capacitor.component == ref
                 and (point:=pin_point(link.target)) is not None]
        return min(((a.x_nm-b.x_nm)**2+(a.y_nm-b.y_nm)**2
                    for a in targets for b in failure_points),default=float('inf'))
    ranked = sorted(owners - fixed, key=lambda ref: (
        -sum(pad.component == ref for pad in baseline.pending_pads), dependency_distance(ref),
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
        record(reference, current, poses, (f"{reference}: permitted orientation domain is "
            + ", ".join(format(angle, "f") + " degrees" for angle in angles)
            + "; unlisted rotations are not searched",), "orientation_domain")
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
                record(reference, proposals[reference][index], poses,
                       ("rigid placement unit cannot represent the proposed pose",))
                continue
            if placement_solution_is_legal(board, trial, options):
                yield reference, replace(board, placements=tuple(trial[pose.reference] for pose in board.placements))
            elif constraints is not None and len(constraints) < 32:
                record(reference, proposals[reference][index], trial,
                       placement_rejection_reasons(board, trial, options))


def improve_package_access(
    baseline: PackageAccessResult, fanout_options: FanoutOptions, *,
    options: PackageAccessOptions | None = None,
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    plane_options: PlaneStitchOptions | None = None,
    on_progress: ProgressCallback | None = None,
) -> PackageAccessResult:
    """Rank bounded local probes, then fully validate identity-preserving moves.

    Disposable local access measurements never replace critical/plane/boundary
    preflight, and rejected trials retain the complete incumbent geometry.
    """
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
    probes: list[PackagePlacementProbe] = []
    constraints: list[PackagePlacementConstraint] = []
    moves = 0
    seen = {_placement_fingerprint(baseline.source)}
    if plane_options is None and options.reserve_plane_contacts and baseline.source.zones:
        plane_options = PlaneStitchOptions()
    stop_reason = "ready" if baseline.ready else "disabled"
    while not accepted.ready and len(records) < options.maximum_trials:
        improved = False
        attempted = False
        for reference, trial, probe_index in _ranked_placement_trials(
                accepted, fanout_options, options, placement_options, plane_options,
                seen, probes, on_progress, constraints):
            signature = _placement_fingerprint(trial)
            if signature in seen:
                continue
            seen.add(signature)
            attempted = True
            emit(on_progress, "package_access_trial", "started",
                 index=len(records) + 1, reference=reference, placement_probe_index=probe_index)
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
                tuple(sorted(candidate.failed_critical_nets if candidate else accepted.failed_critical_nets)),
                outcome, probe_index))
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
            stop_reason = ("trial_budget_exhausted" if len(records) >= options.maximum_trials else
                           "no_improvement" if attempted else "no_legal_unseen_trials")
            break
    if accepted.ready:
        stop_reason = "ready"
    elif options.maximum_trials and len(records) >= options.maximum_trials:
        stop_reason = "trial_budget_exhausted"
    emit(on_progress, "package_placement_feedback", "finished", reason=stop_reason,
         trials=len(records), probes=len(probes), probe_limit=options.maximum_placement_probes,
         accepted_moves=moves)
    return replace(accepted, trials=tuple(records), accepted_moves=moves,
                   placement_probes=tuple(probes), placement_stop_reason=stop_reason,
                   placement_constraints=tuple(constraints))


def _ranked_placement_trials(
    baseline: PackageAccessResult, fanout_options: FanoutOptions,
    options: PackageAccessOptions, placement_options: PlacementPlannerOptions,
    plane_options: PlaneStitchOptions | None, seen: set[str],
    records: list[PackagePlacementProbe], on_progress: ProgressCallback | None,
    constraints: list[PackagePlacementConstraint] | None = None,
) -> Iterator[tuple[str, PhysicalBoard, int | None]]:
    """Rank a bounded legal look-ahead; unprobed moves retain generator order.

    Probe copper is disposable. Every selected pose still receives full global,
    critical, ordinary, plane and boundary preflight before it can be accepted.
    The probe budget is shared across accepted moves, not restarted per move.
    """
    remaining = max(0, options.maximum_placement_probes - len(records))
    trials = iter(package_placement_trials(baseline, placement_options, options.movement_nm,
                                         constraints=constraints))
    sampled = []
    local_seen = set(seen)
    emit(on_progress, "package_placement_ranking", "started", remaining_probes=remaining)
    while len(sampled) < remaining:
        try:
            reference, trial = next(trials)
        except StopIteration:
            break
        signature = _placement_fingerprint(trial)
        if signature in local_seen:
            continue
        local_seen.add(signature)
        emit(on_progress, "package_placement_probe", "started", index=len(records) + 1,
             reference=reference, placement_fingerprint=signature)
        probe = _placement_access_probe(baseline, reference, trial, fanout_options,
            placement_options, plane_options, options.boundary_options, len(records) + 1)
        records.append(probe)
        sampled.append((probe.rank, len(sampled), reference, trial, probe.index))
        emit(on_progress, "package_placement_probe", "finished", index=probe.index,
             reference=reference, placement_fingerprint=signature, rank=list(probe.rank),
             ordinary_pending=len(probe.ordinary_pending_pads),
             plane_pending=len(probe.plane_pending_pads),
             boundary_pending=len(probe.boundary_pending_pads), hard_findings=probe.hard_findings)
    emit(on_progress, "package_placement_ranking", "finished", probes=len(sampled),
         total_probes=len(records), probe_limit=options.maximum_placement_probes)
    for _, _, reference, trial, index in sorted(sampled, key=lambda item: item[:2]):
        yield reference, trial, index
    for reference, trial in trials:
        yield reference, trial, None


def _placement_access_probe(
    baseline: PackageAccessResult, reference: str, trial: PhysicalBoard,
    fanout_options: FanoutOptions, placement_options: PlacementPlannerOptions,
    plane_options: PlaneStitchOptions | None, boundary_options: BoundaryAccessOptions,
    index: int,
) -> PackagePlacementProbe:
    from .drc import run_physical_drc
    from .hard_macros import materialize_hard_macros
    owner = materialize_hard_macros(trial)
    fanout = route_fanout(owner, fanout_options)
    plane = stitch_zone_pads(fanout.board, plane_options) if plane_options else None
    board = plane.board if plane else fanout.board
    boundary = analyze_boundary_access(board, fanout, boundary_options)
    ordinary_pending = frozenset(fanout.pending_pads)
    plane_pending = frozenset(plane.pending_pads if plane else ())
    boundary_pending = frozenset(boundary.pending_pads)
    pending = ordinary_pending | plane_pending | boundary_pending
    required = {item.pad for item in baseline.fanout.pin_analysis}
    tested = {item.pad for item in fanout.pin_analysis}
    lost = (required - tested) | (baseline.fanout.escaped_pads - fanout.escaped_pads)
    if baseline.boundary is not None:
        lost |= {p.pad for p in baseline.boundary.ports} - {p.pad for p in boundary.ports}
    hard = sum(f.severity.value == "error" and f.code not in
               {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"} for f in run_physical_drc(board).findings)
    quality = placement_metrics(trial, {p.reference: p for p in trial.placements},
                                placement_options).quality_vector
    rank = (hard, len(lost), len(pending - baseline.pending_pads), len(pending), *quality)
    return PackagePlacementProbe(index, reference, _placement_fingerprint(trial),
        tuple(sorted(ordinary_pending)), tuple(sorted(plane_pending)),
        tuple(sorted(boundary_pending)), tuple(sorted(lost)), hard, quality, rank)


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

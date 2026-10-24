"""Fresh, net-atomic reuse of unambiguous generated single critical routes.

Preparation binds a donor's exact source and ordinary prefix. Acceptance still
checks current committed copper; a failed sibling group is never reuse authority.
Pairs, source/macro nets, required arrays and shared returns keep their own paths.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field, replace
from typing import TYPE_CHECKING, Iterable

from .physical import PhysicalBoard, RouteKind, TrackSegment, Via
from .routing import GlobalRoutingResult, _placement_fingerprint
from .routing_clearance import _geometry_identity

if TYPE_CHECKING:
    from .critical import CriticalNetResult, CriticalRoutingResult
    from .fanout import FanoutResult


@dataclass(frozen=True, slots=True)
class SingleCriticalReuseCandidate:
    result: CriticalNetResult
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...]
    source: PhysicalBoard = field(repr=False, compare=False)
    geometry_digest: str = ""

    @property
    def net(self) -> str:
        return self.result.nets[0]


@dataclass(frozen=True, slots=True)
class CriticalReusePreparation:
    candidates: tuple[SingleCriticalReuseCandidate, ...] = ()
    rejected: tuple[tuple[tuple[str, ...], str], ...] = ()
    rejection_reason: str = ""


_ROUTING_METADATA = frozenset({
    "critical_routing", "detailed_routing", "global_routing_fingerprint", "fabrication_ready",
})
_SINGLE_KINDS = frozenset({RouteKind.CRITICAL, RouteKind.CLOCK, RouteKind.RF_FEED, RouteKind.POWER})


def _source_matches(source: PhysicalBoard, donor: PhysicalBoard) -> bool:
    if {k: v for k, v in donor.metadata.items() if k not in _ROUTING_METADATA} != {
            k: v for k, v in source.metadata.items() if k not in _ROUTING_METADATA}:
        return False
    return replace(donor, tracks=source.tracks, vias=source.vias, metadata=source.metadata) == source


def _ordinary_prefix(source: PhysicalBoard, reserved: FanoutResult | None):
    if reserved is None:
        return source.tracks, source.vias, ""
    if (reserved.board.tracks != (*source.tracks, *reserved.created_tracks)
            or reserved.board.vias != (*source.vias, *reserved.created_vias)
            or replace(reserved.board, tracks=source.tracks, vias=source.vias) != source
            or reserved.added_track_count != len(reserved.created_tracks)
            or reserved.added_via_count != len(reserved.created_vias)
            or Counter(reserved.decoupling_tracks) - Counter(reserved.created_tracks)
            or Counter(reserved.decoupling_vias) - Counter(reserved.created_vias)):
        return (), (), "stale ordinary ownership"
    # Match the critical router's fresh read-only semantic owner boundary.
    from .bypass_reservations import verified_bypass_reservations
    bypass_tracks, bypass_vias, reason = verified_bypass_reservations(source, reserved)
    if reason is not None:
        return (), (), reason
    from .zone_geometry import distribution_zone_nets
    protected = {r.net for r in source.net_routing_rules if r.kind is not RouteKind.GENERAL}
    regional = {z.net for z in source.zones if z.reserve_routing}
    protected.update(distribution_zone_nets(source) - regional)
    feeds = Counter((*bypass_tracks, *bypass_vias))
    for item in (*reserved.created_tracks, *reserved.created_vias):
        if item.net not in protected:
            continue
        if feeds[item]:
            feeds[item] -= 1
        else:
            return (), (), "ordinary prefix contains nonordinary owner"
    return reserved.board.tracks, reserved.board.vias, ""


def prepare_single_critical_reuse(
    board: PhysicalBoard, global_route: GlobalRoutingResult, donor: CriticalRoutingResult,
    *, reserved_accesses: FanoutResult | None = None,
) -> CriticalReusePreparation:
    """Extract fresh-valid generated candidates; never trust a batch success flag.

    The source must be the same materialized source used by the donor, including
    every footprint, rule and required-array occurrence. A missing/unknown plane
    prefix is unsupported. Rejected candidates are not returned as copper.
    """
    from .critical import _fingerprint, _track_length
    if (global_route.placement_fingerprint != _placement_fingerprint(board)
            or donor.global_routing_fingerprint != global_route.routing_fingerprint):
        return CriticalReusePreparation(rejection_reason="stale global routing fingerprint")
    if not _source_matches(board, donor.board):
        return CriticalReusePreparation(rejection_reason="stale source board")
    if (donor.locked_tracks != donor.board.tracks or donor.locked_vias != donor.board.vias
            or donor.routing_fingerprint != _fingerprint(
                global_route.routing_fingerprint, donor.locked_tracks, donor.locked_vias, donor.nets)):
        return CriticalReusePreparation(rejection_reason="stale donor copper fingerprint")
    if donor.plane_reservation_track_count or donor.plane_reservation_via_count:
        return CriticalReusePreparation(rejection_reason="unsupported plane prefix ownership")
    prefix_tracks, prefix_vias, reason = _ordinary_prefix(board, reserved_accesses)
    if reason:
        return CriticalReusePreparation(rejection_reason=reason)
    track_count = len(reserved_accesses.created_tracks) if reserved_accesses else 0
    via_count = len(reserved_accesses.created_vias) if reserved_accesses else 0
    if (donor.reserved_track_count != track_count or donor.reserved_via_count != via_count
            or donor.locked_tracks[:len(prefix_tracks)] != prefix_tracks
            or donor.locked_vias[:len(prefix_vias)] != prefix_vias):
        return CriticalReusePreparation(rejection_reason="donor ordinary prefix mismatch")
    suffix_tracks = donor.locked_tracks[len(prefix_tracks):]
    suffix_vias = donor.locked_vias[len(prefix_vias):]
    if Counter(prefix_tracks) & Counter(suffix_tracks) or Counter(prefix_vias) & Counter(suffix_vias):
        return CriticalReusePreparation(rejection_reason="generated suffix aliases reserved ownership")
    named = Counter(name for item in donor.nets for name in item.nets)
    prefix_nets = {item.net for item in (*prefix_tracks, *prefix_vias)}
    rules = {r.net: r for r in board.net_routing_rules}
    candidates, rejected = [], []
    for item in donor.nets:
        reason = ""
        if len(item.nets) != 1 or not item.connected or item.diagnostics or item.candidate_rejected:
            continue
        name = item.nets[0]
        rule = rules.get(name)
        tracks = tuple(t for t in suffix_tracks if t.net == name)
        vias = tuple(v for v in suffix_vias if v.net == name)
        if rule is None or rule.kind not in _SINGLE_KINDS or rule.differential_partner:
            reason = "not a single-ended critical owner"
        elif named[name] != 1 or name in prefix_nets or item.strategy == "immutable_hard_macro":
            reason = "source, macro or ambiguous net ownership"
        elif (item.return_via_count or item.paired_via_transitions
              or item.shared_reference_transition_count or item.coupled_length_nm
              or item.uncoupled_lengths_nm or item.local_preparation is not None):
            reason = "paired or shared-return ownership"
        elif any(v.finish != "standard" or v.technology is not None for v in vias):
            reason = "unsupported generated via process ownership"
        elif (not tracks or item.track_count != len(tracks) or item.via_count != len(vias)
              or item.lengths_nm != (_track_length(tracks),)
              or len(set(tracks)) != len(tracks) or len(set(vias)) != len(vias)):
            reason = "generated contribution count or measurement mismatch"
        if reason:
            rejected.append((item.nets, reason))
            continue
        candidate = SingleCriticalReuseCandidate(item, tracks, vias, board,
                                                 _geometry_identity((item.nets, tracks, vias)))
        result, fresh_tracks, fresh_vias = validate_single_critical_reuse(
            board, candidate, prefix_tracks, prefix_vias)
        if not result.connected:
            rejected.append((item.nets, "; ".join(result.diagnostics)))
            continue
        candidates.append(replace(candidate, result=result, tracks=fresh_tracks, vias=fresh_vias))
    return CriticalReusePreparation(tuple(candidates), tuple(rejected))


def validate_single_critical_reuse(
    board: PhysicalBoard, candidate: SingleCriticalReuseCandidate,
    committed_tracks: Iterable[TrackSegment], committed_vias: Iterable[Via],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Measure and check this generated contribution against the current prefix.

    Call at the normal single-net job's turn. Current earlier critical copper may
    reject it even when preparation succeeded. Inputs and donor remain unchanged.
    """
    from .critical import (CriticalNetResult, _budget_diagnostics, _budget_warnings, _external_assumptions,
                           _external_evidence, _reject_reserved_plane_tracks,
                           _track_length, _validate_candidate)
    tracks, vias = tuple(committed_tracks), tuple(committed_vias)
    def reject(reason):
        return CriticalNetResult(candidate.result.nets, False, 0, 0, (), 0,
            diagnostics=(reason,), strategy="revalidated_single_net", candidate_rejected=True), (), ()
    if candidate.source != board:
        return reject("stale reuse source board")
    if (candidate.geometry_digest != _geometry_identity(
            (candidate.result.nets, candidate.tracks, candidate.vias))
            or len(candidate.result.nets) != 1):
        return reject("stale reuse contribution geometry")
    name = candidate.net
    rule = next((r for r in board.net_routing_rules if r.net == name), None)
    if rule is None or rule.kind not in _SINGLE_KINDS or rule.differential_partner:
        return reject("reuse requires a single-ended critical rule")
    if (tracks[:len(board.tracks)] != board.tracks or vias[:len(board.vias)] != board.vias
            or any(t.net == name for t in (*tracks, *vias))):
        return reject("current prefix lost source ownership or already owns candidate net")
    result = CriticalNetResult((name,), True, len(candidate.tracks), len(candidate.vias),
        (_track_length(candidate.tracks),), 0,
        diagnostics=_budget_diagnostics(rule, candidate.tracks, candidate.vias),
        warnings=_budget_warnings(rule, candidate.tracks),
        assumptions=_external_assumptions(rule), evidence_digests=_external_evidence(rule),
        strategy="revalidated_single_net")
    result, proposed_tracks, proposed_vias = _reject_reserved_plane_tracks(
        board, result, candidate.tracks, candidate.vias, {r.net: r for r in board.net_routing_rules})
    if not result.connected or result.diagnostics:
        return replace(result, connected=False, track_count=0, via_count=0,
                       candidate_rejected=True), (), ()
    if result.warnings:
        # Donor copper over its current length budget is not reused: a fresh
        # route may still meet it, and keeps the over-length fallback if not.
        return replace(result, connected=False, track_count=0, via_count=0, diagnostics=result.warnings,
                       warnings=(), candidate_rejected=True), (), ()
    return _validate_candidate(board, result, proposed_tracks, proposed_vias, list(tracks), list(vias))

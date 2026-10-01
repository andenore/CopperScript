"""Profile-driven exact routing for critical nets.

This stage consumes global guides and creates locked copper before the general
detailed router.  Electrical impedance and RF performance remain external
qualification concerns even when their geometric proxies are satisfied.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum
from hashlib import sha256
import json
from math import hypot
from types import MappingProxyType
from typing import Callable

from .physical import (
    CopperLayer,
    NetRoutingRule,
    PhysicalBoard,
    Point,
    RouteKind,
    TrackSegment,
    Via,
)
from .routing import GlobalNetRoute, GlobalRoutingResult, GlobalViaProposal
from .geometry import segment_distance_squared
from .routing_vias import physical_via_span
from .routing_layers import routing_layers
from .drc import DrcSeverity, run_physical_drc
from .pair_search import PairSearchCandidate, PairSearchStats, paired_candidates
from .pair_refine import PairRefinementStats, paired_shortcuts
from .local_critical import local_surface_candidates


class CriticalRoutingStatus(str, Enum):
    SUCCESS = "success"
    WARNING = "warning"
    FAILED = "failed"


@dataclass(frozen=True, slots=True)
class CriticalNetResult:
    nets: tuple[str, ...]
    connected: bool
    track_count: int
    via_count: int
    lengths_nm: tuple[int, ...]
    skew_nm: int
    diagnostics: tuple[str, ...] = ()
    assumptions: tuple[str, ...] = ()
    coupled_length_nm: int = 0
    uncoupled_lengths_nm: tuple[int, ...] = ()
    paired_via_transitions: int = 0
    return_via_count: int = 0
    tuned_length_nm: int = 0
    evidence_digests: tuple[str, ...] = ()
    strategy: str = "global_guide"
    candidate_rejected: bool = False
    search_states: int = 0
    candidate_attempts: int = 0
    pair_searches: int = 0
    local_candidate_attempts: int = 0
    guide_length_nm: int | None = None
    pair_refinement_attempts: int = 0
    pair_refinement_candidates: int = 0


@dataclass(frozen=True, slots=True)
class CriticalRoutingResult:
    status: CriticalRoutingStatus
    board: PhysicalBoard
    nets: tuple[CriticalNetResult, ...]
    locked_tracks: tuple[TrackSegment, ...]
    locked_vias: tuple[Via, ...]
    global_routing_fingerprint: str
    routing_fingerprint: str

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-critical-route/v0.1",
            "status": self.status.value,
            "global_routing_fingerprint": self.global_routing_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "nets": [
                {
                    "nets": list(item.nets),
                    "connected": item.connected,
                    "track_count": item.track_count,
                    "via_count": item.via_count,
                    "lengths_nm": list(item.lengths_nm),
                    "skew_nm": item.skew_nm,
                    "diagnostics": list(item.diagnostics),
                    "assumptions": list(item.assumptions),
                    "coupled_length_nm": item.coupled_length_nm,
                    "uncoupled_lengths_nm": list(item.uncoupled_lengths_nm),
                    "paired_via_transitions": item.paired_via_transitions,
                    "return_via_count": item.return_via_count,
                    "tuned_length_nm": item.tuned_length_nm,
                    "evidence_digests": list(item.evidence_digests),
                    "strategy": item.strategy,
                    "candidate_rejected": item.candidate_rejected,
                    "search_states": item.search_states,
                    "candidate_attempts": item.candidate_attempts,
                    "pair_searches": item.pair_searches,
                    "local_candidate_attempts": item.local_candidate_attempts,
                    "guide_length_nm": item.guide_length_nm,
                    "pair_refinement_attempts": item.pair_refinement_attempts,
                    "pair_refinement_candidates": item.pair_refinement_candidates,
                }
                for item in self.nets
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


def route_critical_nets(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    *, on_progress: Callable[[str, tuple[str, ...], CriticalNetResult | None], None] | None = None,
) -> CriticalRoutingResult:
    """Materialize exact locked copper for all non-general routing rules."""

    if board.tracks or board.vias:
        raise ValueError("critical routing requires a board without existing copper")
    routes = {item.net: item for item in global_route.routes}
    rules = {item.net: item for item in board.net_routing_rules}
    processed: set[str] = set()
    coupled_pairs: set[frozenset[str]] = set()
    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    results: list[CriticalNetResult] = []
    for rule in sorted(
        (item for item in board.net_routing_rules if item.kind is not RouteKind.GENERAL),
        key=lambda item: (-item.priority, item.kind.value, item.net),
    ):
        if rule.net in processed:
            continue
        group = (tuple(sorted((rule.net, rule.differential_partner)))
                 if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS} else (rule.net,))
        if on_progress:
            on_progress("started", group, None)
        if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}:
            partner_name = rule.differential_partner
            assert partner_name is not None
            partner_rule = rules.get(partner_name)
            if (
                partner_rule is None
                or partner_rule.differential_partner != rule.net
                or partner_rule.kind is not rule.kind
            ):
                result = CriticalNetResult(
                    tuple(sorted((rule.net, partner_name))),
                    False,
                    0,
                    0,
                    (),
                    0,
                    ("differential/CAN routing rules must be symmetric",),
                )
                results.append(result)
                processed.update((rule.net, partner_name))
                if on_progress:
                    on_progress("finished", group, result)
                continue
            result, pair_tracks, pair_vias = _route_pair(
                board, rule, partner_rule, routes
            )
            result, pair_tracks, pair_vias = _reject_reserved_plane_tracks(
                board, result, pair_tracks, pair_vias, rules,
            )
            result, pair_tracks, pair_vias = _validate_candidate(
                board, result, pair_tracks, pair_vias, tracks, vias,
            )
            if not result.connected and all(
                routes.get(name) is not None and routes[name].connected
                for name in (rule.net, partner_name)
            ):
                first, second = sorted((rule, partner_rule), key=lambda item: item.net)
                search_board = replace(board, tracks=tuple(tracks), vias=tuple(vias))
                stats = PairSearchStats()
                for pitch_nm in (1_000_000, 500_000, 250_000):
                    for candidate in paired_candidates(
                        search_board, first, second, routes[first.net], routes[second.net],
                        stats=stats, pitch_nm=pitch_nm,
                    ):
                        attempt, proposed_tracks, proposed_vias = _route_pair(
                            board, first, second, routes,
                            exact_tracks=(candidate.first, candidate.second),
                        )
                        attempt, proposed_tracks, proposed_vias = _validate_candidate(
                            board, attempt, proposed_tracks, proposed_vias, tracks, vias,
                        )
                        if attempt.connected:
                            result, pair_tracks, pair_vias = attempt, proposed_tracks, proposed_vias
                            result, pair_tracks, pair_vias = _improve_pair_spine(
                                board, search_board, first, second, routes, candidate,
                                result, pair_tracks, pair_vias, tracks, vias,
                            )
                            break
                    if result.connected:
                        break
                result = replace(result, search_states=stats.expanded_states,
                                 candidate_attempts=stats.candidates, pair_searches=stats.searches)
                if not result.connected:
                    result = replace(result, diagnostics=(*result.diagnostics,
                        f"joint pair search: {stats.searches} searches, {stats.expanded_states} states, "
                        f"{stats.candidates} candidates; none accepted"))
            processed.update((rule.net, partner_name))
            coupled_pairs.add(frozenset((rule.net, partner_name)))
            results.append(result)
            tracks.extend(pair_tracks)
            vias.extend(pair_vias)
        else:
            result, net_tracks, net_vias = _route_single(
                board, rule, routes.get(rule.net)
            )
            result, net_tracks, net_vias = _reject_reserved_plane_tracks(
                board, result, net_tracks, net_vias, rules,
            )
            result, net_tracks, net_vias = _validate_candidate(
                board, result, net_tracks, net_vias, tracks, vias,
            )
            result, net_tracks, net_vias = _improve_single_surface(
                board, rule, routes.get(rule.net), result, net_tracks, net_vias, tracks, vias,
            )
            # Single-ended critical nets can reuse the exact octilinear maze
            # engine. Pairs must NEVER be repaired as independent ordinary nets.
            if (not result.connected and routes.get(rule.net) is not None
                    and routes[rule.net].connected
                    and (rule.kind not in {RouteKind.RF_FEED, RouteKind.CLOCK}
                         or len(routes[rule.net].accesses) == 2)):
                repaired, net_tracks, net_vias = _route_single_exact(
                    board, rule, global_route, tracks, vias,
                )
                result = replace(repaired, local_candidate_attempts=result.local_candidate_attempts,
                                 guide_length_nm=result.guide_length_nm)
            processed.add(rule.net)
            results.append(result)
            tracks.extend(net_tracks)
            vias.extend(net_vias)
        if on_progress:
            on_progress("finished", group, result)

    conflict_diagnostics = _intersection_diagnostics(tuple(tracks), coupled_pairs)
    if conflict_diagnostics:
        results.append(
            CriticalNetResult(
                ("<critical-batch>",),
                False,
                0,
                0,
                (),
                0,
                conflict_diagnostics,
            )
        )
    failed = any(not item.connected or item.diagnostics for item in results)
    warnings = any(item.assumptions for item in results)
    status = (
        CriticalRoutingStatus.FAILED
        if failed
        else CriticalRoutingStatus.WARNING
        if warnings
        else CriticalRoutingStatus.SUCCESS
    )
    metadata = dict(board.metadata)
    metadata.update(
        {
            "critical_routing": "complete" if not failed else "failed",
            "detailed_routing": "partial",
            "global_routing_fingerprint": global_route.routing_fingerprint,
            "fabrication_ready": "false",
        }
    )
    routed_board = replace(
        board,
        tracks=tuple(tracks),
        vias=tuple(vias),
        metadata=MappingProxyType(metadata),
    )
    fingerprint = _fingerprint(global_route.routing_fingerprint, tracks, vias, results)
    return CriticalRoutingResult(
        status,
        routed_board,
        tuple(results),
        tuple(tracks),
        tuple(vias),
        global_route.routing_fingerprint,
        fingerprint,
    )


def _improve_pair_spine(
    board: PhysicalBoard, search_board: PhysicalBoard,
    first: NetRoutingRule, second: NetRoutingRule, routes: dict[str, GlobalNetRoute],
    candidate: PairSearchCandidate, incumbent: CriticalNetResult,
    incumbent_tracks: tuple[TrackSegment, ...], incumbent_vias: tuple[Via, ...],
    committed_tracks: list[TrackSegment], committed_vias: list[Via],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """A rejected shortcut retains the complete accepted pair and its metrics."""
    stats = PairRefinementStats()
    for lanes in paired_shortcuts(search_board, candidate, first, second, stats=stats):
        # Do not silently resize, relabel or move a pair to a forbidden layer,
        # even if a faulty proposer passes native minimum-geometry checks.
        if any(t.net != rule.net or t.width_nm != (rule.width_nm or board.rules.default_track_width_nm)
               or t.layer not in routing_layers(board, rule.net, rule)
               for rule, lane in zip((first, second), lanes) for t in lane):
            continue
        proposed, tracks, vias = _route_pair(board, first, second, routes, exact_tracks=lanes)
        proposed, tracks, vias = _reject_reserved_plane_tracks(
            board, proposed, tracks, vias, {first.net: first, second.net: second},
        )
        proposed, tracks, vias = _validate_candidate(board, proposed, tracks, vias, committed_tracks, committed_vias)
        if (proposed.connected and all(a <= b for a, b in zip(proposed.lengths_nm, incumbent.lengths_nm))
                and (sum(proposed.lengths_nm), len(tracks)) < (sum(incumbent.lengths_nm), len(incumbent_tracks))):
            incumbent = replace(proposed, strategy="joint_pair_refined")
            incumbent_tracks, incumbent_vias = tracks, vias
    return replace(incumbent, pair_refinement_attempts=stats.attempts,
                   pair_refinement_candidates=stats.candidates), incumbent_tracks, incumbent_vias


def _validate_candidate(
    board: PhysicalBoard,
    result: CriticalNetResult,
    tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...],
    committed_tracks: list[TrackSegment],
    committed_vias: list[Via],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Accept a critical group atomically, using authoritative native geometry.

    A coarse guide is not clearance evidence. Check both members, return vias,
    every physical land, and previously accepted critical groups together.
    Unrelated open nets and route-completeness are expected at this stage; no
    other hard finding is waived. Rejected copper never becomes an obstacle.
    """
    if not tracks and not vias:
        return result, (), ()
    candidate = replace(
        board, tracks=(*committed_tracks, *tracks),
        vias=(*committed_vias, *vias),
    )
    diagnostics = list(result.diagnostics)
    for finding in run_physical_drc(candidate).findings:
        if finding.severity is not DrcSeverity.ERROR:
            continue
        if finding.code == "DRC-ROUTE-INCOMPLETE":
            continue
        if finding.code == "DRC-OPEN-NET" and not set(finding.nets).intersection(result.nets):
            continue
        diagnostics.append(f"{finding.code}: {finding.message}")
    diagnostics = list(dict.fromkeys(diagnostics))
    if not result.connected or diagnostics:
        return replace(
            result, connected=False, track_count=0, via_count=0,
            return_via_count=0, paired_via_transitions=0,
            diagnostics=tuple(diagnostics), candidate_rejected=True,
        ), (), ()
    return result, tracks, vias


def _reject_reserved_plane_tracks(
    board: PhysicalBoard,
    result: CriticalNetResult,
    tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...],
    rules: dict[str, NetRoutingRule],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    forbidden = tuple(
        track for track in tracks
        if track.layer not in routing_layers(board, track.net, rules.get(track.net))
    )
    if not forbidden:
        return result, tracks, vias
    layers = ", ".join(sorted({track.layer.value for track in forbidden}))
    return (
        replace(
            result, connected=False, track_count=0, via_count=0,
            diagnostics=(*result.diagnostics, f"critical route uses reserved plane layer {layers}"),
        ),
        (),
        (),
    )


def _guide_via(
    board: PhysicalBoard, net: str, proposal: GlobalViaProposal, position: Point
) -> Via:
    span = physical_via_span(board, proposal.from_layer, proposal.to_layer)
    if span is None:
        raise ValueError(
            f"no legal physical via for {proposal.from_layer.value} to "
            f"{proposal.to_layer.value} on {net}"
        )
    return Via(
        net, position, board.rules.default_via_size_nm,
        board.rules.default_via_drill_nm, *span,
    )


def _route_single_exact(
    board: PhysicalBoard, rule: NetRoutingRule, global_route: GlobalRoutingResult,
    committed_tracks: list[TrackSegment], committed_vias: list[Via],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Bounded single-net repair against immutable earlier critical copper."""
    from .detailed import DetailedRouterOptions, route_detailed

    search_board = replace(
        board, tracks=tuple(committed_tracks), vias=tuple(committed_vias),
        net_routing_rules=tuple(
            replace(item, kind=RouteKind.GENERAL) if item.net == rule.net else item
            for item in board.net_routing_rules
        ),
    )
    detailed = route_detailed(
        search_board, global_route,
        DetailedRouterOptions(
            maximum_passes=2, maximum_search_states=20_000,
            progressive_guides=True, constrained_pins_first=True,
        ), only_nets=frozenset({rule.net}),
    )
    tracks = detailed.board.tracks[len(committed_tracks):]
    vias = detailed.board.vias[len(committed_vias):]
    net = next((item for item in detailed.nets if item.net == rule.net), None)
    result = CriticalNetResult(
        (rule.net,), net is not None and net.connected,
        len(tracks), len(vias), (_track_length(tracks),), 0,
        net.diagnostics if net is not None else ("no exact critical-net attempt",),
        _external_assumptions(rule), evidence_digests=_external_evidence(rule),
        strategy="exact_single_net",
    )
    return _validate_candidate(board, result, tracks, vias,
                               committed_tracks, committed_vias)


def _improve_single_surface(
    board: PhysicalBoard, rule: NetRoutingRule, guide: GlobalNetRoute | None,
    incumbent: CriticalNetResult, incumbent_tracks: tuple[TrackSegment, ...],
    incumbent_vias: tuple[Via, ...], committed_tracks: list[TrackSegment],
    committed_vias: list[Via],
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    # Global transitions are handled by their existing owner. This initial
    # improvement only compares surface-guide candidates, never paired nets.
    if guide is None or not guide.connected or guide.vias or any(access.via for access in guide.accesses):
        return incumbent, incumbent_tracks, incumbent_vias
    guide_length = incumbent.lengths_nm[0] if incumbent.lengths_nm else None
    search_board = replace(board, tracks=tuple(committed_tracks), vias=tuple(committed_vias))
    best, best_tracks, best_vias = incumbent, incumbent_tracks, incumbent_vias
    attempts = 0
    for tracks in local_surface_candidates(search_board, rule):
        attempts += 1
        length = _track_length(tracks)
        if best.connected and not best_vias and length >= best.lengths_nm[0]:
            continue
        candidate = CriticalNetResult(
            (rule.net,), True, len(tracks), 0, (length,), 0,
            diagnostics=_budget_diagnostics(rule, tracks, ()),
            assumptions=_external_assumptions(rule), evidence_digests=_external_evidence(rule),
            strategy="local_surface_tree",
        )
        candidate, tracks, vias = _validate_candidate(
            board, candidate, tracks, (), committed_tracks, committed_vias,
        )
        if candidate.connected and (not best.connected or length < best.lengths_nm[0]):
            best, best_tracks, best_vias = candidate, tracks, vias
    return replace(best, local_candidate_attempts=attempts, guide_length_nm=guide_length), best_tracks, best_vias


def _route_single(
    board: PhysicalBoard,
    rule: NetRoutingRule,
    guide: GlobalNetRoute | None,
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    if guide is None or not guide.connected:
        return (
            CriticalNetResult(
                (rule.net,), False, 0, 0, (), 0, ("missing connected global guide",)
            ),
            (),
            (),
        )
    if rule.kind in {RouteKind.CLOCK, RouteKind.RF_FEED} and len(guide.accesses) != 2:
        return (
            CriticalNetResult(
                (rule.net,),
                False,
                0,
                0,
                (),
                0,
                (f"{rule.kind.value} v0.1 requires point-to-point topology",),
            ),
            (),
            (),
        )
    width = rule.width_nm or board.rules.default_track_width_nm
    tracks = [
        TrackSegment(rule.net, item.start, item.end, width, item.layer)
        for item in guide.segments
    ]
    tracks.extend(_pin_stubs(rule.net, guide, width))
    vias = tuple(dict.fromkeys((
        *(_guide_via(board, rule.net, item, item.position) for item in guide.vias),
        *(access.via for access in guide.accesses if access.via is not None),
    )))
    diagnostics = _budget_diagnostics(rule, tuple(tracks), vias)
    assumptions = _external_assumptions(rule)
    evidence = _external_evidence(rule)
    length = _track_length(tuple(tracks))
    return (
        CriticalNetResult(
            (rule.net,),
            not diagnostics,
            len(tracks),
            len(vias),
            (length,),
            0,
            diagnostics,
            assumptions,
            0,
            (),
            0,
            0,
            0,
            evidence,
        ),
        tuple(tracks),
        vias,
    )


def _route_pair(
    board: PhysicalBoard,
    first_rule: NetRoutingRule,
    second_rule: NetRoutingRule,
    routes: dict[str, GlobalNetRoute],
    *, exact_tracks: tuple[tuple[TrackSegment, ...], tuple[TrackSegment, ...]] | None = None,
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    first_name, second_name = sorted((first_rule.net, second_rule.net))
    first = first_rule if first_rule.net == first_name else second_rule
    second = second_rule if first is first_rule else first_rule
    guide = routes.get(first.net)
    partner_guide = routes.get(second.net)
    if guide is None or partner_guide is None or not guide.connected or not partner_guide.connected:
        return (
            CriticalNetResult(
                (first.net, second.net),
                False,
                0,
                0,
                (),
                0,
                ("both pair members require connected global guides",),
            ),
            (),
            (),
        )
    if len(guide.accesses) != 2 or len(partner_guide.accesses) != 2:
        return (
            CriticalNetResult(
                (first.net, second.net),
                False,
                0,
                0,
                (),
                0,
                ("coupled pair routing v0.1 requires two terminals per member",),
            ),
            (),
            (),
        )
    first_width = first.width_nm or board.rules.default_track_width_nm
    second_width = second.width_nm or board.rules.default_track_width_nm
    gap = first.pair_gap_nm or second.pair_gap_nm
    if first_width != second_width or gap is None or second.pair_gap_nm != gap:
        return (
            CriticalNetResult(
                (first.net, second.net),
                False,
                0,
                0,
                (),
                0,
                ("pair members require identical width and gap profiles",),
            ),
            (),
            (),
        )
    offset = (first_width + gap) // 2
    aligned = _aligned_pair_paths(guide, partner_guide, offset) if exact_tracks is None else ([], [])
    if aligned is None:
        positive, negative, junctions = _offset_guides(guide, offset)
    else:
        positive, negative = aligned
        junctions = {}
    first_tracks = [
        TrackSegment(first.net, start, end, first_width, layer)
        for layer, start, end in positive
        if start != end
    ]
    second_tracks = [
        TrackSegment(second.net, start, end, second_width, layer)
        for layer, start, end in negative
        if start != end
    ]
    _join_offset_junctions(first.net, first_width, 0, junctions, first_tracks)
    _join_offset_junctions(second.net, second_width, 1, junctions, second_tracks)
    if exact_tracks is not None:
        first_tracks, second_tracks = list(exact_tracks[0]), list(exact_tracks[1])
    if aligned is None:
        first_tracks.extend(
            _pair_pin_stubs(first.net, guide, first_width, positive, 1, first_width + gap)
        )
        second_tracks.extend(
            _pair_pin_stubs(
                second.net, partner_guide, second_width, negative, 0,
                first_width + gap,
            )
        )
    max_skew = min(
        value
        for value in (first.max_skew_nm, second.max_skew_nm)
        if value is not None
    ) if first.max_skew_nm is not None or second.max_skew_nm is not None else None
    tuned_length = 0
    if max_skew is not None:
        tuned_length = _tune_pair(first_tracks, second_tracks, max_skew,
                                  min(value for value in (first.tuning_amplitude_limit_nm,
                                                          second.tuning_amplitude_limit_nm)
                                      if value is not None)
                                  if first.tuning_amplitude_limit_nm is not None or second.tuning_amplitude_limit_nm is not None
                                  else 0)
    pair_vias: list[Via] = []
    return_vias: list[Via] = []
    for item in (() if exact_tracks is not None else guide.vias):
        for net, sign in ((first.net, 1), (second.net, -1)):
            pair_vias.append(_guide_via(
                board, net, item,
                Point(item.position.x_nm + sign * offset, item.position.y_nm),
            ))
        if first.require_return_vias or second.require_return_vias:
            return_net = first.return_via_net or second.return_via_net
            assert return_net is not None
            return_vias.append(_guide_via(board, return_net, item, item.position))
    first_length = _track_length(tuple(first_tracks))
    second_length = _track_length(tuple(second_tracks))
    skew = abs(first_length - second_length)
    center_spacing = first_width + gap
    coupled_length = _coupled_length(first_tracks, second_tracks, center_spacing)
    uncoupled = (max(0, first_length - coupled_length), max(0, second_length - coupled_length))
    first_pair_vias = tuple(v for v in pair_vias if v.net == first.net)
    second_pair_vias = tuple(v for v in pair_vias if v.net == second.net)
    paired_transitions = _paired_via_transitions(first_pair_vias, second_pair_vias, center_spacing)
    diagnostics = list(
        _budget_diagnostics(first, tuple(first_tracks), tuple(v for v in pair_vias if v.net == first.net))
    )
    diagnostics.extend(
        _budget_diagnostics(second, tuple(second_tracks), tuple(v for v in pair_vias if v.net == second.net))
    )
    if max_skew is not None and skew > max_skew:
        diagnostics.append(f"pair skew {skew} nm exceeds {max_skew} nm")
    uncoupled_limit = min(
        value
        for value in (first.maximum_uncoupled_length_nm, second.maximum_uncoupled_length_nm)
        if value is not None
    ) if first.maximum_uncoupled_length_nm is not None or second.maximum_uncoupled_length_nm is not None else None
    if uncoupled_limit is not None and max(uncoupled) > uncoupled_limit:
        diagnostics.append(
            f"pair uncoupled length {max(uncoupled)} nm exceeds {uncoupled_limit} nm"
        )
    if len(first_pair_vias) != len(second_pair_vias) or paired_transitions != len(first_pair_vias):
        diagnostics.append("differential via transitions are not geometrically paired")
    return_limit = min(
        value for value in (first.maximum_return_via_distance_nm,
                            second.maximum_return_via_distance_nm)
        if value is not None
    ) if first.maximum_return_via_distance_nm is not None or second.maximum_return_via_distance_nm is not None else None
    if return_vias and return_limit is not None and offset > return_limit:
        diagnostics.append(
            f"return via distance {offset} nm exceeds {return_limit} nm"
        )
    assumptions = tuple(
        dict.fromkeys(
            (
                *_external_assumptions(first),
                *_external_assumptions(second),
                "coupled pin fanout remains subject to authoritative physical DRC",
            )
        )
    )
    evidence = tuple(dict.fromkeys((*_external_evidence(first), *_external_evidence(second))))
    tracks = tuple((*first_tracks, *second_tracks))
    return (
        CriticalNetResult(
            (first.net, second.net),
            not diagnostics,
            len(tracks),
            len(pair_vias) + len(return_vias),
            (first_length, second_length),
            skew,
            tuple(diagnostics),
            assumptions,
            coupled_length,
            uncoupled,
            paired_transitions,
            len(return_vias),
            tuned_length,
            evidence,
            strategy=("joint_pair_search" if exact_tracks is not None else
                      "aligned_pair" if aligned is not None else "global_guide"),
        ),
        tracks,
        tuple((*pair_vias, *return_vias)),
    )


def _aligned_pair_paths(
    first: GlobalNetRoute, second: GlobalNetRoute, offset: int,
) -> tuple[list[tuple[CopperLayer, Point, Point]], list[tuple[CopperLayer, Point, Point]]] | None:
    """Try a straight coupled channel with symmetric 45-degree pin tapers.

    Unlike offsetting one member's guide, this uses the midpoint of *both*
    physical terminals. Only unambiguous, same-layer, aligned terminals are
    supported here. Clearance and connectivity still gate the whole candidate.
    """
    if first.vias or second.vias:
        return None
    accesses = (*first.accesses, *second.accesses)
    if len({access.layer for access in accesses}) != 1 or any(
        access.via is not None or access.region_only for access in accesses
    ):
        return None
    by_component = {access.pad.component: access for access in second.accesses}
    if len(by_component) != 2 or set(by_component) != {
        access.pad.component for access in first.accesses
    }:
        return None
    pairs = [(access.pad_position, by_component[access.pad.component].pad_position)
             for access in first.accesses]
    for horizontal in (True, False):
        def axial(point: Point) -> int:
            return point.x_nm if horizontal else point.y_nm

        def lateral(point: Point) -> int:
            return point.y_nm if horizontal else point.x_nm

        if any(axial(a) != axial(b) for a, b in pairs):
            continue
        ordered = sorted(pairs, key=lambda pair: axial(pair[0]))
        (a, b), (c, d) = ordered
        if lateral(a) != lateral(c) or lateral(b) != lateral(d):
            continue
        mid = (lateral(a) + lateral(b)) // 2
        sign = 1 if lateral(a) > lateral(b) else -1
        first_lane, second_lane = mid + sign * offset, mid - sign * offset
        taper = max(abs(lateral(a) - first_lane), abs(lateral(b) - second_lane))
        if axial(c) - axial(a) <= 2 * taper:
            continue

        def path(start: Point, end: Point, lane: int) -> list[tuple[CopperLayer, Point, Point]]:
            def point(x: int, y: int) -> Point:
                return Point(x, y) if horizontal else Point(y, x)

            ramp = abs(lateral(start) - lane)
            points = (start, point(axial(start) + ramp, lane),
                      point(axial(end) - ramp, lane), end)
            return [(accesses[0].layer, p, q) for p, q in zip(points, points[1:])
                    if p != q]

        return path(a, c, first_lane), path(b, d, second_lane)
    return None


def _tune_pair(first: list[TrackSegment], second: list[TrackSegment],
               max_skew_nm: int, amplitude_limit_nm: int) -> int:
    first_length, second_length = _track_length(tuple(first)), _track_length(tuple(second))
    excess = abs(first_length - second_length) - max_skew_nm
    if excess <= 0 or amplitude_limit_nm <= 0:
        return 0
    target = first if first_length < second_length else second
    candidates = sorted(
        ((round(hypot(track.end.x_nm - track.start.x_nm,
                      track.end.y_nm - track.start.y_nm)), index, track)
         for index, track in enumerate(target)
         if track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm),
        reverse=True,
    )
    if not candidates:
        return 0
    _, index, track = candidates[0]
    amplitude = min(amplitude_limit_nm, (excess + 1) // 2)
    if amplitude <= 0:
        return 0
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    first_point = Point(track.start.x_nm + dx // 3, track.start.y_nm + dy // 3)
    second_point = Point(track.start.x_nm + 2 * dx // 3, track.start.y_nm + 2 * dy // 3)
    normal = Point(0, amplitude) if dx else Point(amplitude, 0)
    raised_first = Point(first_point.x_nm + normal.x_nm, first_point.y_nm + normal.y_nm)
    raised_second = Point(second_point.x_nm + normal.x_nm, second_point.y_nm + normal.y_nm)
    replacement = (
        TrackSegment(track.net, track.start, first_point, track.width_nm, track.layer),
        TrackSegment(track.net, first_point, raised_first, track.width_nm, track.layer),
        TrackSegment(track.net, raised_first, raised_second, track.width_nm, track.layer),
        TrackSegment(track.net, raised_second, second_point, track.width_nm, track.layer),
        TrackSegment(track.net, second_point, track.end, track.width_nm, track.layer),
    )
    target[index:index + 1] = replacement
    return 2 * amplitude


def _coupled_length(
    first_tracks: list[TrackSegment],
    second_tracks: list[TrackSegment],
    center_spacing_nm: int,
) -> int:
    """Measure unioned parallel overlaps, including unequal mitered segments.

    Integer diagonal offsets/intersections have at most a few nanometres of
    quantization error. A 4 nm geometry tolerance is not an electrical tolerance
    or permission to waive physical DRC. Split edges cannot double-count overlap.
    """
    def measure(tracks: list[TrackSegment], partners: list[TrackSegment]) -> int:
        total = 0
        for first in tracks:
            dx, dy = first.end.x_nm - first.start.x_nm, first.end.y_nm - first.start.y_nm
            length = hypot(dx, dy)
            intervals = []
            for second in partners:
                if first.layer is not second.layer:
                    continue
                sx, sy = second.end.x_nm - second.start.x_nm, second.end.y_nm - second.start.y_nm
                if abs(dx * sy - dy * sx) > 2 * max(length, hypot(sx, sy)):
                    continue
                distances = [abs(dx * (p.y_nm - first.start.y_nm)
                                 - dy * (p.x_nm - first.start.x_nm)) / length
                             for p in (second.start, second.end)]
                if any(abs(distance - center_spacing_nm) > 4 for distance in distances):
                    continue
                projections = sorted((dx * (p.x_nm - first.start.x_nm)
                                      + dy * (p.y_nm - first.start.y_nm)) / length
                                     for p in (second.start, second.end))
                low, high = max(0, projections[0]), min(length, projections[1])
                if low < high:
                    intervals.append((low, high))
            edge_length, end = 0.0, 0.0
            for low, high in sorted(intervals):
                edge_length += max(0, high - max(low, end))
                end = max(end, high)
            total += round(edge_length)
        return total

    return min(measure(first_tracks, second_tracks), measure(second_tracks, first_tracks))


def _paired_via_transitions(
    first: tuple[Via, ...], second: tuple[Via, ...], center_spacing_nm: int
) -> int:
    remaining = list(second)
    count = 0
    for via in first:
        match = next(
            (
                item for item in remaining
                if item.from_layer is via.from_layer
                and item.to_layer is via.to_layer
                and (item.position.x_nm - via.position.x_nm) ** 2
                + (item.position.y_nm - via.position.y_nm) ** 2
                == center_spacing_nm ** 2
            ),
            None,
        )
        if match is not None:
            remaining.remove(match)
            count += 1
    return count


def _offset_guides(
    guide: GlobalNetRoute, offset: int
) -> tuple[
    list[tuple[CopperLayer, Point, Point]],
    list[tuple[CopperLayer, Point, Point]],
    dict[tuple[CopperLayer, Point], tuple[list[Point], list[Point]]],
]:
    positive: list[tuple[CopperLayer, Point, Point]] = []
    negative: list[tuple[CopperLayer, Point, Point]] = []
    junctions: dict[tuple[CopperLayer, Point], tuple[list[Point], list[Point]]] = {}
    for segment in guide.segments:
        dx = segment.end.x_nm - segment.start.x_nm
        dy = segment.end.y_nm - segment.start.y_nm
        if dx and dy:
            normal_x = -offset if dy > 0 else offset
            normal_y = offset if dx > 0 else -offset
        elif dx:
            normal_x = 0
            normal_y = offset if dx > 0 else -offset
        else:
            normal_x = -offset if dy > 0 else offset
            normal_y = 0
        plus_start = Point(segment.start.x_nm + normal_x, segment.start.y_nm + normal_y)
        plus_end = Point(segment.end.x_nm + normal_x, segment.end.y_nm + normal_y)
        minus_start = Point(segment.start.x_nm - normal_x, segment.start.y_nm - normal_y)
        minus_end = Point(segment.end.x_nm - normal_x, segment.end.y_nm - normal_y)
        positive.append((segment.layer, plus_start, plus_end))
        negative.append((segment.layer, minus_start, minus_end))
        for original, plus, minus in (
            (segment.start, plus_start, minus_start),
            (segment.end, plus_end, minus_end),
        ):
            plus_points, minus_points = junctions.setdefault(
                (segment.layer, original), ([], [])
            )
            plus_points.append(plus)
            minus_points.append(minus)
    return positive, negative, junctions


def _join_offset_junctions(
    net: str,
    width: int,
    point_index: int,
    junctions: dict[tuple[CopperLayer, Point], tuple[list[Point], list[Point]]],
    tracks: list[TrackSegment],
) -> None:
    for (layer, _), points in sorted(junctions.items(), key=lambda item: (str(item[0][0]), item[0][1].x_nm, item[0][1].y_nm)):
        selected = points[point_index]
        unique = tuple(dict.fromkeys(selected))
        for point in unique[1:]:
            if point != unique[0]:
                tracks.append(TrackSegment(net, unique[0], point, width, layer))


def _pin_stubs(net: str, guide: GlobalNetRoute, width: int) -> tuple[TrackSegment, ...]:
    result: list[TrackSegment] = []
    for access in guide.accesses:
        if access.tracks:
            result.extend(access.tracks)
        elif access.pad_position != access.access_position:
            result.append(
                TrackSegment(net, access.pad_position, access.access_position, width, access.layer)
            )
    return tuple(result)


def _pair_pin_stubs(
    net: str,
    guide: GlobalNetRoute,
    width: int,
    paths: list[tuple[CopperLayer, Point, Point]],
    lane_index: int,
    lane_pitch: int,
) -> tuple[TrackSegment, ...]:
    endpoints = [
        (layer, point)
        for layer, start, end in paths
        for point in (start, end)
    ]
    result: list[TrackSegment] = []
    xs = [point.x_nm for _, point in endpoints]
    ys = [point.y_nm for _, point in endpoints]
    horizontal = max(xs) - min(xs) >= max(ys) - min(ys)
    for access in guide.accesses:
        layer, point = min(
            endpoints,
            key=lambda item: (
                abs(item[1].x_nm - access.pad_position.x_nm)
                + abs(item[1].y_nm - access.pad_position.y_nm),
                str(item[0]),
                item[1].x_nm,
                item[1].y_nm,
            ),
        )
        if access.pad_position == point:
            continue
        escape = (lane_index + 1) * max(lane_pitch, width)
        if horizontal:
            outside = (
                min(xs) - escape
                if point.x_nm <= (min(xs) + max(xs)) // 2
                else max(xs) + escape
            )
            corners = (
                access.pad_position,
                Point(outside, access.pad_position.y_nm),
                Point(outside, point.y_nm),
                point,
            )
        else:
            outside = (
                min(ys) - escape
                if point.y_nm <= (min(ys) + max(ys)) // 2
                else max(ys) + escape
            )
            corners = (
                access.pad_position,
                Point(access.pad_position.x_nm, outside),
                Point(point.x_nm, outside),
                point,
            )
        for start, end in zip(corners, corners[1:]):
            if start != end:
                result.append(TrackSegment(net, start, end, width, layer))
    return tuple(result)


def _budget_diagnostics(
    rule: NetRoutingRule,
    tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...],
) -> tuple[str, ...]:
    diagnostics: list[str] = []
    length = _track_length(tracks)
    if rule.max_length_nm is not None and length > rule.max_length_nm:
        diagnostics.append(f"route length {length} nm exceeds {rule.max_length_nm} nm")
    if rule.max_vias is not None and len(vias) > rule.max_vias:
        diagnostics.append(f"via count {len(vias)} exceeds {rule.max_vias}")
    return tuple(diagnostics)


def _external_assumptions(rule: NetRoutingRule) -> tuple[str, ...]:
    assumptions: list[str] = []
    if rule.target_impedance_ohms is not None and rule.impedance_evidence_digest is None:
        assumptions.append(
            f"{rule.target_impedance_ohms} ohm impedance requires external stackup/field-solver qualification"
        )
    if rule.kind is RouteKind.RF_FEED:
        assumptions.append("RF feed and antenna performance require simulation and physical validation")
    if rule.kind is RouteKind.POWER:
        assumptions.append("power-route current and thermal capacity require external validation")
    return tuple(assumptions)


def _external_evidence(rule: NetRoutingRule) -> tuple[str, ...]:
    return (rule.impedance_evidence_digest,) if rule.impedance_evidence_digest else ()


def _track_length(tracks: tuple[TrackSegment, ...]) -> int:
    return sum(
        round(hypot(item.end.x_nm - item.start.x_nm, item.end.y_nm - item.start.y_nm))
        for item in tracks
    )


def _intersection_diagnostics(
    tracks: tuple[TrackSegment, ...], coupled_pairs: set[frozenset[str]]
) -> tuple[str, ...]:
    diagnostics: list[str] = []
    for index, first in enumerate(tracks):
        for second in tracks[index + 1 :]:
            if first.net == second.net or first.layer is not second.layer:
                continue
            if frozenset((first.net, second.net)) in coupled_pairs:
                continue
            if _segments_intersect(first.start, first.end, second.start, second.end):
                diagnostics.append(
                    f"critical routes {first.net!r} and {second.net!r} intersect on {first.layer.value}"
                )
    return tuple(sorted(set(diagnostics)))


def _segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool:
    def cross(p: Point, q: Point, r: Point) -> int:
        return (q.x_nm - p.x_nm) * (r.y_nm - p.y_nm) - (q.y_nm - p.y_nm) * (r.x_nm - p.x_nm)

    values = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b))
    return (
        (values[0] == 0 and _within(a, c, b))
        or (values[1] == 0 and _within(a, d, b))
        or (values[2] == 0 and _within(c, a, d))
        or (values[3] == 0 and _within(c, b, d))
        or ((values[0] > 0) != (values[1] > 0) and (values[2] > 0) != (values[3] > 0))
    )


def _within(a: Point, point: Point, b: Point) -> bool:
    return (
        min(a.x_nm, b.x_nm) <= point.x_nm <= max(a.x_nm, b.x_nm)
        and min(a.y_nm, b.y_nm) <= point.y_nm <= max(a.y_nm, b.y_nm)
    )


def _fingerprint(
    global_fingerprint: str,
    tracks: list[TrackSegment],
    vias: list[Via],
    results: list[CriticalNetResult],
) -> str:
    document = {
        "global": global_fingerprint,
        "tracks": [
            (item.net, item.layer.value, item.start.x_nm, item.start.y_nm, item.end.x_nm, item.end.y_nm, item.width_nm)
            for item in tracks
        ],
        "vias": [
            (item.net, item.position.x_nm, item.position.y_nm, item.from_layer.value, item.to_layer.value, item.size_nm, item.drill_nm)
            for item in vias
        ],
        "results": [
            (item.nets, item.connected, item.lengths_nm, item.skew_nm,
             item.diagnostics, item.assumptions, item.strategy,
             item.candidate_rejected, item.evidence_digests,
             item.search_states, item.candidate_attempts, item.pair_searches,
             item.local_candidate_attempts, item.guide_length_nm,
             item.pair_refinement_attempts, item.pair_refinement_candidates)
            for item in results
        ],
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()

"""Profile-driven exact routing for critical nets.

This stage consumes global guides and creates locked copper before the general
detailed router.  Electrical impedance and RF performance remain external
qualification concerns even when their geometric proxies are satisfied.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal
from enum import Enum
from hashlib import sha256
import json
from math import hypot, sqrt
from types import MappingProxyType
from typing import Callable, Iterable

from .fanout import FanoutResult

from .engineering import EvidenceGrade, propagation_delay
from .physical import (
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PhysicalBoard,
    Point,
    RouteKind,
    StackupLayerKind,
    TrackSegment,
    Via,
)
from .routing import GlobalNetRoute, GlobalRoutingResult, GlobalViaProposal, _placement_fingerprint
from .breakout import BreakoutRegions
from .geometry import segment_distance_squared
from .routing_clearance import RoutingClearanceIndex
from .routing_vias import physical_via_span
from .routing_layers import routing_layers
from .surface_path import via_inside_board
from .drc import DrcSeverity, run_physical_drc
from .pair_search import PairSearchCandidate, PairSearchStats, paired_candidates
from .pair_vias import paired_via_candidates, transition_spacing
from .pair_refine import PairRefinementStats, paired_shortcuts
from .local_critical import local_surface_candidates
from .return_paths import (shared_reference_plane, transition_contact_layers,
                           pair_reference_intent_covers)
from .critical_bundles import (BUNDLE_REPAIR_LIMIT, BundleRepair, CriticalBundle, bundle_document,
                               bundle_job_order, plan_bundles)
from .critical_tuning import MatchTuningMember, MatchTuningResult, UnitTuner, match_tuning_document


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
    pair_search_order: str | None = None
    pair_state_limit: int | None = None
    pair_budget_exhausted: bool = False
    shared_reference_transition_count: int = 0
    # First-failing gate of every rejected exact candidate, counted per gate
    # (most frequent first), plus at most three example messages.
    rejections: tuple[tuple[str, int], ...] = ()
    rejection_examples: tuple[str, ...] = ()


def critical_net_document(item: CriticalNetResult) -> dict[str, object]:
    """JSON-ready form of one group result, shared by reports and checkpoints."""
    return {
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
        "shared_reference_transition_count": item.shared_reference_transition_count,
        "tuned_length_nm": item.tuned_length_nm,
        "evidence_digests": list(item.evidence_digests),
        "strategy": item.strategy,
        "candidate_rejected": item.candidate_rejected,
        "search_states": item.search_states,
        "candidate_attempts": item.candidate_attempts,
        "pair_search_order": item.pair_search_order,
        "pair_state_limit": item.pair_state_limit,
        "pair_budget_exhausted": item.pair_budget_exhausted,
        "pair_searches": item.pair_searches,
        "local_candidate_attempts": item.local_candidate_attempts,
        "guide_length_nm": item.guide_length_nm,
        "pair_refinement_attempts": item.pair_refinement_attempts,
        "pair_refinement_candidates": item.pair_refinement_candidates,
        "rejections": dict(item.rejections),
        "rejection_examples": list(item.rejection_examples),
    }


def critical_lane_table(
    board: PhysicalBoard, results: Iterable[CriticalNetResult],
) -> tuple[dict[str, object], ...]:
    """Per critical net: routed length, signal vias, layers and screening delay.

    Lengths and layers cover the net's track copper on ``board``. Via counts
    are the net's own vias; return vias belong to their reference net. The
    delay is quasi-TEM screening evidence from the declared stack-up's
    dielectric next to each layer, excludes via barrels, and is not sign-off.
    """
    order = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    lanes: list[dict[str, object]] = []
    for item in results:
        if any(net.startswith("<") for net in item.nets):
            continue
        for net in item.nets:
            tracks = tuple(track for track in board.tracks if track.net == net)
            per_layer: dict[CopperLayer, list[TrackSegment]] = {}
            for track in tracks:
                per_layer.setdefault(track.layer, []).append(track)
            layers = sorted(per_layer, key=lambda layer: (order.get(layer, len(order)), layer.value))
            delay_ps, permittivity, reason = _lane_delay(board, per_layer, layers)
            lanes.append({
                "net": net,
                "group": list(item.nets),
                "connected": item.connected,
                "routed_length_nm": _track_length(tracks),
                "layer_lengths_nm": {layer.value: _track_length(tuple(per_layer[layer])) for layer in layers},
                "layers": [layer.value for layer in layers],
                "via_count": sum(via.net == net for via in board.vias),
                "estimated_delay_ps": delay_ps,
                "effective_permittivity": permittivity,
                "delay_evidence_grade": EvidenceGrade.SCREENING.value if delay_ps is not None else None,
                "delay_reason": reason,
            })
    return tuple(lanes)


def _lane_delay(
    board: PhysicalBoard, per_layer: dict[CopperLayer, list[TrackSegment]],
    layers: list[CopperLayer],
) -> tuple[float | None, dict[str, float], str | None]:
    if not board.stackup.physical_layers:
        return None, {}, "no stack-up declared"
    if not layers:
        return None, {}, "no routed copper"
    total = Decimal(0)
    permittivity: dict[str, float] = {}
    for layer in layers:
        tracks = per_layer[layer]
        length = _track_length(tuple(tracks))
        if length <= 0:
            continue
        widths: dict[int, int] = {}
        for track in tracks:
            widths[track.width_nm] = widths.get(track.width_nm, 0) + _track_length((track,))
        width = min(widths, key=lambda value: (-widths[value], value))
        effective, reason = _effective_permittivity(board, layer, width)
        if effective is None:
            return None, {}, reason
        permittivity[layer.value] = float(effective.quantize(Decimal("0.0001")))
        try:
            delay = propagation_delay(length, effective).value
        except ValueError as exc:
            return None, {}, f"{layer.value}: {exc}"
        assert delay is not None
        total += delay
    picoseconds = (total * Decimal(10) ** 12).quantize(Decimal("0.001"))
    return float(picoseconds), permittivity, None


def _effective_permittivity(
    board: PhysicalBoard, layer: CopperLayer, width_nm: int,
) -> tuple[Decimal | None, str | None]:
    """Screening effective permittivity from the dielectric next to ``layer``.

    Outer layers use the Hammerstad-Jensen microstrip estimate with the
    dielectric toward the board's interior; inner layers use the
    thickness-weighted permittivity of the dielectrics on both sides.
    """
    physical = board.stackup.physical_layers
    index = next((position for position, item in enumerate(physical)
                  if item.copper_layer is layer), None)
    if index is None:
        return None, f"stack-up does not declare {layer.value}"
    copper = board.stackup.copper_layers
    if layer in (copper[0], copper[-1]):
        inward = index + 1 if layer is copper[0] else index - 1
        neighbours = [physical[inward]] if 0 <= inward < len(physical) else []
    else:
        neighbours = [physical[position] for position in (index - 1, index + 1)
                      if 0 <= position < len(physical)]
    if not neighbours or any(item.kind is not StackupLayerKind.DIELECTRIC for item in neighbours):
        return None, f"no dielectric declared next to {layer.value}"
    missing = [item.id for item in neighbours if item.relative_permittivity is None]
    if missing:
        return None, f"dielectric {missing[0]} declares no relative permittivity"
    if len(neighbours) == 1:
        dielectric = neighbours[0]
        er = float(dielectric.relative_permittivity)
        effective = (er + 1) / 2 + (er - 1) / 2 / sqrt(1 + 12 * dielectric.thickness_nm / width_nm)
    else:
        effective = (sum(float(item.relative_permittivity) * item.thickness_nm for item in neighbours)
                     / sum(item.thickness_nm for item in neighbours))
    return Decimal(f"{effective:.6f}"), None


@dataclass(frozen=True, slots=True)
class CriticalRoutingResult:
    status: CriticalRoutingStatus
    board: PhysicalBoard
    nets: tuple[CriticalNetResult, ...]
    locked_tracks: tuple[TrackSegment, ...]
    locked_vias: tuple[Via, ...]
    global_routing_fingerprint: str
    routing_fingerprint: str
    reserved_track_count: int = 0
    reserved_via_count: int = 0
    # Bundles of pairs between the same two components: order and repairs.
    bundles: tuple[CriticalBundle, ...] = ()
    # Length-match tuning outcome per ``length_match`` group (plan R3).
    match_tuning: tuple[MatchTuningResult, ...] = ()

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-critical-route/v0.1",
            "status": self.status.value,
            "global_routing_fingerprint": self.global_routing_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "reserved_track_count": self.reserved_track_count,
            "reserved_via_count": self.reserved_via_count,
            "nets": [critical_net_document(item) for item in self.nets],
            "lanes": list(critical_lane_table(self.board, self.nets)),
            "bundles": [bundle_document(item) for item in self.bundles],
            "match_tuning": [match_tuning_document(item) for item in self.match_tuning],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


def _pair_prefers_via_escape(
    board: PhysicalBoard, first: GlobalNetRoute, second: GlobalNetRoute,
) -> bool:
    """Prefer matched via portals for an internally located SMD terminal pair.

    Local copper-pad center bounds are an inexpensive ordering heuristic, not
    proof that surface escape is impossible. Both members and every same-number
    land at one endpoint must be internal. Perimeter/THT/ambiguous cases retain
    surface-first search; all exact/native gates and fallback budgets remain.
    """
    placements = {item.reference: item for item in board.placements}
    partners = {access.pad.component: access for access in second.accesses}
    margin = board.rules.default_via_size_nm + 2*board.rules.minimum_clearance_nm
    for access in first.accesses:
        other = partners.get(access.pad.component)
        placement = placements.get(access.pad.component)
        if other is None or placement is None:
            continue
        pads = tuple(p for p in board.footprints[placement.footprint].pads
                     if p.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE})
        if not pads:
            continue
        min_x, max_x = min(p.position.x_nm for p in pads), max(p.position.x_nm for p in pads)
        min_y, max_y = min(p.position.y_nm for p in pads), max(p.position.y_nm for p in pads)
        matches = [tuple(p for p in pads if p.number == terminal.pad.pad)
                   for terminal in (access, other)]
        if all(lands and all(p.kind is PadKind.SMD
                and min_x+margin < p.position.x_nm < max_x-margin
                and min_y+margin < p.position.y_nm < max_y-margin for p in lands)
               for lands in matches):
            return True
    return False


def route_critical_nets(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    *, on_progress: Callable[[str, tuple[str, ...], CriticalNetResult | None], None] | None = None,
    reserved_accesses: FanoutResult | None = None,
    pair_state_limit: int | None = None,
    bundle_repair_limit: int = BUNDLE_REPAIR_LIMIT,
) -> CriticalRoutingResult:
    """Route critical groups around explicit, immutable ordinary package access.

    The locked prefix includes reservations, but ownership remains with fanout.
    Reservations cannot be arbitrary prior area copper or critical-net stubs.

    Groups are routed by priority, kind and name, except that the pairs of a
    bundle (same two components, kind and priority) are routed outermost
    first along their terminal row. A bundle pair that fails on spacing
    against an earlier pair of its bundle may rip that pair up once per
    attempt, at most ``bundle_repair_limit`` attempts per bundle.

    After every group is accepted, each ``length_match`` group over its
    ``max_skew`` is tuned (``_tune_match_groups``); a group that cannot be
    brought within the limit keeps its copper and fails the stage.
    """

    if pair_state_limit is not None and pair_state_limit <= 0:
        raise ValueError("critical pair aggregate state bound must be positive")
    if bundle_repair_limit < 0:
        raise ValueError("critical bundle repair limit cannot be negative")
    from .hard_macros import macro_source, materialize_hard_macros
    board = materialize_hard_macros(macro_source(board))
    if board.hard_macros and global_route.placement_fingerprint != _placement_fingerprint(board):
        raise ValueError("critical global guides are stale")
    routes = {item.net: item for item in global_route.routes}
    rules = {item.net: item for item in board.net_routing_rules}
    coupled_pairs: set[frozenset[str]] = set()
    tracks: list[TrackSegment] = list(board.tracks)
    vias: list[Via] = list(board.vias)
    if reserved_accesses is not None:
        if global_route.placement_fingerprint != _placement_fingerprint(board):
            raise ValueError("package-access critical guides are stale")
        if (replace(reserved_accesses.board, tracks=board.tracks, vias=board.vias) != board
                or reserved_accesses.board.tracks != (*board.tracks, *reserved_accesses.created_tracks)
                or reserved_accesses.board.vias != (*board.vias, *reserved_accesses.created_vias)):
            raise ValueError("package-access reservations are stale or contain unowned copper")
        protected = {rule.net for rule in board.net_routing_rules if rule.kind is not RouteKind.GENERAL}
        regional_nets = {zone.net for zone in board.zones if zone.reserve_routing}
        protected.update(zone.net for zone in board.zones if zone.net not in regional_nets)
        if any(item.net in protected for item in (*reserved_accesses.created_tracks,
                                                  *reserved_accesses.created_vias)):
            raise ValueError("ordinary package-access reservations cannot contain critical or zone copper")
        def hard(checked: PhysicalBoard) -> tuple:
            return tuple(finding for finding in run_physical_drc(checked).findings
                         if finding.severity is DrcSeverity.ERROR
                         and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})

        fatal = hard(reserved_accesses.board)
        if fatal:
            # Only findings the reservations introduce count against them. A
            # finding the board already had (e.g. a footprint's own land too
            # near its locating hole) stays for final DRC to report.
            baseline = {finding.fingerprint for finding in hard(board)}
            fatal = tuple(finding for finding in fatal if finding.fingerprint not in baseline)
        if fatal:
            # A structurally valid but physically failed preflight is a partial
            # design result, not an API misuse. Keep evidence; never route over it.
            nets = (CriticalNetResult(("<package-reservations>",), False, 0, 0, (), 0,
                                      tuple(f"{f.code}: {f.message}" for f in fatal)),)
            tracks, vias = list(reserved_accesses.board.tracks), list(reserved_accesses.board.vias)
            failed_board = replace(reserved_accesses.board,
                metadata={**board.metadata, "critical_routing": "failed", "detailed_routing": "partial",
                          "fabrication_ready": "false"})
            return CriticalRoutingResult(CriticalRoutingStatus.FAILED, failed_board, nets,
                tuple(tracks), tuple(vias), global_route.routing_fingerprint,
                _fingerprint(global_route.routing_fingerprint, tracks, vias, list(nets)), len(tracks), len(vias))
        tracks.extend(reserved_accesses.created_tracks)
        vias.extend(reserved_accesses.created_vias)
    results: list[CriticalNetResult] = []
    from .drc import explicit_copper_connectivity
    owner_graph = explicit_copper_connectivity(board) if board.materialized_macros else None
    net_by_name = {n.name: n for n in board.nets}
    jobs = _critical_jobs(board)
    bundles = plan_bundles(board, jobs, rules, bundle_repair_limit)
    jobs = bundle_job_order(jobs, bundles)
    bundle_of = {group: index for index, bundle in enumerate(bundles) for group in bundle.order}
    repairs: list[list[BundleRepair]] = [[] for _ in bundles]
    # Copper accepted by this stage, per group in acceptance order. ``tracks``
    # and ``vias`` are always the base copper followed by these entries.
    base_tracks, base_vias = tuple(tracks), tuple(vias)
    committed: list[tuple[tuple[str, ...], tuple[TrackSegment, ...], tuple[Via, ...]]] = []
    result_index: dict[tuple[str, ...], int] = {}
    for rule, group in jobs:
        if on_progress:
            on_progress("started", group, None)
        if owner_graph is not None and any(t.net == rule.net for t in board.tracks):
            if all(name in net_by_name and owner_graph.net_connected(net_by_name[name]) for name in group):
                # Preserve pre-routed copper and still measure actual budgets.
                # Differential macros need paired access/return certificates;
                # single-ended RF/clock/power owner nets are accepted here.
                owned_tracks = tuple(t for t in board.tracks if t.net == rule.net)
                owned_vias = tuple(v for v in board.vias if v.net == rule.net)
                diagnostics = _budget_diagnostics(rule, owned_tracks, owned_vias)
                if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}:
                    diagnostics += ("pre-routed differential macro requires a paired geometry/return certificate",)
                result = CriticalNetResult(group, not diagnostics, len(owned_tracks), len(owned_vias),
                    (_track_length(owned_tracks),), 0, diagnostics, _external_assumptions(rule),
                    evidence_digests=_external_evidence(rule), strategy="immutable_hard_macro")
                result_index[group] = len(results)
                results.append(result)
                if on_progress:
                    on_progress("finished", group, result)
                continue
        rerouted: tuple[tuple[str, ...], CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]] | None = None
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
                result_index[group] = len(results)
                results.append(result)
                if on_progress:
                    on_progress("finished", group, result)
                continue
            blockers: set[str] = set()
            result, pair_tracks, pair_vias = _route_pair_group(
                board, rule, partner_rule, routes, rules, tracks, vias, pair_state_limit, blockers,
            )
            if not result.connected and group in bundle_of:
                index = bundle_of[group]
                bundle_groups = set(bundles[index].order)
                suspects = [entry for entry, entry_tracks, entry_vias in committed
                            if entry in bundle_groups and (entry_tracks or entry_vias)
                            and blockers.intersection(entry)]
                for blocker in suspects:
                    if len(repairs[index]) >= bundle_repair_limit:
                        break
                    record, repaired = _bundle_repair(
                        board, routes, rules, pair_state_limit, base_tracks, base_vias, committed,
                        (rule, partner_rule), group, blocker,
                    )
                    repairs[index].append(record)
                    if repaired is not None:
                        (result, pair_tracks, pair_vias), (blocker_result, blocker_tracks, blocker_vias) = repaired
                        committed = [entry for entry in committed if entry[0] != blocker]
                        results[result_index[blocker]] = blocker_result
                        tracks = [*base_tracks, *(t for _, items, _ in committed for t in items)]
                        vias = [*base_vias, *(v for _, _, items in committed for v in items)]
                        rerouted = (blocker, blocker_result, blocker_tracks, blocker_vias)
                        break
            coupled_pairs.add(frozenset((rule.net, partner_name)))
            net_tracks, net_vias = pair_tracks, pair_vias
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
                repaired_single, net_tracks, net_vias = _route_single_exact(
                    board, rule, global_route, tracks, vias,
                )
                result = replace(repaired_single, local_candidate_attempts=result.local_candidate_attempts,
                                 guide_length_nm=result.guide_length_nm)
        result_index[group] = len(results)
        results.append(result)
        committed.append((group, tuple(net_tracks), tuple(net_vias)))
        tracks.extend(net_tracks)
        vias.extend(net_vias)
        if on_progress:
            on_progress("finished", group, result)
        if rerouted is not None:
            blocker, blocker_result, blocker_tracks, blocker_vias = rerouted
            committed.append((blocker, blocker_tracks, blocker_vias))
            tracks.extend(blocker_tracks)
            vias.extend(blocker_vias)
            if on_progress:
                on_progress("started", blocker, None)
                on_progress("finished", blocker, blocker_result)

    match_tuning = _tune_match_groups(board, rules, base_tracks, base_vias, committed, results, result_index)
    if any(item.status == "tuned" for item in match_tuning):
        tracks = [*base_tracks, *(t for _, items, _ in committed for t in items)]
        vias = [*base_vias, *(v for _, _, items in committed for v in items)]

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
    failed = (any(not item.connected or item.diagnostics for item in results)
              or any(item.status == "failed" for item in match_tuning))
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
        len(reserved_accesses.created_tracks) if reserved_accesses else 0,
        len(reserved_accesses.created_vias) if reserved_accesses else 0,
        tuple(replace(bundle, repairs=tuple(records)) for bundle, records in zip(bundles, repairs)),
        match_tuning,
    )


def _critical_jobs(board: PhysicalBoard) -> list[tuple[NetRoutingRule, tuple[str, ...]]]:
    """Critical groups in the default order: priority, kind, then net name.

    A pair is one group, placed at its first member's position. Each net
    belongs to the first group that names it.
    """
    jobs: list[tuple[NetRoutingRule, tuple[str, ...]]] = []
    seen: set[str] = set()
    for rule in sorted(
        (item for item in board.net_routing_rules if item.kind is not RouteKind.GENERAL),
        key=lambda item: (-item.priority, item.kind.value, item.net),
    ):
        if rule.net in seen:
            continue
        group = (tuple(sorted((rule.net, rule.differential_partner)))
                 if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS} else (rule.net,))
        seen.update(group)
        jobs.append((rule, group))
    return jobs


_Committed = list[tuple[tuple[str, ...], tuple[TrackSegment, ...], tuple[Via, ...]]]


def _tune_match_groups(
    board: PhysicalBoard, rules: dict[str, NetRoutingRule],
    base_tracks: tuple[TrackSegment, ...], base_vias: tuple[Via, ...],
    committed: _Committed, results: list[CriticalNetResult],
    result_index: dict[tuple[str, ...], int],
) -> tuple[MatchTuningResult, ...]:
    """Tune each ``length_match`` group over its ``max_skew`` (plan R3).

    Runs once every critical group is accepted, group by group in declaration
    order. Groups within the limit are untouched; groups with a member that is
    not connected yet are reported ``incomplete`` and judged by final
    verification. A tuned group's copper and results replace the entries in
    ``committed`` and ``results`` only when the whole group ends within its
    limit; otherwise nothing changes.
    """
    if not board.match_groups:
        return ()
    from .signal_integrity import verify_match_groups

    current = replace(board, tracks=(*base_tracks, *(t for _, items, _ in committed for t in items)),
                      vias=(*base_vias, *(v for _, _, items in committed for v in items)))
    outcomes: list[MatchTuningResult] = []
    for check, group in zip(verify_match_groups(current), board.match_groups):
        before = {member.net: member.length_nm for member in check.members}
        if check.status != "fail":
            outcomes.append(MatchTuningResult(
                group.id, group.max_skew_nm, "within_limit" if check.status == "pass" else "incomplete",
                check.skew_nm, check.skew_nm,
                tuple(MatchTuningMember(net, before[net], before[net]) for net in group.nets)))
            continue
        trial, tuned, bumps, reason = _tune_match_group(
            board, rules, base_tracks, base_vias, committed, results, result_index, group.nets,
            group.max_skew_nm, before)
        after = dict(before)
        if trial is not None:
            copper = (*base_tracks, *(t for _, items, _ in trial for t in items))
            for net in group.nets:
                after[net] = _track_length(tuple(t for t in copper if t.net == net))
            skew = max(after.values()) - min(after.values())
            if skew > group.max_skew_nm:
                trial, after, reason = None, dict(before), f"skew is still {skew} nm after tuning"
        if trial is None:
            outcomes.append(MatchTuningResult(
                group.id, group.max_skew_nm, "failed", check.skew_nm, check.skew_nm,
                tuple(MatchTuningMember(net, before[net], before[net]) for net in group.nets),
                reason=reason))
            continue
        committed[:] = trial
        for unit, result in tuned.items():
            results[result_index[unit]] = result
        outcomes.append(MatchTuningResult(
            group.id, group.max_skew_nm, "tuned", check.skew_nm,
            max(after.values()) - min(after.values()),
            tuple(MatchTuningMember(net, before[net], after[net]) for net in group.nets), bumps))
    return tuple(outcomes)


def _tune_match_group(
    board: PhysicalBoard, rules: dict[str, NetRoutingRule],
    base_tracks: tuple[TrackSegment, ...], base_vias: tuple[Via, ...],
    committed: _Committed, results: list[CriticalNetResult],
    result_index: dict[tuple[str, ...], int], nets: tuple[str, ...], max_skew_nm: int,
    lengths: dict[str, int],
) -> tuple[_Committed | None, dict[tuple[str, ...], CriticalNetResult], int, str]:
    """Lengthen every unit of one group toward its longest member.

    A unit is one accepted critical group touching the group: a pair is tuned
    as one unit, both members gaining the same length, so its own skew is
    kept; a single-ended net is tuned alone. Each unit is first tuned to match
    its longest group member to the group's longest member, then, if that does
    not fit or validate, by the minimum that brings its shortest member within
    ``max_skew``. Each step passes the profile gates and the atomic native-DRC
    validation against all other copper, including units already tuned in
    this group. Returns the trial copper, the tuned results, the bump count
    and, on failure, ``None`` and the reason. Nothing is changed here.
    """
    members = set(nets)
    longest = max(lengths.values())
    units = [index for index, (unit, items, _) in enumerate(committed)
             if items and members.intersection(unit)]
    covered = {net for index in units for net in committed[index][0]}
    fixed = [net for net in nets if net not in covered and lengths[net] < longest - max_skew_nm]
    if fixed:
        return None, {}, 0, f"{fixed[0]} has no accepted critical copper to tune"
    trial = list(committed)
    tuned: dict[tuple[str, ...], CriticalNetResult] = {}
    bumps = 0
    breakout = BreakoutRegions(board)
    for index in units:
        unit, unit_tracks, unit_vias = trial[index]
        label = "/".join(unit)
        own = [lengths[net] for net in unit if net in members]
        need, aim = longest - max_skew_nm - min(own), longest - max(own)
        if need <= 0:
            continue
        if aim < need:
            return None, {}, 0, (f"{label}: members differ by {max(own) - min(own)} nm, more than "
                                 f"max_skew; a pair is tuned as one unit")
        amplitudes = [rules[net].tuning_amplitude_limit_nm for net in unit
                      if rules[net].tuning_amplitude_limit_nm is not None]
        if not amplitudes:
            return None, {}, 0, f"{label}: no tuning_amplitude_limit is declared"
        spacing = 0
        if len(unit) == 2:
            first, second = rules[unit[0]], rules[unit[1]]
            spacing = ((first.width_nm or board.rules.default_track_width_nm)
                       + (first.pair_gap_nm or second.pair_gap_nm or 0))
        clearance = max(board.rules.minimum_clearance_nm, *(rules[net].clearance_nm or 0 for net in unit))
        other_tracks = [*base_tracks, *(t for position, (_, items, _) in enumerate(trial)
                                        if position != index for t in items)]
        other_vias = [*base_vias, *(v for position, (_, _, items) in enumerate(trial)
                                    if position != index for v in items)]
        tuner = UnitTuner(board, RoutingClearanceIndex(replace(board, tracks=tuple(other_tracks),
                                                               vias=(*other_vias, *unit_vias))),
                          unit_tracks, unit, spacing, min(amplitudes), clearance)
        # Bumps add an even length: match the longest member (one nanometre
        # over it only when an odd exact target leaves no choice), else add
        # the least that brings the unit within max_skew.
        full = aim - aim % 2 if aim - aim % 2 >= need else aim + 1
        reason = ""
        for target in dict.fromkeys((full, need + need % 2)):
            proposed, count, reason = tuner.tune(target)
            if proposed is None:
                continue
            if breakout:
                # Guard: copper keeps the breakout width only inside a terminal
                # land's region, and the net's own width outside it (plan R1),
                # even if a bump crosses a region boundary.
                proposed = tuple(piece for track in proposed for piece in breakout.split_tracks(
                    (track,), rules[track.net].width_nm or board.rules.default_track_width_nm))
            candidate = _retuned_result(board, rules, results[result_index[unit]], proposed, unit_vias)
            if not candidate.diagnostics:
                candidate, proposed, _ = _validate_candidate(
                    board, candidate, proposed, unit_vias, other_tracks, other_vias)
            if candidate.connected and not candidate.diagnostics:
                trial[index] = (unit, proposed, unit_vias)
                tuned[unit] = candidate
                bumps += count
                break
            reason = _rejection_gate(candidate.diagnostics)[1]
        else:
            return None, {}, 0, f"{label}: {reason}"
    return trial, tuned, bumps, ""


def _retuned_result(
    board: PhysicalBoard, rules: dict[str, NetRoutingRule], original: CriticalNetResult,
    tracks: tuple[TrackSegment, ...], vias: tuple[Via, ...],
) -> CriticalNetResult:
    """An accepted group's metrics and profile gates for new copper.

    The gates and their messages match the routing candidates: each member's
    length and via budget, and for a pair its skew and uncoupled length.
    """
    per_net = [tuple(t for t in tracks if t.net == net) for net in original.nets]
    lengths = tuple(_track_length(items) for items in per_net)
    diagnostics: list[str] = []
    for net, items in zip(original.nets, per_net):
        diagnostics.extend(_budget_diagnostics(rules[net], items, tuple(v for v in vias if v.net == net)))
    if len(original.nets) != 2:
        return replace(original, track_count=len(tracks), lengths_nm=lengths,
                       diagnostics=tuple(diagnostics))
    first, second = rules[original.nets[0]], rules[original.nets[1]]
    skew = abs(lengths[0] - lengths[1])
    skews = [value for value in (first.max_skew_nm, second.max_skew_nm) if value is not None]
    if skews and skew > min(skews):
        diagnostics.append(f"pair skew {skew} nm exceeds {min(skews)} nm")
    spacing = (first.width_nm or board.rules.default_track_width_nm) + (first.pair_gap_nm or second.pair_gap_nm or 0)
    coupled = _coupled_length(list(per_net[0]), list(per_net[1]), spacing)
    uncoupled = tuple(max(0, length - coupled) for length in lengths)
    limits = [value for value in (first.maximum_uncoupled_length_nm, second.maximum_uncoupled_length_nm)
              if value is not None]
    if limits and max(uncoupled) > min(limits):
        diagnostics.append(f"pair uncoupled length {max(uncoupled)} nm exceeds {min(limits)} nm")
    return replace(original, track_count=len(tracks), lengths_nm=lengths, skew_nm=skew,
                   coupled_length_nm=coupled, uncoupled_lengths_nm=uncoupled,
                   diagnostics=tuple(diagnostics))


# First-failing gates that name another net's copper as the obstacle.
_SPACING_GATES = frozenset({"DRC-CLEARANCE", "DRC-SHORT", "DRC-HOLE-CLEARANCE", "DRC-DRILL-SPACING"})


def _route_pair_group(
    board: PhysicalBoard, rule: NetRoutingRule, partner_rule: NetRoutingRule,
    routes: dict[str, GlobalNetRoute], rules: dict[str, NetRoutingRule],
    tracks: list[TrackSegment], vias: list[Via], pair_state_limit: int | None,
    blockers: set[str] | None = None,
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Route one symmetric pair against the committed ``tracks``/``vias``.

    The coarse candidate is tried first, then the bounded joint searches.
    ``blockers`` collects the other nets named by spacing findings of every
    rejected candidate whose first-failing gate is a spacing gate.
    """
    def observe(attempt: CriticalNetResult, found: set[str]) -> None:
        if (blockers is not None and not attempt.connected
                and _rejection_gate(attempt.diagnostics)[0] in _SPACING_GATES):
            blockers.update(found)

    partner_name = partner_rule.net
    found: set[str] = set()
    result, pair_tracks, pair_vias = _route_pair(
        board, rule, partner_rule, routes
    )
    result, pair_tracks, pair_vias = _reject_reserved_plane_tracks(
        board, result, pair_tracks, pair_vias, rules,
    )
    result, pair_tracks, pair_vias = _validate_candidate(
        board, result, pair_tracks, pair_vias, tracks, vias, spacing_nets=found,
    )
    observe(result, found)
    if not result.connected and all(
        routes.get(name) is not None and routes[name].connected
        for name in (rule.net, partner_name)
    ):
        first, second = sorted((rule, partner_rule), key=lambda item: item.net)
        search_board = replace(board, tracks=tuple(tracks), vias=tuple(vias))
        stats = PairSearchStats()
        # The failed coarse-guide candidate has already had an atomic
        # gate. Internal lands should not consume all surface maze
        # resolutions before exploring a legal paired layer escape.
        prefer_vias = _pair_prefers_via_escape(search_board, routes[first.net], routes[second.net])
        searchers = ((paired_via_candidates, paired_candidates) if prefer_vias
                     else (paired_candidates, paired_via_candidates))
        rejected: list[tuple[str, str]] = []
        for searcher in searchers:
            for pitch_nm in (1_000_000, 500_000, 250_000):
                # A limited tier samples both families and all pitches,
                # rather than spending its entire cap on the first port.
                # Full-budget standalone calls retain historical bounds.
                limits = {}
                if pair_state_limit is not None:
                    remaining = pair_state_limit - stats.expanded_states
                    if remaining <= 0:
                        break
                    slice_size = min(remaining, max(1, pair_state_limit // 6))
                    limits = {"maximum_total_states": stats.expanded_states + slice_size,
                              "maximum_states": max(1, (slice_size + 1) // 2),
                              "maximum_searches": 2}
                for candidate in searcher(
                    search_board, first, second, routes[first.net], routes[second.net],
                    stats=stats, pitch_nm=pitch_nm, **limits,
                ):
                    attempt, proposed_tracks, proposed_vias = _route_pair(
                        board, first, second, routes,
                        exact_tracks=(candidate.first, candidate.second),
                        exact_via_pairs=candidate.via_pairs,
                        exact_return_vias=candidate.return_vias,
                    )
                    attempt, proposed_tracks, proposed_vias = _reject_reserved_plane_tracks(
                        board, attempt, proposed_tracks, proposed_vias, rules,
                    )
                    found = set()
                    attempt, proposed_tracks, proposed_vias = _validate_candidate(
                        board, attempt, proposed_tracks, proposed_vias, tracks, vias,
                        spacing_nets=found,
                    )
                    if attempt.connected:
                        result, pair_tracks, pair_vias = attempt, proposed_tracks, proposed_vias
                        if not candidate.via_pairs:
                            result, pair_tracks, pair_vias = _improve_pair_spine(
                                board, search_board, first, second, routes, candidate,
                                result, pair_tracks, pair_vias, tracks, vias,
                            )
                        break
                    observe(attempt, found)
                    rejected.append(_rejection_gate(attempt.diagnostics))
                if result.connected:
                    break
            if result.connected:
                break
        rejections, examples = _rejection_summary(rejected)
        result = replace(result, search_states=stats.expanded_states,
                         candidate_attempts=stats.candidates, pair_searches=stats.searches,
                         pair_search_order=("via_first_internal_lands" if prefer_vias else "surface_first"),
                         pair_state_limit=pair_state_limit,
                         pair_budget_exhausted=(pair_state_limit is not None
                                               and stats.expanded_states >= pair_state_limit
                                               and not result.connected),
                         rejections=rejections, rejection_examples=examples)
        if not result.connected:
            reasons = ", ".join(f"{gate}={count}" for gate, count in rejections)
            result = replace(result, diagnostics=(*result.diagnostics,
                f"joint pair search: {stats.searches} searches, {stats.expanded_states} states, "
                f"{stats.candidates} candidates; none accepted"
                + (f" (first-failing gates: {reasons})" if reasons else "")))
    return result, pair_tracks, pair_vias


def _bundle_repair(
    board: PhysicalBoard, routes: dict[str, GlobalNetRoute], rules: dict[str, NetRoutingRule],
    pair_state_limit: int | None, base_tracks: tuple[TrackSegment, ...], base_vias: tuple[Via, ...],
    committed: list[tuple[tuple[str, ...], tuple[TrackSegment, ...], tuple[Via, ...]]],
    failed_rules: tuple[NetRoutingRule, NetRoutingRule], failed: tuple[str, ...],
    blocker: tuple[str, ...],
) -> tuple[BundleRepair, tuple[tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]],
                               tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]] | None]:
    """Rip up ``blocker``, route ``failed`` first, then route ``blocker`` again.

    Both routes use the same coarse/exact candidates and atomic validation as
    the main pass, against all other committed copper. Nothing is changed
    here: the caller commits both results only when both are accepted, and
    otherwise keeps its state exactly as it was.
    """
    others = [entry for entry in committed if entry[0] != blocker]
    reduced_tracks = [*base_tracks, *(t for _, items, _ in others for t in items)]
    reduced_vias = [*base_vias, *(v for _, _, items in others for v in items)]
    first = _route_pair_group(board, *failed_rules, routes, rules,
                              reduced_tracks, reduced_vias, pair_state_limit)
    if not first[0].connected:
        return BundleRepair(failed, blocker, False,
                            "failed group still has no accepted candidate without the ripped-up copper"), None
    second = _route_pair_group(board, rules[blocker[0]], rules[blocker[1]], routes, rules,
                               [*reduced_tracks, *first[1]], [*reduced_vias, *first[2]],
                               pair_state_limit)
    if not second[0].connected:
        return BundleRepair(failed, blocker, False,
                            "ripped-up group has no accepted candidate after the failed group"), None
    return BundleRepair(failed, blocker, True, "both groups accepted"), (first, second)


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
        # A breakout width is the profile's own; DRC holds it to its region.
        if any(t.net != rule.net
               or t.width_nm not in (rule.width_nm or board.rules.default_track_width_nm,
                                     rule.breakout_width_nm)
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
    *, spacing_nets: set[str] | None = None,
) -> tuple[CriticalNetResult, tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Accept a critical group atomically, using authoritative native geometry.

    A coarse guide is not clearance evidence. Check both members, return vias,
    every physical land, and previously accepted critical groups together.
    Unrelated open nets and route-completeness are expected at this stage.
    Hard findings that already exist without the candidate (for example a
    footprint's own land-to-hole spacing) are not attributable to it and do
    not reject it; final verification still reports them. Every other hard
    finding rejects the candidate. Rejected copper never becomes an obstacle.
    ``spacing_nets``, when given, receives the other nets named by the spacing
    findings (clearance, short, hole or drill spacing) the candidate introduces.
    Group skew (``DRC-LENGTH-MATCH``) is deferred: it depends on every member
    and on the tuning pass that runs after all groups are accepted, which
    gates it, and final verification still reports it.
    """
    if not tracks and not vias:
        return result, (), ()
    try:
        candidate = replace(board, tracks=(*committed_tracks, *tracks),
                            vias=(*committed_vias, *vias))
    except ValueError as exc:
        if "hard-macro" not in str(exc):
            raise
        return replace(result, connected=False, track_count=0, via_count=0,
            diagnostics=(*result.diagnostics, str(exc)), candidate_rejected=True), (), ()

    def hard(checked: PhysicalBoard) -> list:
        return [
            finding for finding in run_physical_drc(checked).findings
            if finding.severity is DrcSeverity.ERROR
            and finding.code != "DRC-ROUTE-INCOMPLETE"
            and finding.code != "DRC-LENGTH-MATCH"
            and not (finding.code == "DRC-OPEN-NET"
                     and not set(finding.nets).intersection(result.nets))
        ]

    findings = hard(candidate)
    if findings:
        baseline = {finding.fingerprint for finding in hard(
            replace(board, tracks=tuple(committed_tracks), vias=tuple(committed_vias)))}
        # The pair's own nets are open in the baseline; an open net is never
        # pre-existing evidence for the candidate.
        findings = [finding for finding in findings
                    if finding.code == "DRC-OPEN-NET" or finding.fingerprint not in baseline]
    if spacing_nets is not None:
        for finding in findings:
            if finding.code in _SPACING_GATES:
                spacing_nets.update(set(finding.nets).difference(result.nets))
    diagnostics = list(result.diagnostics)
    for finding in findings:
        diagnostics.append(f"{finding.code}: {finding.message}")
    diagnostics = list(dict.fromkeys(diagnostics))
    if not result.connected or diagnostics:
        return replace(
            result, connected=False, track_count=0, via_count=0,
            return_via_count=0, paired_via_transitions=0, shared_reference_transition_count=0,
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
    # Next to a terminal land the breakout width applies (plan R1).
    tracks = list(BreakoutRegions(board).split_tracks(tracks))
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
    exact_via_pairs: tuple[tuple[Via, Via], ...] = (),
    exact_return_vias: tuple[Via, ...] = (),
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
    if aligned is None and (not guide.segments or not partner_guide.segments):
        # Terminals inside one global tile give a connected guide without
        # segments, so the offset fallback has nothing to follow. Reject the
        # coarse candidate; the caller still runs the exact paired searches.
        return (
            CriticalNetResult(
                (first.net, second.net),
                False,
                0,
                0,
                (),
                0,
                ("coarse pair guide has no segments to offset",),
            ),
            (),
            (),
        )
    if exact_tracks is None and guide.vias:
        blocked = _coarse_transition_conflict(board, first, second, guide, offset)
        if blocked is not None:
            # Global transitions are proposals. Never emit one the profile or
            # static obstacles rule out; the exact paired searches still run.
            return (
                CriticalNetResult((first.net, second.net), False, 0, 0, (), 0, (blocked,)),
                (),
                (),
            )
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
    breakout = BreakoutRegions(board)
    if breakout:
        # Cut both members at their breakout-region boundaries: the pieces
        # next to a terminal land carry the breakout width, the rest the
        # profile width (plan R1). Lengths and tuning use the cut geometry.
        first_tracks = list(breakout.split_tracks(first_tracks, first_width))
        second_tracks = list(breakout.split_tracks(second_tracks, second_width))
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
    if tuned_length and breakout:
        # Cut the new serpentine bumps too; they are axis-aligned, so their
        # lengths do not change.
        first_tracks = list(breakout.split_tracks(first_tracks, first_width))
        second_tracks = list(breakout.split_tracks(second_tracks, second_width))
    pair_vias: list[Via] = [v for pair in exact_via_pairs for v in pair]
    return_vias: list[Via] = list(exact_return_vias)
    for item in (() if exact_tracks is not None else guide.vias):
        for net, sign in ((first.net, 1), (second.net, -1)):
            pair_vias.append(_guide_via(
                board, net, item,
                Point(item.position.x_nm + sign * offset, item.position.y_nm),
            ))
        if (first.require_return_vias or second.require_return_vias) and shared_reference_plane(
                board, first, second, (item.from_layer, item.to_layer), (item.position,)) is None:
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
    shared_references: list[CopperLayer] = []
    if exact_via_pairs:
        # Via-pad spacing is deliberately wider than trace pitch. Require
        # explicit matched transition provenance; never infer a pair from two
        # arbitrary distant vias. Native DRC independently verifies contacts,
        # every through-span layer, drill envelopes and complete connectivity.
        pitch = transition_spacing(board, first, second)
        valid = all(a.net == first.net and b.net == second.net
            and (a.from_layer,a.to_layer,a.size_nm,a.drill_nm,a.technology)
                == (b.from_layer,b.to_layer,b.size_nm,b.drill_nm,b.technology)
            and abs(hypot(a.position.x_nm-b.position.x_nm,
                          a.position.y_nm-b.position.y_nm)-pitch) <= 4
            for a,b in exact_via_pairs)
        valid = valid and len(set(pair_vias)) == len(pair_vias)
        paired_transitions = len(exact_via_pairs) if valid else 0
        if not valid:
            diagnostics.append("explicit differential transition geometry is not paired")
        if exact_tracks is None:
            diagnostics.append("explicit via transitions require exact joint tracks")
    if first.require_return_vias or second.require_return_vias:
        requested = [r for r in (first,second) if r.require_return_vias]
        transition_pairs = exact_via_pairs or tuple(zip(first_pair_vias, second_pair_vias))
        for a,b in transition_pairs:
            first_layers = transition_contact_layers(tuple(first_tracks), a)
            second_layers = transition_contact_layers(tuple(second_tracks), b)
            reference = (shared_reference_plane(board, first, second, first_layers,
                         (a.position, b.position)) if first_layers == second_layers else None)
            if reference is not None:
                if pair_reference_intent_covers(board, reference, first.return_via_net,
                                                tuple((*first_tracks, *second_tracks))):
                    shared_references.append(reference)
                    continue
                diagnostics.append("declared shared reference does not cover the complete paired route")
            if not any(all(v.net == r.return_via_net
                and (v.from_layer,v.to_layer) == (a.from_layer,a.to_layer)
                and max(hypot(v.position.x_nm-s.position.x_nm,v.position.y_nm-s.position.y_nm)
                        for s in (a,b)) <= r.maximum_return_via_distance_nm
                for r in requested) for v in return_vias):
                diagnostics.append("paired transition lacks a permitted nearby return via")
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
    if not exact_via_pairs and return_vias and return_limit is not None and offset > return_limit:
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
    if exact_via_pairs:
        assumptions = (*assumptions,
            "layer transitions require layer-specific impedance and via-stub qualification",
            "reference-via placement does not verify refilled ground-plane continuity")
    if shared_references:
        assumptions = (*assumptions,
            "shared-reference transitions use declared "
            + ",".join(sorted({layer.value for layer in shared_references}))
            + "; zone intent does not certify filled return-path continuity or impedance")
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
            strategy=("joint_pair_via_search" if exact_via_pairs else
                      "joint_pair_search" if exact_tracks is not None else
                      "aligned_pair" if aligned is not None else "global_guide"),
            shared_reference_transition_count=len(shared_references),
        ),
        tracks,
        tuple((*pair_vias, *return_vias)),
    )


def _coarse_transition_conflict(
    board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
    guide: GlobalNetRoute, offset: int,
) -> str | None:
    """Explain why the coarse candidate may not place its guide transitions.

    A profile with no via budget or only one common signal layer gets no
    transitions at all. Otherwise each signal and return via must clear every
    land (vias never overlap pads, whatever their net), drilled holes, via
    keep-outs and the board edge. Spacing to other proposed or committed copper
    is still left to the candidate's native DRC gate.
    """
    common = set(routing_layers(board, first.net, first)) & set(routing_layers(board, second.net, second))
    if any(rule.max_vias == 0 for rule in (first, second)) or len(common) < 2:
        return (f"coarse pair guide proposes {len(guide.vias)} layer transition(s) "
                "but the pair profile allows none")
    index = RoutingClearanceIndex(board)
    for item in guide.vias:
        sites = [(first.net, Point(item.position.x_nm + offset, item.position.y_nm)),
                 (second.net, Point(item.position.x_nm - offset, item.position.y_nm))]
        if first.require_return_vias or second.require_return_vias:
            return_net = first.return_via_net or second.return_via_net
            assert return_net is not None
            sites.append((return_net, item.position))
        for net, position in sites:
            via = _guide_via(board, net, item, position)
            if not (via_inside_board(board, position, via.size_nm)
                    and index.can_via(net, position, via.size_nm, via.from_layer, via.to_layer,
                                      via.drill_nm, check_hole_copper=True)):
                return (f"coarse guide transition for {net} at ({position.x_nm}, {position.y_nm}) nm "
                        "violates pad clearance or a via keep-out")
    return None


def _aligned_pair_paths(
    first: GlobalNetRoute, second: GlobalNetRoute, offset: int,
) -> tuple[list[tuple[CopperLayer, Point, Point]], list[tuple[CopperLayer, Point, Point]]] | None:
    """Try a straight coupled channel with symmetric 45-degree pin tapers.

    Unlike offsetting one member's guide, this uses the midpoint of *both*
    physical terminals. Only unambiguous, same-layer, aligned terminals are
    supported here: at each end the two lands sit side by side across the
    channel, both ends share one midpoint and keep the same member order.
    The land pitch may differ from the pair pitch (width + gap) at either or
    both ends, for example 0.5 mm package lands to 0.4 mm connector lands.
    Each land then tapers at 45 degrees to its lane, symmetrically about the
    channel centre; the tapers are uncoupled length. Clearance and
    connectivity still gate the whole candidate.
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
        # One straight channel: a shared midpoint and the same member order
        # at both ends. The land pitch at each end is free.
        if (lateral(a) + lateral(b) != lateral(c) + lateral(d)
                or lateral(a) == lateral(b)
                or (lateral(a) > lateral(b)) != (lateral(c) > lateral(d))):
            continue
        mid = (lateral(a) + lateral(b)) // 2
        sign = 1 if lateral(a) > lateral(b) else -1
        first_lane, second_lane = mid + sign * offset, mid - sign * offset
        start_taper = max(abs(lateral(a) - first_lane), abs(lateral(b) - second_lane))
        end_taper = max(abs(lateral(c) - first_lane), abs(lateral(d) - second_lane))
        if axial(c) - axial(a) <= start_taper + end_taper:
            continue

        def path(start: Point, end: Point, lane: int) -> list[tuple[CopperLayer, Point, Point]]:
            def point(x: int, y: int) -> Point:
                return Point(x, y) if horizontal else Point(y, x)

            # 45-degree tapers: each ramp's axial run equals its lateral shift.
            points = (start, point(axial(start) + abs(lateral(start) - lane), lane),
                      point(axial(end) - abs(lateral(end) - lane), lane), end)
            return [(accesses[0].layer, p, q) for p, q in zip(points, points[1:])
                    if p != q]

        return path(a, c, first_lane), path(b, d, second_lane)
    return None


_MAX_TUNING_BUMPS = 8


def _tune_pair(first: list[TrackSegment], second: list[TrackSegment],
               max_skew_nm: int, amplitude_limit_nm: int) -> int:
    """Compensate excess intra-pair skew next to where the mismatch arises.

    The shorter member receives rectangular serpentine bumps on its
    axis-aligned segments nearest the terminal or bend where the length
    difference accumulates, starting at the segment end nearest that point
    and bulging away from the partner. Every bump is at most
    ``amplitude_limit_nm`` tall and 3 x width wide, and stays 3 x width from
    the next bump and from both segment ends. The fewest bumps (at most
    eight) that bring the skew within ``max_skew_nm`` are used; if they do not
    fit, the geometry is unchanged and the skew gate reports the mismatch.
    The caller validates the tuned pair atomically (budgets and native DRC).
    Returns the added length in nanometres.
    """
    first_length, second_length = _track_length(tuple(first)), _track_length(tuple(second))
    excess = abs(first_length - second_length) - max_skew_nm
    if excess <= 0 or amplitude_limit_nm <= 0:
        return 0
    target, partner = (first, second) if first_length < second_length else (second, first)
    count = -(-excess // (2 * amplitude_limit_nm))
    if count > _MAX_TUNING_BUMPS:
        return 0
    height = -(-excess // (2 * count))
    placements: dict[int, tuple[bool, int]] = {}
    remaining = count
    for index, from_start in _tuning_sites(target, partner):
        track = target[index]
        length = abs(track.end.x_nm - track.start.x_nm) + abs(track.end.y_nm - track.start.y_nm)
        # n bumps of pitch p need (2n + 1) * p along the segment.
        capacity = (length // (3 * track.width_nm) - 1) // 2
        if capacity <= 0:
            continue
        placements[index] = (from_start, min(capacity, remaining))
        remaining -= placements[index][1]
        if not remaining:
            break
    if remaining:
        return 0
    for index in sorted(placements, reverse=True):
        from_start, bumps = placements[index]
        target[index:index + 1] = _serpentine(target[index], bumps, height, from_start, partner)
    return 2 * height * count


def _tuning_sites(
    target: list[TrackSegment], partner: list[TrackSegment],
) -> list[tuple[int, bool]]:
    """Axis-aligned target segments, nearest the mismatch origin first.

    Each entry is ``(index, from_start)``: bumps start at the segment's start
    when that end is nearer the origin. The origin is the terminal or bend of
    the shorter member where the partner gains the most length, found by
    projecting segment midpoints onto the partner path. Geometry that is not
    a simple two-ended path falls back to the longest segments first.
    """
    axis = [index for index, track in enumerate(target)
            if track.start != track.end
            and (track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm)]
    walk, partner_walk = _ordered_chain(target), _ordered_chain(partner)
    if walk is None or partner_walk is None:
        return [(index, True) for index in sorted(
            axis, key=lambda index: (-_segment_length(target[index]), index))]
    vertices = [target[walk[0][0]].start if walk[0][1] else target[walk[0][0]].end]
    for index, forward in walk:
        vertices.append(target[index].end if forward else target[index].start)
    arcs = [0.0]
    for index, _ in walk:
        arcs.append(arcs[-1] + _segment_length(target[index]))
    path = [(partner[index].start, partner[index].end) if forward
            else (partner[index].end, partner[index].start) for index, forward in partner_walk]
    if (hypot(vertices[0].x_nm - path[-1][1].x_nm, vertices[0].y_nm - path[-1][1].y_nm)
            + hypot(vertices[-1].x_nm - path[0][0].x_nm, vertices[-1].y_nm - path[0][0].y_nm)
            < hypot(vertices[0].x_nm - path[0][0].x_nm, vertices[0].y_nm - path[0][0].y_nm)
            + hypot(vertices[-1].x_nm - path[-1][1].x_nm, vertices[-1].y_nm - path[-1][1].y_nm)):
        path = [(end, start) for start, end in reversed(path)]
    partner_length = sum(hypot(b.x_nm - a.x_nm, b.y_nm - a.y_nm) for a, b in path)
    # Partner-minus-target arc length at each target segment midpoint. Its
    # growth between consecutive samples belongs to the vertex between them.
    lead = [0.0]
    for position, (index, _) in enumerate(walk):
        track = target[index]
        midpoint = ((track.start.x_nm + track.end.x_nm) / 2, (track.start.y_nm + track.end.y_nm) / 2)
        lead.append(_arc_projection(path, midpoint) - (arcs[position] + arcs[position + 1]) / 2)
    lead.append(partner_length - arcs[-1])
    gains = [lead[vertex + 1] - lead[vertex] for vertex in range(len(vertices))]
    origin = max(range(len(gains)), key=lambda vertex: (gains[vertex], -vertex))
    sites = []
    for position, (index, forward) in enumerate(walk):
        if index not in axis:
            continue
        if position >= origin:
            distance, near_walk_start = arcs[position] - arcs[origin], True
        else:
            distance, near_walk_start = arcs[origin] - arcs[position + 1], False
        sites.append((distance, position, index, near_walk_start == forward))
    return [(index, from_start) for _, _, index, from_start in sorted(sites)]


def _ordered_chain(tracks: list[TrackSegment]) -> list[tuple[int, bool]] | None:
    """Walk a simple two-ended path; ``(index, walked start-to-end)`` per segment."""
    live = [index for index, track in enumerate(tracks) if track.start != track.end]
    ends: dict[Point, list[int]] = {}
    for index in live:
        for point in (tracks[index].start, tracks[index].end):
            ends.setdefault(point, []).append(index)
    terminals = sorted((point for point, items in ends.items() if len(items) == 1),
                       key=lambda point: (point.x_nm, point.y_nm))
    if not live or len(terminals) != 2 or any(len(items) > 2 for items in ends.values()):
        return None
    walk: list[tuple[int, bool]] = []
    used: set[int] = set()
    point = terminals[0]
    while next_items := [index for index in ends[point] if index not in used]:
        index = next_items[0]
        used.add(index)
        forward = tracks[index].start == point
        walk.append((index, forward))
        point = tracks[index].end if forward else tracks[index].start
    return walk if len(used) == len(live) else None


def _nearest_on_segment(
    point: tuple[float, float], start: Point, end: Point,
) -> tuple[float, float, float, float]:
    """Distance, nearest x/y and fraction along ``start``-``end`` for ``point``."""
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    length_squared = dx * dx + dy * dy
    fraction = 0.0 if not length_squared else min(1.0, max(0.0, (
        (point[0] - start.x_nm) * dx + (point[1] - start.y_nm) * dy) / length_squared))
    x, y = start.x_nm + fraction * dx, start.y_nm + fraction * dy
    return hypot(x - point[0], y - point[1]), x, y, fraction


def _arc_projection(path: list[tuple[Point, Point]], point: tuple[float, float]) -> float:
    """Arc length along ``path`` of the point nearest ``point`` (first on ties)."""
    best: tuple[float, float] | None = None
    travelled = 0.0
    for start, end in path:
        length = hypot(end.x_nm - start.x_nm, end.y_nm - start.y_nm)
        distance, _, _, fraction = _nearest_on_segment(point, start, end)
        if best is None or distance < best[0]:
            best = (distance, travelled + fraction * length)
        travelled += length
    return best[1] if best is not None else 0.0


def _serpentine(track: TrackSegment, bumps: int, height: int, from_start: bool,
                partner: list[TrackSegment]) -> list[TrackSegment]:
    """Replace one axis-aligned segment with ``bumps`` rectangular bumps."""
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    length = abs(dx) + abs(dy)
    ux, uy = (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)
    pitch = 3 * track.width_nm
    offsets = [pitch + 2 * pitch * bump for bump in range(bumps)]
    if not from_start:
        offsets = sorted(length - pitch - offset for offset in offsets)
    nx, ny = _away_from_partner(track, partner)

    def at(distance: int, raised: bool = False) -> Point:
        return Point(track.start.x_nm + ux * distance + (nx * height if raised else 0),
                     track.start.y_nm + uy * distance + (ny * height if raised else 0))

    points = [track.start]
    for offset in offsets:
        points.extend((at(offset), at(offset, True), at(offset + pitch, True), at(offset + pitch)))
    points.append(track.end)
    return [TrackSegment(track.net, start, end, track.width_nm, track.layer)
            for start, end in zip(points, points[1:]) if start != end]


def _away_from_partner(track: TrackSegment, partner: list[TrackSegment]) -> tuple[int, int]:
    """Unit normal of an axis-aligned segment pointing away from its partner."""
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    ux, uy = (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)
    left = (-uy, ux)
    midpoint = ((track.start.x_nm + track.end.x_nm) / 2, (track.start.y_nm + track.end.y_nm) / 2)
    nearby = [item for item in partner if item.layer is track.layer and item.start != item.end] or [
        item for item in partner if item.start != item.end]
    if nearby:
        _, x, y, _ = min((_nearest_on_segment(midpoint, item.start, item.end) for item in nearby),
                         key=lambda nearest: nearest[0])
        side = left[0] * (x - midpoint[0]) + left[1] * (y - midpoint[1])
        if side > 0:
            return -left[0], -left[1]
        if side < 0:
            return left
    return (0, 1) if dx else (1, 0)


def _segment_length(track: TrackSegment) -> float:
    return hypot(track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm)


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


# Candidate gates in the order they are evaluated: profile budgets and pair
# geometry in ``_route_pair``, plane reservation, then native DRC. Native DRC
# codes are reported verbatim; an open pair net is the connectivity gate.
_GATE_PREFIXES = (
    ("route length ", "length_budget"),
    ("via count ", "via_budget"),
    ("explicit differential transition geometry is not paired", "via_pairing"),
    ("explicit via transitions require exact joint tracks", "via_pairing"),
    ("paired transition lacks a permitted nearby return via", "return_via"),
    ("pair skew ", "skew"),
    ("pair uncoupled length ", "uncoupled_length"),
    ("differential via transitions are not geometrically paired", "via_pairing"),
    ("return via distance ", "return_via"),
    ("critical route uses reserved plane layer", "plane_reservation"),
)


def _diagnostic_gate(message: str) -> str:
    if message.startswith("DRC-OPEN-NET"):
        return "connectivity"
    if message.startswith("DRC-"):
        return message.split(":", 1)[0]
    for prefix, gate in _GATE_PREFIXES:
        if message.startswith(prefix):
            return gate
    return "hard_macro" if "hard-macro" in message else "other"


def _rejection_gate(diagnostics: tuple[str, ...]) -> tuple[str, str]:
    """Return the first-failing gate and its message for a rejected candidate.

    Diagnostics accumulate in evaluation order. A missing connection is only
    reported when no geometric or profile gate failed first, because illegal
    copper usually also leaves the pair electrically open.
    """
    for message in diagnostics:
        gate = _diagnostic_gate(message)
        if gate != "connectivity":
            return gate, message
    if diagnostics:
        return "connectivity", diagnostics[0]
    return "other", "candidate rejected without a diagnostic"


def _rejection_summary(
    rejected: list[tuple[str, str]],
) -> tuple[tuple[tuple[str, int], ...], tuple[str, ...]]:
    """Counted first-failing gates (most frequent first) and three examples.

    Examples cover distinct gates first, in summary order, then further
    distinct messages in candidate order. The result is deterministic.
    """
    counts: dict[str, int] = {}
    for gate, _ in rejected:
        counts[gate] = counts.get(gate, 0) + 1
    summary = tuple(sorted(counts.items(), key=lambda item: (-item[1], item[0])))
    examples: list[str] = []
    for gate, _ in summary:
        message = next(message for item, message in rejected if item == gate)
        if len(examples) < 3 and message not in examples:
            examples.append(message)
    for _, message in rejected:
        if len(examples) >= 3:
            break
        if message not in examples:
            examples.append(message)
    return summary, tuple(examples)


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
             item.pair_refinement_attempts, item.pair_refinement_candidates,
             item.rejections, item.rejection_examples)
            for item in results
        ],
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()

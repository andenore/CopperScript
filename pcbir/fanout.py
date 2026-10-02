"""Deterministic, optional component-level SMD escape planning.

This is physical copper, not electrical intent. Each escape is checked as a
lead-in/via proposal before reservation; unsuccessful pads are reported explicitly.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from types import MappingProxyType
from typing import Mapping

from .drc import placed_pad_shape, run_physical_drc
from .geometry import point_in_polygon
from .physical import (
    BoardSide, CopperLayer, FootprintPad, PadKind, PadReference, PhysicalBoard,
    Placement, Point, RouteKind, TrackSegment, Via, nm_from_mm,
)
from .placement import transformed_pad_position
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .routing_vias import physical_via_span
from .pin_escape import checked_access_path, checked_access_paths
from .escape_assignment import (EscapeAssignmentOptions, EscapeAssignmentReport,
                                EscapeCandidate, improve_escape_assignment)


@dataclass(frozen=True, slots=True)
class FanoutOptions:
    minimum_component_pads: int = 12
    maximum_neighbor_distance_nm: int = nm_from_mm("1.5")
    step_nm: int = nm_from_mm("0.5")
    maximum_radius_nm: int = nm_from_mm("3")
    constrained_pins_first: bool = True
    two_leg_escapes: bool = True
    maximum_two_leg_candidates: int = 256
    joint_escapes: bool = True
    assignment_options: EscapeAssignmentOptions = EscapeAssignmentOptions()

    def __post_init__(self) -> None:
        if min(self.minimum_component_pads, self.maximum_neighbor_distance_nm,
               self.step_nm, self.maximum_radius_nm, self.maximum_two_leg_candidates) <= 0:
            raise ValueError("fanout options must be positive")


@dataclass(frozen=True, slots=True)
class FanoutPinAnalysis:
    """Analyzed immutable-domain slack and selected alternative; not signoff."""

    pad: PadReference
    legal_candidate_count: int
    selected_candidate_index: int | None
    diagnostic: str = ""
    two_leg_candidate_count: int = 0


@dataclass(frozen=True, slots=True)
class FanoutResult:
    board: PhysicalBoard
    accesses: Mapping[PadReference, Point]
    pending_pads: tuple[PadReference, ...]
    added_track_count: int
    added_via_count: int
    created_vias: tuple[Via, ...] = ()
    pin_analysis: tuple[FanoutPinAnalysis, ...] = ()
    created_tracks: tuple[TrackSegment, ...] = ()
    assignment: EscapeAssignmentReport | None = None


def route_fanout(
    board: PhysicalBoard, options: FanoutOptions | None = None,
    *, only_nets: frozenset[str] | None = None,
) -> FanoutResult:
    """Escape only crowded ordinary-net SMD pads to legal through-vias."""

    if board.hard_macros and not board.materialized_macros:
        raise ValueError("materialize hard macros before package escape")
    options = options or FanoutOptions()
    if only_nets is not None:
        unknown = only_nets - {net.name for net in board.nets}
        if unknown:
            raise ValueError(f"unknown fanout nets: {', '.join(sorted(unknown))}")
        if not only_nets:
            return FanoutResult(board, MappingProxyType({}), (), 0, 0)
    if len(board.stackup.copper_layers) < 2:
        return FanoutResult(board, MappingProxyType({}), (), 0, 0)
    clearance = RoutingClearanceIndex(board)
    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads
                  if len(net.pads) >= 2}
    rules = {item.net: item for item in board.net_routing_rules}
    zone_nets = {zone.net for zone in board.zones}
    from .hard_macros import macro_owned_pads
    owned = macro_owned_pads(board)
    pads: list[tuple[int, PadReference, Point, object, object]] = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        if len(footprint.pads) < options.minimum_component_pads:
            continue
        surface = [pad for pad in footprint.pads if pad.kind is PadKind.SMD]
        for pad in surface:
            reference = PadReference(placement.reference, pad.number)
            net = net_by_pad.get(reference)
            if (reference in owned or net is None or net in zone_nets
                    or only_nets is not None and net not in only_nets):
                continue
            rule = rules.get(net)
            if rule is not None and rule.kind is not RouteKind.GENERAL:
                continue
            position = transformed_pad_position(board, placement, pad.number)
            neighbor = min((abs(position.x_nm - other.x_nm) + abs(position.y_nm - other.y_nm)
                            for other_pad in surface if other_pad.number != pad.number
                            for other in (transformed_pad_position(
                                board, placement, other_pad.number),)), default=10**18)
            if neighbor > options.maximum_neighbor_distance_nm:
                continue
            pads.append((neighbor, reference, position, placement, pad))

    # Analyze every pin against the same immutable input before reserving an
    # easy neighbor's escape. Domains are physically legal alternatives, not
    # guaranteed mutually compatible routes or proof of onward connectivity.
    domains: dict[PadReference, tuple[EscapeCandidate, ...]] = {}
    pin_by_ref = {item[1]: item for item in pads}
    for _, reference, position, placement, pad in pads:
        net = net_by_pad[reference]
        choices = _legal_choices(board, clearance, net, position, placement, pad,
                                 _candidates(position, placement.position, options))
        if options.two_leg_escapes and not choices:
            choices = _legal_choices(board, clearance, net, position, placement, pad,
                                    _two_leg_candidates(position, placement.position, options))
        domains[reference] = choices

    ordered = sorted(pads, key=lambda item: (
        len(domains[item[1]]) if options.constrained_pins_first else 0,
        item[0], item[1],
    ))
    ordered_refs = tuple(item[1] for item in ordered)
    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    selected: dict[PadReference, int] = {}
    for _, reference, _, _, _ in ordered:
        for index, candidate in enumerate(domains[reference]):
            if _reserve(board, clearance, tracks, vias, candidate):
                selected[reference] = index
                break
    greedy = dict(selected)
    assignment = None
    if options.joint_escapes:
        def expand(reference):
            if not options.two_leg_escapes:
                return ()
            _, _, position, placement, pad = pin_by_ref[reference]
            return _legal_choices(board, RoutingClearanceIndex(board), net_by_pad[reference],
                position, placement, pad, _two_leg_candidates(position, placement.position, options),
                multiple_orders=True)
        selected, assignment = improve_escape_assignment(
            board, ordered_refs, domains, selected, expand, options.assignment_options)

    def materialize(selection):
        index = RoutingClearanceIndex(board)
        new_tracks, new_vias, accesses = [], [], {}
        for reference in ordered_refs:
            if reference in selection:
                candidate = domains[reference][selection[reference]]
                if _reserve(board, index, new_tracks, new_vias, candidate):
                    accesses[reference] = candidate[0][-1].end
        return (replace(board, tracks=(*board.tracks, *new_tracks), vias=(*board.vias, *new_vias))
                if new_tracks or new_vias else board), new_tracks, new_vias, accesses

    routed, tracks, vias, accesses = materialize(selected)
    # Geometry queries are the fast gate; native DRC is the final transactional
    # gate. A new manufacturing violation rejects the whole fanout proposal.
    fatal = {"DRC-SHORT", "DRC-CLEARANCE", "DRC-BOARD-EDGE", "DRC-HOLE-CLEARANCE",
             "DRC-DRILL-SPACING", "DRC-COPPER-KEEPOUT", "DRC-VIA-SPAN",
             "DRC-TRACK-WIDTH", "DRC-VIA-PAD-OVERLAP"}
    before = run_physical_drc(board)
    after = run_physical_drc(routed)
    before_count = {code: sum(item.code == code for item in before.findings) for code in fatal}
    def failed_gate(findings):
        return any(sum(item.code == code for item in findings) > before_count[code] for code in fatal)
    # Pairwise assignment is a proposal, not a substitute for exact whole-board
    # materialization/native acceptance. A failed improvement retains greedy.
    if assignment is not None and (set(accesses) != set(selected) or failed_gate(after.findings)):
        selected = greedy
        assignment = replace(assignment, native_accepted=False)
        routed, tracks, vias, accesses = materialize(selected)
        after = run_physical_drc(routed)
    pending = tuple(reference for reference in ordered_refs if reference not in accesses)
    analysis = tuple(FanoutPinAnalysis(reference, len(domains[reference]),
        selected.get(reference) if reference in accesses else None,
        ("no legal candidate against immutable input" if not domains[reference]
         else "all initial candidates blocked by selected escapes") if reference not in accesses else "",
        sum(len(path) == 2 for path, _ in domains[reference])) for reference in ordered_refs)
    if failed_gate(after.findings):
        return FanoutResult(board, MappingProxyType({}),
                            tuple(item[1] for item in pads), 0, 0, pin_analysis=tuple(
                                replace(item, selected_candidate_index=None,
                                        diagnostic="whole fanout proposal rejected by native DRC")
                                for item in analysis), assignment=assignment)
    return FanoutResult(routed, MappingProxyType(accesses), tuple(pending),
                        len(tracks), len(vias), tuple(vias), tuple(analysis), tuple(tracks), assignment)


def _legal_choices(board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
                   position: Point, placement: Placement, pad: FootprintPad,
                   endpoints, *, multiple_orders: bool = False) -> tuple[EscapeCandidate, ...]:
    rule = next((r for r in board.net_routing_rules if r.net == net), None)
    side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    allowed = routing_layers(board, net, rule)
    if side not in allowed or len(allowed) < 2:
        return ()
    span = physical_via_span(board, side, next(layer for layer in allowed if layer is not side))
    if span is None:
        return ()
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    size, drill = board.rules.default_via_size_nm, board.rules.default_via_drill_nm
    bounds = placed_pad_shape(position, pad, placement).bounds
    choices = []
    for endpoint in endpoints:
        margin = size // 2 + board.rules.minimum_clearance_nm
        if (not point_in_polygon(endpoint, board.outline.vertices)
                or _distance_to_outline(endpoint, board.outline.vertices) < margin):
            continue
        if (bounds.min_x - size // 2 <= endpoint.x_nm <= bounds.max_x + size // 2
                and bounds.min_y - size // 2 <= endpoint.y_nm <= bounds.max_y + size // 2):
            continue  # No unqualified ordinary via-in-pad process.
        coincident = tuple(v for v in board.vias if v.position == endpoint)
        existing = next((v for v in coincident if v.net == net
                         and (v.from_layer, v.to_layer) == span[:2]), None)
        if coincident and existing is None:
            continue
        via = None if existing else Via(net, endpoint, size, drill, *span)
        if via is not None and not clearance.can_via(net, endpoint, size, span[0], span[1], drill_nm=drill):
            continue
        paths = (checked_access_paths(board, clearance, net, position, endpoint, width, side,
                                     allow_orthogonal=False) if multiple_orders else
                 (checked_access_path(board, clearance, net, position, endpoint, width, side,
                                      allow_orthogonal=False),))
        choices.extend((path, via) for path in paths if path)
    return tuple(dict.fromkeys(choices))


def _reserve(board, clearance, tracks, vias, candidate: EscapeCandidate) -> bool:
    path, via = candidate
    if via is not None and any(v.net == via.net and v.position == via.position
                              and (v.from_layer, v.to_layer) == (via.from_layer, via.to_layer)
                              for v in (*board.vias, *vias)):
        via = None
    if (not all(clearance.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in path)
            or via is not None and not clearance.can_via(via.net, via.position, via.size_nm,
                via.from_layer, via.to_layer, drill_nm=via.drill_nm)):
        return False
    for track in path:
        if track not in board.tracks and track not in tracks:
            tracks.append(track)
            clearance.add_track(track, locked=True)
    if via is not None:
        vias.append(via)
        clearance.add_via(via, locked=True)
    return True


def _candidates(position: Point, center: Point, options: FanoutOptions):
    directions = ((1, 0), (-1, 0), (0, 1), (0, -1),
                  (1, 1), (1, -1), (-1, 1), (-1, -1))
    directions = sorted(directions, key=lambda pair: (
        -(pair[0] * (position.x_nm - center.x_nm)
          + pair[1] * (position.y_nm - center.y_nm)), pair,
    ))
    for step in range(1, options.maximum_radius_nm // options.step_nm + 1):
        for dx, dy in directions:
            yield Point(position.x_nm + dx * step * options.step_nm,
                        position.y_nm + dy * step * options.step_nm)


def _distance_to_outline(point: Point, vertices: tuple[Point, ...]) -> float:
    from .geometry import point_segment_distance_squared
    return min(float(point_segment_distance_squared(point, first, second)) ** 0.5
               for first, second in zip(vertices, (*vertices[1:], vertices[0])))


def _two_leg_candidates(position: Point, center: Point, options: FanoutOptions):
    """Bounded off-ray sites, in near-to-far shells on a half-step lattice."""
    step_nm = max(1, options.step_nm // 2)
    emitted = 0
    for ring in range(1, options.maximum_radius_nm // step_nm + 1):
        offsets = set()
        for along in range(-ring + 1, ring):
            for dx, dy in ((ring, along), (-ring, along), (along, ring), (along, -ring)):
                if dx and dy and abs(dx) != abs(dy):
                    offsets.add((dx, dy))
        for dx, dy in sorted(offsets, key=lambda pair: (
            pair[0] * pair[0] + pair[1] * pair[1],
            -(pair[0] * (position.x_nm - center.x_nm)
              + pair[1] * (position.y_nm - center.y_nm)), pair,
        )):
            if emitted >= options.maximum_two_leg_candidates:
                return
            emitted += 1
            yield Point(position.x_nm + dx * step_nm, position.y_nm + dy * step_nm)

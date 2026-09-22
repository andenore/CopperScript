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

from .physical import (
    CopperLayer,
    NetRoutingRule,
    PhysicalBoard,
    Point,
    RouteKind,
    TrackSegment,
    Via,
)
from .routing import GlobalNetRoute, GlobalRoutingResult


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
                }
                for item in self.nets
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


def route_critical_nets(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
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
                continue
            result, pair_tracks, pair_vias = _route_pair(
                board, rule, partner_rule, routes
            )
            processed.update((rule.net, partner_name))
            coupled_pairs.add(frozenset((rule.net, partner_name)))
            results.append(result)
            tracks.extend(pair_tracks)
            vias.extend(pair_vias)
        else:
            result, net_tracks, net_vias = _route_single(
                board, rule, routes.get(rule.net)
            )
            processed.add(rule.net)
            results.append(result)
            tracks.extend(net_tracks)
            vias.extend(net_vias)

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
    vias = tuple(
        Via(
            rule.net,
            item.position,
            board.rules.default_via_size_nm,
            board.rules.default_via_drill_nm,
            item.from_layer,
            item.to_layer,
        )
        for item in guide.vias
    )
    diagnostics = _budget_diagnostics(rule, tuple(tracks), vias)
    assumptions = _external_assumptions(rule)
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
        ),
        tuple(tracks),
        vias,
    )


def _route_pair(
    board: PhysicalBoard,
    first_rule: NetRoutingRule,
    second_rule: NetRoutingRule,
    routes: dict[str, GlobalNetRoute],
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
    positive, negative, junctions = _offset_guides(guide, offset)
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
    first_tracks.extend(
        _pair_pin_stubs(first.net, guide, first_width, positive, 1, first_width + gap)
    )
    second_tracks.extend(
        _pair_pin_stubs(
            second.net,
            partner_guide,
            second_width,
            negative,
            0,
            second_width + gap,
        )
    )
    pair_vias: list[Via] = []
    for item in guide.vias:
        for net, sign in ((first.net, 1), (second.net, -1)):
            pair_vias.append(
                Via(
                    net,
                    Point(item.position.x_nm + sign * offset, item.position.y_nm),
                    board.rules.default_via_size_nm,
                    board.rules.default_via_drill_nm,
                    item.from_layer,
                    item.to_layer,
                )
            )
    first_length = _track_length(tuple(first_tracks))
    second_length = _track_length(tuple(second_tracks))
    skew = abs(first_length - second_length)
    diagnostics = list(
        _budget_diagnostics(first, tuple(first_tracks), tuple(v for v in pair_vias if v.net == first.net))
    )
    diagnostics.extend(
        _budget_diagnostics(second, tuple(second_tracks), tuple(v for v in pair_vias if v.net == second.net))
    )
    max_skew = min(
        value
        for value in (first.max_skew_nm, second.max_skew_nm)
        if value is not None
    ) if first.max_skew_nm is not None or second.max_skew_nm is not None else None
    if max_skew is not None and skew > max_skew:
        diagnostics.append(f"pair skew {skew} nm exceeds {max_skew} nm")
    assumptions = tuple(
        dict.fromkeys(
            (
                *_external_assumptions(first),
                *_external_assumptions(second),
                "coupled pin fanout remains subject to authoritative physical DRC",
            )
        )
    )
    tracks = tuple((*first_tracks, *second_tracks))
    return (
        CriticalNetResult(
            (first.net, second.net),
            not diagnostics,
            len(tracks),
            len(pair_vias),
            (first_length, second_length),
            skew,
            tuple(diagnostics),
            assumptions,
        ),
        tracks,
        tuple(pair_vias),
    )


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
        if access.pad_position != access.access_position:
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
    if rule.target_impedance_ohms is not None:
        assumptions.append(
            f"{rule.target_impedance_ohms} ohm impedance requires external stackup/field-solver qualification"
        )
    if rule.kind is RouteKind.RF_FEED:
        assumptions.append("RF feed and antenna performance require simulation and physical validation")
    if rule.kind is RouteKind.POWER:
        assumptions.append("power-route current and thermal capacity require external validation")
    return tuple(assumptions)


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
            (item.nets, item.connected, item.lengths_nm, item.skew_nm, item.diagnostics, item.assumptions)
            for item in results
        ],
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()

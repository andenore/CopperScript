"""Routed critical-lane review (D-PHY plan D6).

A report-only review of the routed copper of every critical net against common
high-speed layout guidance, so lanes can be checked from the route report
alone. It reads a board and never changes copper.

* Per net: the lane-table row (``critical_lane_table``): routed length from
  tracks only (via barrels excluded, as length matching measures it), layers
  and the net's own via count.
* Per pair: the intra-pair skew against the pair's ``max_skew`` (the smaller
  member value, as physical DRC). Per ``length_match`` group: its skew (longest
  minus shortest member) against ``max_skew`` (``verify_match_groups``), with
  the group's tuning outcome.
* Spacing: the least edge-to-edge distance from the net's tracks and vias to
  copper of another net on a shared layer (tracks, vias and pads). It is split
  by where the net's own copper is, inside one of the net's breakout regions
  (``BreakoutRegions.region``) or outside, and by the neighbour's class:
  ``critical`` (a net with a non-general routing rule) or ``signal`` (any
  other net). The pair partner, nets that own a copper zone (planes and pours)
  and pads without a net are ignored. Neighbours come from
  ``RoutingClearanceIndex`` within ``LANE_REVIEW_RADIUS_NM``; a class with no
  copper that near is ``None``.
* Coupled length: the length of a pair member's tracks outside its breakout
  regions that lies closer than ``COUPLING_FACTOR`` x the pair's ``pair_gap``,
  edge to edge, to copper of another critical pair.
* Bends: the direction change where exactly two of the net's tracks meet on one
  layer; a junction of three or more is a branch, not a bend. Bends sharper
  than ``SHARP_BEND_DEGREES`` are listed with their locations.
"""

from __future__ import annotations

from math import atan2, ceil, degrees, hypot, inf, sqrt
from typing import Iterable, Sequence

from .critical import CriticalNetResult, critical_lane_table
from .critical_tuning import MatchTuningResult
from .geometry import (RoundedConvexShape, point_in_polygon, segments_intersect,
                       shape_distance_squared, shapes_clear)
from .physical import CopperLayer, PhysicalBoard, Point, RouteKind, TrackSegment, nm_from_mm
from .routing_clearance import RoutingClearanceIndex
from .signal_integrity import verify_match_groups


# Spacing search radius; farther copper is not reported.
LANE_REVIEW_RADIUS_NM = nm_from_mm(1)
# Pair-to-pair guidance: at least this multiple of the pair gap, edge to edge.
COUPLING_FACTOR = 2
SHARP_BEND_DEGREES = 45

_PAIR_KINDS = frozenset({RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS})
_CLASSES = ("critical", "signal")


def critical_lane_review(
    board: PhysicalBoard, results: Iterable[CriticalNetResult],
    match_tuning: Sequence[MatchTuningResult] = (),
) -> dict[str, object]:
    """JSON-ready review of the critical copper on ``board``.

    ``results`` are the critical stage's group results: they name the reviewed
    nets and whether each group connected. ``match_tuning`` gives each
    ``length_match`` group's tuning outcome.
    """
    results = tuple(results)
    lanes = critical_lane_table(board, results)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    critical = frozenset(net for net, rule in rules.items() if rule.kind is not RouteKind.GENERAL)
    zone_nets = frozenset(zone.net for zone in board.zones)

    def partner_of(net: str) -> str | None:
        rule = rules.get(net)
        if rule is None or rule.kind not in _PAIR_KINDS or rule.differential_partner is None:
            return None
        other = rules.get(rule.differential_partner)
        return rule.differential_partner if other is not None and other.differential_partner == net else None

    paired = frozenset(net for net in rules if partner_of(net) is not None)
    nets: list[dict[str, object]] = []
    if lanes:
        index = RoutingClearanceIndex(board)
        order = board.stackup.copper_layers
        reviewed = {lane["net"] for lane in lanes}
        # Copper shapes of each reviewed net: its tracks, in ``tracks`` order, then its vias.
        tracks: dict[str, list[TrackSegment]] = {}
        for track in board.tracks:
            if track.net in reviewed:
                tracks.setdefault(track.net, []).append(track)
        copper: dict[str, list[tuple[RoundedConvexShape, tuple[CopperLayer, ...]]]] = {
            net: [(RoundedConvexShape((t.start, t.end), t.width_nm // 2), (t.layer,)) for t in items]
            for net, items in tracks.items()
        }
        for via in board.vias:
            if via.net in reviewed:
                first, last = sorted((order.index(via.from_layer), order.index(via.to_layer)))
                copper.setdefault(via.net, []).append(
                    (RoundedConvexShape((via.position,), via.size_nm // 2), order[first:last + 1]))
        for lane in lanes:
            net = str(lane["net"])
            partner = partner_of(net)
            ignored = zone_nets | {net, partner}
            outside = [(track, shape) for track, (shape, _) in zip(tracks.get(net, ()), copper.get(net, ()))
                       if index.breakout.region(net, shape.spine) is None]
            gap = rules[net].pair_gap_nm if partner is not None else None
            threshold = COUPLING_FACTOR * gap if gap else None
            coupled, coupled_nets = (_coupled_length(index, outside, paired - {net, partner}, threshold)
                                     if threshold is not None else (None, set()))
            nets.append({
                **lane,
                "partner": partner,
                "breakout": net in index.breakout.declared,
                "spacing": _spacing(index, net, copper.get(net, ()), ignored, critical),
                "coupling_threshold_nm": threshold,
                "coupled_length_nm": coupled,
                "coupled_nets": sorted(coupled_nets),
                "bends": _bends(tracks.get(net, ())),
            })
    lengths = {str(lane["net"]): int(lane["routed_length_nm"]) for lane in lanes}
    pairs = []
    for item in results:
        if len(item.nets) != 2 or partner_of(item.nets[0]) != item.nets[1]:
            continue
        limits = [value for value in (rules[net].max_skew_nm for net in item.nets) if value is not None]
        limit = min(limits) if limits else None
        skew = abs(lengths[item.nets[0]] - lengths[item.nets[1]])
        pairs.append({
            "nets": list(item.nets),
            "lengths_nm": [lengths[net] for net in item.nets],
            "skew_nm": skew,
            "max_skew_nm": limit,
            "status": ("incomplete" if not item.connected else "no_limit" if limit is None
                       else "pass" if skew <= limit else "fail"),
        })
    tuning = {item.id: item.status for item in match_tuning}
    return {
        "search_radius_nm": LANE_REVIEW_RADIUS_NM,
        "coupling_factor": COUPLING_FACTOR,
        "sharp_bend_degrees": SHARP_BEND_DEGREES,
        "ignored_zone_nets": sorted(zone_nets),
        "nets": nets,
        "pairs": pairs,
        "match_groups": [{**check.to_dict(), "tuning_status": tuning.get(check.id)}
                         for check in verify_match_groups(board)],
    }


def lane_review_line(review: dict[str, object]) -> str:
    """One console line summarising a review (route-board and the critical preflight)."""
    def mm(value: int) -> str:
        return f"{value / 1e6:.3f} mm"

    nets, pairs, groups = review["nets"], review["pairs"], review["match_groups"]
    worst = max((pair["skew_nm"] for pair in pairs), default=None)
    spacing = []
    for kind in _CLASSES:
        found = [(item["spacing"]["outside_breakout"][kind], item["net"]) for item in nets
                 if item["spacing"]["outside_breakout"][kind] is not None]
        if found:
            least, net = min(found, key=lambda entry: entry[0]["distance_nm"])
            spacing.append(f"{kind}={mm(least['distance_nm'])} ({net} to {least['neighbour']})")
        else:
            spacing.append(f"{kind}=none within {mm(review['search_radius_nm'])}")
    sharpest = max((item["bends"]["sharpest_degrees"] for item in nets
                    if item["bends"]["sharpest_degrees"] is not None), default=None)
    return "; ".join((
        f"CRITICAL LANES: {len(nets)} nets",
        f"pairs={len(pairs)}, worst skew={'n/a' if worst is None else mm(worst)}, "
        f"over max_skew={sum(pair['status'] == 'fail' for pair in pairs)}",
        f"match groups={len(groups)}, over max_skew={sum(group['status'] == 'fail' for group in groups)}",
        "min spacing outside breakout: " + ", ".join(spacing),
        f"coupled length={mm(sum(item['coupled_length_nm'] or 0 for item in nets))}",
        f"bends>{review['sharp_bend_degrees']}deg={sum(item['bends']['sharp_count'] for item in nets)}"
        + (f" (sharpest {sharpest} deg)" if sharpest is not None else ""),
    ))


def _spacing(
    index: RoutingClearanceIndex, net: str,
    own: Iterable[tuple[RoundedConvexShape, tuple[CopperLayer, ...]]],
    ignored: frozenset[str | None], critical: frozenset[str],
) -> dict[str, dict[str, dict[str, object] | None]]:
    """Least edge-to-edge distance per own-copper region and neighbour class."""
    best: dict[tuple[str, str], tuple[float, object, RoundedConvexShape, tuple[CopperLayer, ...]]] = {}
    for shape, layers in own:
        region = "outside_breakout" if index.breakout.region(net, shape.spine) is None else "inside_breakout"
        for other in index.nearby_copper(shape, layers, LANE_REVIEW_RADIUS_NM):
            if other.net in ignored or other.net.startswith("<"):
                continue
            key = (region, "critical" if other.net in critical else "signal")
            limit = best[key][0] if key in best else LANE_REVIEW_RADIUS_NM
            # Exact integer pruning first; only a possibly nearer object is measured.
            if shapes_clear(shape, other.shape, ceil(limit)):
                continue
            distance = (sqrt(shape_distance_squared(shape, other.shape))
                        - shape.radius_nm - other.shape.radius_nm)
            if distance < limit:
                best[key] = (distance, other, shape, layers)
    document: dict[str, dict[str, dict[str, object] | None]] = {}
    for region in ("inside_breakout", "outside_breakout"):
        document[region] = {}
        for kind in _CLASSES:
            if (region, kind) not in best:
                document[region][kind] = None
                continue
            distance, other, shape, layers = best[region, kind]
            pad = other.pad_reference if other.is_pad else None
            document[region][kind] = {
                "distance_nm": max(0, round(distance)),
                "neighbour": other.net,
                "object": "pad" if other.is_pad else "via" if len(other.shape.spine) == 1 else "track",
                "pad": f"{pad.component}.{pad.pad}" if pad is not None else None,
                "layer": next(layer for layer in layers if layer in other.layers).value,
                "at_nm": _nearest_point(shape.spine, other.shape.spine),
            }
    return document


def _coupled_length(
    index: RoutingClearanceIndex, tracks: Sequence[tuple[TrackSegment, RoundedConvexShape]],
    neighbours: frozenset[str], threshold_nm: int,
) -> tuple[int, set[str]]:
    """Length of ``tracks`` closer than ``threshold_nm`` (edge to edge) to copper of ``neighbours``."""
    total = 0.0
    nets: set[str] = set()
    for track, shape in tracks:
        intervals = []
        for other in index.nearby_copper(shape, (track.layer,), threshold_nm):
            if other.net not in neighbours or shapes_clear(shape, other.shape, threshold_nm):
                continue
            reach = threshold_nm + shape.radius_nm + other.shape.radius_nm
            interval = _reach_interval(track.start, track.end, other.shape.spine, reach)
            if interval is not None:
                intervals.append(interval)
                nets.add(other.net)
        reached = -inf
        for low, high in sorted(intervals):
            if high > reached:
                total += high - max(low, reached)
                reached = high
    return round(total), nets


def _bends(tracks: Sequence[TrackSegment]) -> dict[str, object]:
    """Direction changes where exactly two tracks meet on one layer."""
    far_ends: dict[tuple[CopperLayer, Point], list[Point]] = {}
    for track in tracks:
        if track.start != track.end:
            far_ends.setdefault((track.layer, track.start), []).append(track.end)
            far_ends.setdefault((track.layer, track.end), []).append(track.start)
    sharpest: float | None = None
    sharp: list[dict[str, object]] = []
    for (layer, node), ends in far_ends.items():
        if len(ends) != 2:
            continue
        ux, uy = node.x_nm - ends[0].x_nm, node.y_nm - ends[0].y_nm
        vx, vy = ends[1].x_nm - node.x_nm, ends[1].y_nm - node.y_nm
        angle = round(degrees(atan2(abs(ux * vy - uy * vx), ux * vx + uy * vy)), 2)
        sharpest = angle if sharpest is None else max(sharpest, angle)
        if angle > SHARP_BEND_DEGREES:
            sharp.append({"at_nm": [node.x_nm, node.y_nm], "layer": layer.value, "degrees": angle})
    return {"sharpest_degrees": sharpest, "sharp_count": len(sharp), "sharp": sharp}


def _edges(spine: tuple[Point, ...]) -> tuple[tuple[Point, Point], ...]:
    if len(spine) <= 2:
        return ((spine[0], spine[-1]),)
    return tuple(zip(spine, (*spine[1:], spine[0])))


def _project(point: Point, start: Point, end: Point) -> tuple[float, float]:
    """The point of segment ``start``-``end`` nearest ``point``."""
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    length_squared = dx * dx + dy * dy
    if length_squared == 0:
        return float(start.x_nm), float(start.y_nm)
    t = min(1.0, max(0.0, ((point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy) / length_squared))
    return start.x_nm + t * dx, start.y_nm + t * dy


def _nearest_point(own: tuple[Point, ...], other: tuple[Point, ...]) -> list[int]:
    """The point of ``own`` (a via centre or track centreline) nearest ``other``'s spine."""
    a, b = own[0], own[-1]
    if len(other) >= 3 and point_in_polygon(a, other):
        return [a.x_nm, a.y_nm]
    best: tuple[float, tuple[float, float]] | None = None
    for c, d in _edges(other):
        if a != b and c != d and segments_intersect(a, b, c, d):
            rx, ry, sx, sy = b.x_nm - a.x_nm, b.y_nm - a.y_nm, d.x_nm - c.x_nm, d.y_nm - c.y_nm
            denominator = rx * sy - ry * sx
            if denominator:
                t = ((c.x_nm - a.x_nm) * sy - (c.y_nm - a.y_nm) * sx) / denominator
                return [round(a.x_nm + t * rx), round(a.y_nm + t * ry)]
        for mine, theirs in (((a.x_nm, a.y_nm), _project(a, c, d)), ((b.x_nm, b.y_nm), _project(b, c, d)),
                             (_project(c, a, b), (c.x_nm, c.y_nm)), (_project(d, a, b), (d.x_nm, d.y_nm))):
            distance = hypot(mine[0] - theirs[0], mine[1] - theirs[1])
            if best is None or distance < best[0]:
                best = (distance, mine)
    assert best is not None
    return [round(best[1][0]), round(best[1][1])]


def _reach_interval(start: Point, end: Point, spine: tuple[Point, ...],
                    reach: float) -> tuple[float, float] | None:
    """Positions along ``start``-``end`` (from ``start``) within ``reach`` of a convex spine.

    The reach of a convex spine is convex, so the positions form one interval:
    the hull of those within reach of each spine edge (two end disks and a band).
    """
    length = hypot(end.x_nm - start.x_nm, end.y_nm - start.y_nm)
    if length == 0:
        return None
    ux, uy = (end.x_nm - start.x_nm) / length, (end.y_nm - start.y_nm) / length
    low, high = inf, -inf
    for c, d in _edges(spine):
        pieces = [_disk(start, ux, uy, c, reach), _disk(start, ux, uy, d, reach)]
        size = hypot(d.x_nm - c.x_nm, d.y_nm - c.y_nm)
        if size:
            ex, ey = (d.x_nm - c.x_nm) / size, (d.y_nm - c.y_nm) / size
            wx, wy = start.x_nm - c.x_nm, start.y_nm - c.y_nm
            along = _slab(wx * ex + wy * ey, ux * ex + uy * ey, 0, size)
            across = _slab(wx * -ey + wy * ex, ux * -ey + uy * ex, -reach, reach)
            pieces.append((max(along[0], across[0]), min(along[1], across[1])))
        for piece_low, piece_high in pieces:
            if piece_low <= piece_high:
                low, high = min(low, piece_low), max(high, piece_high)
    low, high = max(low, 0.0), min(high, length)
    return (low, high) if low < high else None


def _disk(start: Point, ux: float, uy: float, centre: Point, radius: float) -> tuple[float, float]:
    wx, wy = start.x_nm - centre.x_nm, start.y_nm - centre.y_nm
    middle = -(ux * wx + uy * wy)
    discriminant = middle * middle - (wx * wx + wy * wy - radius * radius)
    if discriminant < 0:
        return inf, -inf
    root = sqrt(discriminant)
    return middle - root, middle + root


def _slab(offset: float, rate: float, low: float, high: float) -> tuple[float, float]:
    """Positions ``s`` with ``low <= offset + rate * s <= high``."""
    if rate == 0:
        return (-inf, inf) if low <= offset <= high else (inf, -inf)
    first, second = (low - offset) / rate, (high - offset) / rate
    return min(first, second), max(first, second)

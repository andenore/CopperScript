"""Conservative RF reference screening on ACTUAL native filled polygons.

This does not establish plane electrical connectivity, characteristic impedance
or exported-Gerber fidelity. Missing support or fill is explicitly incomplete.
"""
from __future__ import annotations

from math import hypot
from bisect import bisect_right
from .qualification import check
from .power_integrity import number


class _EdgeYIndex:
    """Exact half-open Y-interval index for the existing even/odd ray test.

    Original oriented edges and the original crossing arithmetic are retained.
    Horizontal edges never cross a horizontal ray. Selecting a median lower
    endpoint guarantees progress even with repeated coordinates or tiny spans.
    """
    __slots__ = ('center','low','high','lower','upper','left','right')

    def __init__(self, edges):
        records = [(min(a[1],b[1]),max(a[1],b[1]),(a,b)) for a,b in edges if a[1]!=b[1]]
        self.center = None
        if not records:
            return
        self.center = sorted(record[0] for record in records)[len(records)//2]
        left, right, crossing = [], [], []
        for low, high, edge in records:
            if high <= self.center:
                left.append(edge)
            elif low > self.center:
                right.append(edge)
            else:
                crossing.append((low,high,edge))
        lower = sorted(crossing,key=lambda record:record[0])
        upper = sorted(crossing,key=lambda record:record[1])
        self.lower = tuple(record[0] for record in lower)
        self.upper = tuple(record[1] for record in upper)
        self.low = tuple(record[2] for record in lower)
        self.high = tuple(record[2] for record in upper)
        self.left = _EdgeYIndex(left) if left else None
        self.right = _EdgeYIndex(right) if right else None

    def at(self, y):
        node = self
        while node is not None and node.center is not None:
            if y < node.center:
                yield from node.low[:bisect_right(node.lower,y)]
                node = node.left
            else:
                yield from node.high[bisect_right(node.upper,y):]
                node = node.right if y > node.center else None


def native_fill_polygon(fill):
    """Decode exact reversed-edge hole bridges without healing arbitrary shapes.

    KiCad can fracture a hole into an outer walk containing a zero-width slit.
    Remove ONLY identical reversed edge pairs, polygonize the unchanged remaining
    linework, and retain faces using the original walk's even/odd fill rule.
    Reject crossings, dropped linework and all other invalid representations.
    """
    from collections import Counter
    from shapely.geometry import Polygon, LineString
    from shapely.ops import polygonize, unary_union
    shape = Polygon(fill["outer"], fill["holes"])
    if shape.is_valid and not shape.is_empty:
        return shape
    if fill["holes"]:
        raise ValueError("unsupported invalid polygon with explicit holes")
    vertices = [tuple(point) for point in fill["outer"]]
    edges = [(a, b) for a, b in zip(vertices, vertices[1:] + vertices[:1]) if a != b]
    counts = Counter(edges)
    bridges = {edge for edge, count in counts.items() if count == 1 and counts.get(edge[::-1]) == 1}
    if not bridges or any(count != 1 for count in counts.values()):
        raise ValueError("not a reversible native hole bridge")
    lines = [LineString(edge) for edge in edges if edge not in bridges]
    faces = list(polygonize(lines))
    if not faces or any(not face.is_valid for face in faces):
        raise ValueError("native hole bridge does not form valid faces")
    linework = unary_union(lines)
    if not linework.difference(unary_union([face.boundary for face in faces])).is_empty:
        raise ValueError("native hole bridge has dropped/crossing linework")
    crossing_edges = _EdgeYIndex(edges)
    def inside(point):
        x, y, result = point.x, point.y, False
        for a, b in crossing_edges.at(y):
            if (a[1] > y) != (b[1] > y) and x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]:
                result = not result
        return result
    selected = [face for face in faces if inside(face.representative_point())]
    if not selected:
        raise ValueError("empty native filled faces")
    return unary_union(selected)


def reference_coverage(native: dict, *, identifier: str, net: str, reference_net: str,
                       reference_layer: str, margin_nm: int) -> dict:
    try:
        from shapely.geometry import Polygon, LineString
        from shapely.ops import unary_union
    except ImportError:
        return check(identifier, "incomplete", "Install copperscript[qualification] for vector geometry")
    if type(margin_nm) is not int or margin_nm < 0:
        raise ValueError("reference margin_nm must be a nonnegative integer")
    layers = native["layers"]
    if len(layers) != len(set(layers)) or reference_layer not in layers:
        raise ValueError("invalid reference layer/order")
    tracks = [track for track in native["tracks"] if track["net"] == net]
    if not tracks:
        return check(identifier, "incomplete", f"No supported straight tracks for {net}")
    if any(track["layer"] not in layers or
           abs(layers.index(track["layer"]) - layers.index(reference_layer)) != 1 for track in tracks):
        return check(identifier, "fail", "Signal tracks do not share the declared adjacent reference")
    fills = [fill for fill in native["fills"] if fill["net"] == reference_net
             and fill["layer"] == reference_layer]
    if not fills:
        return check(identifier, "incomplete", "No native filled reference copper; zone outlines are not evidence")
    try:
        polygons = [native_fill_polygon(fill) for fill in fills]
    except ValueError:
        # Native slit-fractured holes can be invalid simple polygons; do NOT heal
        # them with buffer(0) and accidentally erase a slit or small keepout.
        return check(identifier, "incomplete", "Unsupported/invalid filled polygon; no automatic healing")
    copper = unary_union(polygons)
    uncovered = []
    length = 0.0
    for index, track in enumerate(tracks):
        width = number(track["width_nm"], "track width")
        start, end = track["start"], track["end"]
        if start == end:
            raise ValueError("zero-length RF track")
        length += hypot(end[0] - start[0], end[1] - start[1])
        # Square caps/miter joins conservatively contain the rounded conductor.
        envelope = LineString([start, end]).buffer(width / 2 + margin_nm, cap_style=3, join_style=2)
        if not copper.covers(envelope):
            uncovered.append(index)
    return check(identifier, "fail" if uncovered else "pass",
                 "Projected conductor coverage only; no electrical/impedance proof",
                 unsupported_segment_indices=uncovered, track_count=len(tracks),
                 total_segment_length_nm=length, reference_layer=reference_layer, margin_nm=margin_nm)

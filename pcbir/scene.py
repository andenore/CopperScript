"""A small, read-only geometry scene for looking at boards.

A scene is what ``copper view`` draws and what ``copper issues`` measures
against: pads, tracks, vias, copper areas and board edges in millimetres. It is
built either from a saved ``.kicad_pcb`` (any generator) or from an in-memory
:class:`~pcbir.physical.PhysicalBoard` (for intermediate solver states).

This is a viewer model, not an importer: it does not validate the board, does
not refill zones and carries no qualification meaning. Unfilled zones are kept
as outlines so they are never mistaken for copper.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from math import cos, radians, sin
from pathlib import Path

Point = tuple[float, float]


@dataclass(frozen=True, slots=True)
class ScenePad:
    ref: str
    pad: str
    net: str
    layers: tuple[str, ...]
    polygon: tuple[Point, ...]
    center: Point
    drill: float = 0.0


@dataclass(frozen=True, slots=True)
class SceneTrack:
    net: str
    layer: str
    start: Point
    end: Point
    width: float


@dataclass(frozen=True, slots=True)
class SceneVia:
    net: str
    center: Point
    size: float
    drill: float
    layers: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class SceneArea:
    """Copper area. ``kind`` is ``fill`` (real copper) or ``zone`` (outline only)."""

    net: str
    layer: str
    outer: tuple[Point, ...]
    holes: tuple[tuple[Point, ...], ...] = ()
    kind: str = "fill"


@dataclass(frozen=True, slots=True)
class SceneFootprint:
    ref: str
    center: Point
    rotation: float
    side: str  # "front" | "back"


@dataclass(slots=True)
class Scene:
    name: str
    copper_layers: tuple[str, ...]
    pads: list[ScenePad] = field(default_factory=list)
    tracks: list[SceneTrack] = field(default_factory=list)
    vias: list[SceneVia] = field(default_factory=list)
    areas: list[SceneArea] = field(default_factory=list)
    edges: list[tuple[Point, Point]] = field(default_factory=list)
    footprints: dict[str, SceneFootprint] = field(default_factory=dict)
    source: str = ""

    def pad(self, ref: str, pad: str) -> list[ScenePad]:
        return [item for item in self.pads if item.ref == ref and item.pad == pad]

    def bounds(self) -> tuple[float, float, float, float]:
        xs, ys = [], []
        for a, b in self.edges:
            xs += [a[0], b[0]]
            ys += [a[1], b[1]]
        if not xs:
            for item in self.pads:
                xs.append(item.center[0])
                ys.append(item.center[1])
        if not xs:
            return (0.0, 0.0, 1.0, 1.0)
        return (min(xs), min(ys), max(xs), max(ys))


# --- shapes ---------------------------------------------------------------------------

def _rotate(x: float, y: float, degrees: float) -> Point:
    # KiCad: positive angles turn counter-clockwise on screen, with y pointing down.
    a = radians(degrees)
    return (x * cos(a) + y * sin(a), -x * sin(a) + y * cos(a))


def _arc_points(cx: float, cy: float, r: float, start: float, sweep: float, steps: int) -> list[Point]:
    return [(cx + r * cos(radians(start + sweep * i / steps)), cy + r * sin(radians(start + sweep * i / steps)))
            for i in range(steps + 1)]


def pad_polygon(shape: str, width: float, height: float, angle: float, center: Point,
                rratio: float = 0.25) -> tuple[Point, ...]:
    """Polygon for a pad; ``angle`` uses the KiCad (screen counter-clockwise) convention."""

    w, h = width / 2, height / 2
    if shape == "circle":
        local = _arc_points(0, 0, min(w, h), 0, 360, 24)[:-1]
    elif shape == "oval":
        r = min(w, h)
        if w >= h:
            run = w - r
            local = _arc_points(run, 0, r, -90, 180, 8) + _arc_points(-run, 0, r, 90, 180, 8)
        else:
            run = h - r
            local = _arc_points(0, run, r, 0, 180, 8) + _arc_points(0, -run, r, 180, 180, 8)
    elif shape == "roundrect" and rratio > 0:
        r = min(width, height) * rratio
        iw, ih = w - r, h - r
        local = (_arc_points(iw, ih, r, 0, 90, 4) + _arc_points(-iw, ih, r, 90, 90, 4)
                 + _arc_points(-iw, -ih, r, 180, 90, 4) + _arc_points(iw, -ih, r, 270, 90, 4))
    else:  # rect; trapezoid and custom pads are approximated by their anchor rectangle
        local = [(-w, -h), (w, -h), (w, h), (-w, h)]
    return tuple((center[0] + rx, center[1] + ry) for rx, ry in (_rotate(x, y, angle) for x, y in local))


def arc_points(start: Point, mid: Point, end: Point, segment_degrees: float = 10.0) -> list[Point]:
    """Points along the circular arc through start, mid and end (a chord if collinear)."""

    from math import atan2, ceil, degrees, hypot, pi
    (ax, ay), (bx, by), (cx, cy) = start, mid, end
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if abs(d) < 1e-12:
        return [start, end]
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    r = hypot(ax - ux, ay - uy)
    a0, a1, am = (atan2(p[1] - uy, p[0] - ux) for p in (start, end, mid))
    sweep = (a1 - a0) % (2 * pi)
    if (am - a0) % (2 * pi) > sweep:  # mid lies on the other way round
        sweep -= 2 * pi
    steps = max(2, ceil(abs(degrees(sweep)) / segment_degrees))
    points = [(ux + r * cos(a0 + sweep * i / steps), uy + r * sin(a0 + sweep * i / steps)) for i in range(steps + 1)]
    points[0], points[-1] = start, end
    return points


def track_polygon(start: Point, end: Point, width: float) -> tuple[Point, ...]:
    """Stadium polygon of a round-capped track."""

    from math import atan2, degrees
    r = width / 2
    heading = degrees(atan2(end[1] - start[1], end[0] - start[0]))
    return tuple(_arc_points(end[0], end[1], r, heading - 90, 180, 6)
                 + _arc_points(start[0], start[1], r, heading + 90, 180, 6))


# --- KiCad reader ---------------------------------------------------------------------

def _atoms(node, tag):
    return [child for child in node if isinstance(child, list) and child and child[0] == tag]


def _first(node, tag):
    found = _atoms(node, tag)
    return found[0] if found else None


def _xy(node) -> Point:
    return (float(node[1]), float(node[2]))


def _layer_names(node, copper: tuple[str, ...]) -> tuple[str, ...]:
    layers = _first(node, "layers") or _first(node, "layer")
    if layers is None:
        return ()
    names = []
    for value in layers[1:]:
        if not isinstance(value, str):
            continue
        if value == "*.Cu":
            names.extend(copper)
        elif value == "F&B.Cu":
            names.extend((copper[0], copper[-1]))
        elif value.endswith(".Cu"):
            names.append(value)
    return tuple(dict.fromkeys(names))


def _net_name(node, nets: dict[str, str]) -> str:
    net = _first(node, "net")
    if net is None or len(net) < 2:
        return ""
    if len(net) >= 3 and isinstance(net[2], str):
        return net[2]
    return nets.get(net[1], net[1])  # numbered (KiCad <= 9) or name-only nets


def _ring(pts_node) -> tuple[Point, ...]:
    return tuple(_xy(child) for child in pts_node[1:] if isinstance(child, list) and child and child[0] == "xy")


def scene_from_kicad_pcb(path: str | Path) -> Scene:
    from .importers.kicad_mod import _parse_sexpr
    path = Path(path)
    root = _parse_sexpr(path.read_text(encoding="utf-8"), str(path))
    layer_table = _first(root, "layers")
    copper = tuple(item[1] for item in (layer_table[1:] if layer_table else ())
                   if isinstance(item, list) and len(item) > 1 and str(item[1]).endswith(".Cu"))
    if not copper:
        copper = ("F.Cu", "B.Cu")
    nets = {item[1]: item[2] for item in _atoms(root, "net") if len(item) >= 3}
    scene = Scene(name=path.stem, copper_layers=copper, source=str(path))
    footprint_zones = []
    # Real designators are reserved first so a renamed duplicate never takes one.
    reserved = set()
    for footprint in _atoms(root, "footprint"):
        props = {p[1]: p[2] for p in _atoms(footprint, "property") if len(p) > 2}
        reserved.add(props.get("CopperScriptPath") or props.get("Reference") or "")
    reserved.discard("")

    for footprint in _atoms(root, "footprint"):
        at = _first(footprint, "at")
        fx, fy = _xy(at)
        frot = float(at[3]) if len(at) > 3 else 0.0
        side = "back" if (_first(footprint, "layer") or [None, "F.Cu"])[1] == "B.Cu" else "front"
        ref = path = ""
        for prop in _atoms(footprint, "property"):
            if len(prop) > 2 and prop[1] == "Reference":
                ref = prop[2]
            elif len(prop) > 2 and prop[1] == "CopperScriptPath":
                path = prop[2]
        ref = path or ref  # hierarchical CopperScript paths (PWR/C_SS) match report names
        for text in _atoms(footprint, "fp_text"):
            if not ref and len(text) > 2 and text[1] == "reference":
                ref = text[2]
        if not ref or ref in scene.footprints:
            # Blank (logos, fiducials) or duplicate designators keep their pads apart under the
            # same names CopperReview gives them: <reference, else footprint name>#n.
            name = footprint[1] if len(footprint) > 1 and isinstance(footprint[1], str) else ""
            base = ref or name.rsplit(":", 1)[-1] or "FP"
            n = 1 if not ref else 2
            while f"{base}#{n}" in scene.footprints or f"{base}#{n}" in reserved:
                n += 1
            ref = f"{base}#{n}"
        scene.footprints[ref] = SceneFootprint(ref, (fx, fy), frot, side)
        for pad in _atoms(footprint, "pad"):
            if len(pad) < 4:
                continue
            number, kind, shape = pad[1], pad[2], pad[3]
            pat = _first(pad, "at")
            px, py = _xy(pat)
            # KiCad writes the pad's absolute angle and omits it when it is 0.
            pangle = float(pat[3]) if len(pat) > 3 else 0.0
            ox, oy = _rotate(px, py, frot)
            center = (fx + ox, fy + oy)
            size = _first(pad, "size")
            width, height = float(size[1]), float(size[2])
            ratio = _first(pad, "roundrect_rratio")
            layers = _layer_names(pad, copper)
            if not layers:
                continue  # paste/mask-only apertures
            drill_node = _first(pad, "drill")
            drill = float(drill_node[1]) if drill_node and len(drill_node) > 1 and \
                drill_node[1] != "oval" else 0.0
            scene.pads.append(ScenePad(
                ref, str(number), _net_name(pad, nets), layers,
                pad_polygon(shape, width, height, pangle, center, float(ratio[1]) if ratio else 0.25),
                center, drill))
        # Zones inside a footprint (module copper, keepouts) are stored in board coordinates.
        footprint_zones.extend(_atoms(footprint, "zone"))

    for segment in _atoms(root, "segment"):
        layer = (_first(segment, "layer") or [None, ""])[1]
        scene.tracks.append(SceneTrack(_net_name(segment, nets), layer, _xy(_first(segment, "start")),
                                       _xy(_first(segment, "end")), float(_first(segment, "width")[1])))
    for arc in _atoms(root, "arc"):
        layer = (_first(arc, "layer") or [None, ""])[1]
        points = arc_points(*(_xy(_first(arc, key)) for key in ("start", "mid", "end")))
        width = float(_first(arc, "width")[1])
        for a, b in zip(points, points[1:]):
            if a != b:
                scene.tracks.append(SceneTrack(_net_name(arc, nets), layer, a, b, width))
    for via in _atoms(root, "via"):
        layers = _layer_names(via, copper)
        if len(layers) == 2:  # a span names its end layers; it covers everything between
            i, j = sorted(copper.index(name) for name in layers if name in copper)
            layers = copper[i:j + 1]
        scene.vias.append(SceneVia(_net_name(via, nets), _xy(_first(via, "at")),
                                   float(_first(via, "size")[1]), float(_first(via, "drill")[1]), layers))
    for zone in _atoms(root, "zone") + footprint_zones:
        if _first(zone, "keepout") is not None:
            continue  # rule areas are not copper
        net = _net_name(zone, nets) or (_first(zone, "net_name") or [None, ""])[1]
        filled_layers = set()
        for polygon in _atoms(zone, "filled_polygon"):
            layer = (_first(polygon, "layer") or [None, ""])[1]
            filled_layers.add(layer)
            scene.areas.append(SceneArea(net, layer, _ring(_first(polygon, "pts")), (), "fill"))
        outline = _first(zone, "polygon")
        if outline is None or len(_atoms(_first(outline, "pts") or [], "xy")) < 3:
            continue
        for layer in _layer_names(zone, copper):
            if layer not in filled_layers:  # unfilled layers keep their outline, never fake copper
                scene.areas.append(SceneArea(net, layer, _ring(_first(outline, "pts")), (), "zone"))

    for kind in ("gr_line", "gr_rect", "gr_poly", "gr_arc", "gr_circle"):
        for item in _atoms(root, kind):
            if (_first(item, "layer") or [None, ""])[1] != "Edge.Cuts":
                continue
            if kind == "gr_line":
                scene.edges.append((_xy(_first(item, "start")), _xy(_first(item, "end"))))
            elif kind == "gr_rect":
                (x0, y0), (x1, y1) = _xy(_first(item, "start")), _xy(_first(item, "end"))
                ring = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                scene.edges.extend(zip(ring, ring[1:] + ring[:1]))
            elif kind == "gr_poly":
                ring = list(_ring(_first(item, "pts")))
                scene.edges.extend(zip(ring, ring[1:] + ring[:1]))
            elif kind == "gr_arc":
                points = arc_points(*(_xy(_first(item, key)) for key in ("start", "mid", "end")))
                scene.edges.extend(zip(points, points[1:]))
            elif kind == "gr_circle":
                c, e = _xy(_first(item, "center")), _xy(_first(item, "end"))
                r = ((e[0] - c[0]) ** 2 + (e[1] - c[1]) ** 2) ** 0.5
                ring = _arc_points(c[0], c[1], r, 0, 360, 48)
                scene.edges.extend(zip(ring, ring[1:]))
    return scene


# --- PhysicalBoard reader -------------------------------------------------------------

def _mm(value: int) -> float:
    return value / 1_000_000


def scene_from_physical(board) -> Scene:
    """Scene of an in-memory PhysicalBoard, including unexported solver states."""

    from .drc import placed_pad_shape
    from .physical import BoardSide, PadKind
    from .placement import transformed_local_point
    copper = tuple(layer.value for layer in board.stackup.copper_layers)
    scene = Scene(name=board.name, copper_layers=copper, source="PhysicalBoard")
    net_of = {}
    for net in board.nets:
        for pad in net.pads:
            net_of[(pad.component, pad.pad)] = net.name
    for placement in board.placements:
        footprint = board.footprints.get(placement.footprint)
        side = "back" if placement.side is BoardSide.BACK else "front"
        center = (_mm(placement.position.x_nm), _mm(placement.position.y_nm))
        scene.footprints[placement.reference] = SceneFootprint(
            placement.reference, center, float(placement.rotation_degrees), side)
        if footprint is None:
            continue
        for pad in footprint.pads:
            if pad.kind is PadKind.APERTURE or pad.kind is PadKind.NON_PLATED_THROUGH_HOLE:
                continue
            position = transformed_local_point(placement, pad.position)
            shape = placed_pad_shape(position, pad, placement)
            polygon = _rounded_polygon(shape.spine, shape.radius_nm)
            if pad.kind is PadKind.SMD:
                layers = (copper[-1] if side == "back" else copper[0],)
            else:
                layers = copper
            drill = _mm(pad.drill.width_nm) if pad.drill else 0.0
            scene.pads.append(ScenePad(placement.reference, pad.number,
                                       net_of.get((placement.reference, pad.number), ""), layers, polygon,
                                       (_mm(position.x_nm), _mm(position.y_nm)), drill))
    for track in board.tracks:
        scene.tracks.append(SceneTrack(track.net, track.layer.value, (_mm(track.start.x_nm), _mm(track.start.y_nm)),
                                       (_mm(track.end.x_nm), _mm(track.end.y_nm)), _mm(track.width_nm)))
    for via in board.vias:
        i, j = sorted((copper.index(via.from_layer.value), copper.index(via.to_layer.value)))
        scene.vias.append(SceneVia(via.net, (_mm(via.position.x_nm), _mm(via.position.y_nm)),
                                   _mm(via.size_nm), _mm(via.drill_nm), copper[i:j + 1]))
    filled = {(fill.zone_id, fill.layer.value) for fill in board.zone_fills}
    zones = {zone.id: zone for zone in board.zones}
    for fill in board.zone_fills:
        zone = zones.get(fill.zone_id)
        for polygon in fill.polygons:
            scene.areas.append(SceneArea(zone.net if zone else "", fill.layer.value, _points(polygon.outer),
                                         tuple(_points(hole) for hole in polygon.holes), "fill"))
    for zone in board.zones:
        for layer in zone.layers:
            if (zone.id, layer.value) not in filled:
                scene.areas.append(SceneArea(zone.net, layer.value, _points(zone.outline.outer), (), "zone"))
    for polygon in board.polygons:
        scene.areas.append(SceneArea(polygon.net, polygon.layer.value, _points(polygon.outline), (), "fill"))
    ring = [(_mm(p.x_nm), _mm(p.y_nm)) for p in board.outline.vertices]
    scene.edges.extend(zip(ring, ring[1:] + ring[:1]))
    return scene


def _points(ring) -> tuple[Point, ...]:
    return tuple((_mm(p.x_nm), _mm(p.y_nm)) for p in ring.vertices)


def _rounded_polygon(spine, radius_nm: int) -> tuple[Point, ...]:
    points = [(_mm(p.x_nm), _mm(p.y_nm)) for p in spine]
    r = _mm(radius_nm)
    if r > 0:
        points = [q for x, y in points for q in _arc_points(x, y, r, 0, 360, 16)]
    return tuple(_convex_hull(points))


def _convex_hull(points: list[Point]) -> list[Point]:
    pts = sorted(set(points))
    if len(pts) < 3:
        return pts

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lower, upper = [], []
    for p in pts:
        while len(lower) >= 2 and cross(lower[-2], lower[-1], p) <= 0:
            lower.pop()
        lower.append(p)
    for p in reversed(pts):
        while len(upper) >= 2 and cross(upper[-2], upper[-1], p) <= 0:
            upper.pop()
        upper.append(p)
    return lower[:-1] + upper[:-1]

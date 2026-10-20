"""Import placed KiCad boards (``.kicad_pcb``, KiCad 5 to 10) into physical IR.

The importer reconstructs a *placed, unrouted* board so CopperScript's router
can be compared with other routers on third-party designs. It reads the board
outline, copper stackup, footprints and their poses, pad nets, copper zones,
rule-area keepouts and design rules. Tracks, vias, zone fills, text and
dimensions are counted and dropped.

Each embedded footprint is normalised to front-side library form and imported
by the strict ``.kicad_mod`` importer, so its geometry rules apply unchanged.
Board-only settings that importer rejects are approximated by a superset of
the original copper (for example a custom pad becomes its bounding rectangle)
or dropped when they do not affect copper geometry. Every approximation is
recorded in ``BoardImportResult.approximations`` and in the board metadata;
an approximated board is comparison input, not fabrication data.

Conventions follow CopperScript's own KiCad exporter: a KiCad footprint-local
point ``(u, v)`` at footprint angle ``A`` lies at ``P + R(-A)(u, v)``. Back-side
footprints store locally Y-mirrored geometry, so their IR library form is
``(u, -v)`` with IR rotation ``A - 180``. Pad angles are stored in board
coordinates: local pad rotation is ``a - A`` on the front and ``A - a`` on the
back.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field, replace
from decimal import Decimal
from hashlib import sha256
import json
from math import atan2, cos, hypot, radians, sin
from pathlib import Path
import re

from ..physical import (
    BoardBoundaryPath,
    BoardCutout,
    BoardOutline,
    BoardSide,
    BoundaryArc,
    BoundaryLine,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    DesignRules,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    PolygonRing,
    PolygonWithHoles,
    Stackup,
    nm_from_mm,
)
from .kicad_mod import (
    KiCadModImportError,
    SExpr,
    _atom,
    _children,
    _first,
    _lists,
    _parse_sexpr,
    _tag,
    parse_kicad_mod,
)


class KiCadPcbImportError(ValueError):
    """A board that cannot be reconstructed as placed, routable physical IR."""


@dataclass(frozen=True, slots=True)
class BoardImportResult:
    board: PhysicalBoard
    source_version: str | None
    # Copper-relevant changes made to fit the IR, as "description: count".
    approximations: tuple[str, ...] = ()
    # Items that do not affect placed, unrouted copper (text, 3D models...).
    warnings: tuple[str, ...] = ()
    # Existing routing and other dropped board items, by KiCad tag.
    dropped: tuple[tuple[str, int], ...] = ()


# Distance within which outline endpoints are treated as one point.
_OUTLINE_SNAP_NM = 2_000
_KICAD_DEFAULT_CLEARANCE_NM = nm_from_mm("0.2")
_KICAD_DEFAULT_TRACK_NM = nm_from_mm("0.25")
_KICAD_DEFAULT_VIA_NM = nm_from_mm("0.8")
_KICAD_DEFAULT_VIA_DRILL_NM = nm_from_mm("0.4")
_SUPPORTED_ATTRIBUTES = {"smd", "through_hole", "exclude_from_bom", "exclude_from_pos_files"}
_ROUTING_ITEMS = {"segment", "arc", "via"}
_DROPPED_BOARD_ITEMS = {
    "gr_text", "gr_text_box", "dimension", "group", "target", "image", "generated",
    "table", "embedded_fonts", "embedded_files", "title_block", "paper", "page",
    "generator", "generator_version", "general", "version", "host", "property",
}
_GRAPHIC_TAGS = {"fp_line", "fp_rect", "fp_circle", "fp_arc", "fp_poly", "fp_curve"}
_BOARD_GRAPHIC_TAGS = {"gr_line", "gr_rect", "gr_circle", "gr_arc", "gr_poly", "gr_curve"}
# Pad children the strict importer accepts unchanged.
_PAD_KEPT = {"at", "size", "layers", "roundrect_rratio", "drill", "uuid", "tstamp",
             "pinfunction", "pintype", "remove_unused_layers", "thermal_bridge_angle"}
# Pad children that do not change the pad's copper shape.
_PAD_NON_COPPER = {"solder_mask_margin", "solder_paste_margin", "solder_paste_margin_ratio",
                   "solder_paste_ratio", "die_length", "die_delay", "teardrops", "tenting",
                   "zone_connect", "thermal_width", "thermal_gap", "thermal_bridge_width",
                   "zone_layer_connections", "keep_end_layers", "padstack", "net",
                   "options", "primitives", "chamfer_ratio", "chamfer", "rect_delta",
                   "clearance", "property"}


def load_kicad_pcb(path: str | Path, *, project: str | Path | None = None) -> BoardImportResult:
    """Read a ``.kicad_pcb`` and, when present, its ``.kicad_pro`` design rules."""

    source = Path(path)
    if source.suffix.casefold() != ".kicad_pcb":
        raise KiCadPcbImportError(f"{source}: expected a .kicad_pcb board file")
    try:
        text = source.read_bytes().decode("utf-8-sig")
    except (OSError, UnicodeDecodeError) as exc:
        raise KiCadPcbImportError(f"cannot read {source}: {exc}") from exc
    project_path = Path(project) if project is not None else source.with_suffix(".kicad_pro")
    project_json = None
    if project_path.is_file():
        try:
            project_json = json.loads(project_path.read_text(encoding="utf-8-sig"))
        except (OSError, ValueError) as exc:
            raise KiCadPcbImportError(f"cannot read {project_path}: {exc}") from exc
    elif project is not None:
        raise KiCadPcbImportError(f"{project_path}: project file not found")
    return parse_kicad_pcb(text, source=str(source), name=source.stem, project=project_json)


def parse_kicad_pcb(text: str, *, source: str = "<memory>", name: str = "imported",
                    project: dict | None = None) -> BoardImportResult:
    """Reconstruct a placed, unrouted board from KiCad board text."""

    try:
        root = _parse_sexpr(text, source)
    except KiCadModImportError as exc:
        raise KiCadPcbImportError(str(exc)) from exc
    if _tag(root) != "kicad_pcb":
        raise KiCadPcbImportError(f"{source}: root expression must be 'kicad_pcb'")
    version = _atom(_first(root, "version"), 1)
    log = _Log()
    aliases = _layer_aliases(root, source)
    if aliases:
        root = _rename_layers(root, aliases)
        log.warn("KiCad 5 custom copper layer names mapped to canonical names")

    copper = _copper_layers(root, source)
    stackup = Stackup(copper, thickness_nm=_thickness(root))
    dropped: Counter[str] = Counter()
    footprints: dict[str, PhysicalFootprint] = {}
    footprint_texts: dict[str, str] = {}
    placements: list[Placement] = []
    net_pads: dict[str, list[PadReference]] = {}
    references: Counter[str] = Counter()
    edges: list[_Edge] = []
    keepouts: list[CopperKeepout] = []
    zones: list[CopperZone] = []

    for item in _lists(root[1:]):
        tag = _tag(item)
        if tag in {"footprint", "module"}:
            imported = _import_footprint(item, source, copper, log)
            reference = imported.reference or "REF**"
            references[reference] += 1
            if references[reference] > 1:
                log.approx("duplicate reference renamed")
                reference = f"{reference}#{references[reference]}"
            library_name = _unique_footprint_name(imported, footprints, footprint_texts)
            placements.append(Placement(reference, library_name, imported.position,
                                        imported.rotation, imported.side, imported.value))
            for number, net in imported.pad_nets:
                pads = net_pads.setdefault(net, [])
                pad = PadReference(reference, number)
                if pad not in pads:
                    pads.append(pad)
            edges.extend(imported.edges)
            keepouts.extend(replace(keepout, id=f"{reference}/{keepout.id}")
                            for keepout in imported.copper_obstacles)
            companion = imported.companion
            if companion is not None:
                companion_reference = f"{reference}~opposite"
                companion_name = _unique_footprint_name(companion, footprints, footprint_texts)
                placements.append(Placement(companion_reference, companion_name, companion.position,
                                            companion.rotation, companion.side, companion.value))
                for number, net in companion.pad_nets:
                    pads = net_pads.setdefault(net, [])
                    pad = PadReference(companion_reference, number)
                    if pad not in pads:
                        pads.append(pad)
        elif tag in _BOARD_GRAPHIC_TAGS:
            layer = _atom(_first(item, "layer"), 1)
            if layer == "Edge.Cuts":
                edges.extend(_board_edges(item, source, log))
            elif layer in {layer.value for layer in copper}:
                keepouts.extend(_copper_graphic_keepouts(item, CopperLayer(layer), f"graphic-{len(keepouts)}", log))
            else:
                dropped[tag] += 1
        elif tag == "zone":
            zone_keepouts, zone_copper = _import_zone(item, copper, f"zone-{len(zones) + len(keepouts)}", log)
            keepouts.extend(zone_keepouts)
            zones.extend(zone_copper)
        elif tag in _ROUTING_ITEMS:
            dropped[tag] += 1
        elif tag in {"net", "net_class", "layers", "setup", "general", "paper", "page",
                     "title_block", "version", "generator", "generator_version", "host"}:
            continue
        else:
            dropped[tag or "?"] += 1

    for zone in [zone for zone in zones if not net_pads.get(zone.net)]:
        # Copper on a net without pads is an obstacle for every routed net.
        zones.remove(zone)
        keepouts.append(CopperKeepout(zone.id, zone.layers, zone.outline))
        log.approx("zone on a net without pads as keepout")
    outline = _outline(edges, placements, footprints, source, log)
    rules, net_rules = _design_rules(root, project, net_pads, log)
    nets = tuple(PhysicalNet(net, tuple(pads)) for net, pads in sorted(net_pads.items()) if net)
    metadata = {
        "source_format": "kicad_pcb",
        "source_path": source,
        "source_sha256": sha256(text.encode("utf-8")).hexdigest(),
        "source_version": version or "",
        "imported_unrouted": "true",
        "fabrication_ready": "false",
        # The imported placement is the input under test (see drc.EXTERNAL_PLACEMENT_KEY).
        "placement_legality": "external",
    }
    approximations = log.approximations()
    if approximations:
        metadata["import_approximations"] = " | ".join(approximations)
    try:
        board = PhysicalBoard(
            name=name,
            outline=outline,
            footprints=footprints,
            placements=tuple(placements),
            nets=nets,
            stackup=stackup,
            rules=rules,
            metadata=metadata,
            net_routing_rules=net_rules,
            zones=tuple(zones),
            copper_keepouts=tuple(keepouts),
        )
    except ValueError as exc:
        raise KiCadPcbImportError(f"{source}: {exc}") from exc
    return BoardImportResult(board, version, approximations, log.warning_list(),
                             tuple(sorted(dropped.items())))


class _Log:
    def __init__(self) -> None:
        self._approximations: Counter[str] = Counter()
        self._warnings: Counter[str] = Counter()

    def approx(self, message: str) -> None:
        self._approximations[message] += 1

    def warn(self, message: str) -> None:
        self._warnings[message] += 1

    def approximations(self) -> tuple[str, ...]:
        return tuple(f"{message}: {count}" for message, count in sorted(self._approximations.items()))

    def warning_list(self) -> tuple[str, ...]:
        return tuple(f"{message}: {count}" for message, count in sorted(self._warnings.items()))


# --------------------------------------------------------------------------- geometry

def _nm(value: SExpr, source: str) -> int:
    try:
        return int((Decimal(str(value)) * 1_000_000).to_integral_value())
    except Exception as exc:  # noqa: BLE001 - any malformed number is a board error
        raise KiCadPcbImportError(f"{source}: invalid coordinate {value!r}") from exc


def _xy(node: list[SExpr] | None, source: str) -> Point:
    if node is None or len(node) < 3:
        raise KiCadPcbImportError(f"{source}: missing coordinate")
    return Point(_nm(node[1], source), _nm(node[2], source))


def _angle(node: list[SExpr] | None) -> Decimal:
    value = _atom(node, 3) if node is not None else None
    return Decimal(value) if value is not None else Decimal(0)


def _turn(degrees: Decimal) -> Decimal:
    """Angle in [0, 360); Decimal ``%`` keeps the dividend's sign."""
    return ((degrees % 360) + 360) % 360


def _rotate(point: Point, degrees: Decimal | float) -> Point:
    """KiCad file angle: local (u, v) at angle A is R(-A)(u, v) on the board."""
    angle = radians(-float(degrees))
    return Point(round(point.x_nm * cos(angle) - point.y_nm * sin(angle)),
                 round(point.x_nm * sin(angle) + point.y_nm * cos(angle)))


def _to_board(point: Point, position: Point, degrees: Decimal) -> Point:
    offset = _rotate(point, degrees)
    return Point(position.x_nm + offset.x_nm, position.y_nm + offset.y_nm)


def _rotate_about(point: Point, center: Point, degrees: float) -> Point:
    angle = radians(degrees)
    dx, dy = point.x_nm - center.x_nm, point.y_nm - center.y_nm
    return Point(round(center.x_nm + dx * cos(angle) - dy * sin(angle)),
                 round(center.y_nm + dx * sin(angle) + dy * cos(angle)))


def _legacy_arc(center: Point, start: Point, sweep: float) -> tuple[Point, Point, Point]:
    """KiCad 5 arc (centre, start point, sweep) as start/mid/end points."""
    return start, _rotate_about(start, center, sweep / 2), _rotate_about(start, center, sweep)


def _arc_polyline(start: Point, mid: Point, end: Point, steps_per_quadrant: int = 8) -> list[Point]:
    """Points of a three-point arc, including both ends."""
    ax, ay, bx, by, cx, cy = start.x_nm, start.y_nm, mid.x_nm, mid.y_nm, end.x_nm, end.y_nm
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if d == 0:
        return [start, end]
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    radius = hypot(ax - ux, ay - uy)
    a0, a1, a2 = (atan2(y - uy, x - ux) for x, y in ((ax, ay), (bx, by), (cx, cy)))
    from math import pi
    def ccw(a, b):
        return (b - a) % (2 * pi)
    sweep = ccw(a0, a2)
    if ccw(a0, a1) > sweep:  # the arc passes through mid the other way round
        sweep -= 2 * pi
    steps = max(2, int(abs(sweep) / (pi / 2) * steps_per_quadrant) + 1)
    points = [Point(round(ux + radius * cos(a0 + sweep * i / steps)),
                    round(uy + radius * sin(a0 + sweep * i / steps))) for i in range(steps + 1)]
    points[0], points[-1] = start, end
    return points


def _bezier_points(points: list[Point], steps: int = 16) -> list[Point]:
    p0, p1, p2, p3 = points
    result = []
    for i in range(steps + 1):
        t = i / steps
        a, b, c, d = (1 - t) ** 3, 3 * t * (1 - t) ** 2, 3 * t * t * (1 - t), t ** 3
        result.append(Point(round(a * p0.x_nm + b * p1.x_nm + c * p2.x_nm + d * p3.x_nm),
                            round(a * p0.y_nm + b * p1.y_nm + c * p2.y_nm + d * p3.y_nm)))
    return result


# --------------------------------------------------------------------------- layers

def _layer_aliases(root: list[SExpr], source: str) -> dict[str, str]:
    """Custom copper layer names mapped to KiCad's canonical names.

    KiCad 5 files reference copper layers by their user names ("Top",
    "Inner1.Cu"); the layer id fixes the canonical layer: 0 is F.Cu, 31 is
    B.Cu, and inner ids are numbered in stack order. KiCad 6 and later always
    write the canonical name first, so their entries map to themselves.
    """
    layers_node = _first(root, "layers")
    entries = []
    for item in _lists(layers_node[1:] if layers_node else []):
        identifier, name, kind = item[0], _atom(item, 1), _atom(item, 2)
        if name is None or not str(identifier).isdigit():
            continue
        if kind in {"signal", "power", "mixed", "jumper"} or name.endswith(".Cu"):
            entries.append((int(identifier), name))
    if not entries or all(name in {"F.Cu", "B.Cu"} or re.fullmatch(r"In\d+\.Cu", name) for _, name in entries):
        return {}
    inner = sorted(identifier for identifier, _ in entries if identifier not in {0, 31})
    canonical = {0: "F.Cu", 31: "B.Cu", **{identifier: f"In{index}.Cu" for index, identifier in enumerate(inner, 1)}}
    return {name: canonical[identifier] for identifier, name in entries if canonical.get(identifier, name) != name}


def _rename_layers(node: SExpr, aliases: dict[str, str]) -> SExpr:
    if isinstance(node, str):
        return node
    if _tag(node) in {"layer", "layers"}:
        return [node[0], *(aliases.get(item, item) if isinstance(item, str) else _rename_layers(item, aliases)
                           for item in node[1:])]
    return [_rename_layers(item, aliases) for item in node]


def _copper_layers(root: list[SExpr], source: str) -> tuple[CopperLayer, ...]:
    layers_node = _first(root, "layers")
    names = []
    for item in _lists(layers_node[1:] if layers_node else []):
        name, kind = _atom(item, 1), _atom(item, 2)
        if name and (name.endswith(".Cu") or kind in {"signal", "power", "mixed", "jumper"}):
            names.append(name)
    aliases = _layer_aliases(root, source)
    names = [aliases.get(name, name) for name in names]
    inner = sorted((name for name in names if name.startswith("In")),
                   key=lambda name: int(re.sub(r"\D", "", name) or 0))
    if not names:
        names = ["F.Cu", "B.Cu"]
    layers = ["F.Cu", *inner, "B.Cu"] if len(names) > 1 else ["F.Cu"]
    try:
        stack = tuple(CopperLayer(name) for name in layers)
    except ValueError as exc:
        raise KiCadPcbImportError(f"{source}: unsupported copper layer set {names!r}") from exc
    expected = ("F.Cu", *(f"In{i}.Cu" for i in range(1, len(layers) - 1)), "B.Cu")
    if len(layers) > 2 and tuple(layers) != expected:
        raise KiCadPcbImportError(f"{source}: copper layers are not contiguous: {layers!r}")
    return stack


def _thickness(root: list[SExpr]) -> int:
    general = _first(root, "general")
    thickness = _atom(_first(general, "thickness"), 1) if general is not None else None
    try:
        value = int(Decimal(thickness) * 1_000_000) if thickness else 0
    except Exception:  # noqa: BLE001
        value = 0
    return value if value > 0 else nm_from_mm("1.6")


def _layer_set(node: list[SExpr], copper: tuple[CopperLayer, ...]) -> tuple[CopperLayer, ...]:
    """Copper layers named by a ``layer``/``layers`` child, expanding wildcards."""
    names: list[str] = []
    for tag in ("layer", "layers"):
        child = _first(node, tag)
        if child is not None:
            names.extend(item for item in child[1:] if isinstance(item, str))
    result: list[CopperLayer] = []
    for name in names:
        if name in {"*.Cu", "F&B.Cu"}:
            candidates = copper if name == "*.Cu" else (copper[0], copper[-1])
        elif name.endswith(".Cu"):
            candidates = tuple(layer for layer in copper if layer.value == name)
        else:
            candidates = ()
        result.extend(layer for layer in candidates if layer not in result)
    return tuple(sorted(result, key=copper.index))


# --------------------------------------------------------------------------- footprints

@dataclass(slots=True)
class _ImportedFootprint:
    reference: str
    value: str
    position: Point
    rotation: Decimal
    side: BoardSide
    library_name: str
    footprint: PhysicalFootprint
    library_text: str
    pad_nets: list[tuple[str, str]] = field(default_factory=list)
    edges: list["_Edge"] = field(default_factory=list)
    copper_obstacles: list[CopperKeepout] = field(default_factory=list)
    companion: "_ImportedFootprint | None" = None


def _import_footprint(node: list[SExpr], source: str, copper: tuple[CopperLayer, ...],
                      log: _Log) -> _ImportedFootprint:
    library_name = _atom(node, 1) or "footprint"
    layer = _atom(_first(node, "layer"), 1) or "F.Cu"
    if layer not in {"F.Cu", "B.Cu"}:
        raise KiCadPcbImportError(f"{source}: footprint {library_name!r} on unsupported layer {layer!r}")
    back = layer == "B.Cu"
    at = _first(node, "at")
    position = _xy(at, source) if at is not None else Point(0, 0)
    angle = _angle(at)
    reference, value = "", ""
    for prop in _children(node, "property"):
        if _atom(prop, 1) == "Reference":
            reference = _atom(prop, 2) or ""
        elif _atom(prop, 1) == "Value":
            value = _atom(prop, 2) or ""
    for text in _children(node, "fp_text"):
        if _atom(text, 1) == "reference" and not reference:
            reference = _atom(text, 2) or ""
        elif _atom(text, 1) == "value" and not value:
            value = _atom(text, 2) or ""
    where = f"{source}:{reference or library_name}"

    library: list[SExpr] = ["footprint", library_name, ["layer", "F.Cu"]]
    companion_library: list[SExpr] = ["footprint", library_name + "~opposite", ["layer", "F.Cu"]]
    pad_nets: list[tuple[str, str]] = []
    companion_nets: list[tuple[str, str]] = []
    edges: list[_Edge] = []
    obstacles: list[CopperKeepout] = []
    opposite = "F.Cu" if back else "B.Cu"
    for child in _lists(node[2:]):
        tag = _tag(child)
        if tag == "pad":
            pad_copper = [item for item in (_first(child, "layers") or [])[1:]
                          if isinstance(item, str) and item.endswith(".Cu")]
            on_opposite = _atom(child, 2) in {"smd", "connect"} and pad_copper == [opposite]
            # A surface pad on the footprint's other side is exact copper of a
            # companion placement on that side at the same pose.
            pad, net = _library_pad(child, not back if on_opposite else back, angle, where, log)
            (companion_library if on_opposite else library).append(pad)
            if net and _has_copper(pad):
                (companion_nets if on_opposite else pad_nets).append((pad[1], net))
        elif tag in _GRAPHIC_TAGS:
            graphic_layer = _atom(_first(child, "layer"), 1) or ""
            if graphic_layer == "Edge.Cuts":
                edges.extend(_footprint_edges(child, position, angle, where, log))
            elif graphic_layer.endswith(".Cu"):
                board_child = _graphic_to_board(child, position, angle, where)
                layer_name = graphic_layer
                if layer_name in {item.value for item in copper}:
                    obstacles.extend(_copper_graphic_keepouts(board_child, CopperLayer(layer_name),
                                                              f"copper-graphic-{len(obstacles)}", log))
            else:
                graphic = _library_graphic(child, back)
                if graphic is None:
                    log.warn("ignored degenerate footprint arc")
                else:
                    library.append(graphic)
        elif tag == "zone":
            library.append(_mirror_tree(_swap_side(child), back))
        elif tag == "attr":
            attributes = [item for item in child[1:] if isinstance(item, str)]
            kept = [item for item in attributes if item in _SUPPORTED_ATTRIBUTES]
            for item in attributes:
                if item not in _SUPPORTED_ATTRIBUTES:
                    log.warn(f"ignored footprint attribute {item}")
            if kept:
                library.append(["attr", *kept])
        elif tag in {"clearance", "descr", "tags", "version", "generator", "duplicate_pad_numbers_are_jumpers",
                     "jumper_pad_groups"}:
            library.append(child)
        elif tag in {"solder_mask_margin", "solder_paste_margin", "solder_paste_ratio",
                     "solder_paste_margin_ratio", "zone_connect", "thermal_width", "thermal_gap",
                     "net_tie_pad_groups", "private_layers"}:
            log.approx(f"dropped footprint {tag}")
        elif tag in {"at", "layer", "property", "fp_text", "fp_text_box", "model", "path", "sheetname",
                     "sheetfile", "tstamp", "uuid", "tedit", "locked", "placed", "embedded_fonts",
                     "component_classes", "units", "net_tie_pad_groups", "generator_version", "group"}:
            continue
        else:
            log.warn(f"ignored footprint item {tag}")
    library_text = _dump(library)
    try:
        imported = parse_kicad_mod(library_text, source=where, strict=False)
    except KiCadModImportError as exc:
        raise KiCadPcbImportError(f"{where}: {exc}") from exc
    for message in imported.warnings:
        log.warn(message)
    rotation = _turn((angle - 180) if back else angle)
    companion = None
    if len(companion_library) > 3:
        companion_text = _dump(companion_library)
        try:
            companion_footprint = parse_kicad_mod(companion_text, source=where + "~opposite", strict=False).footprint
        except KiCadModImportError as exc:
            raise KiCadPcbImportError(f"{where}: {exc}") from exc
        log.approx("opposite-side surface pads as a companion placement")
        companion = _ImportedFootprint(
            "", value, position, _turn(angle if back else angle - 180),
            BoardSide.FRONT if back else BoardSide.BACK, library_name + "~opposite",
            companion_footprint, companion_text, companion_nets)
    return _ImportedFootprint(reference, value, position, rotation,
                              BoardSide.BACK if back else BoardSide.FRONT, library_name,
                              imported.footprint, library_text, pad_nets, edges, obstacles, companion)


def _unique_footprint_name(imported: _ImportedFootprint, footprints: dict[str, PhysicalFootprint],
                           texts: dict[str, str]) -> str:
    base = imported.library_name
    for index in range(1, 10_000):
        name = base if index == 1 else f"{base}~{index}"
        known = texts.get(name)
        if known is None:
            footprints[name] = replace(imported.footprint, name=name)
            texts[name] = imported.library_text
            return name
        if known == imported.library_text:
            return name
    raise KiCadPcbImportError(f"too many distinct variants of footprint {base!r}")


def _swap_side(node: SExpr) -> SExpr:
    """Rename back-side layers (B.*) to their front-side counterparts."""
    if isinstance(node, str):
        return "F." + node[2:] if node.startswith("B.") else node
    if _tag(node) in {"layer", "layers"}:
        return [node[0], *(_swap_side(item) for item in node[1:])]
    return [_swap_side(item) for item in node]


_POINT_TAGS = {"start", "end", "center", "mid", "xy", "at", "offset"}


def _mirror_tree(node: SExpr, back: bool) -> SExpr:
    """Mirror footprint-local coordinates (v -> -v) of a back-side subtree."""
    if not back or isinstance(node, str):
        return node
    tag = _tag(node)
    if tag in _POINT_TAGS and len(node) >= 3 and all(isinstance(item, str) for item in node[1:3]):
        try:
            y = Decimal(node[2])
        except Exception:  # noqa: BLE001
            return node
        mirrored = [node[0], node[1], str(-y)]
        if tag == "at":
            mirrored.extend(node[3:])  # caller adjusts any angle
        else:
            mirrored.extend(node[3:])
        return mirrored
    return [_mirror_tree(item, back) for item in node]


def _abs_widths(node: SExpr) -> SExpr:
    if isinstance(node, str):
        return node
    if _tag(node) == "width" and len(node) > 1 and isinstance(node[1], str) and node[1].startswith("-"):
        return [node[0], node[1][1:], *node[2:]]
    return [_abs_widths(item) for item in node]


def _library_graphic(node: list[SExpr], back: bool) -> list[SExpr] | None:
    node = _abs_widths(node)
    tag = _tag(node)
    if tag == "fp_arc" and _first(node, "mid") is None and _first(node, "angle") is not None:
        center = _xy(_first(node, "start"), "arc")
        start = _xy(_first(node, "end"), "arc")
        sweep = float(Decimal(_atom(_first(node, "angle"), 1) or "0"))
        rest = [child for child in node[1:] if _tag(child) not in {"start", "end", "angle"}]
        if abs(sweep) >= 360:
            node = ["fp_circle", ["center", _mm(center.x_nm), _mm(center.y_nm)],
                    ["end", _mm(start.x_nm), _mm(start.y_nm)], *rest]
        else:
            a, m, b = _legacy_arc(center, start, sweep)
            if len({a, m, b}) < 3:
                return None
            node = ["fp_arc", ["start", _mm(a.x_nm), _mm(a.y_nm)], ["mid", _mm(m.x_nm), _mm(m.y_nm)],
                    ["end", _mm(b.x_nm), _mm(b.y_nm)], *rest]
    elif tag == "fp_arc":
        points = [_first(node, key) for key in ("start", "mid", "end")]
        if any(point is None for point in points) or len({tuple(point[1:3]) for point in points}) < 3:
            return None
    if tag == "fp_curve":
        pts = _first(node, "pts")
        points = [_xy(item, "curve") for item in _children(pts, "xy")] if pts else []
        rest = [child for child in node[1:] if _tag(child) != "pts"]
        if len(points) == 4:
            node = ["fp_poly", ["pts", *(["xy", _mm(p.x_nm), _mm(p.y_nm)] for p in _bezier_points(points))],
                    *rest]
    return _mirror_tree(_swap_side(node), back)


def _library_pad(node: list[SExpr], back: bool, footprint_angle: Decimal, where: str,
                 log: _Log) -> tuple[list[SExpr], str]:
    number = node[1] if len(node) > 1 and isinstance(node[1], str) else ""
    kind = node[2] if len(node) > 2 and isinstance(node[2], str) else ""
    if kind == "connect":
        kind = "smd"  # identical copper; edge-contact mask/paste rules do not affect routing
        log.warn("connector-contact pad imported as a surface pad")
    if not number and kind != "np_thru_hole":
        number = "~" + sha256(_dump(node).encode()).hexdigest()[:8]
        log.approx("unnumbered pad given a synthetic number")
    shape = node[3] if len(node) > 3 and isinstance(node[3], str) else ""
    net = ""
    net_node = _first(node, "net")
    if net_node is not None:
        strings = [item for item in net_node[1:] if isinstance(item, str)]
        net = strings[-1] if strings and not (len(strings) == 1 and strings[0].isdigit()) else ""
    children = list(_lists(node[4:]))
    size_node = _first(node, "size")
    width = _nm(size_node[1], where) if size_node is not None else 0
    height = _nm(size_node[2], where) if size_node is not None and len(size_node) > 2 else width

    if shape == "trapezoid":
        delta = _first(node, "rect_delta")
        dx = abs(_nm(delta[1], where)) if delta is not None else 0
        dy = abs(_nm(delta[2], where)) if delta is not None and len(delta) > 2 else 0
        width, height = width + dy, height + dx
        shape = "rect"
        log.approx("trapezoid pad as bounding rectangle")
    elif shape == "custom":
        width, height, offset = _custom_pad_bounds(node, width, height, where)
        shape = "rect"
        log.approx("custom pad as bounding rectangle")
        if offset != (0, 0):
            log.approx("custom pad anchor offset ignored")
    if _first(node, "chamfer_ratio") is not None or _first(node, "chamfer") is not None:
        log.approx("pad chamfers dropped (unchamfered pad contains the copper)")
    if _first(node, "clearance") is not None:
        log.approx("pad-level clearance override dropped")
    for child in children:
        tag = _tag(child)
        if tag not in _PAD_KEPT and tag not in _PAD_NON_COPPER and tag != "property":
            log.warn(f"ignored pad item {tag}")

    at = _first(node, "at")
    position = _xy(at, where) if at is not None else Point(0, 0)
    board_angle = _angle(at)
    if back:
        position = Point(position.x_nm, -position.y_nm)
        local_angle = _turn(footprint_angle - board_angle)
    else:
        local_angle = _turn(board_angle - footprint_angle)
    layers_node = _first(node, "layers")
    raw_layers = [item for item in (layers_node or [])[1:] if isinstance(item, str)]
    layers = [_swap_side(item) if back else item for item in raw_layers]
    if min(width, height) <= 0:
        width, height = max(width, 10_000), max(height, 10_000)
        log.approx("zero-size pad given a 10 um minimum size")
    if kind == "smd" and not any(layer.endswith(".Cu") for layer in layers) and number:
        number = ""  # a paste/mask aperture is not an electrical terminal
        log.warn("copper-less aperture pad number cleared")
    drill = _pad_drill(_first(node, "drill"), kind, where, log)
    if drill is not None and kind in {"thru_hole", "np_thru_hole"}:
        hole = max(_nm(value, where) for value in drill[1:] if value != "oval")
        if kind == "thru_hole" and min(width, height) <= hole:
            width, height = max(width, hole + 10_000), max(height, hole + 10_000)
            log.approx("plated pad enlarged to exceed its drill")
        elif kind == "np_thru_hole" and min(width, height) < hole:
            width, height = max(width, hole), max(height, hole)
            log.warn("non-plated hole pad size raised to its drill")
    result: list[SExpr] = ["pad", number, kind, shape,
                           ["at", _mm(position.x_nm), _mm(position.y_nm), _decimal_text(local_angle)],
                           ["size", _mm(width), _mm(height)],
                           ["layers", *_pad_layers(kind, layers, where, log)]]
    if drill is not None:
        result.append(drill)
    for child in children:
        tag = _tag(child)
        if tag in {"at", "size", "layers", "drill"}:
            continue
        elif tag == "property":
            if _atom(child, 1) == "pad_prop_heatsink":
                result.append(child)
            else:
                log.warn(f"ignored pad property {_atom(child, 1)}")
        elif tag in _PAD_KEPT:
            result.append(child)
    return result, net


def _has_copper(pad: list[SExpr]) -> bool:
    layers = _first(pad, "layers") or []
    return pad[2] != "np_thru_hole" and any(item in {"F.Cu", "*.Cu"} for item in layers[1:])


def _pad_layers(kind: str, layers: list[str], where: str, log: _Log) -> list[str]:
    """Canonical layer set the strict importer accepts, keeping the pad's copper."""
    copper = [layer for layer in layers if layer.endswith(".Cu")]
    if kind in {"thru_hole", "np_thru_hole"}:
        if kind == "thru_hole" and copper != ["*.Cu"] and copper:
            log.approx("plated pad on some copper layers treated as on all layers")
        mask = any(layer.endswith(".Mask") for layer in layers)
        return ["*.Cu", *(["*.Mask"] if mask else [])]
    # smd / connect: footprint-side copper after back-side normalisation.
    if copper and copper != ["F.Cu"]:
        raise KiCadPcbImportError(
            f"{where}: SMD pad copper {copper!r} is not on its footprint's side")
    kept = [layer for layer in ("F.Cu", "F.Mask", "F.Paste") if layer in layers]
    if set(layers) - set(kept):
        log.warn("ignored non-copper pad layers")
    return kept


def _pad_drill(node: list[SExpr] | None, kind: str, where: str, log: _Log) -> list[SExpr] | None:
    """A drill the strict importer accepts: none on SMD pads, no offset, oval with two sizes."""
    if node is None:
        return None
    if kind not in {"thru_hole", "np_thru_hole"}:
        log.warn("ignored drill on a surface pad")
        return None
    if any(isinstance(item, list) for item in node[1:]):
        log.approx("pad drill offset ignored")
    values = [item for item in node[1:] if isinstance(item, str)]
    if values and values[0] == "oval":
        values = values[1:]
        if len(values) >= 2 and values[0] != values[1]:
            return ["drill", "oval", values[0], values[1]]
        return ["drill", values[0]] if values else None
    return ["drill", values[0]] if values else None


def _custom_pad_bounds(node: list[SExpr], width: int, height: int,
                       where: str) -> tuple[int, int, tuple[int, int]]:
    """Bounding box of a custom pad's anchor and primitives, centred on the anchor."""
    xs = [-width / 2, width / 2]
    ys = [-height / 2, height / 2]
    primitives = _first(node, "primitives")
    for primitive in _lists(primitives[1:] if primitives else []):
        stroke = _first(primitive, "width")
        half = _nm(stroke[1], where) / 2 if stroke is not None else 0
        points = [_xy(item, where) for item in _children(_first(primitive, "pts") or [], "xy")]
        for tag in ("start", "end", "mid"):
            item = _first(primitive, tag)
            if item is not None:
                points.append(_xy(item, where))
        if _tag(primitive) == "gr_circle":
            center, edge = _first(primitive, "center"), _first(primitive, "end")
            if center is not None and edge is not None:
                c, e = _xy(center, where), _xy(edge, where)
                r = hypot(e.x_nm - c.x_nm, e.y_nm - c.y_nm)
                points.extend((Point(round(c.x_nm - r), round(c.y_nm - r)), Point(round(c.x_nm + r), round(c.y_nm + r))))
        for point in points:
            xs.extend((point.x_nm - half, point.x_nm + half))
            ys.extend((point.y_nm - half, point.y_nm + half))
    # A centred rectangle containing everything keeps the pad anchor fixed.
    half_width = max(abs(min(xs)), abs(max(xs)))
    half_height = max(abs(min(ys)), abs(max(ys)))
    return round(2 * half_width), round(2 * half_height), (0, 0)


# --------------------------------------------------------------------------- outline

@dataclass(frozen=True, slots=True)
class _Edge:
    start: Point
    end: Point
    mid: Point | None = None  # arcs only


def _board_edges(node: list[SExpr], source: str, log: _Log) -> list[_Edge]:
    tag = _tag(node).replace("gr_", "")
    return _shape_edges(tag, node, lambda point: point, source, log)


def _footprint_edges(node: list[SExpr], position: Point, angle: Decimal, source: str,
                     log: _Log) -> list[_Edge]:
    tag = _tag(node).replace("fp_", "")
    return _shape_edges(tag, node, lambda point: _to_board(point, position, angle), source, log)


def _graphic_to_board(node: list[SExpr], position: Point, angle: Decimal, source: str) -> list[SExpr]:
    def convert(item: SExpr) -> SExpr:
        if isinstance(item, str):
            return item
        if _tag(item) in {"start", "end", "center", "mid", "xy"} and len(item) >= 3:
            point = _to_board(_xy(item, source), position, angle)
            return [item[0], _mm(point.x_nm), _mm(point.y_nm), *item[3:]]
        return [convert(child) for child in item]
    converted = convert(node)
    converted[0] = "gr_" + _tag(node)[3:]
    return converted


def _shape_edges(tag: str, node: list[SExpr], transform, source: str, log: _Log) -> list[_Edge]:
    def point(child_tag: str) -> Point:
        return transform(_xy(_first(node, child_tag), source))
    if tag == "line":
        start, end = point("start"), point("end")
        return [] if start == end else [_Edge(start, end)]
    if tag == "rect":
        a, c = _xy(_first(node, "start"), source), _xy(_first(node, "end"), source)
        corners = [transform(p) for p in (a, Point(c.x_nm, a.y_nm), c, Point(a.x_nm, c.y_nm))]
        return [_Edge(p, q) for p, q in zip(corners, corners[1:] + corners[:1])]
    if tag == "circle":
        center = _xy(_first(node, "center"), source)
        edge = _xy(_first(node, "end"), source)
        opposite = Point(2 * center.x_nm - edge.x_nm, 2 * center.y_nm - edge.y_nm)
        top = _rotate_about(edge, center, 90)
        bottom = _rotate_about(edge, center, -90)
        e, o, t, b = (transform(p) for p in (edge, opposite, top, bottom))
        return [_Edge(e, o, t), _Edge(o, e, b)]
    if tag == "arc":
        if _first(node, "mid") is not None:
            return [_Edge(point("start"), point("end"), point("mid"))]
        center = _xy(_first(node, "start"), source)
        start = _xy(_first(node, "end"), source)
        sweep = float(Decimal(_atom(_first(node, "angle"), 1) or "0"))
        a, m, b = _legacy_arc(center, start, sweep)
        return [_Edge(transform(a), transform(b), transform(m))]
    if tag in {"poly", "curve"}:
        pts = _first(node, "pts")
        points: list[Point] = []
        for item in _lists(pts[1:] if pts else []):
            if _tag(item) == "xy":
                points.append(transform(_xy(item, source)))
            elif _tag(item) == "arc":
                a = transform(_xy(_first(item, "start"), source))
                m = transform(_xy(_first(item, "mid"), source))
                b = transform(_xy(_first(item, "end"), source))
                points.extend(_arc_polyline(a, m, b)[:-1] + [b])
                log.approx("outline polygon arc as line segments")
        if tag == "curve" and len(points) == 4:
            points = _bezier_points(points)
            log.approx("outline Bezier curve as line segments")
            return [_Edge(p, q) for p, q in zip(points, points[1:]) if p != q]
        if len(points) >= 3:
            return [_Edge(p, q) for p, q in zip(points, points[1:] + points[:1]) if p != q]
        return []
    log.warn(f"ignored outline item {tag}")
    return []


def _outline(edges: list[_Edge], placements: list[Placement], footprints: dict[str, PhysicalFootprint],
             source: str, log: _Log) -> BoardOutline:
    if not edges:
        log.approx("no Edge.Cuts outline: placement bounding box plus 2 mm")
        return _bounding_outline(placements, footprints, source)
    loops = _chain(edges, log)
    if not loops:
        log.approx("Edge.Cuts not closed: outline is its bounding box")
        points = [p for edge in edges for p in (edge.start, edge.end, *((edge.mid,) if edge.mid else ()))]
        x0, y0 = min(p.x_nm for p in points), min(p.y_nm for p in points)
        x1, y1 = max(p.x_nm for p in points), max(p.y_nm for p in points)
        if x1 - x0 <= 0 or y1 - y0 <= 0:
            raise KiCadPcbImportError(f"{source}: Edge.Cuts encloses no area")
        return BoardOutline((Point(x0, y0), Point(x1, y0), Point(x1, y1), Point(x0, y1)))
    rings = [(_loop_ring(loop), loop) for loop in loops]
    rings.sort(key=lambda item: -abs(_area(item[0])))
    outer_ring, outer_loop = rings[0]
    candidates = []
    for index, (ring, _) in enumerate(rings[1:], 1):
        try:
            candidates.append(BoardCutout(f"cutout-{index}", tuple(ring)))
        except ValueError:
            log.approx("degenerate outline loop ignored")
    cutouts = []
    for cutout in candidates:
        try:
            BoardOutline(tuple(outer_ring), (*cutouts, cutout))
            cutouts.append(cutout)
        except ValueError:
            log.approx("outline loop not strictly inside the board ignored")
    try:
        if 2 <= len(outer_loop) <= 256:
            segments = tuple(
                BoundaryArc(f"edge-{index}", edge.start, edge.mid, edge.end) if edge.mid is not None
                else BoundaryLine(f"edge-{index}", edge.start, edge.end)
                for index, edge in enumerate(outer_loop))
            return BoardOutline.from_path(BoardBoundaryPath(segments), cutouts=tuple(cutouts))
        return BoardOutline(tuple(outer_ring), tuple(cutouts))
    except ValueError as exc:
        try:
            log.approx("outline path rejected; used its polygon")
            return BoardOutline(tuple(outer_ring), tuple(cutouts))
        except ValueError:
            raise KiCadPcbImportError(f"{source}: invalid board outline: {exc}") from exc


def _chain(edges: list[_Edge], log: _Log) -> list[list[_Edge]]:
    """Join edges into closed loops, snapping endpoints closer than 2 µm."""
    canonical: list[Point] = []

    def snap(point: Point) -> Point:
        for known in canonical:
            if abs(known.x_nm - point.x_nm) <= _OUTLINE_SNAP_NM and abs(known.y_nm - point.y_nm) <= _OUTLINE_SNAP_NM:
                if known != point:
                    log.approx("outline endpoints snapped together")
                return known
        canonical.append(point)
        return point

    pending = [_Edge(snap(edge.start), snap(edge.end), edge.mid) for edge in edges]
    pending = [edge for edge in pending if edge.start != edge.end]
    loops: list[list[_Edge]] = []
    while pending:
        loop = [pending.pop(0)]
        while loop[-1].end != loop[0].start:
            tail = loop[-1].end
            index = next((i for i, edge in enumerate(pending) if edge.start == tail or edge.end == tail), None)
            if index is None:
                log.approx("open outline chain ignored")
                loop = []
                break
            edge = pending.pop(index)
            loop.append(edge if edge.start == tail else _Edge(edge.end, edge.start, edge.mid))
        if len(loop) >= 2 or (len(loop) == 1 and loop[0].mid is not None):
            loops.append(loop)
    return loops


def _loop_ring(loop: list[_Edge]) -> list[Point]:
    ring: list[Point] = []
    for edge in loop:
        points = _arc_polyline(edge.start, edge.mid, edge.end) if edge.mid is not None else [edge.start, edge.end]
        ring.extend(points[:-1])
    return ring


def _area(ring: list[Point]) -> float:
    return sum(a.x_nm * b.y_nm - b.x_nm * a.y_nm for a, b in zip(ring, ring[1:] + ring[:1])) / 2


def _bounding_outline(placements: list[Placement], footprints: dict[str, PhysicalFootprint],
                      source: str) -> BoardOutline:
    from ..placement import transformed_local_point
    points = []
    for placement in placements:
        footprint = footprints[placement.footprint]
        for pad in footprint.pads:
            half = max(pad.size.width_nm, pad.size.height_nm)
            centre = transformed_local_point(placement, pad.position)
            points.extend((Point(centre.x_nm - half, centre.y_nm - half), Point(centre.x_nm + half, centre.y_nm + half)))
        for corner in footprint.courtyard or ():
            points.append(transformed_local_point(placement, corner))
    if not points:
        raise KiCadPcbImportError(f"{source}: no outline and no placed geometry")
    margin = nm_from_mm(2)
    x0, y0 = min(p.x_nm for p in points) - margin, min(p.y_nm for p in points) - margin
    x1, y1 = max(p.x_nm for p in points) + margin, max(p.y_nm for p in points) + margin
    return BoardOutline((Point(x0, y0), Point(x1, y0), Point(x1, y1), Point(x0, y1)))


# --------------------------------------------------------------------------- zones and copper

def _polygon_rings(zone: list[SExpr], source: str, log: _Log) -> list[list[Point]]:
    rings = []
    for polygon in _children(zone, "polygon"):
        pts = _first(polygon, "pts")
        points: list[Point] = []
        for item in _lists(pts[1:] if pts else []):
            if _tag(item) == "xy":
                points.append(_xy(item, source))
            elif _tag(item) == "arc":
                a, m, b = (_xy(_first(item, t), source) for t in ("start", "mid", "end"))
                points.extend(_arc_polyline(a, m, b)[:-1] + [b])
                log.approx("zone outline arc as line segments")
        if len(points) >= 3:
            rings.append(points)
    return rings


def _import_zone(node: list[SExpr], copper: tuple[CopperLayer, ...], identifier: str,
                 log: _Log) -> tuple[list[CopperKeepout], list[CopperZone]]:
    layers = _layer_set(node, copper)
    rings = _polygon_rings(node, identifier, log)
    if not layers or not rings:
        log.warn("ignored zone without copper layers or outline")
        return [], []
    name = _atom(_first(node, "name"), 1) or identifier
    keepout = _first(node, "keepout")
    if keepout is not None:
        rules = {_tag(item): _atom(item, 1) for item in _lists(keepout[1:])}
        blocks = {key: rules.get(key) == "not_allowed" for key in ("tracks", "vias", "pads", "copperpour", "footprints")}
        if not any((blocks["tracks"], blocks["vias"], blocks["pads"], blocks["copperpour"])):
            return [], []
        result = []
        for index, ring in enumerate(rings):
            try:
                result.append(CopperKeepout(
                    f"{name}-{index}" if len(rings) > 1 else name, layers,
                    PolygonWithHoles(PolygonRing(tuple(ring))),
                    block_tracks=blocks["tracks"], block_vias=blocks["vias"], block_pads=blocks["pads"],
                    block_zones=blocks["copperpour"], block_footprints=blocks["footprints"]))
            except ValueError:
                log.approx("degenerate keepout ignored")
        return result, []
    net_name = _atom(_first(node, "net_name"), 1)
    if not net_name:
        net_node = _first(node, "net")
        strings = [item for item in (net_node or [])[1:] if isinstance(item, str) and not item.isdigit()]
        net_name = strings[-1] if strings else ""
    if not net_name:
        log.warn("ignored copper zone without a net")
        return [], []
    priority = int(_atom(_first(node, "priority"), 1) or 0)
    connect = _first(node, "connect_pads")
    clearance = _first(connect, "clearance") if connect is not None else None
    minimum = _first(node, "min_thickness")
    zones = []
    for index, ring in enumerate(rings):
        try:
            zones.append(CopperZone(
                f"{identifier}-{net_name}-{index}", net_name, layers,
                PolygonWithHoles(PolygonRing(tuple(ring))), priority=priority,
                clearance_nm=_nm(clearance[1], identifier) if clearance is not None else None,
                **({"minimum_width_nm": _nm(minimum[1], identifier)}
                   if minimum is not None and _nm(minimum[1], identifier) > 0 else {})))
        except ValueError:
            log.approx("degenerate zone ignored")
    return [], zones


def _copper_graphic_keepouts(node: list[SExpr], layer: CopperLayer, identifier: str,
                             log: _Log) -> list[CopperKeepout]:
    """Copper drawn as graphics: a track/via/zone obstacle for every net."""
    edges = _shape_edges(_tag(node).split("_", 1)[1], node, lambda point: point, identifier, log)
    stroke = _first(_first(node, "stroke") or [], "width") or _first(node, "width")
    half = (_nm(stroke[1], identifier) // 2) if stroke is not None else 0
    log.approx("copper graphic as keepout for all nets")
    result = []
    for index, edge in enumerate(edges):
        xs = [edge.start.x_nm, edge.end.x_nm] + ([edge.mid.x_nm] if edge.mid else [])
        ys = [edge.start.y_nm, edge.end.y_nm] + ([edge.mid.y_nm] if edge.mid else [])
        x0, x1, y0, y1 = min(xs) - half - 1, max(xs) + half + 1, min(ys) - half - 1, max(ys) + half + 1
        result.append(CopperKeepout(f"{identifier}-{index}", (layer,), PolygonWithHoles(PolygonRing((
            Point(x0, y0), Point(x1, y0), Point(x1, y1), Point(x0, y1))))))
    return result


# --------------------------------------------------------------------------- rules

@dataclass(frozen=True, slots=True)
class _NetClass:
    name: str
    clearance_nm: int
    track_nm: int
    via_nm: int
    via_drill_nm: int


def _design_rules(root: list[SExpr], project: dict | None, net_pads: dict[str, list[PadReference]],
                  log: _Log) -> tuple[DesignRules, tuple[NetRoutingRule, ...]]:
    classes, assignments, floors = _project_classes(project, net_pads) if project else _legacy_classes(root)
    default = classes.get("Default") or _NetClass("Default", _KICAD_DEFAULT_CLEARANCE_NM, _KICAD_DEFAULT_TRACK_NM,
                                                  _KICAD_DEFAULT_VIA_NM, _KICAD_DEFAULT_VIA_DRILL_NM)
    if not classes:
        log.approx("no net classes: KiCad default rules")
    hole_clearance = floors.get("min_hole_clearance", 0)
    if hole_clearance <= 0:
        hole_clearance = default.clearance_nm
        log.approx("zero hole clearance floor: used default net-class clearance")
    track_floor = floors.get("min_track_width", 0)
    widths = [default.track_nm] + [item.track_nm for item in classes.values()]
    minimum_track = min([w for w in widths if w > 0] + ([track_floor] if track_floor > 0 else []))
    via_drill = min(default.via_drill_nm, default.via_nm - 1)
    rules = DesignRules(
        minimum_clearance_nm=max(1, default.clearance_nm),
        minimum_hole_clearance_nm=max(1, hole_clearance),
        minimum_track_width_nm=max(1, min(minimum_track, default.track_nm)),
        default_track_width_nm=max(1, default.track_nm),
        default_via_size_nm=max(2, default.via_nm),
        default_via_drill_nm=max(1, via_drill),
    )
    net_rules = []
    for net in sorted(net_pads):
        cls = classes.get(assignments.get(net, "Default"))
        if cls is None or cls.name == "Default" or (cls.track_nm, cls.clearance_nm) == (default.track_nm, default.clearance_nm):
            continue
        net_rules.append(NetRoutingRule(net, width_nm=cls.track_nm or None, clearance_nm=cls.clearance_nm or None))
    return rules, tuple(net_rules)


def _project_classes(project: dict, net_pads: dict[str, list[PadReference]]):
    settings = project.get("net_settings", {}) or {}
    classes: dict[str, _NetClass] = {}
    assignments: dict[str, str] = {}
    for item in settings.get("classes", []) or []:
        def mm(key, fallback):
            value = item.get(key)
            return nm_from_mm(str(value)) if isinstance(value, (int, float)) and value > 0 else fallback
        cls = _NetClass(item.get("name", "Default"), mm("clearance", _KICAD_DEFAULT_CLEARANCE_NM),
                        mm("track_width", _KICAD_DEFAULT_TRACK_NM), mm("via_diameter", _KICAD_DEFAULT_VIA_NM),
                        mm("via_drill", _KICAD_DEFAULT_VIA_DRILL_NM))
        classes[cls.name] = cls
        for net in item.get("nets", []) or []:  # KiCad 6/7
            assignments[net] = cls.name
    for net, value in (settings.get("netclass_assignments") or {}).items():  # KiCad 7+
        names = value if isinstance(value, list) else [value]
        if names:
            assignments[net] = names[0]
    for pattern in settings.get("netclass_patterns", []) or []:  # KiCad 8+
        regex = _wildcard(pattern.get("pattern", ""))
        for net in net_pads:
            if net not in assignments and regex.fullmatch(net):
                assignments[net] = pattern.get("netclass", "Default")
    rules = (project.get("board", {}) or {}).get("design_settings", {}).get("rules", {}) or {}
    floors = {key: nm_from_mm(str(value)) for key, value in rules.items()
              if key in {"min_clearance", "min_track_width", "min_hole_clearance"}
              and isinstance(value, (int, float)) and value > 0}
    return classes, assignments, floors


def _legacy_classes(root: list[SExpr]):
    classes: dict[str, _NetClass] = {}
    assignments: dict[str, str] = {}
    for node in _children(root, "net_class"):
        name = _atom(node, 1) or "Default"
        def value(tag, fallback):
            child = _first(node, tag)
            return _nm(child[1], "net_class") if child is not None else fallback
        classes[name] = _NetClass(name, value("clearance", _KICAD_DEFAULT_CLEARANCE_NM),
                                  value("trace_width", _KICAD_DEFAULT_TRACK_NM),
                                  value("via_dia", _KICAD_DEFAULT_VIA_NM), value("via_drill", _KICAD_DEFAULT_VIA_DRILL_NM))
        for add in _children(node, "add_net"):
            if _atom(add, 1):
                assignments[_atom(add, 1)] = name
    return classes, assignments, {}


def _wildcard(pattern: str) -> re.Pattern[str]:
    return re.compile("".join(".*" if ch == "*" else "." if ch == "?" else re.escape(ch) for ch in pattern))


# --------------------------------------------------------------------------- text

def _mm(value_nm: int) -> str:
    return _decimal_text(Decimal(value_nm) / Decimal(1_000_000))


def _decimal_text(value: Decimal) -> str:
    text = format(value.normalize(), "f")
    return "0" if text in {"-0", ""} else text


def _dump(node: SExpr) -> str:
    if isinstance(node, str):
        if node and not any(ch in node for ch in ' ()"\t\n\r\\'):
            return node
        return '"' + node.replace("\\", "\\\\").replace('"', '\\"') + '"'
    return "(" + " ".join(_dump(item) for item in node) + ")"

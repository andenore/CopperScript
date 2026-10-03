"""Import KiCad ``.kicad_mod`` files into normalized physical IR.

The importer intentionally supports a conservative subset of KiCad 6 through
10 footprints, including explicit jumper-pad groups. Fabrication-relevant
constructs that cannot yet be represented raise an error instead of being
silently discarded. Presentation-only items
such as 3D models and user text are reported as warnings and can be promoted to
errors with ``strict=True``.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP
from hashlib import sha256
from math import hypot, isqrt
from pathlib import Path
from typing import Iterator, TypeAlias
from ..pad_connections import InternalPadGroup, merge_internal_pad_groups

from ..physical import (
    CopperKeepout,
    CopperLayer,
    FootprintArc,
    FootprintCircle,
    FootprintGraphic,
    FootprintLayer,
    FootprintLine,
    FootprintPad,
    FootprintPolygon,
    FootprintRectangle,
    PadKind,
    PadShape,
    PhysicalFootprint,
    Point,
    PolygonRing,
    PolygonWithHoles,
    Size,
    ZoneConnection,
    nm_from_mm,
)


SExpr: TypeAlias = str | list["SExpr"]


class KiCadModImportError(ValueError):
    """A malformed or unsupported KiCad footprint construct."""


@dataclass(frozen=True, slots=True)
class FootprintImportResult:
    footprint: PhysicalFootprint
    warnings: tuple[str, ...]
    source_version: str | None


@dataclass(frozen=True, slots=True)
class _Token:
    value: str
    line: int
    column: int


_LAYER_MAP = {
    "F.SilkS": FootprintLayer.SILKSCREEN,
    "F.Fab": FootprintLayer.FABRICATION,
    "F.CrtYd": FootprintLayer.COURTYARD,
    "F.Adhes": FootprintLayer.ADHESIVE,
    "Dwgs.User": FootprintLayer.DOCUMENTATION,
    "User.Drawings": FootprintLayer.DOCUMENTATION,
    "F.Mask": FootprintLayer.SOLDER_MASK,
    "F.Paste": FootprintLayer.SOLDER_PASTE,
}


def load_kicad_mod(path: str | Path, *, strict: bool = False) -> FootprintImportResult:
    """Read and import one KiCad footprint library file."""

    source_path = Path(path)
    if source_path.suffix.casefold() != ".kicad_mod":
        raise KiCadModImportError(
            f"{source_path}: expected a .kicad_mod footprint file"
        )
    try:
        data = source_path.read_bytes()
        text = data.decode("utf-8-sig")
    except (OSError, UnicodeDecodeError) as exc:
        raise KiCadModImportError(f"cannot read {source_path}: {exc}") from exc
    result = parse_kicad_mod(text, source=str(source_path), strict=strict)
    metadata = dict(result.footprint.metadata)
    metadata["source_sha256"] = sha256(data).hexdigest()
    return replace(result, footprint=replace(result.footprint, metadata=metadata))


def parse_kicad_mod(
    text: str,
    *,
    source: str = "<memory>",
    strict: bool = False,
) -> FootprintImportResult:
    """Parse one KiCad footprint into backend-neutral physical IR."""

    root = _parse_sexpr(text, source)
    if _tag(root) not in {"footprint", "module"}:
        raise KiCadModImportError(
            f"{source}: root expression must be 'footprint', got {_tag(root)!r}"
        )
    name = _required_atom(root, 1, source, "footprint name")
    footprint_layer_node = _first(root, "layer")
    if footprint_layer_node is not None:
        footprint_layer = _required_atom(
            footprint_layer_node, 1, source, "footprint layer"
        )
        if footprint_layer != "F.Cu":
            raise KiCadModImportError(
                f"{source}: only front-side footprints are supported; "
                f"got layer {footprint_layer!r}"
            )
    version_node = _first(root, "version")
    version = _atom(version_node, 1) if version_node is not None else None
    generator_node = _first(root, "generator")
    generator = _atom(generator_node, 1) if generator_node is not None else ""

    warnings: list[str] = []
    clearance_node = _first(root, "clearance")
    clearance_nm = (
        _number_nm(_required_atom(clearance_node, 1, source, "footprint clearance"), source, "footprint clearance")
        if clearance_node is not None else None
    )
    pads: list[FootprintPad] = []
    graphics: list[FootprintGraphic] = []
    keepouts: list[CopperKeepout] = []
    footprint_attributes: set[str] = set()
    duplicate_jumpers = False
    internal_groups = []
    for child in _lists(root[2:]):
        child_tag = _tag(child)
        if child_tag == "pad":
            pads.append(_parse_pad(child, source))
        elif child_tag == "zone":
            keepouts.append(_parse_footprint_keepout(child, source, len(keepouts)))
        elif child_tag in {"fp_line", "fp_rect", "fp_circle", "fp_arc", "fp_poly"}:
            graphic = _parse_graphic(child, source, warnings)
            if graphic is not None:
                graphics.append(graphic)
        elif child_tag in {"fp_text", "fp_text_box"}:
            text_kind = _atom(child, 1) or "user"
            if text_kind not in {"reference", "value"}:
                warnings.append(f"ignored footprint text ({text_kind})")
        elif child_tag == "model":
            warnings.append("ignored 3D model reference")
        elif child_tag == "duplicate_pad_numbers_are_jumpers":
            value = _required_atom(child, 1, source, "duplicate pad jumper setting")
            if value not in {"yes", "no"}:
                raise KiCadModImportError(f"{source}: invalid duplicate pad jumper setting")
            duplicate_jumpers = value == "yes"
        elif child_tag == "jumper_pad_groups":
            try:
                for group in child[1:]:
                    if not isinstance(group, list) or any(not isinstance(n, str) for n in group):
                        raise ValueError("jumper groups require lists of pad numbers")
                    internal_groups.append(InternalPadGroup(tuple(group)))
            except ValueError as exc:
                raise KiCadModImportError(f"{source}: {exc}") from exc
        elif child_tag == "embedded_fonts":
            if _required_atom(child, 1, source, "embedded fonts setting") != "no":
                warnings.append("ignored embedded footprint fonts")
        elif child_tag == "attr":
            attributes = {item for item in child[1:] if isinstance(item, str)}
            unsupported = attributes - {
                "smd", "through_hole", "exclude_from_bom", "exclude_from_pos_files"
            }
            if unsupported:
                raise KiCadModImportError(
                    f"{source}: unsupported footprint attributes {sorted(unsupported)!r}"
                )
            footprint_attributes.update(attributes)
        elif child_tag in {
            "solder_mask_margin",
            "solder_paste_margin",
            "solder_paste_ratio",
            "zone_connect",
            "thermal_width",
            "thermal_gap",
            "private_layers",
            "net_tie_pad_groups",
        }:
            raise KiCadModImportError(
                f"{source}: unsupported fabrication-critical footprint setting "
                f"{child_tag!r}"
            )
        elif child_tag in {
            "version",
            "generator",
            "generator_version",
            "layer",
            "clearance",
            "descr",
            "tags",
            "property",
            "uuid",
            "tedit",
        }:
            continue
        elif child_tag:
            warnings.append(f"ignored unsupported footprint item {child_tag!r}")

    if not pads:
        warnings.append("footprint contains no pads")
    warnings = list(dict.fromkeys(warnings))
    if strict and warnings:
        raise KiCadModImportError(f"{source}: {warnings[0]}")

    metadata = {
        "source_format": "kicad_mod",
        "source_path": source,
        "source_sha256": sha256(text.encode("utf-8")).hexdigest(),
    }
    if version:
        metadata["source_version"] = version
    if generator:
        metadata["source_generator"] = generator
    if warnings:
        metadata["import_warnings"] = " | ".join(warnings)

    if duplicate_jumpers:
        from collections import Counter
        counts = Counter(p.number for p in pads if p.number and p.kind not in {
            PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE})
        internal_groups.extend(InternalPadGroup((number,)) for number, count in counts.items() if count > 1)
    try:
        footprint = PhysicalFootprint(
            name=name,
            pads=tuple(pads),
            body_size=_bounding_size(pads, graphics),
            source_library_id=name,
            graphics=tuple(graphics),
            keepouts=tuple(keepouts),
            metadata=metadata,
            courtyard=_courtyard_polygon(graphics),
            clearance_nm=clearance_nm,
            exclude_from_bom="exclude_from_bom" in footprint_attributes,
            exclude_from_pos_files="exclude_from_pos_files" in footprint_attributes,
            internal_pad_groups=merge_internal_pad_groups(internal_groups),
        )
    except ValueError as exc:
        raise KiCadModImportError(f"{source}: {exc}") from exc
    return FootprintImportResult(footprint, tuple(warnings), version)


def _parse_footprint_keepout(
    node: list[SExpr], source: str, index: int
) -> CopperKeepout:
    """Import the copper effect of a footprint-local KiCad keepout zone."""

    keepout_node = _required_child(node, "keepout", source, "footprint keepout")
    layer_node = _first(node, "layer") or _first(node, "layers")
    if layer_node is None:
        raise KiCadModImportError(f"{source}: footprint keepout has no layers")
    raw_layers = tuple(item for item in layer_node[1:] if isinstance(item, str))
    if not raw_layers or any(item not in {"F.Cu", "F.CrtYd"} for item in raw_layers):
        raise KiCadModImportError(
            f"{source}: unsupported footprint keepout layers {raw_layers!r}"
        )
    if "F.Cu" not in raw_layers:
        raise KiCadModImportError(f"{source}: footprint keepout has no copper layer")
    placement_node = _first(node, "placement")
    if placement_node is not None:
        enabled = _first(placement_node, "enabled")
        if enabled is not None and _required_atom(
            enabled, 1, source, "placement keepout state"
        ) != "no":
            raise KiCadModImportError(
                f"{source}: active footprint placement keepouts are unsupported"
            )
    for tag in ("net", "net_name"):
        item = _first(node, tag)
        if item is not None and _required_atom(item, 1, source, tag) not in {"", "0"}:
            raise KiCadModImportError(
                f"{source}: connected footprint zones are unsupported"
            )

    blocked: dict[str, bool] = {}
    for item in _lists(keepout_node[1:]):
        tag = _tag(item)
        if tag not in {"tracks", "vias", "pads", "copperpour", "footprints"}:
            raise KiCadModImportError(
                f"{source}: unsupported footprint keepout rule {tag!r}"
            )
        value = _required_atom(item, 1, source, f"keepout {tag}")
        if value not in {"allowed", "not_allowed"}:
            raise KiCadModImportError(f"{source}: invalid keepout {tag} rule")
        blocked[tag] = value == "not_allowed"
    if not {"tracks", "vias", "pads", "copperpour"} <= blocked.keys():
        raise KiCadModImportError(f"{source}: incomplete footprint keepout rules")

    polygon = _required_child(node, "polygon", source, "keepout polygon")
    pts = _required_child(polygon, "pts", source, "keepout polygon points")
    if any(_tag(item) != "xy" for item in _lists(pts[1:])):
        raise KiCadModImportError(f"{source}: unsupported keepout polygon item")
    vertices = tuple(_point(item, source) for item in _children(pts, "xy"))
    try:
        outline = PolygonWithHoles(PolygonRing(vertices))
    except ValueError as exc:
        raise KiCadModImportError(f"{source}: invalid keepout polygon: {exc}") from exc
    allowed = {
        "layer", "layers", "uuid", "name", "hatch", "connect_pads",
        "min_thickness", "keepout", "placement", "fill", "polygon", "net", "net_name",
    }
    for child in _lists(node[1:]):
        if _tag(child) not in allowed:
            raise KiCadModImportError(
                f"{source}: unsupported footprint keepout item {_tag(child)!r}"
            )
    name_node = _first(node, "name")
    name = _atom(name_node, 1) if name_node is not None else None
    return CopperKeepout(
        id=f"{name or 'keepout'}-{index}",
        layers=(CopperLayer.FRONT,),
        outline=outline,
        block_tracks=blocked["tracks"],
        block_vias=blocked["vias"],
        block_pads=blocked["pads"],
        block_zones=blocked["copperpour"],
        block_footprints=blocked.get("footprints", False),
    )


def _parse_pad(node: list[SExpr], source: str) -> FootprintPad:
    number = _required_atom(node, 1, source, "pad number")
    kind_text = _required_atom(node, 2, source, "pad kind")
    shape_text = _required_atom(node, 3, source, "pad shape")
    try:
        kind = {
            "smd": PadKind.SMD,
            "connect": PadKind.SMD,
            "thru_hole": PadKind.THROUGH_HOLE,
            "np_thru_hole": PadKind.NON_PLATED_THROUGH_HOLE,
        }[kind_text]
    except KeyError as exc:
        raise KiCadModImportError(f"{source}: unsupported pad kind {kind_text!r}") from exc
    try:
        shape = {
            "circle": PadShape.CIRCLE,
            "oval": PadShape.OVAL,
            "rect": PadShape.RECTANGLE,
            "roundrect": PadShape.ROUNDRECT,
        }[shape_text]
    except KeyError as exc:
        raise KiCadModImportError(
            f"{source}: unsupported fabrication-critical pad shape {shape_text!r}"
        ) from exc

    at = _required_child(node, "at", source, "pad position")
    position = _point(at, source)
    rotation = _decimal(_atom(at, 3) or "0", source, "pad rotation")
    size_node = _required_child(node, "size", source, "pad size")
    size = _size(size_node, source)
    layers_node = _required_child(node, "layers", source, "pad layers")
    layers = tuple(item for item in layers_node[1:] if isinstance(item, str))
    if kind is PadKind.SMD and "F.Cu" not in layers and set(layers) <= {"F.Mask", "F.Paste"}:
        kind = PadKind.APERTURE
    has_mask, has_paste = _validate_pad_layers(kind, layers, source)
    if kind_text == "connect" and (not has_mask or has_paste):
        raise KiCadModImportError(
            f"{source}: connector-contact pad {number!r} must have mask but no paste"
        )
    drill = _parse_drill(_first(node, "drill"), kind, source)

    ratio_node = _first(node, "roundrect_rratio")
    ratio_ppm = 250_000
    if ratio_node is not None:
        ratio = _decimal(
            _required_atom(ratio_node, 1, source, "roundrect ratio"),
            source,
            "roundrect ratio",
        )
        ratio_ppm = int(
            (ratio * Decimal(1_000_000)).to_integral_value(rounding=ROUND_HALF_UP)
        )

    property_node = _first(node, "property")
    heatsink = False
    if property_node is not None:
        property_name = _required_atom(property_node, 1, source, "pad property")
        if property_name != "pad_prop_heatsink":
            raise KiCadModImportError(
                f"{source}: unsupported fabrication-critical pad property {property_name!r}"
            )
        heatsink = True
    zone_node = _first(node, "zone_connect")
    zone_connection = None
    if zone_node is not None:
        zone_value = _required_atom(zone_node, 1, source, "pad zone connection")
        try:
            zone_connection = {
                "1": ZoneConnection.NONE,
                "2": ZoneConnection.THERMAL,
                "3": ZoneConnection.SOLID,
                "4": ZoneConnection.THT_THERMAL,
            }[zone_value]
        except KeyError as exc:
            raise KiCadModImportError(
                f"{source}: unsupported pad zone connection {zone_value!r}"
            ) from exc
    remove_node = _first(node, "remove_unused_layers")
    remove_unused_layers = False
    if remove_node is not None:
        remove_value = _required_atom(remove_node, 1, source, "remove unused layers")
        if remove_value not in {"yes", "no"}:
            raise KiCadModImportError(f"{source}: invalid remove_unused_layers value")
        remove_unused_layers = remove_value == "yes"

    thermal_angle = _first(node, "thermal_bridge_angle")
    if thermal_angle is not None and _required_atom(
        thermal_angle, 1, source, "pad thermal angle"
    ) != "45":
        raise KiCadModImportError(
            f"{source}: pad {number!r} uses unsupported non-default thermal bridge angle"
        )

    allowed_children = {
        "at",
        "size",
        "layers",
        "roundrect_rratio",
        "drill",
        "uuid",
        "tstamp",
        "pinfunction",
        "pintype",
        "property",
        "zone_connect",
        "remove_unused_layers",
        "thermal_bridge_angle",
    }
    for child in _lists(node[4:]):
        if _tag(child) not in allowed_children:
            raise KiCadModImportError(
                f"{source}: pad {number!r} uses unsupported fabrication modifier "
                f"{_tag(child)!r}"
            )

    return FootprintPad(
        number=number,
        position=position,
        size=size,
        kind=kind,
        shape=shape,
        rotation_degrees=rotation,
        drill=drill,
        roundrect_ratio_ppm=ratio_ppm,
        has_solder_mask=has_mask,
        has_solder_paste=has_paste,
        zone_connection=zone_connection,
        heatsink=heatsink,
        remove_unused_layers=remove_unused_layers,
        connector_contact=kind_text == "connect",
    )


def _validate_pad_layers(
    kind: PadKind, layers: tuple[str, ...], source: str
) -> tuple[bool, bool]:
    layer_set = set(layers)
    if kind in {PadKind.SMD, PadKind.APERTURE}:
        allowed = {"F.Cu", "F.Mask", "F.Paste"}
        expected_copper = kind is PadKind.SMD
        if ("F.Cu" in layer_set) != expected_copper or not layer_set <= allowed:
            raise KiCadModImportError(
                f"{source}: unsupported {kind.value} pad layers {layers!r}"
            )
        return "F.Mask" in layer_set, "F.Paste" in layer_set
    allowed = {"*.Cu", "*.Mask"}
    if "*.Cu" not in layer_set or not layer_set <= allowed:
        raise KiCadModImportError(
            f"{source}: unsupported through-hole pad layers {layers!r}"
        )
    return "*.Mask" in layer_set, False


def _parse_drill(
    node: list[SExpr] | None, kind: PadKind, source: str
) -> Size | None:
    if node is None:
        if kind in {PadKind.SMD, PadKind.APERTURE}:
            return None
        raise KiCadModImportError(f"{source}: through-hole pad is missing a drill")
    if kind in {PadKind.SMD, PadKind.APERTURE}:
        raise KiCadModImportError(f"{source}: non-drilled pad unexpectedly contains a drill")
    if any(isinstance(item, list) for item in node[1:]):
        raise KiCadModImportError(
            f"{source}: offset or compound drill definitions are not supported"
        )
    values = [item for item in node[1:] if isinstance(item, str)]
    if not values:
        raise KiCadModImportError(f"{source}: empty drill definition")
    if values[0] == "oval":
        if len(values) != 3:
            raise KiCadModImportError(f"{source}: oval drill requires width and height")
        return Size(
            _number_nm(values[1], source, "drill width"),
            _number_nm(values[2], source, "drill height"),
        )
    if len(values) != 1:
        raise KiCadModImportError(f"{source}: unsupported drill definition {values!r}")
    diameter = _number_nm(values[0], source, "drill diameter")
    return Size(diameter, diameter)


def _parse_graphic(
    node: list[SExpr], source: str, warnings: list[str]
) -> FootprintGraphic | None:
    layer_node = _required_child(node, "layer", source, "graphic layer")
    layer_name = _required_atom(layer_node, 1, source, "graphic layer")
    layer = _LAYER_MAP.get(layer_name)
    if layer is None:
        if layer_name in {"F.Cu", "Edge.Cuts"}:
            raise KiCadModImportError(
                f"{source}: unsupported fabrication-critical graphic layer {layer_name!r}"
            )
        warnings.append(f"ignored graphic on unsupported layer {layer_name!r}")
        return None
    width = _graphic_width(node, source)
    filled = _graphic_filled(node, source)
    tag = _tag(node)
    if tag == "fp_line":
        start = _point(_required_child(node, "start", source, "line start"), source)
        end = _point(_required_child(node, "end", source, "line end"), source)
        if start == end:
            warnings.append(f"ignored zero-length graphic on {layer_name}")
            return None
        return FootprintLine(
            start,
            end,
            width,
            layer,
        )
    if tag == "fp_rect":
        return FootprintRectangle(
            _point(_required_child(node, "start", source, "rectangle start"), source),
            _point(_required_child(node, "end", source, "rectangle end"), source),
            width,
            layer,
            filled,
        )
    if tag == "fp_circle":
        return FootprintCircle(
            _point(_required_child(node, "center", source, "circle center"), source),
            _point(_required_child(node, "end", source, "circle end"), source),
            width,
            layer,
            filled,
        )
    if tag == "fp_arc":
        if filled:
            raise KiCadModImportError(f"{source}: filled arcs are not supported")
        mid = _first(node, "mid")
        if mid is None:
            raise KiCadModImportError(
                f"{source}: legacy angle-based arcs are not supported yet"
            )
        return FootprintArc(
            _point(_required_child(node, "start", source, "arc start"), source),
            _point(mid, source),
            _point(_required_child(node, "end", source, "arc end"), source),
            width,
            layer,
        )
    if tag == "fp_poly":
        pts = _required_child(node, "pts", source, "polygon points")
        points = tuple(_point(item, source) for item in _children(pts, "xy"))
        return FootprintPolygon(points, width, layer, filled)
    raise KiCadModImportError(f"{source}: unsupported footprint graphic {tag!r}")


def _graphic_width(node: list[SExpr], source: str) -> int:
    stroke = _first(node, "stroke")
    width = _first(stroke, "width") if stroke is not None else _first(node, "width")
    if width is None:
        raise KiCadModImportError(f"{source}: footprint graphic is missing stroke width")
    return _number_nm(
        _required_atom(width, 1, source, "stroke width"), source, "stroke width"
    )


def _graphic_filled(node: list[SExpr], source: str) -> bool:
    fill = _first(node, "fill")
    if fill is None:
        return False
    value = _required_atom(fill, 1, source, "graphic fill")
    if value not in {"none", "solid", "no", "yes"}:
        raise KiCadModImportError(f"{source}: unsupported graphic fill {value!r}")
    return value in {"solid", "yes"}


def _bounding_size(
    pads: list[FootprintPad], graphics: list[FootprintGraphic]
) -> Size:
    points: list[Point] = []
    for pad in pads:
        half_width = pad.size.width_nm // 2
        half_height = pad.size.height_nm // 2
        points.extend(
            (
                Point(pad.position.x_nm - half_width, pad.position.y_nm - half_height),
                Point(pad.position.x_nm + half_width, pad.position.y_nm + half_height),
            )
        )
    for graphic in graphics:
        if isinstance(graphic, (FootprintLine, FootprintRectangle, FootprintArc)):
            points.extend((graphic.start, graphic.end))
            if isinstance(graphic, FootprintArc):
                points.append(graphic.midpoint)
        elif isinstance(graphic, FootprintCircle):
            dx = graphic.end.x_nm - graphic.center.x_nm
            dy = graphic.end.y_nm - graphic.center.y_nm
            squared = dx * dx + dy * dy
            radius = isqrt(squared)
            if radius * radius != squared:
                radius += 1
            points.extend(
                (
                    Point(graphic.center.x_nm - radius, graphic.center.y_nm - radius),
                    Point(graphic.center.x_nm + radius, graphic.center.y_nm + radius),
                )
            )
        elif isinstance(graphic, FootprintPolygon):
            points.extend(graphic.points)
    if not points:
        return Size.mm(1, 1)
    width = max(point.x_nm for point in points) - min(point.x_nm for point in points)
    height = max(point.y_nm for point in points) - min(point.y_nm for point in points)
    return Size(max(width, 1), max(height, 1))


def _courtyard_polygon(graphics: list[FootprintGraphic]) -> tuple[Point, ...]:
    courtyard = [
        graphic
        for graphic in graphics
        if getattr(graphic, "layer", None) is FootprintLayer.COURTYARD
    ]
    polygons = [
        graphic.points for graphic in courtyard if isinstance(graphic, FootprintPolygon)
    ]
    if len(polygons) == 1:
        return polygons[0]
    rectangles = [
        graphic for graphic in courtyard if isinstance(graphic, FootprintRectangle)
    ]
    if len(rectangles) == 1 and len(courtyard) == 1:
        rectangle = rectangles[0]
        return (
            rectangle.start,
            Point(rectangle.end.x_nm, rectangle.start.y_nm),
            rectangle.end,
            Point(rectangle.start.x_nm, rectangle.end.y_nm),
        )
    points: list[Point] = []
    for graphic in courtyard:
        if isinstance(graphic, (FootprintLine, FootprintRectangle, FootprintArc)):
            points.extend((graphic.start, graphic.end))
            if isinstance(graphic, FootprintArc):
                points.append(graphic.midpoint)
        elif isinstance(graphic, FootprintCircle):
            radius = round(
                hypot(
                    graphic.end.x_nm - graphic.center.x_nm,
                    graphic.end.y_nm - graphic.center.y_nm,
                )
            )
            points.extend(
                (
                    Point(graphic.center.x_nm - radius, graphic.center.y_nm - radius),
                    Point(graphic.center.x_nm + radius, graphic.center.y_nm + radius),
                )
            )
        elif isinstance(graphic, FootprintPolygon):
            points.extend(graphic.points)
    if not points:
        return ()
    min_x = min(point.x_nm for point in points)
    min_y = min(point.y_nm for point in points)
    max_x = max(point.x_nm for point in points)
    max_y = max(point.y_nm for point in points)
    return (
        Point(min_x, min_y),
        Point(max_x, min_y),
        Point(max_x, max_y),
        Point(min_x, max_y),
    )


def _parse_sexpr(text: str, source: str) -> list[SExpr]:
    tokens = list(_tokenize(text, source))
    index = 0

    def parse_list() -> list[SExpr]:
        nonlocal index
        result: list[SExpr] = []
        while index < len(tokens):
            token = tokens[index]
            index += 1
            if token.value == "(":
                result.append(parse_list())
            elif token.value == ")":
                return result
            else:
                result.append(token.value)
        raise KiCadModImportError(f"{source}: unclosed '(' expression")

    if not tokens or tokens[0].value != "(":
        raise KiCadModImportError(f"{source}: expected '(' at start of footprint")
    index = 1
    root = parse_list()
    if index != len(tokens):
        token = tokens[index]
        raise KiCadModImportError(
            f"{source}:{token.line}:{token.column}: unexpected token after footprint"
        )
    return root


def _tokenize(text: str, source: str) -> Iterator[_Token]:
    index = 0
    line = 1
    column = 1
    while index < len(text):
        character = text[index]
        if character.isspace():
            if character == "\n":
                line += 1
                column = 1
            else:
                column += 1
            index += 1
            continue
        if character == "#":
            while index < len(text) and text[index] != "\n":
                index += 1
                column += 1
            continue
        if character in "()":
            yield _Token(character, line, column)
            index += 1
            column += 1
            continue
        token_line, token_column = line, column
        if character == '"':
            index += 1
            column += 1
            value: list[str] = []
            while index < len(text) and text[index] != '"':
                current = text[index]
                if current == "\\":
                    index += 1
                    column += 1
                    if index >= len(text):
                        break
                    current = text[index]
                    current = {"n": "\n", "r": "\r", "t": "\t"}.get(
                        current, current
                    )
                value.append(current)
                if current == "\n":
                    line += 1
                    column = 1
                else:
                    column += 1
                index += 1
            if index >= len(text):
                raise KiCadModImportError(
                    f"{source}:{token_line}:{token_column}: unterminated string"
                )
            index += 1
            column += 1
            yield _Token("".join(value), token_line, token_column)
            continue
        start = index
        while index < len(text) and not text[index].isspace() and text[index] not in "()":
            index += 1
            column += 1
        yield _Token(text[start:index], token_line, token_column)


def _tag(node: list[SExpr] | None) -> str | None:
    return node[0] if node and isinstance(node[0], str) else None


def _lists(items: list[SExpr]) -> Iterator[list[SExpr]]:
    return (item for item in items if isinstance(item, list))


def _children(node: list[SExpr], tag: str) -> list[list[SExpr]]:
    return [child for child in _lists(node[1:]) if _tag(child) == tag]


def _first(node: list[SExpr] | None, tag: str) -> list[SExpr] | None:
    if node is None:
        return None
    return next((child for child in _lists(node[1:]) if _tag(child) == tag), None)


def _required_child(
    node: list[SExpr], tag: str, source: str, description: str
) -> list[SExpr]:
    child = _first(node, tag)
    if child is None:
        raise KiCadModImportError(f"{source}: missing {description}")
    return child


def _atom(node: list[SExpr] | None, index: int) -> str | None:
    if node is None or len(node) <= index or not isinstance(node[index], str):
        return None
    return node[index]


def _required_atom(
    node: list[SExpr], index: int, source: str, description: str
) -> str:
    value = _atom(node, index)
    if value is None:
        raise KiCadModImportError(f"{source}: missing {description}")
    return value


def _point(node: list[SExpr], source: str) -> Point:
    return Point(
        _number_nm(_required_atom(node, 1, source, "x coordinate"), source, "x"),
        _number_nm(_required_atom(node, 2, source, "y coordinate"), source, "y"),
    )


def _size(node: list[SExpr], source: str) -> Size:
    return Size(
        _number_nm(_required_atom(node, 1, source, "width"), source, "width"),
        _number_nm(_required_atom(node, 2, source, "height"), source, "height"),
    )


def _number_nm(value: str, source: str, description: str) -> int:
    number = _decimal(value, source, description)
    return nm_from_mm(number)


def _decimal(value: str, source: str, description: str) -> Decimal:
    try:
        return Decimal(value)
    except InvalidOperation as exc:
        raise KiCadModImportError(
            f"{source}: invalid {description} value {value!r}"
        ) from exc

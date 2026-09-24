"""Deterministic KiCad 8 PCB backend for the CopperScript physical IR."""

from __future__ import annotations

from hashlib import sha256
import json
import re
import uuid

from ..physical import (
    BoardSide,
    CopperLayer,
    CopperKeepout,
    CopperZone,
    FootprintArc,
    FootprintCircle,
    FootprintGraphic,
    FootprintLayer,
    FootprintLine,
    FootprintPad,
    FootprintPolygon,
    FootprintRectangle,
    PadKind,
    PadReference,
    PadShape,
    PhysicalBoard,
    PhysicalFootprint,
    IslandPolicy,
    Placement,
    Point,
    ZoneConnection,
    ViaKind,
)
from ..placement import resolved_copper_keepouts
from .base import Artifact, ArtifactManifest


KICAD_PCB_FORMAT = "20240108"
KICAD_PCB_TARGET_VERSION = "8.0"
_UUID_SEED = b"CopperScript KiCad PCB backend v0.1\0"


class KiCadPcbBackend:
    """Generate a self-contained KiCad 8 ``.kicad_pcb`` artifact."""

    name = "kicad-pcb"
    target_version = KICAD_PCB_TARGET_VERSION

    def generate(self, board: PhysicalBoard) -> ArtifactManifest:
        if board.stackup.copper_layers[0] is not CopperLayer.FRONT or board.stackup.copper_layers[-1] is not CopperLayer.BACK:
            raise ValueError("KiCad stackups must start at F.Cu and end at B.Cu")
        content = _render(board)
        warnings: list[str] = []
        if board.metadata.get("prototype_footprints") == "true":
            warnings.append(
                "The board uses generated proxy footprints and is for inspection only; "
                "resolve verified package footprints before fabrication."
            )
        if board.metadata.get("prototype_placement") == "true":
            warnings.append(
                "Components use deterministic draft placement; review and constrain "
                "placement before fabrication."
            )
        if board.metadata.get("planned_placement") == "true":
            warnings.append(
                "Components use an automated placement candidate with coarse "
                "routability estimation; review constraints before routing."
            )
        import_warnings = board.metadata.get("footprint_import_warnings")
        if import_warnings:
            warnings.extend(import_warnings.splitlines())
        if not board.tracks:
            warnings.append("The board contains no routed tracks.")
        return ArtifactManifest(
            backend=self.name,
            target_version=self.target_version,
            artifacts=(
                Artifact(
                    f"{_safe_name(board.name)}.kicad_pcb",
                    "application/x-kicad-pcb",
                    content,
                ),
            ),
            warnings=tuple(warnings),
        )


def _render(board: PhysicalBoard) -> str:
    net_codes = {
        net.name: index
        for index, net in enumerate(sorted(board.nets, key=lambda item: item.name), 1)
    }
    pad_nets = {
        pad: net.name
        for net in board.nets
        for pad in net.pads
    }
    lines = [
        "(kicad_pcb",
        f"  (version {KICAD_PCB_FORMAT})",
        '  (generator "copperscript")',
        '  (generator_version "0.1.0")',
        "  (general",
        f"    (thickness {_mm(board.stackup.thickness_nm)})",
        "  )",
        '  (paper "A4")',
        "  (layers",
        *_copper_layer_lines(board),
        '    (32 "B.Adhes" user "B.Adhesive")',
        '    (33 "F.Adhes" user "F.Adhesive")',
        '    (34 "B.Paste" user)',
        '    (35 "F.Paste" user)',
        '    (36 "B.SilkS" user "b.silkscreen")',
        '    (37 "F.SilkS" user "f.silkscreen")',
        '    (38 "B.Mask" user)',
        '    (39 "F.Mask" user)',
        '    (44 "Edge.Cuts" user)',
        '    (46 "B.CrtYd" user "B.Courtyard")',
        '    (47 "F.CrtYd" user "F.Courtyard")',
        '    (48 "B.Fab" user)',
        '    (49 "F.Fab" user)',
        "  )",
        "  (setup",
        "    (pad_to_mask_clearance 0)",
        "    (allow_soldermask_bridges_in_footprints no)",
        "  )",
        '  (net 0 "")',
    ]
    for name, code in net_codes.items():
        lines.append(f"  (net {code} {_quote(name)})")

    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        lines.extend(
            _footprint_lines(board, placement, footprint, pad_nets, net_codes)
        )

    for index, track in enumerate(board.tracks):
        lines.extend(
            [
                "  (segment",
                f"    (start {_point(track.start)})",
                f"    (end {_point(track.end)})",
                f"    (width {_mm(track.width_nm)})",
                f"    (layer {_quote(track.layer.value)})",
                f"    (net {net_codes[track.net]})",
                f'    (uuid "{_stable_uuid(board.name, "segment", str(index))}")',
                "  )",
            ]
        )

    for index, via in enumerate(board.vias):
        technology = next((item for item in board.stackup.via_technologies if item.id == via.technology), None)
        via_kind = ""
        if technology is not None and technology.kind in {ViaKind.BLIND, ViaKind.BURIED}:
            via_kind = " blind"
        elif technology is not None and technology.kind is ViaKind.MICROVIA:
            via_kind = " micro"
        lines.extend(
            [
                f"  (via{via_kind}",
                f"    (at {_point(via.position)})",
                f"    (size {_mm(via.size_nm)})",
                f"    (drill {_mm(via.drill_nm)})",
                f"    (layers {_quote(via.from_layer.value)} {_quote(via.to_layer.value)})",
                f"    (net {net_codes[via.net]})",
                f'    (uuid "{_stable_uuid(board.name, "via", str(index))}")',
                "  )",
            ]
        )

    for zone in sorted(board.zones, key=lambda item: (-item.priority, item.id)):
        for layer in sorted(zone.layers, key=lambda item: item.value):
            lines.extend(_zone_lines(board, zone, layer, net_codes))

    for keepout in sorted(resolved_copper_keepouts(board), key=lambda item: item.id):
        for layer in sorted(keepout.layers, key=lambda item: item.value):
            lines.extend(_copper_keepout_lines(board, keepout, layer))

    vertices = board.outline.vertices
    for index, start in enumerate(vertices):
        end = vertices[(index + 1) % len(vertices)]
        lines.extend(
            [
                "  (gr_line",
                f"    (start {_point(start)})",
                f"    (end {_point(end)})",
                "    (stroke (width 0.05) (type default))",
                '    (layer "Edge.Cuts")',
                f'    (uuid "{_stable_uuid(board.name, "outline", str(index))}")',
                "  )",
            ]
        )
    lines.append(")")
    return "\n".join(lines) + "\n"


def _copper_layer_lines(board: PhysicalBoard) -> list[str]:
    result: list[str] = []
    for index, layer in enumerate(board.stackup.copper_layers):
        number = 0 if layer is CopperLayer.FRONT else 31 if layer is CopperLayer.BACK else int(layer.value[2:-3])
        result.append(f'    ({number} "{layer.value}" signal)')
    return result


def _zone_lines(
    board: PhysicalBoard,
    zone: CopperZone,
    layer: CopperLayer,
    net_codes: dict[str, int],
) -> list[str]:
    if zone.outline.holes:
        raise ValueError(
            f"KiCad zone backend does not yet support holes in zone {zone.id!r}; "
            "use an explicit copper keepout"
        )
    if zone.thermal.spoke_count != 4:
        raise ValueError("KiCad zone backend supports four-spoke thermals")
    connection = {
        ZoneConnection.THERMAL: "",
        # KiCad's on-disk spelling is deliberately different from the UI:
        # `yes` means a solid pad connection and an omitted token means thermal.
        ZoneConnection.SOLID: " yes",
        ZoneConnection.NONE: " no",
        ZoneConnection.THT_THERMAL: " thru_hole_only",
    }[zone.pad_connection]
    clearance = zone.clearance_nm or board.rules.minimum_clearance_nm
    island_mode = {
        IslandPolicy.REMOVE_ALL: 0,
        IslandPolicy.KEEP_ALL: 1,
        IslandPolicy.REMOVE_BELOW_AREA: 2,
    }[zone.island_policy]
    fill = (
        f"    (fill yes (thermal_gap {_mm(zone.thermal.gap_nm)}) "
        f"(thermal_bridge_width {_mm(zone.thermal.spoke_width_nm)}) "
        f"(island_removal_mode {island_mode})"
    )
    if zone.island_policy is IslandPolicy.REMOVE_BELOW_AREA:
        assert zone.minimum_island_area_nm2 is not None
        fill += f" (island_area_min {_area_mm2(zone.minimum_island_area_nm2)})"
    fill += ")"
    result = [
        "  (zone",
        f"    (net {net_codes[zone.net]})",
        f"    (net_name {_quote(zone.net)})",
        f"    (layer {_quote(layer.value)})",
        f'    (uuid "{_stable_uuid(board.name, "zone", zone.id, layer.value)}")',
        f"    (name {_quote(zone.id)})",
        "    (hatch edge 0.5)",
    ]
    if zone.priority:
        result.append(f"    (priority {zone.priority})")
    result.extend(
        [
            f"    (connect_pads{connection} (clearance {_mm(clearance)}))",
            f"    (min_thickness {_mm(zone.minimum_width_nm)})",
            fill,
            "    (polygon",
            "      (pts",
            *(f"        (xy {_point(point)})" for point in zone.outline.outer.vertices),
            "      )",
            "    )",
            "  )",
        ]
    )
    return result


def _copper_keepout_lines(
    board: PhysicalBoard,
    keepout: CopperKeepout,
    layer: CopperLayer,
) -> list[str]:
    if keepout.outline.holes:
        raise ValueError(
            f"KiCad zone backend does not support holes in keepout {keepout.id!r}"
        )
    setting = lambda blocked: "not_allowed" if blocked else "allowed"
    return [
        "  (zone",
        "    (net 0)",
        '    (net_name "")',
        f"    (layer {_quote(layer.value)})",
        f'    (uuid "{_stable_uuid(board.name, "copper-keepout", keepout.id, layer.value)}")',
        f"    (name {_quote(keepout.id)})",
        "    (hatch edge 0.5)",
        "    (keepout",
        f"      (tracks {setting(keepout.block_tracks)})",
        f"      (vias {setting(keepout.block_vias)})",
        f"      (pads {setting(keepout.block_pads)})",
        f"      (copperpour {setting(keepout.block_zones)})",
        f"      (footprints {setting(keepout.block_footprints)})",
        "    )",
        "    (polygon",
        "      (pts",
        *(f"        (xy {_point(point)})" for point in keepout.outline.outer.vertices),
        "      )",
        "    )",
        "  )",
    ]


def _footprint_lines(
    board: PhysicalBoard,
    placement: Placement,
    footprint: PhysicalFootprint,
    pad_nets: dict[PadReference, str],
    net_codes: dict[str, int],
) -> list[str]:
    side_layer = "F.Cu" if placement.side is BoardSide.FRONT else "B.Cu"
    silk_layer = "F.SilkS" if placement.side is BoardSide.FRONT else "B.SilkS"
    footprint_uuid = _stable_uuid(board.name, "footprint", placement.reference)
    lines = [
        f"  (footprint {_quote(footprint.name)}",
        f"    (layer {_quote(side_layer)})",
        f'    (uuid "{footprint_uuid}")',
        f"    (at {_point(placement.position)} {_decimal(placement.rotation_degrees)})",
    ]
    lines.extend(
        _property_lines(
            board.name,
            placement.reference,
            "Reference",
            _kicad_reference(placement.reference),
            0,
            -(footprint.body_size.height_nm // 2 + 1500000),
            silk_layer,
        )
    )
    lines.extend(
        _property_lines(
            board.name,
            placement.reference,
            "Value",
            placement.value,
            0,
            footprint.body_size.height_nm // 2 + 1500000,
            "F.Fab" if placement.side is BoardSide.FRONT else "B.Fab",
            hidden=True,
        )
    )
    if placement.source_path:
        lines.extend(
            _property_lines(
                board.name,
                placement.reference,
                "CopperScriptPath",
                placement.source_path,
                0,
                0,
                "F.Fab" if placement.side is BoardSide.FRONT else "B.Fab",
                hidden=True,
            )
        )
    attribute = (
        "smd"
        if all(pad.kind in {PadKind.SMD, PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE} for pad in footprint.pads)
        else "through_hole"
    )
    exclusions = (
        (" exclude_from_bom" if footprint.exclude_from_bom else "")
        + (" exclude_from_pos_files" if footprint.exclude_from_pos_files else "")
    )
    lines.append(f"    (attr {attribute}{exclusions})")
    if footprint.clearance_nm is not None:
        lines.append(f"    (clearance {_mm(footprint.clearance_nm)})")
    if footprint.graphics:
        for index, graphic in enumerate(footprint.graphics):
            lines.extend(
                _graphic_lines(board.name, placement, graphic, index)
            )
    else:
        half_width = footprint.body_size.width_nm // 2
        half_height = footprint.body_size.height_nm // 2
        lines.extend(
            [
                "    (fp_rect",
                f"      (start {_relative_point(-half_width, -half_height)})",
                f"      (end {_relative_point(half_width, half_height)})",
                "      (stroke (width 0.15) (type default))",
                "      (fill none)",
                f"      (layer {_quote(silk_layer)})",
                f'      (uuid "{_stable_uuid(board.name, "body", placement.reference)}")',
                "    )",
            ]
        )
    for index, pad in enumerate(footprint.pads):
        net_name = pad_nets.get(PadReference(placement.reference, pad.number))
        lines.extend(
            _pad_lines(board.name, placement, pad, index, net_name, net_codes)
        )
    lines.append("  )")
    return lines


def _property_lines(
    board_name: str,
    reference: str,
    name: str,
    value: str,
    x_nm: int,
    y_nm: int,
    layer: str,
    *,
    hidden: bool = False,
) -> list[str]:
    lines = [
        f"    (property {_quote(name)} {_quote(value)}",
        f"      (at {_relative_point(x_nm, y_nm)} 0)",
        f"      (layer {_quote(layer)})",
    ]
    if hidden:
        lines.append("      (hide yes)")
    lines.extend(
        [
            f'      (uuid "{_stable_uuid(board_name, "property", reference, name)}")',
            "      (effects (font (size 1 1) (thickness 0.15)))",
            "    )",
        ]
    )
    return lines


def _pad_lines(
    board_name: str,
    placement: Placement,
    pad: FootprintPad,
    index: int,
    net_name: str | None,
    net_codes: dict[str, int],
) -> list[str]:
    kind = {
        PadKind.SMD: "connect" if pad.connector_contact else "smd",
        PadKind.APERTURE: "smd",
        PadKind.THROUGH_HOLE: "thru_hole",
        PadKind.NON_PLATED_THROUGH_HOLE: "np_thru_hole",
    }[pad.kind]
    shape = {
        PadShape.CIRCLE: "circle",
        PadShape.OVAL: "oval",
        PadShape.RECTANGLE: "rect",
        PadShape.ROUNDRECT: "roundrect",
    }[pad.shape]
    if pad.kind in {PadKind.SMD, PadKind.APERTURE}:
        side = "F" if placement.side is BoardSide.FRONT else "B"
        pad_layers = [] if pad.kind is PadKind.APERTURE else [f"{side}.Cu"]
        if pad.has_solder_paste:
            pad_layers.append(f"{side}.Paste")
        if pad.has_solder_mask:
            pad_layers.append(f"{side}.Mask")
    else:
        pad_layers = ["*.Cu"]
        if pad.has_solder_mask:
            pad_layers.append("*.Mask")
    layers = " ".join(_quote(layer) for layer in pad_layers)
    lines = [
        f"    (pad {_quote(pad.number)} {kind} {shape}",
        # KiCad stores the pad angle in board coordinates even though its
        # position is local to the footprint. Without the placement angle,
        # rotated rectangular pads overlap their neighbours after export.
        f"      (at {_point(pad.position)} {_decimal((pad.rotation_degrees + placement.rotation_degrees) % 360)})",
        f"      (size {_mm(pad.size.width_nm)} {_mm(pad.size.height_nm)})",
    ]
    if pad.drill is not None:
        if pad.drill.width_nm == pad.drill.height_nm:
            lines.append(f"      (drill {_mm(pad.drill.width_nm)})")
        else:
            lines.append(
                f"      (drill oval {_mm(pad.drill.width_nm)} "
                f"{_mm(pad.drill.height_nm)})"
            )
    lines.append(f"      (layers {layers})")
    if pad.shape is PadShape.ROUNDRECT:
        lines.append(
            f"      (roundrect_rratio {_ratio(pad.roundrect_ratio_ppm)})"
        )
    if pad.heatsink:
        lines.append("      (property pad_prop_heatsink)")
    if pad.zone_connection is not None:
        lines.append(
            "      (zone_connect "
            + {
                ZoneConnection.NONE: "1",
                ZoneConnection.THERMAL: "2",
                ZoneConnection.SOLID: "3",
                ZoneConnection.THT_THERMAL: "4",
            }[pad.zone_connection]
            + ")"
        )
    if pad.remove_unused_layers:
        lines.append("      (remove_unused_layers yes)")
    if net_name is not None and pad.kind not in {
        PadKind.NON_PLATED_THROUGH_HOLE,
        PadKind.APERTURE,
    }:
        lines.append(f"      (net {net_codes[net_name]} {_quote(net_name)})")
    lines.extend(
        [
            '      (pintype "passive")',
            f'      (uuid "{_stable_uuid(board_name, "pad", placement.reference, str(index), pad.number)}")',
            "    )",
        ]
    )
    return lines


def _graphic_lines(
    board_name: str,
    placement: Placement,
    graphic: FootprintGraphic,
    index: int,
) -> list[str]:
    layer = _footprint_layer(graphic.layer, placement.side)
    item_uuid = _stable_uuid(
        board_name,
        "footprint-graphic",
        placement.reference,
        str(index),
        graphic.__class__.__name__,
    )
    common = [
        f"      (stroke (width {_mm(graphic.width_nm)}) (type default))",
    ]
    if isinstance(graphic, FootprintLine):
        return [
            "    (fp_line",
            f"      (start {_point(graphic.start)})",
            f"      (end {_point(graphic.end)})",
            *common,
            f"      (layer {_quote(layer)})",
            f'      (uuid "{item_uuid}")',
            "    )",
        ]
    if isinstance(graphic, FootprintRectangle):
        return [
            "    (fp_rect",
            f"      (start {_point(graphic.start)})",
            f"      (end {_point(graphic.end)})",
            *common,
            f"      (fill {'solid' if graphic.filled else 'none'})",
            f"      (layer {_quote(layer)})",
            f'      (uuid "{item_uuid}")',
            "    )",
        ]
    if isinstance(graphic, FootprintCircle):
        return [
            "    (fp_circle",
            f"      (center {_point(graphic.center)})",
            f"      (end {_point(graphic.end)})",
            *common,
            f"      (fill {'solid' if graphic.filled else 'none'})",
            f"      (layer {_quote(layer)})",
            f'      (uuid "{item_uuid}")',
            "    )",
        ]
    if isinstance(graphic, FootprintArc):
        return [
            "    (fp_arc",
            f"      (start {_point(graphic.start)})",
            f"      (mid {_point(graphic.midpoint)})",
            f"      (end {_point(graphic.end)})",
            *common,
            f"      (fill none)",
            f"      (layer {_quote(layer)})",
            f'      (uuid "{item_uuid}")',
            "    )",
        ]
    if isinstance(graphic, FootprintPolygon):
        points = " ".join(f"(xy {_point(point)})" for point in graphic.points)
        return [
            "    (fp_poly",
            f"      (pts {points})",
            *common,
            f"      (fill {'solid' if graphic.filled else 'none'})",
            f"      (layer {_quote(layer)})",
            f'      (uuid "{item_uuid}")',
            "    )",
        ]
    raise TypeError(f"unsupported footprint graphic {type(graphic).__name__}")


def _footprint_layer(layer: FootprintLayer, side: BoardSide) -> str:
    prefix = "F" if side is BoardSide.FRONT else "B"
    return {
        FootprintLayer.SILKSCREEN: f"{prefix}.SilkS",
        FootprintLayer.FABRICATION: f"{prefix}.Fab",
        FootprintLayer.COURTYARD: f"{prefix}.CrtYd",
        FootprintLayer.ADHESIVE: f"{prefix}.Adhes",
        FootprintLayer.DOCUMENTATION: "Dwgs.User",
        FootprintLayer.SOLDER_MASK: f"{prefix}.Mask",
        FootprintLayer.SOLDER_PASTE: f"{prefix}.Paste",
    }[layer]


def _point(point: Point) -> str:
    return f"{_mm(point.x_nm)} {_mm(point.y_nm)}"


def _relative_point(x_nm: int, y_nm: int) -> str:
    return f"{_mm(x_nm)} {_mm(y_nm)}"


def _mm(value_nm: int) -> str:
    sign = "-" if value_nm < 0 else ""
    whole, fractional = divmod(abs(value_nm), 1_000_000)
    if not fractional:
        return f"{sign}{whole}"
    return f"{sign}{whole}.{fractional:06d}".rstrip("0")


def _area_mm2(value_nm2: int) -> str:
    whole, fractional = divmod(value_nm2, 1_000_000_000_000)
    if not fractional:
        return str(whole)
    return f"{whole}.{fractional:012d}".rstrip("0")


def _ratio(value_ppm: int) -> str:
    whole, fractional = divmod(value_ppm, 1_000_000)
    if not fractional:
        return str(whole)
    return f"{whole}.{fractional:06d}".rstrip("0")


def _decimal(value) -> str:
    text = format(value, "f")
    if "." in text:
        text = text.rstrip("0").rstrip(".")
    return text or "0"


def _quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _safe_name(value: str) -> str:
    safe = re.sub(r"[^A-Za-z0-9_]", "_", value).strip("_")
    return safe or "CopperScript"


def _kicad_reference(value: str) -> str:
    safe = re.sub(r"[^A-Za-z0-9_]", "_", value)
    return safe if safe and safe[0].isalpha() else f"U_{safe}"


def _stable_uuid(*parts: str) -> str:
    digest = bytearray(
        sha256(_UUID_SEED + "\0".join(parts).encode("utf-8")).digest()[:16]
    )
    digest[6] = (digest[6] & 0x0F) | 0x40
    digest[8] = (digest[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(digest)))

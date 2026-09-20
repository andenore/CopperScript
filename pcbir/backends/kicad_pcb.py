"""Deterministic KiCad 8 PCB backend for the CopperScript physical IR."""

from __future__ import annotations

from hashlib import sha256
import json
import re
import uuid

from ..physical import (
    BoardSide,
    CopperLayer,
    FootprintPad,
    PadKind,
    PadReference,
    PadShape,
    PhysicalBoard,
    PhysicalFootprint,
    Placement,
    Point,
)
from .base import Artifact, ArtifactManifest


KICAD_PCB_FORMAT = "20240108"
KICAD_PCB_TARGET_VERSION = "8.0"
_UUID_SEED = b"CopperScript KiCad PCB backend v0.1\0"


class KiCadPcbBackend:
    """Generate a self-contained KiCad 8 ``.kicad_pcb`` artifact."""

    name = "kicad-pcb"
    target_version = KICAD_PCB_TARGET_VERSION

    def generate(self, board: PhysicalBoard) -> ArtifactManifest:
        if set(board.stackup.copper_layers) != {
            CopperLayer.FRONT,
            CopperLayer.BACK,
        }:
            raise ValueError(
                "KiCad PCB backend currently supports a two-layer F.Cu/B.Cu stackup"
            )
        content = _render(board)
        warnings: list[str] = []
        if board.metadata.get("prototype_footprints") == "true":
            warnings.append(
                "The board uses generated proxy footprints and is for inspection only; "
                "resolve verified package footprints before fabrication."
            )
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
        '    (0 "F.Cu" signal)',
        '    (31 "B.Cu" signal)',
        '    (36 "B.SilkS" user "b.silkscreen")',
        '    (37 "F.SilkS" user "f.silkscreen")',
        '    (44 "Edge.Cuts" user)',
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
        lines.extend(
            [
                "  (via",
                f"    (at {_point(via.position)})",
                f"    (size {_mm(via.size_nm)})",
                f"    (drill {_mm(via.drill_nm)})",
                f"    (layers {_quote(via.from_layer.value)} {_quote(via.to_layer.value)})",
                f"    (net {net_codes[via.net]})",
                f'    (uuid "{_stable_uuid(board.name, "via", str(index))}")',
                "  )",
            ]
        )

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
        f"    (at {_point(placement.position)} {placement.rotation_degrees})",
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
        if all(pad.kind is PadKind.SMD for pad in footprint.pads)
        else "through_hole"
    )
    lines.append(f"    (attr {attribute})")
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
    for pad in footprint.pads:
        net_name = pad_nets.get(PadReference(placement.reference, pad.number))
        lines.extend(
            _pad_lines(board.name, placement, pad, net_name, net_codes)
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
    net_name: str | None,
    net_codes: dict[str, int],
) -> list[str]:
    kind = {
        PadKind.SMD: "smd",
        PadKind.THROUGH_HOLE: "thru_hole",
        PadKind.NON_PLATED_THROUGH_HOLE: "np_thru_hole",
    }[pad.kind]
    shape = {
        PadShape.CIRCLE: "circle",
        PadShape.OVAL: "oval",
        PadShape.RECTANGLE: "rect",
        PadShape.ROUNDRECT: "roundrect",
    }[pad.shape]
    if pad.kind is PadKind.SMD:
        side = "F" if placement.side is BoardSide.FRONT else "B"
        layers = f'"{side}.Cu" "{side}.Paste" "{side}.Mask"'
    else:
        layers = '"*.Cu" "*.Mask"'
    lines = [
        f"    (pad {_quote(pad.number)} {kind} {shape}",
        f"      (at {_point(pad.position)})",
        f"      (size {_mm(pad.size.width_nm)} {_mm(pad.size.height_nm)})",
    ]
    if pad.drill_nm is not None:
        lines.append(f"      (drill {_mm(pad.drill_nm)})")
    lines.append(f"      (layers {layers})")
    if pad.shape is PadShape.ROUNDRECT:
        lines.append("      (roundrect_rratio 0.25)")
    if net_name is not None and pad.kind is not PadKind.NON_PLATED_THROUGH_HOLE:
        lines.append(f"      (net {net_codes[net_name]} {_quote(net_name)})")
    lines.extend(
        [
            '      (pintype "passive")',
            f'      (uuid "{_stable_uuid(board_name, "pad", placement.reference, pad.number)}")',
            "    )",
        ]
    )
    return lines


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

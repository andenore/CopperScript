"""Fabrication, stencil, and assembly gates over normalized physical IR."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from math import hypot

from .drc import placed_pad_shape
from .geometry import RoundedConvexShape, shape_distance_squared, shapes_clear
from .physical import FootprintArc, FootprintLayer, FootprintLine, PadKind, PhysicalBoard, Point
from .placement import transformed_local_point


class ProcessGateStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"
    INCOMPLETE = "incomplete"


@dataclass(frozen=True, slots=True)
class ProcessCapability:
    value: int
    source: str
    revision: str

    def __post_init__(self) -> None:
        if self.value <= 0 or not self.source or not self.revision:
            raise ValueError("process capabilities require positive value and provenance")


@dataclass(frozen=True, slots=True)
class FabricationAssemblyProfile:
    id: str
    minimum_drill_nm: ProcessCapability
    minimum_mask_web_nm: ProcessCapability
    stencil_thickness_nm: ProcessCapability
    minimum_paste_area_ratio_ppm: ProcessCapability
    maximum_component_height_nm: ProcessCapability | None = None
    require_courtyards: bool = True
    solder_mask_expansion_nm: ProcessCapability | None = None
    minimum_silkscreen_clearance_nm: ProcessCapability | None = None
    minimum_slot_width_nm: ProcessCapability | None = None
    maximum_copper_imbalance_ppm: ProcessCapability | None = None
    require_orientation_marks: bool = True
    allow_edge_plating: bool = False
    minimum_non_plated_drill_nm: ProcessCapability | None = None


@dataclass(frozen=True, slots=True)
class ProcessFinding:
    code: str
    gate: str
    message: str
    objects: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class ProcessDrcReport:
    fabrication: ProcessGateStatus
    stencil: ProcessGateStatus
    assembly: ProcessGateStatus
    findings: tuple[ProcessFinding, ...]

    @property
    def passed(self) -> bool:
        return all(status is ProcessGateStatus.PASS for status in (
            self.fabrication, self.stencil, self.assembly
        ))


def run_process_drc(board: PhysicalBoard, profile: FabricationAssemblyProfile) -> ProcessDrcReport:
    findings: list[ProcessFinding] = []
    fabrication_failed = False
    stencil_failed = False
    assembly_failed = False
    fabrication_incomplete = False
    for hole in board.mechanical_holes:
        limit = profile.minimum_non_plated_drill_nm
        if limit is None:
            fabrication_incomplete = True
            findings.append(ProcessFinding(
                "FAB-NPTH-LIMIT-MISSING", "fabrication",
                "board-owned NPTH requires a separately qualified non-plated drill limit",
                (hole.id,),
            ))
        elif hole.diameter_nm < limit.value:
            fabrication_failed = True
            findings.append(ProcessFinding(
                "FAB-NPTH-MIN", "fabrication",
                "board-owned NPTH is below the qualified non-plated process limit", (hole.id,),
            ))
    placed_pads: list[tuple[str, object, object, object, RoundedConvexShape]] = []
    silk_shapes: list[tuple[str, RoundedConvexShape]] = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        if profile.require_courtyards and not footprint.courtyard:
            assembly_failed = True
            findings.append(ProcessFinding("ASM-COURTYARD-MISSING", "assembly",
                                           f"{placement.reference} has no audited courtyard",
                                           (placement.reference,)))
        if (profile.maximum_component_height_nm is not None and footprint.height_nm is not None
                and footprint.height_nm > profile.maximum_component_height_nm.value):
            assembly_failed = True
            findings.append(ProcessFinding("ASM-HEIGHT", "assembly",
                                           f"{placement.reference} exceeds the assembly height limit",
                                           (placement.reference,)))
        for pad_index, pad in enumerate(footprint.pads):
            identity = f"{placement.reference}.{pad.number or f'aperture:{pad_index}'}"
            position = transformed_local_point(placement, pad.position)
            shape = placed_pad_shape(position, pad, placement)
            placed_pads.append((identity, position, pad, placement, shape))
            if pad.drill is not None and min(pad.drill.width_nm, pad.drill.height_nm) < profile.minimum_drill_nm.value:
                fabrication_failed = True
                findings.append(ProcessFinding("FAB-DRILL-MIN", "fabrication",
                                               f"{identity} drill is below the qualified process limit", (identity,)))
            if pad.kind in {PadKind.SMD, PadKind.APERTURE} and pad.has_solder_paste:
                width, height = pad.size.width_nm, pad.size.height_nm
                area = width * height
                wall = 2 * (width + height) * profile.stencil_thickness_nm.value
                ratio_ppm = area * 1_000_000 // wall
                if ratio_ppm < profile.minimum_paste_area_ratio_ppm.value:
                    stencil_failed = True
                    findings.append(ProcessFinding("STENCIL-AREA-RATIO", "stencil",
                                                   f"{identity} paste aperture area ratio is too low", (identity,)))
            if (pad.drill is not None and pad.drill.width_nm != pad.drill.height_nm
                    and profile.minimum_slot_width_nm is not None
                    and min(pad.drill.width_nm, pad.drill.height_nm) < profile.minimum_slot_width_nm.value):
                fabrication_failed = True
                findings.append(ProcessFinding("FAB-SLOT-MIN", "fabrication",
                                               f"{identity} slot is below the qualified process limit", (identity,)))
        if profile.require_orientation_marks and footprint.metadata.get("polarized") == "true":
            if not any(getattr(graphic, "layer", None) is FootprintLayer.SILKSCREEN
                       for graphic in footprint.graphics):
                assembly_failed = True
                findings.append(ProcessFinding("ASM-ORIENTATION-MARK", "assembly",
                                               f"{placement.reference} has no silkscreen orientation mark",
                                               (placement.reference,)))
        if footprint.metadata.get("castellated") == "true" and not profile.allow_edge_plating:
            fabrication_failed = True
            findings.append(ProcessFinding("FAB-EDGE-PLATING", "fabrication",
                                           f"{placement.reference} requires an edge-plating-capable profile",
                                           (placement.reference,)))
        for graphic_index, graphic in enumerate(footprint.graphics):
            if getattr(graphic, "layer", None) is not FootprintLayer.SILKSCREEN:
                continue
            identity = f"silk:{placement.reference}:{graphic_index}"
            if isinstance(graphic, FootprintLine):
                silk_shapes.append((identity, RoundedConvexShape(
                    (_placed_point(placement, graphic.start), _placed_point(placement, graphic.end)),
                    graphic.width_nm // 2)))
            elif isinstance(graphic, FootprintArc):
                fabrication_incomplete = True
                findings.append(ProcessFinding("FAB-SILK-ARC-UNSUPPORTED", "fabrication",
                                               f"{identity} requires normalized arc artwork", (identity,)))
    expansion = profile.solder_mask_expansion_nm.value if profile.solder_mask_expansion_nm else 0
    for index, (left_name, _, left_pad, _, left_shape) in enumerate(placed_pads):
        if not left_pad.has_solder_mask:
            continue
        left_mask = RoundedConvexShape(left_shape.spine, left_shape.radius_nm + expansion)
        for right_name, _, right_pad, _, right_shape in placed_pads[index + 1:]:
            if not right_pad.has_solder_mask:
                continue
            right_mask = RoundedConvexShape(right_shape.spine, right_shape.radius_nm + expansion)
            if not shapes_clear(left_mask, right_mask, profile.minimum_mask_web_nm.value):
                fabrication_failed = True
                findings.append(ProcessFinding("FAB-MASK-WEB", "fabrication",
                                               f"mask web between {left_name} and {right_name} is below profile",
                                               tuple(sorted((left_name, right_name)))))
    if profile.minimum_silkscreen_clearance_nm is not None:
        for silk_name, silk in silk_shapes:
            for pad_name, _, pad, _, pad_shape in placed_pads:
                if not pad.has_solder_mask:
                    continue
                mask = RoundedConvexShape(pad_shape.spine, pad_shape.radius_nm + expansion)
                if not shapes_clear(silk, mask, profile.minimum_silkscreen_clearance_nm.value):
                    fabrication_failed = True
                    findings.append(ProcessFinding("FAB-SILK-MASK", "fabrication",
                                                   f"{silk_name} is too close to {pad_name} mask opening",
                                                   (silk_name, pad_name)))
    if profile.maximum_copper_imbalance_ppm is not None:
        top, bottom = _copper_area(board)
        total = top + bottom
        imbalance = 0 if total == 0 else abs(top - bottom) * 1_000_000 // total
        if imbalance > profile.maximum_copper_imbalance_ppm.value:
            fabrication_failed = True
            findings.append(ProcessFinding("FAB-COPPER-BALANCE", "fabrication",
                                           f"outer-layer copper imbalance {imbalance} ppm exceeds profile"))
    return ProcessDrcReport(
        ProcessGateStatus.FAIL if fabrication_failed else ProcessGateStatus.INCOMPLETE if fabrication_incomplete else ProcessGateStatus.PASS,
        ProcessGateStatus.FAIL if stencil_failed else ProcessGateStatus.PASS,
        ProcessGateStatus.FAIL if assembly_failed else ProcessGateStatus.PASS,
        tuple(sorted(findings, key=lambda item: (item.gate, item.code, item.objects))),
    )


def _placed_point(placement: object, point: Point) -> Point:
    from math import cos, radians, sin
    angle = radians(float(getattr(placement, "rotation_degrees")))
    x = -point.x_nm if getattr(placement, "side").value == "back" else point.x_nm
    return Point(getattr(placement, "position").x_nm + round(x * cos(angle) - point.y_nm * sin(angle)),
                 getattr(placement, "position").y_nm + round(x * sin(angle) + point.y_nm * cos(angle)))


def _copper_area(board: PhysicalBoard) -> tuple[int, int]:
    areas = {board.stackup.copper_layers[0]: 0, board.stackup.copper_layers[-1]: 0}
    for track in board.tracks:
        if track.layer in areas:
            areas[track.layer] += round(hypot(track.end.x_nm - track.start.x_nm,
                                             track.end.y_nm - track.start.y_nm)) * track.width_nm
    for fill in board.zone_fills:
        if fill.layer in areas:
            for polygon in fill.polygons:
                points = polygon.outer.vertices
                outer = abs(sum(a.x_nm * b.y_nm - b.x_nm * a.y_nm
                                for a, b in zip(points, (*points[1:], points[0])))) // 2
                holes = 0
                for hole in polygon.holes:
                    hp = hole.vertices
                    holes += abs(sum(a.x_nm * b.y_nm - b.x_nm * a.y_nm
                                     for a, b in zip(hp, (*hp[1:], hp[0])))) // 2
                areas[fill.layer] += outer - holes
    return areas[board.stackup.copper_layers[0]], areas[board.stackup.copper_layers[-1]]

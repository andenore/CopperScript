"""Fabrication, stencil, and assembly gates over normalized physical IR."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from math import isqrt

from .physical import PadKind, PhysicalBoard
from .placement import transformed_pad_position


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
    placed_pads: list[tuple[str, object, object]] = []
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
        for pad in footprint.pads:
            identity = f"{placement.reference}.{pad.number}"
            position = transformed_pad_position(board, placement, pad.number)
            placed_pads.append((identity, position, pad))
            if pad.drill is not None and min(pad.drill.width_nm, pad.drill.height_nm) < profile.minimum_drill_nm.value:
                fabrication_failed = True
                findings.append(ProcessFinding("FAB-DRILL-MIN", "fabrication",
                                               f"{identity} drill is below the qualified process limit", (identity,)))
            if pad.kind is PadKind.SMD and pad.has_solder_paste:
                width, height = pad.size.width_nm, pad.size.height_nm
                area = width * height
                wall = 2 * (width + height) * profile.stencil_thickness_nm.value
                ratio_ppm = area * 1_000_000 // wall
                if ratio_ppm < profile.minimum_paste_area_ratio_ppm.value:
                    stencil_failed = True
                    findings.append(ProcessFinding("STENCIL-AREA-RATIO", "stencil",
                                                   f"{identity} paste aperture area ratio is too low", (identity,)))
    for index, (left_name, left_pos, left_pad) in enumerate(placed_pads):
        if not left_pad.has_solder_mask:
            continue
        for right_name, right_pos, right_pad in placed_pads[index + 1:]:
            if not right_pad.has_solder_mask:
                continue
            dx, dy = left_pos.x_nm - right_pos.x_nm, left_pos.y_nm - right_pos.y_nm
            center = isqrt(dx * dx + dy * dy)
            web = center - max(left_pad.size.width_nm, left_pad.size.height_nm) // 2 - max(right_pad.size.width_nm, right_pad.size.height_nm) // 2
            if web < profile.minimum_mask_web_nm.value:
                fabrication_failed = True
                findings.append(ProcessFinding("FAB-MASK-WEB", "fabrication",
                                               f"mask web between {left_name} and {right_name} is below profile",
                                               tuple(sorted((left_name, right_name)))))
    return ProcessDrcReport(
        ProcessGateStatus.FAIL if fabrication_failed else ProcessGateStatus.PASS,
        ProcessGateStatus.FAIL if stencil_failed else ProcessGateStatus.PASS,
        ProcessGateStatus.FAIL if assembly_failed else ProcessGateStatus.PASS,
        tuple(sorted(findings, key=lambda item: (item.gate, item.code, item.objects))),
    )

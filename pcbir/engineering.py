"""Evidence-graded conservative engineering screening calculations."""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, getcontext
from enum import Enum

getcontext().prec = 28


class EvidenceGrade(str, Enum):
    SCREENING = "screening"
    REDUCED_ORDER = "reduced_order"
    EXTERNAL_SOLVER = "external_solver"
    MEASURED = "measured"


class AnalysisStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"
    INDETERMINATE = "indeterminate"


@dataclass(frozen=True, slots=True)
class EngineeringResult:
    analysis: str
    status: AnalysisStatus
    evidence_grade: EvidenceGrade
    value: Decimal | None
    unit: str
    claim_scope: str
    validity: tuple[str, ...]


def dc_trace_resistance(length_nm: int, width_nm: int, copper_thickness_nm: int,
                        *, resistivity_ohm_m: Decimal = Decimal("1.724e-8"),
                        maximum_ohms: Decimal | None = None) -> EngineeringResult:
    if min(length_nm, width_nm, copper_thickness_nm) <= 0:
        raise ValueError("trace dimensions must be positive")
    length_m = Decimal(length_nm) / Decimal(1_000_000_000)
    area_m2 = Decimal(width_nm) * Decimal(copper_thickness_nm) / Decimal(10**18)
    resistance = resistivity_ohm_m * length_m / area_m2
    status = AnalysisStatus.INDETERMINATE if maximum_ohms is None else (
        AnalysisStatus.PASS if resistance <= maximum_ohms else AnalysisStatus.FAIL
    )
    return EngineeringResult("dc_trace_resistance", status, EvidenceGrade.SCREENING,
                             resistance, "ohm", "uniform rectangular copper at reference temperature",
                             ("ignores vias, spreading resistance, plating variation, and self-heating",))


def creepage_screen(measured_nm: int, required_nm: int, *, profile_source: str | None) -> EngineeringResult:
    if measured_nm < 0 or required_nm <= 0:
        raise ValueError("creepage distances are invalid")
    if not profile_source:
        return EngineeringResult("creepage", AnalysisStatus.INDETERMINATE,
                                 EvidenceGrade.SCREENING, Decimal(measured_nm), "nm",
                                 "geometric screening only", ("no sourced safety profile",))
    return EngineeringResult("creepage", AnalysisStatus.PASS if measured_nm >= required_nm else AnalysisStatus.FAIL,
                             EvidenceGrade.SCREENING, Decimal(measured_nm), "nm",
                             f"geometric comparison against {profile_source}",
                             ("pollution degree, material group, coating, altitude, and transients must match profile",))

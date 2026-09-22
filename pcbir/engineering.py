"""Evidence-graded conservative engineering screening calculations."""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, getcontext
from enum import Enum
from hashlib import sha256
from math import log, pi, sqrt

from .geometry import point_in_polygon
from .physical import PhysicalBoard

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
    evidence_digest: str | None = None


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


def microstrip_impedance(width_nm: int, copper_thickness_nm: int,
                         dielectric_height_nm: int, relative_permittivity: Decimal,
                         *, target_ohms: Decimal | None = None,
                         tolerance_ohms: Decimal = Decimal("0")) -> EngineeringResult:
    """Hammerstad-style microstrip screening, not field-solver signoff."""
    if min(width_nm, copper_thickness_nm, dielectric_height_nm) <= 0 or relative_permittivity <= 1:
        raise ValueError("microstrip geometry and permittivity are invalid")
    width = width_nm / dielectric_height_nm
    thickness = copper_thickness_nm / dielectric_height_nm
    effective_width = width + thickness / pi * (1 + log(4 * pi / max(thickness, 1e-12)))
    er = float(relative_permittivity)
    effective_er = (er + 1) / 2 + (er - 1) / (2 * sqrt(1 + 12 / effective_width))
    impedance = (60 / sqrt(effective_er) * log(8 / effective_width + effective_width / 4)
                 if effective_width <= 1 else
                 120 * pi / (sqrt(effective_er) * (effective_width + 1.393
                                                    + 0.667 * log(effective_width + 1.444))))
    value = Decimal(str(round(impedance, 9)))
    status = AnalysisStatus.INDETERMINATE if target_ohms is None else (
        AnalysisStatus.PASS if abs(value - target_ohms) <= tolerance_ohms else AnalysisStatus.FAIL)
    return EngineeringResult("microstrip_impedance", status, EvidenceGrade.SCREENING,
                             value, "ohm", "uniform isolated microstrip estimate",
                             ("requires field-solver qualification for fabrication release",
                              "ignores solder mask, roughness, weave and local copper"))


def propagation_delay(length_nm: int, effective_permittivity: Decimal,
                      *, maximum_seconds: Decimal | None = None) -> EngineeringResult:
    if length_nm <= 0 or effective_permittivity <= 1:
        raise ValueError("delay inputs are invalid")
    seconds = (Decimal(length_nm) / Decimal(1_000_000_000)
               * effective_permittivity.sqrt() / Decimal("299792458"))
    status = AnalysisStatus.INDETERMINATE if maximum_seconds is None else (
        AnalysisStatus.PASS if seconds <= maximum_seconds else AnalysisStatus.FAIL)
    return EngineeringResult("propagation_delay", status, EvidenceGrade.SCREENING,
                             seconds, "s", "quasi-TEM uniform transmission line",
                             ("effective permittivity must be established for the routed geometry",))


def return_path_continuity(board: PhysicalBoard, net: str,
                           reference_net: str) -> EngineeringResult:
    zones = {zone.id: zone.net for zone in board.zones}
    fills = [fill for fill in board.zone_fills if zones.get(fill.zone_id) == reference_net]
    tracks = [track for track in board.tracks if track.net == net]
    if not tracks or not fills:
        return EngineeringResult("return_path_continuity", AnalysisStatus.INDETERMINATE,
                                 EvidenceGrade.SCREENING, None, "fraction",
                                 "midpoint coverage by adjacent reference-plane fill",
                                 ("route or normalized reference-plane fill is unavailable",))
    layer_index = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    covered = 0
    for track in tracks:
        midpoint = type(track.start)((track.start.x_nm + track.end.x_nm) // 2,
                                     (track.start.y_nm + track.end.y_nm) // 2)
        index = layer_index[track.layer]
        adjacent = {value for value in (index - 1, index + 1)
                    if 0 <= value < len(board.stackup.copper_layers)}
        if any(layer_index[fill.layer] in adjacent
               and any(point_in_polygon(midpoint, polygon.outer.vertices)
                       and not any(point_in_polygon(midpoint, hole.vertices)
                                   for hole in polygon.holes)
                       for polygon in fill.polygons)
               for fill in fills):
            covered += 1
    fraction = Decimal(covered) / Decimal(len(tracks))
    return EngineeringResult("return_path_continuity",
                             AnalysisStatus.PASS if covered == len(tracks) else AnalysisStatus.FAIL,
                             EvidenceGrade.SCREENING, fraction, "fraction",
                             "track-midpoint coverage on an adjacent reference layer",
                             ("does not model plane resonances, stitching inductance or split-edge fringing",))


def dc_net_voltage_drop(board: PhysicalBoard, net: str, current_amperes: Decimal,
                        copper_thickness_nm: int, via_plating_thickness_nm: int,
                        *, maximum_volts: Decimal | None = None) -> EngineeringResult:
    if current_amperes < 0 or copper_thickness_nm <= 0 or via_plating_thickness_nm <= 0:
        raise ValueError("DC network inputs are invalid")
    resistance = Decimal(0)
    for track in board.tracks:
        if track.net != net:
            continue
        length = Decimal(str(sqrt((track.end.x_nm - track.start.x_nm) ** 2
                                  + (track.end.y_nm - track.start.y_nm) ** 2)))
        resistance += Decimal("1.724e-8") * (length / Decimal(1_000_000_000)) / (
            Decimal(track.width_nm) * Decimal(copper_thickness_nm) / Decimal(10**18))
    for via in board.vias:
        if via.net != net:
            continue
        depth = Decimal(board.stackup.thickness_nm) / Decimal(1_000_000_000)
        diameter = Decimal(via.drill_nm) / Decimal(1_000_000_000)
        plating = Decimal(via_plating_thickness_nm) / Decimal(1_000_000_000)
        resistance += Decimal("1.724e-8") * depth / (Decimal(str(pi)) * diameter * plating)
    drop = resistance * current_amperes
    status = AnalysisStatus.INDETERMINATE if maximum_volts is None else (
        AnalysisStatus.PASS if drop <= maximum_volts else AnalysisStatus.FAIL)
    return EngineeringResult("dc_net_voltage_drop", status, EvidenceGrade.SCREENING,
                             drop, "V", "series-path upper-bound screening",
                             ("topology is treated as series and ignores current sharing and temperature coefficient",))


def thermal_screen(power_watts: Decimal, thermal_resistance_k_per_w: Decimal,
                   ambient_celsius: Decimal, maximum_celsius: Decimal,
                   *, model_source: str) -> EngineeringResult:
    if power_watts < 0 or thermal_resistance_k_per_w <= 0 or not model_source:
        raise ValueError("thermal screening requires positive sourced model inputs")
    temperature = ambient_celsius + power_watts * thermal_resistance_k_per_w
    return EngineeringResult("thermal_temperature", AnalysisStatus.PASS if temperature <= maximum_celsius else AnalysisStatus.FAIL,
                             EvidenceGrade.REDUCED_ORDER, temperature, "degC",
                             f"single-resistance steady-state model from {model_source}",
                             ("ignores airflow distribution, spreading, radiation and transient behavior",))


def external_solver_result(analysis: str, value: Decimal | None, unit: str,
                           report_bytes: bytes, *, tool: str, version: str,
                           passed: bool, claim_scope: str) -> EngineeringResult:
    if not tool or not version or not report_bytes or not claim_scope:
        raise ValueError("external evidence requires tool identity, report, and scope")
    digest = sha256(report_bytes).hexdigest()
    return EngineeringResult(analysis, AnalysisStatus.PASS if passed else AnalysisStatus.FAIL,
                             EvidenceGrade.EXTERNAL_SOLVER, value, unit,
                             f"{claim_scope}; {tool} {version}", (), digest)

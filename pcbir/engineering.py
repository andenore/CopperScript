"""Evidence-graded conservative engineering screening calculations."""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, getcontext
from enum import Enum
from hashlib import sha256
from math import exp, log, pi, sqrt

from .geometry import point_in_polygon
from .physical import PhysicalBoard, TrackSegment

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


def _hammerstad_microstrip(width_nm: int, copper_thickness_nm: int, dielectric_height_nm: int,
                           relative_permittivity: Decimal) -> tuple[float, float]:
    """Return (single-ended ohms, effective permittivity) for a microstrip."""
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
    return impedance, effective_er


def _ohms(value: float) -> Decimal:
    return Decimal(str(round(value, 9)))


def _target_status(value: Decimal, target_ohms: Decimal | None,
                   tolerance_ohms: Decimal) -> AnalysisStatus:
    if target_ohms is None:
        return AnalysisStatus.INDETERMINATE
    return AnalysisStatus.PASS if abs(value - target_ohms) <= tolerance_ohms else AnalysisStatus.FAIL


def microstrip_impedance(width_nm: int, copper_thickness_nm: int,
                         dielectric_height_nm: int, relative_permittivity: Decimal,
                         *, target_ohms: Decimal | None = None,
                         tolerance_ohms: Decimal = Decimal("0")) -> EngineeringResult:
    """Hammerstad-style microstrip screening, not field-solver signoff."""
    impedance, _ = _hammerstad_microstrip(width_nm, copper_thickness_nm,
                                          dielectric_height_nm, relative_permittivity)
    value = _ohms(impedance)
    return EngineeringResult("microstrip_impedance", _target_status(value, target_ohms, tolerance_ohms),
                             EvidenceGrade.SCREENING,
                             value, "ohm", "uniform isolated microstrip estimate",
                             ("requires field-solver qualification for fabrication release",
                              "ignores solder mask, roughness, weave and local copper"))


def microstrip_effective_permittivity(width_nm: int, copper_thickness_nm: int,
                                      dielectric_height_nm: int,
                                      relative_permittivity: Decimal) -> Decimal:
    """Hammerstad quasi-static effective permittivity (screening, for delay estimates)."""
    _, effective_er = _hammerstad_microstrip(width_nm, copper_thickness_nm,
                                             dielectric_height_nm, relative_permittivity)
    return Decimal(str(round(effective_er, 9)))


def edge_coupled_microstrip_impedance(width_nm: int, gap_nm: int, copper_thickness_nm: int,
                                      dielectric_height_nm: int, relative_permittivity: Decimal,
                                      *, target_ohms: Decimal | None = None,
                                      tolerance_ohms: Decimal = Decimal("0")) -> EngineeringResult:
    """Edge-coupled microstrip differential impedance (screening).

    The single-ended Hammerstad estimate is coupled with the IPC-2141A factor
    ``Zdiff = 2 * Z0 * (1 - 0.48 * exp(-0.96 * s / h))``.
    """
    if gap_nm <= 0:
        raise ValueError("differential gap must be positive")
    single, _ = _hammerstad_microstrip(width_nm, copper_thickness_nm,
                                       dielectric_height_nm, relative_permittivity)
    differential = 2 * single * (1 - 0.48 * exp(-0.96 * gap_nm / dielectric_height_nm))
    value = _ohms(differential)
    validity = ["requires field-solver qualification for fabrication release",
                "ignores solder mask, roughness, weave and local copper"]
    w_h, s_h = width_nm / dielectric_height_nm, gap_nm / dielectric_height_nm
    if not (0.1 <= w_h <= 2.0 and 0.2 <= s_h <= 3.0):
        validity.append("outside the IPC-2141A fitted range 0.1 <= w/h <= 2, 0.2 <= s/h <= 3")
    return EngineeringResult("edge_coupled_microstrip_impedance",
                             _target_status(value, target_ohms, tolerance_ohms),
                             EvidenceGrade.SCREENING, value, "ohm",
                             "Hammerstad single-ended with IPC-2141A edge coupling",
                             tuple(validity))


def _symmetric_stripline(width_nm: int, copper_thickness_nm: int, plane_spacing_nm: int,
                         relative_permittivity: Decimal) -> float:
    """IPC-2141A centred stripline; ``plane_spacing_nm`` is the plane-to-plane distance."""
    argument = 4 * plane_spacing_nm / (0.67 * pi * (0.8 * width_nm + copper_thickness_nm))
    if argument <= 1:
        raise ValueError("stripline trace is too wide for the IPC-2141A estimate")
    return 60 / sqrt(float(relative_permittivity)) * log(argument)


def stripline_impedance(width_nm: int, copper_thickness_nm: int, height_above_nm: int,
                        height_below_nm: int, relative_permittivity: Decimal,
                        *, target_ohms: Decimal | None = None,
                        tolerance_ohms: Decimal = Decimal("0")) -> EngineeringResult:
    """Symmetric or offset stripline single-ended impedance (screening).

    Heights are the dielectric thicknesses from the trace to the planes above
    and below. Equal heights use the IPC-2141A symmetric stripline formula
    with plane spacing ``b = h1 + h2 + t``; an offset trace combines the two
    symmetric lines of spacing ``2h + t`` in parallel, ``Z = 2 Za Zb / (Za + Zb)``.
    """
    if min(width_nm, copper_thickness_nm, height_above_nm, height_below_nm) <= 0 or relative_permittivity < 1:
        raise ValueError("stripline geometry and permittivity are invalid")
    spacing = height_above_nm + height_below_nm + copper_thickness_nm
    if height_above_nm == height_below_nm:
        impedance = _symmetric_stripline(width_nm, copper_thickness_nm, spacing, relative_permittivity)
        scope = "IPC-2141A symmetric stripline estimate"
    else:
        above = _symmetric_stripline(width_nm, copper_thickness_nm,
                                     2 * height_above_nm + copper_thickness_nm, relative_permittivity)
        below = _symmetric_stripline(width_nm, copper_thickness_nm,
                                     2 * height_below_nm + copper_thickness_nm, relative_permittivity)
        impedance = 2 * above * below / (above + below)
        scope = "offset stripline estimate (parallel symmetric lines)"
    value = _ohms(impedance)
    validity = ["requires field-solver qualification for fabrication release",
                "assumes solid reference planes above and below and a homogeneous dielectric"]
    if width_nm / spacing >= 0.35 or copper_thickness_nm / spacing >= 0.25:
        validity.append("outside the IPC-2141A fitted range w/b < 0.35, t/b < 0.25")
    return EngineeringResult("stripline_impedance", _target_status(value, target_ohms, tolerance_ohms),
                             EvidenceGrade.SCREENING, value, "ohm", scope, tuple(validity))


def edge_coupled_stripline_impedance(width_nm: int, gap_nm: int, copper_thickness_nm: int,
                                     height_above_nm: int, height_below_nm: int,
                                     relative_permittivity: Decimal,
                                     *, target_ohms: Decimal | None = None,
                                     tolerance_ohms: Decimal = Decimal("0")) -> EngineeringResult:
    """Edge-coupled stripline differential impedance (screening).

    ``Zdiff = 2 * Z0 * (1 - 0.347 * exp(-2.9 * s / b))`` (IPC-2141A), with the
    symmetric/offset single-ended ``Z0`` and plane spacing ``b``.
    """
    if gap_nm <= 0:
        raise ValueError("differential gap must be positive")
    single = stripline_impedance(width_nm, copper_thickness_nm, height_above_nm,
                                 height_below_nm, relative_permittivity)
    spacing = height_above_nm + height_below_nm + copper_thickness_nm
    differential = 2 * float(single.value) * (1 - 0.347 * exp(-2.9 * gap_nm / spacing))
    value = _ohms(differential)
    return EngineeringResult("edge_coupled_stripline_impedance",
                             _target_status(value, target_ohms, tolerance_ohms),
                             EvidenceGrade.SCREENING, value, "ohm",
                             f"{single.claim_scope} with IPC-2141A edge coupling", single.validity)


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
    return return_path_coverage(board, net, (reference_net,))[0]


def return_path_coverage(board: PhysicalBoard, net: str, reference_nets: tuple[str, ...]
                         ) -> tuple[EngineeringResult, tuple[TrackSegment, ...]]:
    """Midpoint coverage of ``net``'s tracks by adjacent-layer reference fills.

    Returns the screening result and the uncovered track segments in board
    order. A fill of any zone on one of ``reference_nets`` counts.
    """
    zones = {zone.id: zone.net for zone in board.zones}
    references = set(reference_nets)
    fills = [fill for fill in board.zone_fills if zones.get(fill.zone_id) in references]
    tracks = [track for track in board.tracks if track.net == net]
    if not tracks or not fills:
        return EngineeringResult("return_path_continuity", AnalysisStatus.INDETERMINATE,
                                 EvidenceGrade.SCREENING, None, "fraction",
                                 "midpoint coverage by adjacent reference-plane fill",
                                 ("route or normalized reference-plane fill is unavailable",)), ()
    layer_index = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    uncovered: list[TrackSegment] = []
    for track in tracks:
        midpoint = type(track.start)((track.start.x_nm + track.end.x_nm) // 2,
                                     (track.start.y_nm + track.end.y_nm) // 2)
        index = layer_index[track.layer]
        adjacent = {value for value in (index - 1, index + 1)
                    if 0 <= value < len(board.stackup.copper_layers)}
        if not any(layer_index[fill.layer] in adjacent
                   and any(point_in_polygon(midpoint, polygon.outer.vertices)
                           and not any(point_in_polygon(midpoint, hole.vertices)
                                       for hole in polygon.holes)
                           for polygon in fill.polygons)
                   for fill in fills):
            uncovered.append(track)
    covered = len(tracks) - len(uncovered)
    fraction = Decimal(covered) / Decimal(len(tracks))
    return EngineeringResult("return_path_continuity",
                             AnalysisStatus.PASS if covered == len(tracks) else AnalysisStatus.FAIL,
                             EvidenceGrade.SCREENING, fraction, "fraction",
                             "track-midpoint coverage on an adjacent reference layer",
                             ("does not model plane resonances, stitching inductance or split-edge fringing",)
                             ), tuple(uncovered)


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

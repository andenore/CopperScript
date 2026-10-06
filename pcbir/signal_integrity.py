"""Signal-integrity screening of routing intent (D-PHY plan L2-L5, L7).

Every result here is screening evidence (``EvidenceGrade.SCREENING``): closed
form impedance estimates, adjacency of declared reference planes, midpoint
return-path coverage and routed-length bookkeeping. None of it replaces a
field solver, the fabricator's stack-up qualification or channel sign-off.
Warnings never add constraints the source did not declare.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
import json
from math import hypot
from typing import Callable, Iterable

from .engineering import (
    AnalysisStatus,
    EngineeringResult,
    EvidenceGrade,
    edge_coupled_microstrip_impedance,
    edge_coupled_stripline_impedance,
    microstrip_effective_permittivity,
    microstrip_impedance,
    return_path_coverage,
    stripline_impedance,
)
from .physical import (
    CopperLayer,
    NetRoutingRule,
    PhysicalBoard,
    RouteKind,
    Stackup,
    StackupLayerKind,
    TrackSegment,
)


SI_IMPEDANCE = "SI-IMPEDANCE"
SI_NO_STACKUP = "SI-NO-STACKUP"
SI_NO_REFERENCE_PLANE = "SI-NO-REFERENCE-PLANE"
SI_LAYER_GROUP = "SI-LAYER-GROUP"
SI_LAYER_GROUP_SPLIT = "SI-LAYER-GROUP-SPLIT"

DIFFERENTIAL_KINDS = frozenset({RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS})
REFERENCE_PLANE_KINDS = frozenset({RouteKind.DIFFERENTIAL, RouteKind.CLOCK, RouteKind.RF_FEED})
RETURN_PATH_KINDS = frozenset({RouteKind.CRITICAL, RouteKind.DIFFERENTIAL, RouteKind.CLOCK,
                               RouteKind.CAN_BUS, RouteKind.RF_FEED})


@dataclass(frozen=True, slots=True)
class SiWarning:
    code: str
    message: str
    nets: tuple[str, ...] = ()
    layers: tuple[str, ...] = ()

    def to_dict(self) -> dict[str, object]:
        return {"code": self.code, "severity": "warning", "message": self.message,
                "nets": list(self.nets), "layers": list(self.layers)}

    def __str__(self) -> str:
        return f"WARNING {self.code}: {self.message}"


# --- stack-up geometry -----------------------------------------------------


@dataclass(frozen=True, slots=True)
class LayerGeometry:
    """Transmission-line structure of one copper layer in a declared stack-up.

    Outer layers are microstrip over the adjacent dielectric; inner layers are
    stripline between the dielectrics above and below. Adjacent copper layers
    are assumed to be reference planes (see ``reference_plane_warnings``).
    Stripline permittivity is the thickness-weighted mean of both dielectrics.
    """

    layer: CopperLayer
    structure: str
    copper_thickness_nm: int
    height_above_nm: int | None
    height_below_nm: int | None
    relative_permittivity: Decimal

    @property
    def dielectric_height_nm(self) -> int:
        """Microstrip height, or the plane-to-plane spacing of a stripline."""
        if self.structure == "microstrip":
            return self.height_above_nm or self.height_below_nm or 0
        return (self.height_above_nm or 0) + (self.height_below_nm or 0) + self.copper_thickness_nm


def layer_geometry(stackup: Stackup, layer: CopperLayer) -> LayerGeometry | None:
    """Return the screening geometry of ``layer`` or None without a stack-up."""

    layers = stackup.physical_layers
    index = next((i for i, item in enumerate(layers) if item.copper_layer is layer), None)
    if index is None:
        return None
    above = layers[index - 1] if index > 0 else None
    below = layers[index + 1] if index + 1 < len(layers) else None
    neighbours = [item for item in (above, below) if item is not None]
    if not neighbours or any(item.kind is not StackupLayerKind.DIELECTRIC
                             or item.relative_permittivity is None for item in neighbours):
        return None
    thickness = layers[index].thickness_nm
    if len(neighbours) == 1:
        return LayerGeometry(layer, "microstrip", thickness,
                             above.thickness_nm if above is not None else None,
                             below.thickness_nm if below is not None else None,
                             neighbours[0].relative_permittivity)
    total = above.thickness_nm + below.thickness_nm
    er = (above.relative_permittivity * above.thickness_nm
          + below.relative_permittivity * below.thickness_nm) / Decimal(total)
    return LayerGeometry(layer, "stripline", thickness, above.thickness_nm, below.thickness_nm,
                         er.quantize(Decimal("0.000001")).normalize())


def effective_permittivity(stackup: Stackup, layer: CopperLayer, width_nm: int) -> Decimal | None:
    """Quasi-static effective permittivity for delay estimates (screening)."""

    geometry = layer_geometry(stackup, layer)
    if geometry is None:
        return None
    if geometry.structure == "stripline":
        return geometry.relative_permittivity
    return microstrip_effective_permittivity(width_nm, geometry.copper_thickness_nm,
                                             geometry.dielectric_height_nm,
                                             geometry.relative_permittivity)


# --- L2 impedance screening ------------------------------------------------


@dataclass(frozen=True, slots=True)
class ImpedanceEstimate:
    nets: tuple[str, ...]
    layer: str
    structure: str
    width_nm: int
    gap_nm: int | None
    single_ended_ohms: Decimal | None
    differential_ohms: Decimal | None
    target_single_ended_ohms: int | None
    target_differential_ohms: int | None
    tolerance_percent: Decimal
    status: AnalysisStatus
    validity: tuple[str, ...] = ()
    evidence_grade: EvidenceGrade = EvidenceGrade.SCREENING

    def to_dict(self) -> dict[str, object]:
        return {
            "nets": list(self.nets), "layer": self.layer, "structure": self.structure,
            "width_nm": self.width_nm, "gap_nm": self.gap_nm,
            "single_ended_ohms": _text(self.single_ended_ohms),
            "differential_ohms": _text(self.differential_ohms),
            "target_single_ended_ohms": self.target_single_ended_ohms,
            "target_differential_ohms": self.target_differential_ohms,
            "tolerance_percent": _text(self.tolerance_percent),
            "status": self.status.value, "validity": list(self.validity),
            "evidence_grade": self.evidence_grade.value,
        }

    def __str__(self) -> str:
        parts = []
        if self.differential_ohms is not None:
            parts.append(f"diff {_ohm(self.differential_ohms)}"
                         + (f" (target {self.target_differential_ohms} ±{_text(self.tolerance_percent)}%)"
                            if self.target_differential_ohms is not None else ""))
        if self.single_ended_ohms is not None:
            parts.append(f"SE {_ohm(self.single_ended_ohms)}"
                         + (f" (target {self.target_single_ended_ohms} ±{_text(self.tolerance_percent)}%)"
                            if self.target_single_ended_ohms is not None else ""))
        geometry = f"w={_mm(self.width_nm)}mm" + (f" g={_mm(self.gap_nm)}mm" if self.gap_nm else "")
        return (f"IMPEDANCE {','.join(self.nets)} {self.layer} {self.structure} {geometry}: "
                f"{'; '.join(parts) or 'not screened'}: {self.status.value} (screening)")


def screen_impedance(board: PhysicalBoard) -> tuple[tuple[ImpedanceEstimate, ...], tuple[SiWarning, ...]]:
    """Screen every rule with an impedance target on every allowed layer."""

    estimates: list[ImpedanceEstimate] = []
    warnings: list[SiWarning] = []
    targeted = [rule for rule in board.net_routing_rules
                if rule.target_impedance_ohms is not None or rule.target_single_ended_ohms is not None]

    def key(rule: NetRoutingRule) -> tuple[object, ...]:
        return (rule.kind, rule.width_nm, rule.pair_gap_nm, rule.allowed_layers,
                rule.target_impedance_ohms, rule.target_single_ended_ohms,
                rule.effective_impedance_tolerance_percent)

    for nets, rule in _rule_sets(targeted, key):
        differential = rule.kind in DIFFERENTIAL_KINDS
        target_diff = rule.target_impedance_ohms if differential else None
        target_se = (rule.target_single_ended_ohms if differential
                     else rule.target_single_ended_ohms or rule.target_impedance_ohms)
        tolerance = rule.effective_impedance_tolerance_percent
        label = "/".join(nets)
        if not board.stackup.physical_layers:
            declared = ", ".join(text for text in (
                f"{target_diff} ohm differential" if target_diff is not None else "",
                f"{target_se} ohm single-ended" if target_se is not None else "") if text)
            warnings.append(SiWarning(
                SI_NO_STACKUP,
                f"{label} declares an impedance target ({declared}) but the board has no declared "
                "stack-up; impedance is not screened (declare mechanical stackup)",
                nets))
            continue
        width = rule.width_nm or board.rules.default_track_width_nm
        gap = rule.pair_gap_nm if differential else None
        for layer in rule.allowed_layers or board.stackup.copper_layers:
            estimate = _estimate(board.stackup, layer, width, gap, nets,
                                 target_se, target_diff, tolerance)
            estimates.append(estimate)
            warnings.extend(_impedance_warnings(estimate, label))
    return tuple(estimates), tuple(warnings)


def _estimate(stackup: Stackup, layer: CopperLayer, width: int, gap: int | None,
              nets: tuple[str, ...], target_se: int | None, target_diff: int | None,
              tolerance: Decimal) -> ImpedanceEstimate:
    geometry = layer_geometry(stackup, layer)
    if geometry is None:
        return ImpedanceEstimate(nets, layer.value, "unknown", width, gap, None, None, target_se,
                                 target_diff, tolerance, AnalysisStatus.INDETERMINATE,
                                 ("layer has no adjacent declared dielectric",))
    try:
        if geometry.structure == "microstrip":
            single = microstrip_impedance(width, geometry.copper_thickness_nm,
                                          geometry.dielectric_height_nm, geometry.relative_permittivity)
            pair = (edge_coupled_microstrip_impedance(
                width, gap, geometry.copper_thickness_nm, geometry.dielectric_height_nm,
                geometry.relative_permittivity) if gap is not None else None)
        else:
            single = stripline_impedance(width, geometry.copper_thickness_nm, geometry.height_above_nm,
                                         geometry.height_below_nm, geometry.relative_permittivity)
            pair = (edge_coupled_stripline_impedance(
                width, gap, geometry.copper_thickness_nm, geometry.height_above_nm,
                geometry.height_below_nm, geometry.relative_permittivity) if gap is not None else None)
    except ValueError as exc:
        return ImpedanceEstimate(nets, layer.value, geometry.structure, width, gap, None, None,
                                 target_se, target_diff, tolerance, AnalysisStatus.INDETERMINATE,
                                 (f"cannot be screened: {exc}",))
    checks = [_within(single.value, target_se, tolerance), _within(pair.value if pair else None,
                                                                    target_diff, tolerance)]
    known = [check for check in checks if check is not None]
    status = (AnalysisStatus.INDETERMINATE if not known else
              AnalysisStatus.PASS if all(known) else AnalysisStatus.FAIL)
    validity = tuple(dict.fromkeys((*single.validity, *(pair.validity if pair else ()))))
    return ImpedanceEstimate(nets, layer.value, _structure_name(geometry), width, gap,
                             single.value, pair.value if pair else None, target_se, target_diff,
                             tolerance, status, validity)


def _structure_name(geometry: LayerGeometry) -> str:
    if geometry.structure == "microstrip":
        return "microstrip"
    return ("symmetric_stripline" if geometry.height_above_nm == geometry.height_below_nm
            else "offset_stripline")


def _within(value: Decimal | None, target: int | None, tolerance: Decimal) -> bool | None:
    if value is None or target is None:
        return None
    return abs(value - Decimal(target)) <= Decimal(target) * tolerance / Decimal(100)


def _impedance_warnings(estimate: ImpedanceEstimate, label: str) -> list[SiWarning]:
    if estimate.status is AnalysisStatus.INDETERMINATE:
        if estimate.single_ended_ohms is None:
            return [SiWarning(SI_IMPEDANCE, f"{label} on {estimate.layer}: impedance "
                              f"{'; '.join(estimate.validity)}", estimate.nets, (estimate.layer,))]
        return []
    result = []
    geometry = (f"{estimate.structure}, width {_mm(estimate.width_nm)} mm"
                + (f", gap {_mm(estimate.gap_nm)} mm" if estimate.gap_nm else ""))
    for kind, value, target in (("differential", estimate.differential_ohms, estimate.target_differential_ohms),
                                ("single-ended", estimate.single_ended_ohms, estimate.target_single_ended_ohms)):
        if _within(value, target, estimate.tolerance_percent) is False:
            result.append(SiWarning(
                SI_IMPEDANCE,
                f"{label} on {estimate.layer}: {kind} estimate {_ohm(value)} is outside "
                f"{target} ohm ±{_text(estimate.tolerance_percent)}% ({geometry}); "
                "screening estimate, not sign-off",
                estimate.nets, (estimate.layer,)))
    return result


# --- L3 reference planes and return paths ----------------------------------


def reference_plane_warnings(board: PhysicalBoard) -> tuple[SiWarning, ...]:
    """Warn when a differential/clock/RF rule may use a layer without an adjacent plane.

    Any declared ``copper_zone`` on an adjacent copper layer counts as a
    reference; whether it is continuous under the route is a post-fill check.
    """

    copper = board.stackup.copper_layers
    zoned = {layer for zone in board.zones for layer in zone.layers}
    rules = [rule for rule in board.net_routing_rules if rule.kind in REFERENCE_PLANE_KINDS]
    warnings: list[SiWarning] = []
    for nets, rule in _rule_sets(rules, lambda item: (item.kind, item.allowed_layers)):
        missing = []
        for layer in rule.allowed_layers or copper:
            index = copper.index(layer)
            adjacent = [copper[i] for i in (index - 1, index + 1) if 0 <= i < len(copper)]
            if not any(item in zoned for item in adjacent):
                missing.append((layer, adjacent))
        if missing:
            detail = "; ".join(f"{layer.value} (adjacent: {', '.join(item.value for item in adjacent)})"
                               for layer, adjacent in missing)
            warnings.append(SiWarning(
                SI_NO_REFERENCE_PLANE,
                f"{rule.kind.value} rule for {'/'.join(nets)} allows a layer with no copper_zone on an "
                f"adjacent copper layer: {detail}; declare a reference plane or remove the layer "
                "from allowed_layers",
                nets, tuple(layer.value for layer, _ in missing)))
    return tuple(warnings)


@dataclass(frozen=True, slots=True)
class ReturnPathResult:
    net: str
    reference_nets: tuple[str, ...]
    result: EngineeringResult
    uncovered: tuple[TrackSegment, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "net": self.net, "reference_nets": list(self.reference_nets),
            "status": self.result.status.value, "fraction": _text(self.result.value),
            "evidence_grade": self.result.evidence_grade.value,
            "claim_scope": self.result.claim_scope, "validity": list(self.result.validity),
            "uncovered_segments": [
                {"layer": track.layer.value, "start": [track.start.x_nm, track.start.y_nm],
                 "end": [track.end.x_nm, track.end.y_nm], "width_nm": track.width_nm}
                for track in self.uncovered
            ],
        }


def return_path_report(board: PhysicalBoard, *,
                       kinds: Iterable[RouteKind] = RETURN_PATH_KINDS) -> tuple[ReturnPathResult, ...]:
    """Run return-path continuity for every critical net of a filled board.

    The reference is the rule's ``return_via_net`` when declared, otherwise
    every net that owns a copper zone (except the net itself). Without
    normalized zone fills the result is indeterminate, never a pass.
    """

    selected = frozenset(kinds)
    zone_nets = tuple(sorted({zone.net for zone in board.zones}))
    results = []
    for rule in sorted(board.net_routing_rules, key=lambda item: item.net):
        if rule.kind not in selected:
            continue
        references = ((rule.return_via_net,) if rule.return_via_net
                      else tuple(net for net in zone_nets if net != rule.net))
        result, uncovered = return_path_coverage(board, rule.net, references)
        results.append(ReturnPathResult(rule.net, references, result, uncovered))
    return tuple(results)


# --- L4 length matching ----------------------------------------------------


def routed_track_length_nm(board: PhysicalBoard, net: str) -> int:
    """Total routed track length of ``net`` (same rounding as physical DRC)."""
    return sum(round(hypot(track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm))
               for track in board.tracks if track.net == net)


@dataclass(frozen=True, slots=True)
class MatchMemberLength:
    net: str
    length_nm: int
    routed: bool


@dataclass(frozen=True, slots=True)
class MatchGroupResult:
    id: str
    max_skew_nm: int
    members: tuple[MatchMemberLength, ...]
    skew_nm: int | None
    status: str

    @property
    def complete(self) -> bool:
        return all(member.routed for member in self.members)

    def to_dict(self) -> dict[str, object]:
        longest = max(member.length_nm for member in self.members)
        return {
            "id": self.id, "max_skew_nm": self.max_skew_nm, "skew_nm": self.skew_nm,
            "status": self.status,
            "members": [{"net": member.net, "length_nm": member.length_nm, "routed": member.routed,
                         "shortfall_nm": longest - member.length_nm} for member in self.members],
        }

    def __str__(self) -> str:
        skew = "n/a until every member is routed" if self.skew_nm is None else f"{_mm(self.skew_nm)} mm"
        return (f"MATCH GROUP {self.id}: {len(self.members)} nets, max skew {_mm(self.max_skew_nm)} mm, "
                f"skew {skew}: {self.status}")


def verify_match_groups(board: PhysicalBoard) -> tuple[MatchGroupResult, ...]:
    """Member lengths and group skew from routed tracks.

    A group is ``pass`` or ``fail`` only once every member net is connected;
    before that it is ``incomplete`` with ``skew_nm`` None.
    """

    if not board.match_groups:
        return ()
    from .drc import explicit_copper_connectivity

    members = frozenset(net for group in board.match_groups for net in group.nets)
    graph = explicit_copper_connectivity(board, only_nets=members)
    physical = {net.name: net for net in board.nets}
    results = []
    for group in board.match_groups:
        lengths = tuple(MatchMemberLength(net, routed_track_length_nm(board, net),
                                          graph.net_connected(physical[net]))
                        for net in group.nets)
        if all(member.routed for member in lengths):
            skew = max(m.length_nm for m in lengths) - min(m.length_nm for m in lengths)
            status = "pass" if skew <= group.max_skew_nm else "fail"
        else:
            skew, status = None, "incomplete"
        results.append(MatchGroupResult(group.id, group.max_skew_nm, lengths, skew, status))
    return tuple(results)


# --- L5 layer groups -------------------------------------------------------


def layer_groups(board: PhysicalBoard) -> dict[str, tuple[NetRoutingRule, ...]]:
    groups: dict[str, list[NetRoutingRule]] = {}
    for rule in sorted(board.net_routing_rules, key=lambda item: item.net):
        if rule.layer_group is not None:
            groups.setdefault(rule.layer_group, []).append(rule)
    return {name: tuple(groups[name]) for name in sorted(groups)}


def layer_group_warnings(board: PhysicalBoard) -> tuple[SiWarning, ...]:
    """Pre-route: members of one ``layer_group`` must allow the same layer set."""

    warnings = []
    for name, rules in layer_groups(board).items():
        nets = tuple(rule.net for rule in rules)
        if len(rules) == 1:
            warnings.append(SiWarning(SI_LAYER_GROUP, f"layer group {name!r} has a single member "
                                      f"({nets[0]})", nets))
            continue
        allowed = {rule.net: _ordered(board, rule.allowed_layers or board.stackup.copper_layers)
                   for rule in rules}
        if len(set(allowed.values())) > 1:
            detail = "; ".join(f"{net}: {','.join(layer.value for layer in layers)}"
                               for net, layers in allowed.items())
            warnings.append(SiWarning(
                SI_LAYER_GROUP, f"layer group {name!r} members allow different layer sets: {detail}",
                nets, tuple(sorted({layer.value for layers in allowed.values() for layer in layers}))))
    return tuple(warnings)


@dataclass(frozen=True, slots=True)
class LayerGroupMember:
    net: str
    allowed_layers: tuple[str, ...]
    routed_layers: tuple[str, ...]
    main_layer: str | None


@dataclass(frozen=True, slots=True)
class LayerGroupReport:
    name: str
    members: tuple[LayerGroupMember, ...]

    @property
    def consistent(self) -> bool:
        return len({member.main_layer for member in self.members if member.main_layer}) <= 1

    def to_dict(self) -> dict[str, object]:
        return {"name": self.name, "consistent": self.consistent, "members": [
            {"net": member.net, "allowed_layers": list(member.allowed_layers),
             "routed_layers": list(member.routed_layers), "main_layer": member.main_layer}
            for member in self.members]}


def layer_group_report(board: PhysicalBoard) -> tuple[tuple[LayerGroupReport, ...], tuple[SiWarning, ...]]:
    """Post-route: each member's routed layers and its main-run layer.

    The main layer carries the most routed track length (ties go to the upper
    layer). A warning reports groups whose routed members' main runs differ.
    """

    reports, warnings = [], []
    for name, rules in layer_groups(board).items():
        members = []
        for rule in rules:
            totals: dict[CopperLayer, int] = {}
            for track in board.tracks:
                if track.net == rule.net:
                    totals[track.layer] = totals.get(track.layer, 0) + round(
                        hypot(track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm))
            routed = _ordered(board, tuple(totals))
            main = max(routed, key=lambda layer: totals[layer]) if routed else None
            members.append(LayerGroupMember(
                rule.net, tuple(layer.value for layer in _ordered(board, rule.allowed_layers
                                                                  or board.stackup.copper_layers)),
                tuple(layer.value for layer in routed), main.value if main else None))
        report = LayerGroupReport(name, tuple(members))
        reports.append(report)
        if not report.consistent:
            detail = "; ".join(f"{member.net}: {member.main_layer}" for member in members if member.main_layer)
            warnings.append(SiWarning(
                SI_LAYER_GROUP_SPLIT, f"layer group {name!r} main runs are on different layers: {detail}",
                tuple(member.net for member in members),
                tuple(sorted({member.main_layer for member in members if member.main_layer}))))
    return tuple(reports), tuple(warnings)


# --- L7 aggregate pre-route check ------------------------------------------


@dataclass(frozen=True, slots=True)
class SiCheckReport:
    board: str
    copper_layers: tuple[str, ...]
    stackup_declared: bool
    thickness_nm: int
    impedance: tuple[ImpedanceEstimate, ...]
    match_groups: tuple[MatchGroupResult, ...]
    layer_groups: tuple[tuple[str, tuple[str, ...]], ...]
    warnings: tuple[SiWarning, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": "copperscript-si-check/v0.1",
            "board": self.board,
            "evidence_grade": EvidenceGrade.SCREENING.value,
            "stackup": {"declared": self.stackup_declared, "copper_layers": list(self.copper_layers),
                        "thickness_nm": self.thickness_nm},
            "impedance": [item.to_dict() for item in self.impedance],
            "match_groups": [item.to_dict() for item in self.match_groups],
            "layer_groups": [{"name": name, "nets": list(nets)} for name, nets in self.layer_groups],
            "warnings": [item.to_dict() for item in self.warnings],
        }

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2, sort_keys=True) + "\n"

    def lines(self) -> list[str]:
        stack = (f"declared stack-up {_mm(self.thickness_nm)} mm" if self.stackup_declared
                 else "no declared stack-up")
        result = [f"SI screening for {self.board}: {len(self.copper_layers)} copper layers, {stack} "
                  "(screening evidence, not sign-off)"]
        result.extend(str(item) for item in self.impedance)
        result.extend(str(item) for item in self.match_groups)
        result.extend(f"LAYER GROUP {name}: {', '.join(nets)}" for name, nets in self.layer_groups)
        result.extend(str(item) for item in self.warnings)
        result.append(f"SI check: {len(self.warnings)} warning(s)")
        return result


def si_check(board: PhysicalBoard) -> SiCheckReport:
    """Pre-route screening: impedance (L2), reference planes (L3), layer groups (L5)
    and a summary of length-match groups (L4)."""

    estimates, impedance_warnings = screen_impedance(board)
    warnings = (*impedance_warnings, *reference_plane_warnings(board), *layer_group_warnings(board))
    return SiCheckReport(
        board.name,
        tuple(layer.value for layer in board.stackup.copper_layers),
        bool(board.stackup.physical_layers),
        board.stackup.thickness_nm,
        estimates,
        verify_match_groups(board),
        tuple((name, tuple(rule.net for rule in rules)) for name, rules in layer_groups(board).items()),
        tuple(warnings),
    )


# --- helpers ---------------------------------------------------------------


def _rule_sets(rules: Iterable[NetRoutingRule], key: Callable[[NetRoutingRule], object]
               ) -> list[tuple[tuple[str, ...], NetRoutingRule]]:
    """Group differential partners whose screening inputs are identical."""

    by_net = {rule.net: rule for rule in rules}
    done: set[str] = set()
    result = []
    for net in sorted(by_net):
        if net in done:
            continue
        rule = by_net[net]
        partner = by_net.get(rule.differential_partner) if rule.differential_partner else None
        if (partner is not None and partner.net not in done
                and partner.differential_partner == rule.net and key(partner) == key(rule)):
            nets = (rule.net, partner.net)
        else:
            nets = (rule.net,)
        done.update(nets)
        result.append((nets, rule))
    return result


def _ordered(board: PhysicalBoard, layers: Iterable[CopperLayer]) -> tuple[CopperLayer, ...]:
    order = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    return tuple(sorted(set(layers), key=lambda layer: order.get(layer, len(order))))


def _mm(value_nm: int | None) -> str:
    if value_nm is None:
        return "-"
    return format((Decimal(value_nm) / Decimal(1_000_000)).normalize(), "f")


def _ohm(value: Decimal | None) -> str:
    return "-" if value is None else f"{value.quantize(Decimal('0.1'))} ohm"


def _text(value: Decimal | None) -> str | None:
    return None if value is None else format(value.normalize(), "f")

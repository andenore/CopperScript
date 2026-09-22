"""Fail-closed physical design-rule checking and geometry-bound signoff."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum
from fractions import Fraction
from hashlib import sha256
import json
from math import cos, hypot, isqrt, radians, sin
from typing import Iterable

from .physical import (
    BoardSide,
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PadReference,
    PadShape,
    PhysicalBoard,
    Point,
    TrackSegment,
    Via,
)
from .placement import transformed_pad_position
from .geometry import (
    RoundedConvexShape,
    SpatialIndex,
    SpatialItem,
    point_in_polygon,
    point_segment_distance_squared,
    segment_distance_squared,
    shape_distance_squared,
    shapes_clear,
)
from .placement import placement_solution_is_legal


class DrcSeverity(str, Enum):
    ERROR = "error"
    WARNING = "warning"
    INFO = "info"


class DrcDisposition(str, Enum):
    ACTIVE = "active"
    WAIVED = "waived"


class DrcDecision(str, Enum):
    PASS = "pass"
    PASS_WITH_WAIVERS = "pass_with_waivers"
    FAIL = "fail"


class DrcCompleteness(str, Enum):
    COMPLETE = "complete"
    INCOMPLETE = "incomplete"


class DrcCoverageStatus(str, Enum):
    EXECUTED = "executed"
    NOT_APPLICABLE = "not_applicable"
    UNSUPPORTED = "unsupported"
    FAILED = "failed"


@dataclass(frozen=True, slots=True)
class DrcCoverage:
    check: str
    status: DrcCoverageStatus
    required: bool
    detail: str = ""


@dataclass(frozen=True, slots=True)
class DrcFinding:
    code: str
    severity: DrcSeverity
    message: str
    objects: tuple[str, ...] = ()
    nets: tuple[str, ...] = ()
    layers: tuple[str, ...] = ()
    required_nm: int | None = None
    measured_nm: int | None = None
    disposition: DrcDisposition = DrcDisposition.ACTIVE
    waiver_reason: str | None = None

    @property
    def fingerprint(self) -> str:
        return _digest(
            {
                "code": self.code,
                "message": self.message,
                "objects": self.objects,
                "nets": self.nets,
                "layers": self.layers,
                "required_nm": self.required_nm,
                "measured_nm": self.measured_nm,
            }
        )


@dataclass(frozen=True, slots=True)
class DrcWaiver:
    finding_fingerprint: str
    reason: str
    approved_by: str

    def __post_init__(self) -> None:
        if not self.finding_fingerprint or not self.reason or not self.approved_by:
            raise ValueError("a DRC waiver requires a finding, reason, and approver")


@dataclass(frozen=True, slots=True)
class SignoffToken:
    schema: str
    board_digest: str
    rules_digest: str
    report_digest: str
    decision: DrcDecision
    completeness: DrcCompleteness

    @property
    def token_digest(self) -> str:
        return _digest(
            {
                "schema": self.schema,
                "board_digest": self.board_digest,
                "rules_digest": self.rules_digest,
                "report_digest": self.report_digest,
                "decision": self.decision.value,
                "completeness": self.completeness.value,
            }
        )


@dataclass(frozen=True, slots=True)
class PhysicalDrcPolicy:
    require_completed_detailed_route: bool = True
    fail_on_warnings: bool = False


@dataclass(frozen=True, slots=True)
class PhysicalDrcReport:
    decision: DrcDecision
    completeness: DrcCompleteness
    findings: tuple[DrcFinding, ...]
    coverage: tuple[DrcCoverage, ...]
    token: SignoffToken

    def to_json(self) -> str:
        return json.dumps(_report_document(self, include_token=True), indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True, slots=True)
class _PadCopper:
    identity: str
    net: str
    position: Point
    shape: RoundedConvexShape
    layers: tuple[CopperLayer, ...]


def run_physical_drc(
    board: PhysicalBoard,
    waivers: Iterable[DrcWaiver] = (),
    policy: PhysicalDrcPolicy | None = None,
) -> PhysicalDrcReport:
    """Check exact physical IR and bind the result to a canonical geometry digest."""

    policy = policy or PhysicalDrcPolicy()
    findings: list[DrcFinding] = []
    coverage: list[DrcCoverage] = []

    if policy.require_completed_detailed_route and board.metadata.get("detailed_routing") != "complete":
        findings.append(
            _finding(
                "DRC-ROUTE-INCOMPLETE",
                DrcSeverity.ERROR,
                "detailed routing is not complete for this exact board",
            )
        )
        coverage.append(DrcCoverage("route_completeness", DrcCoverageStatus.FAILED, True))
    else:
        coverage.append(DrcCoverage("route_completeness", DrcCoverageStatus.EXECUTED, True))

    _check_connectivity(board, findings)
    coverage.append(DrcCoverage("connectivity", DrcCoverageStatus.EXECUTED, True))
    if not placement_solution_is_legal(
        board, {item.reference: item for item in board.placements}
    ):
        findings.append(_finding("DRC-PLACEMENT", DrcSeverity.ERROR, "component placement violates board, keepout, or courtyard legality"))
    coverage.append(DrcCoverage("placement_legality", DrcCoverageStatus.EXECUTED, True))
    _check_track_rules(board, findings)
    coverage.append(DrcCoverage("track_width_and_budgets", DrcCoverageStatus.EXECUTED, True))
    _check_differential_rules(board, findings)
    coverage.append(DrcCoverage("differential_geometry", DrcCoverageStatus.EXECUTED, True))
    _check_vias(board, findings)
    coverage.append(DrcCoverage("via_geometry_and_budgets", DrcCoverageStatus.EXECUTED, True))
    _check_board_edge(board, findings)
    coverage.append(DrcCoverage("copper_to_board_edge", DrcCoverageStatus.EXECUTED, True))
    _check_copper_spacing(board, findings)
    coverage.append(DrcCoverage("shorts_and_clearance", DrcCoverageStatus.EXECUTED, True))

    if board.zone_fills:
        _check_zone_fill_spacing(board, findings)
        coverage.append(DrcCoverage("copper_zones", DrcCoverageStatus.EXECUTED, False,
                                    "checked content-bound normalized fill polygons"))
    else:
        coverage.append(
            DrcCoverage(
                "copper_zones",
                DrcCoverageStatus.UNSUPPORTED if board.zones else DrcCoverageStatus.NOT_APPLICABLE,
                False,
                "zone intent is refilled and checked by the pinned KiCad manufacturing stage"
                if board.zones else "",
            )
        )
    for name, detail in (
        ("solder_mask", "mask sliver and expansion checks require final artwork"),
        ("silkscreen", "silkscreen-to-mask checks require final artwork"),
        ("creepage", "creepage requires a declared safety profile"),
        ("signal_integrity", "impedance and signal integrity require external qualification"),
        ("thermal", "thermal/current qualification requires external analysis"),
    ):
        coverage.append(DrcCoverage(name, DrcCoverageStatus.UNSUPPORTED, False, detail))

    waiver_map = {item.finding_fingerprint: item for item in waivers}
    waived: list[DrcFinding] = []
    for finding in sorted(findings, key=_finding_key):
        waiver = waiver_map.get(finding.fingerprint)
        if waiver is not None:
            finding = replace(
                finding,
                disposition=DrcDisposition.WAIVED,
                waiver_reason=f"{waiver.reason} (approved by {waiver.approved_by})",
            )
        waived.append(finding)

    completeness = (
        DrcCompleteness.COMPLETE
        if all(
            not item.required
            or item.status in {DrcCoverageStatus.EXECUTED, DrcCoverageStatus.NOT_APPLICABLE}
            for item in coverage
        )
        else DrcCompleteness.INCOMPLETE
    )
    blocking = any(
        item.disposition is DrcDisposition.ACTIVE
        and (
            item.severity is DrcSeverity.ERROR
            or (policy.fail_on_warnings and item.severity is DrcSeverity.WARNING)
        )
        for item in waived
    )
    if blocking or completeness is DrcCompleteness.INCOMPLETE:
        decision = DrcDecision.FAIL
    elif any(item.disposition is DrcDisposition.WAIVED for item in waived):
        decision = DrcDecision.PASS_WITH_WAIVERS
    else:
        decision = DrcDecision.PASS

    board_digest = physical_board_digest(board)
    rules_digest = _rules_digest(board, policy)
    provisional = PhysicalDrcReport(
        decision,
        completeness,
        tuple(waived),
        tuple(coverage),
        SignoffToken("copperscript-signoff/v0.1", board_digest, rules_digest, "", decision, completeness),
    )
    report_digest = _digest(_report_document(provisional, include_token=False))
    token = replace(provisional.token, report_digest=report_digest)
    return replace(provisional, token=token)


def run_incremental_physical_drc(
    board: PhysicalBoard,
    changed_objects: Iterable[str],
    waivers: Iterable[DrcWaiver] = (),
    policy: PhysicalDrcPolicy | None = None,
) -> PhysicalDrcReport:
    """Authoritative incremental contract.

    The current implementation intentionally delegates to the full exact run;
    this establishes result equivalence before introducing cached spatial
    invalidation as a performance-only optimization.
    """
    tuple(changed_objects)  # validate/consume a potentially lazy caller input
    return run_physical_drc(board, waivers, policy)


def physical_board_digest(board: PhysicalBoard) -> str:
    """Digest all geometry and rules that can affect manufacturing signoff."""

    return _digest(
        {
            "name": board.name,
            "outline": [(p.x_nm, p.y_nm) for p in board.outline.vertices],
            "stackup": {
                "layers": [layer.value for layer in board.stackup.copper_layers],
                "thickness_nm": board.stackup.thickness_nm,
                "physical_layers": [repr(layer) for layer in board.stackup.physical_layers],
                "via_technologies": [repr(item) for item in board.stackup.via_technologies],
            },
            "rules": {
                "clearance": board.rules.minimum_clearance_nm,
                "track": board.rules.default_track_width_nm,
                "via": board.rules.default_via_size_nm,
                "drill": board.rules.default_via_drill_nm,
            },
            "placements": [
                (p.reference, p.footprint, p.position.x_nm, p.position.y_nm, str(p.rotation_degrees), p.side.value)
                for p in sorted(board.placements, key=lambda item: item.reference)
            ],
            "footprints": [
                _footprint_document(name, footprint)
                for name, footprint in sorted(board.footprints.items())
            ],
            "nets": [
                (net.name, [(pad.component, pad.pad) for pad in sorted(net.pads)])
                for net in sorted(board.nets, key=lambda item: item.name)
            ],
            "tracks": [_track_identity(track) for track in sorted(board.tracks, key=_track_identity)],
            "vias": [_via_identity(via) for via in sorted(board.vias, key=_via_identity)],
            "routing_rules": [_routing_rule_document(rule) for rule in sorted(board.net_routing_rules, key=lambda item: item.net)],
            "regions": [repr(item) for item in sorted(board.regions, key=lambda item: item.name)],
            "keepouts": [repr(item) for item in sorted(board.keepouts, key=lambda item: item.name)],
            "placement_rules": [repr(item) for item in sorted(board.placement_rules, key=lambda item: item.reference)],
            "relative_rules": [repr(item) for item in board.relative_rules],
            "placement_groups": [repr(item) for item in sorted(board.placement_groups, key=lambda item: item.name)],
            "zones": [repr(item) for item in sorted(board.zones, key=lambda item: item.id)],
            "copper_keepouts": [repr(item) for item in sorted(board.copper_keepouts, key=lambda item: item.id)],
            "zone_fills": [repr(item) for item in sorted(board.zone_fills, key=lambda item: (item.zone_id, item.layer.value))],
            "metadata": tuple(sorted(board.metadata.items())),
        }
    )


def _check_connectivity(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    layer_indexes = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    tracks = {name: [item for item in board.tracks if item.net == name] for name in (net.name for net in board.nets)}
    vias = {name: [item for item in board.vias if item.net == name] for name in (net.name for net in board.nets)}
    placements = {item.reference: item for item in board.placements}
    for net in sorted(board.nets, key=lambda item: item.name):
        if len(net.pads) < 2:
            continue
        parent: dict[tuple[int, int, int], tuple[int, int, int]] = {}

        def find(node: tuple[int, int, int]) -> tuple[int, int, int]:
            parent.setdefault(node, node)
            if parent[node] != node:
                parent[node] = find(parent[node])
            return parent[node]

        def union(first: tuple[int, int, int], second: tuple[int, int, int]) -> None:
            left, right = find(first), find(second)
            if left != right:
                parent[max(left, right)] = min(left, right)

        for track in tracks[net.name]:
            layer = layer_indexes[track.layer]
            union((layer, track.start.x_nm, track.start.y_nm), (layer, track.end.x_nm, track.end.y_nm))
        for via in vias[net.name]:
            first = layer_indexes[via.from_layer]
            second = layer_indexes[via.to_layer]
            for layer in range(min(first, second), max(first, second)):
                union((layer, via.position.x_nm, via.position.y_nm), (layer + 1, via.position.x_nm, via.position.y_nm))

        pad_nodes: list[tuple[int, int, int]] = []
        for pad_ref in sorted(net.pads):
            placement = placements[pad_ref.component]
            footprint = board.footprints[placement.footprint]
            pad = next(item for item in footprint.pads if item.number == pad_ref.pad)
            point = transformed_pad_position(board, placement, pad_ref.pad)
            if pad.kind is PadKind.SMD:
                layer = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
                nodes = [(layer_indexes[layer], point.x_nm, point.y_nm)]
            else:
                nodes = [(index, point.x_nm, point.y_nm) for index in range(len(layer_indexes))]
                for first, second in zip(nodes, nodes[1:]):
                    union(first, second)
            for node in nodes:
                find(node)
                for track in tracks[net.name]:
                    if layer_indexes[track.layer] == node[0] and _point_segment_distance(point, track.start, track.end) <= track.width_nm / 2:
                        union(node, (node[0], track.start.x_nm, track.start.y_nm))
                for via in vias[net.name]:
                    if via.position == point:
                        union(node, (node[0], point.x_nm, point.y_nm))
            pad_nodes.append(nodes[0])
        if len({find(node) for node in pad_nodes}) > 1:
            findings.append(
                _finding(
                    "DRC-OPEN-NET",
                    DrcSeverity.ERROR,
                    f"net {net.name!r} is not electrically connected by exact copper",
                    nets=(net.name,),
                    objects=tuple(f"pad:{item.component}.{item.pad}" for item in sorted(net.pads)),
                )
            )


def _check_track_rules(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    rules = {item.net: item for item in board.net_routing_rules}
    for index, track in enumerate(board.tracks):
        rule = rules.get(track.net)
        required = rule.width_nm if rule and rule.width_nm is not None else None
        if required is not None and track.width_nm < required:
            findings.append(_finding("DRC-TRACK-WIDTH", DrcSeverity.ERROR, f"track {index} on {track.net!r} is below its routing profile width", objects=(f"track:{index}",), nets=(track.net,), layers=(track.layer.value,), required_nm=required, measured_nm=track.width_nm))
    for net, rule in sorted(rules.items()):
        net_tracks = [item for item in board.tracks if item.net == net]
        length = sum(round(hypot(item.end.x_nm - item.start.x_nm, item.end.y_nm - item.start.y_nm)) for item in net_tracks)
        if rule.max_length_nm is not None and length > rule.max_length_nm:
            findings.append(_finding("DRC-MAX-LENGTH", DrcSeverity.ERROR, f"net {net!r} exceeds its maximum routed length", nets=(net,), required_nm=rule.max_length_nm, measured_nm=length))
        disallowed = [item for item in net_tracks if rule.allowed_layers and item.layer not in rule.allowed_layers]
        if disallowed:
            findings.append(_finding("DRC-LAYER", DrcSeverity.ERROR, f"net {net!r} uses a layer excluded by its routing profile", nets=(net,), layers=tuple(sorted({item.layer.value for item in disallowed}))))


def _check_vias(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    rules = {item.net: item for item in board.net_routing_rules}
    layers = set(board.stackup.copper_layers)
    for index, via in enumerate(board.vias):
        if via.from_layer not in layers or via.to_layer not in layers:
            findings.append(_finding("DRC-VIA-SPAN", DrcSeverity.ERROR, f"via {index} references a layer outside the stackup", objects=(f"via:{index}",), nets=(via.net,)))
        if via.size_nm - via.drill_nm <= 0:
            findings.append(_finding("DRC-ANNULAR-RING", DrcSeverity.ERROR, f"via {index} has no annular ring", objects=(f"via:{index}",), nets=(via.net,)))
    for net, rule in sorted(rules.items()):
        count = sum(item.net == net for item in board.vias)
        if rule.max_vias is not None and count > rule.max_vias:
            findings.append(_finding("DRC-MAX-VIAS", DrcSeverity.ERROR, f"net {net!r} exceeds its maximum via count", nets=(net,), required_nm=rule.max_vias, measured_nm=count))


def _check_differential_rules(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    rules = {item.net: item for item in board.net_routing_rules}
    processed: set[frozenset[str]] = set()
    for rule in sorted(board.net_routing_rules, key=lambda item: item.net):
        if rule.differential_partner is None:
            continue
        pair = frozenset((rule.net, rule.differential_partner))
        if pair in processed:
            continue
        processed.add(pair)
        partner = rules.get(rule.differential_partner)
        if partner is None or partner.differential_partner != rule.net:
            findings.append(_finding("DRC-DIFF-PROFILE", DrcSeverity.ERROR,
                                     "differential routing profiles are not symmetric",
                                     nets=tuple(sorted(pair))))
            continue
        lengths = {
            net: sum(round(hypot(track.end.x_nm - track.start.x_nm,
                                 track.end.y_nm - track.start.y_nm))
                     for track in board.tracks if track.net == net)
            for net in pair
        }
        skew = abs(lengths[rule.net] - lengths[rule.differential_partner])
        limit_values = [value for value in (rule.max_skew_nm, partner.max_skew_nm)
                        if value is not None]
        if limit_values and skew > min(limit_values):
            findings.append(_finding("DRC-DIFF-SKEW", DrcSeverity.ERROR,
                                     f"differential pair skew is {skew} nm",
                                     nets=tuple(sorted(pair)), required_nm=min(limit_values),
                                     measured_nm=skew))
        via_counts = {net: sum(via.net == net for via in board.vias) for net in pair}
        if len(set(via_counts.values())) != 1:
            findings.append(_finding("DRC-DIFF-VIAS", DrcSeverity.ERROR,
                                     "differential pair members have unequal via transitions",
                                     nets=tuple(sorted(pair))))


def _check_board_edge(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    clearance = board.rules.minimum_clearance_nm
    edges = tuple(zip(board.outline.vertices, (*board.outline.vertices[1:], board.outline.vertices[0])))
    for index, track in enumerate(board.tracks):
        margin = min(
            _fraction_sqrt_floor(segment_distance_squared(track.start, track.end, first, second))
            for first, second in edges
        ) - track.width_nm / 2
        inside = all(_point_in_polygon_or_edge(point, board.outline.vertices) for point in (track.start, track.end))
        if not inside or margin < clearance:
            findings.append(_finding("DRC-BOARD-EDGE", DrcSeverity.ERROR, f"track {index} violates copper-to-board-edge clearance", objects=(f"track:{index}",), nets=(track.net,), layers=(track.layer.value,), required_nm=clearance, measured_nm=max(0, round(margin))))
    for index, via in enumerate(board.vias):
        margin = min(_point_segment_distance(via.position, first, second) for first, second in edges) - via.size_nm / 2
        if not _point_in_polygon_or_edge(via.position, board.outline.vertices) or margin < clearance:
            findings.append(_finding("DRC-BOARD-EDGE", DrcSeverity.ERROR, f"via {index} violates copper-to-board-edge clearance", objects=(f"via:{index}",), nets=(via.net,), required_nm=clearance, measured_nm=max(0, round(margin))))
    for pad in _copper_pads(board):
        spine_edges = ((pad.shape.spine[0], pad.shape.spine[0]),) if len(pad.shape.spine) == 1 else (
            ((pad.shape.spine[0], pad.shape.spine[1]),) if len(pad.shape.spine) == 2 else
            tuple(zip(pad.shape.spine, (*pad.shape.spine[1:], pad.shape.spine[0])))
        )
        margin = min(
            _fraction_sqrt_floor(segment_distance_squared(start, end, first, second))
            for start, end in spine_edges for first, second in edges
        ) - pad.shape.radius_nm
        if not all(_point_in_polygon_or_edge(point, board.outline.vertices)
                   for point in pad.shape.spine) or margin < clearance:
            findings.append(_finding("DRC-BOARD-EDGE", DrcSeverity.ERROR, f"{pad.identity} violates copper-to-board-edge clearance", objects=(pad.identity,), nets=(pad.net,), layers=tuple(layer.value for layer in pad.layers), required_nm=clearance, measured_nm=max(0, round(margin))))


def _check_copper_spacing(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    rules = {item.net: item for item in board.net_routing_rules}
    tracks = list(enumerate(board.tracks))
    for position, (left_index, left) in enumerate(tracks):
        for right_index, right in tracks[position + 1 :]:
            if left.net == right.net or left.layer is not right.layer:
                continue
            distance_squared = segment_distance_squared(left.start, left.end, right.start, right.end)
            required_twice = left.width_nm + right.width_nm + 2 * _clearance(board, rules.get(left.net), rules.get(right.net))
            if 4 * distance_squared < required_twice * required_twice:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"tracks {left_index} and {right_index} violate copper spacing", objects=(f"track:{left_index}", f"track:{right_index}"), nets=tuple(sorted((left.net, right.net))), layers=(left.layer.value,), required_nm=(required_twice + 1) // 2, measured_nm=_fraction_sqrt_floor(distance_squared)))
    for track_index, track in tracks:
        for via_index, via in enumerate(board.vias):
            if track.net == via.net or not _via_covers_layer(board, via, track.layer):
                continue
            distance_squared = point_segment_distance_squared(via.position, track.start, track.end)
            required_twice = track.width_nm + via.size_nm + 2 * _clearance(board, rules.get(track.net), rules.get(via.net))
            if 4 * distance_squared < required_twice * required_twice:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"track {track_index} and via {via_index} violate copper spacing", objects=(f"track:{track_index}", f"via:{via_index}"), nets=tuple(sorted((track.net, via.net))), layers=(track.layer.value,), required_nm=(required_twice + 1) // 2, measured_nm=_fraction_sqrt_floor(distance_squared)))
    for left_index, left in enumerate(board.vias):
        for right_index in range(left_index + 1, len(board.vias)):
            right = board.vias[right_index]
            if left.net == right.net or not _via_spans_overlap(board, left, right):
                continue
            distance_squared = (left.position.x_nm - right.position.x_nm) ** 2 + (left.position.y_nm - right.position.y_nm) ** 2
            required_twice = left.size_nm + right.size_nm + 2 * _clearance(board, rules.get(left.net), rules.get(right.net))
            if 4 * distance_squared < required_twice * required_twice:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"vias {left_index} and {right_index} violate copper spacing", objects=(f"via:{left_index}", f"via:{right_index}"), nets=tuple(sorted((left.net, right.net))), required_nm=(required_twice + 1) // 2, measured_nm=isqrt(distance_squared)))
    pads = _copper_pads(board)
    pad_by_id = {pad.identity: pad for pad in pads}
    pad_index = SpatialIndex(
        SpatialItem(pad.identity, pad.shape.bounds.expanded(board.rules.minimum_clearance_nm))
        for pad in pads
    )
    for track_index, track in tracks:
        track_shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        for pad in pads:
            if track.net == pad.net or track.layer not in pad.layers:
                continue
            clearance = _clearance(board, rules.get(track.net), rules.get(pad.net))
            distance_squared = shape_distance_squared(track_shape, pad.shape)
            required = track_shape.radius_nm + pad.shape.radius_nm + clearance
            if distance_squared < required * required:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"track {track_index} and {pad.identity} violate copper spacing", objects=(f"track:{track_index}", pad.identity), nets=tuple(sorted((track.net, pad.net))), layers=(track.layer.value,), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))
    for via_index, via in enumerate(board.vias):
        for pad in pads:
            if via.net == pad.net or not any(_via_covers_layer(board, via, layer) for layer in pad.layers):
                continue
            via_shape = RoundedConvexShape((via.position,), via.size_nm // 2)
            clearance = _clearance(board, rules.get(via.net), rules.get(pad.net))
            distance_squared = shape_distance_squared(via_shape, pad.shape)
            required = via_shape.radius_nm + pad.shape.radius_nm + clearance
            if distance_squared < required * required:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"via {via_index} and {pad.identity} violate copper spacing", objects=(f"via:{via_index}", pad.identity), nets=tuple(sorted((via.net, pad.net))), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))
    visited_pad_pairs: set[tuple[str, str]] = set()
    for left in pads:
        for right_id in pad_index.query(left.shape.bounds.expanded(board.rules.minimum_clearance_nm)):
            if right_id == left.identity:
                continue
            pair = tuple(sorted((left.identity, right_id)))
            if pair in visited_pad_pairs:
                continue
            visited_pad_pairs.add(pair)
            right = pad_by_id[right_id]
            if left.net == right.net or not set(left.layers).intersection(right.layers):
                continue
            clearance = _clearance(board, rules.get(left.net), rules.get(right.net))
            distance_squared = shape_distance_squared(left.shape, right.shape)
            required = left.shape.radius_nm + right.shape.radius_nm + clearance
            if distance_squared < required * required:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"{left.identity} and {right.identity} violate copper spacing", objects=(left.identity, right.identity), nets=tuple(sorted((left.net, right.net))), layers=tuple(sorted(layer.value for layer in set(left.layers).intersection(right.layers))), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))


def _copper_pads(board: PhysicalBoard) -> tuple[_PadCopper, ...]:
    pad_nets = {pad: net.name for net in board.nets for pad in net.pads}
    result: list[_PadCopper] = []
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        for pad in footprint.pads:
            reference = PadReference(placement.reference, pad.number)
            net = pad_nets.get(reference)
            if net is None or pad.kind is PadKind.NON_PLATED_THROUGH_HOLE:
                continue
            if pad.kind is PadKind.SMD:
                layers = (CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
            else:
                layers = tuple(board.stackup.copper_layers)
            position = transformed_pad_position(board, placement, pad.number)
            result.append(_PadCopper(f"pad:{placement.reference}.{pad.number}", net,
                                     position, _pad_shape(position, pad, placement), layers))
    return tuple(result)


def _check_zone_fill_spacing(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    zone_nets = {zone.id: zone.net for zone in board.zones}
    pads = _copper_pads(board)
    rules = {item.net: item for item in board.net_routing_rules}
    for fill in sorted(board.zone_fills, key=lambda item: (item.zone_id, item.layer.value)):
        net = zone_nets[fill.zone_id]
        for polygon_index, polygon in enumerate(fill.polygons):
            objects = f"zone-fill:{fill.zone_id}:{fill.layer.value}:{polygon_index}"
            for track_index, track in enumerate(board.tracks):
                if track.net == net or track.layer is not fill.layer:
                    continue
                shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
                clearance = _clearance(board, rules.get(net), rules.get(track.net))
                if not _shape_clear_of_region(shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates track {track_index} clearance",
                                             objects=(objects, f"track:{track_index}"),
                                             nets=tuple(sorted((net, track.net))), layers=(fill.layer.value,)))
            for via_index, via in enumerate(board.vias):
                if via.net == net or not _via_covers_layer(board, via, fill.layer):
                    continue
                shape = RoundedConvexShape((via.position,), via.size_nm // 2)
                clearance = _clearance(board, rules.get(net), rules.get(via.net))
                if not _shape_clear_of_region(shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates via {via_index} clearance",
                                             objects=(objects, f"via:{via_index}"),
                                             nets=tuple(sorted((net, via.net))), layers=(fill.layer.value,)))
            for pad in pads:
                if pad.net == net or fill.layer not in pad.layers:
                    continue
                clearance = _clearance(board, rules.get(net), rules.get(pad.net))
                if not _shape_clear_of_region(pad.shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates {pad.identity} clearance",
                                             objects=(objects, pad.identity),
                                             nets=tuple(sorted((net, pad.net))), layers=(fill.layer.value,)))


def _shape_clear_of_region(shape: RoundedConvexShape, region: object,
                           clearance_nm: int) -> bool:
    outer = getattr(region, "outer").vertices
    holes = tuple(hole.vertices for hole in getattr(region, "holes"))
    def in_material(point: Point) -> bool:
        return point_in_polygon(point, outer) and not any(point_in_polygon(point, hole) for hole in holes)
    if any(in_material(point) for point in shape.spine):
        return False
    shape_edges = ((shape.spine[0], shape.spine[0]),) if len(shape.spine) == 1 else (
        ((shape.spine[0], shape.spine[1]),) if len(shape.spine) == 2 else
        tuple(zip(shape.spine, (*shape.spine[1:], shape.spine[0])))
    )
    boundaries = (outer, *holes)
    required = shape.radius_nm + clearance_nm
    for boundary in boundaries:
        edges = tuple(zip(boundary, (*boundary[1:], boundary[0])))
        if any(segment_distance_squared(a, b, c, d) < required * required
               for a, b in shape_edges for c, d in edges):
            return False
    return True


def _pad_shape(position: Point, pad: object, placement: object) -> RoundedConvexShape:
    width = getattr(pad, "size").width_nm
    height = getattr(pad, "size").height_nm
    shape = getattr(pad, "shape")
    angle = float(getattr(placement, "rotation_degrees") + getattr(pad, "rotation_degrees"))
    if shape is PadShape.CIRCLE:
        return RoundedConvexShape((position,), min(width, height) // 2)
    if shape is PadShape.OVAL:
        radius = min(width, height) // 2
        half_run = (max(width, height) - 2 * radius) // 2
        local = ((-half_run, 0), (half_run, 0)) if width >= height else ((0, -half_run), (0, half_run))
        return RoundedConvexShape(tuple(_rotate_offset(position, x, y, angle) for x, y in local), radius)
    radius = 0
    if shape is PadShape.ROUNDRECT:
        radius = min(width, height) * getattr(pad, "roundrect_ratio_ppm") // 1_000_000
    half_width, half_height = width // 2 - radius, height // 2 - radius
    spine = tuple(
        _rotate_offset(position, x, y, angle)
        for x, y in ((-half_width, -half_height), (half_width, -half_height),
                     (half_width, half_height), (-half_width, half_height))
    )
    return RoundedConvexShape(spine, radius)


def _rotate_offset(center: Point, x: int, y: int, degrees: float) -> Point:
    angle = radians(degrees)
    return Point(center.x_nm + round(x * cos(angle) - y * sin(angle)),
                 center.y_nm + round(x * sin(angle) + y * cos(angle)))


def _clearance(board: PhysicalBoard, first: NetRoutingRule | None, second: NetRoutingRule | None) -> int:
    return max(board.rules.minimum_clearance_nm, first.clearance_nm if first and first.clearance_nm else 0, second.clearance_nm if second and second.clearance_nm else 0)


def _via_covers_layer(board: PhysicalBoard, via: Via, layer: CopperLayer) -> bool:
    indexes = {item: index for index, item in enumerate(board.stackup.copper_layers)}
    low, high = sorted((indexes[via.from_layer], indexes[via.to_layer]))
    return low <= indexes[layer] <= high


def _via_spans_overlap(board: PhysicalBoard, first: Via, second: Via) -> bool:
    return any(_via_covers_layer(board, first, layer) and _via_covers_layer(board, second, layer) for layer in board.stackup.copper_layers)


def _segment_distance(a: Point, b: Point, c: Point, d: Point) -> float:
    if _segments_intersect(a, b, c, d):
        return 0.0
    return min(_point_segment_distance(point, first, second) for point, first, second in ((a, c, d), (b, c, d), (c, a, b), (d, a, b)))


def _segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool:
    def orientation(p: Point, q: Point, r: Point) -> int:
        value = (q.y_nm - p.y_nm) * (r.x_nm - q.x_nm) - (q.x_nm - p.x_nm) * (r.y_nm - q.y_nm)
        return (value > 0) - (value < 0)

    def on_segment(p: Point, q: Point, r: Point) -> bool:
        return min(p.x_nm, r.x_nm) <= q.x_nm <= max(p.x_nm, r.x_nm) and min(p.y_nm, r.y_nm) <= q.y_nm <= max(p.y_nm, r.y_nm)

    o1, o2, o3, o4 = orientation(a, b, c), orientation(a, b, d), orientation(c, d, a), orientation(c, d, b)
    if o1 != o2 and o3 != o4:
        return True
    return (o1 == 0 and on_segment(a, c, b)) or (o2 == 0 and on_segment(a, d, b)) or (o3 == 0 and on_segment(c, a, d)) or (o4 == 0 and on_segment(c, b, d))


def _point_segment_distance(point: Point, start: Point, end: Point) -> float:
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    if dx == 0 and dy == 0:
        return hypot(point.x_nm - start.x_nm, point.y_nm - start.y_nm)
    fraction = max(0.0, min(1.0, ((point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy) / (dx * dx + dy * dy)))
    return hypot(point.x_nm - (start.x_nm + fraction * dx), point.y_nm - (start.y_nm + fraction * dy))


def _point_in_polygon_or_edge(point: Point, polygon: tuple[Point, ...]) -> bool:
    return point_in_polygon(point, polygon)


def _fraction_sqrt_floor(value: Fraction) -> int:
    numerator = value.numerator
    denominator = value.denominator
    estimate = isqrt(numerator // denominator)
    while (estimate + 1) * (estimate + 1) * denominator <= numerator:
        estimate += 1
    return estimate


def _rules_digest(board: PhysicalBoard, policy: PhysicalDrcPolicy) -> str:
    return _digest({"physical": physical_board_digest(board), "policy": {"require_completed_detailed_route": policy.require_completed_detailed_route, "fail_on_warnings": policy.fail_on_warnings}})


def _report_document(report: PhysicalDrcReport, *, include_token: bool) -> dict[str, object]:
    document: dict[str, object] = {
        "schema": "copperscript-physical-drc/v0.1",
        "decision": report.decision.value,
        "completeness": report.completeness.value,
        "coverage": [{"check": item.check, "status": item.status.value, "required": item.required, "detail": item.detail} for item in report.coverage],
        "findings": [{"fingerprint": item.fingerprint, "code": item.code, "severity": item.severity.value, "message": item.message, "objects": list(item.objects), "nets": list(item.nets), "layers": list(item.layers), "required_nm": item.required_nm, "measured_nm": item.measured_nm, "disposition": item.disposition.value, "waiver_reason": item.waiver_reason} for item in report.findings],
    }
    if include_token:
        document["token"] = {"schema": report.token.schema, "board_digest": report.token.board_digest, "rules_digest": report.token.rules_digest, "report_digest": report.token.report_digest, "decision": report.token.decision.value, "completeness": report.token.completeness.value, "token_digest": report.token.token_digest}
    return document


def _finding(code: str, severity: DrcSeverity, message: str, **kwargs: object) -> DrcFinding:
    return DrcFinding(code, severity, message, **kwargs)  # type: ignore[arg-type]


def _finding_key(item: DrcFinding) -> tuple[object, ...]:
    return (item.code, item.objects, item.nets, item.layers, item.message)


def _track_identity(item: TrackSegment) -> tuple[object, ...]:
    endpoints = sorted(((item.start.x_nm, item.start.y_nm), (item.end.x_nm, item.end.y_nm)))
    return (item.net, item.layer.value, endpoints[0], endpoints[1], item.width_nm)


def _via_identity(item: Via) -> tuple[object, ...]:
    return (item.net, item.position.x_nm, item.position.y_nm, item.from_layer.value, item.to_layer.value, item.size_nm, item.drill_nm, item.technology)


def _routing_rule_document(item: NetRoutingRule) -> tuple[object, ...]:
    return (item.net, item.kind.value, item.priority, item.width_nm, item.clearance_nm,
            tuple(layer.value for layer in item.allowed_layers), item.max_vias,
            item.max_length_nm, item.differential_partner, item.pair_gap_nm,
            item.max_skew_nm, item.topology, item.target_impedance_ohms,
            item.maximum_uncoupled_length_nm, item.maximum_stub_length_nm,
            item.tuning_amplitude_limit_nm, item.require_return_vias,
            item.return_via_net, item.maximum_return_via_distance_nm,
            item.impedance_evidence_digest)


def _footprint_document(name: str, footprint: object) -> tuple[object, ...]:
    pads = getattr(footprint, "pads")
    return (
        name,
        getattr(footprint, "source_library_id"),
        (getattr(footprint, "body_size").width_nm, getattr(footprint, "body_size").height_nm),
        tuple(
            (
                pad.number,
                pad.position.x_nm,
                pad.position.y_nm,
                pad.size.width_nm,
                pad.size.height_nm,
                pad.kind.value,
                pad.shape.value,
                str(pad.rotation_degrees),
                None if pad.drill is None else (pad.drill.width_nm, pad.drill.height_nm),
                pad.roundrect_ratio_ppm,
                pad.has_solder_mask,
                pad.has_solder_paste,
            )
            for pad in pads
        ),
        tuple(repr(item) for item in getattr(footprint, "graphics")),
        tuple((point.x_nm, point.y_nm) for point in getattr(footprint, "courtyard")),
        tuple(sorted(getattr(footprint, "metadata").items())),
        getattr(footprint, "height_nm"),
    )


def _digest(document: object) -> str:
    return sha256(json.dumps(document, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

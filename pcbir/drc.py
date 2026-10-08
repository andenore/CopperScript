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
    ComponentHoleClearance,
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PadReference,
    PadShape,
    PhysicalBoard,
    Point,
    TrackSegment,
    TuningStyle,
    Via,
)
from .placement import resolved_copper_keepouts, transformed_local_point, transformed_pad_position
from .geometry import (
    RoundedConvexShape,
    SpatialIndex,
    SpatialItem,
    point_in_polygon,
    point_segment_distance_at_least,
    point_segment_distance_squared,
    segment_distance_at_least,
    segment_distance_squared,
    shape_distance_squared,
    shapes_clear,
)
from .placement import placement_solution_is_legal
from .breakout import BreakoutRegions
from .copper_connectivity import (CopperContact, PhysicalCopperConnectivity,
                                  copper_contact_roots, via_copper_contact)


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
class DrcBreakoutRelaxation:
    """A width, clearance or pair-gap check passed only under breakout values.

    ``required_nm`` is the normal value the copper does not meet and
    ``relaxed_nm`` the breakout value it does meet (plan R1). Like
    ``measured_nm`` they are track widths or copper-to-copper spacings; a zone
    fill spacing is not measured. ``regions`` names, for each net whose
    breakout value applied, the terminal land whose region holds its copper
    and the net's breakout length.
    """

    check: str  # "track_width", "clearance" or "pair_gap"
    objects: tuple[str, ...]
    nets: tuple[str, ...]
    layers: tuple[str, ...]
    required_nm: int
    relaxed_nm: int
    measured_nm: int | None
    regions: tuple[tuple[str, str, int], ...]


@dataclass(frozen=True, slots=True)
class DrcHoleClearanceRelaxation:
    """A pad-to-own-hole check passed only under a component-scoped rule.

    ``objects`` are the component's pad and its own non-plated hole.
    ``required_nm`` is the board ``minimum_hole_clearance`` the pad does not
    meet, ``relaxed_nm`` the scoped ``hole_clearance`` it does meet and
    ``measured_nm`` the pad-copper-to-drill spacing.
    """

    component: str
    objects: tuple[str, ...]
    nets: tuple[str, ...]
    required_nm: int
    relaxed_nm: int
    measured_nm: int
    reason: str


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
    # Checks that passed only because breakout-region values applied.
    breakout_relaxations: tuple[DrcBreakoutRelaxation, ...] = ()
    # Pad-to-own-hole checks that passed only under a component hole_clearance.
    hole_clearance_relaxations: tuple[DrcHoleClearanceRelaxation, ...] = ()

    def to_json(self) -> str:
        return json.dumps(_report_document(self, include_token=True), indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True, slots=True)
class _PadCopper:
    identity: str
    net: str
    position: Point
    shape: RoundedConvexShape
    layers: tuple[CopperLayer, ...]
    clearance_nm: int = 0
    component: str = ""


def placement_copper_findings(board: PhysicalBoard) -> tuple[DrcFinding, ...]:
    """Geometric copper/drill checks for editing an unrouted placement.

    Connectivity and completed-route gates belong to routing signoff, not an
    editor save. Keepout coverage failures are still reported, never waived.
    """
    findings: list[DrcFinding] = []
    _check_board_edge(board, findings)
    _check_copper_spacing(board, findings)
    _check_non_plated_hole_clearance(board, findings)
    _check_via_hole_clearance(board, findings)
    _check_drill_spacing(board, findings)
    _check_copper_keepouts(board, findings)
    _check_zone_routing_reservations(board, findings)
    return tuple(findings)


def run_physical_drc(
    board: PhysicalBoard,
    waivers: Iterable[DrcWaiver] = (),
    policy: PhysicalDrcPolicy | None = None,
) -> PhysicalDrcReport:
    """Check exact physical IR and bind the result to a canonical geometry digest."""

    policy = policy or PhysicalDrcPolicy()
    findings: list[DrcFinding] = []
    coverage: list[DrcCoverage] = []
    relaxations: list[DrcBreakoutRelaxation] = []

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
    _check_track_rules(board, findings, relaxations)
    coverage.append(DrcCoverage("track_width_and_budgets", DrcCoverageStatus.EXECUTED, True))
    _check_differential_rules(board, findings)
    coverage.append(DrcCoverage("differential_geometry", DrcCoverageStatus.EXECUTED, True))
    _check_length_match(board, findings)
    coverage.append(DrcCoverage(
        "length_match",
        DrcCoverageStatus.EXECUTED if board.match_groups else DrcCoverageStatus.NOT_APPLICABLE,
        True,
        "group skew is checked once every member net is connected" if board.match_groups else "",
    ))
    _check_vias(board, findings)
    coverage.append(DrcCoverage("via_geometry_and_budgets", DrcCoverageStatus.EXECUTED, True))
    _check_board_edge(board, findings)
    coverage.append(DrcCoverage("copper_to_board_edge", DrcCoverageStatus.EXECUTED, True))
    _check_copper_spacing(board, findings, relaxations)
    coverage.append(DrcCoverage("shorts_and_clearance", DrcCoverageStatus.EXECUTED, True))
    if any(rule.breakout_length_nm is not None for rule in board.net_routing_rules):
        coverage.append(DrcCoverage(
            "breakout_regions", DrcCoverageStatus.EXECUTED, True,
            "breakout width, pair gap and clearance apply only to copper within breakout_length "
            "of a terminal land; checks they relaxed are listed in breakout_relaxations",
        ))
    hole_relaxations: list[DrcHoleClearanceRelaxation] = []
    _check_non_plated_hole_clearance(board, findings, hole_relaxations)
    coverage.append(DrcCoverage("non_plated_hole_clearance", DrcCoverageStatus.EXECUTED, True))
    if board.component_hole_clearances:
        coverage.append(DrcCoverage(
            "component_hole_clearance", DrcCoverageStatus.EXECUTED, True,
            "scoped hole_clearance applies only between a component's own pads and its own "
            "non-plated holes; checks it relaxed are listed in hole_clearance_relaxations",
        ))
    _check_via_hole_clearance(board, findings)
    coverage.append(DrcCoverage("via_hole_to_copper", DrcCoverageStatus.EXECUTED, True))
    _check_drill_spacing(board, findings)
    coverage.append(DrcCoverage("drill_to_drill_spacing", DrcCoverageStatus.EXECUTED, True))
    keepout_covered = _check_copper_keepouts(board, findings)
    _check_zone_routing_reservations(board, findings)
    if any(zone.reserve_routing for zone in board.zones):
        coverage.append(DrcCoverage("zone_routing_reservations", DrcCoverageStatus.EXECUTED, True,
                                    "net-aware reserved polygons checked against all tracks, including macro copper"))
    coverage.append(DrcCoverage(
        "copper_keepouts",
        DrcCoverageStatus.EXECUTED if keepout_covered else DrcCoverageStatus.FAILED,
        True,
        "polygonal keepout checks for tracks, vias, and component pads",
    ))

    if board.zone_fills:
        _check_zone_fill_spacing(board, findings, relaxations)
        coverage.append(DrcCoverage("copper_zones", DrcCoverageStatus.EXECUTED, False,
                                    "checked content-bound normalized fill polygons"))
        if board.outline.circular_boundary or board.outline.boundary_path or board.outline.cutouts or board.mechanical_holes or board.mechanical_slots:
            findings.append(_finding(
                "DRC-MECHANICAL-FILL-UNSUPPORTED", DrcSeverity.ERROR,
                "filled-zone material coverage for curved outlines/cutouts/mechanical holes is not yet qualified",
            ))
            coverage.append(DrcCoverage(
                "mechanical_zone_material", DrcCoverageStatus.UNSUPPORTED, True,
                "requires hole-aware filled-region containment; boundary-only checks are insufficient",
            ))
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
        tuple(sorted(relaxations, key=lambda item: (item.check, item.objects, item.nets, item.layers))),
        tuple(sorted(hole_relaxations, key=lambda item: (item.component, item.objects))),
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
    from .mechanical_references import reference_scene

    return _digest(
        {
            "name": board.name,
            "outline": [(p.x_nm, p.y_nm) for p in board.outline.vertices],
            "circular_boundary": repr(board.outline.circular_boundary),
            "boundary_path": repr(board.outline.boundary_path),
            "cutouts": [(c.id, [(p.x_nm, p.y_nm) for p in c.vertices]) for c in board.outline.cutouts],
            "mechanical_holes": [repr(h) for h in sorted(board.mechanical_holes, key=lambda h: h.id)],
            "mechanical_slots": [repr(s) for s in sorted(board.mechanical_slots,key=lambda s:s.id)],
            "mechanical_references": [reference_scene(r)
                                      for r in sorted(board.mechanical_references,key=lambda r:r.id)],
            "datums": [repr(d) for d in sorted(board.datums, key=lambda d:d.id)],
            "boundary_edges": [repr(e) for e in sorted(board.boundary_edges, key=lambda e:e.id)],
            "attachments": [repr(a) for a in sorted(board.attachments, key=lambda a:a.id)],
            "body_overhangs": [repr(a) for a in sorted(board.body_overhangs,key=lambda a:a.id)],
            "component_heights": [repr(a) for a in sorted(board.component_heights,key=lambda a:a.id)],
            "assembly_envelopes": [repr(a) for a in sorted(board.assembly_envelopes,key=lambda a:a.id)],
            "assembly_access": [repr(a) for a in sorted(board.assembly_access,key=lambda a:a.id)],
            "stackup": {
                "layers": [layer.value for layer in board.stackup.copper_layers],
                "thickness_nm": board.stackup.thickness_nm,
                "physical_layers": [repr(layer) for layer in board.stackup.physical_layers],
                "via_technologies": [repr(item) for item in board.stackup.via_technologies],
            },
            "rules": {
                "minimum_slot_width": board.rules.minimum_slot_width_nm,
                "clearance": board.rules.minimum_clearance_nm,
                "hole_clearance": board.rules.minimum_hole_clearance_nm,
                "minimum_track_width": board.rules.minimum_track_width_nm,
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
            **({"match_groups": [(group.id, group.nets, group.max_skew_nm)
                                 for group in sorted(board.match_groups, key=lambda item: item.id)]}
               if board.match_groups else {}),
            # Present only when declared, so other boards' digests are unchanged.
            **({"component_hole_clearances": [
                (rule.reference, rule.clearance_nm, rule.reason)
                for rule in sorted(board.component_hole_clearances, key=lambda item: item.reference)]}
               if board.component_hole_clearances else {}),
            "regions": [repr(item) for item in sorted(board.regions, key=lambda item: item.name)],
            "keepouts": [repr(item) for item in sorted(board.keepouts, key=lambda item: item.name)],
            "placement_rules": [repr(item) for item in sorted(board.placement_rules, key=lambda item: item.reference)],
            "relative_rules": [repr(item) for item in board.relative_rules],
            "placement_groups": [repr(item) for item in sorted(board.placement_groups, key=lambda item: item.name)],
            "rigid_clusters": [repr(item) for item in sorted(board.rigid_clusters, key=lambda item: item.name)],
            "hard_macros": [repr(item) for item in sorted(board.hard_macros, key=lambda item: item.cluster)],
            "materialized_macros": board.materialized_macros,
            "zones": [repr(item) for item in sorted(board.zones, key=lambda item: item.id)],
            **({"zone_routing_reservations": sorted(zone.id for zone in board.zones if zone.reserve_routing)}
               if any(zone.reserve_routing for zone in board.zones) else {}),
            "copper_keepouts": [repr(item) for item in sorted(board.copper_keepouts, key=lambda item: item.id)],
            "zone_fills": [repr(item) for item in sorted(board.zone_fills, key=lambda item: (item.zone_id, item.layer.value))],
            "metadata": tuple(sorted(board.metadata.items())),
        }
    )


def explicit_copper_connectivity(
    board: PhysicalBoard, *, only_nets: frozenset[str] | None = None,
    include_internal_connections: bool = True,
    tracks: tuple[TrackSegment, ...] | None = None, vias: tuple[Via, ...] | None = None,
) -> PhysicalCopperConnectivity:
    """Build the shared exact graph used by native DRC and land closure.

    Zone outlines/fill claims do not join this explicit-copper graph. A net
    filter saves work for local stitching without changing contact semantics.
    Declared installed-component groups join roots by default; disable them
    for bare-board continuity. Internal edges never add fabrication geometry.
    ``tracks``/``vias`` evaluate candidate copper against the board's pads
    without building (and revalidating) a whole replacement board.
    """
    tracks = board.tracks if tracks is None else tracks
    vias = board.vias if vias is None else vias
    objects: list[CopperContact] = []
    pad_nodes: dict[PadReference, list[str]] = {}
    internal_connections = []
    # Include every physical land, even when a logical pin has repeated numbers.
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        assigned = {pad.pad: net.name for net in board.nets
                    if only_nets is None or net.name in only_nets for pad in net.pads
                    if pad.component == placement.reference}
        for index, pad in enumerate(footprint.pads):
            net = assigned.get(pad.number)
            if net is None or pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                continue
            position = transformed_local_point(placement, pad.position)
            layers = ((CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
                      if pad.kind is PadKind.SMD else board.stackup.copper_layers)
            identity = f"pad:{placement.reference}.{pad.number}:{index}"
            drill = None
            if pad.kind is PadKind.THROUGH_HOLE and pad.drill is not None:
                drill_pad = replace(pad, size=pad.drill, kind=PadKind.SMD, drill=None, shape=(
                    PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm else PadShape.OVAL))
                drill = placed_pad_shape(position, drill_pad, placement)
            objects.append(CopperContact(identity, net, tuple(layers),
                                         placed_pad_shape(position, pad, placement), drill))
            pad_nodes.setdefault(PadReference(placement.reference, pad.number), []).append(identity)
        if include_internal_connections:
            for group in footprint.internal_pad_groups:
                nodes = tuple(node for number in group.numbers for node in
                              pad_nodes.get(PadReference(placement.reference, number), ()))
                if len(nodes) > 1:
                    internal_connections.append(nodes)
    objects.extend(CopperContact(f"track:{index}", track.net, (track.layer,),
                                RoundedConvexShape((track.start, track.end), track.width_nm // 2))
                   for index, track in enumerate(tracks)
                   if only_nets is None or track.net in only_nets)
    for index, via in enumerate(vias):
        if only_nets is not None and via.net not in only_nets:
            continue
        objects.append(via_copper_contact(via, board.stackup.copper_layers, identity=f"via:{index}"))
    groups = tuple(internal_connections)
    return PhysicalCopperConnectivity(copper_contact_roots(tuple(objects), groups),
                                      {pad: tuple(nodes) for pad, nodes in pad_nodes.items()}, groups)


def _check_connectivity(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    graph = explicit_copper_connectivity(board)
    for net in sorted(board.nets, key=lambda item: item.name):
        if not graph.net_connected(net):
            findings.append(
                _finding(
                    "DRC-OPEN-NET",
                    DrcSeverity.ERROR,
                    f"net {net.name!r} is not connected by exact copper and declared internal connections",
                    nets=(net.name,),
                    objects=tuple(f"pad:{item.component}.{item.pad}" for item in sorted(net.pads)),
                )
            )


def _check_track_rules(board: PhysicalBoard, findings: list[DrcFinding],
                       relaxations: list[DrcBreakoutRelaxation] | None = None) -> None:
    rules = {item.net: item for item in board.net_routing_rules}
    spacing = (_BreakoutSpacing(board, relaxations)
               if any(rule.breakout_width_nm is not None for rule in rules.values()) else None)
    for index, track in enumerate(board.tracks):
        rule = rules.get(track.net)
        required = max(
            board.rules.minimum_track_width_nm,
            rule.width_nm if rule and rule.width_nm is not None else 0,
        )
        # Inside a breakout region the breakout width replaces the profile width.
        relaxed, land = required, None
        if spacing is not None and rule is not None and rule.breakout_width_nm is not None:
            land = spacing.land(track.net, (track.start, track.end))
            if land is not None:
                relaxed = min(required, max(board.rules.minimum_track_width_nm, rule.breakout_width_nm))
        if track.width_nm < relaxed:
            findings.append(_finding("DRC-TRACK-WIDTH", DrcSeverity.ERROR, f"track {index} on {track.net!r} is below its minimum or routing profile width", objects=(f"track:{index}",), nets=(track.net,), layers=(track.layer.value,), required_nm=relaxed, measured_nm=track.width_nm))
        elif track.width_nm < required and spacing is not None:
            spacing.record("track_width", (f"track:{index}",), ((track.net, land),),
                           (track.layer.value,), required, relaxed, track.width_nm)
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


def _check_length_match(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    """Hard group-skew check for fully routed ``length_match`` groups."""
    if not board.match_groups:
        return
    from .signal_integrity import verify_match_groups

    for result in verify_match_groups(board):
        if result.status != "fail":
            continue
        members = ", ".join(f"{member.net} {member.length_nm} nm" for member in result.members)
        findings.append(_finding(
            "DRC-LENGTH-MATCH", DrcSeverity.ERROR,
            f"length-match group {result.id!r} skew is {result.skew_nm} nm ({members})",
            objects=(f"match_group:{result.id}",),
            nets=tuple(sorted(member.net for member in result.members)),
            required_nm=result.max_skew_nm, measured_nm=result.skew_nm,
        ))


def _check_board_edge(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    clearance = board.rules.minimum_clearance_nm
    from .mechanical import ring_edges, shape_in_outline
    if board.outline.circular_boundary is not None or board.outline.boundary_path:
        # Exact disk containment; do not replace it with the inscribed grid ring.
        objects = [(f"track:{i}", t.net, RoundedConvexShape((t.start, t.end), (t.width_nm + 1) // 2))
                   for i, t in enumerate(board.tracks)]
        objects.extend((f"via:{i}", v.net, RoundedConvexShape((v.position,), (v.size_nm + 1) // 2))
                       for i, v in enumerate(board.vias))
        objects.extend((p.identity, p.net, p.shape) for p in _copper_pads(board))
        for identity, net, shape in objects:
            if not shape_in_outline(shape, board.outline, clearance):
                findings.append(_finding(
                    "DRC-BOARD-EDGE", DrcSeverity.ERROR,
                    f"{identity} violates circular board/cutout clearance",
                    objects=(identity,), nets=(net,), required_nm=clearance,
                ))
        return
    edges = (*ring_edges(board.outline.vertices),
             *(edge for cutout in board.outline.cutouts for edge in ring_edges(cutout.vertices)))
    for index, track in enumerate(board.tracks):
        margin = min(
            _fraction_sqrt_floor(segment_distance_squared(track.start, track.end, first, second))
            for first, second in edges
        ) - track.width_nm / 2
        inside = shape_in_outline(RoundedConvexShape((track.start, track.end), (track.width_nm + 1) // 2), board.outline, clearance)
        if not inside or margin < clearance:
            findings.append(_finding("DRC-BOARD-EDGE", DrcSeverity.ERROR, f"track {index} violates copper-to-board-edge clearance", objects=(f"track:{index}",), nets=(track.net,), layers=(track.layer.value,), required_nm=clearance, measured_nm=max(0, round(margin))))
    for index, via in enumerate(board.vias):
        margin = min(_point_segment_distance(via.position, first, second) for first, second in edges) - via.size_nm / 2
        if not shape_in_outline(RoundedConvexShape((via.position,), (via.size_nm + 1) // 2), board.outline, clearance) or margin < clearance:
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
        if not shape_in_outline(pad.shape, board.outline, clearance) or margin < clearance:
            findings.append(_finding("DRC-BOARD-EDGE", DrcSeverity.ERROR, f"{pad.identity} violates copper-to-board-edge clearance", objects=(pad.identity,), nets=(pad.net,), layers=tuple(layer.value for layer in pad.layers), required_nm=clearance, measured_nm=max(0, round(margin))))


def _check_copper_spacing(board: PhysicalBoard, findings: list[DrcFinding],
                          relaxations: list[DrcBreakoutRelaxation] | None = None) -> None:
    spacing = _BreakoutSpacing(board, relaxations)
    tracks = list(enumerate(board.tracks))
    track_lands = [spacing.land(track.net, (track.start, track.end)) for track in board.tracks]
    via_lands = [spacing.land(via.net, (via.position,)) for via in board.vias]
    for position, (left_index, left) in enumerate(tracks):
        for right_index, right in tracks[position + 1 :]:
            if left.net == right.net or left.layer is not right.layer:
                continue
            clearance, normal = spacing.between(left.net, track_lands[left_index], right.net, track_lands[right_index])
            required_twice = left.width_nm + right.width_nm + 2 * clearance
            if not segment_distance_at_least(left.start, left.end, right.start, right.end, required_twice, denominator=2):
                distance_squared = segment_distance_squared(left.start, left.end, right.start, right.end)
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"tracks {left_index} and {right_index} violate {spacing.subject(left.net, right.net)}", objects=(f"track:{left_index}", f"track:{right_index}"), nets=tuple(sorted((left.net, right.net))), layers=(left.layer.value,), required_nm=(required_twice + 1) // 2, measured_nm=_fraction_sqrt_floor(distance_squared)))
            elif clearance < normal:
                spacing.note(segment_distance_squared(left.start, left.end, right.start, right.end),
                             left.width_nm + right.width_nm, normal, clearance,
                             (f"track:{left_index}", f"track:{right_index}"),
                             ((left.net, track_lands[left_index]), (right.net, track_lands[right_index])),
                             (left.layer.value,))
    for track_index, track in tracks:
        for via_index, via in enumerate(board.vias):
            if track.net == via.net or not _via_covers_layer(board, via, track.layer):
                continue
            clearance, normal = spacing.between(track.net, track_lands[track_index], via.net, via_lands[via_index])
            required_twice = track.width_nm + via.size_nm + 2 * clearance
            if not point_segment_distance_at_least(via.position, track.start, track.end, required_twice, denominator=2):
                distance_squared = point_segment_distance_squared(via.position, track.start, track.end)
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"track {track_index} and via {via_index} violate {spacing.subject(track.net, via.net)}", objects=(f"track:{track_index}", f"via:{via_index}"), nets=tuple(sorted((track.net, via.net))), layers=(track.layer.value,), required_nm=(required_twice + 1) // 2, measured_nm=_fraction_sqrt_floor(distance_squared)))
            elif clearance < normal:
                spacing.note(point_segment_distance_squared(via.position, track.start, track.end),
                             track.width_nm + via.size_nm, normal, clearance,
                             (f"track:{track_index}", f"via:{via_index}"),
                             ((track.net, track_lands[track_index]), (via.net, via_lands[via_index])),
                             (track.layer.value,))
    for left_index, left in enumerate(board.vias):
        for right_index in range(left_index + 1, len(board.vias)):
            right = board.vias[right_index]
            if left.net == right.net or not _via_spans_overlap(board, left, right):
                continue
            distance_squared = (left.position.x_nm - right.position.x_nm) ** 2 + (left.position.y_nm - right.position.y_nm) ** 2
            clearance, normal = spacing.between(left.net, via_lands[left_index], right.net, via_lands[right_index])
            required_twice = left.size_nm + right.size_nm + 2 * clearance
            if 4 * distance_squared < required_twice * required_twice:
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"vias {left_index} and {right_index} violate {spacing.subject(left.net, right.net)}", objects=(f"via:{left_index}", f"via:{right_index}"), nets=tuple(sorted((left.net, right.net))), required_nm=(required_twice + 1) // 2, measured_nm=isqrt(distance_squared)))
            elif clearance < normal:
                spacing.note(distance_squared, left.size_nm + right.size_nm, normal, clearance,
                             (f"via:{left_index}", f"via:{right_index}"),
                             ((left.net, via_lands[left_index]), (right.net, via_lands[right_index])), ())
    pads = _copper_pads(board)
    pad_by_id = {pad.identity: pad for pad in pads}
    pad_lands = {pad.identity: spacing.land(pad.net, pad.shape.spine) for pad in pads}

    def pad_reach(pad: _PadCopper) -> int:
        reach = max(board.rules.minimum_clearance_nm, pad.clearance_nm)
        rule = spacing.rules.get(pad.net)
        if rule is not None and pad.net in spacing.regions.declared:
            # Compare a breakout net's lands at their normal spacing too, so
            # the region that lets a pin field pass is recorded.
            reach = max(reach, rule.clearance_nm or 0, rule.pair_gap_nm or 0)
        return reach

    pad_index = SpatialIndex(
        SpatialItem(pad.identity, pad.shape.bounds.expanded(pad_reach(pad)))
        for pad in pads
    )
    for track_index, track in tracks:
        track_shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        for pad in pads:
            if track.net == pad.net or track.layer not in pad.layers:
                continue
            relaxed, normal = spacing.between(track.net, track_lands[track_index], pad.net, pad_lands[pad.identity])
            clearance, normal = max(pad.clearance_nm, relaxed), max(pad.clearance_nm, normal)
            required = track_shape.radius_nm + pad.shape.radius_nm + clearance
            if not shapes_clear(track_shape, pad.shape, clearance):
                distance_squared = shape_distance_squared(track_shape, pad.shape)
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"track {track_index} and {pad.identity} violate {spacing.subject(track.net, pad.net)}", objects=(f"track:{track_index}", pad.identity), nets=tuple(sorted((track.net, pad.net))), layers=(track.layer.value,), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))
            elif clearance < normal:
                spacing.note(shape_distance_squared(track_shape, pad.shape),
                             2 * (track_shape.radius_nm + pad.shape.radius_nm), normal, clearance,
                             (f"track:{track_index}", pad.identity),
                             ((track.net, track_lands[track_index]), (pad.net, pad_lands[pad.identity])),
                             (track.layer.value,))
    for via_index, via in enumerate(board.vias):
        via_shape = RoundedConvexShape((via.position,), via.size_nm // 2)
        for pad in pads:
            if not any(_via_covers_layer(board, via, layer) for layer in pad.layers):
                continue
            if via.net == pad.net:
                if via.finish != "filled-capped" and not shapes_clear(via_shape, pad.shape, 1):
                    findings.append(_finding(
                        "DRC-VIA-PAD-OVERLAP", DrcSeverity.ERROR,
                        f"ordinary via {via_index} overlaps {pad.identity}; via-in-pad requires explicit qualification",
                        objects=(f"via:{via_index}", pad.identity), nets=(via.net,),
                        layers=tuple(layer.value for layer in pad.layers
                                     if _via_covers_layer(board, via, layer)),
                    ))
                continue
            relaxed, normal = spacing.between(via.net, via_lands[via_index], pad.net, pad_lands[pad.identity])
            clearance, normal = max(pad.clearance_nm, relaxed), max(pad.clearance_nm, normal)
            required = via_shape.radius_nm + pad.shape.radius_nm + clearance
            if not shapes_clear(via_shape, pad.shape, clearance):
                distance_squared = shape_distance_squared(via_shape, pad.shape)
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"via {via_index} and {pad.identity} violate {spacing.subject(via.net, pad.net)}", objects=(f"via:{via_index}", pad.identity), nets=tuple(sorted((via.net, pad.net))), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))
            elif clearance < normal:
                spacing.note(shape_distance_squared(via_shape, pad.shape),
                             2 * (via_shape.radius_nm + pad.shape.radius_nm), normal, clearance,
                             (f"via:{via_index}", pad.identity),
                             ((via.net, via_lands[via_index]), (pad.net, pad_lands[pad.identity])), ())
    visited_pad_pairs: set[tuple[str, str]] = set()
    for left in pads:
        for right_id in pad_index.query(left.shape.bounds.expanded(pad_reach(left))):
            if right_id == left.identity:
                continue
            pair = tuple(sorted((left.identity, right_id)))
            if pair in visited_pad_pairs:
                continue
            visited_pad_pairs.add(pair)
            right = pad_by_id[right_id]
            if left.net == right.net or not set(left.layers).intersection(right.layers):
                continue
            relaxed, normal = spacing.between(left.net, pad_lands[left.identity], right.net, pad_lands[right.identity])
            clearance = max(left.clearance_nm, right.clearance_nm, relaxed)
            normal = max(left.clearance_nm, right.clearance_nm, normal)
            required = left.shape.radius_nm + right.shape.radius_nm + clearance
            if not shapes_clear(left.shape, right.shape, clearance):
                distance_squared = shape_distance_squared(left.shape, right.shape)
                code = "DRC-SHORT" if distance_squared == 0 else "DRC-CLEARANCE"
                findings.append(_finding(code, DrcSeverity.ERROR, f"{left.identity} and {right.identity} violate {spacing.subject(left.net, right.net)}", objects=(left.identity, right.identity), nets=tuple(sorted((left.net, right.net))), layers=tuple(sorted(layer.value for layer in set(left.layers).intersection(right.layers))), required_nm=required, measured_nm=_fraction_sqrt_floor(distance_squared)))
            elif clearance < normal:
                spacing.note(shape_distance_squared(left.shape, right.shape),
                             2 * (left.shape.radius_nm + right.shape.radius_nm), normal, clearance,
                             (left.identity, right.identity),
                             ((left.net, pad_lands[left.identity]), (right.net, pad_lands[right.identity])),
                             tuple(sorted(layer.value for layer in set(left.layers).intersection(right.layers))))


def _check_zone_routing_reservations(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    from .zone_geometry import ZoneRoutingReservations
    reservations = ZoneRoutingReservations(board)
    if not reservations.zones:
        return
    for index, track in enumerate(board.tracks):
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        for zone in reservations.blocking_zones(track.net, shape, track.layer):
            findings.append(_finding(
                "DRC-ZONE-RESERVATION", DrcSeverity.ERROR,
                f"track {index} enters routing reservation {zone.id} for {zone.net}",
                objects=(zone.id, f"track:{index}"), nets=(zone.net, track.net),
            ))


def _check_copper_keepouts(
    board: PhysicalBoard, findings: list[DrcFinding]
) -> bool:
    """Check all placed and board-level copper keepouts against actual copper."""

    covered = True
    local_owner = {
        f"{placement.reference}/{local.id}": placement.reference
        for placement in board.placements
        for local in board.footprints[placement.footprint].keepouts
    }
    for keepout in sorted(resolved_copper_keepouts(board), key=lambda item: item.id):
        if keepout.outline.holes:
            covered = False
            findings.append(_finding(
                "DRC-KEEPOUT-UNSUPPORTED", DrcSeverity.ERROR,
                f"keepout {keepout.id} has polygon holes that native DRC cannot validate",
                objects=(keepout.id,),
            ))
            continue
        region = RoundedConvexShape(keepout.outline.outer.vertices)
        for index, track in enumerate(board.tracks):
            if not keepout.block_tracks or track.layer not in keepout.layers:
                continue
            copper = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
            if not shapes_clear(region, copper):
                findings.append(_finding(
                    "DRC-COPPER-KEEPOUT", DrcSeverity.ERROR,
                    f"track {index} enters copper keepout {keepout.id}",
                    objects=(keepout.id, f"track:{index}"),
                    nets=(track.net,), layers=(track.layer.value,),
                ))
        for index, via in enumerate(board.vias):
            if not keepout.block_vias or not any(
                _via_covers_layer(board, via, layer) for layer in keepout.layers
            ):
                continue
            copper = RoundedConvexShape((via.position,), via.size_nm // 2)
            if not shapes_clear(region, copper):
                findings.append(_finding(
                    "DRC-COPPER-KEEPOUT", DrcSeverity.ERROR,
                    f"via {index} enters copper keepout {keepout.id}",
                    objects=(keepout.id, f"via:{index}"), nets=(via.net,),
                ))
        if keepout.block_pads:
            for placement in board.placements:
                if placement.reference == local_owner.get(keepout.id):
                    # A footprint's own local rule areas protect it from
                    # foreign pads, tracks and vias, not its own pads.
                    continue
                footprint = board.footprints[placement.footprint]
                for pad_index, pad in enumerate(footprint.pads):
                    if pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                        continue
                    pad_layers = (
                        (CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
                        if pad.kind is PadKind.SMD else tuple(board.stackup.copper_layers)
                    )
                    if not set(pad_layers).intersection(keepout.layers):
                        continue
                    point = transformed_local_point(placement, pad.position)
                    copper = placed_pad_shape(point, pad, placement)
                    if not shapes_clear(region, copper):
                        findings.append(_finding(
                            "DRC-COPPER-KEEPOUT", DrcSeverity.ERROR,
                            f"pad {placement.reference}.{pad.number} enters copper keepout {keepout.id}",
                            objects=(keepout.id, f"pad:{placement.reference}.{pad.number}:{pad_index}"),
                        ))
    return covered


def _copper_pads(board: PhysicalBoard) -> tuple[_PadCopper, ...]:
    pad_nets = {pad: net.name for net in board.nets for pad in net.pads}
    result: list[_PadCopper] = []
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        for pad_index, pad in enumerate(footprint.pads):
            reference = PadReference(placement.reference, pad.number)
            if pad.kind in {
                PadKind.NON_PLATED_THROUGH_HOLE,
                PadKind.APERTURE,
            }:
                continue
            net = pad_nets.get(
                reference, f"<unconnected:{placement.reference}.{pad.number}:{pad_index}>"
            )
            if pad.kind is PadKind.SMD:
                layers = (CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
            else:
                layers = tuple(board.stackup.copper_layers)
            position = transformed_local_point(placement, pad.position)
            result.append(_PadCopper(f"pad:{placement.reference}.{pad.number}:{pad_index}", net,
                                     position, placed_pad_shape(position, pad, placement), layers,
                                     footprint.clearance_nm or 0, placement.reference))
    return tuple(result)


def non_plated_holes(board: PhysicalBoard) -> tuple[tuple[str, RoundedConvexShape], ...]:
    """Return placed drill envelopes, including unnumbered mechanical holes."""

    holes: list[tuple[str, RoundedConvexShape]] = []
    from .mechanical import hole_shape,slot_shape
    holes.extend((f"mechanical-hole:{hole.id}", hole_shape(hole)) for hole in board.mechanical_holes)
    holes.extend((f"mechanical-slot:{slot.id}",slot_shape(slot)) for slot in board.mechanical_slots)
    for placement in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[placement.footprint]
        for index, pad in enumerate(footprint.pads):
            if pad.kind is not PadKind.NON_PLATED_THROUGH_HOLE:
                continue
            assert pad.drill is not None
            drill_pad = replace(
                pad,
                size=pad.drill,
                shape=(PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm
                       else PadShape.OVAL),
            )
            position = transformed_local_point(placement, pad.position)
            holes.append((_footprint_hole_identity(placement.reference, pad.number, index),
                          placed_pad_shape(position, drill_pad, placement)))
    return tuple(holes)


def _footprint_hole_identity(reference: str, number: str, index: int) -> str:
    return f"hole:{reference}.{number}:{index}"


def _scoped_hole_rules(board: PhysicalBoard) -> dict[str, ComponentHoleClearance]:
    """Footprint NPTH identities of each component with a ``hole_clearance`` rule."""
    scoped = {rule.reference: rule for rule in board.component_hole_clearances}
    owned: dict[str, ComponentHoleClearance] = {}
    for placement in board.placements:
        rule = scoped.get(placement.reference)
        if rule is None:
            continue
        for index, pad in enumerate(board.footprints[placement.footprint].pads):
            if pad.kind is PadKind.NON_PLATED_THROUGH_HOLE:
                owned[_footprint_hole_identity(placement.reference, pad.number, index)] = rule
    return owned


def _check_non_plated_hole_clearance(
    board: PhysicalBoard, findings: list[DrcFinding],
    relaxations: list[DrcHoleClearanceRelaxation] | None = None,
) -> None:
    """Copper-to-NPTH clearance; a component's scoped rule covers only its own pads and holes."""
    clearance = board.rules.minimum_hole_clearance_nm
    scoped = _scoped_hole_rules(board)
    pads = _copper_pads(board)
    for identity, hole in non_plated_holes(board):
        rule = scoped.get(identity)
        for pad in pads:
            if rule is not None and pad.component == rule.reference:
                _check_scoped_pad_hole(pad, identity, hole, rule, clearance, findings, relaxations)
            elif not shapes_clear(pad.shape, hole, clearance):
                findings.append(_finding("DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"{pad.identity} violates {identity} drill clearance",
                    objects=(pad.identity, identity), nets=(pad.net,)))
        for index, track in enumerate(board.tracks):
            copper = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
            if not shapes_clear(copper, hole, clearance):
                findings.append(_finding(
                    "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"track {index} violates {identity} drill clearance",
                    objects=(f"track:{index}", identity), nets=(track.net,),
                    layers=(track.layer.value,),
                ))
        for index, via in enumerate(board.vias):
            copper = RoundedConvexShape((via.position,), via.size_nm // 2)
            if not shapes_clear(copper, hole, clearance):
                findings.append(_finding(
                    "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"via {index} violates {identity} drill clearance",
                    objects=(f"via:{index}", identity), nets=(via.net,),
                ))


def _check_scoped_pad_hole(
    pad: _PadCopper, identity: str, hole: RoundedConvexShape, rule: ComponentHoleClearance,
    board_clearance: int, findings: list[DrcFinding],
    relaxations: list[DrcHoleClearanceRelaxation] | None,
) -> None:
    """Check one pad against its own footprint's hole under the component's scoped rule."""
    if shapes_clear(pad.shape, hole, board_clearance):
        return
    distance_squared = shape_distance_squared(pad.shape, hole)
    measured = max(0, (_fraction_sqrt_floor(Fraction(4 * distance_squared))
                       - 2 * (pad.shape.radius_nm + hole.radius_nm)) // 2)
    if not shapes_clear(pad.shape, hole, rule.clearance_nm):
        findings.append(_finding(
            "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
            f"{pad.identity} violates {identity} drill clearance "
            f"(checked against the scoped hole_clearance of {rule.reference})",
            objects=(pad.identity, identity), nets=(pad.net,),
            required_nm=rule.clearance_nm, measured_nm=measured,
        ))
    elif relaxations is not None:
        relaxations.append(DrcHoleClearanceRelaxation(
            rule.reference, (pad.identity, identity), (pad.net,),
            board_clearance, rule.clearance_nm, measured, rule.reason))


def _check_via_hole_clearance(
    board: PhysicalBoard, findings: list[DrcFinding],
) -> None:
    """All plated via drills must clear foreign copper, regardless of finish."""

    pads = _copper_pads(board)
    clearance = board.rules.minimum_hole_clearance_nm
    for via_index, via in enumerate(board.vias):
        hole = RoundedConvexShape((via.position,), via.drill_nm // 2)
        for track_index, track in enumerate(board.tracks):
            if track.net == via.net or not _via_covers_layer(board, via, track.layer):
                continue
            copper = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
            if not shapes_clear(hole, copper, clearance):
                findings.append(_finding(
                    "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"via {via_index} drill violates track {track_index} clearance",
                    objects=(f"via:{via_index}", f"track:{track_index}"),
                    nets=tuple(sorted((via.net, track.net))),
                    required_nm=clearance,
                ))
        for other_index, other in enumerate(board.vias):
            if other_index == via_index or other.net == via.net or not _via_spans_overlap(board, via, other):
                continue
            copper = RoundedConvexShape((other.position,), other.size_nm // 2)
            if not shapes_clear(hole, copper, clearance):
                findings.append(_finding(
                    "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"via {via_index} drill violates via {other_index} copper clearance",
                    objects=(f"via:{via_index}", f"via:{other_index}"),
                    nets=tuple(sorted((via.net, other.net))),
                    required_nm=clearance,
                ))
        for pad in pads:
            if pad.net == via.net or not any(
                _via_covers_layer(board, via, layer) for layer in pad.layers
            ):
                continue
            if not shapes_clear(hole, pad.shape, clearance):
                findings.append(_finding(
                    "DRC-HOLE-CLEARANCE", DrcSeverity.ERROR,
                    f"via {via_index} drill violates {pad.identity} clearance",
                    objects=(f"via:{via_index}", pad.identity),
                    nets=tuple(sorted((via.net, pad.net))),
                    required_nm=clearance,
                ))


def _check_drill_spacing(board: PhysicalBoard, findings: list[DrcFinding]) -> None:
    """Check via-to-via and via-to-plated-pad drills, independent of net."""

    clearance = board.rules.minimum_hole_clearance_nm
    npth = non_plated_holes(board)
    other_drills = [(f"via:{i}", RoundedConvexShape((v.position,), (v.drill_nm + 1) // 2))
                    for i, v in enumerate(board.vias)]
    for placement in board.placements:
        for i, pad in enumerate(board.footprints[placement.footprint].pads):
            if pad.kind is PadKind.THROUGH_HOLE and pad.drill is not None:
                drill = replace(pad, size=pad.drill, kind=PadKind.SMD, drill=None,
                                shape=PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm else PadShape.OVAL)
                other_drills.append((f"drill:{placement.reference}:{i}",
                    placed_pad_shape(transformed_local_point(placement, pad.position), drill, placement)))
    for i, (identity, hole) in enumerate(npth):
        for other_id, other in (*npth[:i], *other_drills):
            if not shapes_clear(hole, other, clearance):
                findings.append(_finding("DRC-DRILL-SPACING", DrcSeverity.ERROR,
                    f"{identity} and {other_id} violate hole spacing",
                    objects=(identity, other_id), required_nm=clearance))
    for first_index, first in enumerate(board.vias):
        first_hole = RoundedConvexShape((first.position,), first.drill_nm // 2)
        for second_index in range(first_index + 1, len(board.vias)):
            second = board.vias[second_index]
            second_hole = RoundedConvexShape((second.position,), second.drill_nm // 2)
            if not shapes_clear(first_hole, second_hole, clearance):
                findings.append(_finding(
                    "DRC-DRILL-SPACING", DrcSeverity.ERROR,
                    f"vias {first_index} and {second_index} violate hole spacing",
                    objects=(f"via:{first_index}", f"via:{second_index}"),
                    nets=tuple(sorted({first.net, second.net})),
                    required_nm=clearance,
                ))
        for placement in board.placements:
            footprint = board.footprints[placement.footprint]
            for pad_index, pad in enumerate(footprint.pads):
                if pad.kind is not PadKind.THROUGH_HOLE or pad.drill is None:
                    continue
                drill_pad = replace(
                    pad, size=pad.drill, kind=PadKind.SMD, drill=None,
                    shape=(PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm
                           else PadShape.OVAL),
                )
                position = transformed_local_point(placement, pad.position)
                other_hole = placed_pad_shape(position, drill_pad, placement)
                if not shapes_clear(first_hole, other_hole, clearance):
                    findings.append(_finding(
                        "DRC-DRILL-SPACING", DrcSeverity.ERROR,
                        f"via {first_index} violates {placement.reference}.{pad.number} hole spacing",
                        objects=(f"via:{first_index}", f"pad:{placement.reference}.{pad.number}:{pad_index}"),
                        nets=(first.net,), required_nm=clearance,
                    ))


def _check_zone_fill_spacing(board: PhysicalBoard, findings: list[DrcFinding],
                             relaxations: list[DrcBreakoutRelaxation] | None = None) -> None:
    zone_nets = {zone.id: zone.net for zone in board.zones}
    pads = _copper_pads(board)
    spacing = _BreakoutSpacing(board, relaxations)
    # Fill copper is never in a breakout region; the other object may be.
    track_lands = [spacing.land(track.net, (track.start, track.end)) for track in board.tracks]
    via_lands = [spacing.land(via.net, (via.position,)) for via in board.vias]
    pad_lands = {pad.identity: spacing.land(pad.net, pad.shape.spine) for pad in pads}
    for fill in sorted(board.zone_fills, key=lambda item: (item.zone_id, item.layer.value)):
        net = zone_nets[fill.zone_id]
        for polygon_index, polygon in enumerate(fill.polygons):
            objects = f"zone-fill:{fill.zone_id}:{fill.layer.value}:{polygon_index}"
            for track_index, track in enumerate(board.tracks):
                if track.net == net or track.layer is not fill.layer:
                    continue
                shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
                clearance, normal = spacing.between(net, None, track.net, track_lands[track_index])
                if not _shape_clear_of_region(shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates track {track_index} clearance",
                                             objects=(objects, f"track:{track_index}"),
                                             nets=tuple(sorted((net, track.net))), layers=(fill.layer.value,)))
                elif clearance < normal and not _shape_clear_of_region(shape, polygon, normal):
                    spacing.record(spacing.check(net, track.net), (objects, f"track:{track_index}"),
                                   ((net, None), (track.net, track_lands[track_index])),
                                   (fill.layer.value,), normal, clearance, None)
            for via_index, via in enumerate(board.vias):
                if via.net == net or not _via_covers_layer(board, via, fill.layer):
                    continue
                shape = RoundedConvexShape((via.position,), via.size_nm // 2)
                clearance, normal = spacing.between(net, None, via.net, via_lands[via_index])
                if not _shape_clear_of_region(shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates via {via_index} clearance",
                                             objects=(objects, f"via:{via_index}"),
                                             nets=tuple(sorted((net, via.net))), layers=(fill.layer.value,)))
                elif clearance < normal and not _shape_clear_of_region(shape, polygon, normal):
                    spacing.record(spacing.check(net, via.net), (objects, f"via:{via_index}"),
                                   ((net, None), (via.net, via_lands[via_index])),
                                   (fill.layer.value,), normal, clearance, None)
            for pad in pads:
                if pad.net == net or fill.layer not in pad.layers:
                    continue
                relaxed, normal = spacing.between(net, None, pad.net, pad_lands[pad.identity])
                clearance, normal = max(pad.clearance_nm, relaxed), max(pad.clearance_nm, normal)
                if not _shape_clear_of_region(pad.shape, polygon, clearance):
                    findings.append(_finding("DRC-ZONE-CLEARANCE", DrcSeverity.ERROR,
                                             f"{objects} violates {pad.identity} clearance",
                                             objects=(objects, pad.identity),
                                             nets=tuple(sorted((net, pad.net))), layers=(fill.layer.value,)))
                elif clearance < normal and not _shape_clear_of_region(pad.shape, polygon, normal):
                    spacing.record(spacing.check(net, pad.net), (objects, pad.identity),
                                   ((net, None), (pad.net, pad_lands[pad.identity])),
                                   (fill.layer.value,), normal, clearance, None)


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


def placed_pad_shape(position: Point, pad: object, placement: object) -> RoundedConvexShape:
    width = getattr(pad, "size").width_nm
    height = getattr(pad, "size").height_nm
    shape = getattr(pad, "shape")
    # Pad angles exported to KiCad are in board coordinates. Convert its
    # Cartesian-positive angle to the IR's screen-coordinate convention.
    angle = -float(getattr(placement, "rotation_degrees") + getattr(pad, "rotation_degrees"))
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


class _BreakoutSpacing:
    """Region-aware copper spacing for physical DRC (plan R1).

    Every requirement is exactly ``_clearance`` when no routing rule declares
    breakout properties. A check that passes only under breakout values is
    recorded with the regions that justified it.
    """

    def __init__(self, board: PhysicalBoard,
                 relaxations: list[DrcBreakoutRelaxation] | None) -> None:
        self.board = board
        self.rules = {item.net: item for item in board.net_routing_rules}
        self.regions = BreakoutRegions(board)
        self.relaxations = relaxations

    def land(self, net: str, spine: tuple[Point, ...]) -> str | None:
        """The terminal land whose breakout region holds this copper of ``net``."""
        return self.regions.region(net, spine) if net in self.regions.declared else None

    def between(self, first: str, first_land: str | None,
                second: str, second_land: str | None) -> tuple[int, int]:
        """Required spacing with breakout regions applied, and without them."""
        if not self.regions:
            value = _clearance(self.board, self.rules.get(first), self.rules.get(second))
            return value, value
        return (self.regions.spacing_nm(first, first_land, second, second_land),
                self.regions.spacing_nm(first, None, second, None))

    def check(self, first: str, second: str) -> str:
        return "pair_gap" if self.regions and self.regions.pair(first, second) else "clearance"

    def subject(self, first: str, second: str) -> str:
        return "pair gap" if self.check(first, second) == "pair_gap" else "copper spacing"

    def note(self, distance_squared: Fraction | int, radii_twice: int, required: int, relaxed: int,
             objects: tuple[str, ...], copper: tuple[tuple[str, str | None], ...],
             layers: tuple[str, ...]) -> None:
        """Record a relaxation when copper meets ``relaxed`` but not ``required``.

        ``distance_squared`` is between the spines and ``radii_twice`` the sum
        of both copper widths; ``copper`` pairs each net with its region.
        """
        needed = radii_twice + 2 * required
        if 4 * distance_squared >= needed * needed:
            return
        measured = (_fraction_sqrt_floor(Fraction(4 * distance_squared)) - radii_twice) // 2
        self.record(self.check(copper[0][0], copper[1][0]), objects, copper, layers,
                    required, relaxed, measured)

    def record(self, check: str, objects: tuple[str, ...],
               copper: tuple[tuple[str, str | None], ...], layers: tuple[str, ...],
               required: int, relaxed: int, measured: int | None) -> None:
        if self.relaxations is None:
            return
        regions = tuple(sorted({
            (net, land, self.rules[net].breakout_length_nm or 0)
            for net, land in copper if land is not None and self.regions.relaxes(net, land, check)
        }))
        self.relaxations.append(DrcBreakoutRelaxation(
            check, objects, tuple(sorted({net for net, _ in copper})), layers,
            required, relaxed, measured, regions))


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
    if report.breakout_relaxations:
        # Present only when breakout values relaxed a check, so reports and
        # digests of boards without breakout regions are unchanged.
        document["breakout_relaxations"] = [{
            "check": item.check, "objects": list(item.objects), "nets": list(item.nets),
            "layers": list(item.layers), "required_nm": item.required_nm,
            "relaxed_nm": item.relaxed_nm, "measured_nm": item.measured_nm,
            "regions": [{"net": net, "land": land, "breakout_length_nm": length}
                        for net, land, length in item.regions],
        } for item in report.breakout_relaxations]
    if report.hole_clearance_relaxations:
        # Likewise present only when a component-scoped hole_clearance relaxed a check.
        document["hole_clearance_relaxations"] = [{
            "component": item.component, "objects": list(item.objects), "nets": list(item.nets),
            "required_nm": item.required_nm, "relaxed_nm": item.relaxed_nm,
            "measured_nm": item.measured_nm, "reason": item.reason,
        } for item in report.hole_clearance_relaxations]
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
    return (item.net, item.position.x_nm, item.position.y_nm, item.from_layer.value, item.to_layer.value, item.size_nm, item.drill_nm, item.technology, item.finish)


def _routing_rule_document(item: NetRoutingRule) -> tuple[object, ...]:
    document = (item.net, item.kind.value, item.priority, item.width_nm, item.clearance_nm,
            tuple(layer.value for layer in item.allowed_layers), item.max_vias,
            item.max_length_nm, item.differential_partner, item.pair_gap_nm,
            item.max_skew_nm, item.topology, item.target_impedance_ohms,
            item.maximum_uncoupled_length_nm, item.maximum_stub_length_nm,
            item.tuning_amplitude_limit_nm, item.require_return_vias, item.reserve_corridor,
            item.return_via_net, item.maximum_return_via_distance_nm,
            item.impedance_evidence_digest, item.return_via_policy.value,
            item.shared_reference_layer.value if item.shared_reference_layer is not None else None)
    # Signal-integrity/breakout intent extends the document only when declared,
    # so digests of rules that do not use it are unchanged.
    signal_intent = (
        item.target_single_ended_ohms,
        None if item.impedance_tolerance_percent is None else str(item.impedance_tolerance_percent),
        item.layer_group, item.breakout_length_nm, item.breakout_width_nm,
        item.breakout_gap_nm, item.breakout_clearance_nm)
    if any(value is not None for value in signal_intent):
        document = (*document, signal_intent)
    # Tuning geometry (plan R10) likewise, only when it differs from the default.
    if item.tuning_style is not TuningStyle.BUMPS or item.tuning_spacing_nm is not None:
        document = (*document, ("tuning", item.tuning_style.value, item.tuning_spacing_nm))
    return document


def _footprint_document(name: str, footprint: object) -> tuple[object, ...]:
    pads = getattr(footprint, "pads")
    return (
        name,
        getattr(footprint, "source_library_id"),
        tuple(group.numbers for group in getattr(footprint, "internal_pad_groups")),
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
                pad.zone_connection.value if pad.zone_connection else None,
                pad.heatsink,
                pad.remove_unused_layers,
                pad.connector_contact,
            )
            for pad in pads
        ),
        tuple(repr(item) for item in getattr(footprint, "graphics")),
        tuple(repr(item) for item in getattr(footprint, "keepouts")),
        tuple((point.x_nm, point.y_nm) for point in getattr(footprint, "courtyard")),
        tuple(sorted(getattr(footprint, "metadata").items())),
        getattr(footprint, "height_nm"),
        getattr(footprint, "clearance_nm"),
        getattr(footprint, "exclude_from_bom"),
        getattr(footprint, "exclude_from_pos_files"),
    )


def _digest(document: object) -> str:
    return sha256(json.dumps(document, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

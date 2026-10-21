"""Physical-design readiness gates and placement workflow orchestration."""

from __future__ import annotations

from dataclasses import asdict, dataclass, replace
from enum import Enum
import json
from types import MappingProxyType

from .physical import PhysicalBoard
from .placement import (
    CorridorReservation,
    PlacementAlgorithmError,
    PlacementCandidate,
    PlacementMetrics,
    PlacementPlannerOptions,
    generate_placement_candidates,
    reserved_corridors,
    select_placement_candidate,
)
from .placement_escape import EscapeChannel, EscapeSpacingModel


class LayoutStage(str, Enum):
    PREPARE = "prepare"
    PLACE = "place"
    ROUTE = "route"
    VERIFY = "verify"


class GateStatus(str, Enum):
    PASS = "pass"
    WARNING = "warning"
    BLOCKED = "blocked"
    NOT_RUN = "not_run"


@dataclass(frozen=True, slots=True)
class LayoutFinding:
    code: str
    stage: LayoutStage
    status: GateStatus
    message: str
    subject: str | None = None


@dataclass(frozen=True, slots=True)
class LayoutGate:
    stage: LayoutStage
    status: GateStatus
    summary: str


@dataclass(frozen=True, slots=True)
class LayoutReport:
    board_name: str
    gates: tuple[LayoutGate, ...]
    findings: tuple[LayoutFinding, ...]
    metrics: PlacementMetrics
    candidates: tuple[PlacementCandidate, ...] = ()
    selected_candidate: str = "candidate-00"
    algorithm: str = "hierarchical-analytical/hybrid-legalize/route-refine-v0.2"
    escape_channels: tuple[EscapeChannel, ...] = ()
    corridors: CorridorReservation = CorridorReservation()

    def to_json(self) -> str:
        return json.dumps(
            {
                "schema": "copperscript-layout-report/v0.1",
                "board": self.board_name,
                "algorithm": self.algorithm,
                "selected_candidate": self.selected_candidate,
                "gates": [
                    {
                        "stage": gate.stage.value,
                        "status": gate.status.value,
                        "summary": gate.summary,
                    }
                    for gate in self.gates
                ],
                "findings": [
                    {
                        "code": finding.code,
                        "stage": finding.stage.value,
                        "status": finding.status.value,
                        "message": finding.message,
                        **(
                            {"subject": finding.subject}
                            if finding.subject is not None
                            else {}
                        ),
                    }
                    for finding in self.findings
                ],
                "metrics": _metrics_json(self.metrics),
                "escape_channels": [asdict(channel) | {"deficit_nm": channel.deficit_nm}
                                    for channel in self.escape_channels],
                "candidates": [
                    {
                        "id": candidate.candidate_id,
                        "seed": candidate.seed,
                        "selected": candidate.candidate_id == self.selected_candidate,
                        "metrics": _metrics_json(candidate.metrics),
                        "statistics": {
                            "analytical_iterations": candidate.statistics.analytical_iterations,
                            "exact_repair_count": candidate.statistics.exact_repair_count,
                            "relative_repair_count": candidate.statistics.relative_repair_count,
                            "refinement_move_count": candidate.statistics.refinement_move_count,
                            "refinement_swap_count": candidate.statistics.refinement_swap_count,
                            "routing_feedback_passes": candidate.statistics.routing_feedback_passes,
                            **({"support_patterns": candidate.statistics.support_patterns}
                               if candidate.statistics.support_patterns is not None else {}),
                            **({"package_patterns": candidate.statistics.package_patterns}
                               if candidate.statistics.package_patterns else {}),
                            **({"failed_seed_attempts": [asdict(failure)
                                for failure in candidate.statistics.failed_seed_attempts]}
                               if candidate.statistics.failed_seed_attempts else {}),
                        },
                    }
                    for candidate in self.candidates
                ],
                "reserved_corridors": [
                    {
                        "name": corridor.name,
                        "nets": list(corridor.nets),
                        "side": corridor.side.value if corridor.side is not None else "both",
                        "margin_nm": corridor.margin_nm,
                        "terminal_components": list(corridor.terminal_references),
                        "polygon_nm": [
                            [point.x_nm, point.y_nm]
                            for point in corridor.keepout.outline.vertices
                        ],
                    }
                    for corridor in self.corridors.corridors
                ],
                "skipped_corridors": [
                    {"nets": list(item.nets), "reason": item.reason}
                    for item in self.corridors.skipped
                ],
            },
            indent=2,
            sort_keys=True,
        ) + "\n"


@dataclass(frozen=True, slots=True)
class PlacementPlan:
    board: PhysicalBoard
    report: LayoutReport
    candidates: tuple[PlacementCandidate, ...] = ()


class PlacementPlanningError(ValueError):
    pass


def plan_placement(
    board: PhysicalBoard,
    options: PlacementPlannerOptions | None = None,
    *, progress=None,
) -> PlacementPlan:
    """Create deterministic Pareto candidates and select one legal placement."""

    options = options or PlacementPlannerOptions()
    try:
        candidates = generate_placement_candidates(board, options, progress=progress)
    except PlacementAlgorithmError as exc:
        raise PlacementPlanningError(str(exc)) from exc
    selected = select_placement_candidate(candidates)
    metadata = dict(board.metadata)
    metadata.update(
        {
            "prototype_placement": "false",
            "planned_placement": "true",
            "layout_stage": LayoutStage.PLACE.value,
            "placement_algorithm": "hierarchical-analytical/hybrid-legalize/route-refine-v0.2",
            "placement_candidate": selected.candidate_id,
            "placement_candidate_count": str(len(candidates)),
            "fabrication_ready": "false",
        }
    )
    planned = replace(
        board,
        placements=selected.placements,
        metadata=MappingProxyType(metadata),
    )
    channels = EscapeSpacingModel(board, margin_nm=options.escape_margin_nm,
        transit_lanes=options.escape_transit_lanes).channels({p.reference: p for p in selected.placements})
    findings = _findings(board, selected.metrics, candidates, channels) + placement_intent_findings(planned)
    gates = _gates(findings)
    return PlacementPlan(
        planned,
        LayoutReport(
            board.name,
            gates,
            findings,
            selected.metrics,
            candidates,
            selected.candidate_id,
            escape_channels=channels,
            corridors=reserved_corridors(board),
        ),
        candidates,
    )


def _metrics_json(metrics: PlacementMetrics) -> dict[str, int]:
    return {
        "component_count": metrics.component_count,
        "net_count": metrics.net_count,
        "estimated_connection_count": metrics.estimated_connection_count,
        "half_perimeter_wire_length_nm": metrics.half_perimeter_wire_length_nm,
        "crossing_count": metrics.crossing_count,
        "estimated_via_count": metrics.estimated_via_count,
        "pin_escape_pressure": metrics.pin_escape_pressure,
        "escape_channel_penalty_nm": metrics.escape_channel_penalty_nm,
        "escape_channel_deficit_nm": metrics.escape_channel_deficit_nm,
        "escape_channel_pair_count": metrics.escape_channel_pair_count,
        "constraint_penalty_nm": metrics.constraint_penalty_nm,
        "minimum_constraint_margin_nm": metrics.minimum_constraint_margin_nm,
        "group_spread_nm": metrics.group_spread_nm,
        "power_domain_penalty_nm": metrics.power_domain_penalty_nm,
        "weighted_wire_length_nm": metrics.weighted_wire_length_nm,
        "local_connection_length_nm": metrics.local_connection_length_nm,
        "congestion_bin_count": metrics.congestion_bin_count,
        "congestion_capacity_per_bin": metrics.congestion_capacity_per_bin,
        "congestion_overflow": metrics.congestion_overflow,
        "maximum_congestion_utilization_ppm": metrics.maximum_congestion_utilization_ppm,
    }


def _findings(
    board: PhysicalBoard,
    metrics: PlacementMetrics,
    candidates: tuple[PlacementCandidate, ...],
    channels: tuple[EscapeChannel, ...] = (),
) -> tuple[LayoutFinding, ...]:
    findings: list[LayoutFinding] = []
    if board.metadata.get("prototype_footprints") == "true":
        findings.append(
            LayoutFinding(
                "PROXY_FOOTPRINTS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                "proxy footprints are sufficient for workflow testing but not physical signoff",
            )
        )
    if board.metadata.get("footprint_import_warnings"):
        findings.append(
            LayoutFinding(
                "FOOTPRINT_IMPORT_WARNINGS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                "one or more resolved footprints have lossy-import warnings",
            )
        )
    omitted = board.metadata.get("omitted_components", "")
    if omitted:
        findings.append(
            LayoutFinding(
                "OMITTED_COMPONENTS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                f"components without selected footprints were omitted: {omitted}",
            )
        )
    omitted_targets = board.metadata.get("omitted_constraint_targets", "")
    if omitted_targets:
        findings.append(
            LayoutFinding(
                "OMITTED_CONSTRAINT_TARGETS",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                f"physical constraints target omitted components: {omitted_targets}",
            )
        )
    for channel in channels:
        findings.append(LayoutFinding(
            "ESCAPE_CHANNEL_DEFICIT", LayoutStage.PLACE, GateStatus.WARNING,
            f"{channel.left}/{channel.right}: facing {channel.axis}-channel has "
            f"{channel.gap_nm / 1_000_000:.3f} mm, estimated target "
            f"{channel.required_gap_nm / 1_000_000:.3f} mm; fixed/proximity constraints "
            "remain authoritative and joint package access must be verified",
            f"{channel.left},{channel.right}",
        ))
    corridors = reserved_corridors(board)
    for skipped in corridors.skipped:
        findings.append(
            LayoutFinding(
                "CORRIDOR_NOT_RESERVED",
                LayoutStage.PREPARE,
                GateStatus.WARNING,
                f"reserve_corridor for {'/'.join(skipped.nets)} was not applied: {skipped.reason}",
                "/".join(skipped.nets),
            )
        )
    if corridors.corridors:
        findings.append(
            LayoutFinding(
                "RESERVED_CORRIDORS",
                LayoutStage.PLACE,
                GateStatus.PASS,
                f"kept {len(corridors.corridors)} differential-pair corridor(s) free of "
                "non-terminal components: "
                + ", ".join("/".join(item.nets) for item in corridors.corridors),
            )
        )
    if metrics.congestion_overflow:
        findings.append(
            LayoutFinding(
                "ESTIMATED_CONGESTION",
                LayoutStage.PLACE,
                GateStatus.WARNING,
                f"coarse layer-aware routing estimate has {metrics.congestion_overflow} overflow units",
            )
        )
    if metrics.crossing_count:
        findings.append(
            LayoutFinding(
                "ESTIMATED_CROSSINGS",
                LayoutStage.PLACE,
                GateStatus.WARNING,
                f"coarse routing estimate contains {metrics.crossing_count} same-layer crossings",
            )
        )
    findings.append(
        LayoutFinding(
            "PLACEMENT_CANDIDATES",
            LayoutStage.PLACE,
            GateStatus.PASS,
            f"generated {len(candidates)} non-dominated deterministic placement candidate(s)",
        )
    )
    findings.append(
        LayoutFinding(
            "DETAILED_ROUTING_NOT_RUN",
            LayoutStage.ROUTE,
            GateStatus.NOT_RUN,
            "placement includes a coarse routability estimate only; no copper routes were generated",
        )
    )
    findings.append(
        LayoutFinding(
            "PHYSICAL_SIGNOFF_NOT_RUN",
            LayoutStage.VERIFY,
            GateStatus.NOT_RUN,
            "DRC, SI/PI, thermal, RF, DFX, and independent CAM verification remain required",
        )
    )
    return tuple(findings)


def _gates(findings: tuple[LayoutFinding, ...]) -> tuple[LayoutGate, ...]:
    prepare_warning = any(
        item.stage is LayoutStage.PREPARE and item.status is GateStatus.WARNING
        for item in findings
    )
    place_warning = any(
        item.stage is LayoutStage.PLACE and item.status is GateStatus.WARNING
        for item in findings
    )
    return (
        LayoutGate(
            LayoutStage.PREPARE,
            GateStatus.WARNING if prepare_warning else GateStatus.PASS,
            "inputs accepted for placement exploration"
            if prepare_warning
            else "physical inputs are complete for placement",
        ),
        LayoutGate(
            LayoutStage.PLACE,
            GateStatus.WARNING if place_warning else GateStatus.PASS,
            "legal placement produced with routability warnings"
            if place_warning
            else "legal placement produced with no estimated congestion or crossings",
        ),
        LayoutGate(LayoutStage.ROUTE, GateStatus.NOT_RUN, "detailed routing is not run by plan-layout"),
        LayoutGate(
            LayoutStage.VERIFY,
            GateStatus.BLOCKED,
            "release is blocked until routing and signoff pass",
        ),
    )


def placement_intent_findings(board: PhysicalBoard, *, remote_mm: float = 8.0,
                              edge_mm: float = 2.0) -> tuple[LayoutFinding, ...]:
    """Placement facts that usually mean an authored pose fights the routing.

    * REMOTE_FIXED_PART: a fixed small part (at most four pads) whose nearest
      pad partner on a signal net is more than ``remote_mm`` away (for example
      a crystal load capacitor fixed far from its crystal). Parts within
      ``edge_mm`` of the board outline are skipped: connectors, antennas and
      switches are fixed at the edge for mechanical reasons.
    * FIXED_PART_IN_PAIR_CORRIDOR: a fixed part that sits in the path of a
      differential pair between fixed terminals (automatic pair corridors).
    * AUTO_PAIR_CORRIDORS: the corridors movable parts were kept out of.

    Planes, power rails, decoupling returns and nets with more than twelve
    pads are ignored when looking for partners: they connect almost everything.
    """
    from math import hypot
    from .placement import _keepout_blocks, _placement_polygon, _rules, automatic_pair_corridors, \
        transformed_pad_position
    from .zone_geometry import distribution_zone_nets
    findings: list[LayoutFinding] = []
    rules = _rules(board)
    poses = {pose.reference: pose for pose in board.placements}
    fixed = {ref for ref, rule in rules.items() if rule.fixed_position is not None and ref in poses}
    excluded = set(distribution_zone_nets(board)) | {domain.net for domain in board.power_domains}
    excluded |= {link.return_net for link in board.decoupling_links}
    outline = board.outline.vertices
    edges = [(outline[i], outline[(i + 1) % len(outline)]) for i in range(len(outline))]

    def edge_distance_mm(point):
        best_nm = float("inf")
        for a, b in edges:
            dx, dy = b.x_nm - a.x_nm, b.y_nm - a.y_nm
            length2 = dx * dx + dy * dy
            t = 0.0 if length2 == 0 else max(0.0, min(1.0, ((point.x_nm - a.x_nm) * dx
                                                          + (point.y_nm - a.y_nm) * dy) / length2))
            best_nm = min(best_nm, hypot(point.x_nm - a.x_nm - t * dx, point.y_nm - a.y_nm - t * dy))
        return best_nm / 1_000_000

    for reference in sorted(fixed):
        pose = poses[reference]
        pads = board.footprints[pose.footprint].pads
        if len(pads) > 4:
            continue
        # Edge parts (connectors, antennas, switches) are fixed for mechanical reasons.
        if min(edge_distance_mm(point) for point in _placement_polygon(board, pose)) <= edge_mm:
            continue
        best = None
        for net in board.nets:
            if net.name in excluded or len(net.pads) > 12:
                continue
            own = [pad for pad in net.pads if pad.component == reference]
            if not own:
                continue
            for pad in own:
                here = transformed_pad_position(board, pose, pad.pad)
                for other in net.pads:
                    if other.component == reference or other.component not in poses:
                        continue
                    there = transformed_pad_position(board, poses[other.component], other.pad)
                    distance = hypot(here.x_nm - there.x_nm, here.y_nm - there.y_nm) / 1_000_000
                    if best is None or distance < best[0]:
                        best = (distance, net.name, f"{other.component}.{other.pad}")
        if best is not None and best[0] > remote_mm:
            findings.append(LayoutFinding(
                "REMOTE_FIXED_PART", LayoutStage.PLACE, GateStatus.WARNING,
                f"{reference} is fixed {best[0]:.1f} mm from its nearest signal partner {best[2]} "
                f"(net {best[1]}); if it serves that pin, move it there or drop its fixed_placement",
                reference))
    corridors = automatic_pair_corridors(board)
    blocking: dict[str, list[str]] = {}
    for corridor in corridors:
        for reference in sorted(fixed):
            if reference in corridor.terminal_references:
                continue
            pose = poses[reference]
            if _keepout_blocks(board, corridor.keepout, pose, _placement_polygon(board, pose)):
                blocking.setdefault(reference, []).append("/".join(corridor.nets))
    for reference, pairs in sorted(blocking.items()):
        findings.append(LayoutFinding(
            "FIXED_PART_IN_PAIR_CORRIDOR", LayoutStage.PLACE, GateStatus.WARNING,
            f"{reference} is fixed in the straight path of differential pair(s) {', '.join(pairs)}; "
            "those pairs have to route around it",
            reference))
    if corridors:
        findings.append(LayoutFinding(
            "AUTO_PAIR_CORRIDORS", LayoutStage.PLACE, GateStatus.PASS,
            f"kept movable parts out of {len(corridors)} differential-pair path(s) between fixed terminals: "
            + ", ".join("/".join(item.nets) for item in corridors)))
    return tuple(findings)

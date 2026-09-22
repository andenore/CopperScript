"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport, run_physical_drc
from .physical import PhysicalBoard
from .placement import PlacementPlannerOptions
from .routeflow import PlacementRoutingFeedbackOptions, PlacementRoutingResult, optimize_placement_for_routing
from .routing import GlobalRouterOptions


class PhysicalFlowStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"


@dataclass(frozen=True, slots=True)
class RoutingPipelineResult:
    status: PhysicalFlowStatus
    placement_and_global: PlacementRoutingResult
    critical: CriticalRoutingResult
    detailed: DetailedRoutingResult
    drc: PhysicalDrcReport

    @property
    def board(self) -> PhysicalBoard:
        return self.detailed.board


def run_routing_pipeline(
    board: PhysicalBoard,
    *,
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    feedback_options: PlacementRoutingFeedbackOptions | None = None,
    detailed_options: DetailedRouterOptions | None = None,
    drc_policy: PhysicalDrcPolicy | None = None,
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order without weakening an earlier gate."""

    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    placement = optimize_placement_for_routing(
        board, placement_options, global_options, feedback_options
    )
    critical = route_critical_nets(placement.board, placement.global_route)
    detailed = route_detailed(critical.board, placement.global_route, detailed_options)
    drc = run_physical_drc(detailed.board, policy=drc_policy)
    passed = (
        placement.full_route_certified
        and critical.status is not CriticalRoutingStatus.FAILED
        and detailed.status is DetailedRoutingStatus.SUCCESS
        and drc.decision is DrcDecision.PASS
    )
    return RoutingPipelineResult(
        PhysicalFlowStatus.PASS if passed else PhysicalFlowStatus.FAIL,
        placement,
        critical,
        detailed,
        drc,
    )

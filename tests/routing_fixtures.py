"""Shared synthetic routing fixtures: test geometry, not reusable components."""
from pcbir import (GlobalRouterOptions, FootprintPad, PhysicalFootprint, Point, PolygonRing,
                   PolygonWithHoles, Size, nm_from_mm)
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions
from pcbir.flow import run_routing_pipeline
from pcbir.placement import PlacementPlannerOptions
from pcbir.routeflow import PlacementRoutingFeedbackOptions


def terminal():
    return PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))


def rectangle(x0, y0, x1, y1):
    return PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))))


def hard_findings(board):
    return tuple(f for f in run_physical_drc(board).findings
        if f.severity.value == "error" and f.code not in
        {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})


def pipeline(board, *, movable=(), plane_options=None):
    return run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,
            analytical_iterations=0, refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements
                                       if p.reference not in movable)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        fanout_options=FanoutOptions(minimum_component_pads=2,
            maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False),
        plane_stitch_options=plane_options)

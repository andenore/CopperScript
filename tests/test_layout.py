from __future__ import annotations

from dataclasses import replace
import json

import pytest

from pcbir import (
    GateStatus,
    LayoutStage,
    PlacementPlannerOptions,
    PlacementPlanningError,
    plan_placement,
    prototype_physicalize,
)
from pcbir.loader import load_board
from pcbir.physical import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.physicalize import PrototypePhysicalOptions


def _prototype_board():
    return prototype_physicalize(load_board("examples/valid_board.copper"))


def test_planner_is_deterministic_and_marks_scope() -> None:
    physical = _prototype_board()

    first = plan_placement(physical)
    second = plan_placement(physical)

    assert first == second
    assert first.board.metadata["planned_placement"] == "true"
    assert first.board.metadata["prototype_placement"] == "false"
    assert first.board.metadata["fabrication_ready"] == "false"
    assert [gate.stage for gate in first.report.gates] == list(LayoutStage)
    assert first.report.gates[0].status is GateStatus.WARNING
    assert first.report.gates[1].status in {GateStatus.PASS, GateStatus.WARNING}
    assert first.report.gates[2].status is GateStatus.NOT_RUN
    assert first.report.gates[3].status is GateStatus.BLOCKED


def test_planner_produces_in_bounds_non_overlapping_placement() -> None:
    plan = plan_placement(_prototype_board())
    placements = plan.board.placements
    clearance = PlacementPlannerOptions().component_clearance_nm
    edge = PlacementPlannerOptions().edge_clearance_nm
    width = max(point.x_nm for point in plan.board.outline.vertices)
    height = max(point.y_nm for point in plan.board.outline.vertices)

    rectangles: list[tuple[str, int, int, int, int]] = []
    for placement in placements:
        size = plan.board.footprints[placement.footprint].body_size
        half_width = (size.width_nm + 1) // 2
        half_height = (size.height_nm + 1) // 2
        left = placement.position.x_nm - half_width
        right = placement.position.x_nm + half_width
        top = placement.position.y_nm - half_height
        bottom = placement.position.y_nm + half_height
        assert left >= edge
        assert top >= edge
        assert right <= width - edge
        assert bottom <= height - edge
        rectangles.append((placement.reference, left, top, right, bottom))

    for index, (left_ref, left_x, left_y, right_x, right_y) in enumerate(rectangles):
        for right_ref, other_left, other_top, other_right, other_bottom in rectangles[index + 1 :]:
            separated = (
                right_x + clearance <= other_left
                or other_right + clearance <= left_x
                or right_y + clearance <= other_top
                or other_bottom + clearance <= left_y
            )
            assert separated, f"{left_ref} overlaps {right_ref}"


def test_planner_preserves_fixed_placement() -> None:
    physical = _prototype_board()
    original = next(item for item in physical.placements if item.reference == "U1")

    plan = plan_placement(
        physical,
        PlacementPlannerOptions(fixed_references=frozenset({"U1"})),
    )

    placed = next(item for item in plan.board.placements if item.reference == "U1")
    assert placed == original


def test_layout_report_is_machine_readable() -> None:
    report = plan_placement(_prototype_board()).report
    document = json.loads(report.to_json())

    assert document["schema"] == "copperscript-layout-report/v0.1"
    assert document["gates"][2]["stage"] == "route"
    assert document["gates"][2]["status"] == "not_run"
    assert document["metrics"]["component_count"] > 0
    assert report.to_json() == report.to_json()


def test_planner_reports_when_board_cannot_be_legalized() -> None:
    physical = prototype_physicalize(
        load_board("examples/valid_board.copper"),
        PrototypePhysicalOptions(board_width_mm=10, board_height_mm=10, margin_mm=1),
    )

    with pytest.raises(PlacementPlanningError, match="cannot legalize"):
        plan_placement(physical)


def test_planner_refuses_to_invalidate_existing_routes() -> None:
    physical = _prototype_board()
    routed = replace(
        physical,
        tracks=(
            TrackSegment(
                physical.nets[0].name,
                Point.mm(1, 1),
                Point.mm(2, 1),
                nm_from_mm("0.25"),
                CopperLayer.FRONT,
            ),
        ),
    )

    with pytest.raises(PlacementPlanningError, match="unrouted board"):
        plan_placement(routed)


def test_planner_rejects_unmodeled_rotated_collision_geometry() -> None:
    physical = _prototype_board()
    rotated = replace(
        physical,
        placements=(replace(physical.placements[0], rotation_degrees=45),)
        + physical.placements[1:],
    )

    with pytest.raises(PlacementPlanningError, match="non-orthogonal rotation"):
        plan_placement(rotated)

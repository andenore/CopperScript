from __future__ import annotations

from dataclasses import replace
import json

import pytest

from pcbir import (
    BoardOutline,
    ComponentPlacementRule,
    FootprintPad,
    GateStatus,
    LayoutStage,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementPlannerOptions,
    PlacementPlanningError,
    PlacementTarget,
    RelativePlacementKind,
    RelativePlacementRule,
    Size,
    Stackup,
    compile_source,
    placement_metrics,
    placement_solution_is_legal,
    plan_placement,
    prototype_physicalize,
    transformed_pad_position,
)
from pcbir.loader import load_board
from pcbir.physical import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.physicalize import PrototypePhysicalOptions


def _prototype_board():
    return prototype_physicalize(load_board("examples/valid_board/board.copper"))


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
        if int(placement.rotation_degrees) % 180:
            half_width = (size.height_nm + 1) // 2
            half_height = (size.width_nm + 1) // 2
        else:
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


def test_dense_packages_get_a_soft_escape_channel_when_space_is_available() -> None:
    footprint = PhysicalFootprint(
        "test/dense-ic",
        tuple(
            FootprintPad(str(index + 1), Point.mm((index % 8 - 3.5) * 0.6,
                                                    (index // 8 - 1.5) * 0.6),
                         Size.mm(0.3, 0.3))
            for index in range(32)
        ),
        Size.mm(6, 6),
    )
    board = PhysicalBoard(
        "DensePlacement",
        BoardOutline.rectangle(60, 40),
        {footprint.name: footprint},
        (
            Placement("U1", footprint.name, Point.mm(20, 20)),
            Placement("U2", footprint.name, Point.mm(29, 20)),
        ),
        (),
    )
    options = PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
                                      refinement_passes=0)
    before = placement_metrics(board, {item.reference: item for item in board.placements}, options)
    plan = plan_placement(board, options)
    after = plan.report.metrics

    assert before.high_pin_spacing_penalty_nm > 0
    assert after.high_pin_spacing_penalty_nm < before.high_pin_spacing_penalty_nm
    assert placement_solution_is_legal(
        board, {item.reference: item for item in plan.board.placements}, options
    )


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
    assert "pin_escape_pressure" in document["metrics"]
    assert "high_pin_spacing_penalty_nm" in document["metrics"]
    assert "minimum_constraint_margin_nm" in document["metrics"]
    assert document["candidates"]
    assert "analytical_iterations" in document["candidates"][0]["statistics"]
    assert report.to_json() == report.to_json()


def test_planner_reports_when_board_cannot_be_legalized() -> None:
    physical = prototype_physicalize(
        load_board("examples/valid_board/board.copper"),
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


def test_planner_normalizes_a_movable_non_orthogonal_seed() -> None:
    physical = _prototype_board()
    rotated = replace(
        physical,
        placements=(replace(physical.placements[0], rotation_degrees=45),)
        + physical.placements[1:],
    )

    planned = plan_placement(rotated)

    assert all(item.rotation_degrees % 90 == 0 for item in planned.board.placements)


def test_explicit_45_degree_orientation_is_legal_and_selected() -> None:
    footprint = PhysicalFootprint(
        "test/diagonal",
        (FootprintPad("1", Point.mm(1, 0), Size.mm("0.5", "0.5")),),
        Size.mm(4, 2),
    )
    board = PhysicalBoard(
        "DiagonalPlacement", BoardOutline.rectangle(30, 30),
        {footprint.name: footprint},
        (Placement("U1", footprint.name, Point.mm(15, 15)),),
        (),
        placement_rules=(ComponentPlacementRule(
            "U1", allowed_orientations=(45,),
        ),),
    )
    plan = plan_placement(board, PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0,
    ))

    placed = plan.board.placements[0]
    assert placed.rotation_degrees == 45
    assert placement_solution_is_legal(board, {"U1": placed})
    assert transformed_pad_position(board, placed, "1") != Point.mm(16, 15)


def test_hierarchy_and_interfaces_create_semantic_placement_groups() -> None:
    physical = prototype_physicalize(load_board("examples/hierarchical_board/board.copper"))

    module_group = next(
        group for group in physical.placement_groups if group.name == "module:PWR"
    )

    assert module_group.source == "hierarchy"
    assert "PWR/U1" in module_group.references
    assert module_group.anchor in module_group.references


def test_pad_coordinates_and_orientation_drive_wirelength_metrics() -> None:
    footprint = PhysicalFootprint(
        "test/two-pad",
        (
            FootprintPad("1", Point.mm(-1, 0), Size.mm(1, 1)),
            FootprintPad("2", Point.mm(1, 0), Size.mm(1, 1)),
        ),
        Size.mm(3, 2),
    )
    board = PhysicalBoard(
        "PadAware",
        BoardOutline.rectangle(30, 20),
        {footprint.name: footprint},
        (
            Placement("R1", footprint.name, Point.mm(5, 10)),
            Placement("R2", footprint.name, Point.mm(20, 10)),
        ),
        (
            PhysicalNet(
                "N",
                (PadReference("R1", "2"), PadReference("R2", "1")),
            ),
        ),
    )
    placements = {item.reference: item for item in board.placements}
    options = PlacementPlannerOptions(candidate_count=1)

    assert transformed_pad_position(board, placements["R1"], "2") == Point.mm(6, 10)
    unrotated = placement_metrics(board, placements, options)
    rotated = dict(placements)
    rotated["R1"] = replace(placements["R1"], rotation_degrees=180)
    rotated_metrics = placement_metrics(board, rotated, options)

    assert unrotated.half_perimeter_wire_length_nm == nm_from_mm(13)
    assert rotated_metrics.half_perimeter_wire_length_nm == nm_from_mm(15)


def test_layer_aware_coarse_router_reports_same_layer_crossings() -> None:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point.mm(0, 0), Size.mm(0.5, 0.5)),),
        Size.mm(1, 1),
    )
    board = PhysicalBoard(
        "CrossingEstimate",
        BoardOutline.rectangle(30, 30),
        {footprint.name: footprint},
        (
            Placement("L", footprint.name, Point.mm(5, 15)),
            Placement("R", footprint.name, Point.mm(25, 15)),
            Placement("T", footprint.name, Point.mm(15, 5)),
            Placement("B", footprint.name, Point.mm(15, 25)),
        ),
        (
            PhysicalNet("H", (PadReference("L", "1"), PadReference("R", "1"))),
            PhysicalNet("V", (PadReference("T", "1"), PadReference("B", "1"))),
        ),
        stackup=Stackup((CopperLayer.FRONT,)),
    )

    metrics = placement_metrics(
        board,
        {item.reference: item for item in board.placements},
        PlacementPlannerOptions(candidate_count=1),
    )

    assert metrics.crossing_count >= 1
    assert metrics.estimated_via_count >= 1


def test_analytical_phase_improves_the_valid_example_seed() -> None:
    physical = _prototype_board()
    baseline = plan_placement(
        physical,
        PlacementPlannerOptions(
            candidate_count=1,
            analytical_iterations=0,
            refinement_passes=0,
        ),
    )
    analytical = plan_placement(
        physical,
        PlacementPlannerOptions(
            candidate_count=1,
            analytical_iterations=40,
            refinement_passes=0,
        ),
    )

    assert (
        analytical.report.metrics.half_perimeter_wire_length_nm
        < baseline.report.metrics.half_perimeter_wire_length_nm
    )


def test_hybrid_legalizer_records_bounded_exact_repair(monkeypatch) -> None:
    import pcbir.placement as engine

    original = engine._best_legal_choice
    forced = 0

    def force_one_repair(*args, **kwargs):
        nonlocal forced
        placed = args[3]
        if not placed:
            forced += 1
            return None
        return original(*args, **kwargs)

    monkeypatch.setattr(engine, "_best_legal_choice", force_one_repair)
    plan = plan_placement(
        _prototype_board(),
        PlacementPlannerOptions(
            candidate_count=1,
            analytical_iterations=0,
            refinement_passes=0,
        ),
    )

    assert forced >= 1
    assert any(candidate.statistics.exact_repair_count for candidate in plan.candidates)


def test_candidate_frontier_and_refinement_are_deterministic() -> None:
    options = PlacementPlannerOptions(
        candidate_count=3,
        analytical_iterations=8,
        refinement_passes=1,
    )

    first = plan_placement(_prototype_board(), options)
    second = plan_placement(_prototype_board(), options)

    assert first == second
    assert 1 <= len(first.candidates) <= 3
    assert all(
        candidate.statistics.routing_feedback_passes >= 1
        for candidate in first.candidates
    )
    vectors = [candidate.metrics.quality_vector for candidate in first.candidates]
    assert all(
        not (
            all(left <= right for left, right in zip(other, vector, strict=True))
            and any(left < right for left, right in zip(other, vector, strict=True))
        )
        for index, vector in enumerate(vectors)
        for other in vectors[:index] + vectors[index + 1 :]
    )


def test_source_regions_keepouts_fixed_positions_and_proximity_are_enforced() -> None:
    board = compile_source(
        """
        board ConstrainedPlacement {
            use library "tiny";
            component R1: RESISTOR { footprint = "0402"; }
            component R2: RESISTOR { footprint = "0402"; }
            constraint placement_region(R1) {
                name = "left"; x = 2mm; y = 2mm; width = 16mm; height = 16mm;
            }
            constraint fixed_placement(R2) {
                x = 22mm; y = 10mm; rotation = 0; side = "front";
            }
            constraint max_distance(R1, R2.1) { distance = 8mm; }
            constraint keepout() {
                name = "hole"; x = 8mm; y = 8mm; width = 3mm; height = 3mm;
            }
        }
        """
    )
    physical = prototype_physicalize(
        board,
        PrototypePhysicalOptions(board_width_mm=30, board_height_mm=22, margin_mm=3),
    )

    plan = plan_placement(
        physical,
        PlacementPlannerOptions(candidate_count=1, analytical_iterations=10),
    )
    placements = {item.reference: item for item in plan.board.placements}

    assert placements["R2"].position == Point.mm(22, 10)
    assert 2_000_000 <= placements["R1"].position.x_nm <= 18_000_000
    assert plan.report.metrics.constraint_penalty_nm == 0
    assert plan.report.metrics.minimum_constraint_margin_nm >= 0


def test_relative_repair_uses_exact_pad_coordinates_off_the_placement_grid() -> None:
    anchor = PhysicalFootprint(
        "test/anchor",
        (FootprintPad("1", Point.mm("2.333", "0.222"), Size.mm("0.1", "0.1")),),
        Size.mm(1, 1),
    )
    satellite = PhysicalFootprint(
        "test/satellite",
        (FootprintPad("1", Point(0, 0), Size.mm("0.1", "0.1")),),
        Size.mm("0.2", "0.2"),
    )
    board = PhysicalBoard(
        "OffGridPadConstraint",
        BoardOutline.rectangle(25, 20),
        {anchor.name: anchor, satellite.name: satellite},
        (
            Placement("U1", anchor.name, Point.mm(10, 10)),
            Placement("C1", satellite.name, Point.mm(3, 3)),
        ),
        (),
        relative_rules=(
            RelativePlacementRule(
                RelativePlacementKind.MAX_DISTANCE,
                (PlacementTarget("C1"), PlacementTarget("U1", "1")),
                distance_nm=nm_from_mm("0.1"),
            ),
        ),
    )

    plan = plan_placement(
        board,
        PlacementPlannerOptions(
            candidate_count=1,
            analytical_iterations=0,
            refinement_passes=0,
            fixed_references=frozenset({"U1"}),
        ),
    )

    assert plan.report.metrics.constraint_penalty_nm == 0
    assert plan.report.metrics.minimum_constraint_margin_nm >= 0

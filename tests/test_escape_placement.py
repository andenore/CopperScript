from dataclasses import replace
import json

import pytest

from pcbir import (BoardOutline, BoardSide, ComponentPlacementRule, CopperLayer,
    DesignRules, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, PlacementPlannerOptions, PlacementTarget, Point,
    RelativePlacementKind, RelativePlacementRule, Size, Stackup, nm_from_mm,
    placement_metrics, placement_solution_is_legal, plan_placement)
from pcbir.physical import NetRoutingRule, PadKind, PlacementRegion
from pcbir.physical import RigidPlacementCluster, RigidPlacementMember
from pcbir.clusters import cluster_placement_matches, cluster_placements, footprint_geometry_digest
from pcbir.placement import _escape_model, _fast_score, _spread_escape_components
from pcbir.placement_escape import EscapeSide, EscapeSpacingModel


def fixture(companion=False):
    pads = []
    for side in range(4):
        for index in range(16):
            tangent = (index - 7.5) * .5
            x, y = ((tangent, -5.75), (5.75, tangent), (tangent, 5.75), (-5.75, tangent))[side]
            pads.append(FootprintPad(str(side*16+index+1), Point.mm(x, y),
                Size.mm(.25, 1.5) if side % 2 == 0 else Size.mm(1.5, .25)))
    large = PhysicalFootprint("large", tuple(pads), Size.mm(13.4, 13.4))
    pads = []
    for side in range(4):
        for index in range(3):
            tangent = (index - 1) * .5
            x, y = ((tangent, -.9), (.9, tangent), (tangent, .9), (-.9, tangent))[side]
            pads.append(FootprintPad(str(side*3+index+1), Point.mm(x, y), Size.mm(.28, .28)))
    small = PhysicalFootprint("small", tuple(pads), Size.mm(2.5, 2.5))
    cap = PhysicalFootprint("cap", (FootprintPad("1", Point.mm(-.35, 0), Size.mm(.3, .3)),
                                  FootprintPad("2", Point.mm(.35, 0), Size.mm(.3, .3))), Size.mm(1.2, .8))
    placements = [Placement("U_MCU", "large", Point.mm(20, 15)),
                  Placement("U_SENSOR", "small", Point.mm(20, 24))]
    relatives = ()
    if companion:
        placements.append(Placement("C1", "cap", Point.mm(17.5, 24)))
        relatives = (RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget("U_SENSOR"), PlacementTarget("C1")), distance_nm=nm_from_mm(3)),)
    return PhysicalBoard("escape", BoardOutline.rectangle(60, 45),
        {f.name: f for f in (large, small, cap)}, tuple(placements),
        (PhysicalNet("GND", tuple(PadReference(p.reference, pad.number)
            for p in placements for pad in {"large": large, "small": small, "cap": cap}[p.footprint].pads)),),
        relative_rules=relatives, rules=DesignRules(minimum_clearance_nm=nm_from_mm(.09),
            minimum_track_width_nm=nm_from_mm(.09), default_track_width_nm=nm_from_mm(.2)))


def poses(board):
    return {p.reference: p for p in board.placements}


def options(**changes):
    return PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
        refinement_passes=0, **changes)


def test_dense_small_pair_has_a_directional_channel_and_both_via_banks():
    board = fixture()
    model = EscapeSpacingModel(board)
    channels = model.channels(poses(board))
    assert len(channels) == 1
    channel = channels[0]
    assert channel.axis == "y"
    assert channel.left_side is EscapeSide.SOUTH and channel.right_side is EscapeSide.NORTH
    assert channel.left_depth_nm > 0 and channel.right_depth_nm > 0
    assert channel.left_rows >= 1 and channel.right_rows >= 1
    assert channel.required_gap_nm > nm_from_mm(2.16)
    assert channel.deficit_nm > 0


def test_spreading_preserves_fixed_mcu_and_moves_sensor_with_its_decoupler():
    board = fixture(companion=True)
    source = poses(board)
    opts = options(fixed_references=frozenset({"U_MCU"}))
    assert placement_solution_is_legal(board, source, opts)
    result, moved = _spread_escape_components(board, source, opts)
    assert moved and result["U_MCU"] == source["U_MCU"]
    assert placement_solution_is_legal(board, result, opts)
    assert not _escape_model(board, opts).channels(result)
    for axis in ("x_nm", "y_nm"):
        assert (getattr(result["U_SENSOR"].position, axis)-getattr(source["U_SENSOR"].position, axis)
                == getattr(result["C1"].position, axis)-getattr(source["C1"].position, axis))


def test_locked_short_channel_warns_instead_of_rejecting_or_unlocking():
    board = fixture()
    board = replace(board, placement_rules=tuple(ComponentPlacementRule(p.reference,
        fixed_position=p.position, fixed_rotation_degrees=p.rotation_degrees) for p in board.placements))
    plan = plan_placement(board, options())
    assert plan.board.placements == board.placements
    assert plan.report.metrics.escape_channel_deficit_nm > 0
    assert any(f.code == "ESCAPE_CHANNEL_DEFICIT" for f in plan.report.findings)
    report = json.loads(plan.report.to_json())
    channel = report["escape_channels"][0]
    assert channel["left"] == "U_MCU" and channel["right"] == "U_SENSOR"
    assert channel["deficit_nm"] == channel["required_gap_nm"] - channel["gap_nm"]
    assert plan.board.metadata["fabrication_ready"] == "false"


def test_escape_deficit_precedes_short_wire_length_in_local_and_final_scores():
    board = fixture()
    original = poses(board)
    spread = {**original, "U_SENSOR": replace(original["U_SENSOR"], position=Point.mm(20, 30))}
    assert _fast_score(board, spread) < _fast_score(board, original)
    assert placement_metrics(board, spread, options()).quality_vector < placement_metrics(board, original, options()).quality_vector


def test_no_net_no_demand_and_more_layers_do_not_erase_landing_space():
    board = fixture()
    assert not EscapeSpacingModel(replace(board, nets=())).channels(poses(board))
    more = replace(board, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)))
    assert EscapeSpacingModel(board).channels(poses(board)) == EscapeSpacingModel(more).channels(poses(more))


def test_via_size_drill_spacing_margin_and_transit_lanes_change_target():
    board = fixture()
    base = EscapeSpacingModel(board).channels(poses(board))[0]
    larger = replace(board, rules=replace(board.rules, default_via_size_nm=nm_from_mm(1.2)))
    assert EscapeSpacingModel(larger).channels(poses(larger))[0].required_gap_nm > base.required_gap_nm
    drill = replace(board, rules=replace(board.rules, minimum_hole_clearance_nm=nm_from_mm(1)))
    assert EscapeSpacingModel(drill).channels(poses(drill))[0].required_gap_nm > base.required_gap_nm
    assert EscapeSpacingModel(board, margin_nm=nm_from_mm(1)).channels(poses(board))[0].required_gap_nm > base.required_gap_nm
    assert EscapeSpacingModel(board, transit_lanes=2).channels(poses(board))[0].required_gap_nm > base.required_gap_nm


def test_surface_only_restriction_does_not_invent_via_banks():
    board = replace(fixture(), net_routing_rules=(NetRoutingRule("GND",
        allowed_layers=(CopperLayer.FRONT,), max_vias=0),))
    model = EscapeSpacingModel(board)
    channel = model.channels(poses(board), include_clear=True)[0]
    assert channel.left_rows == channel.right_rows == 0
    assert channel.required_gap_nm > 0


def test_opposite_board_sides_are_not_a_surface_escape_channel():
    board = fixture()
    positions = poses(board)
    positions["U_SENSOR"] = replace(positions["U_SENSOR"], side=BoardSide.BACK)
    assert not EscapeSpacingModel(board).channels(positions)


def test_profile_rotations_and_mirrors_are_cached_without_xy_or_stale_rules():
    board = fixture()
    model = EscapeSpacingModel(board)
    pose = board.placements[1]
    local = model.profile(pose)
    assert model.profile(replace(pose, position=Point.mm(35, 35))) is local
    rotated = model.profile(replace(pose, rotation_degrees=45, side=BoardSide.BACK))
    assert rotated is not local and rotated.bounds.max_x > local.bounds.max_x
    with pytest.raises(TypeError):
        rotated.sides[EscapeSide.NORTH] = local.sides[EscapeSide.NORTH]
    changed = replace(board, rules=replace(board.rules, default_via_size_nm=nm_from_mm(1.2)))
    assert EscapeSpacingModel(changed).profile(pose).sides != local.sides


def test_passive_is_a_blocker_without_its_own_bank_but_companion_is_exempt():
    board = fixture(companion=True)
    source = poses(board)
    source["C1"] = replace(source["C1"], position=Point.mm(20, 22.5))
    independent = replace(board, relative_rules=())
    channels = EscapeSpacingModel(independent).channels(source)
    assert any(c.right == "U_MCU" or c.left == "U_MCU" for c in channels)
    assert all(d.pad_count == 0 for d in EscapeSpacingModel(board).profile(source["C1"]).sides.values())
    assert not any({c.left, c.right} == {"C1", "U_SENSOR"} for c in EscapeSpacingModel(board).channels(source))


def test_spreading_is_bounded_disabled_and_deterministic():
    board = fixture()
    source = poses(board)
    assert _spread_escape_components(board, source, options(escape_spacing_passes=0)) == (source, 0)
    assert _spread_escape_components(board, source, options()) == _spread_escape_components(board, source, options())
    tight = options(escape_max_movement_nm=nm_from_mm(.2), fixed_references=frozenset({"U_MCU"}))
    result, count = _spread_escape_components(board, source, tight)
    for ref in source:
        assert abs(result[ref].position.x_nm-source[ref].position.x_nm) <= count*nm_from_mm(.2)
        assert abs(result[ref].position.y_nm-source[ref].position.y_nm) <= count*nm_from_mm(.2)


def test_pair_cache_matches_fresh_models_after_pose_changes_and_reverts():
    board = fixture(companion=True)
    model = EscapeSpacingModel(board)
    initial = poses(board)
    trials = [initial, {**initial, "U_SENSOR": replace(initial["U_SENSOR"], position=Point.mm(35, 30))},
              {**initial, "U_SENSOR": replace(initial["U_SENSOR"], rotation_degrees=45)},
              {**initial, "U_SENSOR": replace(initial["U_SENSOR"], side=BoardSide.BACK)}, initial]
    for trial in trials * 2:
        for include_clear in (False, True):
            assert model.channels(trial, include_clear=include_clear) == EscapeSpacingModel(board).channels(
                trial, include_clear=include_clear)
    assert len(model._positioned) == len(initial)
    assert len(model._pairs) <= len(initial)*(len(initial)-1)


@pytest.mark.parametrize("angle", [0, 45])
def test_spacing_moves_whole_rigid_unit_without_changing_member_poses(angle):
    board = fixture(companion=True)
    initial = poses(board)
    cluster = RigidPlacementCluster("sensor-macro", PlacementTarget("U_SENSOR"), tuple(
        RigidPlacementMember(ref, initial[ref].footprint,
            footprint_geometry_digest(board.footprints[initial[ref].footprint]),
            Point(initial[ref].position.x_nm-initial["U_SENSOR"].position.x_nm,
                  initial[ref].position.y_nm-initial["U_SENSOR"].position.y_nm))
        for ref in ("U_SENSOR", "C1")), "synthetic placement regression", allowed_rotations=(0, 45))
    board = replace(board, rigid_clusters=(cluster,), placement_rules=tuple(
        ComponentPlacementRule(ref, allowed_orientations=(0, 45, 90)) for ref in ("U_SENSOR", "C1")))
    initial.update(cluster_placements(board, cluster, replace(initial["U_SENSOR"], rotation_degrees=angle)))
    opts = options(fixed_references=frozenset({"U_MCU"}))
    assert placement_solution_is_legal(board, initial, opts)
    model = EscapeSpacingModel(board)
    assert not any({c.left, c.right} == {"U_SENSOR", "C1"} for c in model.channels(initial, include_clear=True))
    result, moved = _spread_escape_components(board, initial, opts, model)
    assert moved and result["U_MCU"] == initial["U_MCU"]
    assert cluster_placement_matches(board, result)
    assert placement_solution_is_legal(board, result, opts)
    assert not model.channels(result)
    assert all(result[ref].rotation_degrees == initial[ref].rotation_degrees for ref in result)


def test_through_hole_pads_do_not_require_surface_via_banks():
    board = fixture()
    footprints = {name: replace(fp, pads=tuple(replace(pad,
        kind=PadKind.THROUGH_HOLE, drill=Size.mm(.1, .1)) for pad in fp.pads))
        for name, fp in board.footprints.items()}
    board = replace(board, footprints=footprints)
    assert not EscapeSpacingModel(board).channels(poses(board))


def test_tight_region_retains_unmet_spacing_without_breaking_hard_legality():
    board = fixture()
    board = replace(board, regions=(PlacementRegion("sensor-window",
        BoardOutline.rectangle(2.5, 2.5, origin=Point.mm(18.75, 22.75))),),
        placement_rules=(ComponentPlacementRule("U_SENSOR", region="sensor-window", allowed_orientations=(0,)),))
    source = poses(board)
    opts = options(fixed_references=frozenset({"U_MCU"}))
    result, moved = _spread_escape_components(board, source, opts)
    assert result == source and not moved
    assert placement_solution_is_legal(board, result, opts)
    assert _escape_model(board, opts).channels(result)


def test_spreading_rejects_coarse_congestion_regression(monkeypatch):
    from types import SimpleNamespace
    board = fixture()
    source = poses(board)
    monkeypatch.setattr("pcbir.placement._coarse_route", lambda board, placements, options:
        SimpleNamespace(overflow=0 if placements == source else 1))
    result, moved = _spread_escape_components(board, source, options(fixed_references=frozenset({"U_MCU"})))
    assert result == source and not moved


@pytest.mark.parametrize("change", [{"escape_margin_nm": -1}, {"escape_transit_lanes": -1},
    {"escape_spacing_passes": -1}, {"escape_max_movement_nm": 0}])
def test_invalid_escape_options_fail(change):
    with pytest.raises(ValueError, match="escape"):
        options(**change)


def local_companion_board():
    chip = PhysicalFootprint("chip", (
        FootprintPad("1", Point.mm(1, -.6), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(1, .6), Size.mm(.3, .3))), Size.mm(2, 2))
    cap = PhysicalFootprint("capacitor", (
        FootprintPad("1", Point.mm(-.3, 0), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(.3, 0), Size.mm(.3, .3))), Size.mm(1, .5))
    board = PhysicalBoard("local", BoardOutline.rectangle(30, 30),
        {fp.name: fp for fp in (chip, cap)}, (
            Placement("U", chip.name, Point.mm(10, 10)),
            Placement("C1", cap.name, Point.mm(20, 20)),
            Placement("C2", cap.name, Point.mm(23, 20))), (),
        placement_rules=(ComponentPlacementRule("U", fixed_position=Point.mm(10, 10),
                                               fixed_rotation_degrees=0),),
        relative_rules=tuple(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget(f"C{i}", "1"), PlacementTarget("U", str(i))),
            distance_nm=nm_from_mm(1.5)) for i in (1, 2)))
    return board


def test_fixed_anchor_companions_share_order_but_stay_flexible_and_legal():
    from pcbir.placement import _relative_cluster_order
    board = local_companion_board()
    outsider = Placement("X", "chip", Point.mm(12, 10))
    board = replace(board, placements=(*board.placements, outsider))
    order = _relative_cluster_order(board, ["X", "C2", "C1"], poses(board))
    assert order == ["C1", "C2", "X"]
    first = plan_placement(board, options())
    second = plan_placement(board, options())
    assert first == second
    assert poses(first.board)["U"] == poses(board)["U"]
    assert placement_solution_is_legal(first.board, poses(first.board), options())
    # Flexible proximity does not preserve the arbitrary original separation.
    assert (poses(first.board)["C2"].position.x_nm - poses(first.board)["C1"].position.x_nm
            != poses(board)["C2"].position.x_nm - poses(board)["C1"].position.x_nm)


def test_minimum_distance_and_alignment_do_not_merge_local_units():
    from pcbir.physical import AlignmentAxis
    from pcbir.placement import _relative_cluster_order
    from pcbir.placement_escape import placement_units
    board = local_companion_board()
    board = replace(board, relative_rules=(
        RelativePlacementRule(RelativePlacementKind.MIN_DISTANCE,
            (PlacementTarget("C1"), PlacementTarget("U")), distance_nm=nm_from_mm(2)),
        RelativePlacementRule(RelativePlacementKind.ALIGN,
            (PlacementTarget("C2"), PlacementTarget("U")), axis=AlignmentAxis.X)))
    assert placement_units(board)["C1"] == frozenset({"C1"})
    assert placement_units(board)["C2"] == frozenset({"C2"})
    assert _relative_cluster_order(board, ["C2", "C1"], poses(board)) == ["C2", "C1"]


@pytest.mark.parametrize("slot_left, slot_right", [(11.66, 12.70), (11.72, 12.74)])
def test_local_xy_candidates_find_off_grid_slot_missed_by_eight_rays(slot_left, slot_right):
    board = local_companion_board()
    chip = replace(board.footprints["chip"], pads=(
        replace(board.footprints["chip"].pads[0], position=Point.mm(1, 0)),))
    board = replace(board, footprints={**board.footprints, "chip": chip},
        placements=(replace(board.placements[0], position=Point.mm(10.13, 10.27)), board.placements[1]),
        placement_rules=(ComponentPlacementRule("U", fixed_position=Point.mm(10.13, 10.27),
                                               fixed_rotation_degrees=0),
                         ComponentPlacementRule("C1", region="slot", fixed_rotation_degrees=0)),
        regions=(PlacementRegion("slot", BoardOutline((Point.mm(slot_left, 10.64), Point.mm(slot_right, 10.64),
                                                        Point.mm(slot_right, 11.29), Point.mm(slot_left, 11.29)))),),
        relative_rules=(replace(board.relative_rules[0], distance_nm=nm_from_mm(1.1)),))
    plan = plan_placement(board, options())
    assert placement_solution_is_legal(plan.board, poses(plan.board), options())
    cap = poses(plan.board)["C1"]
    assert cap.position.x_nm % nm_from_mm(1) != 0
    assert cap.position.y_nm % nm_from_mm(1) != 0


def test_pad_distance_prefers_facing_pin_even_with_identical_global_wirelength():
    from pcbir.placement import _hpwl, _choose_orientations, _local_connection_length
    board = local_companion_board()
    board = replace(board, placements=(board.placements[0], replace(board.placements[1],
        position=Point.mm(12.1, 9.4), rotation_degrees=180),
        Placement("P", "capacitor", Point.mm(5, 5)), Placement("Q", "capacitor", Point.mm(25, 25))),
        relative_rules=(replace(board.relative_rules[0], distance_nm=nm_from_mm(2)),),
        nets=(PhysicalNet("POWER", tuple(PadReference(ref, "1") for ref in ("U", "C1", "P", "Q"))),))
    original = poses(board)
    facing = {**original, "C1": replace(original["C1"], rotation_degrees=0)}
    assert _hpwl(board, facing) == _hpwl(board, original)
    assert _local_connection_length(board, facing) < _local_connection_length(board, original)
    assert _fast_score(board, facing) < _fast_score(board, original)
    assert placement_metrics(board, facing, options()).quality_vector < placement_metrics(board, original, options()).quality_vector
    assert _choose_orientations(board, original, {"U", "P", "Q"})["C1"].rotation_degrees == 0


def test_shared_companion_never_translates_either_fixed_anchor():
    board = local_companion_board()
    chip = PhysicalFootprint("small-chip", (FootprintPad("1", Point(0, 0), Size.mm(.2, .2)),), Size.mm(1, 1))
    board = replace(board, footprints={**board.footprints, chip.name: chip},
        placements=(Placement("U", chip.name, Point.mm(8, 10)), board.placements[1],
                    Placement("V", chip.name, Point.mm(12, 10))),
        placement_rules=tuple(ComponentPlacementRule(ref, fixed_position=Point.mm(x, 10),
            fixed_rotation_degrees=0) for ref, x in (("U", 8), ("V", 12))),
        relative_rules=tuple(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget("C1", str(i)), PlacementTarget(ref, "1")), distance_nm=nm_from_mm(2.2))
            for i, ref in enumerate(("U", "V"), 1)))
    result = plan_placement(board, options()).board
    assert placement_solution_is_legal(result, poses(result), options())
    assert poses(result)["U"] == poses(board)["U"]
    assert poses(result)["V"] == poses(board)["V"]


@pytest.mark.parametrize("unrelated_failure", [False, True])
def test_local_repair_moves_actual_blocker_instead_of_unrelated_recent_component(unrelated_failure):
    from pcbir.placement import _exact_repair, _adjacency
    board = local_companion_board()
    board = replace(board, placements=(board.placements[0], board.placements[1],
        Placement("X", "capacitor", Point.mm(12, 9.4)),
        Placement("Y", "capacitor", Point.mm(20, 20))),
        relative_rules=(board.relative_rules[0],),
        placement_rules=(*board.placement_rules, ComponentPlacementRule("C1", region="slot")),
        regions=(PlacementRegion("slot", BoardOutline((Point.mm(11.4, 8.9), Point.mm(12.6, 8.9),
                                                        Point.mm(12.6, 9.9), Point.mm(11.4, 9.9)))),))
    if unrelated_failure:
        board = replace(board,
            placements=(*board.placements, Placement("Z", "chip", Point.mm(22, 10))),
            placement_rules=(*board.placement_rules,
                ComponentPlacementRule("Z", fixed_position=Point.mm(22, 10), fixed_rotation_degrees=0)),
            relative_rules=(*board.relative_rules,
                RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
                    (PlacementTarget("Y", "1"), PlacementTarget("Z", "1")), distance_nm=nm_from_mm(3))))
    targets = poses(board)
    placed = {ref: pose for ref, pose in targets.items() if ref != "C1"}
    opts = options(exact_repair_limit=2)
    result = _exact_repair(board, "C1", placed, ["X", "Y"], targets, _adjacency(board), opts, 0)
    assert result is not None
    assert result["U"] == targets["U"] and result["Y"] == targets["Y"]
    assert result["X"] != targets["X"]
    assert placement_solution_is_legal(board, result, opts) is not unrelated_failure


def test_fixed_anchor_companion_is_reserved_before_unrelated_movable_macro():
    from pcbir.cluster_placement import place_rigid_clusters
    from pcbir.placement import _legal
    board = local_companion_board()
    board = replace(board, placements=(board.placements[0], board.placements[1],
        Placement("M1", "capacitor", Point.mm(12, 9.4)),
        Placement("M2", "capacitor", Point.mm(14, 9.4))), relative_rules=(board.relative_rules[0],))
    fp = board.footprints["capacitor"]
    cluster = RigidPlacementCluster("unrelated", PlacementTarget("M1"), tuple(
        RigidPlacementMember(ref, fp.name, footprint_geometry_digest(fp), Point.mm(x, 0))
        for ref, x in (("M1", 0), ("M2", 2))), "synthetic regression", allowed_rotations=(0,))
    board = replace(board, rigid_clusters=(cluster,))
    targets, original, phase_options = place_rigid_clusters(board, poses(board), poses(board), options())
    assert targets["U"] == poses(board)["U"]
    assert _legal(targets["C1"], {ref: targets[ref] for ref in ("U", "M1", "M2")}, board, options())
    assert targets["M1"] != poses(board)["M1"]
    assert "C1" not in phase_options.fixed_references  # Proximity remains flexible.
    assert {"M1", "M2"} <= phase_options.fixed_references
    assert cluster_placement_matches(board, targets)
    assert placement_solution_is_legal(board, targets, options())


def test_failed_local_relation_reports_measured_limit_and_blockers_without_moving_lock():
    from pcbir import PlacementPlanningError
    board = local_companion_board()
    board = replace(board, placements=board.placements[:2],
                    relative_rules=(replace(board.relative_rules[0], distance_nm=nm_from_mm(.1)),))
    with pytest.raises(PlacementPlanningError, match="maximum distance 100000 nm, actual.*local candidate blockers: U"):
        plan_placement(board, options())
    assert board.placements[0].position == Point.mm(10, 10)


@pytest.mark.parametrize("movable_macro", [False, True])
def test_joint_packing_reserves_the_only_slot_for_the_smallest_feasible_domain(movable_macro):
    from pcbir.placement import _best_legal_choice, _adjacency, _legal
    board = local_companion_board()
    board = replace(board,
        relative_rules=tuple(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget(ref, "1"), PlacementTarget("U", "1")), distance_nm=nm_from_mm(2))
            for ref in ("C1", "C2")),
        placement_rules=(*board.placement_rules,
            ComponentPlacementRule("C2", region="only-slot", fixed_rotation_degrees=0)),
        regions=(PlacementRegion("only-slot", BoardOutline((Point.mm(11.5, 9.15), Point.mm(12.5, 9.15),
                Point.mm(12.5, 9.65), Point.mm(11.5, 9.65)))),))
    if movable_macro:
        fp = board.footprints["capacitor"]
        board = replace(board, placements=(*board.placements,
            Placement("M1", fp.name, Point.mm(22, 22)),
            Placement("M2", fp.name, Point.mm(24, 22))),
            rigid_clusters=(RigidPlacementCluster("unrelated", PlacementTarget("M1"), tuple(
                RigidPlacementMember(ref, fp.name, footprint_geometry_digest(fp), Point.mm(x, 0))
                for ref, x in (("M1", 0), ("M2", 2))), "synthetic regression", allowed_rotations=(0,)),))
    initial = poses(board)
    opts = options()
    greedy = _best_legal_choice(board, "C1", initial["C1"], {"U": initial["U"]}, initial,
                               _adjacency(board), opts, 0, limit=opts.legalization_candidates)
    only_pose = replace(initial["C2"], position=Point.mm(12, 9.4), rotation_degrees=0)
    assert not _legal(only_pose, {"C1": greedy, "U": initial["U"]}, board, opts)
    plan = plan_placement(board, opts)
    assert poses(plan.board)["C2"] == only_pose
    assert poses(plan.board)["U"] == initial["U"]
    assert placement_solution_is_legal(plan.board, poses(plan.board), opts)
    assert cluster_placement_matches(plan.board, poses(plan.board))
    assert plan.candidates[0].statistics.exact_repair_count > 0
    assert plan == plan_placement(board, opts)


def test_exhausted_large_anchor_pack_retries_independent_close_pair(monkeypatch):
    import pcbir.placement as engine
    board = local_companion_board()
    extras = tuple(Placement(f"C{i}", "capacitor", Point.mm(17+i, 20)) for i in range(3, 8))
    board = replace(board, placements=(*board.placements, *extras),
        relative_rules=(
            *(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
                (PlacementTarget(ref, "1"), PlacementTarget("U", "1")),
                distance_nm=nm_from_mm(2 if ref in {"C1", "C2"} else 20))
              for ref in ("C1", "C2", *(p.reference for p in extras))),
            RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
                (PlacementTarget("C1", "1"), PlacementTarget("C2", "1")),
                distance_nm=nm_from_mm(3))),
        placement_rules=(*board.placement_rules,
            ComponentPlacementRule("C2", region="only-slot", fixed_rotation_degrees=0)),
        regions=(PlacementRegion("only-slot", BoardOutline((Point.mm(11.5, 9.15), Point.mm(12.5, 9.15),
            Point.mm(12.5, 9.65), Point.mm(11.5, 9.65)))),))
    pack = engine._pack_local_components
    attempted = []
    def bounded(*args, **kwargs):
        attempted.append(tuple(args[1]))
        return None if len(args[1]) > 6 else pack(*args, **kwargs)
    monkeypatch.setattr(engine, "_pack_local_components", bounded)
    plan = plan_placement(board, options())
    assert any(len(group) > 6 for group in attempted)
    assert any(set(group) == {"C1", "C2"} for group in attempted)
    assert placement_solution_is_legal(plan.board, poses(plan.board), options())


def test_impossible_local_placement_bounds_illegal_probes(monkeypatch):
    import pcbir.placement as engine
    from itertools import repeat
    board = local_companion_board()
    initial = poses(board)
    opts = options(legalization_candidates=2)
    calls = 0

    def illegal(*args):
        nonlocal calls
        calls += 1
        assert calls <= 256, "an impossible local relation must not exhaust the board grid"
        return False

    monkeypatch.setattr(engine, "_legal", illegal)
    monkeypatch.setattr(engine, "_relative_candidates", lambda *args, **kwargs: ())
    monkeypatch.setattr(engine, "_candidate_positions", lambda *args: repeat(Point.mm(10, 10)))
    assert engine._best_legal_choice(board, "C1", initial["C1"], {"U": initial["U"]},
        initial, engine._adjacency(board), opts, 0, limit=2) is None
    assert calls > 0


def test_joint_packing_refines_a_narrow_row_when_individual_coarse_poses_conflict():
    import pcbir.placement as engine
    board = local_companion_board()
    board = replace(board,
        regions=tuple(PlacementRegion(ref, BoardOutline((
            Point.mm(11.55, low-.25), Point.mm(12.55, low-.25),
            Point.mm(12.55, high+.25), Point.mm(11.55, high+.25))))
            for ref, low, high in (("C1", 9.55, 9.70), ("C2", 10.50, 10.65))),
        placement_rules=(*board.placement_rules, *(ComponentPlacementRule(
            ref, region=ref, fixed_rotation_degrees=0) for ref in ("C1", "C2"))))
    initial, opts = poses(board), options()
    fixed = {"U": initial["U"]}
    coarse = {ref: tuple(p for p in engine._relative_candidates(board, ref, initial[ref], fixed, opts)
                        if engine._legal(p, fixed, board, opts)) for ref in ("C1", "C2")}
    assert coarse["C1"] and coarse["C2"]
    assert not any(engine._legal(right, {**fixed, "C1": left}, board, opts)
                   for left in coarse["C1"] for right in coarse["C2"])
    result = engine._pack_local_components(board, ["C1", "C2"], fixed, initial, opts, allow_general=False)
    assert result is not None and result["U"] == initial["U"]
    assert placement_solution_is_legal(board, result, opts)


@pytest.mark.parametrize("intermediate_pad", ["1", "2"])
def test_anchored_neighborhood_only_composes_identical_pad_targets(intermediate_pad):
    from pcbir.placement import _anchored_candidate_rules
    board = local_companion_board()
    board = replace(board, relative_rules=(board.relative_rules[0],
        RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget("C2", "1"), PlacementTarget("C1", intermediate_pad)),
            distance_nm=nm_from_mm(1.5))))
    derived = _anchored_candidate_rules(board, {"U"})
    if intermediate_pad == "1":
        assert derived == (RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget("C2", "1"), PlacementTarget("U", "1")), distance_nm=nm_from_mm(3)),)
    else:
        assert derived == ()  # Distinct pins cannot be treated as the same point.
    assert derived == _anchored_candidate_rules(replace(board, relative_rules=board.relative_rules[::-1]), {"U"})


def test_indirect_companion_joins_anchor_pack_and_keeps_exact_pair_distance():
    board = local_companion_board()
    board = replace(board, relative_rules=(board.relative_rules[0],
        RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget("C2", "1"), PlacementTarget("C1", "1")), distance_nm=nm_from_mm(1.5))))
    plan = plan_placement(board, options())
    assert plan.candidates[0].statistics.exact_repair_count > 0
    assert placement_solution_is_legal(plan.board, poses(plan.board), options())
    assert plan.board.relative_rules == board.relative_rules  # Candidate reach never becomes a source constraint.
    assert poses(plan.board)["U"] == poses(board)["U"]


def test_joint_search_retains_search_budget_when_lookahead_is_exhausted(monkeypatch):
    import pcbir.placement as engine
    board = local_companion_board()
    references = [f"C{i}" for i in range(1, 5)]
    board = replace(board,
        placements=(board.placements[0], *(replace(board.placements[1], reference=ref) for ref in references)),
        relative_rules=tuple(RelativePlacementRule(RelativePlacementKind.MAX_DISTANCE,
            (PlacementTarget(ref, "1"), PlacementTarget("U", "1")), distance_nm=nm_from_mm(20))
            for ref in references))
    initial = poses(board)
    # Four parts have four interchangeable legal slots. Ranking every slot at
    # every depth must not consume the budget for this simple feasible packing.
    monkeypatch.setattr(engine, "_relative_candidates", lambda board, ref, pose, *args, **kwargs:
        tuple(replace(pose, position=Point.mm(x, 10)) for x in (15, 17, 19, 21)))
    opts = options(legalization_candidates=1, exact_repair_candidates=1)
    result = engine._pack_local_components(board, references, {"U": initial["U"]}, initial,
                                          opts, allow_general=False)
    assert result is not None and placement_solution_is_legal(board, result, opts)


@pytest.mark.parametrize("offset,clearance,expected", [
    ((1.3, 1.4), .5, False),  # Exact diagonal clearance, not bounding-box clearance.
    ((1.3, 1.4), .51, True),
    ((.5, .5), 0, True),
    ((1, 0), 0, False),
])
def test_rectangular_courtyard_clearance_matches_edges_and_diagonals(offset, clearance, expected):
    from pcbir.placement import _polygons_too_close
    first = tuple(Point.mm(x, y) for x, y in ((0, 0), (1, 0), (1, 1), (0, 1)))
    second = tuple(Point.mm(x+offset[0], y+offset[1]) for x, y in ((0, 0), (1, 0), (1, 1), (0, 1)))
    assert _polygons_too_close(first, second, nm_from_mm(clearance)) is expected


@pytest.mark.parametrize("phase", ["local_pack", "legal_choice"])
def test_local_domain_keeps_outer_candidates_within_the_probe_budget(monkeypatch, phase):
    import pcbir.placement as engine
    board = local_companion_board()
    initial = poses(board)
    opts = options(legalization_candidates=2)
    probe_limit = opts.legalization_candidates*64
    outer = replace(initial["C1"], position=Point.mm(12, 9.4), rotation_degrees=0)
    # Nearer poses all overlap the fixed IC; only the last outer pose is legal.
    candidates = tuple(replace(outer, position=Point(nm_from_mm(10.5)+i, nm_from_mm(9.4)))
                       for i in range(probe_limit)) + (outer,)
    monkeypatch.setattr(engine, "_relative_candidates", lambda *args, **kwargs: candidates)
    legal = engine._legal
    calls = 0
    def counted(*args, **kwargs):
        nonlocal calls
        calls += 1
        return legal(*args, **kwargs)
    monkeypatch.setattr(engine, "_legal", counted)
    if phase == "local_pack":
        result = engine._pack_local_components(board, ["C1"], {"U": initial["U"]}, initial,
                                               opts, allow_general=False)
        assert result is not None and result["C1"] == outer
    else:
        result = engine._best_legal_choice(board, "C1", initial["C1"], {"U": initial["U"]},
                                           initial, engine._adjacency(board), opts, 0)
        assert result == outer
    assert calls <= probe_limit+1  # Domain probes plus final accepted-pose gate.


def test_distant_obstacles_do_not_change_local_candidate_sampling():
    from pcbir.placement import _relative_candidates
    board = local_companion_board()
    far = Placement("X", "capacitor", Point.mm(12, 24))
    board = replace(board, placements=(*board.placements, far))
    initial = poses(board)
    local = _relative_candidates(board, "C1", initial["C1"], {"U": initial["U"]}, options())
    assert local == _relative_candidates(board, "C1", initial["C1"],
                                         {"U": initial["U"], "X": far}, options())


def test_region_sampling_does_not_add_a_second_board_edge_clearance():
    from pcbir.placement import _candidate_positions, _project_coordinate, _outline_bounds, _legal
    board = local_companion_board()
    board = replace(board, placement_rules=(*board.placement_rules,
        ComponentPlacementRule("C1", region="local-bank", fixed_rotation_degrees=0)),
        regions=(PlacementRegion("local-bank", BoardOutline((Point.mm(18, 18), Point.mm(20, 18),
                    Point.mm(20, 20), Point.mm(18, 20)))),))
    target = poses(board)["C1"]
    candidates = _candidate_positions(board, target, options())
    assert Point.mm(19, 19) in candidates
    assert all(_legal(replace(target, position=point, rotation_degrees=0), {}, board, options())
               for point in candidates)
    coordinate = [30., 30.]
    _project_coordinate(board, target, coordinate, _outline_bounds(board.outline), options())
    assert coordinate == [19.5, 19.75]


@pytest.mark.parametrize("phase", ["local_pack", "legal_choice"])
def test_invalid_local_domain_does_not_starve_a_legal_region_grid(monkeypatch, phase):
    import pcbir.placement as engine
    board = local_companion_board()
    board = replace(board, placement_rules=(*board.placement_rules,
        ComponentPlacementRule("C1", region="local-bank", fixed_rotation_degrees=0)),
        relative_rules=(replace(board.relative_rules[0], distance_nm=nm_from_mm(15)),),
        regions=(PlacementRegion("local-bank", BoardOutline((Point.mm(18, 18), Point.mm(20, 18),
                    Point.mm(20, 20), Point.mm(18, 20)))),))
    initial = poses(board)
    opts = options(legalization_candidates=2)
    invalid = tuple(replace(initial["C1"], position=Point(nm_from_mm(10.5)+i, nm_from_mm(9.4)))
                    for i in range(opts.legalization_candidates*64+1))
    monkeypatch.setattr(engine, "_relative_candidates", lambda *args, **kwargs: invalid)
    if phase == "local_pack":
        packed = engine._pack_local_components(board, ["C1"], {"U": initial["U"]}, initial,
                                                opts, allow_general=True)
        assert packed is not None
        choice = packed["C1"]
    else:
        choice = engine._best_legal_choice(board, "C1", initial["C1"], {"U": initial["U"]},
                                           initial, engine._adjacency(board), opts, 0)
    assert choice is not None and choice.position == Point.mm(19, 19)

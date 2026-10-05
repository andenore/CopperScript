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

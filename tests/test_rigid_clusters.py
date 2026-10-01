from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir import (
    BoardOutline, BoardSide, ComponentPlacementRule, CopperKeepout, CopperLayer,
    FootprintPad, PhysicalBoard, PhysicalFootprint, Placement, PlacementTarget,
    Point, PolygonRing, PolygonWithHoles, RigidPlacementCluster,
    RigidPlacementMember, Size, plan_placement, PlacementPlannerOptions,
    PlacementPlanningError, placement_solution_is_legal,
)
from pcbir.clusters import (
    cluster_placement_matches, cluster_placements, footprint_geometry_digest,
    move_placement_unit,
)
from pcbir.placement import resolved_copper_keepouts, transformed_pad_position
from pcbir.drc import physical_board_digest, run_physical_drc
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.routeflow import detailed_failure_trials
from pcbir.physical import PadReference, PhysicalNet, TrackSegment, nm_from_mm


def fixture_board(*, keepouts=(), fixed=()):
    fp = PhysicalFootprint("test/rf", (FootprintPad("1", Point.mm(0.5, 0), Size.mm(0.2, 0.2)),),
                           Size.mm(1, 1), source_library_id="Test:RF")
    cluster = RigidPlacementCluster(
        "matching", PlacementTarget("U1", "1"), (
            RigidPlacementMember("U1", fp.name, footprint_geometry_digest(fp), Point.mm(-0.5, 0)),
            RigidPlacementMember("L1", fp.name, footprint_geometry_digest(fp), Point.mm(2.5, 0), 90),
        ), "synthetic test fixture, not vendor reference geometry", (0, 45, 90, 180, 270), keepouts,
    )
    board = PhysicalBoard("RigidTest", BoardOutline.rectangle(30, 30), {fp.name: fp}, (
        Placement("U1", fp.name, Point.mm(10, 10)),
        Placement("L1", fp.name, Point.mm(13, 10), 90),
        Placement("J1", fp.name, Point.mm(23, 23)),
    ), (), rigid_clusters=(cluster,), placement_rules=tuple(fixed) + (
        ComponentPlacementRule("U1", allowed_orientations=(0, 45, 90, 180, 270)),
        ComponentPlacementRule("L1", allowed_orientations=(0, 90, 135, 180, 270)),
    ))
    return board


def mapping(board):
    return {item.reference: item for item in board.placements}


def options(**kwargs):
    return PlacementPlannerOptions(candidate_count=1, analytical_iterations=1,
                                   refinement_passes=1, legalization_candidates=12, **kwargs)


@pytest.mark.parametrize("angle", [0, 45, 90, 180, 270])
def test_whole_macro_transforms_preserve_anchor_pad_and_member_rotation(angle):
    board = fixture_board()
    cluster = board.rigid_clusters[0]
    anchor = replace(mapping(board)["U1"], position=Point.mm(15, 15), rotation_degrees=angle)
    moved = cluster_placements(board, cluster, anchor)
    assert moved["U1"] == anchor
    assert moved["L1"].rotation_degrees == (Decimal(angle) + 90) % 360
    complete = {**mapping(board), **moved}
    assert cluster_placement_matches(board, complete)
    assert placement_solution_is_legal(board, complete)
    # Anchor pad is the same real pad, not the component's invented center.
    assert transformed_pad_position(board, moved["U1"], "1") != anchor.position


def test_planner_keeps_rigid_geometry_and_is_deterministic():
    board = fixture_board()
    first = plan_placement(board, options())
    assert first == plan_placement(board, options())
    assert cluster_placement_matches(first.board, mapping(first.board))
    assert placement_solution_is_legal(first.board, mapping(first.board), options())


def test_fixed_nonanchor_member_freezes_complete_cluster():
    board = fixture_board()
    plan = plan_placement(board, options(fixed_references=frozenset({"L1"})))
    assert mapping(plan.board)["L1"] == mapping(board)["L1"]
    assert mapping(plan.board)["U1"] == mapping(board)["U1"]


def test_conflicting_fixed_members_fail_instead_of_splitting():
    board = fixture_board()
    bad = replace(board, placements=tuple(
        replace(item, position=Point.mm(17, 10)) if item.reference == "L1" else item
        for item in board.placements))
    with pytest.raises(PlacementPlanningError, match="rigid cluster"):
        plan_placement(bad, options(fixed_references=frozenset({"U1", "L1"})))


def test_member_pose_or_identity_changes_are_not_legal():
    board = fixture_board()
    poses = mapping(board)
    poses["L1"] = replace(poses["L1"], position=Point.mm(14, 10))
    assert not placement_solution_is_legal(board, poses)
    assert any(item.code == "DRC-PLACEMENT" for item in run_physical_drc(
        replace(board, placements=tuple(poses.values()))).findings)
    poses = mapping(board)
    poses["J1"] = replace(poses["J1"], reference="pretend")
    assert not placement_solution_is_legal(board, poses)


def test_changed_pad_geometry_rejects_template_binding():
    board = fixture_board()
    fp = board.footprints["test/rf"]
    changed = replace(fp, pads=(replace(fp.pads[0], position=Point.mm(0.6, 0)),))
    with pytest.raises(ValueError, match="digest mismatch"):
        replace(board, footprints={fp.name: changed})


def test_wrong_anchor_pad_overlap_and_mirroring_fail_closed():
    board = fixture_board()
    cluster = board.rigid_clusters[0]
    with pytest.raises(ValueError, match="unique physical pad"):
        replace(board, rigid_clusters=(replace(cluster, anchor=PlacementTarget("U1", "99")),))
    with pytest.raises(ValueError, match="overlapping rigid"):
        replace(board, rigid_clusters=(cluster, replace(cluster, name="overlap")))
    with pytest.raises(ValueError, match="mirroring"):
        replace(board, placements=tuple(replace(item, side=BoardSide.BACK) for item in board.placements))


def test_keepout_transforms_layers_export_and_legality():
    local = CopperKeepout("clear", (CopperLayer.FRONT,), PolygonWithHoles(
        PolygonRing((Point.mm(4, -1), Point.mm(6, -1), Point.mm(6, 1), Point.mm(4, 1)))),
        block_footprints=True)
    board = fixture_board(keepouts=(local,))
    moved = {**mapping(board), **cluster_placements(board, board.rigid_clusters[0],
             replace(mapping(board)["U1"], rotation_degrees=90))}
    board = replace(board, placements=tuple(moved.values()))
    keepout = resolved_copper_keepouts(board)[0]
    assert keepout.id == "cluster/matching/clear"
    assert keepout.layers == (CopperLayer.FRONT,)
    assert keepout.outline.outer.vertices[0] == Point.mm(9, 5.5)
    content = KiCadPcbBackend().generate(board).artifacts[0].content
    assert "cluster/matching/clear" in content
    assert '(xy 9 5.5)' in content
    poses = mapping(board)
    poses["J1"] = replace(poses["J1"], position=Point.mm(10, 4.5))
    assert not placement_solution_is_legal(board, poses)


def test_template_rules_are_bound_to_signoff_digest():
    board = fixture_board()
    other = replace(board, rigid_clusters=(replace(board.rigid_clusters[0], allowed_rotations=(0, 90)),))
    assert physical_board_digest(board) != physical_board_digest(other)


def test_member_feedback_move_expands_and_fixed_companion_blocks_it():
    board = fixture_board()
    poses = mapping(board)
    changed = replace(poses["L1"], position=Point.mm(14, 10))
    trial = move_placement_unit(board, poses, "L1", changed)
    assert trial["U1"].position == Point.mm(11, 10)
    assert trial["J1"] == poses["J1"]
    assert placement_solution_is_legal(board, trial)
    assert not placement_solution_is_legal(board, trial, options(fixed_references=frozenset({"U1"})))
    with pytest.raises(ValueError, match="rotation is not allowed"):
        move_placement_unit(board, poses, "L1", replace(changed, rotation_degrees=110))


def test_complete_cluster_trial_rejects_internal_collision():
    board = fixture_board()
    cluster = board.rigid_clusters[0]
    bad = replace(cluster, members=(cluster.members[0], replace(cluster.members[1], position=Point.mm(0, 0))))
    board = replace(board, rigid_clusters=(bad,))
    with pytest.raises(PlacementPlanningError, match="rigid cluster"):
        plan_placement(board, options())


def test_feedback_trials_preserve_units_and_never_move_under_copper():
    board = fixture_board()
    board = replace(board, nets=(PhysicalNet("RF", (PadReference("L1", "1"), PadReference("J1", "1"))),))
    trials = detailed_failure_trials(board, frozenset({"RF"}), options(), nm_from_mm(1), 8)
    assert trials
    assert all(cluster_placement_matches(item, mapping(item)) for item in trials)
    assert any(mapping(item)["U1"] != mapping(board)["U1"] for item in trials)
    assert not detailed_failure_trials(replace(board, tracks=(TrackSegment(
        "RF", Point.mm(1, 1), Point.mm(2, 1), nm_from_mm(0.2), CopperLayer.FRONT),)),
        ("RF",), options(), nm_from_mm(1), 4)


def test_cluster_keepouts_affect_route_clearance_not_unrelated_layers():
    from pcbir.routing_clearance import RoutingClearanceIndex
    local = CopperKeepout("clear", (CopperLayer.FRONT,), PolygonWithHoles(
        PolygonRing((Point.mm(4, -1), Point.mm(6, -1), Point.mm(6, 1), Point.mm(4, 1)))))
    board = fixture_board(keepouts=(local,))
    board = replace(board, nets=(PhysicalNet("RF", (PadReference("L1", "1"), PadReference("J1", "1"))),))
    index = RoutingClearanceIndex(board)
    track = TrackSegment("RF", Point.mm(14.5, 8), Point.mm(14.5, 12), nm_from_mm(0.2), CopperLayer.FRONT)
    assert not index.can_track(track.net, track.start, track.end, track.width_nm, track.layer)
    assert index.can_track(track.net, track.start, track.end, track.width_nm, CopperLayer.BACK)


@pytest.mark.parametrize("layer, blocked", [(CopperLayer.FRONT, True), (CopperLayer.BACK, False)])
def test_cluster_keepout_agrees_with_installed_kicad(layer, blocked, tmp_path):
    import json
    from pathlib import Path
    import shutil
    import subprocess

    cli = shutil.which("kicad-cli")
    installed = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if cli is None and installed.is_file():
        cli = str(installed)
    if cli is None:
        pytest.skip("independent cluster keepout check requires installed KiCad CLI")
    local = CopperKeepout("clear", (CopperLayer.FRONT,), PolygonWithHoles(
        PolygonRing((Point.mm(4, -1), Point.mm(6, -1), Point.mm(6, 1), Point.mm(4, 1)))))
    board = fixture_board(keepouts=(local,))
    board = replace(board,
                    nets=(PhysicalNet("RF", (PadReference("L1", "1"), PadReference("J1", "1"))),),
                    tracks=(TrackSegment("RF", Point.mm(14.5, 8), Point.mm(14.5, 12), nm_from_mm(0.2), layer),))
    for artifact in KiCadPcbBackend().generate(board).artifacts:
        (tmp_path / artifact.name).write_text(artifact.content, encoding="utf-8")
    report = tmp_path / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--format", "json", "--severity-all", "--output",
                             str(report), str(tmp_path / "RigidTest.kicad_pcb")],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    violations = json.loads(report.read_text())["violations"]
    assert any(item["type"] == "items_not_allowed" for item in violations) == blocked

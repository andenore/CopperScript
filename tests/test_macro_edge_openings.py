"""Antenna corner intent is generic host geometry, not component-name inference."""
from dataclasses import replace

import pytest

from pcbir.clusters import cluster_placements, cluster_edge_opening_rejections
from pcbir.hard_macros import materialize_hard_macros
from pcbir.physical import BoardOutline, BoardCutout, Point
from pcbir.placement import placement_solution_is_legal, placement_rejection_reasons
from test_hard_macros import fixture


def corner_fixture(tmp_path):
    board, asset, bind = fixture(tmp_path)
    asset.update(schema="copperlib-physical-hard-macro/v0.5", zones=[], plane_returns=[],
                 polygons=[], width_contracts=[],
                 edge_openings=[dict(keepout="clear", edge_indices=[0, 1])])
    asset["allowed_rotations"] = [0, 45, 90, 180, 270]
    asset["keepouts"] = [dict(id="clear", layers=["F.Cu", "B.Cu"],
        vertices=[[4000000,-4000000],[6000000,-4000000],
                  [6000000,-2000000],[4000000,-2000000]],
        block_tracks=False, block_vias=True, block_zones=True)]
    return board, asset, bind


@pytest.mark.parametrize("angle,origin", [(0,(24,4)), (90,(4,6)), (180,(6,26)), (270,(26,24))])
def test_whole_edges_follow_rotation_to_each_host_corner(tmp_path, angle, origin):
    _, _, bind = corner_fixture(tmp_path)
    board = bind()
    poses = {p.reference:p for p in board.placements}
    poses.update(cluster_placements(board, board.rigid_clusters[0],
        replace(poses["U1"], position=Point.mm(*origin), rotation_degrees=angle)))
    board = replace(board, placements=tuple(poses.values()))
    assert not cluster_edge_opening_rejections(board, poses)
    assert placement_solution_is_legal(board, poses)
    result = materialize_hard_macros(board)
    assert result.materialized_macros == ("unit",)
    assert not cluster_edge_opening_rejections(result, poses)


@pytest.mark.parametrize("origin", [(20,8), (24,8), (24,3), (25,4)])
def test_island_single_edge_and_partial_overhang_fail_before_copper(tmp_path, origin):
    _, _, bind = corner_fixture(tmp_path)
    board = bind()
    poses = {p.reference:p for p in board.placements}
    poses.update(cluster_placements(board, board.rigid_clusters[0], replace(poses["U1"], position=Point.mm(*origin))))
    board = replace(board, placements=tuple(poses.values()))
    assert cluster_edge_opening_rejections(board, poses)
    assert not placement_solution_is_legal(board, poses)
    assert any("edge" in reason for reason in placement_rejection_reasons(board, poses))
    with pytest.raises(ValueError, match="complete rigid macro"):
        materialize_hard_macros(board)
    assert not board.tracks


def test_45_degree_transform_does_not_qualify_rectangular_corner(tmp_path):
    _, _, bind = corner_fixture(tmp_path)
    board = bind()
    poses = {p.reference:p for p in board.placements}
    poses.update(cluster_placements(board, board.rigid_clusters[0], replace(poses["U1"], rotation_degrees=45)))
    assert cluster_edge_opening_rejections(board, poses)


def test_curved_host_is_not_accepted_from_approximated_query_ring(tmp_path):
    _, _, bind = corner_fixture(tmp_path)
    board = bind()
    board = replace(board, outline=BoardOutline.rounded_rectangle(30, 30, 2))
    assert "straight polygonal" in cluster_edge_opening_rejections(board, {p.reference:p for p in board.placements})[0]


def test_internal_cutout_is_not_an_outer_corner(tmp_path):
    _, _, bind = corner_fixture(tmp_path)
    board = bind()
    cutout = BoardCutout('interior', (Point.mm(14,6), Point.mm(16,6), Point.mm(16,8), Point.mm(14,8)))
    board = replace(board, outline=replace(board.outline, cutouts=(cutout,)))
    errors = cluster_edge_opening_rejections(board, {p.reference:p for p in board.placements})
    assert len(errors) == 2 and all('outer board perimeter' in reason for reason in errors)


@pytest.mark.parametrize("change", ["legacy", "missing", "out_of_range", "boolean", "duplicate", "not_clearance"])
def test_contract_binding_fails_closed(tmp_path, change):
    _, asset, bind = corner_fixture(tmp_path)
    if change == "legacy": asset["schema"] = "copperlib-physical-hard-macro/v0.4"
    elif change == "missing": asset["edge_openings"][0]["keepout"] = "absent"
    elif change == "out_of_range": asset["edge_openings"][0]["edge_indices"] = [4]
    elif change == "boolean": asset["edge_openings"][0]["edge_indices"] = [True]
    elif change == "duplicate": asset["edge_openings"][0]["edge_indices"] = [0, 0]
    else: asset["keepouts"][0]["block_zones"] = False
    with pytest.raises(ValueError): bind()

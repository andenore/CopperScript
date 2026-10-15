"""Recorded witnesses are node-simple paths, while original edges may be split."""
from dataclasses import replace

import pytest

from pcbir.decoupling_paths import (DecouplingPathNode, DecouplingPathStep,
    propose_crosslayer_decoupling, validate_crosslayer_witness,
    verify_crosslayer_decoupling)
from pcbir.drc import run_physical_drc
from pcbir.physical import CopperLayer, Point, TrackSegment, Via
from test_decoupling_crosslayer_paths import fixture


@pytest.fixture(params=(False, True), ids=("original", "renamed"))
def complete(request):
    board, link = fixture(renamed=request.param)
    result = propose_crosslayer_decoupling(board, link)
    assert result.witness is not None, result.reason
    return result.board, result.witness


def assert_hard_clean(board):
    assert not [finding for finding in run_physical_drc(board).findings
                if finding.severity.value == "error"
                and finding.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


@pytest.mark.parametrize("path_name", ("supply_path", "return_path"))
def test_connected_out_and_back_spur_cannot_gain_semantic_ownership(complete, path_name):
    board, witness = complete
    path = getattr(witness, path_name)
    start = path.steps[0].start
    spur = TrackSegment(path.net, start.position,
        Point(start.position.x_nm - 1_000_000, start.position.y_nm),
        150_000, start.layer)
    appended = replace(board, tracks=(*board.tracks, spur))
    assert_hard_clean(appended)
    # The original complete path remains valid on this fresh appended board.
    assert validate_crosslayer_witness(appended, witness) == ""
    end = DecouplingPathNode(spur.end, spur.layer)
    forged = replace(path, steps=(DecouplingPathStep(spur, start, end),
        DecouplingPathStep(spur, end, start), *path.steps))
    assert validate_crosslayer_witness(appended,
        replace(witness, **{path_name: forged})) == "witness_path_cycle"


@pytest.mark.parametrize("path_name", ("supply_path", "return_path"))
def test_zero_length_original_edge_step_is_a_node_revisit(path_name):
    source, link = fixture()
    routed = propose_crosslayer_decoupling(source, link)
    board, witness = routed.board, routed.witness
    assert witness is not None
    path = getattr(witness, path_name)
    index, step = next((i, s) for i, s in enumerate(path.steps)
                       if isinstance(s.edge, TrackSegment))
    zero = DecouplingPathStep(step.edge, step.start, step.start)
    forged = replace(path, steps=(*path.steps[:index], zero, *path.steps[index:]))
    assert validate_crosslayer_witness(board,
        replace(witness, **{path_name: forged})) == "witness_path_cycle"


@pytest.mark.parametrize("path_name", ("supply_path", "return_path"))
def test_existing_via_out_and_back_cycle_is_rejected(path_name):
    source, link = fixture()
    routed = propose_crosslayer_decoupling(source, link)
    board, witness = routed.board, routed.witness
    assert witness is not None
    path = getattr(witness, path_name)
    index, step = next((i, s) for i, s in enumerate(path.steps) if isinstance(s.edge, Via))
    reverse = DecouplingPathStep(step.edge, step.end, step.start)
    forged = replace(path, steps=(*path.steps[:index], step, reverse, *path.steps[index:]))
    assert validate_crosslayer_witness(board,
        replace(witness, **{path_name: forged})) == "witness_path_cycle"


@pytest.mark.parametrize("path_name", ("supply_path", "return_path"))
def test_genuine_bfs_may_split_one_original_track_at_distinct_nodes(complete, path_name):
    board, witness = complete
    path = getattr(witness, path_name)
    track = next(s.edge for s in path.steps
                 if isinstance(s.edge, TrackSegment) and s.edge.layer is CopperLayer.FRONT)
    middle = Point((track.start.x_nm + track.end.x_nm) // 2,
                   (track.start.y_nm + track.end.y_nm) // 2)
    assert middle not in (track.start, track.end)
    # A same-net branch introduces a graph vertex on the original track. The
    # real BFS traverses two consecutive subsegments of that exact same edge.
    endpoint = (Point(middle.x_nm, middle.y_nm - 500_000)
                if track.start.y_nm == track.end.y_nm
                else Point(middle.x_nm - 500_000, middle.y_nm))
    branch = TrackSegment(path.net, middle, endpoint, track.width_nm, track.layer)
    appended = replace(board, tracks=(*board.tracks, branch))
    assert_hard_clean(appended)
    fresh = verify_crosslayer_decoupling(appended, witness.link)
    assert fresh.witness is not None, fresh.reason
    measured = getattr(fresh.witness, path_name)
    assert any(first.edge is track and second.edge is track
               and first.end == second.start
               for first, second in zip(measured.steps, measured.steps[1:]))
    nodes = (measured.steps[0].start, *(step.end for step in measured.steps))
    assert len(nodes) == len(set(nodes))
    assert validate_crosslayer_witness(appended, fresh.witness) == ""
    assert track in appended.tracks
    assert all(any(step.edge is original for original in (*appended.tracks, *appended.vias))
               for step in measured.steps)


@pytest.mark.parametrize("path_name", ("supply_path", "return_path"))
def test_simple_path_still_requires_original_edge_identity(path_name):
    source, link = fixture()
    routed = propose_crosslayer_decoupling(source, link)
    board, witness = routed.board, routed.witness
    assert witness is not None
    path = getattr(witness, path_name)
    index, step = next((i, s) for i, s in enumerate(path.steps)
                       if isinstance(s.edge, TrackSegment))
    copied = replace(step.edge)
    assert copied == step.edge and copied is not step.edge
    forged = replace(path, steps=(*path.steps[:index], replace(step, edge=copied),
                                  *path.steps[index + 1:]))
    assert validate_crosslayer_witness(board,
        replace(witness, **{path_name: forged})) == "witness_track_invalid"

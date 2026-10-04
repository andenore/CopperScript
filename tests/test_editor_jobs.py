from dataclasses import replace
from time import monotonic, sleep

import pytest

from pcbir.editor.jobs import PlacementJob
from pcbir.editor.scene import RatsnestCache, ratsnest
from pcbir.editor.session import EditorSession
from pcbir.layout import plan_placement
from pcbir.physical import (BoardOutline, FootprintPad, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, Size)
from pcbir.placement import PlacementPlannerOptions


def board():
    fp = PhysicalFootprint("test", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    return PhysicalBoard("Jobs", BoardOutline.rectangle(40, 30), {fp.name: fp},
        tuple(Placement(ref, fp.name, Point.mm(x, y)) for ref, x, y in
              (("A", 5, 5), ("B", 12, 5), ("C", 5, 15), ("D", 12, 15))),
        (PhysicalNet("N1", (PadReference("A", "1"), PadReference("B", "1"))),
         PhysicalNet("N2", (PadReference("C", "1"), PadReference("D", "1")))))


OPTIONS = PlacementPlannerOptions(candidate_count=1, analytical_iterations=2, refinement_passes=0)


def wait(job):
    deadline = monotonic() + 15
    while monotonic() < deadline:
        info = job.poll()
        if info["status"] != "running":
            return info
        sleep(.02)
    job.cancel()
    pytest.fail("placement worker did not finish within test budget")


def test_worker_result_matches_synchronous_planner_and_has_profile():
    source = board()
    job = PlacementJob(source, OPTIONS, revision=7, source_revision="source", timeout_seconds=15)
    try:
        info = wait(job)
        assert info["status"] == "completed", info
        assert job.result == plan_placement(source, OPTIONS).board
        assert info["profile"]["total_calls"] > 0
        assert "placement.py" in info["profile"]["top_cumulative"]
        assert info["input_revision"] == 7
    finally:
        job.cancel()


def test_cancel_stops_process_and_timeout_is_enforced_without_polling():
    job = PlacementJob(board(), OPTIONS, revision=0, source_revision="x", timeout_seconds=15)
    job.cancel()
    assert not job.process.is_alive() and job.poll()["status"] == "cancelled"
    timed = PlacementJob(board(), OPTIONS, revision=0, source_revision="x", timeout_seconds=.01)
    assert timed.done.wait(3)
    assert timed.poll()["status"] == "timed_out"
    assert not timed.process.is_alive()


def test_editor_can_read_and_edit_while_placement_runs(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text("// Physical test fixture\n")
    s = EditorSession(board(), source, OPTIONS)
    original = s.state
    response = s.operation({"action": "start_auto_place", "revision": 0, "budget_seconds": 15})
    assert response["scene"]["placement_job"]["status"] == "running"
    s.operation({"action": "lock", "revision": s.revision, "reference": "A", "locked": True})
    assert s.job.poll()["status"] == "stale" and not s.job.process.is_alive()
    assert s.state.board == original.board and s.pending is None
    assert source.read_text() == "// Physical test fixture\n"
    s.close()


def test_async_result_is_preview_and_stale_source_cannot_apply(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text("// Physical test fixture\n")
    s = EditorSession(board(), source, OPTIONS)
    s.operation({"action": "start_auto_place", "revision": 0, "budget_seconds": 15})
    wait(s.job)
    scene = s.scene()
    assert scene["pending_preview"] and s.revision == 2
    assert s.state.board == board()
    s.operation({"action": "discard", "revision": 2})
    assert s.pending is None
    s.operation({"action": "start_auto_place", "revision": 3, "budget_seconds": 15})
    source.write_text("// Changed externally\n")
    wait(s.job)
    scene = s.scene()
    assert scene["source_stale"] and scene["placement_job"]["status"] == "stale"
    assert s.pending is None
    s.close()


def test_incremental_ratsnest_matches_full_and_reuses_unaffected_nets():
    source = board()
    cache = RatsnestCache()
    assert cache.get(source) == ratsnest(source)
    assert cache.recomputed_nets == ("N1", "N2")
    old_n2 = cache.edges["N2"]
    moved = replace(source, placements=(replace(source.placements[0], position=Point.mm(8, 5)), *source.placements[1:]))
    assert cache.get(moved) == ratsnest(moved)
    assert cache.recomputed_nets == ("N1",) and cache.edges["N2"] is old_n2
    assert cache.get(moved) == ratsnest(moved)
    assert cache.recomputed_nets == ()

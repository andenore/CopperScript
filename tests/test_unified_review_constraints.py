"""Router review regressions: real copper, whole-net budgets and cancellation."""
from collections import Counter
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir import (CopperLayer, CopperZone, NetRoutingRule, Point, PolygonRing,
                   PolygonWithHoles, Size, TrackSegment, nm_from_mm, route_global)
from pcbir.physical import BoardSide, IslandPolicy, ZoneConnection
from pcbir.unified_router import UnifiedRouterOptions, route_unified
from test_unified_router import _board, _route


@pytest.fixture(params=["python", "native"])
def search_backend(request, monkeypatch):
    import pcbir.native as native
    if request.param == "python":
        monkeypatch.setattr(native, "astar", lambda: None)
    elif native.astar() is None:
        pytest.skip("native kernel unavailable")


@pytest.mark.parametrize("limit,connected", [(0, False), (1, True)])
def test_opposite_face_terminals_respect_zero_and_positive_via_budgets(search_backend, limit, connected):
    board = _board({"N": ((2, 3), (8, 3))})
    board = replace(board, placements=(board.placements[0],
        replace(board.placements[1], side=BoardSide.BACK)),
        net_routing_rules=(NetRoutingRule("N", max_vias=limit),))
    result, _, summary = _route(board, maximum_iterations=5, commit_rounds=1)
    assert result.nets[0].connected is connected
    assert len(result.board.vias) <= limit
    assert "DRC-MAX-VIAS" not in summary["drc"]["errors_by_code"]


def _plane_lead_board():
    board = _board({"G": ((2.1, 3), (8, 3))})
    footprint = next(iter(board.footprints.values()))
    footprint = replace(footprint, pads=(replace(footprint.pads[0], size=Size.mm("0.1", "0.1")),))
    outline = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        (("5.25", 1), (24, 1), (24, 24), ("5.25", 24)))))
    return replace(board, footprints={footprint.name: footprint},
        zones=(CopperZone("g", "G", (CopperLayer.FRONT,), outline,
            pad_connection=ZoneConnection.SOLID, island_policy=IslandPolicy.REMOVE_ALL,
            minimum_island_area_nm2=None),))


def test_plane_target_requires_its_lead_in_to_be_committed(search_backend):
    board = _plane_lead_board()
    result, _, summary = _route(board, maximum_iterations=3, commit_rounds=1)
    assert result.nets[0].connected
    assert summary["plane"]["estimated_opens"] == 0
    assert result.board.tracks and not result.board.vias
    assert any(Point.mm("5.1", 6) in (track.start, track.end) for track in result.board.tracks)


def test_native_fill_confirms_the_plane_lead_in_connection(tmp_path):
    from pcbir.plane_verify import verify_filled_planes
    native = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not native.is_file():
        pytest.skip("native refill unavailable")
    result, _, _ = _route(_plane_lead_board(), maximum_iterations=3, commit_rounds=1)
    checked = verify_filled_planes(result.board, kicad_cli=native,
                                  artifact_directory=tmp_path / "native-plane-lead")
    assert checked.passed, checked.findings


@pytest.mark.parametrize("limit,over", [(8, True), (12, False)])
def test_final_length_budget_includes_fixed_tracks(search_backend, limit, over):
    board = _board({"N": ((2, 3), (8, 3), (14, 3))})
    fixed = TrackSegment("N", Point.mm(5, 6), Point.mm(11, 6), nm_from_mm("0.2"), CopperLayer.FRONT)
    board = replace(board, tracks=(fixed,), net_routing_rules=(NetRoutingRule(
        "N", max_length_nm=nm_from_mm(limit)),))
    result, _, summary = _route(board, maximum_iterations=5, commit_rounds=1)
    # Routable copper is kept either way; a length miss is a review warning.
    assert result.nets[0].connected and not result.nets[0].diagnostics
    assert Counter(board.tracks) < Counter(result.board.tracks)
    assert "DRC-MAX-LENGTH" not in summary["drc"]["errors_by_code"]
    if over:
        assert "12000000 nm exceeds 8000000 nm" in " ".join(result.nets[0].warnings)
        assert summary["drc"]["budget_warnings_by_code"] == {"DRC-MAX-LENGTH": 1}
    else:
        assert not result.nets[0].warnings and summary["drc"]["budget_warnings_by_code"] == {}


def test_already_connected_source_over_budget_is_preserved_with_a_warning():
    board = _board({"N": ((2, 3), (8, 3))})
    fixed = TrackSegment("N", Point.mm(5, 6), Point.mm(11, 6), nm_from_mm("0.2"), CopperLayer.FRONT)
    board = replace(board, tracks=(fixed,), net_routing_rules=(NetRoutingRule(
        "N", max_length_nm=nm_from_mm(5)),))
    result, _, _ = _route(board, maximum_iterations=1, commit_rounds=1)
    assert result.nets[0].connected and result.status.value == "success"
    assert result.board.tracks == board.tracks and result.board.vias == board.vias
    assert "6000000 nm exceeds 5000000 nm" in " ".join(result.nets[0].warnings)


@pytest.mark.parametrize("expire_at", ["unified_setup", "unified_iteration"])
def test_expired_deadline_stops_before_starting_nets(monkeypatch, expire_at):
    import pcbir.unified_router as unified
    board = _board({"N": ((2, 3), (8, 3)), "FIXED": ((2, 8), (8, 8))})
    fixed = TrackSegment("FIXED", Point.mm(5, 11), Point.mm(11, 11), nm_from_mm("0.2"), CopperLayer.FRONT)
    board = replace(board, tracks=(fixed,))
    guide = route_global(replace(board, tracks=()))
    clock = [0.0]
    monkeypatch.setattr(unified, "perf_counter", lambda: clock[0])
    events = []
    def observe(phase, event, details):
        events.append((phase, event))
        if phase == expire_at and event == ("finished" if phase == "unified_setup" else "started"):
            clock[0] = 11.0
    result = route_unified(board, guide, UnifiedRouterOptions(time_budget_s=10), on_progress=observe)
    assert result.board.tracks == board.tracks and result.board.vias == board.vias
    nets = {item.net: item for item in result.nets}
    assert nets["FIXED"].connected and not nets["N"].connected
    assert "time budget exhausted" in " ".join(nets["N"].diagnostics)
    if expire_at == "unified_setup":
        assert ("unified_iteration", "started") not in events


def test_expired_negotiation_keeps_its_previous_incumbent(search_backend):
    from pcbir.unified_router import _Router
    board = _board({"N": ((2, 3), (8, 3))})
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    previous, conflicts, _ = router.negotiate(None)
    assert previous["N"].connected and not conflicts
    occupancy = list(router.occupancy.occ)
    current, conflicts, iterations = router.negotiate(-1)
    assert current == previous and not conflicts and iterations == 0
    assert list(router.occupancy.occ) == occupancy

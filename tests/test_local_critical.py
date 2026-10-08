from dataclasses import replace
import json

import pytest

from pcbir import (
    BoardOutline, BoardSide, CopperLayer, CopperKeepout, FootprintPad, GlobalRouterOptions,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, TrackSegment,
    route_global, route_critical_nets, run_physical_drc, nm_from_mm,
)
from pcbir.critical import _improve_single_surface, _route_single, _validate_candidate
from pcbir.local_critical import local_surface_candidates
from pcbir.routing import GlobalRouteSegment


def fixture(tree=True):
    fp = PhysicalFootprint("terminal", (FootprintPad("1", Point(0, 0), Size.mm(.4, .4)),), Size.mm(1, 1))
    poses = (Placement("A", fp.name, Point.mm(5, 5)), Placement("B", fp.name, Point.mm(10, 5)))
    if tree:
        poses += (Placement("C", fp.name, Point.mm(10, 10)),)
    rule = NetRoutingRule("MATCH", RouteKind.CRITICAL, topology="tree" if tree else "point_to_point",
                          allowed_layers=(CopperLayer.FRONT,), max_vias=0)
    board = PhysicalBoard("LocalTree", BoardOutline.rectangle(30, 30), {fp.name: fp}, poses,
                          (PhysicalNet("MATCH", tuple(PadReference(p.reference, "1") for p in poses)),),
                          net_routing_rules=(rule,))
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(1)))
    guide = global_route.routes[0]
    vertices = [Point.mm(5, 5), Point.mm(5, 18), Point.mm(10, 18), Point.mm(10, 5)]
    if tree:
        vertices += [Point.mm(10, 10)]
    segments = tuple(GlobalRouteSegment("MATCH", CopperLayer.FRONT, a, b, nm_from_mm(1), "synthetic")
                     for a, b in zip(vertices, vertices[1:]))
    guide = replace(guide, segments=segments, vias=(), accesses=tuple(
        replace(access, access_position=access.pad_position, tracks=(), via=None) for access in guide.accesses))
    return board, replace(global_route, routes=(guide,))


def hard_findings(board):
    return [f for f in run_physical_drc(board).findings if f.code != "DRC-ROUTE-INCOMPLETE"]


def test_short_tree_replaces_legal_detour_and_preserves_every_branch():
    board, guides = fixture()
    original, tracks, vias = _route_single(board, board.net_routing_rules[0], guides.routes[0])
    original, _, _ = _validate_candidate(board, original, tracks, vias, [], [])
    assert original.connected
    result = route_critical_nets(board, guides)
    net = result.nets[0]
    assert net.connected and net.strategy == "local_surface_tree"
    assert net.lengths_nm == (nm_from_mm(10),)
    assert net.guide_length_nm == original.lengths_nm[0]
    assert net.local_candidate_attempts == 1
    assert net.lengths_nm[0] < net.guide_length_nm
    assert not hard_findings(result.board)
    assert not result.board.vias
    assert route_critical_nets(board, guides) == result
    assert json.loads(result.to_json())["nets"][0]["local_candidate_attempts"] == 1
    assert board.tracks == ()


def test_blocked_local_paths_preserve_legal_incumbent():
    board, guides = fixture(False)
    wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(7, 2), Point.mm(8, 2), Point.mm(8, 15), Point.mm(7, 15),
    ))))
    board = replace(board, copper_keepouts=(wall,))
    original, tracks, vias = _route_single(board, board.net_routing_rules[0], guides.routes[0])
    original, tracks, vias = _validate_candidate(board, original, tracks, vias, [], [])
    assert original.connected
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected and result.nets[0].strategy == "global_guide"
    assert result.locked_tracks == tracks and result.locked_vias == vias
    assert not hard_findings(result.board)


def test_local_tree_never_drops_an_unreachable_terminal():
    board, _ = fixture()
    wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(0, 7), Point.mm(30, 7), Point.mm(30, 8), Point.mm(0, 8),
    ))))
    assert not tuple(local_surface_candidates(replace(board, copper_keepouts=(wall,)), board.net_routing_rules[0]))


def test_local_candidate_must_satisfy_original_length_budget():
    board, guides = fixture()
    rule = replace(board.net_routing_rules[0], max_length_nm=nm_from_mm(2))
    board = replace(board, net_routing_rules=(rule,))
    original, tracks, vias = _route_single(board, rule, guides.routes[0])
    original, tracks, vias = _validate_candidate(board, original, tracks, vias, [], [])
    result, tracks, vias = _improve_single_surface(board, rule, guides.routes[0], original, tracks, vias, [], [])
    assert not result.connected and result.local_candidate_attempts == 1
    assert tracks == vias == ()
    assert any("exceeds" in diagnostic for diagnostic in result.diagnostics)


def test_previous_critical_reservation_is_not_crossed_or_mutated():
    board, guides = fixture(False)
    board = replace(board, nets=(*board.nets, PhysicalNet("BLOCK", ())))
    reserved = [TrackSegment("BLOCK", Point.mm(7, 2), Point.mm(7, 15), nm_from_mm(.2), CopperLayer.FRONT)]
    original, tracks, vias = _route_single(board, board.net_routing_rules[0], guides.routes[0])
    original, tracks, vias = _validate_candidate(board, original, tracks, vias, reserved, [])
    assert original.connected
    result, actual, _ = _improve_single_surface(board, board.net_routing_rules[0], guides.routes[0],
                                               original, tracks, vias, reserved, [])
    assert result.connected and actual == tracks
    assert len(reserved) == 1


def test_repeated_pad_numbers_are_separate_required_lands():
    board, _ = fixture(False)
    fp = board.footprints["terminal"]
    fp = replace(fp, pads=(*fp.pads, replace(fp.pads[0], position=Point.mm(0, 2))))
    board = replace(board, footprints={fp.name: fp})
    candidate = next(local_surface_candidates(board, board.net_routing_rules[0]))
    assert not hard_findings(replace(board, tracks=candidate))


@pytest.mark.parametrize("kind", [RouteKind.RF_FEED, RouteKind.CLOCK])
def test_point_to_point_profile_cannot_be_reinterpreted_as_matching_tree(kind):
    board, _ = fixture()
    rule = replace(board.net_routing_rules[0], kind=kind)
    assert not tuple(local_surface_candidates(board, rule))


@pytest.mark.parametrize("kind", [RouteKind.RF_FEED, RouteKind.CLOCK])
def test_point_to_point_profile_does_not_bypass_limit_with_duplicate_lands(kind):
    board, _ = fixture(False)
    fp = board.footprints["terminal"]
    fp = replace(fp, pads=(*fp.pads, replace(fp.pads[0], position=Point.mm(0, 2))))
    board = replace(board, footprints={fp.name: fp})
    assert not tuple(local_surface_candidates(board, replace(board.net_routing_rules[0], kind=kind)))


def test_physical_land_count_is_bounded():
    board, _ = fixture(False)
    fp = board.footprints["terminal"]
    fp = replace(fp, pads=tuple(replace(fp.pads[0], position=Point.mm(0, n)) for n in range(9)))
    board = replace(board, footprints={fp.name: fp})
    assert not tuple(local_surface_candidates(board, board.net_routing_rules[0]))


def test_progress_is_observational_and_does_not_change_route_identity():
    board, guides = fixture()
    events = []
    plain = route_critical_nets(board, guides)
    observed = route_critical_nets(board, guides, on_progress=lambda *event: events.append(event))
    assert plain == observed
    assert events == [("started", ("MATCH",), None), ("finished", ("MATCH",), observed.nets[0])]


def test_equal_length_alternative_does_not_rewrite_the_incumbent():
    board, guides = fixture(False)
    guide = replace(guides.routes[0], segments=(GlobalRouteSegment(
        "MATCH", CopperLayer.FRONT, Point.mm(5, 5), Point.mm(10, 5), nm_from_mm(1), "direct"),))
    guides = replace(guides, routes=(guide,))
    original, tracks, _ = _route_single(board, board.net_routing_rules[0], guide)
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected and result.nets[0].strategy == "global_guide"
    assert result.locked_tracks == tracks
    assert result.nets[0].lengths_nm == original.lengths_nm


def test_no_common_pad_layer_does_not_invent_a_surface_connection():
    board, _ = fixture(False)
    board = replace(board, placements=(board.placements[0], replace(board.placements[1], side=BoardSide.BACK)))
    assert not tuple(local_surface_candidates(board, board.net_routing_rules[0]))


def test_a_shorter_exact_tree_replaces_a_multi_terminal_guide_tree(monkeypatch):
    board, guides = fixture()
    monkeypatch.setattr("pcbir.critical.local_surface_candidates", lambda *args: iter(()))
    guide, _, _ = _route_single(board, board.net_routing_rules[0], guides.routes[0])
    result = route_critical_nets(board, guides)
    (net,) = result.nets
    assert len(next(item for item in board.nets if item.name == "MATCH").pads) >= 3
    assert net.connected and net.strategy == "exact_single_net"
    assert net.lengths_nm[0] < guide.lengths_nm[0] and net.via_count == 0
    assert net.guide_length_nm == guide.lengths_nm[0]
    assert not hard_findings(result.board)


def test_native_rejection_preserves_incumbent_even_if_builder_proposes_shorter_copper(monkeypatch):
    board, guides = fixture()
    # Deliberately omit the C branch, emulating a defective proposal builder.
    missing_branch = (TrackSegment("MATCH", Point.mm(5, 5), Point.mm(10, 5),
                                   nm_from_mm(.2), CopperLayer.FRONT),)
    monkeypatch.setattr("pcbir.critical.local_surface_candidates", lambda *args: iter((missing_branch,)))
    original, tracks, _ = _route_single(board, board.net_routing_rules[0], guides.routes[0])
    # Isolate the local-candidate gate from the exact-tree comparison.
    monkeypatch.setattr("pcbir.critical._route_single_exact",
                        lambda *args: (replace(original, connected=False), (), ()))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected and result.nets[0].strategy == "global_guide"
    assert result.locked_tracks == tracks
    assert result.nets[0].lengths_nm == original.lengths_nm
    assert not hard_findings(result.board)

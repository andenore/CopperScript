from dataclasses import replace
import json

import pytest

from pcbir import (
    BoardOutline, BoardSide, CopperLayer, CopperKeepout, FootprintPad, GlobalRouterOptions,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, TrackSegment,
    route_global, route_critical_nets, run_physical_drc, nm_from_mm,
)
from pcbir.critical import _improve_single_surface, _route_single, _route_single_exact, _validate_candidate
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


def test_length_bounded_surface_repair_uses_fine_geometric_search(monkeypatch):
    import pcbir.detailed as detailed
    import pcbir.critical as critical
    board, guides = fixture(False)
    rule = replace(board.net_routing_rules[0], max_length_nm=nm_from_mm(6))
    board = replace(board, net_routing_rules=(rule,))
    original = detailed.route_detailed
    seen = []

    def capture(search_board, global_route, options, **kwargs):
        seen.append(options)
        assert global_route.routes[0].connected == guides.routes[0].connected
        assert not global_route.routes[0].segments
        assert guides.routes[0].segments
        assert search_board.net_routing_rules[0].max_length_nm == nm_from_mm(6)
        assert search_board.net_routing_rules[0].max_vias == 0
        return original(search_board, global_route, options, **kwargs)

    monkeypatch.setattr(detailed, "route_detailed", capture)
    # Exercise the board-wide fallback independently of the preceding bounded
    # local-corridor search.
    monkeypatch.setattr(critical,"local_surface_candidates",lambda *args,**kwargs:iter(()))
    result, tracks, vias = _route_single_exact(board, rule, guides, [], [])
    assert result.connected and tracks and not vias
    assert seen[0].pitch_nm == seen[0].minimum_repair_pitch_nm
    assert seen[0].layer_preference_cost == seen[0].direction_preference_cost == 0
    assert result.lengths_nm[0] <= rule.max_length_nm


def test_short_surface_clock_uses_existing_local_maze_before_board_wide_search(monkeypatch):
    import pcbir.detailed as detailed
    board,guides=fixture(False)
    rule=replace(board.net_routing_rules[0],kind=RouteKind.CLOCK,max_length_nm=nm_from_mm(9))
    wall=CopperKeepout('local wall',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((
        Point.mm(7,4),Point.mm(8,4),Point.mm(8,6),Point.mm(7,6)))))
    board=replace(board,net_routing_rules=(rule,),copper_keepouts=(wall,))
    def unnecessary(*args,**kwargs):
        pytest.fail('A legal bounded local clock route must not spend the board-wide fallback budget')
    monkeypatch.setattr(detailed,'route_detailed',unnecessary)
    result,tracks,vias=_route_single_exact(board,rule,guides,[],[])
    assert result.connected and result.strategy=='local_surface_maze'
    assert result.lengths_nm[0]<=rule.max_length_nm and tracks and not vias
    assert not hard_findings(replace(board,tracks=tracks))
    assert board.tracks==board.vias==()


def test_fine_surface_repair_does_not_invent_a_connected_global_guide():
    board, guides = fixture(False)
    rule = replace(board.net_routing_rules[0], max_length_nm=nm_from_mm(6))
    board = replace(board, net_routing_rules=(rule,))
    guides = replace(guides, routes=(replace(guides.routes[0], connected=False),))
    result, tracks, vias = _route_single_exact(board, rule, guides, [], [])
    assert not result.connected
    assert not tracks and not vias


@pytest.mark.parametrize("offset", ("0.05", "-0.05"))
def test_local_clock_maze_can_enter_a_legal_channel_on_the_other_terminal_grid(offset, monkeypatch):
    from decimal import Decimal
    import pcbir.local_critical as local
    original=local.surface_path_between
    calls=[]
    expanded=[]
    def measured(*args,**kwargs):
        calls.append(kwargs['state_budget'])
        observe=kwargs['on_progress']
        def record(phase,event,details):
            if phase=='surface_bridge_search' and event=='finished':
                expanded.append(details['expanded_states'])
            observe(phase,event,details)
        return original(*args,**{**kwargs,'on_progress':record})
    monkeypatch.setattr(local,'surface_path_between',measured)
    board, _ = fixture(False)
    end_y = Decimal(5) + Decimal(offset)
    board = replace(board, outline=BoardOutline.rectangle(15, 10),
        placements=(board.placements[0], replace(board.placements[1], position=Point.mm(10, end_y))),
        rules=replace(board.rules, minimum_track_width_nm=nm_from_mm("0.09"),
                      minimum_clearance_nm=nm_from_mm("0.09")),
        net_routing_rules=(replace(board.net_routing_rules[0],kind=RouteKind.CLOCK,
            width_nm=nm_from_mm("0.09"),clearance_nm=nm_from_mm("0.09"),max_length_nm=nm_from_mm(8)),),
        copper_keepouts=tuple(CopperKeepout(name,(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((
            Point.mm(7, low),Point.mm(8, low),Point.mm(8, high),Point.mm(7, high)))))
            for name,low,high in (("top",0,end_y-Decimal("0.055")),
                                  ("bottom",end_y+Decimal("0.055"),10))))
    shield = CopperKeepout("lead shield", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm("5.4","4.5" if Decimal(offset)<0 else "5.01"),
        Point.mm("6.2","4.5" if Decimal(offset)<0 else "5.01"),
        Point.mm("6.2","4.99" if Decimal(offset)<0 else "5.5"),
        Point.mm("5.4","4.99" if Decimal(offset)<0 else "5.5")))))
    board=replace(board,copper_keepouts=(*board.copper_keepouts,shield))
    candidates=tuple(local_surface_candidates(board,board.net_routing_rules[0],maze=True))
    assert candidates
    assert len(calls)==len(expanded)==2 and calls[0]==20_000
    assert calls[1]==20_000-expanded[0] and sum(expanded)<=20_000
    assert not hard_findings(replace(board,tracks=candidates[0]))
    assert not board.tracks and not board.vias


def test_reserved_reverse_phase_still_runs_after_forward_budget_exhaustion(monkeypatch):
    import pcbir.local_critical as local
    from pcbir.routing_clearance import RoutingClearanceIndex
    original = local.surface_path_between
    board, _ = fixture(False)
    calls, expanded = [], []

    def measured(*args, **kwargs):
        calls.append(kwargs["state_budget"])
        observer = kwargs["on_progress"]
        if len(calls) == 1:
            expanded.append(kwargs["state_budget"])
            observer("surface_bridge_search", "finished",
                     {"expanded_states": kwargs["state_budget"], "outcome": "state_budget_exhausted"})
            return None
        def record(phase, event, details):
            expanded.append(details["expanded_states"])
            observer(phase, event, details)
        return original(*args, **{**kwargs, "on_progress": record})

    monkeypatch.setattr(local, "surface_path_between", measured)
    start, end = Point.mm(5, 5), Point.mm(10, "5.05")
    tracks = local._bounded_surface_maze(board, RoutingClearanceIndex(board), "MATCH",
        start, end, nm_from_mm("0.2"), CopperLayer.FRONT,
        budget=40_000, phase_budget=20_000, detour_nm=nm_from_mm(4))
    assert calls == [20_000, 20_000] and sum(expanded) <= 40_000
    assert tracks and tracks[0].start == start and tracks[-1].end == end
    assert not hard_findings(replace(board, tracks=tracks))
    assert board.tracks == board.vias == ()


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("length_mm", (5, 9))
def test_bounded_critical_surface_maze_preserves_length_and_source_owners(renamed, length_mm):
    board, _ = fixture(False)
    name = "RADIO_Z" if renamed else "MATCH"
    poses = tuple(replace(pose, reference=f"PORT_{index}" if renamed else pose.reference)
                  for index, pose in enumerate(board.placements))
    rule = replace(board.net_routing_rules[0], net=name, kind=RouteKind.RF_FEED,
                   max_length_nm=nm_from_mm(length_mm))
    wall = CopperKeepout("immutable-wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(7, 4), Point.mm(8, 4), Point.mm(8, 6), Point.mm(7, 6)))))
    board = replace(board, placements=poses,
        nets=(PhysicalNet(name, tuple(PadReference(pose.reference, "1") for pose in poses)),),
        net_routing_rules=(rule,), copper_keepouts=(wall,))
    assert not tuple(local_surface_candidates(board, rule))
    alternatives = tuple(local_surface_candidates(board, rule, maze=True))
    assert alternatives and all(track.net == name and track.layer is CopperLayer.FRONT
                                for track in alternatives[0])
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("0.5")))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected == (length_mm == 9)
    assert result.board.placements == board.placements and result.board.copper_keepouts == (wall,)
    assert not result.board.vias
    if length_mm == 9:
        assert result.nets[0].lengths_nm[0] <= rule.max_length_nm
        assert not hard_findings(result.board)
    else:
        assert not result.board.tracks


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
    rule = replace(board.net_routing_rules[0], kind=kind, topology="point_to_point")
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
                        lambda *args, **kwargs: (replace(original, connected=False), (), ()))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected and result.nets[0].strategy == "global_guide"
    assert result.locked_tracks == tracks
    assert result.nets[0].lengths_nm == original.lengths_nm
    assert not hard_findings(result.board)


@pytest.mark.parametrize("name", ["FEED", "RENAMED_PORT"])
def test_rf_improvement_checks_bounded_maze_without_inventing_length_limit(name):
    board, guides = fixture(False)
    rule = replace(board.net_routing_rules[0], net=name, kind=RouteKind.RF_FEED,
                   target_impedance_ohms=50, max_length_nm=None)
    wall = CopperKeepout("feed-obstacle", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(7,4), Point.mm(8,4), Point.mm(8,6), Point.mm(7,6)))))
    board = replace(board, nets=(replace(board.nets[0], name=name),),
                    net_routing_rules=(rule,), copper_keepouts=(wall,))
    guide = replace(guides.routes[0], net=name, segments=tuple(replace(s, net=name) for s in guides.routes[0].segments))
    original, tracks, vias = _route_single(board, rule, guide)
    original, tracks, vias = _validate_candidate(board, original, tracks, vias, [], [])
    assert original.connected
    assert not tuple(local_surface_candidates(board, rule))
    result, shortened, actual_vias = _improve_single_surface(board, rule, guide, original, tracks, vias, [], [])
    assert result.connected and result.strategy == "local_surface_tree"
    assert result.lengths_nm[0] < original.lengths_nm[0]
    assert result.local_candidate_attempts == 1
    assert not actual_vias and all(t.layer is CopperLayer.FRONT for t in shortened)
    assert not hard_findings(replace(board, tracks=shortened))
    assert board.net_routing_rules == (rule,) and rule.max_length_nm is None
    assert rule.target_impedance_ohms == 50


def test_rf_shortening_does_not_invent_missing_guide_or_remove_obstacles():
    board, guides = fixture(False)
    rule = replace(board.net_routing_rules[0], kind=RouteKind.RF_FEED)
    board = replace(board, net_routing_rules=(rule,))
    guide = replace(guides.routes[0], connected=False)
    original, tracks, vias = _route_single(board, rule, guide)
    result, actual, actual_vias = _improve_single_surface(board, rule, guide, original, tracks, vias, [], [])
    assert not result.connected and not actual and not actual_vias


def test_rejected_any_angle_cheap_path_does_not_suppress_bounded_octilinear_search(monkeypatch):
    board, _ = fixture(False)
    rule = replace(board.net_routing_rules[0], kind=RouteKind.RF_FEED)
    angled = (TrackSegment(rule.net, Point.mm(5,5), Point.mm(10,6), nm_from_mm('.2'), CopperLayer.FRONT),)
    monkeypatch.setattr('pcbir.local_critical.surface_path', lambda *args: angled)
    assert not tuple(local_surface_candidates(board, rule))
    path = next(local_surface_candidates(board, rule, maze=True))
    assert path and not hard_findings(replace(board, tracks=path))
    assert all(t.start.x_nm == t.end.x_nm or t.start.y_nm == t.end.y_nm
        or abs(t.start.x_nm-t.end.x_nm) == abs(t.start.y_nm-t.end.y_nm) for t in path)


@pytest.mark.parametrize('name', ['RF_PORT', 'OTHER_RADIO'])
@pytest.mark.parametrize('length_mm', [5, 12])
def test_explicit_via_free_rf_tree_preserves_every_shunt_and_original_budgets(name, length_mm):
    board, guides = fixture()
    rule = replace(board.net_routing_rules[0], net=name, kind=RouteKind.RF_FEED,
                   width_nm=nm_from_mm('.22'), clearance_nm=nm_from_mm('.201'),
                   max_length_nm=nm_from_mm(length_mm), target_impedance_ohms=50)
    board = replace(board, nets=(replace(board.nets[0], name=name),), net_routing_rules=(rule,))
    guide = replace(guides.routes[0], net=name, segments=tuple(replace(s, net=name) for s in guides.routes[0].segments))
    guides = replace(guides, routes=(guide,))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected == (length_mm == 12)
    assert board.net_routing_rules == (rule,)
    assert result.board.placements == board.placements and not result.board.vias
    if length_mm == 12:
        assert result.nets[0].strategy == 'local_surface_tree'
        assert not hard_findings(result.board)
        assert all(t.net == name and t.width_nm == rule.width_nm and t.layer is CopperLayer.FRONT
                   for t in result.board.tracks)
        assert result.nets[0].lengths_nm[0] <= rule.max_length_nm
    else:
        assert not result.board.tracks


def test_rf_tree_opt_in_does_not_enable_clocks_or_permitted_vias():
    board, _ = fixture()
    for rule in (replace(board.net_routing_rules[0], kind=RouteKind.CLOCK),
                 replace(board.net_routing_rules[0], kind=RouteKind.RF_FEED, max_vias=1)):
        assert not tuple(local_surface_candidates(board, rule, maze=True))


def test_rf_tree_does_not_drop_unreachable_shunt_or_prior_copper():
    board, _ = fixture()
    rule = replace(board.net_routing_rules[0], kind=RouteKind.RF_FEED)
    wall = CopperKeepout('radio-wall', (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(0,7), Point.mm(30,7), Point.mm(30,8), Point.mm(0,8)))))
    blocked = replace(board, net_routing_rules=(rule,), copper_keepouts=(wall,))
    assert not tuple(local_surface_candidates(blocked, rule, maze=True))


@pytest.mark.parametrize("renamed", (False, True))
def test_explicit_via_free_critical_tree_can_use_bounded_multi_corner_edges(renamed):
    board, _ = fixture()
    name = "STAR" if renamed else "MATCH"
    rule = replace(board.net_routing_rules[0], net=name, max_length_nm=nm_from_mm(30))
    walls = tuple(CopperKeepout(label, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing(tuple(
        Point.mm(x, y) for x, y in points)))) for label, points in (
            ("east", ((6, 4), (7, 4), (7, 8), (6, 8))),
            ("south", (("4.8", 7), (6, 7), (6, 8), ("4.8", 8)))))
    board = replace(board, nets=(replace(board.nets[0], name=name),),
                    net_routing_rules=(rule,), copper_keepouts=walls)
    assert not tuple(local_surface_candidates(board, rule))
    paths = tuple(local_surface_candidates(board, rule, maze=True))
    assert paths and not hard_findings(replace(board, tracks=paths[0]))
    from pcbir.drc import explicit_copper_connectivity
    assert explicit_copper_connectivity(replace(board, tracks=paths[0])).net_connected(board.nets[0])
    assert all(t.net == name and t.layer is CopperLayer.FRONT for t in paths[0])
    assert board.tracks == board.vias == ()
    assert not tuple(local_surface_candidates(replace(board, net_routing_rules=(replace(rule, max_vias=1),)),
                                              replace(rule, max_vias=1), maze=True))

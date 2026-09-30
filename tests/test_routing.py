from __future__ import annotations

from dataclasses import replace
import json

import pytest
from pcbir.placement import generate_placement_candidates
from pcbir.pin_access import local_access_path
from pcbir.routing import (
    GridNode, PinAccess, _build_graph, _pin_access_candidates,
    _planar_capacity, _route_net,
)
from pcbir.routing_clearance import RoutingClearanceIndex

from pcbir import (
    BoardOutline,
    CopperKeepout,
    CopperLayer,
    CopperZone,
    DetailedRouterOptions,
    FootprintPad,
    FeedbackStatus,
    GlobalRouterOptions,
    GlobalRoutingStatus,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementKeepout,
    PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions,
    Point,
    PolygonRing,
    PolygonWithHoles,
    RouteKind,
    Size,
    Stackup,
    nm_from_mm,
    optimize_placement_for_routing,
    route_global,
    route_detailed,
)
from pcbir.routing_layers import (dedicated_plane_layers, routing_layers,
                                  signal_layer_preferences)


def _two_terminal_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "GlobalRoute",
        BoardOutline.rectangle(40, 30),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(5, 15)),
            Placement("J2", footprint.name, Point.mm(35, 15)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )


def test_global_router_is_deterministic_and_produces_guides_not_copper() -> None:
    board = _two_terminal_board()
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))

    first = route_global(board, options)
    second = route_global(board, options)

    assert first == second
    assert first.status is GlobalRoutingStatus.SUCCESS
    assert first.metrics.unrouted_net_count == 0
    assert first.metrics.total_overflow == 0
    assert first.routes[0].segments
    assert board.tracks == ()
    assert board.vias == ()
    assert json.loads(first.to_json())["schema"] == "copperscript-global-route/v0.1"


def test_global_router_reports_an_impossible_keepout_cut() -> None:
    board = replace(
        _two_terminal_board(),
        copper_keepouts=(
            CopperKeepout("wall", (CopperLayer.FRONT, CopperLayer.BACK),
                PolygonWithHoles(PolygonRing((
                    Point.mm(15, 0), Point.mm(25, 0),
                    Point.mm(25, 30), Point.mm(15, 30),
                ))),
            ),
        ),
    )

    result = route_global(
        board,
        GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"), maximum_iterations=2),
    )

    assert result.status is GlobalRoutingStatus.UNREACHABLE
    assert result.metrics.unrouted_net_count == 1
    assert "no capacity-graph path" in result.routes[0].diagnostics[0]


def test_global_router_obeys_layer_specific_copper_keepout() -> None:
    wall = PolygonWithHoles(PolygonRing((
        Point.mm(15, 0), Point.mm(25, 0),
        Point.mm(25, 30), Point.mm(15, 30),
    )))
    board = replace(
        _two_terminal_board(),
        copper_keepouts=(CopperKeepout("front-wall", (CopperLayer.FRONT,), wall),),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", RouteKind.GENERAL, allowed_layers=(CopperLayer.FRONT,),
        ),),
    )
    blocked = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"), maximum_iterations=2)
    )
    assert blocked.status is GlobalRoutingStatus.UNREACHABLE

    back_allowed = replace(board, net_routing_rules=())
    assert route_global(back_allowed, GlobalRouterOptions(
        tile_size_nm=nm_from_mm("2.5"), maximum_iterations=2
    )).status is GlobalRoutingStatus.SUCCESS


def test_board_wide_inner_ground_plane_is_reserved_for_ground() -> None:
    wall = PolygonWithHoles(PolygonRing((
        Point.mm(15, 0), Point.mm(25, 0),
        Point.mm(25, 30), Point.mm(15, 30),
    )))
    plane = CopperZone(
        "ground-plane", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(39, 1),
            Point.mm(39, 29), Point.mm(1, 29),
        ))),
    )
    base = _two_terminal_board()
    board = replace(
        base,
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                         CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        nets=(*base.nets, PhysicalNet("GND", ())),
        zones=(plane,),
        copper_keepouts=(CopperKeepout(
            "surface-wall", (CopperLayer.FRONT, CopperLayer.BACK), wall,
        ),),
    )
    assert dedicated_plane_layers(board) == {CopperLayer.INTERNAL_1: "GND"}
    assert CopperLayer.INTERNAL_1 not in routing_layers(board, "SIGNAL")
    assert CopperLayer.INTERNAL_1 in routing_layers(board, "GND")

    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    assert guide.status is GlobalRoutingStatus.SUCCESS
    assert not any(segment.layer is CopperLayer.INTERNAL_1
                   for route in guide.routes for segment in route.segments)
    routed = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_search_states=20_000,
    ))
    assert routed.metrics.routed_net_count == 1
    assert any(track.layer is CopperLayer.INTERNAL_2 for track in routed.board.tracks)
    assert not any(track.layer is CopperLayer.INTERNAL_1 for track in routed.board.tracks)

    local = replace(board, zones=(replace(plane, outline=PolygonWithHoles(
        PolygonRing((Point.mm(1, 1), Point.mm(5, 1),
                     Point.mm(5, 5), Point.mm(1, 5))),
    )),))
    assert dedicated_plane_layers(local) == {}
    assert CopperLayer.INTERNAL_1 in routing_layers(local, "SIGNAL")


def test_six_layer_soft_preferences_follow_plane_and_alternate_headings() -> None:
    base = _two_terminal_board()
    plane = CopperZone(
        "ground-plane", "GND", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(39, 1),
            Point.mm(39, 29), Point.mm(1, 29),
        ))),
    )
    board = replace(
        base,
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3,
            CopperLayer.INTERNAL_4, CopperLayer.BACK,
        )),
        nets=(*base.nets, PhysicalNet("GND", ())),
        zones=(plane,),
    )
    ranks, headings = signal_layer_preferences(board)
    assert ranks[CopperLayer.INTERNAL_2] == 0
    assert ranks[CopperLayer.FRONT] == 1
    assert ranks[CopperLayer.INTERNAL_3] == 2
    assert headings == {
        CopperLayer.INTERNAL_2: "h",
        CopperLayer.INTERNAL_3: "n",
        CopperLayer.INTERNAL_4: "h",
    }
    without_plane, _ = signal_layer_preferences(replace(board, zones=()))
    assert set(without_plane.values()) == {0}
    strongly_biased = route_global(
        board, GlobalRouterOptions(
            tile_size_nm=nm_from_mm("2.5"), layer_preference_cost=100,
        ),
    )
    assert strongly_biased.status is GlobalRoutingStatus.SUCCESS
    assert any(
        segment.layer is CopperLayer.INTERNAL_2
        for route in strongly_biased.routes for segment in route.segments
    )


def test_declared_ground_plane_consumes_no_global_wire_capacity() -> None:
    base = _two_terminal_board()
    footprint = next(iter(base.footprints.values()))
    board = replace(
        base,
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                         CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        placements=(*base.placements,
                    Placement("G1", footprint.name, Point.mm(5, 10)),
                    Placement("G2", footprint.name, Point.mm(35, 10))),
        nets=(*base.nets, PhysicalNet("GND", (
            PadReference("G1", "1"), PadReference("G2", "1"),
        ))),
        zones=(CopperZone(
            "ground-plane", "GND", (CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing((
                Point.mm(1, 1), Point.mm(39, 1),
                Point.mm(39, 29), Point.mm(1, 29),
            ))),
        ),),
    )

    result = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(5)))
    ground = next(route for route in result.routes if route.net == "GND")
    assert result.status is GlobalRoutingStatus.SUCCESS
    assert result.metrics.deferred_net_count == 1
    assert result.metrics.unrouted_net_count == 0
    assert ground.deferred_to_zone and not ground.connected
    assert not ground.accesses and not ground.segments and not ground.vias
    document = json.loads(result.to_json())
    assert document["metrics"]["deferred_net_count"] == 1
    assert next(item for item in document["routes"] if item["net"] == "GND")[
        "deferred_to_zone"
    ]


def test_surface_courtyard_does_not_block_global_tracks() -> None:
    base = _two_terminal_board()
    wall = PhysicalFootprint(
        "test/surface-wall",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(10, 30),
    )
    board = replace(
        base,
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        footprints={**base.footprints, wall.name: wall},
        placements=(*base.placements, Placement("W1", wall.name, Point.mm(20, 15))),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1),
        ),),
    )

    result = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"), maximum_iterations=1)
    )

    assert result.status is GlobalRoutingStatus.SUCCESS
    assert all(segment.layer is CopperLayer.FRONT for segment in result.routes[0].segments)
    assert result.routes[0].vias == ()


def test_via_only_keepout_does_not_block_surface_track() -> None:
    base = _two_terminal_board()
    keepout = CopperKeepout(
        "drill-exclusion", (CopperLayer.FRONT, CopperLayer.BACK),
        PolygonWithHoles(PolygonRing((
            Point.mm(15, 0), Point.mm(25, 0),
            Point.mm(25, 30), Point.mm(15, 30),
        ))), block_tracks=False, block_vias=True,
    )
    board = replace(base, copper_keepouts=(keepout,), net_routing_rules=(
        NetRoutingRule("SIGNAL", allowed_layers=(CopperLayer.FRONT,), max_vias=0),
    ))
    route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    assert route.status is GlobalRoutingStatus.SUCCESS
    assert route.routes[0].vias == ()


def test_planar_capacity_accounts_for_a_pad_at_the_crossing() -> None:
    first, second = Point.mm(5, 5), Point.mm(10, 5)
    plain = _planar_capacity(
        first, second, nm_from_mm(5), nm_from_mm("0.5"),
        nm_from_mm("0.1"), CopperLayer.FRONT, (), (),
    )
    obstructed = _planar_capacity(
        first, second, nm_from_mm(5), nm_from_mm("0.5"),
        nm_from_mm("0.1"), CopperLayer.FRONT,
        (((CopperLayer.FRONT,), (nm_from_mm(7), nm_from_mm(4),
                                  nm_from_mm(8), nm_from_mm(6))),), (),
    )
    assert 0 < obstructed < plain


def test_surface_pad_cannot_start_on_forbidden_inner_only_layer() -> None:
    base = _two_terminal_board()
    board = replace(base, stackup=Stackup((
        CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.BACK,
    )), net_routing_rules=(NetRoutingRule(
        "SIGNAL", allowed_layers=(CopperLayer.INTERNAL_1,),
    ),))
    result = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(5)))
    assert result.status is GlobalRoutingStatus.UNREACHABLE
    assert "local/global access" in result.routes[0].diagnostics[0]


def test_through_via_transitions_share_one_cell_resource() -> None:
    base = _two_terminal_board()
    board = replace(base, stackup=Stackup((
        CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.BACK,
    )))
    graph = _build_graph(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("5")))
    transitions = [resource for resource in graph.resources.values()
                   if resource.first.x_index == resource.second.x_index == 3
                   and resource.first.y_index == resource.second.y_index == 2
                   and resource.first.layer_index != resource.second.layer_index]
    assert len(transitions) >= 3
    assert len({resource.identifier for resource in transitions}) == 1
    assert transitions[0].capacity == len(graph.via_sites[transitions[0].identifier])


def test_pad_has_multiple_exact_access_candidates_including_via() -> None:
    base = _two_terminal_board()
    board = replace(base, stackup=Stackup((
        CopperLayer.FRONT, CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2, CopperLayer.BACK,
    )))
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm("5"), pin_access_candidates=8)
    graph = _build_graph(board, options)
    pad = PadReference("J1", "1")
    accesses = _pin_access_candidates(
        board, graph, pad, "SIGNAL", board.stackup.copper_layers,
        None, RoutingClearanceIndex(board), options,
    )
    assert len(accesses) > 1
    assert any(access.via is not None for access in accesses)
    clearance = RoutingClearanceIndex(board)
    for access in accesses:
        assert all(clearance.can_track(track.net, track.start, track.end,
                                       track.width_nm, track.layer)
                   for track in access.tracks)
        if access.via:
            via = access.via
            assert clearance.can_via(via.net, via.position, via.size_nm,
                                     via.from_layer, via.to_layer)


def test_global_router_can_choose_later_access_candidate() -> None:
    board = _two_terminal_board()
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm(5), maximum_iterations=1)
    graph = _build_graph(board, options)
    first = PadReference("J1", "1")
    second = PadReference("J2", "1")
    start = GridNode(0, 1, 2)
    farther = GridNode(0, 6, 2)
    nearer = GridNode(0, 4, 2)
    accesses = {
        ("SIGNAL", first): (PinAccess(first, Point.mm(5, 15), start,
                                       graph.point(start), CopperLayer.FRONT),),
        ("SIGNAL", second): (
            PinAccess(second, Point.mm(35, 15), farther,
                      graph.point(farther), CopperLayer.FRONT),
            PinAccess(second, Point.mm(35, 15), nearer,
                      graph.point(nearer), CopperLayer.FRONT),
        ),
    }
    usage = {item.identifier: 0 for item in graph.resources.values()}
    route = _route_net(board, graph, "SIGNAL", (first, second), None,
                       accesses, 1, usage, usage, 40, options)
    assert route.connected
    assert route.accesses[1].node == nearer


def test_legal_pad_exit_can_attach_to_region_without_center_stub() -> None:
    board = _two_terminal_board()
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm(5))
    graph = _build_graph(board, options)
    centers = {graph.point(node) for node in graph.legal_nodes}
    base = RoutingClearanceIndex(board)

    class CenterBlocked:
        def can_track(self, net, start, end, width, layer):
            return end not in centers and base.can_track(net, start, end, width, layer)

        def can_via(self, net, position, size, first, second):
            return False

    accesses = _pin_access_candidates(
        board, graph, PadReference("J1", "1"), "SIGNAL",
        (CopperLayer.FRONT,), None, CenterBlocked(), options,
    )
    assert accesses
    assert all(access.region_only for access in accesses)
    assert all(access.tracks and access.via is None for access in accesses)


def test_local_pin_access_finds_legal_dogleg_around_foreign_pad() -> None:
    obstacle = PhysicalFootprint(
        "test/obstacle", (FootprintPad("1", Point(0, 0), Size.mm(1, 1)),),
        Size.mm(2, 2),
    )
    board = PhysicalBoard(
        "PinAccess", BoardOutline.rectangle(10, 10),
        {obstacle.name: obstacle},
        (Placement("B", obstacle.name, Point.mm(5, 5)),), (),
    )
    clearance = RoutingClearanceIndex(board)
    route = local_access_path(
        board, clearance, "SIGNAL", Point.mm(2, 5), Point.mm(8, 5),
        CopperLayer.FRONT, nm_from_mm("0.2"), step_nm=nm_from_mm("0.5"),
        detour_nm=nm_from_mm("2"),
    )
    assert route is not None and len(route) >= 2
    assert all(clearance.can_track(item.net, item.start, item.end,
                                   item.width_nm, item.layer) for item in route)


def test_global_router_reports_physical_capacity_overflow() -> None:
    board = replace(
        _two_terminal_board(),
        net_routing_rules=(
            NetRoutingRule(
                "SIGNAL",
                RouteKind.POWER,
                width_nm=nm_from_mm("4.8"),
                clearance_nm=nm_from_mm("0.5"),
                allowed_layers=(CopperLayer.FRONT,),
                max_vias=0,
            ),
        ),
    )

    result = route_global(
        board,
        GlobalRouterOptions(tile_size_nm=nm_from_mm("5"), maximum_iterations=3),
    )

    assert result.status is GlobalRoutingStatus.OVERFLOW
    assert result.metrics.total_overflow > 0
    assert result.hotspots
    assert all(segment.layer is CopperLayer.FRONT for segment in result.routes[0].segments)
    assert result.routes[0].vias == ()


def test_routing_rules_validate_net_layer_and_partner_references() -> None:
    board = _two_terminal_board()

    with pytest.raises(ValueError, match="unknown net"):
        replace(board, net_routing_rules=(NetRoutingRule("MISSING"),))
    with pytest.raises(ValueError, match="unknown partner"):
        replace(
            board,
            net_routing_rules=(
                NetRoutingRule(
                    "SIGNAL",
                    RouteKind.DIFFERENTIAL,
                    differential_partner="MISSING",
                    pair_gap_nm=nm_from_mm("0.2"),
                ),
            ),
        )


def test_real_global_route_certifies_placement_feedback_deterministically() -> None:
    board = _two_terminal_board()
    placement_options = PlacementPlannerOptions(
        candidate_count=2,
        analytical_iterations=4,
        refinement_passes=0,
    )
    router_options = GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))

    first = optimize_placement_for_routing(board, placement_options, router_options)
    second = optimize_placement_for_routing(board, placement_options, router_options)

    assert first == second
    assert first.status is FeedbackStatus.PASS
    assert first.full_route_certified
    assert first.global_route.status is GlobalRoutingStatus.SUCCESS


def test_placement_feedback_can_select_named_legal_candidate() -> None:
    board = _two_terminal_board()
    placement_options = PlacementPlannerOptions(
        candidate_count=2, analytical_iterations=4, refinement_passes=0,
    )
    candidate = generate_placement_candidates(board, placement_options)[-1]
    options = PlacementRoutingFeedbackOptions(
        preferred_candidate_id=candidate.candidate_id,
    )

    result = optimize_placement_for_routing(
        board, placement_options,
        GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")), options,
    )

    assert result.placement_candidate == candidate.candidate_id
    with pytest.raises(ValueError, match="unavailable"):
        optimize_placement_for_routing(
            board, placement_options,
            GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")),
            replace(options, preferred_candidate_id="missing"),
        )


def test_feedback_rolls_back_when_placement_cannot_relieve_capacity() -> None:
    board = replace(
        _two_terminal_board(),
        net_routing_rules=(
            NetRoutingRule(
                "SIGNAL",
                RouteKind.POWER,
                width_nm=nm_from_mm("4.8"),
                clearance_nm=nm_from_mm("0.5"),
                allowed_layers=(CopperLayer.FRONT,),
                max_vias=0,
            ),
        ),
    )

    result = optimize_placement_for_routing(
        board,
        PlacementPlannerOptions(
            candidate_count=1,
            analytical_iterations=0,
            refinement_passes=0,
        ),
        GlobalRouterOptions(tile_size_nm=nm_from_mm("5"), maximum_iterations=2),
        PlacementRoutingFeedbackOptions(
            maximum_iterations=2,
            initial_movement_nm=nm_from_mm("2.5"),
            minimum_movement_nm=nm_from_mm("0.5"),
            maximum_trials_per_iteration=4,
            stagnation_limit=2,
        ),
    )

    assert result.status is FeedbackStatus.WARNING
    assert not result.full_route_certified
    assert result.global_route.status is GlobalRoutingStatus.OVERFLOW
    assert result.iterations
    assert all(item.total_overflow > 0 for item in result.iterations)
    assert result.accepted_moves == sum(item.accepted for item in result.iterations)

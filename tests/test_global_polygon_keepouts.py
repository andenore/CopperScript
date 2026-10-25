"""Concave keepouts preserve real regions without opening isolation cuts."""
from dataclasses import replace
from pcbir import (BoardOutline, CopperKeepout, CopperLayer, GlobalRouterOptions,
                   NetRoutingRule, Point, PolygonRing, PolygonWithHoles, RouteKind)
from pcbir.routing import route_global, _build_graph, GridNode
from test_detailed_routing import _board


def _island_board(*, across=False):
    source = _board()
    barrier = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
        (0, 5), (5, 5), (5, 15), (15, 15), (15, 0), (17, 0),
        (17, 17), (3, 17), (3, 7), (0, 7)))))
    return replace(source, outline=BoardOutline.rectangle(22, 20), placements=(
        replace(source.placements[0], position=Point.mm(8, 3)),
        replace(source.placements[1], position=Point.mm(19 if across else 12, 11)),
    ), copper_keepouts=(CopperKeepout("isolation", source.stackup.copper_layers, barrier),),
        net_routing_rules=(NetRoutingRule("SIGNAL", RouteKind.GENERAL,
                                         allowed_layers=(CopperLayer.FRONT,), max_vias=0),))


def test_concave_barrier_does_not_block_its_entire_bounding_rectangle():
    board = _island_board()
    options = GlobalRouterOptions(tile_size_nm=2_000_000, maximum_iterations=1)
    graph = _build_graph(board, options)
    assert GridNode(0, 4, 4) in graph.legal_nodes  # (9, 9) mm, inside the island
    assert GridNode(0, 7, 4) not in graph.legal_nodes  # (15, 9), actual barrier
    result = route_global(board, options)
    assert result.routes[0].connected
    assert board.tracks == board.vias == ()


def test_concave_barrier_still_separates_the_two_domains():
    result = route_global(_island_board(across=True),
        GlobalRouterOptions(tile_size_nm=2_000_000, maximum_iterations=1))
    assert not result.routes[0].connected


def test_a_thin_wall_between_coarse_centers_cannot_be_skipped():
    source = _board()
    wall = PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in (
        (7.2, 0), (7.3, 0), (7.3, 12), (7.2, 12)))))
    board = replace(source, copper_keepouts=(CopperKeepout("thin", source.stackup.copper_layers, wall),))
    result = route_global(board, GlobalRouterOptions(tile_size_nm=2_000_000, maximum_iterations=1))
    assert not result.routes[0].connected

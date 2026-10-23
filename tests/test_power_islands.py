from __future__ import annotations

from dataclasses import replace

from pcbir import CopperLayer, nm_from_mm
from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles, Point
from pcbir.power_islands import auto_power_layer, plan_power_islands
from pcbir.routing_layers import dedicated_plane_layers, routing_layers

from test_unified_router import _board, _route


def _ground_board():
    board = _board({"P1": ((3, 4), (8, 4), (5, 8)), "P2": ((14, 14), (18, 16)), "S": ((2, 18), (18, 2)),
                    "GND": ((10, 12), (16, 6))},
                   layers=4)
    ring = PolygonRing(tuple(Point.mm(x, y) for x, y in ((0.5, 0.5), (25.5, 0.5), (25.5, 25.5), (0.5, 25.5))))
    zone = CopperZone("gnd", "GND", (CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2), PolygonWithHoles(ring))
    return replace(board, zones=(zone,))


def test_auto_layer_is_the_outermost_of_a_doubled_plane() -> None:
    board = _ground_board()
    assert auto_power_layer(board) is CopperLayer.INTERNAL_2
    single = replace(board, zones=(replace(board.zones[0], layers=(CopperLayer.INTERNAL_1,)),))
    assert auto_power_layer(single) is None


def test_islands_cover_their_rail_pads_and_keep_the_layer_a_plane() -> None:
    board = _ground_board()
    partition = plan_power_islands(board, CopperLayer.INTERNAL_2, ("P1", "P2"))
    assert set(partition.rails) == {"P1", "P2"}
    assert {zone.net for zone in partition.zones} == {"P1", "P2"}
    assert all(zone.priority > board.zones[0].priority for zone in partition.zones)
    assert all(zone.layers == (CopperLayer.INTERNAL_2,) for zone in partition.zones)
    # P1's three nearby pads share one island.
    assert sum(zone.net == "P1" for zone in partition.zones) == 1
    split = replace(board, zones=(*board.zones, *partition.zones))
    assert dedicated_plane_layers(split)[CopperLayer.INTERNAL_2] == "GND"
    assert CopperLayer.INTERNAL_2 not in routing_layers(split, "S")


def test_rails_tap_into_their_islands_with_clean_copper() -> None:
    board = _ground_board()
    partition = plan_power_islands(board, CopperLayer.INTERNAL_2, ("P1", "P2"))
    board = replace(board, zones=(*board.zones, *partition.zones))
    result, drc, summary = _route(board, pitch_nm=nm_from_mm("0.25"))
    assert all(net.connected for net in result.nets), [(n.net, n.diagnostics) for n in result.nets]
    assert summary["drc"]["errors"] == 0
    # Rail copper never runs on the shared plane layer outside its islands.
    assert not [t for t in result.board.tracks if t.layer is CopperLayer.INTERNAL_2]


def test_outline_keeps_holes_and_separates_diagonal_cells() -> None:
    import numpy as np
    from pcbir.power_islands import _outline
    ring = np.ones((5, 5), dtype=bool)
    ring[2, 2] = False
    outline = _outline(ring, 0, 0, 10)
    assert len(outline.outer.vertices) == 4 and len(outline.holes) == 1
    assert len(outline.holes[0].vertices) == 4
    diagonal = np.zeros((2, 2), dtype=bool)
    diagonal[0, 0] = diagonal[1, 1] = True
    outline = _outline(diagonal, 0, 0, 10)
    assert len(outline.outer.vertices) == 4 and not outline.holes


def test_a_rail_whose_rule_excludes_the_layer_gets_no_island() -> None:
    from pcbir.physical import NetRoutingRule, RouteKind
    board = _ground_board()
    rule = NetRoutingRule("P1", RouteKind.GENERAL,
                          allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.BACK))
    board = replace(board, net_routing_rules=(rule,))
    partition = plan_power_islands(board, CopperLayer.INTERNAL_2, ("P1", "P2"))
    assert {zone.net for zone in partition.zones} == {"P2"}
    assert "P1" in partition.skipped


def test_a_rail_one_island_cannot_connect_keeps_no_island() -> None:
    # Q's two pads sit on either side of P2's pads in a narrow host strip, so
    # its corridor would have to cross P2: it is left to ordinary routing.
    board = _board({"Q": ((3, 10), (17, 10)), "P2": ((10, 10), (10, 11))}, layers=4)
    ring = PolygonRing(tuple(Point.mm(x, y) for x, y in ((1, 12), (21, 12), (21, 14.6), (1, 14.6))))
    zone = CopperZone("gnd", "GND", (CopperLayer.INTERNAL_2,), PolygonWithHoles(ring))
    from pcbir.physical import PhysicalNet
    board = replace(board, zones=(zone,), nets=(*board.nets, PhysicalNet("GND", ())))
    split = plan_power_islands(board, CopperLayer.INTERNAL_2, ("Q", "P2"))
    assert "Q" in split.split and {zone.net for zone in split.zones} <= {"P2"}
    loose = plan_power_islands(board, CopperLayer.INTERNAL_2, ("Q", "P2"), single_island=False)
    assert sum(zone.net == "Q" for zone in loose.zones) > 1


def test_island_outlines_have_no_holes() -> None:
    import numpy as np
    from pcbir.power_islands import _outline, _slit_holes
    ring = np.ones((7, 7), dtype=bool)
    ring[3, 3] = False
    slit = _slit_holes(ring)
    outline = _outline(slit, 0, 0, 10)
    assert not outline.holes and slit.sum() == ring.sum() - 3   # a slit of three cells to the top edge


def test_ball_grid_islands_divide_the_field_among_the_rails() -> None:
    from pcbir.geometry import point_in_polygon
    from pcbir.power_islands import plan_ball_grid_islands
    from test_escape_plan import ball_grid
    # 6x6 balls at 0.8 mm (ball (0, 0) at (10, 10) mm); rails N8 (ball (1, 1)) and N29 (ball (4, 4)).
    board = ball_grid(6, "0.45", plane=frozenset({1}))
    partition = plan_ball_grid_islands(board, CopperLayer.BACK, ("N8", "N29"))
    assert set(partition.rails) == {"N8", "N29"}
    islands = {net: [z for z in partition.zones if z.net == net] for net in ("N8", "N29")}
    # Each rail's outward cell lies in one of its islands; the rails split the field between them.
    assert any(point_in_polygon(Point.mm("10.4", "10.4"), z.outline.outer.vertices) for z in islands["N8"])
    assert any(point_in_polygon(Point.mm("13.6", "13.6"), z.outline.outer.vertices) for z in islands["N29"])
    assert not any(point_in_polygon(Point.mm("13.6", "13.6"), z.outline.outer.vertices) for z in islands["N8"])
    assert any(point_in_polygon(Point.mm("11.6", "10.4"), z.outline.outer.vertices) for z in islands["N8"])
    assert all(not z.outline.holes and z.priority > board.zones[0].priority for z in partition.zones)


def test_ball_grid_islands_keep_to_allowed_layers() -> None:
    from pcbir.physical import NetRoutingRule, RouteKind
    from pcbir.power_islands import plan_ball_grid_islands
    from test_escape_plan import ball_grid
    board = ball_grid(6, "0.45", plane=frozenset({1}))
    board = replace(board, net_routing_rules=(NetRoutingRule("N8", RouteKind.GENERAL,
                                                             allowed_layers=(CopperLayer.FRONT,)),))
    partition = plan_ball_grid_islands(board, CopperLayer.BACK, ("N8", "N29"))
    assert {z.net for z in partition.zones} == {"N29"} and "N8" in partition.skipped

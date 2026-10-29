from dataclasses import replace

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone, FootprintPad, GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, Stackup, nm_from_mm, route_global)
from pcbir.cli import _parser, _unified_options

SIX = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
       CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)
OPTIONS = GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"), maximum_iterations=2,
                              layer_assignment_passes=3)


def rectangle(x0, y0, x1, y1):
    return PolygonWithHoles(PolygonRing((Point.mm(x0, y0), Point.mm(x1, y0),
                                         Point.mm(x1, y1), Point.mm(x0, y1))))


def crossing_board(rules=()):
    """Three long horizontal nets and one vertical net that F.Cu cannot carry."""
    footprint = PhysicalFootprint("one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                                  Size.mm(1, 1))
    ends = {"A1": ((3.75, 8.75), (36.25, 8.75)), "A2": ((3.75, 13.75), (36.25, 13.75)),
            "A3": ((3.75, 18.75), (36.25, 18.75)), "B": ((21.25, 3.75), (21.25, 26.25))}
    placements, nets = [], []
    for name, (first, second) in ends.items():
        placements += [Placement(f"{name}_1", footprint.name, Point.mm(*first)),
                       Placement(f"{name}_2", footprint.name, Point.mm(*second))]
        nets.append(PhysicalNet(name, (PadReference(f"{name}_1", "1"), PadReference(f"{name}_2", "1"))))
    return PhysicalBoard(
        "layer-balance", BoardOutline.rectangle(40, 30), {footprint.name: footprint},
        tuple(placements), (*nets, PhysicalNet("GND", ())), stackup=Stackup(SIX),
        zones=(CopperZone("plane", "GND", (CopperLayer.INTERNAL_1,), rectangle(0.5, 0.5, 39.5, 29.5)),),
        copper_keepouts=(CopperKeepout("surface-band", (CopperLayer.FRONT,), rectangle(0, 10.6, 40, 11.9)),),
        net_routing_rules=tuple(rules),
    )


def layers(result, net):
    route = next(item for item in result.routes if item.net == net)
    return {segment.layer for segment in route.segments}


def topology(result):
    return {route.net: (sorted((s.start.x_nm, s.start.y_nm, s.end.x_nm, s.end.y_nm) for s in route.segments),
                        len(route.vias), route.length_nm)
            for route in result.routes}


def test_crossing_run_moves_to_a_spare_signal_layer_without_changing_topology():
    board = crossing_board()
    piled = route_global(board, replace(OPTIONS, layer_assignment_passes=0))
    assert {layer for net in ("A1", "A2", "A3", "B") for layer in layers(piled, net)
            if layer is not CopperLayer.FRONT} == {CopperLayer.INTERNAL_2}
    balanced = route_global(board, OPTIONS)
    assert balanced.metrics.total_overflow == 0
    assert layers(balanced, "B") - {CopperLayer.FRONT} == {CopperLayer.INTERNAL_3}
    assert all(layers(balanced, net) - {CopperLayer.FRONT} == {CopperLayer.INTERNAL_2}
               for net in ("A1", "A2", "A3"))
    # Only labels change: same tile paths, via counts, lengths and status.
    assert topology(balanced) == topology(piled)
    assert balanced.metrics.quality_vector == piled.metrics.quality_vector
    assert not any(segment.layer is CopperLayer.INTERNAL_1 or via.from_layer is CopperLayer.INTERNAL_1
                   or via.to_layer is CopperLayer.INTERNAL_1
                   for route in balanced.routes for segment in route.segments for via in route.vias)
    b = next(route for route in balanced.routes if route.net == "B")
    assert all(CopperLayer.INTERNAL_3 in (via.from_layer, via.to_layer) for via in b.vias
               if via.from_layer is not CopperLayer.FRONT or via.to_layer is not CopperLayer.BACK)
    # Deterministic, including the reported fingerprint.
    assert route_global(board, OPTIONS).routing_fingerprint == balanced.routing_fingerprint


def test_explicit_layers_and_non_ordinary_rules_are_never_relabelled():
    restricted = crossing_board((NetRoutingRule("B", RouteKind.GENERAL, allowed_layers=(
        CopperLayer.FRONT, CopperLayer.INTERNAL_2)),))
    result = route_global(restricted, OPTIONS)
    assert layers(result, "B") <= {CopperLayer.FRONT, CopperLayer.INTERNAL_2}
    critical = crossing_board((NetRoutingRule("B", RouteKind.CRITICAL),))
    fixed = route_global(critical, replace(OPTIONS, layer_assignment_passes=0))
    assert (next(r for r in route_global(critical, OPTIONS).routes if r.net == "B")
            == next(r for r in fixed.routes if r.net == "B"))


def test_board_without_crossings_or_demand_is_unchanged():
    board = crossing_board()
    single = replace(board, nets=(board.nets[0], board.nets[-1]))
    assert (route_global(single, OPTIONS).routes
            == route_global(single, replace(OPTIONS, layer_assignment_passes=0)).routes)


def test_local_demand_spreads_a_parallel_bundle_across_signal_layers():
    footprint = PhysicalFootprint("one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                                  Size.mm(1, 1))
    placements, nets = [], []
    for index, y in enumerate((12.9, 13.75, 14.6)):  # one shared tile row
        placements += [Placement(f"P{index}_1", footprint.name, Point.mm(3.75, y)),
                       Placement(f"P{index}_2", footprint.name, Point.mm(36.25, y))]
        nets.append(PhysicalNet(f"P{index}", (PadReference(f"P{index}_1", "1"),
                                              PadReference(f"P{index}_2", "1"))))
    board = replace(crossing_board(), placements=tuple(placements), nets=(*nets, PhysicalNet("GND", ())))
    inner = lambda result: {layer for net in nets for layer in layers(result, net.name)} - {CopperLayer.FRONT}
    plain = route_global(board, OPTIONS)
    assert inner(plain) == {CopperLayer.INTERNAL_2}
    # One detailed lane per 2.5 mm tile edge: a second run on the edge is full.
    spread = route_global(board, replace(OPTIONS, local_demand_cost=20,
                                         local_demand_pitch_nm=nm_from_mm("2.5")))
    assert len(inner(spread)) > 1 and CopperLayer.INTERNAL_1 not in inner(spread)
    assert topology(spread) == topology(plain)
    assert spread.metrics.total_overflow <= plain.metrics.total_overflow


def test_board_router_enables_smoothing_and_keeps_layer_balance_opt_in():
    args = _parser().parse_args(["route-board", "example.copper"])
    assert (args.layer_assignment_passes, args.local_demand_cost,
            _unified_options(args).route_smoothing) == (0, 0, True)
    assert GlobalRouterOptions().layer_assignment_passes == 0
    enabled = _parser().parse_args([
        "route-board", "example.copper", "--layer-assignment-passes", "3", "--local-demand-cost", "20",
        "--unified-set", "route_smoothing=false",
    ])
    assert (enabled.layer_assignment_passes, enabled.local_demand_cost,
            _unified_options(enabled).route_smoothing) == (3, 20, False)

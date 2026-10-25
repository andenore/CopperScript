"""Uncommitted sampled sites cannot monopolize real terminal launch capacity."""
from dataclasses import replace
from pcbir import routing
from pcbir.global_terminals import register_terminal_capacity
from pcbir.physical import CopperLayer, PadReference, Point, Via, PhysicalNet
from pcbir.routing_clearance import RoutingClearanceIndex
from test_detailed_routing import _board

F, B = CopperLayer.FRONT, CopperLayer.BACK


def _graph(sites):
    a, b = routing.GridNode(0, 0, 0), routing.GridNode(1, 0, 0)
    identifier = "via:0:1:0:0"
    return routing._Graph((F, B), (4_000_000,), (6_000_000,), frozenset((a, b)),
        {routing._edge_key(a, b): routing._Resource(identifier, a, b, len(sites), None)},
        {identifier: tuple(sites)}, {}, {})


def test_terminal_launch_replaces_conflicting_sample_without_adding_free_capacity():
    board = _board()
    sample, remote, launch = Point.mm(4, 6), Point.mm(6, 6), Point.mm("4.25", 6)
    graph = _graph((sample, remote))
    via = Via("SIGNAL", launch, board.rules.default_via_size_nm, board.rules.default_via_drill_nm)
    choice = routing.PinAccess(PadReference("J1", "1"), Point.mm(3, 6),
        routing.GridNode(1, 0, 0), Point.mm(4, 6), B, via=via)
    result, accepted = register_terminal_capacity(board, graph, {("SIGNAL", choice.pad): (choice,)})
    assert accepted["SIGNAL", choice.pad] == (choice,)
    assert result.via_sites["via:0:1:0:0"] == (launch, remote)
    assert all(r.capacity == 2 for r in result.resources.values())
    witnesses = tuple(replace(via, net=f"witness-{i}", position=p)
                      for i, p in enumerate(result.via_sites["via:0:1:0:0"]))
    clear = RoutingClearanceIndex(board)
    assert clear.candidate_vias_clear(witnesses)
    assert all(clear.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer,
                            v.drill_nm, check_hole_copper=True) for v in witnesses)
    assert graph.via_sites["via:0:1:0:0"] == (sample, remote)
    assert board.tracks == board.vias == ()


def test_actual_input_via_can_never_be_replaced_by_a_capacity_proposal():
    source = _board()
    fixed = Via("FIXED", Point.mm(4, 6), source.rules.default_via_size_nm,
                source.rules.default_via_drill_nm)
    board = replace(source, nets=(*source.nets, PhysicalNet("FIXED", ())), vias=(fixed,))
    graph = _graph(())
    via = replace(fixed, net="SIGNAL", position=Point.mm("4.25", 6))
    choice = routing.PinAccess(PadReference("J1", "1"), Point.mm(3, 6),
        routing.GridNode(1, 0, 0), Point.mm(4, 6), B, via=via)
    result, accepted = register_terminal_capacity(board, graph, {("SIGNAL", choice.pad): (choice,)})
    assert not accepted["SIGNAL", choice.pad]
    assert result == graph and board.vias == (fixed,)

"""A reserved package escape is one terminal of its pad, not its replacement."""
from collections import Counter
from dataclasses import replace
from types import MappingProxyType

import pytest

import pcbir.detailed as detail
from pcbir.boundary_access import BoundaryAccessOptions, analyze_boundary_access
from pcbir.drc import explicit_copper_connectivity, run_physical_drc
from pcbir.physical import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad, PadReference,
                            PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
                            PolygonRing, PolygonWithHoles, Size, TrackSegment, Via, nm_from_mm)
from pcbir.pin_escape import RoutingAccess
from pcbir.route_cleanup import EscapeChain, release_unused_escapes
from pcbir.routing import route_global
from test_boundary_access import fixture

PAD = PadReference("U", "1")


def keepout(name, left, top, right, bottom):
    return CopperKeepout(name, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(left, top), Point.mm(right, top), Point.mm(right, bottom), Point.mm(left, bottom)))),
        block_tracks=True, block_vias=True, block_zones=False)


def escaped(*, boxed=False, target=(10, 3)):
    """U.1 has a reserved dogbone (east) and a back-side witness to its port.

    J1 sits straight north of U on the surface. ``boxed`` fences the pad,
    stub and launch via with front-side copper keepouts so the escape is the
    pin's only legal exit.
    """
    package = PhysicalFootprint("package", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(4, 4))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.6, .6)),), Size.mm(1, 1))
    board = PhysicalBoard("escape-terminals", BoardOutline.rectangle(20, 16), {"package": package, "one": one},
        (Placement("U", "package", Point.mm(10, 8)), Placement("J1", "one", Point.mm(*target))),
        (PhysicalNet("A", (PAD, PadReference("J1", "1"))),))
    width = board.rules.default_track_width_nm
    stub = TrackSegment("A", Point.mm(10, 8), Point.mm(10.8, 8), width, CopperLayer.FRONT)
    witness = TrackSegment("A", Point.mm(10.8, 8), Point.mm(13, 8), width, CopperLayer.BACK)
    via = Via("A", Point.mm(10.8, 8), board.rules.default_via_size_nm, board.rules.default_via_drill_nm)
    keepouts = ((keepout("west", 8.8, 6.8, 9.4, 9.2), keepout("east", 11.6, 6.8, 12.2, 9.2),
                 keepout("north", 8.8, 6.8, 12.2, 7.4), keepout("south", 8.8, 8.6, 12.2, 9.2))
                if boxed else ())
    board = replace(board, tracks=(stub, witness), vias=(via,), copper_keepouts=keepouts)
    access = RoutingAccess(witness.end, CopperLayer.BACK, via.position, (witness,))
    return board, MappingProxyType({PAD: access}), (stub, witness), via


def route(board, accesses, created, via, **options):
    source = replace(board, tracks=(), vias=())
    settings = dict(pitch_nm=nm_from_mm(.5), maximum_passes=2) | options
    return detail.route_detailed(board, route_global(source), detail.DetailedRouterOptions(**settings),
        fanout_accesses=accesses, fanout_created_tracks=created,
        fanout_created_vias=frozenset({(via.net, via.position)}))


def hard_errors(board):
    return [f for f in run_physical_drc(board).findings if f.severity.value == "error"
            and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def connected(board):
    graph = explicit_copper_connectivity(board)
    return all(graph.net_connected(net) for net in board.nets)


def test_free_surface_pad_routes_directly_and_releases_unused_escape_copper():
    board, accesses, created, via = escaped()
    result = route(board, accesses, created, via, escape_terminals=True)
    assert result.nets[0].connected and connected(result.board)
    assert not result.board.vias
    assert not Counter(created) & Counter(result.board.tracks)
    assert {t.layer for t in result.board.tracks} == {CopperLayer.FRONT}
    assert not hard_errors(result.board)
    # The historical port-only terminal keeps (and must use) the whole chain.
    legacy = route(board, accesses, created, via)
    assert legacy.nets[0].connected and via in legacy.board.vias
    assert Counter(created) <= Counter(legacy.board.tracks)
    assert result.metrics.total_length_nm < legacy.metrics.total_length_nm


def test_constrained_pin_still_uses_and_keeps_its_escape():
    board, accesses, created, via = escaped(boxed=True)
    assert not hard_errors(board)
    result = route(board, accesses, created, via, escape_terminals=True)
    assert result.nets[0].connected and connected(result.board)
    assert via in result.board.vias and created[0] in result.board.tracks
    assert not hard_errors(result.board)


def test_escape_terminals_offer_launch_witness_and_surface_nodes():
    board, accesses, created, via = escaped()
    options = detail.DetailedRouterOptions(pitch_nm=nm_from_mm(.5), escape_terminals=True)
    grid = detail._build_grid(board, options, board.nets[0].pads, accesses)
    chain = (created[0], *accesses[PAD].path)
    surface = ((detail.DetailedNode(0, grid.xs.index(nm_from_mm(10)), grid.ys.index(nm_from_mm(8))),
                Point.mm(10, 8)),)
    found = detail._escape_terminals(board, grid, "A", accesses[PAD], chain,
                                     board.stackup.copper_layers, surface)
    points = {(grid.layers[node.layer_index], origin) for node, origin in found}
    # The launch via joins both layers; witness nodes need no lead-in.
    assert (CopperLayer.FRONT, via.position) in points and (CopperLayer.BACK, via.position) in points
    assert (CopperLayer.BACK, Point.mm(12, 8)) in points and (CopperLayer.BACK, Point.mm(13, 8)) in points
    assert found[-1] == surface[0]
    assert all(grid.point(node) == origin for node, origin in found[:-1])


def loop_copper():
    """Final copper of a net that used its port, then crossed its own pad.

    Pad U.1 -> stub -> launch via -> back witness -> router via at the port
    -> front run west, north and back east straight over the pad -> J1.
    """
    board, accesses, created, via = escaped()
    width = board.rules.default_track_width_nm
    port_via = Via("A", Point.mm(13, 8), via.size_nm, via.drill_nm)
    route = tuple(TrackSegment("A", Point.mm(*a), Point.mm(*b), width, CopperLayer.FRONT) for a, b in (
        ((13, 8), (13, 9.5)), ((13, 9.5), (8.5, 9.5)), ((8.5, 9.5), (8.5, 8)),
        ((8.5, 8), (11.5, 8)), ((11.5, 8), (11.5, 4.5)), ((11.5, 4.5), (10, 3))))
    return board, accesses, created, via, port_via, route


def test_route_crossing_its_own_pad_releases_the_loop_and_its_tail():
    board, accesses, created, via, port_via, route = loop_copper()
    final = replace(board, tracks=(*board.tracks, *route), vias=(*board.vias, port_via))
    assert connected(final)
    chain = EscapeChain("A", (created[0],), (created[1],), via)
    tracks, vias = release_unused_escapes(
        board, final.tracks, final.vias, (chain,), Counter(created),
        mutable_tracks=Counter((*created, *route)), removable_vias=Counter((via, port_via)))
    released = replace(final, tracks=tracks, vias=vias)
    assert connected(released) and not vias
    assert not Counter(created) & Counter(tracks)
    # Only the run from the pad centre onwards survives: no tail, no loop.
    assert min(min(t.start.x_nm, t.end.x_nm) for t in tracks) == nm_from_mm(10)
    assert max(max(t.start.y_nm, t.end.y_nm) for t in tracks) == nm_from_mm(8)
    assert sum(t.end.x_nm - t.start.x_nm for t in tracks if t.start.y_nm == t.end.y_nm) == nm_from_mm(1.5)
    assert not hard_errors(released)


def test_redundant_cycle_branch_is_removed_without_losing_connectivity():
    board, accesses, created, via, port_via, _ = loop_copper()
    width = board.rules.default_track_width_nm
    # Surface route from the pad to J1 plus a longer parallel detour that
    # leaves and rejoins it: a cycle no escape release can explain.
    route = tuple(TrackSegment("A", Point.mm(*a), Point.mm(*b), width, CopperLayer.FRONT) for a, b in (
        ((10, 8), (10, 6)), ((10, 6), (10, 3)), ((10, 6), (8, 6)), ((8, 6), (8, 4)),
        ((8, 4), (10, 4))))
    final = replace(board, tracks=(*board.tracks, *route))
    chain = EscapeChain("A", (created[0],), (created[1],), via)
    tracks, vias = release_unused_escapes(
        board, final.tracks, final.vias, (chain,), Counter(created),
        mutable_tracks=Counter((*created, *route)), removable_vias=Counter((via,)))
    released = replace(final, tracks=tracks, vias=vias)
    assert connected(released) and not vias
    # The detour and the whole escape go; the direct run keeps its identity.
    assert Counter(tracks) == Counter(route[:2])


def test_release_keeps_copper_whose_removal_would_open_the_net():
    board, accesses, created, via, port_via, route = loop_copper()
    # Without the front run over the pad, the port chain is the only path.
    route = route[:1] + route[4:]
    route = (TrackSegment("A", Point.mm(13, 9.5), Point.mm(11.5, 9.5), route[0].width_nm, CopperLayer.FRONT),
             route[0], TrackSegment("A", Point.mm(11.5, 9.5), Point.mm(11.5, 8), route[0].width_nm,
                                    CopperLayer.FRONT), *route[1:])
    final = replace(board, tracks=(*board.tracks, *route), vias=(*board.vias, port_via))
    assert connected(final)
    chain = EscapeChain("A", (created[0],), (created[1],), via)
    tracks, vias = release_unused_escapes(
        board, final.tracks, final.vias, (chain,), Counter(created),
        mutable_tracks=Counter((*created, *route)), removable_vias=Counter((via, port_via)))
    assert Counter(tracks) == Counter(final.tracks) and Counter(vias) == Counter(final.vias)


def test_unowned_escape_copper_is_never_released():
    board, accesses, created, via, port_via, route = loop_copper()
    final = replace(board, tracks=(*board.tracks, *route), vias=(*board.vias, port_via))
    chain = EscapeChain("A", (created[0],), (created[1],), via)
    tracks, vias = release_unused_escapes(
        board, final.tracks, final.vias, (chain,), Counter(),
        mutable_tracks=Counter(route), removable_vias=Counter((port_via,)))
    assert Counter(created) <= Counter(tracks) and via in vias


def test_escape_terminal_routing_is_deterministic():
    board, accesses, created, via = escaped()
    first = route(board, accesses, created, via, escape_terminals=True)
    second = route(board, accesses, created, via, escape_terminals=True)
    assert first.board.tracks == second.board.tracks and first.board.vias == second.board.vias
    assert first.routing_fingerprint == second.routing_fingerprint


def test_boards_without_escapes_route_identically():
    board, _, _, _ = escaped()
    source = replace(board, tracks=(), vias=())
    guides = route_global(source)
    results = [detail.route_detailed(source, guides, detail.DetailedRouterOptions(
        pitch_nm=nm_from_mm(.5), maximum_passes=2, escape_terminals=flag, route_smoothing=True))
        for flag in (False, True)]
    assert results[0].board == results[1].board
    assert results[0].routing_fingerprint == results[1].routing_fingerprint


@pytest.mark.parametrize("defect", ["missing_path", "missing_via"])
def test_unverified_reserved_escape_still_fails_closed(defect):
    board, accesses, created, via = escaped()
    if defect == "missing_path":
        board = replace(board, tracks=board.tracks[:1])
    else:
        board = replace(board, vias=())
    result = route(board, accesses, created, via, escape_terminals=True)
    assert not result.nets[0].connected
    assert "unverified boundary anchor" in result.nets[0].diagnostics[0]


def test_destination_ports_face_the_nets_other_terminal():
    fan = fixture()
    # J1 moves below the package; the nearest collar edge is still the right one.
    board = replace(fan.board, placements=(fan.board.placements[0],
                                           replace(fan.board.placements[1], position=Point.mm(6, 13.5))))
    fan = replace(fan, board=board)
    nearest = analyze_boundary_access(board, fan)
    facing = analyze_boundary_access(board, fan, BoundaryAccessOptions(destination_ports=True))
    assert nearest.ready and facing.ready
    assert nearest.ports[0].edge == "right" and facing.ports[0].edge == "bottom"
    assert facing.ports[0].path[0].start == fan.accesses[PAD]
    assert analyze_boundary_access(board, fan, BoundaryAccessOptions(destination_ports=True)) == facing

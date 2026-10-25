"""Unified routing over a nested lattice (multi-resolution plan, S2)."""

from __future__ import annotations

from dataclasses import replace
from math import atan2, cos, pi, sin

import numpy as np
import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    DesignRules,
    FootprintPad,
    GlobalRouterOptions,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    Stackup,
    TrackSegment,
    nm_from_mm,
    route_global,
    run_physical_drc,
)
from pcbir.physical import NetRoutingRule
from pcbir.route_summary import connection_summary
from pcbir.routing_lattice import NestedLattice, UniformLattice
from pcbir.unified_router import UnifiedRouterOptions, _Router, route_unified

import test_unified_router as small
from test_fine_regions import _ball_array

FINE = nm_from_mm("0.125")


def _bga_board(*, width=20, height=20, layers=4, n=5, centre=(10, 10), ring_mm=6.0) -> PhysicalBoard:
    """An n x n 0.75 mm ball array whose balls each connect to one pad outside it.

    Pads lie on a square ring around the array in the balls' angular order,
    so the escape is planar apart from the inner balls.
    """
    array = _ball_array(n, "0.75", name=f"test/bga{n * n}_0.75")
    pad = PhysicalFootprint("test/pad", (FootprintPad("1", Point(0, 0), Size.mm("0.5", "0.5")),), Size.mm(1, 1))
    cx, cy = (nm_from_mm(v) for v in centre)
    balls = sorted(array.pads, key=lambda p: atan2(p.position.y_nm, p.position.x_nm))
    placements = [Placement("U1", array.name, Point(cx, cy))]
    nets = []
    ring = nm_from_mm(str(ring_mm))
    step = 2 * pi / len(balls)
    for index, ball in enumerate(balls):
        angle = -pi + (index + 0.5) * step
        dx, dy = cos(angle), sin(angle)
        scale = ring / max(abs(dx), abs(dy))
        # Pads on the ring, on the 0.25 mm lattice.
        x = cx + round(dx * scale / nm_from_mm("0.25")) * nm_from_mm("0.25")
        y = cy + round(dy * scale / nm_from_mm("0.25")) * nm_from_mm("0.25")
        reference = f"P{index}"
        placements.append(Placement(reference, pad.name, Point(x, y)))
        nets.append(PhysicalNet(f"N{index}", (PadReference("U1", ball.number), PadReference(reference, "1"))))
    return PhysicalBoard(
        "NestedBGA", BoardOutline.rectangle(width, height), {array.name: array, pad.name: pad},
        tuple(placements), tuple(nets),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, *(CopperLayer(f"In{i}.Cu") for i in range(1, layers - 1)),
                                       CopperLayer.BACK)),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.1"), default_via_size_nm=nm_from_mm("0.4"),
                          default_via_drill_nm=nm_from_mm("0.2"), minimum_hole_clearance_nm=nm_from_mm("0.15")))


def _route(board: PhysicalBoard, on_progress=None, **options):
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    result = route_unified(board, global_route, UnifiedRouterOptions(**options), on_progress=on_progress)
    drc = run_physical_drc(result.board)
    return result, drc, connection_summary(result.board, drc)


def _setup_event(events):
    return next(d for p, e, d in events if p == "unified_setup" and e == "finished")


# (a) fine_regions="off" is today's router -------------------------------------------


@pytest.mark.parametrize("nets", [
    {"A": ((3, 10), (17, 10))},
    {"H": ((2, 10), (18, 10)), "V": ((10, 2), (10, 18))},
    {**{f"H{i}": ((2, 4 + 2 * i), (18, 4 + 2 * i)) for i in range(3)},
     **{f"V{i}": ((4 + 3 * i, 2), (4 + 3 * i, 18)) for i in range(3)}},
])
def test_off_is_the_default_copper(nets) -> None:
    board = small._board(nets)
    default, _, _ = small._route(board)
    off, _, _ = small._route(board, fine_regions="off")
    assert off.board.tracks == default.board.tracks and off.board.vias == default.board.vias
    assert off == default
    # A board without a qualifying package plans no patch: auto routes on
    # the same uniform 0.25 mm lattice.
    auto, _, _ = small._route(board, fine_regions="auto")
    assert auto.board.tracks == default.board.tracks and auto.board.vias == default.board.vias


def test_options_validate_the_mode() -> None:
    with pytest.raises(ValueError, match="fine_regions"):
        UnifiedRouterOptions(fine_regions="on")
    with pytest.raises(ValueError, match="fine_region_options"):
        UnifiedRouterOptions(fine_regions="auto", fine_region_options={"margin_nm": 0})


# (b), (c) a fine ball array: nested auto against uniform at the fine pitch -----------


@pytest.fixture(scope="module")
def bga_runs():
    board = _bga_board()
    events = []
    nested = _route(board, on_progress=lambda *a: events.append(a), fine_regions="auto")
    uniform_events = []
    uniform = _route(board, on_progress=lambda *a: uniform_events.append(a), pitch_nm=FINE, fine_regions="off")
    return board, (nested, events), (uniform, uniform_events)


def test_bga_routes_completely_on_the_nested_lattice(bga_runs) -> None:
    board, ((result, drc, summary), events), _ = bga_runs
    assert summary["signal"]["opens"] == 0 and all(net.connected for net in result.nets)
    assert summary["drc"]["errors"] == 0
    setup = _setup_event(events)
    assert len(setup["patches"]) == 1 and setup["patches"][0]["factor"] == 2
    # S3: the native kernel serves the nested lattice (no fallback event).
    assert not any(p == "unified_setup" and e == "native_kernel" for p, e, _ in events)
    router = _Router(board, UnifiedRouterOptions(fine_regions="auto"), list(board.nets), None)
    g = router.lattice
    assert isinstance(g, NestedLattice) and setup["nodes"] == g.size
    from pcbir.placement import transformed_local_point
    pose = board.placements[0]
    for ball in board.footprints[pose.footprint].pads:
        node = g.node_at(0, transformed_local_point(pose, ball.position))
        assert node is not None and g.block_index(node % g.plane) == 1


def test_uniform_fine_pitch_reaches_the_same_completion(bga_runs) -> None:
    board, ((nested, _, nested_summary), events), ((uniform, _, summary), uniform_events) = bga_runs
    assert summary["signal"]["opens"] == nested_summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == nested_summary["drc"]["errors"] == 0
    nested_nodes = _setup_event(events)["nodes"]
    grid = _setup_event(uniform_events)["grid"]
    uniform_nodes = grid[0] * grid[1] * grid[2]
    print(f"\nnested nodes {nested_nodes}, uniform 0.125 mm nodes {uniform_nodes}, "
          f"ratio {uniform_nodes / nested_nodes:.2f}; vias {nested.metrics.via_count} vs "
          f"{uniform.metrics.via_count}; length {nested.metrics.total_length_nm / 1e6:.3f} vs "
          f"{uniform.metrics.total_length_nm / 1e6:.3f} mm")
    assert nested_nodes < uniform_nodes


# (d), (e) portals: a path across a patch boundary; claims across it ----------------------


def _patch_router(board: PhysicalBoard, **options) -> _Router:
    return _Router(board, UnifiedRouterOptions(fine_regions="auto", **options), list(board.nets), None)


def _crossing_board() -> PhysicalBoard:
    # Two nets on one layer, each from a ball inside the array's patch to a
    # pad outside it, crossing the patch boundary; X runs across them.
    board = _bga_board(n=5)
    keep = {"N0", "N12"}
    nets = tuple(net for net in board.nets if net.name in keep)
    refs = {pad.component for net in nets for pad in net.pads}
    placements = tuple(p for p in board.placements if p.reference in refs)
    return replace(board, nets=nets, placements=placements)


def test_a_path_from_inside_a_patch_crosses_a_portal_and_commits() -> None:
    board = _crossing_board()
    router = _patch_router(board)
    g = router.lattice
    routes, conflicts, _ = router.negotiate(None)
    assert not conflicts and all(route.connected for route in routes.values())
    crossed = 0
    for route in routes.values():
        for path in route.paths:
            points = [(g.layer_of(n), g.point(n)) for n in path]
            # Collapse portal steps (twins share their point): then no point repeats.
            collapsed = [p for i, p in enumerate(points) if i == 0 or p != points[i - 1]]
            assert len(collapsed) == len(set(collapsed))
            portals = [(a, b) for a, b in zip(path, path[1:]) if g.twin(a) == b]
            crossed += len(portals)
            assert all({g.block_index(a % g.plane), g.block_index(b % g.plane)} == {0, 1} for a, b in portals)
            blocks = {g.block_index(n % g.plane) for n in path}
            if blocks == {0, 1}:
                assert portals
    assert crossed
    accepted, rejected, complete = router.commit(routes, conflicts)
    assert not rejected and complete == set(routes)
    for tracks, _ in accepted.values():
        assert all(t.start != t.end for t in tracks)
    drc = run_physical_drc(replace(board, tracks=tuple(t for v in accepted.values() for t in v[0]),
                                   vias=tuple(v for value in accepted.values() for v in value[1])))
    assert not [f for f in drc.findings if f.severity.value == "error"
                and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def test_claims_cover_portal_twins_so_two_nets_conflict_at_one_boundary_point() -> None:
    board = _crossing_board()
    router = _patch_router(board)
    g = router.lattice
    spec_a, spec_b = router.specs[0], router.specs[1]
    # A boundary point of the patch (a twin pair) on the front layer.
    patch = g.blocks[1]
    point = Point(patch.x0, patch.y0 + 8 * g.pitch)
    fine = g.node_at(0, point)
    base = g.twin(fine)
    assert g.block_index(fine % g.plane) == 1 and g.block_index(base % g.plane) == 0
    # Net A passes through the fine node, net B through the coarse one.
    from pcbir.unified_router import _NetRoute
    inner = fine + patch.nx * 0 + 1          # the fine neighbour inside the patch
    outer = base - 1                         # the coarse neighbour outside it
    a = _NetRoute([[inner, fine]], [()], set(), set(), set(), set(), True, 0.0)
    b = _NetRoute([[outer, base]], [()], set(), set(), set(), set(), True, 0.0)
    router._finish_route(a, spec_a)
    router._finish_route(b, spec_b)
    assert {fine, base} <= a.claims and {fine, base} <= b.claims
    assert {fine, base} <= a.nodes and {fine, base} <= b.nodes
    router.occupancy.add(a, +1)
    router.occupancy.add(b, +1)
    occ = router.occupancy.occ
    assert occ[fine] == occ[base] == 2
    assert router._conflicts({spec_a.name: a, spec_b.name: b}) == {spec_a.name, spec_b.name}


def test_two_nets_forced_through_one_boundary_point_conflict_instead_of_both_committing() -> None:
    # Two front-layer-only nets run from balls inside the patch to pads
    # outside it. A keepout wall on the patch boundary leaves one opening,
    # one track wide, at a portal twin pair: both nets need that site.
    from pcbir.physical import CopperKeepout, PolygonRing, PolygonWithHoles
    board = _bga_board(n=5)
    nets = tuple(net for net in board.nets if net.pads[0].pad in ("R1C0", "R3C0"))
    pads = {net.pads[1].component: side for net, side in zip(nets, (-1, 1))}
    placements = tuple(
        replace(p, position=Point(nm_from_mm(3), nm_from_mm(10) + pads[p.reference] * nm_from_mm(3)))
        if p.reference in pads else p
        for p in board.placements if p.reference == "U1" or p.reference in pads)
    board = replace(board, nets=nets, placements=placements, net_routing_rules=tuple(
        NetRoutingRule(net.name, allowed_layers=(CopperLayer.FRONT,), max_vias=0) for net in nets))
    patch = _patch_router(board).lattice.blocks[1]
    x, gap, half, opening = patch.x0, nm_from_mm(10), nm_from_mm("0.05"), nm_from_mm("0.2")
    walls = tuple(CopperKeepout(f"wall{i}", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point(x - half, y0), Point(x + half, y0), Point(x + half, y1), Point(x - half, y1)))))
        for i, (y0, y1) in enumerate(((nm_from_mm(-1), gap - opening), (gap + opening, nm_from_mm(21)))))
    board = replace(board, copper_keepouts=walls)
    router = _patch_router(board, maximum_iterations=12, commit_rounds=1)
    g = router.lattice
    fine = g.node_at(0, Point(x, gap))
    base = g.twin(fine)
    assert g.block_index(fine % g.plane) == 1 and g.block_index(base % g.plane) == 0
    routes, conflicts, _ = router.negotiate(None)
    # Both paths pass the opening through the twin pair: they keep
    # contesting it (occupancy 2 on both twins) instead of sharing it.
    for route in routes.values():
        assert any(base in path and fine in path for path in route.paths)
        assert {fine, base} <= route.claims
    assert router.occupancy.occ[fine] == router.occupancy.occ[base] == 2
    assert conflicts == {net.name for net in nets}
    accepted, rejected, complete = router.commit(routes, conflicts)
    assert len(complete) == 1 and len(rejected) == 1
    tracks = tuple(t for value in accepted.values() for t in value[0])
    drc = run_physical_drc(replace(board, tracks=tracks))
    assert not [f for f in drc.findings if f.severity.value == "error"
                and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


# (f) congestion export on a nested lattice -----------------------------------------------


def test_congestion_capacity_is_per_area_on_both_lattices(bga_runs) -> None:
    board, ((result, _, _), _), _ = bga_runs
    congestion = result.congestion
    assert congestion is not None
    assert np.isfinite(congestion.utilisation()).all()
    assert np.isfinite(np.asarray(congestion.demand)).all() and np.isfinite(np.asarray(congestion.capacity)).all()
    # An empty board: a 1 mm tile inside the patch (fine nodes) and one
    # outside it (base nodes), both wholly free, have the same capacity.
    empty = replace(board, nets=())
    router = _patch_router(empty, congestion_tile_nm=nm_from_mm(1))
    from pcbir.unified_router import _congestion_map
    congestion = _congestion_map(router, (), nm_from_mm(1))
    patch = router.lattice.blocks[1]
    capacity = np.asarray(congestion.capacity)
    # Tile (7, 7) lies in the patch's margin between its edge (6.75 mm) and
    # the balls; tile (1, 10) is free board far from every pad.
    assert patch.x0 < nm_from_mm(7) and patch.y0 < nm_from_mm(7)
    inside = congestion.tile_of(Point.mm("7.5", "7.5"))
    outside = congestion.tile_of(Point.mm("1.5", "10.5"))
    fine, coarse = capacity[inside[1], inside[0]], capacity[outside[1], outside[0]]
    assert coarse > 0 and abs(fine - coarse) <= 0.1 * coarse
    uniform = _congestion_map(_Router(empty, UnifiedRouterOptions(congestion_tile_nm=nm_from_mm(1)), [], None),
                              (), nm_from_mm(1))
    assert np.isfinite(np.asarray(uniform.capacity)).all()


# (g) a search window is not supported with patches ----------------------------------------


def test_grid_window_with_fine_regions_raises() -> None:
    board = _bga_board()
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    with pytest.raises(ValueError, match="grid_window_nm is not supported with fine_regions"):
        route_unified(board, global_route, UnifiedRouterOptions(
            fine_regions="auto", grid_window_nm=tuple(nm_from_mm(v) for v in (2, 2, 18, 18))))

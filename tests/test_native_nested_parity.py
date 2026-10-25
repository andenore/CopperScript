"""The native kernel on a nested lattice (multi-resolution plan, S3).

``_Router._astar_nested`` (Python) is the reference: on nested fixtures the
kernel must return the same node sequence for random searches under random
congestion, cost terms, neck regions and target-only layers, and route the
nested routing tests' boards to the same copper.
"""

from __future__ import annotations

import random
from dataclasses import replace
from math import atan2, cos, pi, sin
from types import SimpleNamespace

import numpy as np
import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    DesignRules,
    FootprintPad,
    PadReference,
    PadShape,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    Stackup,
    nm_from_mm,
)
from pcbir.routing import GlobalNetRoute, GlobalRouteSegment
from pcbir.routing_lattice import NestedLattice
from pcbir.unified_router import UnifiedRouterOptions, _Router

import test_unified_nested_routing as nested_tests
import test_unified_router as small


@pytest.fixture(autouse=True)
def _kernel():
    import pcbir.native as native
    if native.astar() is None:
        pytest.skip("no native kernel for this platform")


def _array(n: int, pitch: str, ball: str, name: str) -> PhysicalFootprint:
    step = nm_from_mm(pitch)
    half = (n - 1) * step // 2
    pads = tuple(FootprintPad(f"R{row}C{col}", Point(col * step - half, row * step - half),
                              Size.mm(ball, ball), shape=PadShape.CIRCLE)
                 for row in range(n) for col in range(n))
    return PhysicalFootprint(name, pads, Size(nm_from_mm(1) + 2 * half, nm_from_mm(1) + 2 * half))


def _arrays_board(arrays, *, width: int, height: int, layers: int = 4) -> PhysicalBoard:
    """Ball arrays ``(n, pitch, ball size, centre mm, ring mm)``, every ball
    wired to a pad on a ring around its array (as ``_bga_board``)."""
    pad = PhysicalFootprint("test/pad", (FootprintPad("1", Point(0, 0), Size.mm("0.5", "0.5")),), Size.mm(1, 1))
    footprints, placements, nets = {pad.name: pad}, [], []
    q = nm_from_mm("0.25")
    for a, (n, pitch, ball, centre, ring_mm) in enumerate(arrays):
        array = _array(n, pitch, ball, f"test/bga{n * n}_{pitch}")
        footprints[array.name] = array
        cx, cy = (nm_from_mm(v) for v in centre)
        placements.append(Placement(f"U{a}", array.name, Point(cx, cy)))
        balls = sorted(array.pads, key=lambda p: atan2(p.position.y_nm, p.position.x_nm))
        ring, step = nm_from_mm(str(ring_mm)), 2 * pi / len(balls)
        for index, item in enumerate(balls):
            angle = -pi + (index + 0.5) * step
            dx, dy = cos(angle), sin(angle)
            scale = ring / max(abs(dx), abs(dy))
            x, y = cx + round(dx * scale / q) * q, cy + round(dy * scale / q) * q
            reference = f"P{a}_{index}"
            placements.append(Placement(reference, pad.name, Point(x, y)))
            nets.append(PhysicalNet(f"N{a}_{index}", (PadReference(f"U{a}", item.number),
                                                      PadReference(reference, "1"))))
    return PhysicalBoard(
        "NestedParity", BoardOutline.rectangle(width, height), footprints, tuple(placements), tuple(nets),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, *(CopperLayer(f"In{i}.Cu") for i in range(1, layers - 1)),
                                       CopperLayer.BACK)),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.1"), default_via_size_nm=nm_from_mm("0.4"),
                          default_via_drill_nm=nm_from_mm("0.2"), minimum_hole_clearance_nm=nm_from_mm("0.15")))


FIXTURES = {
    # A 0.75 mm array: one k = 2 patch.
    "k2": (lambda: nested_tests._bga_board(), (2,)),
    # A 0.4 mm array: one k = 5 patch (0.05 mm).
    "k5": (lambda: _arrays_board([(5, "0.4", "0.2", (10, 10), 4.0)], width=20, height=20), (5,)),
    # Two 0.75 mm arrays far apart: two patches on one board.
    "two": (lambda: _arrays_board([(4, "0.75", "0.35", (8, 10), 4.0), (4, "0.75", "0.35", (24, 10), 4.0)],
                                  width=32, height=20), (2, 2)),
}


def _guides(router: _Router, names) -> SimpleNamespace:
    """A corridor guide per net, from the board corner region through every patch."""
    g = router.lattice
    points = [Point(g.x0 + 4 * g.pitch, g.y0 + 4 * g.pitch)]
    points += [Point((b.x0 + b.x1) // 2, (b.y0 + b.y1) // 2) for b in g.blocks[1:]]
    points.append(Point(g.x0 + (g.nx - 5) * g.pitch, g.y0 + 4 * g.pitch))
    routes = []
    for name in names:
        segments = tuple(GlobalRouteSegment(name, CopperLayer.FRONT, a, b, nm_from_mm("0.3"), f"r{i}")
                         for i, (a, b) in enumerate(zip(points, points[1:])))
        routes.append(GlobalNetRoute(name, True, (), segments, (), 0))
    return SimpleNamespace(routes=tuple(routes))


def _router(board: PhysicalBoard, **options) -> _Router:
    plain = _Router(board, UnifiedRouterOptions(fine_regions="auto"), list(board.nets), None)
    guides = _guides(plain, [net.name for net in board.nets])
    return _Router(board, UnifiedRouterOptions(fine_regions="auto", bend_cost=0.7, guide_cost=0.9, **options),
                   list(board.nets), None, guides=guides)


def _with_neck(router: _Router, spec, rng: random.Random, centre: int):
    """A copy of ``spec`` with a random neck region around node ``centre``."""
    g = router.lattice
    layer, column = divmod(centre, g.plane)
    point = g.column_point(column)
    region = set(g.nodes_within(layer, point, 6 * g.step_nm(centre)).tolist())
    region |= {g.twin(n) for n in region if g.twin(n) >= 0}
    neck = {n for n in region if rng.random() < 0.5}
    neck_mid = {n for n in region if rng.random() < 0.5}
    arrays = {key: np.fromiter(nodes, dtype=np.int64, count=len(nodes))
              for key, nodes in (("neck", neck), ("neck_mid", neck_mid), ("region", region), ("plane", ()))}
    return replace(spec, neck=frozenset(neck), neck_mid=frozenset(neck_mid), region=frozenset(region),
                   arrays=arrays)


def _congest(router: _Router, rng: random.Random) -> None:
    g, occupancy = router.lattice, router.occupancy
    for _ in range(g.size // 6):
        occupancy.history[rng.randrange(g.size)] = rng.choice((0.5, 1.0, 2.0))
    for _ in range(g.size // 40):
        occupancy.occ[rng.randrange(g.size)] = rng.choice((1, 2))
    for _ in range(g.plane // 20):
        column = rng.randrange(g.plane)
        occupancy.via_history[column] = rng.choice((0.5, 1.5))
        occupancy.via_occ[column] = 1


def _random_node(g: NestedLattice, rng: random.Random, layers, fine: bool) -> int:
    blocks = g.blocks[1:] if fine else g.blocks[:1]
    block = rng.choice(blocks)
    while True:
        ix, iy = rng.randrange(1, block.nx - 1), rng.randrange(1, block.ny - 1)
        column = block.column_offset + iy * block.nx + ix
        if g.live[column]:
            return rng.choice(layers) * g.plane + column


@pytest.mark.parametrize("name", sorted(FIXTURES))
def test_kernel_matches_the_python_nested_search(name) -> None:
    build, factors = FIXTURES[name]
    router = _router(build())
    g = router.lattice
    assert isinstance(g, NestedLattice) and tuple(block.factor for block in g.blocks[1:]) == factors
    kernel = router.native
    assert kernel is not None
    rng = random.Random(sum(map(ord, name)))
    _congest(router, rng)
    searches = crossed = found = turned = 0
    while searches < 60:
        spec = rng.choice(router.specs)
        layers = list(spec.layers)
        mode = searches % 4
        # Sources inside a patch and targets outside it (and back), or both
        # inside: most paths cross a patch boundary.
        source_fine, target_fine = ((True, False), (False, True), (True, True), (False, False))[mode]
        sources = {_random_node(g, rng, layers, source_fine): 0.0}
        for _ in range(rng.randrange(3)):
            sources[_random_node(g, rng, layers, source_fine)] = rng.choice((0.0, 0.5, 1.25))
        targets = {_random_node(g, rng, layers, target_fine) for _ in range(rng.randrange(1, 4))} - set(sources)
        if not targets:
            continue
        if searches % 5 == 0:
            spec = _with_neck(router, spec, rng, next(iter(sources)))
        if searches % 7 == 3 and len(layers) > 1:
            spec = replace(spec, target_only=(layers[-1],))
        extra = {t: rng.choice((0.0, 0.75, 2.5)) for t in targets} if searches % 3 == 1 else None
        present = rng.choice((0.5, 1.3, 4.0))
        router.native = None
        expected = router._search(spec, sources, targets, present, extra)
        router.native = kernel
        router._astar_nested = _no_python_search      # the kernel must not fall back
        assert router._search(spec, sources, targets, present, extra) == expected, (name, searches)
        del router._astar_nested
        searches += 1
        if expected is None:
            continue
        found += 1
        path = expected[0]
        crossed += sum(1 for a, b in zip(path, path[1:]) if g.twin(a) == b)
        turned += sum(1 for a, b, c in zip(path, path[1:], path[2:])
                      if g.layer_of(a) == g.layer_of(c) and g.delta(a, b) != g.delta(b, c))
    # Portals and turns were exercised, and most searches found a path.
    print(f"\n{name}: {searches} searches, {found} found, {crossed} portal steps, {turned} turns")
    assert crossed > 10 and turned > 0 and found >= searches // 2


def _no_python_search(*args, **kwargs):
    raise AssertionError("the native kernel fell back to the Python nested search")


def test_the_kernel_takes_the_nested_lattice_and_its_tables_match_the_search_tables() -> None:
    router = _router(FIXTURES["two"][0]())
    g = router.lattice
    blocks, block_of, ux, uy, twins, steps = g.kernel_tables()
    pux, puy, pblock, ptwins = g.search_tables()
    assert ux.tolist() == list(pux) and uy.tolist() == list(puy)
    assert block_of.tolist() == list(pblock) and twins.tolist() == list(ptwins)
    assert blocks.reshape(-1, 3).tolist() == [[b.column_offset, b.nx, b.ny] for b in g.blocks]
    assert steps.reshape(-1, 8).tolist() == [[m[4] for m in g.block_moves(b.index)] for b in g.blocks]
    tables, _ = router._nested_search_tables()
    assert router._move_base(None).reshape(len(g.blocks), g.nl, 8).tolist() == \
        [[[move[3] for move in table] for table in per_layer] for per_layer in tables]


def _copper(board) -> tuple[list, list]:
    return (sorted((t.net, t.layer.value, t.start.x_nm, t.start.y_nm, t.end.x_nm, t.end.y_nm, t.width_nm)
                   for t in board.tracks),
            sorted((v.net, v.position.x_nm, v.position.y_nm, v.size_nm, v.drill_nm) for v in board.vias))


def _walled_board() -> PhysicalBoard:
    # The keepout wall of test_two_nets_forced_through_one_boundary_point...
    from pcbir.physical import CopperKeepout, NetRoutingRule, PolygonRing, PolygonWithHoles
    board = nested_tests._bga_board(n=5)
    nets = tuple(net for net in board.nets if net.pads[0].pad in ("R1C0", "R3C0"))
    pads = {net.pads[1].component: side for net, side in zip(nets, (-1, 1))}
    placements = tuple(
        replace(p, position=Point(nm_from_mm(3), nm_from_mm(10) + pads[p.reference] * nm_from_mm(3)))
        if p.reference in pads else p
        for p in board.placements if p.reference == "U1" or p.reference in pads)
    board = replace(board, nets=nets, placements=placements, net_routing_rules=tuple(
        NetRoutingRule(net.name, allowed_layers=(CopperLayer.FRONT,), max_vias=0) for net in nets))
    patch = nested_tests._patch_router(board).lattice.blocks[1]
    x, gap, half, opening = patch.x0, nm_from_mm(10), nm_from_mm("0.05"), nm_from_mm("0.2")
    walls = tuple(CopperKeepout(f"wall{i}", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point(x - half, y0), Point(x + half, y0), Point(x + half, y1), Point(x - half, y1)))))
        for i, (y0, y1) in enumerate(((nm_from_mm(-1), gap - opening), (gap + opening, nm_from_mm(21)))))
    return replace(board, copper_keepouts=walls)


def test_nested_routing_boards_route_to_the_python_search_copper(monkeypatch) -> None:
    """Every board of test_unified_nested_routing, routed with the kernel and
    with the Python search (as under COPPER_NO_NATIVE=1): the same copper."""
    import pcbir.native as native
    kernel = native.astar
    cases = [
        ("bga auto", nested_tests._bga_board(), {"fine_regions": "auto"}),
        ("bga uniform 0.125", nested_tests._bga_board(), {"pitch_nm": nested_tests.FINE, "fine_regions": "off"}),
        ("crossing", nested_tests._crossing_board(), {"fine_regions": "auto"}),
        ("walled", _walled_board(), {"fine_regions": "auto", "maximum_iterations": 12, "commit_rounds": 1}),
        ("k5", FIXTURES["k5"][0](), {"fine_regions": "auto"}),
        ("two patches", FIXTURES["two"][0](), {"fine_regions": "auto"}),
    ]
    for nets in ({"A": ((3, 10), (17, 10))}, {"H": ((2, 10), (18, 10)), "V": ((10, 2), (10, 18))}):
        cases.append((f"small {sorted(nets)}", small._board(nets), {"fine_regions": "auto"}))
    for label, board, options in cases:
        results = []
        for use_kernel in (True, False):
            monkeypatch.setattr(native, "astar", kernel if use_kernel else (lambda: None))
            events = []
            result, _, summary = nested_tests._route(board, on_progress=lambda *a: events.append(a), **options)
            assert not any(p == "unified_setup" and e == "native_kernel" for p, e, _ in events)
            results.append((_copper(result.board), summary["signal"]["opens"], summary["drc"]["errors"]))
        assert results[0] == results[1], label

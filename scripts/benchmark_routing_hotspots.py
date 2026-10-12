"""Small, unprofiled, exact-output comparisons for placement/routing hotspots.

Run with ``python -m scripts.benchmark_routing_hotspots --baseline-dir DIR``.
DIR contains pre-change pcbir sources, including backends/kicad_pcb.py. Only
selected function definitions are loaded; helpers/types come from this checkout.
This isolates each intervention rather than comparing different whole pipelines.
Timings are alternating medians, never CI pass/fail thresholds or signoff.
"""
from __future__ import annotations

import argparse
import ast
from contextlib import contextmanager
from dataclasses import replace
from functools import lru_cache
from hashlib import sha256
import inspect
import json
from pathlib import Path
import random
import platform
import statistics
import tempfile
from time import perf_counter

import pcbir.detailed as detailed
import pcbir.drc as drc
import pcbir.hard_macros as macros
import pcbir.placement as placement
import pcbir.surface_path as surface
from pcbir.backends import kicad_pcb as backend
from pcbir import (BoardOutline, BoardSide, CopperLayer, CopperZone, DetailedRouterOptions,
                   FootprintPad, PadReference, PadShape, PadViaInPadRule,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
                   PolygonWithHoles, Size, Stackup, TrackSegment, Via)
from pcbir.clusters import footprint_geometry_digest
from pcbir.geometry import RoundedConvexShape
from pcbir.routing_clearance import RoutingClearanceIndex


def old_functions(module, path, names):
    tree = ast.parse(path.read_text(encoding="utf-8-sig"), filename=str(path))
    nodes = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in names]
    assert {n.name for n in nodes} == set(names)
    namespace = vars(module).copy()
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(path), "exec"), namespace)
    return namespace


@contextmanager
def patched(module, name, value):
    original = getattr(module, name)
    setattr(module, name, value)
    try:
        yield
    finally:
        setattr(module, name, original)


def compare(name, methods, *, repeats=5, prepare=lambda: None):
    missing = object()
    expected = missing
    samples = {method: [] for method in methods}
    for repeat in range(repeats + 1):
        order = list(methods.items())
        if repeat % 2:
            order.reverse()
        for method, function in order:
            prepare()
            started = perf_counter()
            output = function()
            elapsed = perf_counter() - started
            if expected is missing:
                expected = output
            assert output == expected, (name, method, "output differs")
            if repeat:
                samples[method].append(elapsed)
    medians = {key: statistics.median(values) for key, values in samples.items()}
    baseline = next(iter(medians.values()))
    result = dict(case=name, repeats=repeats, exact_outputs=True,
                  output_sha256=sha256(repr(expected).encode()).hexdigest(),
                  seconds=medians, samples_seconds=samples,
                  speedup={key: baseline / value for key, value in medians.items()})
    print(json.dumps({key: value for key, value in result.items() if key != "samples_seconds"}), flush=True)
    return result


def macro_board(scratch):
    """Synthetic six-layer macro plus 1,536 distant package lands."""
    fp = PhysicalFootprint("macro", (FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(1, 1))
    package = PhysicalFootprint("package", tuple(FootprintPad(str(i), Point.mm(
        (i % 4 - 1.5), (i // 4 - 1.5)), Size.mm(.35, .45),
        shape=tuple(PadShape)[i % 4], rotation_degrees=(i % 3) * 45) for i in range(16)), Size.mm(4, 4))
    poses = (Placement("U1", "macro", Point.mm(10, 10)), Placement("R1", "macro", Point.mm(12, 10)),
             *(Placement(f"P{i}", "package", Point.mm(25 + 7 * (i % 12), 25 + 7 * (i // 12)))
               for i in range(96)))
    ground = PhysicalNet("GND", (PadReference("U1", "1"), PadReference("R1", "1")))
    layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
              CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)
    board = PhysicalBoard("hotspot-fixture", BoardOutline.rectangle(120, 120),
        {fp.name: fp, package.name: package}, poses, (ground, PhysicalNet("SIGNAL", ())),
        stackup=Stackup(layers), metadata={"fabrication_profile": "jlcpcb-six-layer"},
        via_in_pad_rules=tuple(PadViaInPadRule(pad) for pad in ground.pads),
        zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(
            PolygonRing((Point.mm(1, 1), Point.mm(119, 1), Point.mm(119, 119), Point.mm(1, 119))))),))
    asset = dict(schema="copperlib-physical-hard-macro/v0.1", source="synthetic performance fixture",
        production_publishable=False, anchor="chip", members=[dict(reference=ref, footprint="macro",
            footprint_digest=footprint_geometry_digest(fp), center_nm=point,
            rotation_degrees="0", edge_clearance_nm=250000)
            for ref, point in (("chip", [0, 0]), ("passive", [2000000, 0]))],
        pad_nets=[["chip", "1", "ground"], ["passive", "1", "ground"]], isolated_pads=[],
        tracks=[dict(net="ground", width_nm=200000, layer="F.Cu",
            points=[{"pad": ["chip", "1"]}, {"pad": ["passive", "1"]}])],
        vias=[dict(net="ground", position_nm=[x, 0], size_nm=300000, drill_nm=200000,
            from_layer="F.Cu", to_layer="B.Cu", technology=None, finish="filled-capped")
            for x in (0, 2000000)], ports=[], protected_regions=[], keepouts=[],
        required_layers=[layer.value for layer in layers], allowed_rotations=[0, 45, 90],
        internal_clearance_nm=0, unresolved=["synthetic fixture, not fabrication evidence"])
    path = scratch / "macro.json"
    path.write_text(json.dumps(asset), encoding="utf-8")
    bound = macros.bind_hard_macro(board, path, expected_sha256=sha256(path.read_bytes()).hexdigest(),
        name="unit", bindings={"chip": "U1", "passive": "R1"}, net_bindings={"ground": "GND"})
    return macros.materialize_hard_macros(bound)


def validation_cases(board, old):
    macro = board.hard_macros[0]
    vias = board.vias
    cached_shape = lru_cache(maxsize=32768)(drc.placed_pad_shape)

    def run(method, *, shape_cache=False):
        with patched(drc, "placed_pad_shape", cached_shape if shape_cache else drc.placed_pad_shape):
            return tuple(method(board, macro, vias) for _ in range(12))

    result = compare("macro-permissions-warm", {
        "linear": lambda: run(old), "cached-pad-linear": lambda: run(old, shape_cache=True),
        "indexed-lands": lambda: run(macros._validate_macro_via_permissions)})

    def cold():
        macros._macro_smd_lands.cache_clear()
        return macros._validate_macro_via_permissions(board, macro, vias)

    return [result, compare("macro-permissions-cold", {
        "linear": lambda: old(board, macro, vias), "indexed-lands": cold})]


def construction_cases(board, old_surface, old_backend, old_validation):
    clear = RoutingClearanceIndex(board)
    args = (board, clear, "SIGNAL", Point.mm(5, 15), Point.mm(9, 19), 250000,
            CopperLayer.FRONT, board.tracks)
    fp = board.footprints["package"]

    def run(function, count=30, *, linear_validation=False):
        with patched(macros, "_validate_macro_via_permissions",
                     old_validation if linear_validation else macros._validate_macro_via_permissions):
            return tuple(function() for _ in range(count))

    return [compare("surface-board-construction", {
        "clone-linear-validation": lambda: run(lambda: old_surface(*args), linear_validation=True),
        "clone-indexed-validation": lambda: run(lambda: old_surface(*args)),
        "explicit-committed-tracks": lambda: run(lambda: surface.surface_path(*args))}),
        compare("library-board-construction", {
        "clone-linear-validation": lambda: run(lambda: old_backend(board, fp, "package"), linear_validation=True),
        "clone-indexed-validation": lambda: run(lambda: old_backend(board, fp, "package")),
        "explicit-uuid-namespace": lambda: run(lambda: backend._render_library_footprint(board, fp, "package"))})]


def placement_cases(board, old_namespace, baseline_path):
    # Same semantic narrow phase, first trying only immutable copper reuse.
    source = ast.get_source_segment(baseline_path.read_text(encoding="utf-8-sig"), next(
        n for n in ast.parse(baseline_path.read_text(encoding="utf-8-sig")).body
        if isinstance(n, ast.FunctionDef) and n.name == "_lands_meet_macro_copper"))
    start = source.index("    copper = [")
    stop = source.index("    if not any", start)
    source = source[:start] + "    copper = _macro_copper_shapes(IdentityKey(tracks), IdentityKey(vias))[0]\n" + source[stop:]
    namespace = vars(placement).copy()
    exec(compile(source, "<copper-only-experiment>", "exec"), namespace)
    copper_only = namespace["_lands_meet_macro_copper"]
    tracks = tuple(TrackSegment("GND", Point.mm(10 + i % 10, 10 + i // 10),
                    Point.mm(10.6 + i % 10, 10.3 + i // 10), 200000, CopperLayer.FRONT)
                   for i in range(100))
    vias = tuple(Via("GND", Point.mm(11 + i, 11), 600000, 300000) for i in range(5))
    rng = random.Random(1210)
    poses = tuple(Placement("P0", "package", Point.mm(rng.randrange(8, 110), rng.randrange(8, 110)),
                            rotation_degrees=rng.choice((0, 45, 90, 135)),
                            side=rng.choice(tuple(BoardSide))) for _ in range(240))
    helpers = {"rebuild-shapes": old_namespace["_lands_meet_macro_copper"],
               "cached-copper": copper_only, "cached-local-and-copper-envelopes": placement._lands_meet_macro_copper}
    results = [compare("placement-macro-copper", {name: lambda f=f: tuple(
        f(board, pose, tracks, vias) for pose in poses) for name, f in helpers.items()})]
    placed = {pose.reference: pose for pose in board.placements if pose.reference != "P0"}
    options = placement.PlacementPlannerOptions()
    original_polygon = old_namespace["_placement_polygon"]
    @lru_cache(maxsize=8192)
    def cached_courtyard(footprint, position, rotation, side):
        fp = footprint.value
        return placement._translated_polygon(placement._rotated_courtyard(
            fp.courtyard, fp.body_size, rotation, side), position)
    def cached_polygon(bb, pose):
        return cached_courtyard(placement.IdentityKey(bb.footprints[pose.footprint]),
                               pose.position, pose.rotation_degrees, pose.side)
    def legality(polygon_function):
        with patched(placement, "_placement_polygon", polygon_function):
            return tuple(placement._legal(p, placed, board, options) for p in poses)
    results.append(compare("placement-legality", {
        "baseline": lambda: tuple(old_namespace["_legal"](p, placed, board, options) for p in poses),
        "cached-macro-envelopes": lambda: legality(original_polygon),
        "macro-envelopes-and-courtyards": lambda: legality(cached_polygon)}))
    return results


def maze_cases(old_namespace, baseline_path):
    text = baseline_path.read_text(encoding="utf-8-sig")
    source = ast.get_source_segment(text, next(n for n in ast.parse(text).body
        if isinstance(n, ast.FunctionDef) and n.name == "_search_once"))
    source = source.replace("    serial = 0", "    congestion_cache = {}\n    serial = 0", 1)
    start = source.index("            congestion = sum(")
    stop = source.index("            outside =", start)
    block = source[start:stop]
    replacement = "            congestion = congestion_cache.get(edge)\n            if congestion is None:\n"
    replacement += "\n".join("    " + line for line in block.rstrip().splitlines()) + "\n"
    replacement += "                congestion_cache[edge] = congestion\n"
    source = source[:start] + replacement + source[stop:]
    namespace = vars(detailed).copy()
    exec(compile(source, "<congestion-only-experiment>", "exec"), namespace)
    # Keep queue ordering and predecessor nodes unchanged; try integer edge
    # and best-cost keys independently of a much broader graph rewrite.
    packed = inspect.getsource(detailed._search_once)
    if "    def node_id(" in packed:
        start = packed.index("    rows, columns =")
        stop = packed.index("    serial = 0", start)
        packed = packed[:start] + packed[stop:]
        packed = packed.replace('        best[node_id(start), ""] = 0', "        best[state] = 0", 1)
        packed = packed.replace("        identifier = node_id(node)\n"
            "        if cost != best.get((identifier, direction)):", "        if cost != best.get(state):", 1)
        packed = packed.replace("            neighbor_id = node_id(neighbor)\n"
            "            edge = (identifier, neighbor_id) if identifier < neighbor_id else (neighbor_id, identifier)",
            "            edge = _edge_key(node, neighbor)", 1)
        packed = packed.replace("best.get((neighbor_id, next_direction), 1 << 60)", "best.get(candidate, 1 << 60)", 1)
        packed = packed.replace("            best[neighbor_id, next_direction] = candidate_cost",
                                "            best[candidate] = candidate_cost", 1)
    cost_namespace = vars(detailed).copy()
    exec(compile(packed, "<edge-cost-only-experiment>", "exec"), cost_namespace)
    packed = packed.replace("    serial = 0", "    rows, columns = len(grid.ys), len(grid.xs)\n"
        "    def node_id(node):\n"
        "        return (node.layer_index * columns + node.x_index) * rows + node.y_index\n"
        "    serial = 0", 1)
    packed = packed.replace("        best[state] = 0", "        best[node_id(start), \"\"] = 0", 1)
    packed = packed.replace("        if cost != best.get(state):", "        identifier = node_id(node)\n"
        "        if cost != best.get((identifier, direction)):", 1)
    packed = packed.replace("            edge = _edge_key(node, neighbor)",
        "            neighbor_id = node_id(neighbor)\n"
        "            edge = (identifier, neighbor_id) if identifier < neighbor_id else (neighbor_id, identifier)", 1)
    packed = packed.replace("best.get(candidate, 1 << 60)", "best.get((neighbor_id, next_direction), 1 << 60)", 1)
    packed = packed.replace("            best[candidate] = candidate_cost", "            best[neighbor_id, next_direction] = candidate_cost", 1)
    packed_namespace = vars(detailed).copy()
    exec(compile(packed, "<packed-key-experiment>", "exec"), packed_namespace)
    methods = {"baseline": old_namespace["_search_once"],
               "congestion-only": namespace["_search_once"], "symmetric-edge-cost": cost_namespace["_search_once"],
               "edge-cost-and-integer-keys": packed_namespace["_search_once"]}
    board = PhysicalBoard("maze-fixture", BoardOutline.rectangle(20, 16), {}, (),
                          (PhysicalNet("S", ()), PhysicalNet("X", ())))
    wall = TrackSegment("X", Point.mm(10, 2), Point.mm(10, 14), 250000, CopperLayer.FRONT)
    board = replace(board, tracks=(wall,))
    from pcbir.routing import GlobalNetRoute, GlobalRouteSegment
    guide = GlobalNetRoute("S", True, (), (GlobalRouteSegment("S", CopperLayer.FRONT,
        Point.mm(2, 8), Point.mm(18, 8), 1000000, "guide"),), (), 16000000)
    results = []
    for octilinear in (False, True):
        options = DetailedRouterOptions(pitch_nm=500000, octilinear_search=octilinear,
                                        layer_preference_cost=10, direction_preference_cost=4)
        usage = {f"node:F.Cu:{x}:8000000": 1 for x in range(4000000, 8000001, 500000)}
        history = {f"node:B.Cu:{x}:8000000": 2 for x in range(11000000, 14000001, 500000)}
        def run(function):
            # Fresh grid/index each time: no cross-method warmed query caches.
            grid = detailed._build_grid(board, options, ())
            start = detailed.DetailedNode(0, grid.xs.index(2000000), grid.ys.index(8000000))
            target = detailed.DetailedNode(0, grid.xs.index(18000000), grid.ys.index(8000000))
            return function(grid, {start}, frozenset((target,)), grid.layers, guide, usage, history,
                RoutingClearanceIndex(board), "S", 250000, options, corridor_only=False,
                state_budget=30000, guide_margin_nm=500000, allow_movable_conflicts=False,
                forbidden_via_positions=frozenset())
        results.append(compare(f"maze-{'octilinear' if octilinear else 'orthogonal'}", {
            name: lambda f=f: run(f) for name, f in methods.items()}))
    # Untimed equivalence matrix exercises directional guide transitions,
    # mutable-obstacle policy, bounded failures, different grids and costs.
    checks = 0
    for pitch in (500000, 700000):
        for mode in range(8):
            options = DetailedRouterOptions(pitch_nm=pitch, octilinear_search=bool(mode % 2),
                via_cost=1 + mode, guide_escape_nm=2000000,
                allow_guide_deviation=mode != 4, layer_preference_cost=mode,
                direction_preference_cost=mode % 3, bend_cost=mode % 4)
            def checked(function):
                bb = replace(board, tracks=()) if mode in (5, 6) else board
                grid = detailed._build_grid(bb, options, ())
                start = detailed.DetailedNode(0, 3, 4)
                target = detailed.DetailedNode(mode % 2, len(grid.xs) - 4, len(grid.ys) - 5)
                clear = RoutingClearanceIndex(bb)
                if mode in (5, 6):
                    clear.add_track(wall)
                try:
                    return function(grid, {start}, frozenset((target,)), grid.layers, guide,
                        usage, history, clear, "S", 250000, options,
                        corridor_only=mode in (2, 3), state_budget=7 if mode == 7 else 30000,
                        guide_margin_nm=500000, allow_movable_conflicts=mode == 5,
                        project_guide_layers=mode in (1, 3),
                        forbidden_via_positions=frozenset((grid.point(start),)) if mode == 6 else frozenset())
                except detailed._SearchBudgetExceeded:
                    return "budget-exceeded"
            expected = checked(methods["baseline"])
            for name, function in methods.items():
                assert checked(function) == expected, (pitch, mode, name)
                checks += 1
    print(json.dumps({"maze_equivalence_checks": checks}), flush=True)
    for result in results:
        result["equivalence_matrix_checks"] = checks
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--only", choices=("macro", "construction", "placement", "maze"))
    args = parser.parse_args()
    root = args.baseline_dir / "pcbir"
    old_macro = old_functions(macros, root / "hard_macros.py", ("_validate_macro_via_permissions",))
    old_surface = old_functions(surface, root / "surface_path.py", ("surface_path",))
    old_backend = old_functions(backend, root / "backends/kicad_pcb.py",
                               ("_render_library_footprint", "_footprint_lines", "_copper_keepout_lines"))
    old_placement = old_functions(placement, root / "placement.py", ("_lands_meet_macro_copper", "_legal", "_placement_polygon"))
    old_maze = old_functions(detailed, root / "detailed.py", ("_search_once",))
    output = {"schema": "copperscript-hotspot-benchmark/v1", "instrumented": False,
              "python": platform.python_version(), "platform": platform.platform(),
              "baseline_hashes": {str(p.relative_to(root)): sha256(p.read_bytes()).hexdigest()
                                  for p in root.rglob("*.py")}, "cases": []}
    output["current_hashes"] = {str(p): sha256(p.read_bytes()).hexdigest() for p in (
        Path("pcbir/_cache.py"), *(Path("pcbir") / p.relative_to(root) for p in root.rglob("*.py")))}
    Path("build").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hotspot-fixture-", dir="build") as directory:
        board = macro_board(Path(directory))
        for name, suite in (("macro", lambda: validation_cases(board, old_macro["_validate_macro_via_permissions"])),
            ("construction", lambda: construction_cases(board, old_surface["surface_path"],
                    old_backend["_render_library_footprint"], old_macro["_validate_macro_via_permissions"]),
            ), ("placement", lambda: placement_cases(board, old_placement, root / "placement.py")),
            ("maze", lambda: maze_cases(old_maze, root / "detailed.py"))):
            if args.only and args.only != name:
                continue
            output["cases"].extend(suite())
            if args.output:
                args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

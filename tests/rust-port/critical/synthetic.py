"""Write replay inputs of synthetic critical-routing boards (see README.md, "Synthetic inputs").

The boards are those the critical-routing unit tests build. Each input is
the dict ``replay-critical`` reads: the board, ``route_global(board)``
computed exactly as its test does, and no reserved accesses or plane-stitch
options. ``--unified`` writes the unified-router inputs under
``tests/rust-port/unified/synthetic/`` instead (``save_inputs_path``, the
same file ``route-board --unified-save-inputs`` writes).

The script re-executes itself with ``PYTHONHASHSEED=0 PYTHONUTF8=1``, as
``generate.py`` runs the CLI.

    python tests/rust-port/critical/synthetic.py [NAME ...]
    python tests/rust-port/critical/synthetic.py --unified [NAME ...]
"""

from __future__ import annotations

import os
import sys
from dataclasses import replace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
OUT = HERE / "synthetic"
UNIFIED_OUT = ROOT / "tests" / "rust-port" / "unified" / "synthetic"
ENVIRONMENT = {"PYTHONHASHSEED": "0", "PYTHONUTF8": "1"}

if __name__ == "__main__" and any(os.environ.get(key) != value for key, value in ENVIRONMENT.items()):
    os.execve(sys.executable, [sys.executable, *sys.argv], {**os.environ, **ENVIRONMENT})

sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tests"))

from pcbir import (  # noqa: E402
    CopperKeepout, CopperLayer, GlobalRouterOptions, NetMatchGroup, Point, PolygonRing, PolygonWithHoles,
    nm_from_mm, route_global,
)
from pcbir.workbench import save_state  # noqa: E402

import test_bundle_tuning as bundle_tuning  # noqa: E402
import test_critical_bundles as bundles  # noqa: E402
import test_group_tuning as group_tuning  # noqa: E402
import test_length_match_tuning as match  # noqa: E402
import test_loose_pair_repair as loose  # noqa: E402
import test_nested_exits as nested  # noqa: E402
import test_planned_crossings as crossings  # noqa: E402
import test_serpentine_tuning as serpentine  # noqa: E402


def _breakout(board, **fields):
    return replace(board, net_routing_rules=tuple(replace(rule, **fields) for rule in board.net_routing_rules))


def _crossing_failed():
    # _swapped() with In2.Cu, the only crossing layer, closed between the rows:
    # the crossing is planned but its pair finds no candidate.
    board = crossings._swapped()
    wall = CopperKeepout("in2-closed", (CopperLayer.INTERNAL_2,), PolygonWithHoles(PolygonRing((
        Point.mm(12, 0), Point.mm(18, 0), Point.mm(18, 20), Point.mm(12, 20)))))
    return replace(board, copper_keepouts=(*board.copper_keepouts, wall))


def _crossing_tuned(amplitude: str = "1.2"):
    # The crossing pair's via paths to In2.Cu count toward its length, so the
    # straight pair needs taller bumps than 0.8 mm ("crossing-tuning-short").
    board = crossings._shared_reference(crossings._swapped())
    nets = tuple(net.name for net in board.nets if net.name != "GND")
    return replace(board, match_groups=(NetMatchGroup("lanes", nets, nm_from_mm("0.1")),),
                   net_routing_rules=tuple(replace(rule, tuning_amplitude_limit_nm=nm_from_mm(amplitude))
                                           for rule in board.net_routing_rules))


def _nested_fallback():
    # The corner wrap with a block across both side pairs' columns: neither
    # nested exit is accepted and both pairs route by the ordinary search.
    board = nested._corner_board()
    block = CopperKeepout("nested-block", (CopperLayer.FRONT, CopperLayer.BACK), PolygonWithHoles(PolygonRing((
        Point.mm("9.5", 14), Point.mm(12, 14), Point.mm(12, 16), Point.mm("9.5", 16)))))
    return replace(board, copper_keepouts=(block,))


def _squeezed():
    board = bundles._bundle_board(bundles._rows(("-1.0", "0", "1.0"), "0.5"),
                                  bundles._rows(("-1.2", "0", "1.2"), "0.4"))
    return _breakout(board, allowed_layers=(CopperLayer.FRONT,), max_vias=0, width_nm=nm_from_mm("0.14"),
                     pair_gap_nm=nm_from_mm("0.26"), clearance_nm=nm_from_mm("0.52"),
                     breakout_length_nm=nm_from_mm("2"), breakout_clearance_nm=nm_from_mm("0.1"),
                     breakout_gap_nm=nm_from_mm("0.2"))


def _match_failed():
    return match._lanes_board(match.TWO_LANES, keepouts=(match._keepout("above", 6, "14.305", 28, 17),
                                                         match._keepout("below", 6, 11, 28, "13.695")))


def _match_within_limit():
    lanes = (("LANE_A", 6, 5, 33), ("LANE_B", 12, 5, 33), ("LANE_C", 18, 5, 33), ("LANE_D", 24, 5, 25))
    return match._lanes_board(lanes, groups={"matched": ("LANE_A", "LANE_B"), "short": ("LANE_C", "LANE_D")})


def _match_incomplete():
    board = match._lanes_board(match.TWO_LANES)
    return replace(board, net_routing_rules=tuple(rule for rule in board.net_routing_rules
                                                  if not rule.net.startswith("LANE_B")))


def _match_breakout_regions():
    return _breakout(match._lanes_board(), breakout_length_nm=nm_from_mm("9.5"),
                     breakout_width_nm=nm_from_mm("0.15"))


def _bumps_outside_breakout():
    board = match._lanes_board(match.TWO_LANES, max_skew="3.7", keepouts=(
        match._keepout("below", 6, 11, 28, "13.69"), match._keepout("above-middle", "9.5", "14.31", 21, 17),
        match._keepout("above-low", 21, "14.81", "24.6", 17)))
    return _breakout(board, breakout_length_nm=nm_from_mm("4"), breakout_clearance_nm=nm_from_mm("0.1"))


def _serpentine():
    return serpentine._serpentine_lanes(keepouts=(match._keepout("above", 6, "14.8", 28, 17),
                                                  match._keepout("below", 6, 11, 28, "13.2")))


# name: (board builder, route_global options or None, replay arguments after the output paths)
CASES = {
    # Critical pair bundles (plan R5, R6): planned crossings.
    "crossing-routed": (crossings._swapped, None, ()),
    "crossing-reversal": (lambda: crossings._reversed(six_layers=True), None, ()),
    "crossing-impossible-layer": (lambda: crossings._reversed(six_layers=False), None, ()),
    "crossing-impossible-vias": (lambda: crossings._swapped(max_vias=1), None, ()),
    "crossing-failed": (_crossing_failed, None, ()),
    "crossing-steps-aside": (lambda: crossings._crossing_beside(
        ("-1", "0", "1"), ("-1.5", "1.5", "0"), ("LANE_A", "LANE_B", "LANE_C"), "LANE_C"), None, ()),
    "crossing-shared-reference": (lambda: crossings._shared_reference(crossings._swapped()), None, ()),
    "crossing-breakout-inside": (lambda: _breakout(crossings._swapped(), breakout_length_nm=nm_from_mm("15"),
                                                   breakout_clearance_nm=nm_from_mm("0.1")), None, ()),
    "crossing-tuned": (_crossing_tuned, None, ()),
    "crossing-tuning-short": (lambda: _crossing_tuned("0.8"), None, ()),
    # Nested corner exits (plan R7).
    "nested-routed": (nested._corner_board, None, ()),
    "nested-breakout": (lambda: nested._corner_board(nested.TIGHT, clearance="0.3", breakout=True), None,
                        ("--pair-states", "4000")),
    "nested-fallback": (_nested_fallback, None, ()),
    # Bundle rip-up repair (plan R6) and the loose (non-bundle) repair.
    "bundle-repair": (lambda: bundles._repair_board(*bundles.REPAIRABLE), None, ("--pair-states", "4000")),
    "bundle-repair-squeezed": (_squeezed, None, ("--pair-states", "20000")),
    "loose-repair": (loose._loose_board, None, ("--pair-states", "2000")),
    # Length-match tuning (plan R3, R9, R10, R12, R14).
    "match-tuned": (match._lanes_board, None, ()),
    "match-failed": (_match_failed, None, ()),
    "match-within-limit": (_match_within_limit, None, ()),
    "match-incomplete": (_match_incomplete, None, ()),
    "match-breakout-regions": (_match_breakout_regions, None, ()),
    "match-bumps-outside-breakout": (_bumps_outside_breakout, None, ()),
    "match-serpentine": (_serpentine, None, ()),
    "bundle-tuning": (lambda: bundle_tuning._bundle(bundle_tuning.HEMMED), None, ("--pair-states", "4000")),
    "group-tuning": (group_tuning._group_board, None, ()),
}


def write(name: str) -> Path:
    build, options, _ = CASES[name]
    board = build()
    route = route_global(board) if options is None else route_global(board, options)
    path = OUT / f"{name}.state.json"
    save_state(path, {"board": board, "global_route": route, "reserved_accesses": None,
                      "plane_stitch_options": None})
    print(f"{path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    return path


# --- Unified router: bus phase ----------------------------------------------


def _bus_skew_board(amplitude: str | None = "1"):
    import test_unified_router as unified
    board = unified._bus_board(amplitude=amplitude)
    extra = unified._board({"C": ((3, 18), (17, 18))})
    return replace(board, footprints={**board.footprints, **extra.footprints},
                   placements=(*board.placements, *extra.placements), nets=(*board.nets, *extra.nets))


def _bus_bundle_board():
    import test_unified_router as unified
    board = unified._array_board({"U": (8, 6), "V": (16, 6)},
                                 {"A": (("U", "15"), ("V", "11")), "B": (("U", "20"), ("V", "16")),
                                  "C": (("U", "10"), ("V", "6"))})
    return replace(board, match_groups=(NetMatchGroup("bus", ("A", "B"), nm_from_mm(5)),))


# name: (board builder, saved UnifiedRouterOptions fields)
UNIFIED = {
    # test_tuned_bus_copper_survives_route_smoothing; replay with --set route_smoothing=true.
    "bus-skew": (_bus_skew_board, {"buses_first": True}),
    # test_buses_first_with_skew_tuning_reports_untunable_members_on_the_board: the skew note.
    "bus-skew-untunable": (lambda: _bus_skew_board(None), {"buses_first": True}),
    # test_bus_bundles_route_the_nets_between_the_bus_parts_with_the_bus; replay with --set bus_bundles=true.
    "bus-bundles": (_bus_bundle_board, {"maximum_iterations": 5}),
}


def write_unified(name: str) -> Path:
    from pcbir.unified_router import UnifiedRouterOptions, route_unified
    build, fields = UNIFIED[name]
    board = build()
    route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    path = UNIFIED_OUT / f"{name}.state.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    route_unified(board, route, UnifiedRouterOptions(save_inputs_path=str(path), **fields))
    print(f"{path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    return path


def main(names: list[str]) -> int:
    unified = names[:1] == ["--unified"]
    cases = UNIFIED if unified else CASES
    names = names[1:] if unified else names
    unknown = set(names) - set(cases)
    if unknown:
        print("unknown inputs: " + ", ".join(sorted(unknown)))
        return 2
    for name in cases:
        if not names or name in names:
            (write_unified if unified else write)(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

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
    CopperKeepout, CopperLayer, FootprintPad, GlobalRouterOptions, GlobalViaProposal, NetMatchGroup, NetRoutingRule,
    PadReference, PhysicalFootprint, Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, Stackup,
    nm_from_mm, route_global,
)
from pcbir.workbench import save_state  # noqa: E402

import test_bundle_tuning as bundle_tuning  # noqa: E402
import test_critical_bundles as bundles  # noqa: E402
import test_group_tuning as group_tuning  # noqa: E402
import test_length_match_tuning as match  # noqa: E402
import test_local_pair_branches as local_branches  # noqa: E402
import test_pair_hybrid_contact_anchor as hybrid_anchor  # noqa: E402
import test_pair_terminal_fallback as terminal_fallback  # noqa: E402
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


def _rules(board, **fields):
    return replace(board, net_routing_rules=tuple(replace(rule, **fields) for rule in board.net_routing_rules))


def _crossover_support(x):
    # test_crossover_with_support_keeps_every_contact_and_rejects_remote_branch.
    board = local_branches.fixture()
    support = PhysicalFootprint("support", (FootprintPad("1", Point.mm(0, "-.25"), Size.mm(".4", ".3")),
                                            FootprintPad("2", Point.mm(0, ".25"), Size.mm(".4", ".3"))), Size.mm(1, 1))
    board = replace(board, footprints={**board.footprints, "support": support},
                    placements=(*board.placements, Placement("ESD", "support", Point.mm(x, 12))),
                    nets=tuple(replace(n, pads=(*n.pads, PadReference("ESD", str(i))))
                               for i, n in enumerate(board.nets, 1)))
    return _rules(board, breakout_length_nm=4500000)


def _support(mode):
    # test_support_contacts_require_one_source_declared_local_endpoint.
    board = local_branches.support_fixture()
    if mode == "remote":
        return replace(board, placements=tuple(replace(p, position=Point.mm(20, 4)) if p.reference == "SUP_P" else p
                                               for p in board.placements))
    if mode == "ambiguous":
        return _rules(board, breakout_length_nm=40000000)
    return _rules(board, breakout_length_nm=None, breakout_width_nm=None)


def _joint_returns(distance):
    # test_joint_supports_cross_layers_with_real_returns_or_roll_back.
    import test_pair_vias as pair_vias
    board = pair_vias.board_fixture(True)
    support = PhysicalFootprint("support", (FootprintPad("1", Point(0, 0), Size.mm(".4", ".4")),), Size.mm(1, 1))
    board = replace(board, footprints={**board.footprints, "support": support},
                    placements=(*board.placements, Placement("SUP_A", "support", Point.mm("6.4", 10)),
                                Placement("SUP_B", "support", Point.mm("6.4", 14))),
                    nets=tuple(replace(n, pads=(*n.pads, PadReference("SUP_" + n.name, "1")))
                               if n.name in {"A", "B"} else n for n in board.nets))
    return _rules(board, breakout_length_nm=4000000, breakout_width_nm=250000,
                  maximum_return_via_distance_nm=distance)


def _bound_pair():
    # test_partial_pair_macro_ports.bound_pair, with the asset in a temporary directory.
    import tempfile
    import test_partial_pair_macro_ports as macro_ports
    with tempfile.TemporaryDirectory() as directory:
        return macro_ports.bound_pair(Path(directory))


def _single_via_board():
    import test_critical_routing as critical_routing
    return replace(critical_routing._pair_board(),
                   stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                                    CopperLayer.BACK)),
                   net_routing_rules=(NetRoutingRule("USB_DP", RouteKind.CRITICAL),))


def _guide_via(board):
    # A guide via on USB_DP from F.Cu to In1.Cu, as the test adds it.
    route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    return replace(route, routes=tuple(
        replace(r, vias=(GlobalViaProposal(r.net, Point.mm(20, 12), CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                           "test-via"),)) if r.net == "USB_DP" else r
        for r in route.routes))


def _cleared(board):
    # Guides as the tests clear them: no access, no coarse copper.
    route = route_global(board)
    return replace(route, routes=tuple(replace(r, connected=False, accesses=(), segments=(), vias=())
                                       for r in route.routes))


def _unconnected(board):
    # Failed coarse access that keeps its pin accesses.
    route = route_global(board)
    return replace(route, routes=tuple(replace(r, connected=False, segments=(), vias=()) for r in route.routes))


# name: (board builder, route_global options or a guide builder or None, replay arguments after the output paths)
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
    # Pair terminal preparation (pair_terminals): local crossovers, support
    # contacts, joint supports and proved macro ports.
    "terminals-crossover": (local_branches.fixture, None, ()),
    "terminals-crossover-rejected": (lambda: _rules(local_branches.fixture(), max_vias=0), None,
                                     ("--pair-states", "1")),
    "terminals-crossover-support": (lambda: _crossover_support(23), None, ()),
    "terminals-crossover-support-remote": (lambda: _crossover_support(18), None, ()),
    "terminals-support-joint": (local_branches.support_fixture, _unconnected, ("--pair-states", "6000")),
    "terminals-support-joint-tiny": (local_branches.support_fixture, None, ("--pair-states", "1")),
    "terminals-support-remote": (lambda: _support("remote"), _unconnected, ("--pair-states", "6000")),
    "terminals-support-ambiguous": (lambda: _support("ambiguous"), _unconnected, ("--pair-states", "6000")),
    "terminals-support-no-contract": (lambda: _support("no-contract"), _unconnected, ("--pair-states", "6000")),
    "terminals-joint-returns": (lambda: _joint_returns(1500000), None, ("--pair-states", "6000")),
    "terminals-joint-returns-far": (lambda: _joint_returns(100000), None, ("--pair-states", "6000")),
    "terminals-fallback": (lambda: terminal_fallback.fixture(False), _cleared, ()),
    "terminals-fallback-duplicates": (lambda: terminal_fallback.fixture(True), _cleared, ()),
    "terminals-fallback-duplicates-tiny": (lambda: terminal_fallback.fixture(True), _cleared,
                                           ("--pair-states", "1")),
    "terminals-support-branches": (terminal_fallback.support_branch_fixture, _cleared, ("--pair-states", "20000")),
    "terminals-support-branches-tiny": (terminal_fallback.support_branch_fixture, _cleared, ("--pair-states", "1")),
    "terminals-support-branches-remote": (lambda: terminal_fallback.support_branch_fixture(4), _cleared, ()),
    "terminals-hybrid-anchor": (hybrid_anchor.hybrid, _cleared, ()),
    "terminals-hybrid-anchor-renamed": (lambda: hybrid_anchor.hybrid(True), _cleared, ()),
    "terminals-macro-ports": (lambda: _bound_pair(), _cleared, ()),
    "terminals-macro-ports-coarse": (lambda: _bound_pair(), None, ()),
    # Single-ended guide vias (test_critical_transition_without_special_technology_is_through_via).
    "single-guide-via": (_single_via_board, _guide_via, ()),
    "single-guide-via-budget": (lambda: _rules(_single_via_board(), max_vias=0), _guide_via, ()),
}


def write(name: str) -> Path:
    build, options, _ = CASES[name]
    board = build()
    if callable(options):
        route = options(board)
    else:
        route = route_global(board) if options is None else route_global(board, options)
    path = OUT / f"{name}.state.json"
    save_state(path, {"board": board, "global_route": route, "reserved_accesses": None,
                      "plane_stitch_options": None})
    print(f"{path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    return path


# --- Package-access reservations ----------------------------------------------


def _bypass_held():
    # test_bypass_checkpoint_replays_only_after_unique_edge_restore_and_fresh_admission.
    from pcbir.decoupling import route_decouplers
    from pcbir.flow import _bypass_copper_reservations
    import test_decoupling_crosslayer_paths as crosslayer
    source, _ = crosslayer.fixture()
    planned = route_decouplers(source)
    held, reason = _bypass_copper_reservations(source, planned.created_tracks, planned.created_vias,
                                               planned.admitted_path_witnesses)
    assert held is not None, reason
    return source, route_global(source), held


def _crossface(mode=None):
    # test_formal_critical_pair_and_retained_clock_admit_only_complete_crossface_prefix.
    import test_crosslayer_reservation_admission as admission
    from pcbir.physical import CopperZone, PhysicalNet, TrackSegment
    import test_critical_routing as critical_routing
    pair = critical_routing._pair_board()
    feed, link = admission.fixture()
    terminal = PhysicalFootprint("single_clock", (FootprintPad("1", Point(0, 0), Size.mm(".4", ".4")),),
                                 Size.mm(1, 1))
    board = replace(pair, rules=feed.rules, stackup=feed.stackup,
        footprints={**pair.footprints, **feed.footprints, terminal.name: terminal},
        placements=(*pair.placements, *(replace(p, position=Point(p.position.x_nm, p.position.y_nm + 10_000_000))
                                        for p in feed.placements),
                    Placement("CLK_A", terminal.name, Point.mm(5, 5)),
                    Placement("CLK_B", terminal.name, Point.mm(20, 5))),
        nets=(*pair.nets, *feed.nets, PhysicalNet("CLOCK_OWNER", (PadReference("CLK_A", "1"),
                                                                 PadReference("CLK_B", "1")))),
        decoupling_links=feed.decoupling_links,
        zones=(CopperZone("ordinary_return_plane", link.return_net, (CopperLayer.INTERNAL_1,),
                          PolygonWithHoles(PolygonRing(pair.outline.vertices))),),
        net_routing_rules=(*pair.net_routing_rules, *feed.net_routing_rules,
                           NetRoutingRule("CLOCK_OWNER", RouteKind.CLOCK, width_nm=150000,
                                          allowed_layers=(CopperLayer.FRONT,), max_vias=0,
                                          max_length_nm=30_000_000)))
    # The bypass-only reservation route-board's critical stage receives (the
    # test's route_fanout result also holds pin accesses, which replay refuses).
    from pcbir.decoupling import route_decouplers
    from pcbir.flow import _bypass_copper_reservations
    planned = route_decouplers(board)
    ordinary, reason = _bypass_copper_reservations(board, planned.created_tracks, planned.created_vias,
                                                   planned.admitted_path_witnesses)
    assert ordinary is not None, reason
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=2_500_000, maximum_iterations=1))
    if mode == "forged":
        extra = replace(ordinary.decoupling_vias[-1], position=Point.mm(30, 20))
        ordinary = replace(ordinary, board=replace(ordinary.board, vias=(*ordinary.board.vias, extra)),
                           created_vias=(*ordinary.created_vias, extra),
                           decoupling_vias=(*ordinary.decoupling_vias, extra),
                           added_via_count=ordinary.added_via_count + 1)
    elif mode == "short":
        # An ordinary (undeclared) reservation track across both pair lands.
        pads = [p for n in board.nets if n.name in {"USB_DP", "USB_DM"} for p in n.pads if p.component == "J1"]
        from pcbir.placement import transformed_pad_position
        poses = {p.reference: p for p in board.placements}
        a, b = (transformed_pad_position(board, poses[p.component], p.pad) for p in pads[:2])
        extra = TrackSegment(link.net, a, b, 150000, CopperLayer.FRONT)
        ordinary = replace(ordinary, board=replace(ordinary.board, tracks=(*ordinary.board.tracks, extra)),
                           created_tracks=(*ordinary.created_tracks, extra),
                           added_track_count=ordinary.added_track_count + 1)
    return board, guides, ordinary


# name: (builder of (board, global route, reserved accesses), replay arguments after the output paths)
RESERVED = {
    "reserved-bypass": (_bypass_held, ()),
    "reserved-crossface": (_crossface, ("--pair-states", "6000")),
    "reserved-crossface-forged": (lambda: _crossface("forged"), ("--pair-states", "6000")),
    "reserved-short": (lambda: _crossface("short"), ("--pair-states", "6000")),
}


def write_reserved(name: str) -> Path:
    board, route, reserved = RESERVED[name][0]()
    path = OUT / f"{name}.state.json"
    save_state(path, {"board": board, "global_route": route, "reserved_accesses": reserved,
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


def _return_crossing(returns=True):
    # test_single_ended_return_vias.crossing: a single-ended net that needs
    # existing return vias to cross a wall (route_unified only_nets={"S"}).
    import test_single_ended_return_vias as return_vias
    return return_vias.crossing(returns=returns)


# name: (board builder, saved UnifiedRouterOptions fields[, (guide builder, only_nets)])
UNIFIED = {
    # test_tuned_bus_copper_survives_route_smoothing; replay with --set route_smoothing=true.
    "bus-skew": (_bus_skew_board, {"buses_first": True}),
    # test_buses_first_with_skew_tuning_reports_untunable_members_on_the_board: the skew note.
    "bus-skew-untunable": (lambda: _bus_skew_board(None), {"buses_first": True}),
    # test_bus_bundles_route_the_nets_between_the_bus_parts_with_the_bus; replay with --set bus_bundles=true.
    "bus-bundles": (_bus_bundle_board, {"maximum_iterations": 5}),
    # test_negotiation_uses_existing_nearby_returns_without_adding_or_dropping_them.
    "return-vias": (_return_crossing, {"pitch_nm": nm_from_mm("0.25"), "maximum_iterations": 4, "commit_rounds": 2},
                    (lambda board: route_global(replace(board, tracks=(), vias=())), frozenset({"S"}))),
    # test_missing_returns_do_not_silently_allow_signal_transitions.
    "return-vias-missing": (lambda: _return_crossing(False),
                            {"pitch_nm": nm_from_mm("0.25"), "maximum_iterations": 3, "commit_rounds": 1},
                            (lambda board: route_global(replace(board, tracks=(), vias=())), frozenset({"S"}))),
}


def write_unified(name: str) -> Path:
    from pcbir.unified_router import UnifiedRouterOptions, route_unified
    build, fields, *custom = UNIFIED[name]
    board = build()
    guides, only_nets = custom[0] if custom else (
        lambda board: route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))), None)
    route = guides(board)
    path = UNIFIED_OUT / f"{name}.state.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    route_unified(board, route, UnifiedRouterOptions(save_inputs_path=str(path), **fields), only_nets=only_nets)
    print(f"{path.relative_to(ROOT)} ({path.stat().st_size} bytes)")
    return path


def main(names: list[str]) -> int:
    unified = names[:1] == ["--unified"]
    cases = UNIFIED if unified else {**CASES, **RESERVED}
    names = names[1:] if unified else names
    unknown = set(names) - set(cases)
    if unknown:
        print("unknown inputs: " + ", ".join(sorted(unknown)))
        return 2
    for name in cases:
        if not names or name in names:
            (write_unified if unified else write_reserved if name in RESERVED else write)(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

"""Planar ordinary nets in the critical stage and macro port halos for the planners."""
import sys
from dataclasses import replace
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))
from test_critical_routing import _pair_board
from test_hard_macros import fixture as macro_fixture

from pcbir import (CopperLayer, FootprintPad, NetRoutingRule, PadReference, PhysicalFootprint,
                   PhysicalNet, Placement, Point, RouteKind, Size, nm_from_mm)
from pcbir.critical import promote_planar_nets
from pcbir.flow import _macro_port_halos, run_routing_pipeline
from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles
from pcbir.unified_router import UnifiedRouterOptions


def _planar_board():
    """The pair fixture plus a front-only, zero-via net across the pair's straight path."""
    board = _pair_board()
    pad = PhysicalFootprint("test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                            Size.mm(1, 1))
    rule = NetRoutingRule("FB", width_nm=nm_from_mm("0.2"), allowed_layers=(CopperLayer.FRONT,),
                          max_vias=0, max_length_nm=nm_from_mm(12))
    return replace(board, footprints={**board.footprints, pad.name: pad},
                   placements=(*board.placements, Placement("R1", pad.name, Point.mm(20, 7)),
                               Placement("R2", pad.name, Point.mm(20, 17))),
                   nets=(*board.nets, PhysicalNet("FB", (PadReference("R1", "1"), PadReference("R2", "1")))),
                   net_routing_rules=(*board.net_routing_rules, rule))


def _pipeline(board):
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    return run_routing_pipeline(board, unified_options=UnifiedRouterOptions(maximum_iterations=6, commit_rounds=1),
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))


def test_planar_zero_via_nets_are_promoted_to_the_critical_stage_only() -> None:
    board = _planar_board()
    promoted, names = promote_planar_nets(board)
    assert names == ("FB",)
    rule = next(r for r in promoted.net_routing_rules if r.net == "FB")
    # Routed first, under its own width, layer and via rules; a two-pin net
    # keeps its point-to-point topology.
    assert rule.kind is RouteKind.CRITICAL and rule.priority == 101
    assert rule.max_vias == 0 and rule.allowed_layers == (CopperLayer.FRONT,) and rule.topology == "point_to_point"
    assert promoted.net_routing_rules[:2] == board.net_routing_rules[:2]
    # A general net with a via budget or two layers is not planar; neither is a
    # zone net nor a net with a bypass link.
    plain = replace(board, net_routing_rules=(*board.net_routing_rules[:2],
                                              replace(board.net_routing_rules[2], max_vias=1)))
    assert promote_planar_nets(plain) == (plain, ())
    zoned = replace(board, zones=(CopperZone("fb", "FB", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(18, 6), Point.mm(22, 6), Point.mm(22, 18), Point.mm(18, 18))))),))
    assert promote_planar_nets(zoned) == (zoned, ())
    assert promote_planar_nets(_pair_board()) == (_pair_board(), ())


def test_the_pair_no_longer_cuts_the_planar_net_and_the_board_keeps_its_rules(monkeypatch) -> None:
    import pcbir.critical as critical
    board = _planar_board()
    # Without the promotion the pair routes first, straight across R1-R2's
    # corridor; the front-only net is left open or detours far beyond its
    # 12 mm limit (here: around the pair, three times the limit).
    monkeypatch.setattr(critical, "promote_planar_nets", lambda b: (b, ()))
    before = next(n for n in _pipeline(board).detailed.nets if n.net == "FB")
    assert not before.connected or before.warnings
    monkeypatch.undo()
    after = _pipeline(board)
    planar = next(g for g in after.critical.nets if g.nets == ("FB",))
    assert planar.connected and not planar.warnings
    assert next(n for n in after.detailed.nets if n.net == "FB").connected
    assert all(g.connected for g in after.critical.nets)
    assert all(t.layer is CopperLayer.FRONT for t in after.board.tracks if t.net == "FB")
    assert not [v for v in after.board.vias if v.net == "FB"]
    # The promoted rule served the critical stage only.
    assert after.board.net_routing_rules == board.net_routing_rules
    assert after.critical.board.net_routing_rules == board.net_routing_rules


def test_macro_port_halos_lie_beyond_ports_of_nets_that_may_change_layer(tmp_path) -> None:
    board, _, bind = macro_fixture(tmp_path)
    bound = bind()
    with_vias = replace(bound, net_routing_rules=(NetRoutingRule("N", RouteKind.CRITICAL, max_vias=2,
                                                                 width_nm=nm_from_mm("0.2")),))
    (halo,) = _macro_port_halos(with_vias)
    assert halo.id == "port-halo-0" and halo.block_vias and halo.block_tracks and not halo.block_zones
    assert halo.layers == tuple(bound.stackup.copper_layers)
    xs = [p.x_nm for p in halo.outline.outer.vertices]
    ys = [p.y_nm for p in halo.outline.outer.vertices]
    port = Point.mm(10, 10)   # the macro port sits on U1.1
    gap = bound.rules.minimum_clearance_nm
    # One clearance past the port, two via pitches along, one to each side.
    assert max(xs) == port.x_nm - gap and min(ys) < port.y_nm < max(ys)
    half = bound.rules.default_via_size_nm + gap + nm_from_mm("0.2")
    assert max(xs) - min(xs) == 2 * half and max(ys) - min(ys) == 2 * half
    # No halo for a net without a via budget, or without a rule.
    assert _macro_port_halos(replace(bound, net_routing_rules=(
        NetRoutingRule("N", RouteKind.CRITICAL, max_vias=0),))) == ()
    assert _macro_port_halos(bound) == ()

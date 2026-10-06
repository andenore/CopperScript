from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone,
                   FootprintPad, NetRoutingRule, PadKind, PadReference, PadShape,
                   PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
                   PolygonWithHoles, PrototypePhysicalOptions, ReturnViaPolicy,
                   Size, ZoneConnection, compile_source, nm_from_mm,
                   physical_board_digest, prototype_physicalize, route_global)
from pcbir.critical import _route_pair, _validate_candidate
from pcbir.pair_vias import paired_via_candidates
from pcbir.return_paths import shared_reference_plane, pair_reference_intent_covers
from test_pair_vias import board_fixture


def shared_board():
    board = board_fixture(True)
    return replace(board, zones=(CopperZone("return", "GND", (CopperLayer.INTERNAL_1,),
                   PolygonWithHoles(PolygonRing(board.outline.vertices)),
                   pad_connection=ZoneConnection.SOLID),),
                   net_routing_rules=tuple(replace(rule,
                       return_via_policy=ReturnViaPolicy.REFERENCE_CHANGE,
                       shared_reference_layer=CopperLayer.INTERNAL_1)
                       for rule in board.net_routing_rules))


def proposal(board):
    routes = {route.net: route for route in route_global(board).routes}
    candidate = next(paired_via_candidates(board, *board.net_routing_rules, routes["A"], routes["B"]))
    result, tracks, vias = _route_pair(board, *board.net_routing_rules, routes,
        exact_tracks=(candidate.first, candidate.second), exact_via_pairs=candidate.via_pairs,
        exact_return_vias=candidate.return_vias)
    return candidate, result, tracks, vias


def test_same_declared_adjacent_reference_omits_useless_vias_and_reports_intent():
    board = shared_board()
    candidate, result, tracks, vias = proposal(board)
    assert candidate.return_vias == ()
    assert result.connected and result.paired_via_transitions == 2
    assert result.return_via_count == 0 and result.shared_reference_transition_count == 2
    assert len(vias) == 4
    assert any("does not certify" in text for text in result.assumptions)
    accepted, _, _ = _validate_candidate(board, result, tracks, vias, [], [])
    assert accepted.connected


def test_default_always_and_mixed_policies_still_require_reference_vias():
    board = shared_board()
    always = replace(board.net_routing_rules[0], return_via_policy=ReturnViaPolicy.ALWAYS,
                     shared_reference_layer=None)
    for rules in ((always, replace(board.net_routing_rules[1],
                   return_via_policy=ReturnViaPolicy.ALWAYS, shared_reference_layer=None)),
                  (always, board.net_routing_rules[1])):
        _, result, _, vias = proposal(replace(board, net_routing_rules=rules))
        assert result.connected and result.return_via_count == 2
        assert result.shared_reference_transition_count == 0
        assert sum(v.net == "GND" for v in vias) == 2


def test_different_reference_layers_still_emit_bridging_ground_vias():
    board = shared_board()
    board = replace(board, zones=(*board.zones, replace(board.zones[0],
                    id="back-reference", layers=(CopperLayer.INTERNAL_2,))),
                    net_routing_rules=tuple(replace(rule,
                    allowed_layers=(CopperLayer.FRONT, CopperLayer.BACK))
                    for rule in board.net_routing_rules))
    _, result, _, vias = proposal(board)
    assert result.connected and result.return_via_count == 2
    assert result.shared_reference_transition_count == 0
    assert all(via.from_layer is CopperLayer.FRONT and via.to_layer is CopperLayer.BACK
               for via in vias if via.net == "GND")


@pytest.mark.parametrize("fault", ("missing", "ambiguous", "holes", "nonadjacent", "outside"))
def test_unknown_or_uncovered_reference_never_omits_mandatory_vias(fault):
    board = shared_board()
    points = (Point.mm(5, 12), Point.mm(35, 12))
    signal_layers = (CopperLayer.FRONT, CopperLayer.INTERNAL_2)
    if fault == "missing":
        board = replace(board, zones=())
    elif fault == "ambiguous":
        board = replace(board, nets=(*board.nets, PhysicalNet("OTHER", ())),
                        zones=(*board.zones, replace(board.zones[0], id="other", net="OTHER")))
    elif fault == "holes":
        hole = PolygonRing((Point.mm(18, 10), Point.mm(22, 10), Point.mm(22, 14), Point.mm(18, 14)))
        board = replace(board, zones=(replace(board.zones[0],
                        outline=PolygonWithHoles(board.zones[0].outline.outer, (hole,))),))
    elif fault == "nonadjacent":
        signal_layers = (CopperLayer.FRONT, CopperLayer.BACK)
    else:
        points = (Point.mm(-1, 12),)
    assert shared_reference_plane(board, *board.net_routing_rules, signal_layers, points) is None


def test_owner_rejects_plane_void_under_route_even_when_transition_sites_are_clear():
    board = shared_board()
    void = CopperKeepout("reference-void", (CopperLayer.INTERNAL_1,),
        PolygonWithHoles(PolygonRing((Point.mm(19, 0), Point.mm(21, 0),
                                     Point.mm(21, 25), Point.mm(19, 25)))),
        block_tracks=False, block_vias=False, block_pads=False, block_zones=True)
    board = replace(board, copper_keepouts=(*board.copper_keepouts, void))
    candidate, result, tracks, vias = proposal(board)
    assert not candidate.return_vias  # Local portals alone are insufficient proof.
    rejected, tracks, vias = _validate_candidate(board, result, tracks, vias, [], [])
    assert not rejected.connected and rejected.candidate_rejected
    assert tracks == vias == () and rejected.shared_reference_transition_count == 0
    assert any("does not cover" in text for text in rejected.diagnostics)


def test_owner_derives_actual_contact_layers_and_rejects_missing_inner_contacts():
    board = shared_board()
    routes = {route.net: route for route in route_global(board).routes}
    candidate, _, _, _ = proposal(board)
    surface = tuple(tuple(t for t in member if t.layer is CopperLayer.FRONT)
                    for member in (candidate.first, candidate.second))
    result, tracks, vias = _route_pair(board, *board.net_routing_rules, routes,
        exact_tracks=surface, exact_via_pairs=candidate.via_pairs)
    rejected, tracks, vias = _validate_candidate(board, result, tracks, vias, [], [])
    assert not rejected.connected and tracks == vias == ()
    assert any("lacks a permitted nearby return via" in text for text in rejected.diagnostics)


def test_reference_screen_checks_entire_track_envelope_and_actual_layers():
    from pcbir import TrackSegment

    board = shared_board()
    track = TrackSegment("A", Point.mm(2, 2), Point.mm(5, 2), nm_from_mm(".2"), CopperLayer.FRONT)
    assert pair_reference_intent_covers(board, CopperLayer.INTERNAL_1, "GND", (track,))
    assert not pair_reference_intent_covers(board, CopperLayer.INTERNAL_1, "GND",
                                           (replace(track, layer=CopperLayer.BACK),))
    edge = replace(track, start=Point.mm(2, ".05"), end=Point.mm(5, ".05"))
    assert not pair_reference_intent_covers(board, CopperLayer.INTERNAL_1, "GND", (edge,))


@pytest.mark.parametrize("kwargs", ({"return_via_policy": "bad"},
    {"return_via_policy": "reference_change"},
    {"return_via_policy": "reference_change", "shared_reference_layer": "In1.Cu"},
    {"shared_reference_layer": "In1.Cu"}))
def test_ambiguous_or_invalid_policy_is_rejected(kwargs):
    with pytest.raises(ValueError):
        NetRoutingRule("S", **kwargs)


def test_source_lowering_validation_and_source_digest_include_new_fields():
    source = '''board Ref {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        net S { R1.1; } net GND { R1.2; }
        constraint routing(S) {
            require_return_vias = true; return_via_net = "GND";
            maximum_return_via_distance = 2mm;
            return_via_policy = "reference_change"; shared_reference_layer = "In1.Cu";
        }
    }'''
    board = prototype_physicalize(compile_source(source), PrototypePhysicalOptions(copper_layers=4))
    rule = board.net_routing_rules[0]
    assert rule.return_via_policy is ReturnViaPolicy.REFERENCE_CHANGE
    assert rule.shared_reference_layer is CopperLayer.INTERNAL_1
    strict = replace(board, net_routing_rules=(replace(rule,
                     return_via_policy=ReturnViaPolicy.ALWAYS, shared_reference_layer=None),))
    assert physical_board_digest(board) != physical_board_digest(strict)
    with pytest.raises(ValueError, match="unavailable shared reference"):
        replace(board, net_routing_rules=(replace(rule, shared_reference_layer=CopperLayer.INTERNAL_3),))
    with pytest.raises(ValueError, match="routes on its declared reference"):
        replace(board, net_routing_rules=(replace(rule, allowed_layers=(CopperLayer.INTERNAL_1,)),))


def test_native_refill_has_no_dangling_return_via_for_shared_reference_pair():
    from pcbir.plane_verify import verify_filled_planes

    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        pytest.skip("KiCad not installed")
    board = shared_board()
    ground = PhysicalFootprint("ground", (FootprintPad("1", Point(0, 0), Size.mm(1, 1),
               kind=PadKind.THROUGH_HOLE, shape=PadShape.CIRCLE, drill=Size.mm(".5", ".5")),),
               Size.mm(2, 2))
    board = replace(board, footprints={**board.footprints, ground.name: ground},
        placements=(*board.placements, Placement("G1", ground.name, Point.mm(5, 5))),
        nets=tuple(replace(net, pads=(PadReference("G1", "1"),)) if net.name == "GND" else net
                   for net in board.nets))
    _, result, tracks, vias = proposal(board)
    assert result.connected and result.shared_reference_transition_count == 2
    evidence = verify_filled_planes(replace(board, tracks=tracks, vias=vias), kicad_cli=cli)
    assert evidence.passed, evidence.findings
    assert evidence.unconnected_count == evidence.other_violation_count == 0

"""D-PHY plan R5: planned paired layer swaps for crossing pairs of a bundle."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
from math import hypot
import json

import pytest

from pcbir import (
    BoardOutline, CopperKeepout, CopperLayer, CopperZone, CriticalRoutingStatus, FootprintPad, NetMatchGroup,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
    PolygonWithHoles, ReturnViaPolicy, RouteKind, Size, Stackup, ZoneConnection, nm_from_mm, route_critical_nets,
    route_global, run_physical_drc,
)
from pcbir.breakout import BreakoutRegions
from pcbir.critical import _bundle_repair, _critical_jobs, _crossing_site_rank
from pcbir.critical_bundles import BUNDLE_REPAIR_LIMIT, crossing_line, plan_bundles
from pcbir.pair_vias import transition_spacing
from pcbir.physical import ComponentPlacementRule, DesignRules, Via

FRONT, IN1, IN2, IN3, IN4, BACK = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                                   CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)
RETURN_DISTANCE = nm_from_mm("1.5")


def _rows(centres: tuple[str, ...], pitch: str) -> list[tuple[Decimal, Decimal]]:
    return [(Decimal(c) - Decimal(pitch) / 2, Decimal(c) + Decimal(pitch) / 2) for c in centres]


def _keepout(name: str, x0, y0, x1, y1) -> CopperKeepout:
    return CopperKeepout(name, (FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1)))))


def _crossing_board(package: tuple[str, ...], connector: tuple[str, ...], names: tuple[str, ...], *,
                    max_vias: int | None = None) -> PhysicalBoard:
    """A package row and a connector row joined by one 0.2/0.2 mm pair per lane.

    ``package`` and ``connector`` are the pair centres in mm along each row, in
    lane order. The package (U1) faces right at x = 9.4 mm, the connector (J1)
    faces left at x = 20.6 mm. Surface keep-outs behind both rows mean an
    inverted lane order cannot be wound around a component on F.Cu. Four
    layers: In1.Cu is a GND plane, pairs may use F.Cu and In2.Cu and require
    GND return vias within 1.5 mm.
    """
    package_pads, connector_pads, nets, rules = [], [], [], []
    for index, ((up, un), (jp, jn), name) in enumerate(zip(_rows(package, "0.5"), _rows(connector, "0.4"), names)):
        p, n = str(2 * index + 1), str(2 * index + 2)
        package_pads += [FootprintPad(p, Point.mm("1.4", up), Size.mm("0.6", "0.25")),
                         FootprintPad(n, Point.mm("1.4", un), Size.mm("0.6", "0.25"))]
        connector_pads += [FootprintPad(p, Point.mm("-1.4", jp), Size.mm("0.6", "0.2")),
                           FootprintPad(n, Point.mm("-1.4", jn), Size.mm("0.6", "0.2"))]
        nets += [PhysicalNet(f"{name}_P", (PadReference("U1", p), PadReference("J1", p))),
                 PhysicalNet(f"{name}_N", (PadReference("U1", n), PadReference("J1", n)))]
        profile = dict(priority=100, width_nm=nm_from_mm("0.2"), pair_gap_nm=nm_from_mm("0.2"),
                       allowed_layers=(FRONT, IN2), max_vias=max_vias, require_return_vias=True,
                       return_via_net="GND", maximum_return_via_distance_nm=RETURN_DISTANCE)
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **profile),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **profile)]
    package_fp = PhysicalFootprint("test/package-row", tuple(package_pads), Size.mm(3, 8))
    connector_fp = PhysicalFootprint("test/connector-row", tuple(connector_pads), Size.mm(3, 8))
    outline = BoardOutline.rectangle(30, 20)
    return PhysicalBoard(
        "PlannedCrossing", outline, {package_fp.name: package_fp, connector_fp.name: connector_fp},
        (Placement("U1", package_fp.name, Point.mm(8, 10)), Placement("J1", connector_fp.name, Point.mm(22, 10))),
        (*nets, PhysicalNet("GND", ())),
        stackup=Stackup(copper_layers=(FRONT, IN1, IN2, BACK)),
        zones=(CopperZone("ground", "GND", (IN1,), PolygonWithHoles(PolygonRing(outline.vertices)),
                          pad_connection=ZoneConnection.SOLID),),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2")),
        copper_keepouts=(_keepout("behind-package", 0, 0, "9.0", 20),
                         _keepout("behind-connector", "21.0", 0, 30, 20)),
        net_routing_rules=tuple(rules),
    )


def _swapped(**options) -> PhysicalBoard:
    """Two lanes whose order is inverted: A above B at U1, B above A at J1."""
    return _crossing_board(("-1.0", "1.0"), ("1.0", "-1.0"), ("LANE_A", "LANE_B"), **options)


def _reversed(*, six_layers: bool) -> PhysicalBoard:
    """Three lanes A B C at U1 and C B A at J1: every lane crosses every other."""
    board = _crossing_board(("-1.5", "0", "1.5"), ("1.5", "0", "-1.5"), ("LANE_A", "LANE_B", "LANE_C"))
    if not six_layers:
        return board
    # In2.Cu references In1.Cu and In3.Cu references In4.Cu, both GND planes.
    return replace(board, stackup=Stackup(copper_layers=(FRONT, IN1, IN2, IN3, IN4, BACK)),
                   zones=(*board.zones, replace(board.zones[0], id="ground-lower", layers=(IN4,))),
                   net_routing_rules=tuple(replace(rule, allowed_layers=(FRONT, IN2, IN3))
                                           for rule in board.net_routing_rules))


def _shared_reference(board: PhysicalBoard) -> PhysicalBoard:
    return replace(board, net_routing_rules=tuple(
        replace(rule, return_via_policy=ReturnViaPolicy.REFERENCE_CHANGE, shared_reference_layer=IN1)
        for rule in board.net_routing_rules))


def _group(name: str) -> tuple[str, str]:
    return (f"{name}_N", f"{name}_P")


def _bundle(board: PhysicalBoard, **options):
    (bundle,) = plan_bundles(board, _critical_jobs(board), {rule.net: rule for rule in board.net_routing_rules},
                             BUNDLE_REPAIR_LIMIT, **options)
    return bundle


def _hard(board: PhysicalBoard) -> set[str]:
    return {finding.code for finding in run_physical_drc(board).findings
            if finding.severity.value == "error" and finding.code != "DRC-ROUTE-INCOMPLETE"}


def _result(result, name: str):
    return next(item for item in result.nets if item.nets == _group(name))


def _assert_one_paired_swap(result, name: str, layer: CopperLayer) -> None:
    """Exactly one paired swap: two vias per member, matched, surface -> layer -> surface."""
    pair = _result(result, name)
    assert pair.connected and pair.strategy == "planned_crossing"
    assert pair.pair_search_order == "planned_crossing" and pair.paired_via_transitions == 2
    for net in _group(name):
        assert sum(via.net == net for via in result.board.vias) == 2
        assert {track.layer for track in result.board.tracks if track.net == net} == {FRONT, layer}


def test_inverted_lane_order_plans_the_fewest_crossings_on_referenced_layers() -> None:
    swapped = _bundle(_swapped())
    (crossing,) = swapped.crossings
    # Both lanes are outermost; the inner one in bundle order (LANE_A) stays.
    assert crossing.group == _group("LANE_B") and crossing.crosses == (_group("LANE_A"),)
    assert (crossing.surface, crossing.layer, crossing.reference_planes) == (FRONT, IN2, (IN1,))
    assert crossing.status == "planned" and swapped.order == (_group("LANE_B"), _group("LANE_A"))
    # Without planning the bundle keeps its R6 order and plans nothing.
    plain = _bundle(_swapped(), plan_crossings=False)
    assert plain.crossings == () and plain.order == swapped.order

    # Every lane of a full reversal crosses every other: the middle lane
    # stays, the two outer lanes cross each other too, so they need two layers.
    (outer, inner) = _bundle(_reversed(six_layers=True)).crossings
    assert (outer.group, outer.layer, outer.reference_planes) == (_group("LANE_C"), IN2, (IN1,))
    assert (inner.group, inner.layer, inner.reference_planes) == (_group("LANE_A"), IN3, (IN4,))
    assert outer.crosses == (_group("LANE_A"), _group("LANE_B"))
    (planned, impossible) = _bundle(_reversed(six_layers=False)).crossings
    assert planned.layer is IN2 and impossible.layer is None and impossible.status == "impossible"
    assert impossible.reason == "it crosses moving pairs on In2.Cu and no other allowed layer is left"

    # Aligned rows, and rows that only change pitch, need no crossing.
    aligned = _crossing_board(("-1.0", "1.0"), ("-1.0", "1.0"), ("LANE_A", "LANE_B"))
    assert _bundle(aligned).crossings == ()
    spread = _crossing_board(("-1.0", "1.0"), ("-2.0", "3.0"), ("LANE_A", "LANE_B"))
    assert _bundle(spread).crossings == ()


def test_the_pair_that_can_swap_layers_is_the_one_moved() -> None:
    board = _swapped()
    # LANE_B (otherwise moved) may not use vias: LANE_A moves instead.
    board = replace(board, net_routing_rules=tuple(
        replace(rule, max_vias=0) if rule.net.startswith("LANE_B") else rule for rule in board.net_routing_rules))
    (crossing,) = _bundle(board).crossings
    assert crossing.group == _group("LANE_A") and crossing.layer is IN2


def test_crossing_pair_routes_with_one_planned_paired_swap_and_return_vias() -> None:
    board = _swapped()
    guides = route_global(board)
    # Without R5 the crossing pair is left to the surface-first joint search,
    # behind the surface pair; within the same bounded budget it finds no
    # candidate at all.
    without = route_critical_nets(board, guides, pair_state_limit=2000, plan_crossings=False)
    (bundle,) = without.bundles
    assert bundle.crossings == () and "crossings" not in json.loads(without.to_json())["bundles"][0]
    failed = _result(without, "LANE_A")
    assert not failed.connected and failed.pair_search_order == "surface_first"
    assert failed.diagnostics[-1].startswith("joint pair search:") and "0 candidates" in failed.diagnostics[-1]

    result = route_critical_nets(board, guides, pair_state_limit=2000)
    assert result.status is not CriticalRoutingStatus.FAILED
    assert [item.nets for item in result.nets] == [_group("LANE_B"), _group("LANE_A")]
    _assert_one_paired_swap(result, "LANE_B", IN2)
    surface = _result(result, "LANE_A")
    assert surface.connected and surface.via_count == 0
    assert {track.layer for track in result.board.tracks if track.net.startswith("LANE_A")} == {FRONT}
    assert not _hard(result.board)

    (crossing,) = result.bundles[0].crossings
    assert crossing.status == "routed" and crossing.reason == ""
    assert [item.component for item in crossing.transitions] == ["J1", "U1"]
    rules = {rule.net: rule for rule in board.net_routing_rules}
    for transition in crossing.transitions:
        assert [net for net, _ in transition.vias] == list(_group("LANE_B"))
        a, b = (point for _, point in transition.vias)
        assert abs(hypot(a.x_nm - b.x_nm, a.y_nm - b.y_nm)
                   - transition_spacing(board, rules["LANE_B_N"], rules["LANE_B_P"])) <= 4
        assert transition.reference == "return_vias" and transition.shared_reference_layer is None
        (returned,) = transition.return_vias
        assert max(hypot(returned.x_nm - p.x_nm, returned.y_nm - p.y_nm) for p in (a, b)) <= RETURN_DISTANCE
        assert any(via.net == "GND" and via.position == returned for via in result.board.vias)
    document = json.loads(result.to_json())["bundles"][0]["crossings"]
    assert document[0]["group"] == list(_group("LANE_B")) and document[0]["layer"] == "In2.Cu"
    assert document[0]["reference_planes"] == ["In1.Cu"] and document[0]["status"] == "routed"
    assert document[0]["transitions"][1]["vias"] == {
        net: [point.x_nm, point.y_nm] for net, point in crossing.transitions[1].vias}


def test_full_reversal_gives_each_crossing_pair_its_own_layer() -> None:
    board = _reversed(six_layers=True)
    result = route_critical_nets(board, route_global(board))
    assert result.status is not CriticalRoutingStatus.FAILED
    assert [item.nets for item in result.nets] == [_group("LANE_C"), _group("LANE_A"), _group("LANE_B")]
    _assert_one_paired_swap(result, "LANE_C", IN2)
    _assert_one_paired_swap(result, "LANE_A", IN3)
    assert _result(result, "LANE_B").via_count == 0
    assert [item.status for item in result.bundles[0].crossings] == ["routed", "routed"]
    assert not _hard(result.board)

    # With one crossing layer the second crossing pair is impossible: it is
    # reported and fails without any search, and the rest still route.
    four = _reversed(six_layers=False)
    limited = route_critical_nets(four, route_global(four))
    impossible = _result(limited, "LANE_A")
    assert not impossible.connected and impossible.search_states == impossible.pair_searches == 0
    assert impossible.diagnostics == ("planned crossing is impossible: it crosses moving pairs on In2.Cu "
                                      "and no other allowed layer is left",)
    assert _result(limited, "LANE_C").connected and _result(limited, "LANE_B").connected
    assert not any(track.net.startswith("LANE_A") for track in limited.board.tracks)
    assert [item.status for item in limited.bundles[0].crossings] == ["routed", "impossible"]
    assert limited.status is CriticalRoutingStatus.FAILED


@pytest.mark.parametrize("max_vias", (0, 1))
def test_a_via_budget_below_two_reports_the_crossing_as_impossible(max_vias) -> None:
    board = _swapped(max_vias=max_vias)
    result = route_critical_nets(board, route_global(board))
    (crossing,) = result.bundles[0].crossings
    reason = f"max_vias {max_vias} on LANE_B_N allows no paired layer swap; it needs 2 vias per member"
    assert (crossing.status, crossing.layer, crossing.reason) == ("impossible", None, reason)
    pair = _result(result, "LANE_B")
    # No silent fallback: no coarse candidate, no surface search, no copper.
    assert not pair.connected and pair.diagnostics == (f"planned crossing is impossible: {reason}",)
    assert pair.search_states == pair.pair_searches == pair.track_count == 0
    assert not any(item.net.startswith("LANE_B") for item in (*result.board.tracks, *result.board.vias))
    assert _result(result, "LANE_A").connected
    assert result.status is CriticalRoutingStatus.FAILED
    assert json.loads(result.to_json())["bundles"][0]["crossings"] == [{
        "group": list(_group("LANE_B")), "crosses": [list(_group("LANE_A"))], "surface": "F.Cu",
        "layer": None, "reference_planes": [], "status": "impossible", "reason": reason, "transitions": []}]
    assert crossing_line(result.bundles[0], crossing) == (
        f"bundle J1-U1 crossing LANE_B_N/LANE_B_P under LANE_A_N/LANE_A_P: impossible ({reason})")


def test_a_declared_shared_reference_replaces_the_return_vias() -> None:
    board = _shared_reference(_swapped())
    result = route_critical_nets(board, route_global(board))
    _assert_one_paired_swap(result, "LANE_B", IN2)
    assert not any(via.net == "GND" for via in result.board.vias)
    assert _result(result, "LANE_B").shared_reference_transition_count == 2
    (crossing,) = result.bundles[0].crossings
    assert [(item.reference, item.shared_reference_layer, item.return_vias) for item in crossing.transitions] == [
        ("shared_reference", IN1, ())] * 2
    assert not _hard(result.board)


def test_crossing_transitions_stay_outside_breakout_regions() -> None:
    board = _swapped()
    board = replace(board, net_routing_rules=tuple(
        replace(rule, breakout_length_nm=nm_from_mm("2"), breakout_clearance_nm=nm_from_mm("0.1"))
        for rule in board.net_routing_rules))
    rules = {rule.net: rule for rule in board.net_routing_rules}
    # Members of a breakout pair are spaced by the pair gap (plan R1), so are
    # their transition vias: 0.8 mm vias 0.2 mm apart.
    assert transition_spacing(board, rules["LANE_B_N"], rules["LANE_B_P"]) == nm_from_mm("1.0") + 4
    result = route_critical_nets(board, route_global(board))
    _assert_one_paired_swap(result, "LANE_B", IN2)
    regions = BreakoutRegions(board)
    crossing_vias = [via for via in result.board.vias if via.net != "LANE_A"]
    assert len(crossing_vias) == 6 and not any(regions.inside_any(via.position) for via in crossing_vias)
    assert not _hard(result.board)


def test_transition_sites_skip_breakout_regions_and_prefer_the_reserved_corridor() -> None:
    board = _swapped()
    rule = ComponentPlacementRule
    board = replace(board, placement_rules=(
        rule("U1", fixed_position=Point.mm(8, 10), fixed_rotation_degrees=0),
        rule("J1", fixed_position=Point.mm(22, 10), fixed_rotation_degrees=0)),
        net_routing_rules=tuple(replace(item, reserve_corridor=True, breakout_length_nm=nm_from_mm("1"),
                                        breakout_clearance_nm=nm_from_mm("0.1"))
                                for item in board.net_routing_rules))
    rank = _crossing_site_rank(board, _group("LANE_B"))

    def vias(*centres) -> tuple[Via, ...]:
        return tuple(Via("LANE_B_N", Point.mm(x, y), nm_from_mm("0.8"), nm_from_mm("0.4")) for x, y in centres)

    # LANE_B runs from (9.4, 11.0) at U1 to (20.6, 9.0) at J1.
    assert rank(vias((15, 10), (15, "10.2"))) == 0  # inside its corridor
    assert rank(vias((15, 10), (15, 13))) == 1  # one via outside it
    assert rank(vias(("10.2", 11), (15, 10))) is None  # in a land's breakout region
    assert rank(vias(("10.2", 9), (15, 10))) is None  # also another net's region
    plain = _crossing_site_rank(_swapped(), _group("LANE_B"))
    assert plain(vias((15, 13), ("10.2", 11))) == 0


def test_bundle_repair_reroutes_a_crossing_pair_with_its_plan() -> None:
    board = _swapped()
    routes = {route.net: route for route in route_global(board).routes}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    routed = route_critical_nets(board, route_global(board))
    (crossing,) = routed.bundles[0].crossings
    surface = tuple(track for track in routed.board.tracks if track.net.startswith("LANE_A"))
    # Rip up the surface pair, route the crossing pair first, then the surface pair.
    record, repaired = _bundle_repair(
        board, routes, rules, None, (), (), [(_group("LANE_A"), surface, ())],
        (rules["LANE_B_N"], rules["LANE_B_P"]), _group("LANE_B"), _group("LANE_A"),
        {_group("LANE_B"): replace(crossing, status="planned", transitions=())})
    assert record.accepted and repaired is not None
    (crossed, _, crossed_vias), (rerouted, _, _) = repaired
    assert crossed.strategy == "planned_crossing" and crossed.paired_via_transitions == 2
    assert sum(via.net in _group("LANE_B") for via in crossed_vias) == 4
    assert rerouted.connected and rerouted.via_count == 0


def test_bundles_without_crossings_route_byte_identically() -> None:
    from test_critical_bundles import _blocked_middle_board

    for board in (_blocked_middle_board(), _crossing_board(("-1.0", "1.0"), ("-1.0", "1.0"), ("LANE_A", "LANE_B"))):
        guides = route_global(board)
        planned = route_critical_nets(board, guides)
        plain = route_critical_nets(board, guides, plan_crossings=False)
        assert planned == plain
        assert planned.to_json() == plain.to_json()
        assert all("crossings" not in bundle for bundle in json.loads(planned.to_json())["bundles"])


def test_planned_crossings_are_deterministic() -> None:
    board = _reversed(six_layers=True)
    guides = route_global(board)
    first = route_critical_nets(board, guides)
    second = route_critical_nets(board, guides)
    assert first == second
    assert first.to_json() == second.to_json()


def test_a_length_match_group_with_a_crossing_pair_is_tuned() -> None:
    board = _shared_reference(_swapped())
    nets = tuple(net.name for net in board.nets if net.name != "GND")
    board = replace(board, match_groups=(NetMatchGroup("lanes", nets, nm_from_mm("0.1")),),
                    net_routing_rules=tuple(replace(rule, tuning_amplitude_limit_nm=nm_from_mm("0.8"))
                                            for rule in board.net_routing_rules))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned" and tuning.skew_before_nm > nm_from_mm("0.1")
    assert tuning.skew_after_nm <= nm_from_mm("0.1")
    _assert_one_paired_swap(result, "LANE_B", IN2)
    assert result.bundles[0].crossings[0].status == "routed"
    assert result.status is not CriticalRoutingStatus.FAILED and not _hard(result.board)


def test_preflight_prints_each_planned_crossing(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path
    import pcbir.critical_preflight as preflight

    board = _swapped()
    routed = route_critical_nets(board, route_global(board))
    monkeypatch.setattr(preflight, "route_critical_nets", lambda *_, **__: routed)
    report = tmp_path / "report.json"
    preflight.main([str(Path(__file__).resolve().parents[1] / "examples/valid_board/board.copper"),
                    "--allow-proxy-footprints", "--layers", "2", "--fab-profile", "generic",
                    "--router-iterations", "1", "--report", str(report)])
    bundles = json.loads(report.read_text())["critical"]["bundles"]
    assert bundles[0]["crossings"][0]["status"] == "routed"
    (crossing,) = routed.bundles[0].crossings

    def mm(points) -> str:
        return "/".join(f"({point.x_nm / 1e6:.3f}, {point.y_nm / 1e6:.3f})" for point in points) + " mm"

    sites = [(mm([point for _, point in item.vias]), mm(item.return_vias)) for item in crossing.transitions]
    assert (f"bundle J1-U1 crossing LANE_B_N/LANE_B_P under LANE_A_N/LANE_A_P: routed, "
            f"F.Cu -> In2.Cu -> F.Cu (reference planes In1.Cu); J1 vias {sites[0][0]}, return vias {sites[0][1]}; "
            f"U1 vias {sites[1][0]}, return vias {sites[1][1]}") in capsys.readouterr().out

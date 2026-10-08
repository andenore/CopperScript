"""D-PHY plan R7: nested exits for bundles that leave a package across a corner."""
from __future__ import annotations

import json

from pcbir import (
    BoardOutline, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, RouteKind, Size, nm_from_mm, route_critical_nets, route_global,
    run_physical_drc,
)
from pcbir.critical import _critical_jobs
from pcbir.critical_bundles import BUNDLE_REPAIR_LIMIT, bundle_document, plan_bundles
from pcbir.critical_review import critical_lane_review
from pcbir.physical import DesignRules

# A 5 mm QFN (U1) at (15, 10) mm and a connector row (J1) 10 mm below it at
# 0.4 mm pitch, like a four-lane camera link. OUTER and INNER leave the
# package's west edge (INNER at the south-west corner) and turn south; FACE_A
# and FACE_B leave the south edge. Each entry: package (x, y) of the P and N
# lands in package mm, and the connector x of P and N in connector mm.
WRAP = (
    ("OUTER", (("-2.4375", "-0.75"), ("-2.4375", "-0.25")), ("-5.1", "-4.7")),
    ("INNER", (("-2.4375", "1.25"), ("-2.4375", "1.75")), ("-3.9", "-3.5")),
    ("FACE_A", (("-1.75", "2.4375"), ("-1.25", "2.4375")), ("-2.7", "-2.3")),
    ("FACE_B", (("-0.75", "2.4375"), ("-0.25", "2.4375")), ("-1.5", "-1.1")),
)
# The outer pair's connector lands sit 0.8 mm outside the inner pair's: its
# column is nearer than the inner pair's band plus a 0.3 mm clearance.
TIGHT = (WRAP[0][:2] + (("-4.7", "-4.3"),), *WRAP[1:])
# Every pair on the west edge: no pair leaves the facing edge, so no corner wrap.
SIDE_ONLY = WRAP[:2]
FACING_ONLY = (*WRAP[2:], ("FACE_C", (("0.25", "2.4375"), ("0.75", "2.4375")), ("-0.3", "0.1")))


def _corner_board(pairs=WRAP, *, clearance: str = "0.2", breakout: bool = False) -> PhysicalBoard:
    """Pairs of 0.14 mm width and 0.26 mm gap (0.4 mm pitch), 0.127 mm max skew."""
    package_pads, connector_pads, nets, rules = [], [], [], []
    for index, (name, lands, columns) in enumerate(pairs):
        for offset, (x, y), suffix, column in zip((1, 2), lands, "PN", columns):
            number = str(2 * index + offset)
            size = Size.mm("0.875", "0.25") if x == "-2.4375" else Size.mm("0.25", "0.875")
            package_pads.append(FootprintPad(number, Point.mm(x, y), size))
            connector_pads.append(FootprintPad(number, Point.mm(column, "-0.5"), Size.mm("0.2", "0.8")))
            nets.append(PhysicalNet(f"{name}_{suffix}", (PadReference("U1", number), PadReference("J1", number))))
        profile = dict(priority=100, width_nm=nm_from_mm("0.14"), pair_gap_nm=nm_from_mm("0.26"),
                       clearance_nm=nm_from_mm(clearance), max_skew_nm=nm_from_mm("0.127"),
                       tuning_amplitude_limit_nm=nm_from_mm("0.3"))
        if breakout:
            profile.update(breakout_length_nm=nm_from_mm(2), breakout_clearance_nm=nm_from_mm("0.1"),
                           breakout_gap_nm=nm_from_mm("0.2"))
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **profile),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **profile)]
    package = PhysicalFootprint("test/qfn-corner", tuple(package_pads), Size.mm(5, 5))
    connector = PhysicalFootprint("test/connector-row", tuple(connector_pads), Size.mm(8, 1))
    return PhysicalBoard(
        "CornerBundle", BoardOutline.rectangle(30, 30), {package.name: package, connector.name: connector},
        (Placement("U1", package.name, Point.mm(15, 10)), Placement("J1", connector.name, Point.mm(15, "20.5"))),
        tuple(nets),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.14")),
        net_routing_rules=tuple(rules),
    )


def _group(name: str) -> tuple[str, str]:
    return (f"{name}_N", f"{name}_P")


def _plan(board: PhysicalBoard, **options):
    rules = {rule.net: rule for rule in board.net_routing_rules}
    (bundle,) = plan_bundles(board, _critical_jobs(board), rules, BUNDLE_REPAIR_LIMIT, **options)
    return bundle


def _result(result, name: str):
    return next(item for item in result.nets if item.nets == _group(name))


def _westmost(result, name: str) -> int:
    return min(min(t.start.x_nm, t.end.x_nm) for t in result.locked_tracks if t.net in _group(name))


def _hard(result) -> set[str]:
    return {finding.code for finding in run_physical_drc(result.board).findings
            if finding.severity.value == "error"} - {"DRC-ROUTE-INCOMPLETE"}


def test_a_corner_wrap_plans_nested_exits_innermost_first() -> None:
    board = _corner_board()
    bundle = _plan(board)
    inner, outer = bundle.nested_exits
    # Distances run west from each pair's land midpoint (x = 12.5625 mm) to
    # its connector column: INNER at 11.3 mm, OUTER at 10.1 mm. OUTER's column
    # clears INNER's band (0.27 + 0.2 + 0.27 mm), so both turn on their column.
    assert (inner.group, inner.component, inner.direction, inner.travel) == (_group("INNER"), "U1", (-1, 0), (0, 1))
    assert (inner.column_nm, inner.turn_nm, inner.inner) == (1_262_500, 1_262_500, None)
    assert (outer.group, outer.column_nm, outer.turn_nm, outer.inner) == (
        _group("OUTER"), 2_462_500, 2_462_500, _group("INNER"))
    # Outermost first (plan R6) put OUTER first and INNER third; the nest
    # keeps those two slots, innermost first. The facing pairs keep theirs.
    unnested = _plan(board, plan_nested_exits=False)
    assert unnested.nested_exits == ()
    assert unnested.order == tuple(map(_group, ("OUTER", "FACE_B", "INNER", "FACE_A")))
    assert bundle.order == tuple(map(_group, ("INNER", "FACE_B", "OUTER", "FACE_A")))
    assert "nested_exits" not in bundle_document(unnested)
    assert bundle_document(bundle)["nested_exits"][1] == {
        "group": list(_group("OUTER")), "component": "U1", "direction": [-1, 0], "travel": [0, 1],
        "column_nm": 2_462_500, "turn_nm": 2_462_500, "inner": list(_group("INNER")),
        "status": "planned", "routed_turn_nm": None, "routed_chamfer_nm": None}

    # With a 0.3 mm clearance OUTER's column (2.0625 mm) is nearer than
    # INNER's turn plus both half-bands and the clearance: it turns there.
    tight = _plan(_corner_board(TIGHT, clearance="0.3"))
    assert [(item.column_nm, item.turn_nm) for item in tight.nested_exits] == [
        (1_262_500, 1_262_500), (2_062_500, 1_262_500 + 270_000 + 300_000 + 270_000)]


def test_nested_exits_turn_on_the_column_instead_of_running_past_it() -> None:
    board = _corner_board()
    guides = route_global(board)
    before = route_critical_nets(board, guides, nested_exits=False)
    after = route_critical_nets(board, guides)
    assert all(item.connected for item in (*before.nets, *after.nets))
    # Today both side pairs run about 1 mm west past their connector column
    # (P lanes at 9.9 and 11.1 mm) before turning, then come back.
    assert _westmost(before, "OUTER") <= nm_from_mm("8.9")
    assert _westmost(before, "INNER") <= nm_from_mm("10.1")
    # Nested, each turns onto its column with one 45-degree diagonal and
    # never runs past it.
    assert _westmost(after, "OUTER") == nm_from_mm("9.9")
    assert _westmost(after, "INNER") == nm_from_mm("11.1")
    for name in ("OUTER", "INNER"):
        assert _result(after, name).strategy == "nested_exit"
        assert all(a < b for a, b in zip(_result(after, name).lengths_nm, _result(before, name).lengths_nm))
    # The corner wraps shrink by about 2.2 and 1.6 mm; the facing pairs do not change.
    assert max(_result(before, "OUTER").lengths_nm) - max(_result(after, "OUTER").lengths_nm) > nm_from_mm("2.1")
    assert max(_result(before, "INNER").lengths_nm) - max(_result(after, "INNER").lengths_nm) > nm_from_mm("1.5")
    for name in ("FACE_A", "FACE_B"):
        assert _result(after, name).lengths_nm == _result(before, name).lengths_nm
    # INNER's diagonal starts right after its port (0.5 mm port, 0.2 mm
    # offset); OUTER's is as long as its own exit allows.
    (bundle,) = after.bundles
    assert [(item.status, item.routed_turn_nm, item.routed_chamfer_nm) for item in bundle.nested_exits] == [
        ("routed", 1_262_500, 562_500), ("routed", 2_462_500, 1_762_500)]
    report = json.loads(after.to_json())["bundles"][0]["nested_exits"]
    assert [item["status"] for item in report] == ["routed", "routed"]
    assert not _hard(after)
    review = critical_lane_review(after.board, after.nets, after.match_tuning)
    assert all(item["bends"]["sharp_count"] == 0 for item in review["nets"])
    assert all(item["status"] == "pass" for item in review["pairs"])


def test_an_outer_pair_turns_as_soon_as_it_clears_the_inner_pair() -> None:
    # Breakout regions relax the 0.3 mm clearance near the lands, so the
    # connector's 0.4 mm-gap lanes are legal there. Searches use a bounded
    # state budget (4000) to keep the test fast.
    board = _corner_board(TIGHT, clearance="0.3", breakout=True)
    guides = route_global(board)
    before = route_critical_nets(board, guides, nested_exits=False, pair_state_limit=4000)
    after = route_critical_nets(board, guides, pair_state_limit=4000)
    # Today OUTER runs 2 mm past its column and leaves INNER no legal route.
    assert _westmost(before, "OUTER") <= nm_from_mm("8.3")
    assert not _result(before, "INNER").connected
    assert all(item.connected for item in after.nets)
    inner, outer = after.bundles[0].nested_exits
    assert (inner.status, inner.routed_turn_nm) == ("routed", 1_262_500)
    # OUTER turns 0.2 mm beyond its plan, as soon as its run and skew bumps
    # clear INNER's copper, then jogs back onto its column just before the
    # connector port.
    assert (outer.status, outer.turn_nm, outer.routed_turn_nm) == ("routed", 2_102_500, 2_302_500)
    assert _westmost(after, "OUTER") == nm_from_mm("12.5625") - outer.routed_turn_nm - nm_from_mm("0.2")
    assert not _hard(after)


def test_bundles_without_a_corner_wrap_route_exactly_as_before() -> None:
    for pairs in (SIDE_ONLY, FACING_ONLY):
        board = _corner_board(pairs)
        assert _plan(board).nested_exits == ()
        guides = route_global(board)
        before = route_critical_nets(board, guides, nested_exits=False)
        after = route_critical_nets(board, guides)
        assert after.locked_tracks == before.locked_tracks
        assert after.to_json() == before.to_json()


def test_nested_exits_are_deterministic() -> None:
    board = _corner_board(TIGHT, clearance="0.3", breakout=True)
    guides = route_global(board)
    first, second = (route_critical_nets(board, guides, pair_state_limit=4000),
                     route_critical_nets(board, guides, pair_state_limit=4000))
    assert first.locked_tracks == second.locked_tracks
    assert first.to_json() == second.to_json()

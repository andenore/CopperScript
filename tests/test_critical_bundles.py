"""D-PHY plan R6: bundle-aware critical ordering and bounded rip-up repair."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
import json

import pytest

from pcbir import (
    BoardOutline, CopperKeepout, CopperLayer, CriticalRoutingStatus, FootprintPad, NetRoutingRule,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
    PolygonWithHoles, RouteKind, Size, nm_from_mm, route_critical_nets, route_global, run_physical_drc,
)
from pcbir.critical import _critical_jobs
from pcbir.critical_bundles import BUNDLE_REPAIR_LIMIT, bundle_job_order, plan_bundles
from pcbir.physical import DesignRules

# Pair names put the middle pair first by name: LANE_B (top), LANE_A (middle),
# LANE_C (bottom) along both terminal rows.
LANES = ("LANE_B", "LANE_A", "LANE_C")


def _rows(centres: tuple[str, ...], pitch: str) -> list[tuple[Decimal, Decimal]]:
    return [(Decimal(c) - Decimal(pitch) / 2, Decimal(c) + Decimal(pitch) / 2) for c in centres]


def _keepout(name: str, x0: str, y0: str, x1: str, y1: str) -> CopperKeepout:
    return CopperKeepout(name, (CopperLayer.FRONT, CopperLayer.BACK), PolygonWithHoles(PolygonRing((
        Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1)))))


def _bundle_board(package_rows, connector_rows, keepouts=(), *, names=LANES,
                  priorities: dict[str, int] | None = None) -> PhysicalBoard:
    """A package row and a connector row joined by one differential pair per lane.

    Rows are ``(P, N)`` land offsets in mm from each component centre. The
    package (U1) faces right at x = 9.4 mm and the connector (J1) faces left at
    x = 20.6 mm. Every pair is 0.2 mm wide with a 0.2 mm gap.
    """
    package_pads, connector_pads, nets, rules = [], [], [], []
    for index, ((up, un), (jp, jn), name) in enumerate(zip(package_rows, connector_rows, names)):
        p, n = str(2 * index + 1), str(2 * index + 2)
        package_pads += [FootprintPad(p, Point.mm("1.4", up), Size.mm("0.6", "0.25")),
                         FootprintPad(n, Point.mm("1.4", un), Size.mm("0.6", "0.25"))]
        connector_pads += [FootprintPad(p, Point.mm("-1.4", jp), Size.mm("0.6", "0.2")),
                           FootprintPad(n, Point.mm("-1.4", jn), Size.mm("0.6", "0.2"))]
        nets += [PhysicalNet(f"{name}_P", (PadReference("U1", p), PadReference("J1", p))),
                 PhysicalNet(f"{name}_N", (PadReference("U1", n), PadReference("J1", n)))]
        profile = dict(priority=(priorities or {}).get(name, 100), width_nm=nm_from_mm("0.2"),
                       pair_gap_nm=nm_from_mm("0.2"))
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **profile),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **profile)]
    package = PhysicalFootprint("test/package-row", tuple(package_pads), Size.mm(3, 8))
    connector = PhysicalFootprint("test/connector-row", tuple(connector_pads), Size.mm(3, 8))
    return PhysicalBoard(
        "PairBundle", BoardOutline.rectangle(30, 20), {package.name: package, connector.name: connector},
        (Placement("U1", package.name, Point.mm(8, 10)), Placement("J1", connector.name, Point.mm(22, 10))),
        tuple(nets),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2")),
        copper_keepouts=tuple(keepouts), net_routing_rules=tuple(rules),
    )


def _group(name: str) -> tuple[str, str]:
    return (f"{name}_N", f"{name}_P")


def _blocked_middle_board(**options) -> PhysicalBoard:
    """Three aligned pairs; a keep-out on the middle lane, a wall above the top lane.

    The middle pair must detour. Above is shorter, but only fits while the top
    pair is unrouted; below fits between the obstacle and the bottom pair.
    """
    centres = ("-1.0", "0", "2.0")
    return _bundle_board(_rows(centres, "0.5"), _rows(centres, "0.4"), (
        _keepout("wall-above-top-lane", "10", "0", "20", "8.55"),
        _keepout("middle-lane-obstacle", "14", "9.7", "16", "10.8"),
    ), **options)


def _repair_board(connector_centres, walls, obstacle) -> PhysicalBoard:
    """Three pairs between an upper and a lower wall, around a small obstacle."""
    upper, lower = walls
    return _bundle_board(_rows(("-1.0", "0", "1.0"), "0.5"), _rows(connector_centres, "0.4"), (
        _keepout("upper-wall", "10", "0", "20", upper),
        _keepout("lower-wall", "10", lower, "20", "20"),
        _keepout("obstacle", *obstacle),
    ))


# The connector row is spread wider and sits higher than the package row; an
# obstacle sits just above the middle lane. Outermost first, the middle pair
# (routed last) finds no path past the bottom pair's copper, and its coarse
# candidate shorts that copper. Routing the middle pair first and the bottom
# pair after it succeeds. Searches use a bounded state budget (4000) to keep
# the test fast.
REPAIRABLE = (("-1.5", "-0.3", "0.9"), ("7.0", "13.0"), ("14", "8.8", "15.5", "10.0"))
# The connector row sits lower and the obstacle covers the middle lane. The
# same rip-up routes the middle pair but then leaves no path for the bottom pair.
UNREPAIRABLE = (("-0.8", "0.4", "1.6"), ("6.9", "13.1"), ("15", "9.7", "16.5", "11.7"))


def test_bundles_are_planned_per_component_pair_kind_and_priority() -> None:
    centres = ("-2.0", "-1.0", "0", "1.0", "2.0")
    names = ("LANE_D", "LANE_A", "LANE_E", "LANE_B", "LANE_C")
    board = _bundle_board(_rows(centres, "0.5"), _rows(centres, "0.4"), names=names)
    single = NetRoutingRule("CLOCK_REF", RouteKind.CLOCK, priority=100)
    board = replace(board, nets=(*board.nets, PhysicalNet("CLOCK_REF", ())),
                    net_routing_rules=(*board.net_routing_rules, single))
    jobs = _critical_jobs(board)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    (bundle,) = plan_bundles(board, jobs, rules, BUNDLE_REPAIR_LIMIT)
    assert bundle.components == ("J1", "U1")
    assert bundle.name_order == tuple(_group(name) for name in sorted(names))
    # Outermost first along the terminal row, the lower row position first on
    # a tie: rows top to bottom are LANE_D, LANE_A, LANE_E, LANE_B, LANE_C.
    assert bundle.order == tuple(_group(name) for name in ("LANE_D", "LANE_C", "LANE_A", "LANE_B", "LANE_E"))
    ordered = bundle_job_order(jobs, (bundle,))
    # The bundle keeps its slots; the single-ended clock keeps its place.
    assert [group for _, group in ordered if group != ("CLOCK_REF",)] == list(bundle.order)
    assert [group for _, group in ordered].index(("CLOCK_REF",)) == [
        group for _, group in jobs].index(("CLOCK_REF",))

    # A different priority is a declared order, not part of the bundle.
    split = _bundle_board(_rows(centres, "0.5"), _rows(centres, "0.4"), names=names,
                          priorities={"LANE_E": 200})
    (rest,) = plan_bundles(split, _critical_jobs(split),
                           {rule.net: rule for rule in split.net_routing_rules}, BUNDLE_REPAIR_LIMIT)
    assert _group("LANE_E") not in rest.order and len(rest.order) == 4
    # A lone pair between two components is not a bundle.
    lone = _bundle_board(_rows(("0",), "0.5"), _rows(("0",), "0.4"), names=("LANE_A",))
    assert plan_bundles(lone, _critical_jobs(lone),
                        {rule.net: rule for rule in lone.net_routing_rules}, BUNDLE_REPAIR_LIMIT) == ()


def test_outermost_first_routes_a_bundle_that_fails_middle_first() -> None:
    board = _blocked_middle_board()
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    (bundle,) = result.bundles
    assert bundle.order == (_group("LANE_B"), _group("LANE_C"), _group("LANE_A"))
    assert [item.nets for item in result.nets] == list(bundle.order)
    assert all(item.connected for item in result.nets), [item.diagnostics for item in result.nets]
    assert result.status is not CriticalRoutingStatus.FAILED
    # The middle pair went below the obstacle, past the straight top pair.
    middle = [t for t in result.locked_tracks if t.net.startswith("LANE_A")]
    assert max(t.start.y_nm for t in middle) > nm_from_mm("10.8")
    assert not {f.code for f in run_physical_drc(result.board).findings} & {"DRC-SHORT", "DRC-CLEARANCE"}
    report = json.loads(result.to_json())["bundles"]
    assert report == [{
        "components": ["J1", "U1"], "kind": "differential", "priority": 100,
        "order": [list(group) for group in bundle.order],
        "name_order": [list(_group(name)) for name in ("LANE_A", "LANE_B", "LANE_C")],
        "repair_limit": BUNDLE_REPAIR_LIMIT, "repairs_attempted": 0, "repairs_accepted": 0, "repairs": [],
    }]

    # Forcing the middle pair first (a higher declared priority takes it out
    # of the bundle) reproduces the name-order failure: its shorter detour
    # above the obstacle leaves the walled top pair no way through.
    middle_first = _blocked_middle_board(priorities={"LANE_A": 200})
    forced = route_critical_nets(middle_first, route_global(middle_first))
    assert forced.nets[0].nets == _group("LANE_A") and forced.nets[0].connected
    top = next(item for item in forced.nets if item.nets == _group("LANE_B"))
    assert not top.connected
    assert top.diagnostics[0].startswith("DRC-SHORT")


def test_bundle_repair_rips_up_a_blocking_pair_and_reroutes_both() -> None:
    board = _repair_board(*REPAIRABLE)
    guides = route_global(board)
    without = route_critical_nets(board, guides, pair_state_limit=4000, bundle_repair_limit=0)
    failed = next(item for item in without.nets if item.nets == _group("LANE_A"))
    assert not failed.connected
    assert failed.diagnostics[0].split(":")[0] in {"DRC-CLEARANCE", "DRC-SHORT"}

    events = []
    result = route_critical_nets(board, guides, pair_state_limit=4000,
                                 on_progress=lambda *event: events.append(event[:2]))
    (bundle,) = result.bundles
    assert bundle.repairs_attempted == bundle.repairs_accepted == 1
    (repair,) = bundle.repairs
    assert (repair.failed, repair.ripped_up, repair.accepted) == (_group("LANE_A"), _group("LANE_C"), True)
    assert all(item.connected for item in result.nets)
    # Report order is unchanged; the ripped-up pair's result is replaced.
    assert [item.nets for item in result.nets] == list(bundle.order)
    assert result.nets[1] != without.nets[1]
    # The repaired pairs are committed last, failed pair first.
    assert [t.net[:6] for t in result.locked_tracks][-1] == "LANE_C"
    assert not {f.code for f in run_physical_drc(result.board).findings} & {"DRC-SHORT", "DRC-CLEARANCE"}
    # The re-routed pair is reported again after the repaired pair finishes.
    assert events[-3:] == [("finished", _group("LANE_A")), ("started", _group("LANE_C")),
                           ("finished", _group("LANE_C"))]
    document = json.loads(result.to_json())["bundles"][0]
    assert document["repairs"] == [{"failed": list(_group("LANE_A")), "ripped_up": list(_group("LANE_C")),
                                    "accepted": True, "reason": "both groups accepted"}]


def test_rejected_bundle_repair_restores_the_previous_state_exactly() -> None:
    board = _repair_board(*UNREPAIRABLE)
    guides = route_global(board)
    baseline = route_critical_nets(board, guides, pair_state_limit=4000, bundle_repair_limit=0)
    result = route_critical_nets(board, guides, pair_state_limit=4000)
    (bundle,) = result.bundles
    assert bundle.repairs_attempted >= 1 and bundle.repairs_accepted == 0
    assert all(not item.accepted and item.reason for item in bundle.repairs)
    assert not next(item for item in result.nets if item.nets == _group("LANE_A")).connected
    # Everything but the repair record is identical to never trying.
    assert result.nets == baseline.nets
    assert result.locked_tracks == baseline.locked_tracks
    assert result.locked_vias == baseline.locked_vias
    assert result.board == baseline.board
    assert result.routing_fingerprint == baseline.routing_fingerprint
    assert replace(result, bundles=baseline.bundles) == baseline


def test_bundle_routing_and_reports_are_deterministic() -> None:
    board = _repair_board(*REPAIRABLE)
    guides = route_global(board)
    first = route_critical_nets(board, guides, pair_state_limit=4000)
    second = route_critical_nets(board, guides, pair_state_limit=4000)
    assert first == second
    assert first.to_json() == second.to_json()


def test_bundle_repair_limit_bounds_attempts() -> None:
    board = _repair_board(*REPAIRABLE)
    guides = route_global(board)
    with pytest.raises(ValueError, match="repair limit"):
        route_critical_nets(board, guides, bundle_repair_limit=-1)
    disabled = route_critical_nets(board, guides, pair_state_limit=4000, bundle_repair_limit=0)
    (bundle,) = disabled.bundles
    assert bundle.repair_limit == 0 and bundle.repairs == ()
    assert disabled.status is CriticalRoutingStatus.FAILED


def test_preflight_reports_the_bundle_order_and_repairs(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path
    import pcbir.critical_preflight as preflight

    board = _blocked_middle_board()
    routed = route_critical_nets(board, route_global(board))
    monkeypatch.setattr(preflight, "route_critical_nets", lambda *_, **__: routed)
    report = tmp_path / "report.json"
    preflight.main([str(Path(__file__).resolve().parents[1] / "examples/valid_board/board.copper"),
                    "--allow-proxy-footprints", "--layers", "2", "--fab-profile", "generic",
                    "--router-iterations", "1", "--report", str(report)])
    bundles = json.loads(report.read_text())["critical"]["bundles"]
    assert [bundle["order"] for bundle in bundles] == [[list(group) for group in routed.bundles[0].order]]
    assert ("bundle J1-U1: order LANE_B_N/LANE_B_P, LANE_C_N/LANE_C_P, LANE_A_N/LANE_A_P; "
            "repairs 0/0 accepted (limit 4)") in capsys.readouterr().out

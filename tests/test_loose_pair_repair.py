"""A failed pair outside any bundle records the rip-ups tried on its behalf."""
from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).parent))
from test_critical_bundles import _keepout

from pcbir import (BoardOutline, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
                   PhysicalNet, Placement, Point, RouteKind, Size, nm_from_mm, route_critical_nets, route_global)
from pcbir.physical import DesignRules
from pcbir.progress import critical_progress
from pcbir.run_issues import Run, run_issues


def _loose_board() -> PhysicalBoard:
    """U1 sends LANE_A to J1 and LANE_B to J2: two pairs sharing a package, no bundle.

    J2 is sealed inside a keep-out, so LANE_B can never launch; LANE_A
    (higher priority) is routed first and is the only committed pair that
    shares LANE_B's package.
    """
    package = PhysicalFootprint("test/package-two-pairs", (
        FootprintPad("1", Point.mm("1.4", "-1.25"), Size.mm("0.6", "0.25")),
        FootprintPad("2", Point.mm("1.4", "-0.75"), Size.mm("0.6", "0.25")),
        FootprintPad("3", Point.mm("1.4", "0.75"), Size.mm("0.6", "0.25")),
        FootprintPad("4", Point.mm("1.4", "1.25"), Size.mm("0.6", "0.25")),
    ), Size.mm(3, 8))
    connector = PhysicalFootprint("test/connector-pair", (
        FootprintPad("1", Point.mm("-1.4", "-0.2"), Size.mm("0.6", "0.2")),
        FootprintPad("2", Point.mm("-1.4", "0.2"), Size.mm("0.6", "0.2")),
    ), Size.mm(3, 2))
    rules = []
    for name, priority in (("LANE_A", 110), ("LANE_B", 100)):
        profile = dict(priority=priority, width_nm=nm_from_mm("0.2"), pair_gap_nm=nm_from_mm("0.2"))
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **profile),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **profile)]
    return PhysicalBoard(
        "LoosePairs", BoardOutline.rectangle(30, 20), {package.name: package, connector.name: connector},
        (Placement("U1", package.name, Point.mm(8, 10)), Placement("J1", connector.name, Point.mm(22, 6)),
         Placement("J2", connector.name, Point.mm(22, 14))),
        (PhysicalNet("LANE_A_P", (PadReference("U1", "1"), PadReference("J1", "1"))),
         PhysicalNet("LANE_A_N", (PadReference("U1", "2"), PadReference("J1", "2"))),
         PhysicalNet("LANE_B_P", (PadReference("U1", "3"), PadReference("J2", "1"))),
         PhysicalNet("LANE_B_N", (PadReference("U1", "4"), PadReference("J2", "2")))),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2")),
        copper_keepouts=(_keepout("sealed-connector", "18", "11", "26", "17"),),
        net_routing_rules=tuple(rules),
    )


def test_a_failed_loose_pair_records_the_rip_up_it_tried(tmp_path) -> None:
    board = _loose_board()
    events = []
    result = route_critical_nets(board, route_global(board), pair_state_limit=2000,
                                 on_progress=critical_progress(lambda phase, event, details:
                                                               events.append((event, dict(details)))))
    lane_a, lane_b = result.nets
    assert lane_a.nets == ("LANE_A_N", "LANE_A_P") and lane_a.connected and lane_a.repairs == ()
    assert lane_b.nets == ("LANE_B_N", "LANE_B_P") and not lane_b.connected
    # No bundle joins the two pairs, yet LANE_A shares LANE_B's package: it
    # was ripped up once, LANE_B still could not launch, and the attempt is
    # on LANE_B's result with how the search ended.
    assert result.bundles == ()
    (repair,) = lane_b.repairs
    assert (repair.failed, repair.ripped_up, repair.accepted) == (lane_b.nets, lane_a.nets, False)
    assert repair.reason == "failed group still has no accepted candidate without the ripped-up copper"
    assert not repair.pair_budget_exhausted and 0 <= repair.search_states < 2000
    # LANE_A's copper is exactly what it was before the attempt.
    assert sum(1 for t in result.locked_tracks if t.net.startswith("LANE_A")) == lane_a.track_count
    assert all(not t.net.startswith("LANE_B") for t in result.locked_tracks)
    # The report, the progress event and the issue list all carry it.
    document = json.loads(result.to_json())
    assert document["nets"][1]["repairs"] == [{
        "failed": list(lane_b.nets), "ripped_up": list(lane_a.nets), "accepted": False,
        "reason": repair.reason, "search_states": repair.search_states, "pair_budget_exhausted": False}]
    assert document["nets"][0]["repairs"] == []
    finished = [details for event, details in events if event == "finished" and details["nets"] == list(lane_b.nets)]
    assert finished[-1]["repairs"] == [{
        "ripped_up": list(lane_a.nets), "accepted": False, "reason": repair.reason,
        "search_states": repair.search_states, "pair_budget_exhausted": False}]
    assert "repairs" not in [details for event, details in events if details["nets"] == list(lane_a.nets)][-1]
    (tmp_path / "route-report.json").write_text(json.dumps({"critical": document}), encoding="utf-8")
    (issue,) = run_issues(Run(tmp_path, tmp_path / "route-report.json", None, None))
    assert issue.code == "CRITICAL-UNROUTED" and issue.nets == list(lane_b.nets)
    assert issue.title.endswith(
        f"; ripping up LANE_A_N/LANE_A_P did not help ({repair.search_states} states)")


def test_a_bundled_pair_keeps_its_repairs_on_the_bundle() -> None:
    # The two pairs of one bundle (same two components) use the bundle's
    # ledger; the group results carry none.
    from test_critical_bundles import REPAIRABLE, _repair_board
    result = route_critical_nets(_repair_board(*REPAIRABLE), route_global(_repair_board(*REPAIRABLE)),
                                 pair_state_limit=4000)
    assert any(bundle.repairs for bundle in result.bundles)
    assert all(item.repairs == () for item in result.nets)
    assert all(not isinstance(item, dict) or item.get("search_states") is not None
               for bundle in json.loads(result.to_json())["bundles"] for item in bundle["repairs"])

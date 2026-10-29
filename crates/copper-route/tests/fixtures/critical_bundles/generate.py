"""Regenerate the `critical_bundles` fixtures from the Python oracle.

``boards.state.json`` holds small boards built by the Python bundle, crossing
and nested-exit tests (no committed checkpoint has a critical pair bundle).
``expected.json`` holds, per board and per ``plan_crossings``/
``plan_nested_exits`` combination, the ``bundle_document`` of every planned
bundle, the console lines of its crossings and nested exits and the bundle job
order, plus ``crossing_outcome`` records for synthetic routed vias. Each board
is planned as ``route_critical_nets`` plans it: materialized hard macros,
then promoted planar nets.

    PYTHONHASHSEED=0 PYTHONPATH=.:tests python crates/copper-route/tests/fixtures/critical_bundles/generate.py
"""

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

import test_critical_bundles as bundles
import test_nested_exits as nested
import test_planned_crossings as crossings

from pcbir.critical import _critical_jobs, promote_planar_nets
from pcbir.critical_bundles import (BUNDLE_REPAIR_LIMIT, _group_lands, bundle_document, bundle_job_order,
                                    crossing_line, crossing_outcome, nested_exit_line, plan_bundles)
from pcbir.hard_macros import macro_source, materialize_hard_macros
from pcbir.physical import NetRoutingRule, PhysicalNet, Point, RouteKind, Via
from pcbir.workbench import save_state

HERE = Path(__file__).resolve().parent


def boards():
    centres = ("-2.0", "-1.0", "0", "1.0", "2.0")
    names = ("LANE_D", "LANE_A", "LANE_E", "LANE_B", "LANE_C")
    five = bundles._bundle_board(bundles._rows(centres, "0.5"), bundles._rows(centres, "0.4"), names=names)
    five = replace(five, nets=(*five.nets, PhysicalNet("CLOCK_REF", ())),
                   net_routing_rules=(*five.net_routing_rules, NetRoutingRule("CLOCK_REF", RouteKind.CLOCK,
                                                                              priority=100)))
    yield "five-lanes", five
    yield "split-priority", bundles._bundle_board(bundles._rows(centres, "0.5"), bundles._rows(centres, "0.4"),
                                                  names=names, priorities={"LANE_E": 200})
    yield "blocked-middle", bundles._blocked_middle_board()
    yield "swapped", crossings._swapped()
    yield "swapped-max-vias-1", crossings._swapped(max_vias=1)
    yield "shared-reference", crossings._shared_reference(crossings._swapped())
    yield "reversed-four", crossings._reversed(six_layers=False)
    yield "reversed-six", crossings._reversed(six_layers=True)
    yield "crossing-beside", crossings._crossing_beside(
        ("-1.5", "-0.5", "0.5", "1.5"), ("-2.25", "-0.75", "2.25", "0.75"),
        ("LANE_A", "LANE_B", "LANE_C", "LANE_D"), "LANE_C")
    yield "corner-wrap", nested._corner_board()
    yield "corner-tight", nested._corner_board(nested.TIGHT, clearance="0.3")
    yield "corner-breakout", nested._corner_board(breakout=True)
    yield "side-only", nested._corner_board(nested.SIDE_ONLY)
    yield "facing-only", nested._corner_board(nested.FACING_ONLY)


def synthetic_vias(board, bundle, crossing):
    """Two vias per member (one 1 mm in from each end) and a return via at the first."""
    nets = {item.name: item for item in board.nets}
    placements = {item.reference: item for item in board.placements}
    lands = _group_lands(board, nets, placements, crossing.group)
    first, second = bundle.components
    vias = []
    for index, net in enumerate(crossing.group):
        for here, there in ((first, second), (second, first)):
            step = 1_000_000 if lands[there].x_nm > lands[here].x_nm else -1_000_000
            vias.append(Via(net, Point(lands[here].x_nm + step, lands[here].y_nm + (2 * index - 1) * 200_000),
                            450_000, 200_000))
    vias.append(Via("GND", Point(lands[first].x_nm + 1_000_000, lands[first].y_nm + 700_000), 450_000, 200_000))
    vias.append(Via("GND", Point(lands[first].x_nm - 1_000_000, lands[first].y_nm - 700_000), 450_000, 200_000))
    return vias


def main() -> None:
    saved = {}
    expected = {}
    for name, board in boards():
        saved[name] = board
        staged = materialize_hard_macros(macro_source(board))
        staged, _ = promote_planar_nets(staged)
        rules = {item.net: item for item in staged.net_routing_rules}
        jobs = _critical_jobs(staged)
        case = {}
        for plan_crossings in (True, False):
            for plan_nested in (True, False):
                planned = plan_bundles(staged, jobs, rules, BUNDLE_REPAIR_LIMIT, plan_crossings=plan_crossings,
                                       plan_nested_exits=plan_nested)
                lines = []
                for bundle in planned:
                    lines += [crossing_line(bundle, item) for item in bundle.crossings]
                    lines += [nested_exit_line(bundle, item) for item in bundle.nested_exits]
                case[f"{int(plan_crossings)}{int(plan_nested)}"] = {
                    "bundles": [bundle_document(bundle) for bundle in planned],
                    "lines": lines,
                    "job_order": [list(group) for _, group in bundle_job_order(jobs, planned)],
                }
        outcomes = []
        for bundle_index, bundle in enumerate(plan_bundles(staged, jobs, rules, BUNDLE_REPAIR_LIMIT)):
            for crossing_index, crossing in enumerate(bundle.crossings):
                vias = synthetic_vias(staged, bundle, crossing) if crossing.layer is not None else []
                for connected in (True, False):
                    outcome = crossing_outcome(staged, bundle.components, crossing, rules, connected,
                                               "no accepted candidate", vias)
                    shown = replace(bundle, crossings=(outcome,))
                    outcomes.append({
                        "bundle": bundle_index, "crossing": crossing_index, "connected": connected,
                        "vias": [[via.net, via.position.x_nm, via.position.y_nm] for via in vias],
                        "document": bundle_document(shown)["crossings"][0],
                        "line": crossing_line(shown, outcome),
                    })
        case["outcomes"] = outcomes
        expected[name] = case
    save_state(HERE / "boards.state.json", saved)
    (HERE / "expected.json").write_text(json.dumps(expected, sort_keys=True, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

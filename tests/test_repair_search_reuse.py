"""Exact reuse of repeated detailed repair searches (CS-087 transactional rip-up).

A displaced-net order loop rebuilds the same trial copper for every order, so a
blocker that leads two orders is searched twice against identical inputs. The
memo may skip only such exact repeats: the routed result, report and
fingerprints must not change, and a failure must never outlive its blockers.
"""
from __future__ import annotations

from dataclasses import replace
import json


from pcbir import routing_benchmark
from pcbir import (
    CopperKeepout,
    CopperLayer,
    GlobalRouterOptions,
    NetRoutingRule,
    PadReference,
    PhysicalNet,
    Placement,
    Point,
    PolygonRing,
    PolygonWithHoles,
    TrackSegment,
    nm_from_mm,
    route_global,
)
from pcbir.routing_clearance import RoutingClearanceIndex
from test_detailed_routing import _board
from test_routing_benchmark import save_run

WIDTH = nm_from_mm("0.25")
# Work counters differ by design: a reuse hit repeats no search.
TELEMETRY = {"search_identity", "search_reuse", "reused_search_seconds",
             "search_seconds", "search_stats"}


def _without_search_work(document: str) -> dict:
    parsed = json.loads(document)
    parsed.pop("search_work")
    return parsed


def _keepout(name: str, corners: tuple[Point, Point]) -> CopperKeepout:
    (x0, y0), (x1, y1) = ((item.x_nm, item.y_nm) for item in corners)
    return CopperKeepout(name, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing(
        tuple(Point(x, y) for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1))))))


def _corridor_board(*front_only: str, crossing: bool = False):
    """SIGNAL crosses three vertical blockers; front walls close both ends.

    A front-only net cannot cross a front-layer SIGNAL track; the others can
    reroute through the back layer with two vias. With ``crossing``, BLOCK3's
    pads move against edge strips, so front-only SIGNAL and BLOCK3 can never
    both connect and ordinary routing must enter transactional repair.
    """
    base = _board()
    footprint = next(iter(base.footprints))
    placements, nets = list(base.placements), list(base.nets)
    for index, x in enumerate((6, 10, 14), 1):
        ends = ("0.8", "11.2") if crossing and index == 3 else (3, 9)
        placements += [Placement(f"B{index}A", footprint, Point.mm(x, ends[0])),
                       Placement(f"B{index}B", footprint, Point.mm(x, ends[1]))]
        nets.append(PhysicalNet(f"BLOCK{index}", (
            PadReference(f"B{index}A", "1"), PadReference(f"B{index}B", "1"))))
    walls = (_keepout("west", (Point.mm(0, 0), Point.mm("2.4", 12))),
             _keepout("east", (Point.mm("17.6", 0), Point.mm(20, 12))))
    if crossing:
        walls += (_keepout("south", (Point.mm(0, 0), Point.mm(20, "0.5"))),
                  _keepout("north", (Point.mm(0, "11.5"), Point.mm(20, 12))))
    return replace(
        base, placements=tuple(placements), nets=tuple(nets), copper_keepouts=walls,
        net_routing_rules=tuple(NetRoutingRule(net, allowed_layers=(CopperLayer.FRONT,))
                                for net in front_only),
    )


def _guides(board):
    return {item.net: item for item in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))).routes}


def _without_telemetry(events):
    return [(phase, event, {key: value for key, value in details.items()
                            if key not in TELEMETRY})
            for phase, event, details in events]


def test_clearance_index_records_ordered_insertions_with_lock_state() -> None:
    board = _corridor_board()
    track = TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6), WIDTH, CopperLayer.FRONT)
    index = RoutingClearanceIndex(board)
    assert index.additions() == ()
    index.add_track(track)
    index.add_track(track, locked=True)
    assert index.additions() == ((track, False), (track, True))
    locked_input = RoutingClearanceIndex(replace(board, tracks=(track,)))
    assert locked_input.additions() == ((track, True),)


def test_benchmark_counts_reused_attempts_and_treats_the_bound_as_an_intervention(tmp_path) -> None:
    events = []
    for reuse in ("miss", "hit"):
        events += [
            {"phase": "detailed_net", "event": "started", "elapsed_seconds": len(events),
             "details": {"net": "V", "stage": "evicted_net"}},
            {"phase": "detailed_net", "event": "finished", "elapsed_seconds": len(events) + 1,
             "details": {"net": "V", "connected": False, "search_reuse": reuse}}]
    save_run(tmp_path / "plain")
    manifest = save_run(tmp_path / "reused")
    manifest["routing_command"] += ["--search-reuse-entries", "0"]
    (tmp_path / "reused/run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (tmp_path / "reused/routing.log").write_text(
        "".join("PROGRESS " + json.dumps(item) + "\n" for item in events), encoding="utf-8")
    plain = routing_benchmark.summarize_run(tmp_path / "plain")
    reused = routing_benchmark.summarize_run(tmp_path / "reused")
    assert reused["work"]["detailed_net_attempts"] == 2
    assert reused["work"]["reused_detailed_attempts"] == 1
    assert reused["routing_settings"] == plain["routing_settings"]



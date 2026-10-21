"""Enclosure-window repair: an open net and the routed nets that wall it in move together."""
from dataclasses import replace

import pcbir.detailed as D
from pcbir.detailed import DetailedNetResult, DetailedRouterOptions, _NetAttempt, _segment_meets_box
from pcbir.importers.kicad_pcb import parse_kicad_pcb
from pcbir.physical import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.routing_clearance import RoutingClearanceIndex

BOARD = """(kicad_pcb (version 20260206) (generator "test")
  (layers (0 "F.Cu" signal) (2 "B.Cu" signal) (25 "Edge.Cuts" user))
  (net 0 "") (net 1 "A") (net 2 "B") (net 3 "C")
{pads}
  (gr_rect (start 0 0) (end 30 20) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
)"""
PADS = {"A": ((5, 10), (25, 10)), "B": ((10, 4), (10, 16)), "C": ((20, 4), (20, 16))}


def _board():
    footprints = []
    for code, (net, ends) in enumerate(PADS.items(), 1):
        for index, (x, y) in enumerate(ends, 1):
            footprints.append(
                f'  (footprint "TP" (layer "F.Cu") (at {x} {y})\n'
                f'    (property "Reference" "TP{net}{index}" (at 0 0 0) (layer "F.SilkS"))\n'
                f'    (pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask") (net {code} "{net}")))')
    return parse_kicad_pcb(BOARD.replace("{pads}", "\n".join(footprints)), source="enclosure").board


def _track(net):
    (x0, y0), (x1, y1) = PADS[net]
    return TrackSegment(net, Point(nm_from_mm(x0), nm_from_mm(y0)), Point(nm_from_mm(x1), nm_from_mm(y1)),
                        nm_from_mm("0.25"), CopperLayer.FRONT)


def _attempt(net, connected, tracks=()):
    result = DetailedNetResult(net, connected, len(tracks), 0, 0, 0,
                               () if connected else (f"detailed search cannot reach TP{net}2.1",))
    return _NetAttempt(result, tuple(tracks), (), frozenset())


def _repair(monkeypatch, fails):
    """Run the repair with a scripted search: ``fails(name, routed_before)`` decides each outcome."""
    board = _board()
    trials = {}
    calls = []
    alive = []  # keep each index alive so its id is never reused within the run

    def search(board, net, rule, guide, usage, history, clearance, options, **kwargs):
        alive.append(clearance)
        routed = trials.setdefault(id(clearance), [])
        calls.append((net.name, tuple(routed)))
        if net.name == "A" and not routed and len(calls) == 1:
            return _attempt("A", True, (_track("A"),))  # the free path, with no other copper
        if fails(net.name, tuple(routed)):
            return _attempt(net.name, False)
        routed.append(net.name)
        return _attempt(net.name, True, (_track(net.name),) if net.name == "A" else ())

    monkeypatch.setattr(D, "_search_detailed_net", search)
    monkeypatch.setattr(D, "_negotiate_window", lambda *args: None)
    selected = {"A": _attempt("A", False), "B": _attempt("B", True, (_track("B"),)),
                "C": _attempt("C", True, (_track("C"),))}
    clearance = RoutingClearanceIndex(board)
    record = []
    options = DetailedRouterOptions(enclosure_repair_nets=4, enclosure_repair_orders=4)
    result, _ = D._enclosure_repair(board, list(board.nets), {}, {}, selected, clearance, options, {},
                                    on_progress=None, reuse=None, repair_trials=record)
    return selected, result, record, calls


def test_net_that_fails_goes_first_in_the_next_order(monkeypatch):
    # C cannot route once B has; with C promoted ahead of the others, all three fit.
    selected, result, record, calls = _repair(monkeypatch, lambda name, before: name == "C" and "B" in before)
    assert all(item.result.connected for item in result.values())
    assert record[0].outcome == "enclosure_window_accepted"
    assert record[0].displaced_nets == ("B", "C")  # both cross the open net's free path
    assert record[0].orders_tried == 2
    assert [name for name, _ in calls[1:]] == ["A", "B", "C", "C", "A", "B"]


def test_failed_window_changes_nothing(monkeypatch):
    selected, result, record, _ = _repair(monkeypatch, lambda name, before: name == "C")
    assert result == selected
    assert record[0].outcome == "enclosure_window_failed"


def test_segment_crossing_a_box_counts_even_with_both_ends_outside():
    box = (0, 0, 10, 10)
    assert _segment_meets_box(Point(-5, 5), Point(15, 5), box)
    assert _segment_meets_box(Point(5, 5), Point(5, 6), box)
    assert not _segment_meets_box(Point(-5, -5), Point(-1, 20), box)
    assert not _segment_meets_box(Point(-5, 12), Point(15, 30), box)


def test_options_reject_out_of_range_enclosure_settings():
    for bad in ({"enclosure_repair_nets": 13}, {"enclosure_repair_orders": 0},
                {"enclosure_repair_radius_nm": 0}, {"enclosure_repair_open_nets": -1}):
        try:
            replace(DetailedRouterOptions(), **bad)
        except ValueError:
            continue
        raise AssertionError(f"accepted {bad}")

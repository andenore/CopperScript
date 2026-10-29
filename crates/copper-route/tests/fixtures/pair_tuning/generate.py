"""Regenerate the `pair_tuning` fixtures from the Python oracle.

``inputs.state.json`` holds the boards once (``boards``) and, per case
(``cases``, naming its boards), the inputs the Python unit tests give
``tune_virtual_pair_spans`` (``tests/test_pair_tuning_spans.py``,
``tests/test_pair_tuning_span_copper_contacts.py``), ``_tune_pair`` with and
without breakout regions, and ``_matched_local_candidates``
(``tests/test_pair_obstacle_tuning_fallback.py``), plus a few neighbouring
variations. Tracks are saved as ``(net, layer, x0, y0, x1, y1, width)`` rows.
``expected.json`` holds Python's outputs: the added length and resulting
tracks (``None`` for a member left unchanged), or the tuned proposals' tracks
(a candidate within its skew limit yields only itself, which the caller has
already tried: recorded as no proposal).

    PYTHONHASHSEED=0 PYTHONPATH=.:tests python crates/copper-route/tests/fixtures/pair_tuning/generate.py
"""

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

import test_pair_obstacle_tuning_fallback as fallback
from test_pair_tuning_spans import AMP, SKEW, W, fixture, fragment

from pcbir.breakout import BreakoutRegions
from pcbir.critical import _tune_pair
from pcbir.pair_search import PairSearchCandidate
from pcbir.pair_terminals import _matched_local_candidates
from pcbir.pair_tuning_spans import tune_virtual_pair_spans
from pcbir.physical import CopperLayer, Point, TrackSegment, Via
from pcbir.workbench import save_state

HERE = Path(__file__).resolve().parent
F = CopperLayer.FRONT


def rows(tracks):
    return [[t.net, t.layer.value, t.start.x_nm, t.start.y_nm, t.end.x_nm, t.end.y_nm, t.width_nm] for t in tracks]


def outcome(added, case, first, second):
    """The added length and each member's tracks, ``None`` for a member left exactly as it was."""
    return {"added": added, **{key: None if tracks == list(case[key]) else rows(tracks)
                               for key, tracks in (("first", first), ("second", second))}}


def spans_case(board, parts, *, held=None, protected=(), vias=(), amplitude=AMP):
    case = {"kind": "spans", "source": board, "first": tuple(parts[0]), "second": tuple(parts[1]),
            "skew": SKEW, "amplitude": amplitude, "margin": 32, "protected": tuple(protected), "vias": tuple(vias)}
    if held is not None:
        case["held"] = held
    return case


def spans_cases():
    board, lanes = fixture()
    parts = tuple(fragment(x) for x in lanes)
    yield "spans-fragmented", spans_case(board, parts)
    yield "spans-unfragmented", spans_case(board, lanes)
    for where in ("source", "held", "source_reverse"):
        edge = TrackSegment("A", parts[0][0].start, parts[0][-1].end, W, F)
        if where == "source_reverse":
            edge = replace(edge, start=edge.end, end=edge.start)
        if where == "held":
            yield f"spans-subdivision-{where}", spans_case(board, parts, held=replace(board, tracks=(edge,)))
        else:
            yield f"spans-subdivision-{where}", spans_case(replace(board, tracks=(edge,)), parts)
    branch = (fragment(lanes[0]), fragment(lanes[1]))
    branch[0].append(TrackSegment("A", Point.mm(10, 5), Point.mm(10, 4.9), 120000, F))
    yield "spans-branch", spans_case(board, branch)
    short, short_lanes = fixture(length=3000000, spacing=600000)
    short_parts = tuple(fragment(x) for x in short_lanes)
    vias = tuple(Via("A", Point(x, 5000000), 450000, 200000, F, CopperLayer.BACK) for x in (6000000, 7000000))
    yield "spans-short", spans_case(short, short_parts)
    yield "spans-short-vias", spans_case(short, short_parts, vias=vias)
    yield "spans-short-one-via", spans_case(short, short_parts, vias=vias[:1])
    yield "spans-short-protected", spans_case(short, short_parts,
                                              protected=tuple(("A", F, t.end) for t in short_parts[0][:-1]))
    yield "spans-short-protected-one", spans_case(short, short_parts, protected=(("A", F, short_parts[0][2].end),))
    neck, neck_lanes = fixture(neck=W)
    yield "spans-equal-neck", spans_case(neck, tuple(fragment(x) for x in neck_lanes))
    yield "spans-amplitude", spans_case(board, parts, amplitude=0)
    yield "spans-small-amplitude", spans_case(board, parts, amplitude=200000)
    yield "spans-stale-intent", spans_case(board, parts, held=replace(board, metadata={"stale": "context"}))
    edge = TrackSegment("OBSTACLE", Point.mm(20, 12), Point.mm(21, 12), 100000, F)
    lost = replace(board, tracks=(edge, edge))
    yield "spans-lost-prefix", spans_case(lost, parts, held=replace(lost, tracks=(edge,)))
    yield "spans-extra-held", spans_case(board, parts, held=replace(board, tracks=(edge,)))
    wall = TrackSegment("OBSTACLE", Point.mm(13, 4.75), Point.mm(14.8, 4.75), 100000, F)
    yield "spans-blocked", spans_case(board, parts, held=replace(board, tracks=(wall,)))
    far = TrackSegment("OBSTACLE", Point.mm(13, 3.0), Point.mm(14.8, 3.0), 100000, F)
    yield "spans-wall-beyond", spans_case(board, parts, held=replace(board, tracks=(far,)))
    reversed_parts = tuple([replace(t, start=t.end, end=t.start) for t in reversed(x)] for x in parts)
    yield "spans-reversed", spans_case(board, reversed_parts)
    names = {"A": "RENAMED_SIGNAL", "B": "RENAMED_PARTNER", "OBSTACLE": "OTHER"}
    renamed = replace(board, nets=tuple(replace(n, name=names[n.name]) for n in board.nets),
                      net_routing_rules=tuple(replace(r, net=names[r.net],
                                                      differential_partner=names[r.differential_partner])
                                              for r in board.net_routing_rules))
    yield "spans-renamed", spans_case(renamed, tuple([replace(t, net=names[t.net]) for t in x] for x in parts))
    yield "spans-swapped", spans_case(board, (parts[1], parts[0]))
    for layer in (CopperLayer.FRONT, CopperLayer.BACK):
        bare = replace(board, placements=(), nets=(), net_routing_rules=())
        first = [TrackSegment("A", Point.mm(10, 5), Point.mm(11, 5), W, layer),
                 TrackSegment("A", Point.mm(10, 4), Point.mm(10, 5), W, layer)]
        second = [TrackSegment("B", Point.mm(5, 10), Point.mm(20, 10), W, layer)]
        yield f"spans-unordered-{layer.value}", spans_case(bare, (first, second))
    for shape in ("parallel", "endpoint_disks"):
        if shape == "parallel":
            rail = TrackSegment("A", Point.mm(5, 4.95), Point.mm(15, 4.95), 100000, F)
            edges = (rail, rail)
        else:
            edges = tuple(TrackSegment("A", Point(t.end.x_nm + 10000, t.end.y_nm - 50000),
                                       Point(t.end.x_nm + 20000, t.end.y_nm - 50000), 100000, F)
                          for t in parts[0])
        yield f"spans-contact-{shape}-held", spans_case(board, parts, held=replace(board, tracks=edges))
        yield f"spans-contact-{shape}-source", spans_case(replace(board, tracks=edges), parts)
    rail = TrackSegment("A", Point.mm(5, 4.95), Point.mm(15, 4.95), 100000, CopperLayer.BACK)
    yield "spans-back-rail", spans_case(board, parts, held=replace(board, tracks=(rail,)))


def tune_case(board, first, second, amplitude, margin):
    return {"kind": "tune", "source": board, "first": tuple(first), "second": tuple(second),
            "skew": SKEW, "amplitude": amplitude, "margin": margin}


def tune_cases():
    for name, kwargs in (("default", {}), ("short", {"length": 3000000, "spacing": 600000}),
                         ("neck", {"neck": W}), ("long", {"length": 20000000})):
        board, lanes = fixture(**kwargs)
        for amplitude, margin in ((AMP, 32), (300000, 0), (120000, 32)):
            yield f"tune-{name}-{amplitude}-{margin}", tune_case(board, *lanes, amplitude, margin)
        yield f"tune-{name}-fragments", tune_case(board, *(fragment(x) for x in lanes), AMP, 32)
    board, lanes = fixture()
    yield "tune-swapped-reversed", tune_case(board, reversed(lanes[1]), lanes[0], AMP, 32)


def matched_case(board, first, second, via_pairs=(), return_vias=()):
    return {"kind": "matched", "source": board, "first": tuple(first), "second": tuple(second),
            "via_pairs": tuple(via_pairs), "return_vias": tuple(return_vias)}


def matched_cases():
    for name, kwargs in (("plain", {}), ("renamed", {"renamed": True}), ("blocked", {"blocked": True})):
        board, candidate, _ = fallback.scene(**kwargs)
        yield f"matched-{name}", matched_case(board, candidate.first, candidate.second)
    board, candidate, _ = fallback.scene()
    yield "matched-swapped", matched_case(board, candidate.second, candidate.first)
    yield "matched-within-limit", matched_case(board, candidate.first, candidate.first)
    for name, change in (("wide-limit", {"max_skew_nm": 3000000}),
                         ("no-amplitude", {"tuning_amplitude_limit_nm": None}),
                         ("small-amplitude", {"tuning_amplitude_limit_nm": 400000})):
        rules = tuple(replace(r, **change) for r in board.net_routing_rules)
        yield f"matched-{name}", matched_case(replace(board, net_routing_rules=rules), candidate.first,
                                              candidate.second)
    via = Via("GND", Point.mm(15, 14), 450000, 200000, F, CopperLayer.BACK)
    yield "matched-return-via", matched_case(board, candidate.first, candidate.second, return_vias=(via,))


def main() -> None:
    boards = {}
    cases = {}
    expected = {}
    for name, case in (*spans_cases(), *tune_cases(), *matched_cases()):
        # Boards are saved once without tracks; each case lists its tracks.
        stored = {key: value for key, value in case.items() if key not in ("source", "held")}
        source = case["source"]
        base = replace(source, tracks=())
        label = next((k for k, v in boards.items() if v == base), f"board-{len(boards)}")
        boards[label] = base
        stored["source"] = label
        stored["source_tracks"] = tuple(map(tuple, rows(source.tracks)))
        held = case.get("held", source)
        if held is not source:
            assert held == replace(source, tracks=held.tracks, metadata=held.metadata)
            stored["held_tracks"] = tuple(map(tuple, rows(held.tracks)))
            stored["held_metadata"] = dict(held.metadata)
        for key in ("first", "second"):
            stored[key] = tuple(map(tuple, rows(case[key])))
        if "protected" in case:
            stored["protected"] = tuple((net, layer.value, p.x_nm, p.y_nm) for net, layer, p in case["protected"])
        cases[name] = stored
        if case["kind"] == "spans":
            first, second = list(case["first"]), list(case["second"])
            added = tune_virtual_pair_spans(first, second, case["skew"], case["amplitude"], case["margin"],
                                            source=case["source"], held=case.get("held", case["source"]),
                                            protected_nodes=case["protected"], proposed_vias=case["vias"],
                                            tuner=_tune_pair)
            expected[name] = outcome(added, case, first, second)
        elif case["kind"] == "tune":
            result = {}
            for label, breakout in (("plain", None), ("breakout", BreakoutRegions(case["source"]))):
                first, second = list(case["first"]), list(case["second"])
                added = _tune_pair(first, second, case["skew"], case["amplitude"], case["margin"], breakout)
                result[label] = outcome(added, case, first, second)
            expected[name] = result
        else:
            board = case["source"]
            first_rule, second_rule = board.net_routing_rules
            candidate = PairSearchCandidate(case["first"], case["second"], 0, 0, via_pairs=case["via_pairs"],
                                            return_vias=case["return_vias"])
            proposals = list(_matched_local_candidates(board, first_rule, second_rule, candidate))
            if proposals == [candidate]:
                proposals = []  # The caller has already tried the untuned candidate.
            expected[name] = [{"first": rows(p.first), "second": rows(p.second)} for p in proposals]
    save_state(HERE / "inputs.state.json", {"boards": boards, "cases": cases})
    (HERE / "expected.json").write_text(json.dumps(expected, sort_keys=True, separators=(",", ":")) + "\n",
                                        encoding="utf-8")


if __name__ == "__main__":
    main()

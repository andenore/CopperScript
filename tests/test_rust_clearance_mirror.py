"""The prototype Rust clearance mirror answers exactly as the Python index."""
from __future__ import annotations

from dataclasses import replace
from random import Random

import pytest

from pcbir import Point, TrackSegment, Via, nm_from_mm
from pcbir.geometry import RoundedConvexShape
from pcbir.native import clearance_mirror
from pcbir.routing_clearance import RoutingClearanceIndex

from tests.test_routing_clearance_remove import LAYERS, NETS, answers, board, copper

pytestmark = pytest.mark.skipif(clearance_mirror() is None,
                                reason="Rust clearance mirror not built (tools/build_clearance_rs.py)")


def boards():
    with_breakout = board()
    # Without a breakout rule, _clear itself is answered by the mirror.
    plain = replace(with_breakout, net_routing_rules=tuple(
        rule for rule in with_breakout.net_routing_rules if rule.breakout_length_nm is None))
    return {"breakout": with_breakout, "plain": plain}


def pair(source):
    native, python = RoutingClearanceIndex(source), RoutingClearanceIndex(source)
    python._mirror = None
    assert native._mirror is not None
    return native, python


def random_queries(native, python, seed: int, count: int = 1500) -> None:
    random = Random(seed)
    for _ in range(count):
        net = random.choice((*NETS, "X"))
        x, y = random.randrange(0, 33_000) * 1000, random.randrange(0, 35_000) * 1000
        layers = tuple(random.sample(LAYERS, random.randrange(1, 5)))
        kind = random.random()
        if kind < 0.6:
            dx, dy = random.randrange(-4_000, 4_000) * 1000, random.randrange(-4_000, 4_000) * 1000
            shape = RoundedConvexShape((Point(x, y), Point(x + dx, y + dy)), random.randrange(0, 400_000))
        elif kind < 0.9:
            shape = RoundedConvexShape((Point(x, y),), random.randrange(0, 700_000))
        else:
            # A small convex polygon spine (pad-like query shape).
            s = random.randrange(100_000, 900_000)
            shape = RoundedConvexShape((Point(x, y), Point(x + s, y), Point(x + s, y + s), Point(x, y + s)),
                                       random.randrange(0, 100_000))
        floor = random.choice((0, 0, nm_from_mm("0.25"), nm_from_mm("0.6")))
        assert (native._clear(net, shape, layers, clearance_floor_nm=floor)
                == python._clear(net, shape, layers, clearance_floor_nm=floor))
        allowed = random.choice((None, *(item.pad_reference for item in python._objects[:12]
                                        if item.pad_reference is not None)))
        assert (native.pad_copper_clear(shape, layers, allowed_pad=allowed)
                == python.pad_copper_clear(shape, layers, allowed_pad=allowed))


@pytest.mark.parametrize("name", ["breakout", "plain"])
def test_fresh_index_answers_match(name):
    native, python = pair(boards()[name])
    assert answers(native) == answers(python)
    random_queries(native, python, seed=7)


@pytest.mark.parametrize("name", ["breakout", "plain"])
def test_additions_removals_and_compaction_stay_in_step(name, monkeypatch):
    monkeypatch.setattr(RoutingClearanceIndex, "_COMPACT_MIN_DEAD", 3)
    native, python = pair(boards()[name])
    items = copper(11, count=80)
    random = Random(5)
    for step in range(4):
        for item, locked in items[step * 20:(step + 1) * 20]:
            for index in (native, python):
                (index.add_via if isinstance(item, Via) else index.add_track)(item, locked=locked)
        removed = random.sample(NETS, 2)
        for net in removed:
            assert native.remove(net) == python.remove(net)
        assert len(native._mirror) == len(native._objects)
        assert answers(native) == answers(python)
        random_queries(native, python, seed=100 + step, count=600)


def test_unbuilt_or_disabled_mirror_falls_back(monkeypatch):
    import pcbir.native as native_module
    monkeypatch.setattr(native_module, "_mirror", False)
    index = RoutingClearanceIndex(boards()["plain"])
    assert index._mirror is None
    track = TrackSegment("A", Point.mm(3, 3), Point.mm(9, 3), nm_from_mm("0.2"), LAYERS[0])
    assert index.can_track("A", track.start, track.end, track.width_nm, track.layer) in (True, False)

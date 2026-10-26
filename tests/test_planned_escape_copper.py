"""Promised exits join the ordinary stage as copper, and the fanout keeps them."""
from collections import Counter
from dataclasses import replace
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from test_unified_router import _board

from pcbir import CopperLayer, CriticalRoutingStatus, FootprintPad, Point, Size, TrackSegment, Via, nm_from_mm
from pcbir.critical import CriticalRoutingResult
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.flow import _copper_keepouts, _release_escape_masks
from pcbir.unified_router import UnifiedRouterOptions


def _reserved(board):
    stem = TrackSegment("S", Point.mm(13, 10), Point.mm(13, 12), nm_from_mm("0.2"), CopperLayer.FRONT)
    via = Via("S", Point.mm(13, 12), nm_from_mm("0.6"), nm_from_mm("0.3"))
    return (stem,), (via,), _copper_keepouts(board, (stem,), (via,), "escape-reserve", fixed=board)


def _events():
    events = []
    return events, lambda phase, event, details: events.append((phase, event, dict(details)))


def test_planned_escapes_join_the_ordinary_board_exact_checked() -> None:
    board = _board({"S": ((10, 7), (10, 17)), "C": ((1, 9), (19, 9))})
    reserved = _reserved(board)
    masked = replace(board, copper_keepouts=reserved[2])
    critical = CriticalRoutingResult(CriticalRoutingStatus.SUCCESS, masked, (), (), (), "", "")
    events, observe = _events()
    _, _, pre = _release_escape_masks(reserved, critical, None, masked,
                                         unified_options=UnifiedRouterOptions(), on_progress=observe)
    # The masks are gone and the promised stem and via are the net's copper.
    assert not any(k.id.startswith("escape-reserve") for k in pre.copper_keepouts)
    assert reserved[0][0] in pre.tracks and reserved[1][0] in pre.vias
    assert [d for p, e, d in events if (p, e) == ("ordinary_escapes", "joined")] == [
        {"tracks": 1, "vias": 1, "reused_tracks": 0, "reused_vias": 0, "dropped_tracks": 0, "dropped_vias": 0,
         "dropped_nets": []}]
    # A promised via another step already placed on its site is reused: no
    # second drill at the same point.
    events, observe = _events()
    _, _, reused = _release_escape_masks(reserved, critical, None,
                                            replace(masked, vias=reserved[1]),
                                            unified_options=UnifiedRouterOptions(), on_progress=observe)
    assert reused.vias.count(reserved[1][0]) == 1 and reserved[0][0] in reused.tracks
    joined = next(d for p, e, d in events if (p, e) == ("ordinary_escapes", "joined"))
    assert (joined["vias"], joined["reused_vias"], joined["dropped_vias"], joined["tracks"]) == (0, 1, 0, 1)
    # Off: the masks go and the copper stays out, as before.
    _, _, plain = _release_escape_masks(reserved, critical, None, masked,
                                           unified_options=UnifiedRouterOptions(planned_escapes_as_copper=False))
    assert plain.tracks == board.tracks and plain.vias == board.vias
    # A promised via a preparation step has since made illegal is left out
    # and counted; the legal stem still joins.
    crossing = TrackSegment("C", Point.mm(12, 12), Point.mm(14, 12), nm_from_mm("0.2"), CopperLayer.FRONT)
    events, observe = _events()
    _, _, cut = _release_escape_masks(reserved, critical, None,
                                         replace(masked, tracks=(crossing,)),
                                         unified_options=UnifiedRouterOptions(), on_progress=observe)
    assert reserved[1][0] not in cut.vias
    joined = next(d for p, e, d in events if (p, e) == ("ordinary_escapes", "joined"))
    assert joined["dropped_vias"] == 1 and joined["dropped_tracks"] + joined["tracks"] == 1
    assert joined["dropped_nets"] == ["S"]


def test_the_ordinary_stage_fanout_reuses_the_joined_planned_escapes() -> None:
    board = _board({"S": ((10, 7), (10, 17)), "C": ((1, 9), (19, 9))}, layers=4)
    fp = next(iter(board.footprints.values()))
    fp = replace(fp, pads=(*fp.pads, FootprintPad("2", Point.mm(-1, 0), Size.mm("0.6", "0.6"))))
    board = replace(board, footprints={fp.name: fp})
    options = FanoutOptions(minimum_component_pads=1)
    # The planning stage's escapes, reserved as masks while critical nets route.
    planned = route_fanout(board, options, only_nets=frozenset({"S"}))
    tracks = tuple((Counter(planned.board.tracks) - Counter(board.tracks)).elements())
    vias = tuple((Counter(planned.board.vias) - Counter(board.vias)).elements())
    assert tracks and vias
    reserved = (tracks, vias, _copper_keepouts(board, tracks, vias, "escape-reserve", fixed=board))
    masked = replace(board, copper_keepouts=reserved[2])
    critical = CriticalRoutingResult(CriticalRoutingStatus.SUCCESS, masked, (), (), (), "", "")
    _, _, pre = _release_escape_masks(reserved, critical, None, masked,
                                         unified_options=UnifiedRouterOptions())
    # The ordinary stage's fanout, with the same options, finds its exits
    # already there: the same accesses, no new copper.
    again = route_fanout(pre, options, only_nets=frozenset({"S"}))
    assert again.accesses == planned.accesses
    assert not again.created_tracks and not again.created_vias
    assert again.board == pre

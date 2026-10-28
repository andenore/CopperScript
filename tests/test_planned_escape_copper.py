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


def test_fine_pitch_plane_pins_keep_a_via_site_free_from_planned_escapes() -> None:
    from pcbir import (BoardOutline, CopperZone, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                       Placement, PolygonRing, PolygonWithHoles, Stackup)
    from pcbir.flow import _plane_pin_halos

    # A 0.5 mm row on the right edge of a package: P's plane is on B.Cu only,
    # so P needs a stitching via in front of its land; G has a pour on its
    # own layer and S is a signal.
    pads = tuple(FootprintPad(str(i + 1), Point.mm("1.4", y), Size.mm("0.6", "0.25"))
                 for i, y in enumerate(("-0.5", "0", "0.5")))
    package = PhysicalFootprint("test/row", pads, Size.mm(3, 3))
    outline = BoardOutline.rectangle(20, 20)
    plane = PolygonWithHoles(PolygonRing(outline.vertices))
    board = PhysicalBoard(
        "PlanePins", outline, {package.name: package}, (Placement("U1", package.name, Point.mm(10, 10)),),
        (PhysicalNet("S", (PadReference("U1", "1"),)), PhysicalNet("P", (PadReference("U1", "2"),)),
         PhysicalNet("G", (PadReference("U1", "3"),))),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                                       CopperLayer.BACK)),
        zones=(CopperZone("p-plane", "P", (CopperLayer.BACK,), plane),
               CopperZone("g-pour", "G", (CopperLayer.FRONT, CopperLayer.INTERNAL_1), plane)))
    # With 0.8 mm vias and 0.2 mm clearance a via in front of P would block
    # its neighbours' straight escapes at this pitch: no halo.
    assert _plane_pin_halos(board) == ()
    from pcbir.physical import DesignRules
    board = replace(board, rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"),
                                             minimum_track_width_nm=nm_from_mm("0.1"),
                                             default_track_width_nm=nm_from_mm("0.15"),
                                             default_via_size_nm=nm_from_mm("0.45"),
                                             default_via_drill_nm=nm_from_mm("0.2")))
    (halo,) = _plane_pin_halos(board)
    assert halo.layers == (CopperLayer.FRONT,) and halo.block_vias and halo.block_tracks
    vertices = halo.outline.outer.vertices
    xs, ys = {v.x_nm for v in vertices}, {v.y_nm for v in vertices}
    gap, via = board.rules.minimum_clearance_nm, board.rules.default_via_size_nm
    # From one clearance past P's outer edge (x = 11.7 mm), a via and a
    # clearance outward, one via plus clearance either side of its centre.
    assert (min(xs), max(xs)) == (nm_from_mm("11.7") + gap, nm_from_mm("11.7") + 2 * gap + via)
    assert (min(ys), max(ys)) == (nm_from_mm(10) - via // 2 - gap, nm_from_mm(10) + via // 2 + gap)

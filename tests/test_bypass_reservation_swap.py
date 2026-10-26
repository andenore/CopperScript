"""A bypass that the escape reservations block takes a ball's site only as a swap."""
from __future__ import annotations

from dataclasses import replace
from types import MappingProxyType

import pcbir.decoupling as decoupling
import pcbir.flow as flow
from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint,
                   PhysicalNet, Placement, Point, Size, Stackup, TrackSegment, Via, nm_from_mm)
from pcbir.decoupling import DecouplingRouteResult
from pcbir.flow import _copper_keepouts, _legal_reservation_nets, _plan_decouplers
from pcbir.physical import BoardSide, DecouplingLink, DesignRules

PIN = PhysicalFootprint("test/pin", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))
CAP = PhysicalFootprint("test/cap", (FootprintPad("1", Point.mm("-0.5", 0), Size.mm("0.6", "0.6")),
                                     FootprintPad("2", Point.mm("0.5", 0), Size.mm("0.6", "0.6"))), Size.mm(2, 1))
LINK = DecouplingLink(PadReference("CAP", "1"), PadReference("U", "1"), PadReference("CAP", "2"), "P", "G")


def _board() -> PhysicalBoard:
    """S's reservation (a stem to a via at (13,12)) lies between U.1 and the capacitor behind it."""
    return PhysicalBoard(
        "Swap", BoardOutline.rectangle(26, 26), {PIN.name: PIN, CAP.name: CAP},
        (Placement("S_0", PIN.name, Point.mm(13, 9)), Placement("S_1", PIN.name, Point.mm(13, 20)),
         Placement("U", PIN.name, Point.mm("14.3", "11.5")), Placement("P_1", PIN.name, Point.mm(4, 4)),
         Placement("G_1", PIN.name, Point.mm(4, 22)),
         Placement("CAP", CAP.name, Point.mm(12, "13.5"), side=BoardSide.BACK)),
        (PhysicalNet("S", (PadReference("S_0", "1"), PadReference("S_1", "1"))),
         PhysicalNet("P", (PadReference("U", "1"), PadReference("CAP", "1"), PadReference("P_1", "1"))),
         PhysicalNet("G", (PadReference("CAP", "2"), PadReference("G_1", "1")))),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.BACK)),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.15"), minimum_track_width_nm=nm_from_mm("0.15"),
                          default_track_width_nm=nm_from_mm("0.2"), default_via_size_nm=nm_from_mm("0.6"),
                          default_via_drill_nm=nm_from_mm("0.3"), minimum_hole_clearance_nm=nm_from_mm("0.25")),
        decoupling_links=(LINK,),
    )


def _reserved(board):
    stem = TrackSegment("S", Point.mm(13, 9), Point.mm(13, 12), nm_from_mm("0.2"), CopperLayer.FRONT)
    via = Via("S", Point.mm(13, 12), nm_from_mm("0.6"), nm_from_mm("0.3"))
    return (stem,), (via,), _copper_keepouts(board, (stem,), (via,), "escape-reserve", fixed=board)


ACROSS = TrackSegment("P", Point.mm(12, 11), Point.mm(14, 11), nm_from_mm("0.2"), CopperLayer.FRONT)
FAR = TrackSegment("P", Point.mm(3, 3), Point.mm(5, 3), nm_from_mm("0.2"), CopperLayer.FRONT)
RESITED = (TrackSegment("S", Point.mm(13, 9), Point.mm(11, 9), nm_from_mm("0.2"), CopperLayer.FRONT),
           Via("S", Point.mm(11, 9), nm_from_mm("0.6"), nm_from_mm("0.3")))


def _fake_router(monkeypatch, bypass_track):
    """route_decouplers: blocked with the S reservation present, routed without it."""
    calls = []

    def fake(board, *, only_links=None, **_):
        calls.append(only_links)
        blocked = any(k.id.startswith("escape-reserve") for k in board.copper_keepouts)
        if blocked:
            return DecouplingRouteResult(board, (), frozenset(), ((LINK, "bounded surface candidates blocked"),),
                                         accesses=MappingProxyType({}))
        routed = replace(board, tracks=(*board.tracks, bypass_track))
        return DecouplingRouteResult(routed, (bypass_track,), frozenset({LINK.target, LINK.capacitor}), (),
                                     accesses=MappingProxyType({}))
    monkeypatch.setattr(decoupling, "route_decouplers", fake)
    return calls


def _fake_escapes(monkeypatch, pending=()):
    asked = []

    def fake(board, options, nets, *, prefix=((), ()), keepouts=()):
        asked.append((frozenset(nets), prefix, tuple(k.id for k in keepouts)))
        fixed = replace(board, tracks=(*board.tracks, *prefix[0]), vias=(*board.vias, *prefix[1]))
        return (RESITED[:1], RESITED[1:], _copper_keepouts(board, RESITED[:1], RESITED[1:], "escape-reserve",
                                                           fixed=fixed), tuple(pending))
    monkeypatch.setattr(flow, "_route_ordinary_escapes", fake)
    return asked


def _events():
    events = []
    return events, lambda phase, event, details: events.append((phase, event, dict(details)))


def test_the_reservation_the_bypass_copper_crosses_is_swapped_for_a_new_site(monkeypatch) -> None:
    board = _board()
    reserved = _reserved(board)
    calls = _fake_router(monkeypatch, ACROSS)
    asked = _fake_escapes(monkeypatch)
    events, observe = _events()
    held, after = _plan_decouplers(board, reserved, observe, fanout_options=object())
    # One retry for the one pending link, with just that link.
    assert calls == [None, frozenset({LINK})]
    # The bypass copper is accepted, nothing is left released, and S's
    # reservation is the re-sited one planned beside the bypass copper with
    # the other reservations (none here) and halos as keepouts.
    assert held[0] == (ACROSS,) and held[3] == ()
    assert asked == [(frozenset({"S"}), ((ACROSS,), ()), ())]
    assert after[0] == RESITED[:1] and after[1] == RESITED[1:]
    assert [k.id for k in after[2]] == ["escape-reserve-track-0", "escape-reserve-via-0"]
    finished = next(d for p, e, d in events if (p, e) == ("bypass_reservation", "finished"))
    assert finished["pending"] == 0 and finished["released"] == 0
    assert finished["swapped"] == [{"link": "CAP.1->U.1", "balls": ["S_0.1"]}] and finished["rejected"] == []


def test_a_reservation_the_bypass_copper_leaves_legal_stays_bound(monkeypatch) -> None:
    board = _board()
    reserved = _reserved(board)
    _fake_router(monkeypatch, FAR)
    asked = _fake_escapes(monkeypatch)
    events, observe = _events()
    held, after = _plan_decouplers(board, reserved, observe, fanout_options=object())
    # The lifted box was only the candidate set: S keeps its site untouched.
    assert held[0] == (FAR,) and held[3] == () and asked == []
    assert after == reserved
    finished = next(d for p, e, d in events if (p, e) == ("bypass_reservation", "finished"))
    assert finished["pending"] == 0 and finished["swapped"] == [] and finished["rejected"] == []


def test_a_bypass_that_would_leave_a_ball_without_a_site_stays_pending(monkeypatch) -> None:
    board = _board()
    reserved = _reserved(board)
    _fake_router(monkeypatch, ACROSS)
    _fake_escapes(monkeypatch, pending=(PadReference("S_0", "1"),))
    events, observe = _events()
    held, after = _plan_decouplers(board, reserved, observe, fanout_options=object())
    # Nothing changes: no bypass copper, the reservation as it was, the link
    # pending with the ball named.
    assert held[0] == () and held[1] == () and held[3] == ()
    assert after == reserved
    finished = next(d for p, e, d in events if (p, e) == ("bypass_reservation", "finished"))
    assert finished["pending"] == 1 and finished["swapped"] == []
    assert finished["rejected"] == [{"link": "CAP.1->U.1", "balls": ["S_0.1"], "pending": ["S_0.1"]}]
    (item,) = finished["pending_links"]
    assert item == {"link": "CAP.1->U.1",
                    "reason": "bounded surface candidates blocked; swap rejected: S_0.1 would lose their site"}


def test_without_a_fanout_planner_only_the_taken_reservation_is_released(monkeypatch) -> None:
    board = _board()
    reserved = _reserved(board)
    _fake_router(monkeypatch, ACROSS)
    held, after = _plan_decouplers(board, reserved, None)
    # The caller restores or re-sites: the taken group is released, exactly,
    # and the reservation copper no longer holds it.
    assert held[0] == (ACROSS,)
    assert [k.id for k in held[3]] == ["escape-reserve-track-0", "escape-reserve-via-0"]
    assert after == ((), (), ())


def test_the_late_pass_uses_the_stage_launch_options_and_reports_the_links(monkeypatch) -> None:
    from types import SimpleNamespace
    from pcbir import CriticalRoutingStatus
    from pcbir.critical import CriticalRoutingResult
    from pcbir.flow import _complete_critical_stage
    board = _board()
    critical = CriticalRoutingResult(CriticalRoutingStatus.SUCCESS, board, (), (), (), "", "")
    options = SimpleNamespace(direct=False, pre_route_decouplers=True, power_islands=None, bga_fanout=False)
    seen = []

    def fake(board, *, only_links=None, fanout_options=None, **_):
        seen.append((only_links, fanout_options))
        if fanout_options is None:
            return DecouplingRouteResult(board, (), frozenset(), ((LINK, "bounded surface candidates blocked"),),
                                         accesses=MappingProxyType({}))
        routed = replace(board, tracks=(*board.tracks, FAR))
        return DecouplingRouteResult(routed, (FAR,), frozenset({LINK.target, LINK.capacitor}), (),
                                     accesses=MappingProxyType({}))
    monkeypatch.setattr(decoupling, "route_decouplers", fake)
    launch = object()
    for given, connected, pending in ((launch, ["CAP.1->U.1"], []), (None, [], [
            {"link": "CAP.1->U.1", "reason": "bounded surface candidates blocked"}])):
        events, observe = _events()
        completed, _ = _complete_critical_stage(critical, None, None, False, access=None, unified_options=options,
                                                plane_stitch_options=None, on_progress=observe,
                                                fanout_options=given)
        finished = next(d for p, e, d in events if (p, e) == ("bypass_completion", "finished"))
        assert finished["connected"] == connected and finished["pending_links"] == pending
        assert (FAR in completed.board.tracks) == bool(connected)
    # The late pass routed every link with the stage's options, pending ones included.
    assert seen == [(None, launch), (None, None)]


def test_legal_reservation_nets_judge_the_whole_group_beside_the_bypass_copper() -> None:
    board = _board()
    tracks, vias, _ = _reserved(board)
    assert _legal_reservation_nets(board, board, (FAR,), (), tracks, vias, {"S"}) == {"S"}
    assert _legal_reservation_nets(board, board, (ACROSS,), (), tracks, vias, {"S"}) == set()
    # A via left legal with its stem cut is not a legal group.
    stem_only = TrackSegment("P", Point.mm(12, "10.5"), Point.mm(14, "10.5"), nm_from_mm("0.2"), CopperLayer.FRONT)
    assert _legal_reservation_nets(board, board, (stem_only,), (), tracks, vias, {"S"}) == set()

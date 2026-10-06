"""Plan R1 for ordinary nets: wide copper necks down next to fine-pitch lands."""
from __future__ import annotations

from collections import Counter
from dataclasses import replace
from types import MappingProxyType

import pytest

import pcbir.detailed as detail
from pcbir import (BoardOutline, CopperLayer, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard,
                   PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size, TrackSegment, Via,
                   nm_from_mm, route_global, run_physical_drc)
from pcbir.breakout import BreakoutRegions
from pcbir.detailed import DetailedRouterOptions, route_detailed
from pcbir.drc import explicit_copper_connectivity
from pcbir.geometry import RoundedConvexShape, point_on_segment, shapes_clear
from pcbir.physical import DesignRules
from pcbir.pin_escape import RoutingAccess, checked_access_path, verified_routing_access
from pcbir.route_cleanup import prune_track_stubs
from pcbir.route_smoothing import smooth_owned_tracks
from pcbir.routing_clearance import RoutingClearanceIndex

FRONT, BACK = CopperLayer.FRONT, CopperLayer.BACK
WIDE, NARROW = nm_from_mm("0.4"), nm_from_mm("0.2")
U1_3 = PadReference("U1", "3")


def _board(*, breakout: bool = True, width: int = WIDE, rotated: bool = False,
           nets=(("VDD", ("U1.3", "J1.1")),)) -> PhysicalBoard:
    """A five-pin 0.4 mm-pitch row (U1) and a large land (J1).

    The 0.25 mm lands leave 0.15 mm between them, so a 0.4 mm track on U1.3
    comes within 0.075 mm of U1.2 and U1.4; the board clearance is 0.1 mm.
    VDD is 0.4 mm wide with, by default, a 0.2 mm breakout width 1 mm from
    its lands. ``rotated`` adds a second row (U2) turned by 90 degrees.
    """
    row = PhysicalFootprint("test/fine-row", tuple(
        FootprintPad(str(index + 1), Point(nm_from_mm("0.4") * (index - 2), 0), Size.mm("0.25", "0.6"))
        for index in range(5)), Size.mm(3, 2))
    land = PhysicalFootprint("test/land", (FootprintPad("1", Point(0, 0), Size.mm("1.2", "1.2")),),
                             Size.mm(2, 2))
    placements = [Placement("U1", row.name, Point.mm(10, 8)), Placement("J1", land.name, Point.mm(14, 14))]
    if rotated:
        placements = [Placement("U1", row.name, Point.mm(6, 6)), Placement("J1", land.name, Point.mm(15, 6)),
                      Placement("U2", row.name, Point.mm(12, 13), rotation_degrees=90),
                      Placement("J2", land.name, Point.mm(4, 15))]
    profile: dict[str, object] = dict(width_nm=width)
    if breakout:
        profile.update(breakout_length_nm=nm_from_mm(1), breakout_width_nm=NARROW)
    return PhysicalBoard(
        "OrdinaryNeckDown", BoardOutline.rectangle(20, 20), {row.name: row, land.name: land},
        tuple(placements),
        tuple(PhysicalNet(name, tuple(PadReference(*pad.split(".")) for pad in pads)) for name, pads in nets),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2")),
        net_routing_rules=(NetRoutingRule("VDD", RouteKind.GENERAL, **profile),),
    )


def _route(board: PhysicalBoard, **options):
    settings = dict(pitch_nm=nm_from_mm("0.5"), maximum_passes=2) | options
    return route_detailed(board, route_global(board), DetailedRouterOptions(**settings))


def _misnecked(board: PhysicalBoard, tracks) -> list[TrackSegment]:
    """VDD copper whose width disagrees with its region: 0.2 mm inside, 0.4 mm outside."""
    regions = BreakoutRegions(board)
    return [t for t in tracks if t.net == "VDD" and t.width_nm
            != (NARROW if regions.region(t.net, (t.start, t.end)) is not None else WIDE)]


def _hard_errors(board: PhysicalBoard) -> set[str]:
    return {f.code for f in run_physical_drc(board).findings
            if f.severity.value == "error" and f.code not in {"DRC-ROUTE-INCOMPLETE"}}


def _connected(board: PhysicalBoard) -> bool:
    graph = explicit_copper_connectivity(board)
    return all(graph.net_connected(net) for net in board.nets)


def _open_ends(board: PhysicalBoard) -> int:
    """Track ends no other copper continues: not on a centre line, a via or a land."""
    from pcbir.drc import placed_pad_shape
    from pcbir.placement import transformed_local_point
    lands = [placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement)
             for placement in board.placements for pad in board.footprints[placement.footprint].pads]
    count = 0
    for index, track in enumerate(board.tracks):
        for point in (track.start, track.end):
            if not (any(number != index and other.layer is track.layer
                        and point_on_segment(point, other.start, other.end)
                        for number, other in enumerate(board.tracks))
                    or any(via.position == point for via in board.vias)
                    or track.layer is FRONT and any(not shapes_clear(RoundedConvexShape((point,)), land, 1)
                                                    for land in lands)):
                count += 1
    return count


# --- search and emission ---------------------------------------------------

def test_a_wide_ordinary_net_reaches_a_fine_pitch_pin_only_by_necking_down() -> None:
    # At its full 0.4 mm no copper can touch U1.3: there is no pin access.
    plain = _board(breakout=False)
    guide = route_global(plain)
    assert not guide.routes[0].connected
    assert "no legal bounded local/global access for U1.3" in guide.routes[0].diagnostics
    assert not _route(plain).nets[0].connected

    board = _board()
    routed = _route(board)
    assert routed.nets[0].connected, routed.nets[0].diagnostics
    tracks = routed.board.tracks
    assert {t.width_nm for t in tracks} == {NARROW, WIDE}
    assert not _misnecked(board, tracks)
    regions = BreakoutRegions(board)
    assert {regions.region("VDD", (t.start, t.end)) for t in tracks} == {"pad:U1.3:2", "pad:J1.1:0", None}
    report = run_physical_drc(routed.board)
    assert not {f.code for f in report.findings} & {"DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH",
                                                    "DRC-OPEN-NET"}
    necked = [item for item in report.breakout_relaxations if item.check == "track_width"]
    assert necked and all((item.required_nm, item.relaxed_nm, item.measured_nm) == (WIDE, NARROW, NARROW)
                          for item in necked)
    assert {land for item in necked for net, land, _ in item.regions} == {"pad:U1.3:2", "pad:J1.1:0"}


def test_routing_checks_and_emits_each_centreline_as_its_pieces() -> None:
    board = _board()
    index = RoutingClearanceIndex(board)
    start, end = Point.mm(10, 8), Point.mm(10, 11)
    assert not index.can_track("VDD", start, end, WIDE, FRONT)
    assert index.can_route("VDD", start, end, WIDE, FRONT)
    inside, outside = index.route_pieces("VDD", start, end, WIDE, FRONT)
    assert (inside.start, inside.width_nm, outside.end, outside.width_nm) == (start, NARROW, end, WIDE)
    assert inside.end == outside.start and BreakoutRegions(board).region("VDD", (inside.start, inside.end))
    # Pad access paths are the same pieces, with movable conflicts too.
    assert checked_access_path(board, index, "VDD", start, end, WIDE, FRONT) == (inside, outside)
    assert index.route_blockers("VDD", start, end, WIDE, FRONT) == (frozenset(), False)
    # Global pin access offers the necked lead-in.
    accesses = route_global(board).routes[0].accesses
    assert any(t.width_nm == NARROW for access in accesses for t in access.tracks)
    # A net without breakout properties is one track of its width.
    plain = RoutingClearanceIndex(_board(breakout=False))
    assert not plain.breakout.ordinary
    assert plain.route_pieces("VDD", start, end, WIDE, FRONT) == (TrackSegment("VDD", start, end, WIDE, FRONT),)
    assert plain.can_route("VDD", start, end, WIDE, FRONT) is plain.can_track("VDD", start, end, WIDE, FRONT)
    # A critical net with breakout properties keeps the critical router's own cut.
    critical = replace(board, net_routing_rules=tuple(replace(rule, kind=RouteKind.CLOCK)
                                                      for rule in board.net_routing_rules))
    assert BreakoutRegions(critical).declared == {"VDD"} and not BreakoutRegions(critical).ordinary


def test_merging_keeps_necked_copper_inside_one_region() -> None:
    board = _board(nets=(("VDD", ("U1.3", "U1.4", "J1.1")),))
    regions = BreakoutRegions(board)
    p, j, q = Point.mm("9.65", "9.25"), Point.mm("10.2", "9.25"), Point.mm("10.75", "9.25")
    assert regions.region("VDD", (p, j)) == "pad:U1.3:2" and regions.region("VDD", (j, q)) == "pad:U1.4:3"
    assert regions.region("VDD", (p, q)) is None
    first, second = (TrackSegment("VDD", a, b, NARROW, FRONT) for a, b in ((p, j), (j, q)))
    # Joined, the run would be outside every region at the breakout width.
    assert detail._merge_collinear_tracks((first, second)) == (TrackSegment("VDD", p, q, NARROW, FRONT),)
    assert detail._merge_collinear_tracks((first, second), regions) == (first, second)
    near = TrackSegment("VDD", Point.mm("9.8", "9.25"), j, NARROW, FRONT)
    shorter = TrackSegment("VDD", near.start, p, NARROW, FRONT)
    assert detail._merge_collinear_tracks((shorter, near), regions) == (
        TrackSegment("VDD", p, j, NARROW, FRONT),)
    # Re-cut, the run is necked in two overlapping regions; copper outside
    # every region gets its profile width back.
    cut = Point(nm_from_mm("10.384602"), p.y_nm)
    assert regions.neck_down((TrackSegment("VDD", p, q, NARROW, FRONT),)) == (
        TrackSegment("VDD", p, cut, NARROW, FRONT), TrackSegment("VDD", cut, q, NARROW, FRONT))
    away = TrackSegment("VDD", Point.mm("9.65", 11), Point.mm("10.75", 11), NARROW, FRONT)
    assert regions.neck_down((away,)) == (replace(away, width_nm=WIDE),)


# --- cleanup ---------------------------------------------------------------

def test_smoothing_keeps_necked_chains_inside_and_wide_chains_outside() -> None:
    board = _board(rotated=True, nets=(("VDD", ("U1.3", "U2.3", "J1.1")),))
    plain = _route(board, octilinear_search=False)
    smoothed = _route(board, octilinear_search=False, route_smoothing=True)
    assert plain.nets[0].connected and smoothed.nets[0].connected
    assert Counter(smoothed.board.tracks) != Counter(plain.board.tracks)
    assert smoothed.metrics.total_length_nm <= plain.metrics.total_length_nm
    for result in (plain, smoothed):
        assert not _misnecked(board, result.board.tracks)
        assert {t.width_nm for t in result.board.tracks} == {NARROW, WIDE}
        assert not _hard_errors(result.board) and _connected(result.board)


def test_smoothing_a_necked_staircase_checks_and_emits_its_pieces() -> None:
    board = _board(breakout=True)
    regions = BreakoutRegions(board)
    corners = [Point.mm(*xy) for xy in ((10, 8), (10, "8.6"), ("10.4", "8.6"), ("10.4", 11), (14, 11), (14, 14))]
    owned = regions.split_tracks(tuple(TrackSegment("VDD", a, b, WIDE, FRONT)
                                       for a, b in zip(corners, corners[1:])), WIDE)
    assert not _misnecked(board, owned) and not _hard_errors(replace(board, tracks=owned))
    result = smooth_owned_tracks(board, owned, RoutingClearanceIndex(replace(board, tracks=owned)))
    assert Counter(result) != Counter(owned)
    final = replace(board, tracks=result)
    assert not _misnecked(board, result) and {t.width_nm for t in result} == {NARROW, WIDE}
    assert not _hard_errors(final) and _connected(final)
    # The staircase between the pins became a 45-degree chord, still necked.
    assert any(t.width_nm == NARROW and t.start.x_nm != t.end.x_nm and t.start.y_nm != t.end.y_nm
               for t in result)


def test_stub_pruning_joins_neck_down_cuts_on_one_centre_line() -> None:
    board = _board()
    narrow, wide = BreakoutRegions(board).split_tracks(
        (TrackSegment("VDD", Point.mm(10, 8), Point.mm(10, "9.5"), WIDE, FRONT),), WIDE)
    tail = TrackSegment("VDD", Point.mm(10, "9.5"), Point.mm(10, "9.6"), WIDE, FRONT)
    branch = (TrackSegment("VDD", Point.mm(10, "9.5"), Point.mm(14, "13.5"), WIDE, FRONT),
              TrackSegment("VDD", Point.mm(14, "13.5"), Point.mm(14, 14), WIDE, FRONT))
    copper = (narrow, wide, tail, *branch)
    # The necked piece touches the dead-end tail only through their widths.
    assert not shapes_clear(RoundedConvexShape((narrow.start, narrow.end), NARROW // 2),
                            RoundedConvexShape((tail.start, tail.end), WIDE // 2), 1)
    assert prune_track_stubs(board, copper) == (narrow, wide, *branch)
    # Without breakout properties such a contact stays protected, as before.
    assert prune_track_stubs(_board(breakout=False), copper) == copper


def _escaped(*, boxed: bool = False):
    """U1.3 has a reserved escape: a necked stub to a launch via and a back witness.

    ``boxed`` fences the pin field and the stub with front keepouts, so the
    escape is the pin's only exit.
    """
    from pcbir.physical import CopperKeepout, PolygonRing, PolygonWithHoles

    def keepout(name, left, top, right, bottom):
        return CopperKeepout(name, (FRONT,), PolygonWithHoles(PolygonRing((
            Point.mm(left, top), Point.mm(right, top), Point.mm(right, bottom), Point.mm(left, bottom)))),
            block_tracks=True, block_vias=True, block_zones=False)

    board = _board()
    stub = BreakoutRegions(board).split_tracks(
        (TrackSegment("VDD", Point.mm(10, 8), Point.mm(10, "9.6"), WIDE, FRONT),), WIDE)
    witness = TrackSegment("VDD", Point.mm(10, "9.6"), Point.mm(10, 11), WIDE, BACK)
    via = Via("VDD", witness.start, board.rules.default_via_size_nm, board.rules.default_via_drill_nm)
    keepouts = ((keepout("west", "8.6", 6, "9.2", "10.4"), keepout("east", "10.8", 6, "11.4", "10.4"),
                 keepout("north", "8.6", 6, "11.4", "7.0"), keepout("south", "8.6", "10.4", "11.4", 11))
                if boxed else ())
    board = replace(board, tracks=(*stub, witness), vias=(via,), copper_keepouts=keepouts)
    access = RoutingAccess(witness.end, BACK, via.position, (witness,))
    return board, MappingProxyType({U1_3: access}), (*stub, witness), via


@pytest.mark.parametrize("boxed", (False, True))
def test_escape_release_and_cleanup_keep_widths_correct(boxed: bool) -> None:
    board, accesses, created, via = _escaped(boxed=boxed)
    assert created[0].width_nm == NARROW and not _hard_errors(board) - {"DRC-OPEN-NET"}
    # The necked land path verifies as the pin's escape.
    assert verified_routing_access(board, U1_3, "VDD", accesses[U1_3], RoutingClearanceIndex(board))
    routed = route_detailed(
        board, route_global(replace(board, tracks=(), vias=())),
        DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=2, escape_terminals=True,
                              route_smoothing=True),
        fanout_accesses=accesses, fanout_created_tracks=created,
        fanout_created_vias=frozenset({(via.net, via.position)}))
    assert routed.nets[0].connected, routed.nets[0].diagnostics
    final = routed.board
    assert not _misnecked(board, final.tracks)
    assert not _hard_errors(final) and _connected(final)
    # Unused escape copper is released without leaving a tail at the cut.
    assert _open_ends(final) == 0
    if boxed:
        assert via in final.vias and set(created[:2]) <= set(final.tracks)
    else:
        assert not final.vias and not set(created[2:]) & set(final.tracks)


def test_re_cutting_pruned_copper_is_transactional() -> None:
    board = _board()
    breakout = BreakoutRegions(board)
    lead = breakout.split_tracks(
        (TrackSegment("VDD", Point.mm(10, 8), Point.mm(10, 11), WIDE, FRONT),
         TrackSegment("VDD", Point.mm(10, 11), Point.mm(14, 11), WIDE, FRONT)), WIDE)
    # A wide remnant ending at the J1 land centre necks down inside its region.
    centred = (*lead, TrackSegment("VDD", Point.mm(14, 11), Point.mm(14, 14), WIDE, FRONT))
    immutable, mutable = detail._neck_down_changed(board, [], centred, (), breakout)
    assert not immutable and not _misnecked(board, mutable) and len(mutable) == len(centred) + 1
    assert _connected(replace(board, tracks=mutable))
    # One that touches J1 only through its full width keeps it: narrowed, the
    # net would fall apart.
    grazing = (*lead, TrackSegment("VDD", Point.mm(14, 11), Point.mm(14, "13.25"), WIDE, FRONT))
    assert _connected(replace(board, tracks=grazing))
    assert not _connected(replace(board, tracks=breakout.neck_down(grazing)))
    assert detail._neck_down_changed(board, [], grazing, (), breakout) == ([], grazing)
    # Input copper is never re-cut.
    fixed = [TrackSegment("VDD", Point.mm(14, 11), Point.mm(14, 14), WIDE, FRONT)]
    assert detail._neck_down_changed(replace(board, tracks=tuple(fixed)), fixed, (), (), breakout) == (fixed, ())


# --- unchanged elsewhere ---------------------------------------------------

def test_nets_without_breakout_properties_route_exactly_as_before(monkeypatch) -> None:
    board = _board(breakout=False, width=NARROW, rotated=True,
                   nets=(("VDD", ("U1.3", "U2.3", "J1.1")), ("SIG", ("U1.1", "J2.1"))))
    options = dict(octilinear_search=False, route_smoothing=True)
    current = _route(board, **options)
    assert current.nets[0].connected and current.nets[1].connected
    # The pre-R1 calls: whole tracks at the profile width.
    monkeypatch.setattr(RoutingClearanceIndex, "can_route", RoutingClearanceIndex.can_track)
    monkeypatch.setattr(RoutingClearanceIndex, "route_pieces",
                        lambda self, *segment: (TrackSegment(*segment),))
    monkeypatch.setattr(RoutingClearanceIndex, "route_blockers",
                        lambda self, *segment: self.blocking_track_nets(TrackSegment(*segment)))
    monkeypatch.setattr(BreakoutRegions, "neck_down", lambda self, tracks: pytest.fail("re-cut"))
    legacy = _route(board, **options)
    assert legacy.board.tracks == current.board.tracks and legacy.board.vias == current.board.vias
    assert legacy.routing_fingerprint == current.routing_fingerprint


def test_neck_down_routing_is_deterministic() -> None:
    board = _board(rotated=True, nets=(("VDD", ("U1.3", "U2.3", "J1.1")),))
    first, second = (_route(board, route_smoothing=True) for _ in range(2))
    assert first.nets[0].connected
    assert first.board.tracks == second.board.tracks and first.board.vias == second.board.vias
    assert first.routing_fingerprint == second.routing_fingerprint
    assert first.to_json() == second.to_json()

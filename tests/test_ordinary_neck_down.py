"""Plan R1 for ordinary nets: wide copper necks down next to fine-pitch lands."""
from __future__ import annotations

from collections import Counter
from dataclasses import replace


from pcbir import (BoardOutline, CopperLayer, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size, TrackSegment, nm_from_mm, route_global, run_physical_drc)
from pcbir.breakout import BreakoutRegions
from pcbir.unified_router import UnifiedRouterOptions, route_unified
from pcbir.drc import explicit_copper_connectivity
from pcbir.geometry import RoundedConvexShape, shapes_clear
from pcbir.physical import DesignRules
from pcbir.pin_escape import checked_access_path
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
    return route_unified(board, route_global(board), UnifiedRouterOptions(**options))


def _misnecked(board: PhysicalBoard, tracks) -> list[TrackSegment]:
    """VDD copper below its permitted minimum; full width is legal inside too."""
    regions = BreakoutRegions(board)
    return [t for t in tracks if t.net == "VDD" and t.width_nm
            < (NARROW if regions.region(t.net, (t.start, t.end)) is not None else WIDE)]


def _hard_errors(board: PhysicalBoard) -> set[str]:
    return {f.code for f in run_physical_drc(board).findings
            if f.severity.value == "error" and f.code not in {"DRC-ROUTE-INCOMPLETE"}}


def _connected(board: PhysicalBoard) -> bool:
    graph = explicit_copper_connectivity(board)
    return all(graph.net_connected(net) for net in board.nets)


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
    # The large J1 land no longer gets a gratuitous neckdown.
    assert {land for item in necked for net, land, _ in item.regions} == {"pad:U1.3:2"}


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


# --- cleanup ---------------------------------------------------------------


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
    # The staircase becomes a checked chord. Its open-space pieces may now
    # use the full width; only genuinely obstructed pin access stays necked.
    assert any(t.start.x_nm != t.end.x_nm and t.start.y_nm != t.end.y_nm for t in result)
    index = RoutingClearanceIndex(final)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in result)


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


# --- unchanged elsewhere ---------------------------------------------------


def test_neck_down_routing_is_deterministic() -> None:
    board = _board(rotated=True, nets=(("VDD", ("U1.3", "U2.3", "J1.1")),))
    first, second = (_route(board, route_smoothing=True) for _ in range(2))
    assert first.nets[0].connected
    assert first.board.tracks == second.board.tracks and first.board.vias == second.board.vias
    assert first.routing_fingerprint == second.routing_fingerprint
    assert first.to_json() == second.to_json()

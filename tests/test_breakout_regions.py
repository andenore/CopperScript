"""D-PHY plan R1: breakout width, gap and clearance apply only near terminal lands."""
from __future__ import annotations

from dataclasses import replace
from math import sqrt

from pcbir import (
    BoardOutline, CopperLayer, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size, TrackSegment, Via,
    nm_from_mm, route_critical_nets, route_global, run_physical_drc,
)
from pcbir.breakout import BreakoutRegions
from pcbir.critical import _route_pair, _route_single, _validate_candidate
from pcbir.geometry import RoundedConvexShape, shape_distance_squared
from pcbir.pair_search import _legal, _tracks
from pcbir.physical import DesignRules, PadShape
from pcbir.routing_clearance import RoutingClearanceIndex

FRONT = CopperLayer.FRONT
ROWS = ("-1.25", "-0.75", "-0.25", "0.25", "0.75", "1.25")
# A test point beside the straight channel, 0.3 mm from the P lane's edge:
# closer than the 0.5 mm channel clearance, farther than the 0.15 mm
# breakout clearance.
TEST_POINT = Point.mm(15, "9.175")
REGION = ("1.0", "0.15")  # breakout length and breakout clearance, in mm


def _row_board(clearance: str = "0.5", *, breakout: tuple[str, str] | None = REGION,
               neck: bool = False, obstacle: bool = True) -> PhysicalBoard:
    """A 0.5 mm-pitch land row (U1) joined to an equal row (J1) by one pair.

    Pads 3 and 4 of each row carry LANE_P/LANE_N; the other lands are
    unconnected neighbours 0.25 mm away. The pair profile is 0.15 mm width,
    0.25 mm gap and ``clearance``; ``breakout`` adds a breakout length and
    clearance, ``neck`` a 0.12 mm breakout width and 0.2 mm breakout gap.
    """
    package = PhysicalFootprint("test/fine-row", tuple(
        FootprintPad(str(index + 1), Point.mm("1.4", y), Size.mm("0.6", "0.25"))
        for index, y in enumerate(ROWS)), Size.mm(3, 4))
    connector = PhysicalFootprint("test/fine-connector", tuple(
        FootprintPad(str(index + 1), Point.mm("-1.4", y), Size.mm("0.6", "0.25"))
        for index, y in enumerate(ROWS)), Size.mm(3, 4))
    point = PhysicalFootprint("test/test-point", (
        FootprintPad("1", Point(0, 0), Size.mm("0.5", "0.5"), shape=PadShape.CIRCLE),), Size.mm(1, 1))
    profile: dict[str, object] = dict(priority=100, width_nm=nm_from_mm("0.15"),
                                      pair_gap_nm=nm_from_mm("0.25"), clearance_nm=nm_from_mm(clearance))
    if breakout is not None:
        profile.update(breakout_length_nm=nm_from_mm(breakout[0]),
                       breakout_clearance_nm=nm_from_mm(breakout[1]))
    if neck:
        profile.update(breakout_width_nm=nm_from_mm("0.12"), breakout_gap_nm=nm_from_mm("0.2"))
    placements = [Placement("U1", package.name, Point.mm(8, 10)),
                  Placement("J1", connector.name, Point.mm(22, 10))]
    if obstacle:
        placements.append(Placement("TP1", point.name, TEST_POINT))
    return PhysicalBoard(
        "BreakoutRow", BoardOutline.rectangle(30, 20),
        {package.name: package, connector.name: connector, point.name: point}, tuple(placements),
        (PhysicalNet("LANE_P", (PadReference("U1", "3"), PadReference("J1", "3"))),
         PhysicalNet("LANE_N", (PadReference("U1", "4"), PadReference("J1", "4"))),
         PhysicalNet("AUX", ())),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.15")),
        net_routing_rules=(
            NetRoutingRule("LANE_P", RouteKind.DIFFERENTIAL, differential_partner="LANE_N", **profile),
            NetRoutingRule("LANE_N", RouteKind.DIFFERENTIAL, differential_partner="LANE_P", **profile),
        ),
    )


def _route(board: PhysicalBoard):
    return route_critical_nets(board, route_global(board), pair_state_limit=20_000)


def _test_point_spacing(tracks) -> float:
    pad = RoundedConvexShape((TEST_POINT,), nm_from_mm("0.25"))
    return min(sqrt(shape_distance_squared(RoundedConvexShape((t.start, t.end)), pad))
               - t.width_nm / 2 - pad.radius_nm for t in tracks)


def _codes(report, *nets: str) -> set[str]:
    return {f.code for f in report.findings if not nets or set(f.nets) & set(nets)}


def _with_copper(board: PhysicalBoard, tracks=(), vias=()) -> PhysicalBoard:
    return replace(board, tracks=tuple(tracks), vias=tuple(vias))


# --- the region ------------------------------------------------------------

def test_the_region_is_each_terminal_land_swept_by_the_breakout_length() -> None:
    regions = BreakoutRegions(_row_board())
    assert regions and regions.declared == {"LANE_P", "LANE_N"}
    # U1.3 spans x 9.1..9.7 mm; 1 mm beyond its edge is the region boundary.
    assert regions.region("LANE_P", (Point.mm("9.4", "9.75"), Point.mm("10.7", "9.75"))) == "pad:U1.3:2"
    assert regions.region("LANE_P", (Point(nm_from_mm("10.7") + 1, nm_from_mm("9.75")),)) is None
    # Plan-view distance from the land outline, not from its centre.
    assert regions.region("LANE_P", (Point.mm("9.4", "10.85"),)) == "pad:U1.3:2"
    assert regions.region("LANE_P", (Point.mm("9.4", "10.9"),)) is None
    assert regions.region("AUX", (Point.mm("9.4", "9.75"),)) is None
    # A crossing segment is cut at the last integer step inside the region.
    pieces = regions.split("LANE_P", Point.mm("9.4", "9.75"), Point.mm(15, "9.75"))
    assert pieces == ((Point.mm("9.4", "9.75"), Point.mm("10.7", "9.75"), "pad:U1.3:2"),
                      (Point.mm("10.7", "9.75"), Point.mm(15, "9.75"), None))
    assert regions.split("LANE_P", *pieces[0][:2]) == (pieces[0],)
    assert regions.split("LANE_P", *pieces[1][:2]) == (pieces[1],)
    # Both regions of the net: three pieces; a 45-degree segment is cut too.
    across = regions.split("LANE_P", Point.mm("9.4", "9.75"), Point.mm("20.6", "9.75"))
    assert [(a.x_nm, b.x_nm, land) for a, b, land in across] == [
        (nm_from_mm("9.4"), nm_from_mm("10.7"), "pad:U1.3:2"),
        (nm_from_mm("10.7"), nm_from_mm("19.3"), None),
        (nm_from_mm("19.3"), nm_from_mm("20.6"), "pad:J1.3:2")]
    diagonal = regions.split("LANE_P", Point.mm("9.7", "9.75"), Point.mm("11.7", "11.75"))
    assert len(diagonal) == 2 and diagonal[0][2] == "pad:U1.3:2" and diagonal[1][2] is None
    # Any other direction is not cut; it is outside unless wholly inside.
    oblique = regions.split("LANE_P", Point.mm("9.7", "9.75"), Point.mm("11.7", "10.75"))
    assert oblique == ((Point.mm("9.7", "9.75"), Point.mm("11.7", "10.75"), None),)
    assert not BreakoutRegions(_row_board(breakout=None))


def test_the_routers_split_tracks_and_carry_the_breakout_width_inside() -> None:
    regions = BreakoutRegions(_row_board(neck=True))
    lane = TrackSegment("LANE_P", Point.mm("9.4", "9.75"), Point.mm(15, "9.75"), nm_from_mm("0.15"), FRONT)
    inside, outside = regions.split_tracks((lane,), nm_from_mm("0.15"))
    assert (inside.end, inside.width_nm) == (Point.mm("10.7", "9.75"), nm_from_mm("0.12"))
    assert (outside.start, outside.width_nm) == (Point.mm("10.7", "9.75"), nm_from_mm("0.15"))
    # Idempotent, and a necked piece moved outside gets the profile width back.
    assert regions.split_tracks((inside, outside), nm_from_mm("0.15")) == (inside, outside)
    assert regions.split_tracks((replace(outside, width_nm=nm_from_mm("0.12")),),
                                nm_from_mm("0.15")) == (outside,)
    foreign = replace(lane, net="AUX")
    assert regions.split_tracks((foreign,)) == (foreign,)


# --- routing ---------------------------------------------------------------

def test_a_pair_escapes_the_pin_field_only_with_breakout_values() -> None:
    routed = _route(_row_board())
    pair = routed.nets[0]
    assert pair.connected, pair.diagnostics
    assert pair.strategy.startswith("joint_pair")
    # The pin field passes at the breakout clearance; the channel keeps the
    # full 0.5 mm from the test point by detouring around it.
    assert _test_point_spacing(routed.locked_tracks) >= nm_from_mm("0.5")
    report = run_physical_drc(routed.board)
    assert not _codes(report) & {"DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH", "DRC-OPEN-NET"}
    assert {(net, land) for item in report.breakout_relaxations
            for net, land, _ in item.regions} == {
        ("LANE_P", "pad:U1.3:2"), ("LANE_P", "pad:J1.3:2"),
        ("LANE_N", "pad:U1.4:3"), ("LANE_N", "pad:J1.4:3")}
    # Clearance 0.5 mm everywhere: the pair cannot leave the pin field.
    assert not _route(_row_board(breakout=None)).nets[0].connected
    # A region too short to clear the neighbouring lands does not help either.
    assert not _route(_row_board(breakout=("0.2", "0.15"))).nets[0].connected
    # Clearance 0.15 mm everywhere routes straight past the test point,
    # 0.3 mm away, violating the intended channel clearance.
    loose = _route(_row_board("0.15", breakout=None))
    assert loose.nets[0].connected and loose.nets[0].strategy == "aligned_pair"
    assert round(_test_point_spacing(loose.locked_tracks)) == nm_from_mm("0.3")


def test_the_routed_pair_necks_down_only_inside_the_region() -> None:
    board = _row_board(neck=True)
    routed = _route(board)
    assert routed.nets[0].connected, routed.nets[0].diagnostics
    regions = BreakoutRegions(board)
    widths = {(regions.region(t.net, (t.start, t.end)) is not None, t.width_nm) for t in routed.locked_tracks}
    assert widths == {(True, nm_from_mm("0.12")), (False, nm_from_mm("0.15"))}
    report = run_physical_drc(routed.board)
    assert not _codes(report) & {"DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH", "DRC-OPEN-NET"}
    necked = [item for item in report.breakout_relaxations if item.check == "track_width"]
    assert necked and all((item.required_nm, item.relaxed_nm, item.measured_nm)
                          == (nm_from_mm("0.15"), nm_from_mm("0.12"), nm_from_mm("0.12")) for item in necked)


def test_a_single_ended_guide_candidate_is_cut_at_the_region_boundary() -> None:
    board = _row_board(obstacle=False)
    clock = NetRoutingRule("LANE_P", RouteKind.CLOCK, priority=100, width_nm=nm_from_mm("0.15"),
                           clearance_nm=nm_from_mm("0.5"), breakout_length_nm=nm_from_mm("1.0"),
                           breakout_width_nm=nm_from_mm("0.12"), breakout_clearance_nm=nm_from_mm("0.15"))
    board = replace(board, net_routing_rules=(clock,))
    guide = next(route for route in route_global(board).routes if route.net == "LANE_P")
    result, tracks, _ = _route_single(board, clock, guide)
    regions = BreakoutRegions(board)
    assert result.connected and {regions.region("LANE_P", (t.start, t.end)) for t in tracks} == {
        "pad:U1.3:2", "pad:J1.3:2", None}
    assert all(t.width_nm == (nm_from_mm("0.15") if regions.region("LANE_P", (t.start, t.end)) is None
                              else nm_from_mm("0.12")) for t in tracks)
    report = run_physical_drc(_with_copper(board, tracks))
    assert not _codes(report, "LANE_P") & {"DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH"}


def test_pair_search_spacing_is_region_aware() -> None:
    board = _row_board(neck=True, obstacle=False)
    index = RoutingClearanceIndex(board)
    width, layer = nm_from_mm("0.15"), FRONT

    def lanes(x0: str, x1: str, half_pitch: str):
        spine = (Point.mm(x0, 10), Point.mm(x1, 10))
        offset = nm_from_mm(half_pitch)
        first = _tracks("LANE_P", tuple(Point(p.x_nm, p.y_nm - offset) for p in spine),
                        width, layer, index.breakout)
        second = _tracks("LANE_N", tuple(Point(p.x_nm, p.y_nm + offset) for p in spine),
                         width, layer, index.breakout)
        return first, second

    # Lanes 0.35 mm apart centre to centre. Next to the lands they neck to
    # 0.12 mm; their 0.23 mm gap needs the 0.2 mm breakout gap...
    near = lanes("10.0", "10.6", "0.175")
    assert all(t.width_nm == nm_from_mm("0.12") for t in (*near[0], *near[1]))
    assert _legal(board, index, *near, board.rules.minimum_clearance_nm)
    # ...but in the channel, at 0.15 mm, their 0.2 mm gap is below the pair gap.
    assert not _legal(board, index, *lanes("12.0", "12.6", "0.175"), board.rules.minimum_clearance_nm)
    assert _legal(board, index, *lanes("12.0", "12.6", "0.2"), board.rules.minimum_clearance_nm)
    # A foreign track 0.2 mm from a lane: legal only inside the region.
    assert index.can_track("LANE_P", Point.mm("10.0", "9.5"), Point.mm("10.6", "9.5"), width, layer)
    index.add_track(TrackSegment("AUX", Point.mm("10.0", "9.15"), Point.mm("13.0", "9.15"), width, layer))
    assert index.can_track("LANE_P", Point.mm("10.0", "9.5"), Point.mm("10.6", "9.5"), width, layer)
    assert not index.can_track("LANE_P", Point.mm("11.0", "9.5"), Point.mm("11.6", "9.5"), width, layer)


# --- physical DRC ----------------------------------------------------------

def _lane_and_via(end_x_nm: int, *, split: bool = False) -> tuple[list[TrackSegment], list[Via]]:
    """A LANE_P track from its U1 land with an AUX via 0.2 mm from its edge."""
    start, boundary = Point.mm("9.4", "9.75"), Point.mm("10.7", "9.75")
    end = Point(end_x_nm, boundary.y_nm)
    width = nm_from_mm("0.15")
    tracks = ([TrackSegment("LANE_P", start, boundary, width, FRONT),
               TrackSegment("LANE_P", boundary, end, width, FRONT)] if split
              else [TrackSegment("LANE_P", start, end, width, FRONT)])
    via = Via("AUX", Point.mm("10.1", "9.325"), nm_from_mm("0.3"), nm_from_mm("0.15"), FRONT, CopperLayer.BACK)
    return [t for t in tracks if t.start != t.end], [via]


def test_drc_relaxes_clearance_only_inside_the_region_and_records_why() -> None:
    board = _row_board(obstacle=False)
    edge = nm_from_mm("10.7")
    inside = run_physical_drc(_with_copper(board, *_lane_and_via(edge)))
    assert not [f for f in inside.findings if f.code in {"DRC-CLEARANCE", "DRC-SHORT"}]
    (relaxation,) = [item for item in inside.breakout_relaxations if item.objects == ("track:0", "via:0")]
    assert (relaxation.check, relaxation.nets, relaxation.layers) == ("clearance", ("AUX", "LANE_P"), ("F.Cu",))
    assert (relaxation.required_nm, relaxation.relaxed_nm, relaxation.measured_nm) == (
        nm_from_mm("0.5"), nm_from_mm("0.15"), nm_from_mm("0.2"))
    assert relaxation.regions == (("LANE_P", "pad:U1.3:2", nm_from_mm("1.0")),)
    document = inside.to_json()
    assert '"breakout_relaxations"' in document and '"land": "pad:U1.3:2"' in document
    assert any(item.check == "breakout_regions" for item in inside.coverage)
    # The same copper reaching 1 nm past the boundary is outside the region.
    outside = run_physical_drc(_with_copper(board, *_lane_and_via(edge + 1)))
    (finding,) = [f for f in outside.findings if f.objects == ("track:0", "via:0")]
    assert finding.code == "DRC-CLEARANCE" and finding.nets == ("AUX", "LANE_P")
    # The lands next to it now need the full 0.5 mm as well.
    assert {f.objects for f in outside.findings if f.code == "DRC-CLEARANCE"} >= {("track:0", "pad:U1.2:1")}
    # Cut at the boundary, the part next to the land is inside again.
    split = run_physical_drc(_with_copper(board, *_lane_and_via(edge + 1, split=True)))
    assert not [f for f in split.findings if f.code in {"DRC-CLEARANCE", "DRC-SHORT"}]


def test_drc_accepts_a_neck_down_inside_the_region_and_rejects_it_outside() -> None:
    board = _row_board(neck=True, obstacle=False)
    narrow, width = nm_from_mm("0.12"), nm_from_mm("0.15")
    neck = TrackSegment("LANE_P", Point.mm("9.4", "9.75"), Point.mm("10.2", "9.75"), narrow, FRONT)
    report = run_physical_drc(_with_copper(board, (neck,)))
    assert "DRC-TRACK-WIDTH" not in _codes(report)
    assert [(item.check, item.required_nm, item.relaxed_nm, item.regions) for item in report.breakout_relaxations
            if item.check == "track_width"] == [
        ("track_width", width, narrow, (("LANE_P", "pad:U1.3:2", nm_from_mm("1.0")),))]
    long_neck = replace(neck, end=Point.mm("10.8", "9.75"))
    (finding,) = [f for f in run_physical_drc(_with_copper(board, (long_neck,))).findings
                  if f.code == "DRC-TRACK-WIDTH"]
    assert (finding.required_nm, finding.measured_nm) == (width, narrow)

    # Pair members 0.2 mm apart: the breakout gap inside, the pair gap outside.
    def pair(x0: str, x1: str) -> tuple[TrackSegment, TrackSegment]:
        return (TrackSegment("LANE_P", Point.mm(x0, "9.825"), Point.mm(x1, "9.825"), width, FRONT),
                TrackSegment("LANE_N", Point.mm(x0, "10.175"), Point.mm(x1, "10.175"), width, FRONT))

    near = run_physical_drc(_with_copper(board, pair("10.0", "10.6")))
    assert not _codes(near) & {"DRC-CLEARANCE", "DRC-SHORT"}
    (gap,) = [item for item in near.breakout_relaxations if item.objects == ("track:0", "track:1")]
    assert (gap.check, gap.required_nm, gap.relaxed_nm, gap.measured_nm) == (
        "pair_gap", nm_from_mm("0.25"), nm_from_mm("0.2"), nm_from_mm("0.2"))
    assert gap.regions == (("LANE_N", "pad:U1.4:3", nm_from_mm("1.0")),
                           ("LANE_P", "pad:U1.3:2", nm_from_mm("1.0")))
    far = run_physical_drc(_with_copper(board, pair("12.0", "12.6")))
    (finding,) = [f for f in far.findings if f.objects == ("track:0", "track:1")]
    assert finding.code == "DRC-CLEARANCE" and "pair gap" in finding.message
    assert finding.required_nm == nm_from_mm("0.4")  # centre distance: width + pair gap
    # The coupled pair gap itself is legal in the channel, below the clearance.
    coupled = run_physical_drc(_with_copper(board, (
        TrackSegment("LANE_P", Point.mm(12, "9.8"), Point.mm("12.6", "9.8"), width, FRONT),
        TrackSegment("LANE_N", Point.mm(12, "10.2"), Point.mm("12.6", "10.2"), width, FRONT))))
    assert not _codes(coupled) & {"DRC-CLEARANCE", "DRC-SHORT"}


def test_critical_validation_applies_the_region_rules() -> None:
    board = _row_board(obstacle=False)
    routes = {route.net: route for route in route_global(board).routes}
    proposed, tracks, vias = _route_pair(board, *board.net_routing_rules, routes)
    assert proposed.strategy == "aligned_pair" and proposed.connected
    accepted, kept, _ = _validate_candidate(board, proposed, tracks, vias, [], [])
    assert accepted.connected and kept == tracks
    # The same copper is illegal when the clearance applies in the pin field...
    plain = _row_board(obstacle=False, breakout=None)
    rejected, kept, _ = _validate_candidate(plain, proposed, tracks, vias, [], [])
    assert not rejected.connected and kept == ()
    assert any(message.startswith("DRC-CLEARANCE") for message in rejected.diagnostics)
    # ...and in the channel, where it passes the test point 0.3 mm away.
    obstructed = _row_board()
    rejected, _, _ = _validate_candidate(obstructed, proposed, tracks, vias, [], [])
    assert not rejected.connected
    assert any("TP1.1" in message for message in rejected.diagnostics)


# --- regression and determinism ---------------------------------------------

def test_nets_without_breakout_properties_are_unaffected() -> None:
    plain = _row_board("0.15", breakout=None)
    report = run_physical_drc(_route(plain).board)
    assert report.breakout_relaxations == ()
    assert "breakout" not in report.to_json()
    assert not any(item.check == "breakout_regions" for item in report.coverage)
    # A pair without breakout properties keeps the plain clearance rule
    # between its members, beside a pair that has them.
    width = nm_from_mm("0.15")
    other = (NetRoutingRule("AUX_P", RouteKind.DIFFERENTIAL, differential_partner="AUX_N", width_nm=width,
                            pair_gap_nm=nm_from_mm("0.25"), clearance_nm=nm_from_mm("0.5")),
             NetRoutingRule("AUX_N", RouteKind.DIFFERENTIAL, differential_partner="AUX_P", width_nm=width,
                            pair_gap_nm=nm_from_mm("0.25"), clearance_nm=nm_from_mm("0.5")))
    copper = (TrackSegment("AUX_P", Point.mm(12, 3), Point.mm(14, 3), width, FRONT),
              TrackSegment("AUX_N", Point.mm(12, "3.4"), Point.mm(14, "3.4"), width, FRONT),
              TrackSegment("LANE_P", Point.mm("9.4", "9.75"), Point.mm("10.2", "9.75"), width, FRONT))

    def aux_findings(board: PhysicalBoard) -> list[str]:
        board = replace(board, nets=(*board.nets, PhysicalNet("AUX_P", ()), PhysicalNet("AUX_N", ())),
                        net_routing_rules=(*board.net_routing_rules, *other), tracks=copper)
        return [f.fingerprint for f in run_physical_drc(board).findings
                if set(f.nets) & {"AUX_P", "AUX_N"} and f.code in {"DRC-CLEARANCE", "DRC-SHORT"}]

    with_breakout = aux_findings(_row_board(obstacle=False))
    assert with_breakout == aux_findings(_row_board(obstacle=False, breakout=None))
    # The 0.25 mm members still violate their own 0.5 mm clearance.
    assert len(with_breakout) == 1
    assert not RoutingClearanceIndex(plain).breakout


def test_breakout_routing_and_drc_are_deterministic() -> None:
    board = _row_board(neck=True)
    first, second = _route(board), _route(board)
    assert first == second
    assert first.routing_fingerprint == second.routing_fingerprint
    reports = run_physical_drc(first.board), run_physical_drc(second.board)
    assert reports[0].to_json() == reports[1].to_json()
    assert reports[0].breakout_relaxations == reports[1].breakout_relaxations

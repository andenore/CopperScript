"""D-PHY plan R2: aligned pair channels with mismatched-pitch 45-degree tapers."""
from __future__ import annotations

from decimal import Decimal
from math import sqrt

from pcbir import (
    BoardOutline, FootprintPad, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, RouteKind, Size, nm_from_mm, route_critical_nets, route_global,
    run_physical_drc,
)
from pcbir.critical import _aligned_pair_paths, _route_pair
from pcbir.physical import DesignRules

CENTRE_Y = nm_from_mm(10)


def _taper_board(chip_pitch: str = "0.5", connector_pitch: str = "0.4", *,
                 connector_shift: str = "0", uncoupled_limit: str | None = None) -> PhysicalBoard:
    """A package pair to a connector pair across a straight 0.4 mm-pitch channel.

    The pair profile is 0.2 mm width + 0.2 mm gap. The package lands are
    ``chip_pitch`` apart, the connector lands ``connector_pitch`` apart, and
    both pairs share one midpoint unless the connector is shifted.
    """
    chip_half, connector_half = Decimal(chip_pitch) / 2, Decimal(connector_pitch) / 2
    shift = Decimal(connector_shift)
    chip = PhysicalFootprint("test/package-pair", (
        FootprintPad("1", Point.mm("1.4", -chip_half), Size.mm("0.6", "0.25")),
        FootprintPad("2", Point.mm("1.4", chip_half), Size.mm("0.6", "0.25")),
    ), Size.mm(3, 3))
    connector = PhysicalFootprint("test/connector-pair", (
        FootprintPad("1", Point.mm("-1.4", shift - connector_half), Size.mm("0.6", "0.2")),
        FootprintPad("2", Point.mm("-1.4", shift + connector_half), Size.mm("0.6", "0.2")),
    ), Size.mm(3, 3))
    profile = dict(priority=100, width_nm=nm_from_mm("0.2"), pair_gap_nm=nm_from_mm("0.2"),
                   maximum_uncoupled_length_nm=(nm_from_mm(uncoupled_limit)
                                                if uncoupled_limit is not None else None))
    return PhysicalBoard(
        "PairTaper", BoardOutline.rectangle(30, 20),
        {chip.name: chip, connector.name: connector},
        (Placement("U1", chip.name, Point.mm(8, 10)), Placement("J1", connector.name, Point.mm(22, 10))),
        (PhysicalNet("LANE_P", (PadReference("U1", "1"), PadReference("J1", "1"))),
         PhysicalNet("LANE_N", (PadReference("U1", "2"), PadReference("J1", "2")))),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"),
                          minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2")),
        net_routing_rules=(
            NetRoutingRule("LANE_P", RouteKind.DIFFERENTIAL, differential_partner="LANE_N", **profile),
            NetRoutingRule("LANE_N", RouteKind.DIFFERENTIAL, differential_partner="LANE_P", **profile),
        ),
    )


def _mirror(point: Point) -> Point:
    return Point(point.x_nm, 2 * CENTRE_Y - point.y_nm)


def _segments(tracks, net: str) -> list[tuple[Point, Point]]:
    return sorted(((t.start, t.end) for t in tracks if t.net == net),
                  key=lambda item: (item[0].x_nm, item[1].x_nm))


def test_mismatched_pitch_lands_use_the_aligned_channel_with_symmetric_tapers() -> None:
    board = _taper_board("0.5", "0.4")
    result = route_critical_nets(board, route_global(board))
    pair = result.nets[0]
    assert pair.connected, pair.diagnostics
    assert pair.strategy == "aligned_pair"
    taper = round(nm_from_mm("0.05") * sqrt(2))
    # Each 0.5 mm land tapers 0.05 mm inward at 45 degrees onto its lane; the
    # 0.4 mm connector lands already sit on the lanes and need no taper.
    assert _segments(result.locked_tracks, "LANE_P") == [
        (Point.mm("9.4", "9.75"), Point.mm("9.45", "9.8")),
        (Point.mm("9.45", "9.8"), Point.mm("20.6", "9.8")),
    ]
    assert _segments(result.locked_tracks, "LANE_N") == [
        (_mirror(start), _mirror(end)) for start, end in _segments(result.locked_tracks, "LANE_P")]
    # Taper length is uncoupled length, reported per member.
    assert pair.uncoupled_lengths_nm == (taper, taper)
    assert pair.lengths_nm[0] - pair.coupled_length_nm == taper
    assert pair.skew_nm == 0
    assert not {f.code for f in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
    assert route_critical_nets(board, route_global(board)) == result


def test_tapers_at_both_ends_go_inward_and_outward_symmetrically() -> None:
    board = _taper_board("0.5", "0.3")
    result = route_critical_nets(board, route_global(board))
    pair = result.nets[0]
    assert pair.connected and pair.strategy == "aligned_pair", pair.diagnostics
    p_tracks = _segments(result.locked_tracks, "LANE_P")
    # Inward from the 0.5 mm package lands, outward to the 0.3 mm connector lands.
    assert p_tracks == [
        (Point.mm("9.4", "9.75"), Point.mm("9.45", "9.8")),
        (Point.mm("9.45", "9.8"), Point.mm("20.55", "9.8")),
        (Point.mm("20.55", "9.8"), Point.mm("20.6", "9.85")),
    ]
    assert _segments(result.locked_tracks, "LANE_N") == [
        (_mirror(start), _mirror(end)) for start, end in p_tracks]
    taper = round(nm_from_mm("0.05") * sqrt(2))
    assert pair.uncoupled_lengths_nm == (2 * taper, 2 * taper)


def test_taper_uncoupled_length_is_held_to_the_declared_limit() -> None:
    taper = round(nm_from_mm("0.05") * sqrt(2))
    tight = _taper_board("0.5", "0.4", uncoupled_limit="0.05")
    routes = {route.net: route for route in route_global(tight).routes}
    rejected, _, _ = _route_pair(tight, *tight.net_routing_rules, routes)
    assert rejected.strategy == "aligned_pair" and not rejected.connected
    assert rejected.uncoupled_lengths_nm == (taper, taper)
    assert rejected.diagnostics == (f"pair uncoupled length {taper} nm exceeds {nm_from_mm('0.05')} nm",)
    loose = _taper_board("0.5", "0.4", uncoupled_limit="0.08")
    routes = {route.net: route for route in route_global(loose).routes}
    accepted, _, _ = _route_pair(loose, *loose.net_routing_rules, routes)
    assert accepted.strategy == "aligned_pair" and accepted.connected


def test_the_channel_requires_one_midpoint_and_the_same_member_order() -> None:
    offset = nm_from_mm("0.2")
    aligned = _taper_board("0.5", "0.4")
    routes = {route.net: route for route in route_global(aligned).routes}
    assert _aligned_pair_paths(routes["LANE_N"], routes["LANE_P"], offset) is not None
    # A connector pair whose midpoint is 0.1 mm off the package midpoint needs
    # a jog, not a straight channel; the exact searches handle it instead.
    shifted = _taper_board("0.5", "0.4", connector_shift="0.1")
    routes = {route.net: route for route in route_global(shifted).routes}
    assert _aligned_pair_paths(routes["LANE_N"], routes["LANE_P"], offset) is None
    # Swapped members at one end would need a crossing.
    swapped = _taper_board("0.5", "-0.4")
    routes = {route.net: route for route in route_global(swapped).routes}
    assert _aligned_pair_paths(routes["LANE_N"], routes["LANE_P"], offset) is None


def test_equal_pitch_channels_keep_their_tapers() -> None:
    board = _taper_board("1.0", "1.0")
    routes = {route.net: route for route in route_global(board).routes}
    first, second = _aligned_pair_paths(routes["LANE_N"], routes["LANE_P"], nm_from_mm("0.2"))
    # Equal 1.0 mm pitch at both ends: identical 0.3 mm tapers, as before R2.
    assert [(start, end) for _, start, end in first] == [
        (Point.mm("9.4", "10.5"), Point.mm("9.7", "10.2")),
        (Point.mm("9.7", "10.2"), Point.mm("20.3", "10.2")),
        (Point.mm("20.3", "10.2"), Point.mm("20.6", "10.5")),
    ]
    assert [(_mirror(start), _mirror(end)) for _, start, end in second] == [
        (start, end) for _, start, end in first]

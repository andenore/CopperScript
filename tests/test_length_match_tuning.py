"""D-PHY plan R3: length-match tuning of accepted critical groups."""
from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path

import pytest

from pcbir import (
    BoardOutline, CopperKeepout, CopperLayer, CriticalRoutingStatus, FootprintPad, NetMatchGroup,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    PolygonRing, PolygonWithHoles, RouteKind, Size, TrackSegment, nm_from_mm, route_critical_nets,
    route_global, run_physical_drc, verify_match_groups,
)
from pcbir.critical import _track_length, _validate_candidate, critical_lane_table
from pcbir.critical_tuning import UnitTuner
from pcbir.geometry import segment_distance_squared
from pcbir.physical import DesignRules
from pcbir.routing_clearance import RoutingClearanceIndex

WIDTH, GAP = nm_from_mm("0.2"), nm_from_mm("0.2")
SPACING = WIDTH + GAP
AMPLITUDE = nm_from_mm("1")
PAIR_SKEW = nm_from_mm("0.05")
# Lanes are (name, y, x of the package lands, x of the connector lands) in mm.
# LANE_A is 28 mm long, LANE_B 20 mm and LANE_C 24 mm.
THREE_LANES = (("LANE_A", 6, 5, 33), ("LANE_B", 14, 5, 25), ("LANE_C", 22, 5, 29))
TWO_LANES = (("LANE_A", 6, 5, 33), ("LANE_B", 14, 5, 29))


def _keepout(name: str, x0, y0, x1, y1) -> CopperKeepout:
    return CopperKeepout(name, (CopperLayer.FRONT, CopperLayer.BACK), PolygonWithHoles(PolygonRing((
        Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1)))))


def _lanes_board(lanes=THREE_LANES, *, groups=None, max_skew="0.1", amplitude=AMPLITUDE,
                 uncoupled=None, keepouts=()) -> PhysicalBoard:
    """One straight 0.2/0.2 mm differential pair per lane, each between its own two parts.

    By default all lanes form the length-match group ``lanes``. ``groups``
    maps group ids to lane names instead.
    """
    lands = PhysicalFootprint("test/pair-lands", (
        FootprintPad("1", Point.mm(0, "-0.2"), Size.mm("0.6", "0.2")),
        FootprintPad("2", Point.mm(0, "0.2"), Size.mm("0.6", "0.2")),
    ), Size.mm(1, 1))
    placements, nets, rules = [], [], []
    for index, (name, y, start, end) in enumerate(lanes):
        source, sink = f"U{index + 1}", f"J{index + 1}"
        placements += [Placement(source, lands.name, Point.mm(start, y)),
                       Placement(sink, lands.name, Point.mm(end, y))]
        nets += [PhysicalNet(f"{name}_N", (PadReference(source, "1"), PadReference(sink, "1"))),
                 PhysicalNet(f"{name}_P", (PadReference(source, "2"), PadReference(sink, "2")))]
        profile = dict(priority=100, width_nm=WIDTH, pair_gap_nm=GAP, max_skew_nm=PAIR_SKEW,
                       tuning_amplitude_limit_nm=amplitude,
                       maximum_uncoupled_length_nm=nm_from_mm(uncoupled) if uncoupled else None)
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **profile),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **profile)]
    groups = groups if groups is not None else {"lanes": tuple(name for name, *_ in lanes)}
    return PhysicalBoard(
        "LengthMatchLanes", BoardOutline.rectangle(40, 30), {lands.name: lands}, tuple(placements), tuple(nets),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=WIDTH),
        net_routing_rules=tuple(rules), copper_keepouts=tuple(keepouts),
        match_groups=tuple(NetMatchGroup(group, tuple(f"{name}_{member}" for name in names for member in "NP"),
                                         nm_from_mm(max_skew)) for group, names in groups.items()),
    )


def _hard(board: PhysicalBoard) -> set[str]:
    """Hard native findings besides the expected partial-routing state."""
    return {finding.code for finding in run_physical_drc(board).findings
            if finding.severity.value == "error" and finding.code not in {"DRC-ROUTE-INCOMPLETE", "DRC-OPEN-NET"}}


def _lane(tracks, name: str) -> tuple[TrackSegment, ...]:
    return tuple(track for track in tracks if track.net.startswith(f"{name}_"))


def _length(tracks, net: str) -> int:
    return _track_length(tuple(track for track in tracks if track.net == net))


def test_pairs_of_a_match_group_are_tuned_as_pairs_into_the_group_skew() -> None:
    board = _lanes_board()
    result = route_critical_nets(board, route_global(board))
    assert result.status is not CriticalRoutingStatus.FAILED
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.skew_before_nm, tuning.reason) == ("tuned", nm_from_mm(8), "")
    assert tuning.skew_after_nm <= nm_from_mm("0.1")
    # Both members of a pair gain the same length, so the pair's own skew is
    # kept within its own max_skew; the longest pair is untouched.
    added = {member.net: member.added_length_nm for member in tuning.members}
    assert added["LANE_A_N"] == added["LANE_A_P"] == 0
    assert added["LANE_B_N"] == added["LANE_B_P"] > 0
    assert added["LANE_C_N"] == added["LANE_C_P"] > 0
    for name in ("LANE_A", "LANE_B", "LANE_C"):
        assert abs(_length(result.board.tracks, f"{name}_N") - _length(result.board.tracks, f"{name}_P")) <= PAIR_SKEW
    (check,) = verify_match_groups(result.board)
    assert check.status == "pass" and check.skew_nm == tuning.skew_after_nm
    assert not _hard(result.board)
    # Bumps are no taller than the amplitude limit and both members bend
    # together at the pair's own lane spacing.
    for name, y in (("LANE_B", 14), ("LANE_C", 22)):
        lane = _lane(result.board.tracks, name)
        assert max(abs(p.y_nm - nm_from_mm(y)) for t in lane for p in (t.start, t.end)) <= SPACING // 2 + AMPLITUDE
        first = [t for t in lane if t.net.endswith("_N")]
        second = [t for t in lane if t.net.endswith("_P")]
        assert min(segment_distance_squared(a.start, a.end, b.start, b.end) for a in first for b in second) == SPACING ** 2
    # Results and the lane table report the tuned copper.
    assert all(item.lengths_nm == (nm_from_mm(28), nm_from_mm(28)) and item.skew_nm == 0 for item in result.nets)
    lanes = {lane["net"]: lane["routed_length_nm"] for lane in critical_lane_table(result.board, result.nets)}
    assert set(lanes.values()) == {nm_from_mm(28)}
    document = json.loads(result.to_json())["match_tuning"][0]
    assert document["status"] == "tuned" and document["bumps"] == tuning.bumps
    assert document["members"][2] == {"net": "LANE_B_N", "length_before_nm": nm_from_mm(20),
                                      "length_after_nm": nm_from_mm(28), "added_length_nm": nm_from_mm(8)}


def test_a_single_ended_member_is_tuned_alone() -> None:
    board = _lanes_board(TWO_LANES)
    land = PhysicalFootprint("test/one-land", (FootprintPad("1", Point.mm(0, 0), Size.mm("0.6", "0.3")),), Size.mm(1, 1))
    board = replace(
        board, footprints={**board.footprints, land.name: land},
        placements=(*board.placements, Placement("U9", land.name, Point.mm(5, 22)),
                    Placement("J9", land.name, Point.mm(21, 22))),
        nets=(*board.nets, PhysicalNet("CLOCK_REF", (PadReference("U9", "1"), PadReference("J9", "1")))),
        net_routing_rules=(*board.net_routing_rules, NetRoutingRule(
            "CLOCK_REF", RouteKind.CLOCK, priority=100, width_nm=WIDTH, tuning_amplitude_limit_nm=AMPLITUDE)),
        match_groups=(NetMatchGroup("lanes", ("LANE_A_N", "LANE_A_P", "LANE_B_N", "LANE_B_P", "CLOCK_REF"),
                                    nm_from_mm("0.1")),),
    )
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned", tuning.reason
    added = {member.net: member.added_length_nm for member in tuning.members}
    assert added == {"LANE_A_N": 0, "LANE_A_P": 0, "LANE_B_N": nm_from_mm(4), "LANE_B_P": nm_from_mm(4),
                     "CLOCK_REF": nm_from_mm(12)}
    assert next(item for item in result.nets if item.nets == ("CLOCK_REF",)).lengths_nm == (nm_from_mm(28),)
    assert not _hard(result.board)


def test_bumps_respect_the_amplitude_limit_and_prefer_the_free_side() -> None:
    # A wall 0.5 mm above LANE_B leaves the side below free: both bumps go
    # below, at the full amplitude limit.
    board = _lanes_board(TWO_LANES, keepouts=(_keepout("wall-above", 6, "14.8", 28, 17),))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.bumps) == ("tuned", 2)
    lane = _lane(result.board.tracks, "LANE_B")
    ys = {p.y_nm for t in lane for p in (t.start, t.end)}
    assert max(ys) == nm_from_mm("14.2")
    assert min(ys) == nm_from_mm("13.8") - AMPLITUDE
    assert not _hard(result.board)

    # Walls on both sides leave 0.5 mm: more, lower bumps within that room.
    narrow = _lanes_board(TWO_LANES, keepouts=(_keepout("wall-above", 6, "14.8", 28, 17),
                                               _keepout("wall-below", 6, 11, 28, "13.2")))
    result = route_critical_nets(narrow, route_global(narrow))
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.bumps) == ("tuned", 4)
    lane = _lane(result.board.tracks, "LANE_B")
    assert all(nm_from_mm("13.3") <= p.y_nm <= nm_from_mm("14.7") for t in lane for p in (t.start, t.end))
    assert not _hard(result.board)


def test_a_unit_that_cannot_match_the_longest_member_adds_the_minimum_within_the_limit() -> None:
    # A loose 6 mm group limit: matching the 7.2 mm pair to the 14.15 mm one
    # needs 6.95 mm, more than three 0.3 mm bumps on its short run can add.
    # The least length that reaches the limit (0.95 mm) still fits.
    board = _lanes_board((("LANE_A", 6, 5, "19.15"), ("LANE_B", 14, 5, "12.2")), max_skew="6",
                         amplitude=nm_from_mm("0.3"))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.skew_before_nm, tuning.skew_after_nm, tuning.bumps) == (
        "tuned", nm_from_mm("6.95"), nm_from_mm(6), 2)
    assert {member.net: member.added_length_nm for member in tuning.members if member.added_length_nm} == {
        "LANE_B_N": nm_from_mm("0.95"), "LANE_B_P": nm_from_mm("0.95")}
    assert not _hard(result.board)


def test_tuning_lands_where_there_is_room_along_the_member() -> None:
    # Below LANE_B is blocked along its whole run, above it from x = 9 to 21 mm.
    board = _lanes_board(TWO_LANES, keepouts=(_keepout("below", 6, 11, 28, "13.69"),
                                              _keepout("above-middle", 9, "14.31", 21, 17)))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned", tuning.reason
    raised = [t for t in _lane(result.board.tracks, "LANE_B")
              if max(t.start.y_nm, t.end.y_nm) > nm_from_mm("14.2")]
    assert raised
    assert all(min(t.start.x_nm, t.end.x_nm) > nm_from_mm(21) for t in raised)
    assert min(p.y_nm for t in _lane(result.board.tracks, "LANE_B") for p in (t.start, t.end)) == nm_from_mm("13.8")
    assert not _hard(result.board)


@pytest.mark.parametrize(("options", "reason"), (
    ({"keepouts": (_keepout("above", 6, "14.305", 28, 17), _keepout("below", 6, 11, 28, "13.695"))},
     "LANE_B_N/LANE_B_P: insufficient tuning room"),
    ({"amplitude": None}, "LANE_B_N/LANE_B_P: no tuning_amplitude_limit is declared"),
    ({"uncoupled": "2"}, "LANE_B_N/LANE_B_P: pair uncoupled length"),
))
def test_impossible_tuning_restores_the_copper_and_fails_verification(options, reason) -> None:
    board = _lanes_board(TWO_LANES, **options)
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    baseline = route_critical_nets(replace(board, match_groups=()), guides)
    (tuning,) = result.match_tuning
    assert tuning.status == "failed" and tuning.reason.startswith(reason)
    assert tuning.skew_before_nm == tuning.skew_after_nm == nm_from_mm(4)
    assert tuning.bumps == 0 and all(member.added_length_nm == 0 for member in tuning.members)
    # Every pair is still accepted; the copper and results are exactly those
    # of never tuning, and the stage fails.
    assert all(item.connected and not item.diagnostics for item in result.nets)
    assert result.locked_tracks == baseline.locked_tracks
    assert result.locked_vias == baseline.locked_vias
    assert result.nets == baseline.nets
    assert result.routing_fingerprint == baseline.routing_fingerprint
    assert result.status is CriticalRoutingStatus.FAILED and baseline.status is not CriticalRoutingStatus.FAILED
    assert result.board.metadata["critical_routing"] == "failed"
    assert "DRC-LENGTH-MATCH" in _hard(result.board)
    assert json.loads(result.to_json())["match_tuning"][0]["reason"] == tuning.reason


def test_groups_within_their_limit_are_untouched() -> None:
    lanes = (("LANE_A", 6, 5, 33), ("LANE_B", 12, 5, 33), ("LANE_C", 18, 5, 33), ("LANE_D", 24, 5, 25))
    board = _lanes_board(lanes, groups={"matched": ("LANE_A", "LANE_B"), "short": ("LANE_C", "LANE_D")})
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    baseline = route_critical_nets(replace(board, match_groups=()), guides)
    assert [(item.id, item.status) for item in result.match_tuning] == [("matched", "within_limit"),
                                                                       ("short", "tuned")]
    matched = result.match_tuning[0]
    assert matched.skew_before_nm == matched.skew_after_nm == 0 and matched.bumps == 0
    for name in ("LANE_A", "LANE_B", "LANE_C"):
        assert _lane(result.locked_tracks, name) == _lane(baseline.locked_tracks, name)
    assert _lane(result.locked_tracks, "LANE_D") != _lane(baseline.locked_tracks, "LANE_D")

    # A board whose only group is within its limit is identical to no group.
    alone = _lanes_board(lanes[:2])
    guides = route_global(alone)
    result = route_critical_nets(alone, guides)
    baseline = route_critical_nets(replace(alone, match_groups=()), guides)
    assert result.match_tuning[0].status == "within_limit"
    assert (result.locked_tracks, result.nets, result.routing_fingerprint) == (
        baseline.locked_tracks, baseline.nets, baseline.routing_fingerprint)


def test_groups_with_an_unrouted_member_are_left_to_final_verification() -> None:
    board = _lanes_board(TWO_LANES)
    board = replace(board, net_routing_rules=tuple(rule for rule in board.net_routing_rules
                                                   if not rule.net.startswith("LANE_B")))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.skew_before_nm, tuning.skew_after_nm) == ("incomplete", None, None)
    assert result.status is not CriticalRoutingStatus.FAILED


def test_tuning_is_deterministic() -> None:
    board = _lanes_board()
    guides = route_global(board)
    first = route_critical_nets(board, guides)
    second = route_critical_nets(board, guides)
    assert first == second
    assert first.to_json() == second.to_json()


def test_candidates_completing_an_over_skew_group_are_not_rejected_during_routing() -> None:
    board = _lanes_board(TWO_LANES, amplitude=None)
    routed = route_critical_nets(board, route_global(board))
    first, second = routed.nets
    assert first.connected and second.connected
    tracks_a, tracks_b = _lane(routed.locked_tracks, "LANE_A"), _lane(routed.locked_tracks, "LANE_B")
    # Together the two pairs violate the group skew, yet the candidate that
    # completes the group passes validation: the tuning pass judges the group.
    assert "DRC-LENGTH-MATCH" in _hard(replace(board, tracks=(*tracks_a, *tracks_b)))
    accepted, accepted_tracks, _ = _validate_candidate(board, second, tracks_b, (), list(tracks_a), [])
    assert accepted.connected and not accepted.diagnostics and accepted_tracks == tracks_b
    # Other hard findings still reject it.
    blocked = replace(board, copper_keepouts=(_keepout("on-lane", 14, 13, 16, 15),))
    rejected, rejected_tracks, _ = _validate_candidate(blocked, second, tracks_b, (), list(tracks_a), [])
    assert not rejected.connected and rejected_tracks == ()
    assert not any(message.startswith("DRC-LENGTH-MATCH") for message in rejected.diagnostics)


def test_pair_bumps_keep_the_lane_spacing_in_any_direction() -> None:
    board = PhysicalBoard("Empty", BoardOutline.rectangle(60, 60), {}, (), ())
    obstacles = RoutingClearanceIndex(board)
    layer = CopperLayer.FRONT
    shapes = {
        "bend": (TrackSegment("A", Point.mm(5, 10), Point.mm(25, 10), WIDTH, layer),
                 TrackSegment("A", Point.mm(25, 10), Point.mm(25, 30), WIDTH, layer),
                 TrackSegment("B", Point.mm(5, "9.6"), Point.mm("25.4", "9.6"), WIDTH, layer),
                 TrackSegment("B", Point.mm("25.4", "9.6"), Point.mm("25.4", 30), WIDTH, layer)),
        "reversed": (TrackSegment("A", Point.mm(30, 50), Point.mm(30, 10), WIDTH, layer),
                     TrackSegment("B", Point.mm("30.4", 10), Point.mm("30.4", 50), WIDTH, layer)),
    }
    for tracks in shapes.values():
        tuner = UnitTuner(board, obstacles, tracks, ("A", "B"), SPACING, AMPLITUDE, nm_from_mm("0.1"))
        tuned, bumps, reason = tuner.tune(nm_from_mm(30))
        assert reason == "" and bumps == 15
        for net in ("A", "B"):
            # Each member gains the same length, its path stays continuous and
            # no bump rises more than the amplitude limit off its own lane.
            assert _length(tuned, net) == _length(tracks, net) + nm_from_mm(30)
            path = [t for t in tuned if t.net == net]
            assert all(a.end == b.start for a, b in zip(path, path[1:]))
            lane = [t for t in tracks if t.net == net]
            assert max(min(segment_distance_squared(p, p, t.start, t.end) for t in lane)
                       for track in path for p in (track.start, track.end)) == AMPLITUDE ** 2
        first = [t for t in tuned if t.net == "A"]
        second = [t for t in tuned if t.net == "B"]
        assert min(segment_distance_squared(a.start, a.end, b.start, b.end)
                   for a in first for b in second) == SPACING ** 2
        # The bump count is bounded.
        bounded, _, reason = UnitTuner(board, obstacles, tracks, ("A", "B"), SPACING, AMPLITUDE,
                                       nm_from_mm("0.1"), bump_limit=4).tune(nm_from_mm(30))
        assert bounded is None and "the limit of 4 bumps" in reason


def test_preflight_prints_one_line_per_match_group(tmp_path, monkeypatch, capsys) -> None:
    import pcbir.critical_preflight as preflight

    board = _lanes_board()
    routed = route_critical_nets(board, route_global(board))
    monkeypatch.setattr(preflight, "route_critical_nets", lambda *_, **__: routed)
    report = tmp_path / "report.json"
    preflight.main([str(Path(__file__).resolve().parents[1] / "examples/valid_board/board.copper"),
                    "--allow-proxy-footprints", "--layers", "2", "--fab-profile", "generic",
                    "--router-iterations", "1", "--report", str(report)])
    assert json.loads(report.read_text())["critical"]["match_tuning"][0]["status"] == "tuned"
    out = capsys.readouterr().out
    assert ("match group lanes: tuned, skew 8.000 -> 0.000 mm (max 0.100 mm); added "
            "LANE_B_N +8.000, LANE_B_P +8.000, LANE_C_N +4.000, LANE_C_P +4.000 mm; "
            f"{routed.match_tuning[0].bumps} bumps") in out


def test_tuned_copper_of_breakout_nets_follows_the_breakout_regions() -> None:
    from pcbir.breakout import BreakoutRegions

    board = _lanes_board()
    # Regions reach 9.5 mm from each land and cover most of every lane, so
    # tuning works next to necked breakout copper.
    board = replace(board, net_routing_rules=tuple(
        replace(rule, breakout_length_nm=nm_from_mm("9.5"), breakout_width_nm=nm_from_mm("0.15"))
        for rule in board.net_routing_rules))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned" and tuning.skew_after_nm <= tuning.max_skew_nm
    assert not _hard(result.board)
    tracks = result.board.tracks
    # Every piece is already cut at the boundaries with the region's width:
    # necked inside a region, the net's own width outside.
    assert BreakoutRegions(result.board).split_tracks(tracks, WIDTH) == tracks
    assert {track.width_nm for track in tracks} == {WIDTH, nm_from_mm("0.15")}

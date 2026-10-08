"""D-PHY plan R14: group serpentines, adjacent units of a length_match group tuned together."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
import json

import pytest

import pcbir.critical as critical
from pcbir import (
    BoardOutline, CopperLayer, CriticalRoutingStatus, FootprintPad, NetMatchGroup, NetRoutingRule, PadReference,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PrototypePhysicalOptions, RouteKind, Size,
    TrackSegment, TuningStyle, compile_source, nm_from_mm, physical_board_digest, prototype_physicalize,
    route_critical_nets, route_global, verify_match_groups,
)
from pcbir.critical import _drop_group_steps, _track_length
from pcbir.critical_group_tuning import GroupTuner, lane_corners
from pcbir.critical_tuning import MatchTuningUnit, bump_chamfers, match_tuning_line
from pcbir.geometry import segment_distance_squared
from pcbir.physical import DesignRules
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.syntax import CopperScriptError

from test_length_match_tuning import _hard, _keepout, _lanes_board, _lane_offsets, _sharp_bends

WIDTH, GAP = nm_from_mm("0.2"), nm_from_mm("0.2")
SPACING = WIDTH + GAP
NAMES = ("LANE_A", "LANE_B", "LANE_C")
# Pairs at a 0.9 mm pitch leave 0.3 mm between neighbouring lanes: at the
# board's 0.1 mm clearance, too little for a pair bump's coupled corners.
LANES = ("LANE_A_N", "LANE_A_P", "LANE_B_N", "LANE_B_P", "LANE_C_N", "LANE_C_P")
# A wall 0.25 mm above LANE_A hems the bundle in from above.
WALL_ABOVE = _keepout("above", 6, "12.5", 24, "13.55")


def _group_board(run="20", *, amplitude="2", keepouts=(WALL_ABOVE,), group: str | None = "bundle",
                 style=TuningStyle.BUMPS, longest="24", max_skew="0.1") -> PhysicalBoard:
    """Three straight 0.2/0.2 mm pairs from U1 to J1 at a 0.9 mm pitch, and a longer pair LANE_L.

    The bundle runs along y = 14.1, 15 and 15.9 mm for ``run`` mm; LANE_L runs
    ``longest`` mm along y = 5 mm and routes first. All eight nets form the
    length-match group ``lanes``; the bundle's pairs the tuning ``group``.
    """
    row = PhysicalFootprint("test/pair-row", tuple(
        FootprintPad(str(2 * index + member + 1), Point.mm(0, Decimal("0.9") * (index - 1) + Decimal("0.4") * member
                                                           - Decimal("0.2")), Size.mm("0.6", "0.2"))
        for index in range(3) for member in (0, 1)), Size.mm(1, 3))
    pair = PhysicalFootprint("test/pair-lands", (FootprintPad("1", Point.mm(0, "-0.2"), Size.mm("0.6", "0.2")),
                                                 FootprintPad("2", Point.mm(0, "0.2"), Size.mm("0.6", "0.2"))),
                             Size.mm(1, 1))
    placements = (Placement("U1", row.name, Point.mm(5, 15)), Placement("J1", row.name, Point.mm(5 + Decimal(run), 15)),
                  Placement("U2", pair.name, Point.mm(5, 5)), Placement("J2", pair.name, Point.mm(5 + Decimal(longest), 5)))
    profile = dict(width_nm=WIDTH, pair_gap_nm=GAP, max_skew_nm=nm_from_mm("0.05"),
                   tuning_amplitude_limit_nm=nm_from_mm(amplitude), tuning_style=style)
    nets, rules = [], []
    for index, name in enumerate((*NAMES, "LANE_L")):
        source, sink = ("U2", "J2") if name == "LANE_L" else ("U1", "J1")
        n, p = ("1", "2") if name == "LANE_L" else (str(2 * index + 1), str(2 * index + 2))
        nets += [PhysicalNet(f"{name}_N", (PadReference(source, n), PadReference(sink, n))),
                 PhysicalNet(f"{name}_P", (PadReference(source, p), PadReference(sink, p)))]
        options = dict(profile, priority=200 if name == "LANE_L" else 100,
                       tuning_group=None if name == "LANE_L" else group)
        rules += [NetRoutingRule(f"{name}_P", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_N", **options),
                  NetRoutingRule(f"{name}_N", RouteKind.DIFFERENTIAL, differential_partner=f"{name}_P", **options)]
    return PhysicalBoard(
        "GroupLanes", BoardOutline.rectangle(40, 30), {row.name: row, pair.name: pair}, placements, tuple(nets),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=WIDTH),
        net_routing_rules=tuple(rules), copper_keepouts=tuple(keepouts),
        match_groups=(NetMatchGroup("lanes", tuple(net.name for net in nets), nm_from_mm(max_skew)),))


def _final_pass_only(monkeypatch) -> None:
    monkeypatch.setattr(critical, "_tune_bundle_pairs", lambda *_, **__: False)


def _offsets(tracks, line: int) -> set[int]:
    return {p.y_nm - line for t in tracks for p in (t.start, t.end)}


def _check_lanes(board: PhysicalBoard, added: int) -> None:
    """Every lane gained ``added``, pairs keep their spacing, neighbours their gap, no bend over 45 degrees."""
    paths = {net: [t for t in board.tracks if t.net == net] for net in LANES}
    for net, path in paths.items():
        assert _track_length(tuple(path)) == nm_from_mm(20) + added
        assert all(a.end == b.start for a, b in zip(path, path[1:]))
        assert _sharp_bends(path) == []
    for first, second in zip(LANES, LANES[1:]):
        gap = SPACING if first[:-2] == second[:-2] else nm_from_mm("0.5")
        assert min(segment_distance_squared(a.start, a.end, b.start, b.end)
                   for a in paths[first] for b in paths[second]) == gap ** 2
    for name in NAMES:
        assert all(SPACING ** 2 <= offset < (SPACING + 2) ** 2
                   for offset in _lane_offsets(paths[f"{name}_N"], paths[f"{name}_P"]))


def test_a_hemmed_bundle_is_bent_as_one_group_while_routing(monkeypatch) -> None:
    board = _group_board()
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    (tuning,) = result.match_tuning
    assert (tuning.status, tuning.skew_before_nm, tuning.skew_after_nm) == ("tuned", nm_from_mm(4), 0)
    assert result.status is not CriticalRoutingStatus.FAILED and not _hard(result.board)
    # One group step, as soon as all three pairs were accepted: every lane
    # gains exactly the 4 mm, in one-sided bumps below the bundle.
    (unit,) = tuning.units
    assert (unit.group, unit.stage, unit.style, unit.lanes) == ("bundle", "routing", "bumps", LANES)
    assert unit.nets == ("LANE_A_N", "LANE_A_P", "LANE_B_N", "LANE_B_P", "LANE_C_N", "LANE_C_P")
    assert unit.bumps == len(unit.amplitudes_nm) and max(unit.amplitudes_nm) <= nm_from_mm(2)
    assert {member.net: member.added_length_nm for member in tuning.members} == {
        **{net: nm_from_mm(4) for net in LANES}, "LANE_L_N": 0, "LANE_L_P": 0}
    _check_lanes(result.board, nm_from_mm(4))
    assert min(p.y_nm for net in LANES for t in result.board.tracks if t.net == net
               for p in (t.start, t.end)) == nm_from_mm("13.9")
    (check,) = verify_match_groups(result.board)
    assert check.status == "pass"
    document = json.loads(result.to_json())["match_tuning"][0]
    assert document["units"][0]["group"] == "bundle" and document["units"][0]["lanes"] == list(LANES)
    assert document["tuning_groups"] == [{"name": "bundle", "nets": list(LANES), "status": "tuned", "reason": ""}]
    line = match_tuning_line(tuning)
    assert f"group bundle {'/'.join(LANES)} bumps {unit.bumps} bumps up to" in line and "while routing" in line
    # Deterministic.
    again = route_critical_nets(board, guides)
    assert again == result and again.to_json() == result.to_json()

    # Pair by pair, LANE_A is hemmed in by the wall and LANE_B.
    alone = route_critical_nets(_group_board(group=None), guides)
    (tuning,) = alone.match_tuning
    assert tuning.status == "failed" and "insufficient tuning room" in tuning.reason

    # The final pass bends the group the same way when nothing ran while routing.
    _final_pass_only(monkeypatch)
    late = route_critical_nets(board, guides)
    (tuning,) = late.match_tuning
    assert tuning.status == "tuned" and [(unit.group, unit.stage) for unit in tuning.units] == [("bundle", "final")]
    _check_lanes(late.board, nm_from_mm(4))
    assert not _hard(late.board)


def test_a_run_too_short_for_a_group_bump_leaves_the_copper_and_says_why() -> None:
    # 6 mm between the lands: a group bump needs the inner lane's 0.6 mm,
    # twice the 2.2 mm lane span and 0.6 mm at each end. A 0.2 mm amplitude
    # also rules out every pair's own bumps.
    board = _group_board("6", longest="10", amplitude="0.2", keepouts=())
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    (tuning,) = result.match_tuning
    assert tuning.status == "failed" and not tuning.units
    (group,) = tuning.tuning_groups
    assert (group.name, group.status) == ("bundle", "failed")
    assert ("the longest straight run of all 6 lanes is 6.000 mm, a one-sided group bump needs 6.200 mm: the "
            "inner lane's 0.600 mm plus twice the 2.200 mm lane span, and 0.600 mm at each end") in group.reason
    assert "tuning group bundle failed (tuning group bundle: insufficient tuning room" in match_tuning_line(tuning)
    baseline = route_critical_nets(replace(_group_board("6", longest="10", amplitude="0.2", keepouts=(), group=None),
                                           match_groups=()), guides)
    assert result.locked_tracks == baseline.locked_tracks and result.nets == baseline.nets


def test_a_group_serpentine_needs_room_on_both_sides() -> None:
    open_board = _group_board(style=TuningStyle.SERPENTINE, keepouts=())
    result = route_critical_nets(open_board, route_global(open_board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned" and not _hard(result.board)
    (unit,) = tuning.units
    assert (unit.style, unit.bumps) == ("serpentine", 0) and unit.legs == len(unit.amplitudes_nm) + 1
    _check_lanes(result.board, nm_from_mm(4))
    # Every lane snakes about its own line, to both sides.
    for net, line in zip(LANES, ("13.9", "14.3", "14.8", "15.2", "15.7", "16.1")):
        offsets = _offsets([t for t in result.board.tracks if t.net == net], nm_from_mm(line))
        assert min(offsets) < 0 < max(offsets)
    # With the wall above, the group falls back to one-sided bumps below.
    walled = _group_board(style=TuningStyle.SERPENTINE)
    result = route_critical_nets(walled, route_global(walled))
    (tuning,) = result.match_tuning
    (unit,) = tuning.units
    assert (tuning.status, unit.style, unit.legs) == ("tuned", "bumps", 0)
    _check_lanes(result.board, nm_from_mm(4))
    for net, line in zip(LANES, ("13.9", "14.3", "14.8", "15.2", "15.7", "16.1")):
        assert min(_offsets([t for t in result.board.tracks if t.net == net], nm_from_mm(line))) == 0


@pytest.mark.parametrize("style", [TuningStyle.BUMPS, TuningStyle.SERPENTINE])
def test_uneven_lanes_gain_exactly_the_same_length(style) -> None:
    # A pair, a single net and a pair at uneven gaps (y in mm).
    lines = {"A": "10", "B": "10.4", "C": "11.13", "D": "11.937", "E": "12.337"}
    tracks = tuple(TrackSegment(net, Point.mm(5, y), Point.mm(45, y), WIDTH, CopperLayer.FRONT)
                   for net, y in lines.items())
    empty = PhysicalBoard("Empty", BoardOutline.rectangle(60, 60), {}, (), ())
    tuner = GroupTuner(empty, RoutingClearanceIndex(empty), tracks, (("A", "B"), ("C",), ("D", "E")),
                       (SPACING, 0, SPACING), nm_from_mm(2), nm_from_mm("0.1"), style=style)
    styles = set()
    for added in (nm_from_mm("1.9"), nm_from_mm("4.3"), nm_from_mm(7) + 2):
        plan = tuner.plan(added)
        assert plan.reason == "" and plan.lanes == tuple(lines)
        styles.add(plan.style)
        paths = {net: [t for t in plan.tracks if t.net == net] for net in lines}
        for net, path in paths.items():
            assert _track_length(tuple(path)) == nm_from_mm(40) + added
            assert _sharp_bends(path) == []
        for first, second in zip(lines, list(lines)[1:]):
            gap = nm_from_mm(lines[second]) - nm_from_mm(lines[first])
            assert min(segment_distance_squared(a.start, a.end, b.start, b.end)
                       for a in paths[first] for b in paths[second]) == gap ** 2
    assert styles == ({"bumps", "serpentine"} if style is TuningStyle.SERPENTINE else {"bumps"})


def test_lane_corners_are_r9_for_a_pair_and_level_rounding_for_more_lanes() -> None:
    # A pair alone takes R9's coupled chamfers: inside c, outside c + d.
    for chamfer in (1, 1_000, 99_999):
        inside, outside = bump_chamfers(4 * chamfer + nm_from_mm(1), chamfer, SPACING)
        assert lane_corners((SPACING,), chamfer) == ((outside, inside), (inside, outside))
    # More lanes: every lane loses the same rounded length per leg, and
    # adjacent offsets never exceed floor((2 - sqrt 2) x gap).
    gaps = (SPACING, nm_from_mm("0.5"), 333_333, SPACING, 1_234_567)
    for chamfer in (1, 17, 2_000, 140_000):
        corners = lane_corners(gaps, chamfer)
        assert corners is not None
        losses = {sum(2 * leg - round((2 * leg * leg) ** 0.5) for leg in pair) for pair in corners}
        assert len(losses) == 1
        assert corners[-1][0] == corners[0][1] == chamfer
        for gap, (low, up), (next_low, next_up) in zip(gaps, corners, corners[1:]):
            step = 2 * gap - int((2 * gap * gap) ** 0.5) - 1
            assert 0 <= low - next_low <= step and 0 <= next_up - up <= step


def test_tuning_group_steps_are_dropped_when_a_unit_reroutes() -> None:
    a, b, c = ("A_N", "A_P"), ("B_N", "B_P"), ("C_N", "C_P")
    tracks = {unit: (TrackSegment(unit[0], Point(0, index), Point(10, index), 1, CopperLayer.FRONT),)
              for index, unit in enumerate((a, b, c))}
    tuned = {unit: (*items, *items) for unit, items in tracks.items()}
    committed = [(a, tuned[a], ()), (b, tuned[b], ()), (c, tracks[c], ())]
    results = ["a", "b-tuned", "c"]
    early = [MatchTuningUnit((*a, *b), "routing", "bumps", 2, 1, 0, (5,), "pair-ab", (*a, *b)),
             MatchTuningUnit(a, "routing", "bumps", 2, 1, 0, (5,))]
    as_routed = {a: (tracks[a], "a-routed"), b: (tracks[b], "b-routed")}
    routed = {"A_N": 1, "A_P": 1, "B_N": 1, "B_P": 1}
    failed = {(*a, *b): 3, c: 3}
    # A re-routed: B goes back to its copper as routed, every step of A and B goes.
    assert _drop_group_steps((a,), committed, results, {a: 0, b: 1, c: 2}, early, routed, failed, as_routed)
    assert committed[1] == (b, tracks[b], ()) and results == ["a", "b-routed", "c"]
    assert early == [] and failed == {c: 3} and as_routed == {a: (tracks[a], "a-routed")}
    assert routed == {"A_N": 1, "A_P": 1}
    # Without group steps nothing changes.
    assert not _drop_group_steps((c,), committed, results, {a: 0, b: 1, c: 2}, early, routed, failed, as_routed)


def test_boards_without_tuning_groups_never_reach_the_group_tuner(monkeypatch) -> None:
    def refuse(*_, **__):
        raise AssertionError("group tuner used without a tuning group")

    monkeypatch.setattr(critical, "GroupTuner", refuse)
    board = _lanes_board()
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned" and not tuning.tuning_groups
    assert all(not unit.group and not unit.lanes for unit in tuning.units)
    document = json.loads(result.to_json())["match_tuning"][0]
    assert "tuning_groups" not in document and all("group" not in unit for unit in document.get("units", ()))
    assert "tuning group" not in match_tuning_line(tuning)


# --- Language ----------------------------------------------------------------


def _source(properties: dict[str, str] | None = None,
            groups: str = "constraint length_match(CKP, CKN, D0P, D0N) { max_skew = 0.1mm; }") -> str:
    properties = properties if properties is not None else {net: 'tuning_group = "lanes";'
                                                            for net in ("CKP", "CKN", "D0P", "D0N")}
    partners = {"CKP": "CKN", "CKN": "CKP", "D0P": "D0N", "D0N": "D0P"}
    rules = "\n".join(f"""constraint routing({net}) {{
            kind = differential; partner = {partner}; width = 0.14mm; pair_gap = 0.26mm;
            tuning_amplitude_limit = 1mm; {properties.get(net, "")}
        }}""" for net, partner in partners.items())
    return f"""board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        component R3: RESISTOR {{ footprint = "0402"; }}
        component R4: RESISTOR {{ footprint = "0402"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        net D0P {{ R3.1; R4.1; }}
        net D0N {{ R3.2; R4.2; }}
        {rules}
        {groups}
    }}"""


JLC = PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer")


def test_tuning_group_lowers_to_the_rule_and_binds_the_digest_only_when_declared() -> None:
    board = prototype_physicalize(compile_source(_source()), JLC)
    assert {rule.net: rule.tuning_group for rule in board.net_routing_rules} == {
        "CKP": "lanes", "CKN": "lanes", "D0P": "lanes", "D0N": "lanes"}
    plain = prototype_physicalize(compile_source(_source({})), JLC)
    assert {rule.tuning_group for rule in plain.net_routing_rules} == {None}
    assert physical_board_digest(board) != physical_board_digest(plain)
    assert physical_board_digest(plain) == physical_board_digest(replace(board, net_routing_rules=tuple(
        replace(rule, tuning_group=None) for rule in board.net_routing_rules)))


@pytest.mark.parametrize("properties, message", [
    ('tuning_group = "";', "'tuning_group' must be a nonempty name"),
    ("tuning_group = 3;", "'tuning_group' must be a nonempty name"),
    ('kind = general; tuning_group = "lanes";', "'tuning_group' requires a critical routing kind"),
])
def test_invalid_tuning_group_values_fail_with_locations(properties: str, message: str) -> None:
    source = f"""board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        net CK {{ R1.1; R2.1; }}
        constraint routing(CK) {{ {properties} }}
    }}"""
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(source, "lanes.copper")
    assert error.value.code == "CMP110"
    assert error.value.location.filename == "lanes.copper"


@pytest.mark.parametrize("properties, groups, message", [
    ({"CKP": 'tuning_group = "lanes";', "CKN": 'tuning_group = "lanes";'}, "",
     "tuning_group 'lanes' requires 'CKP' to belong to a length_match group"),
    ({net: 'tuning_group = "lanes";' for net in ("CKP", "CKN", "D0P", "D0N")},
     "constraint length_match(CKP, CKN) { id = \"ck\"; max_skew = 0.1mm; }"
     "constraint length_match(D0P, D0N) { id = \"d0\"; max_skew = 0.1mm; }",
     "tuning_group 'lanes' joins 'D0P' of length_match group 'd0' and 'CKP' of 'ck'"),
    ({"CKP": 'tuning_group = "lanes";'}, "constraint length_match(CKP, CKN, D0P, D0N) { max_skew = 0.1mm; }",
     "routing rules for 'CKP' and 'CKN' must agree on tuning_group"),
])
def test_tuning_group_semantics_fail_with_the_routing_constraint_location(properties, groups, message) -> None:
    electrical = compile_source(_source(properties, groups), "lanes.copper")
    with pytest.raises(ValueError, match=rf"lanes.copper:\d+:\d+: CMP117: {message}"):
        prototype_physicalize(electrical, JLC)


def test_physical_boards_reject_inconsistent_tuning_groups() -> None:
    with pytest.raises(ValueError, match="tuning group must be a nonempty name"):
        NetRoutingRule("A", RouteKind.CLOCK, tuning_group=" ")
    with pytest.raises(ValueError, match="tuning group requires a critical routing kind"):
        NetRoutingRule("A", tuning_group="lanes")
    board = _group_board()
    # Derived boards may keep the rules without the groups (CMP117 checks the source).
    assert replace(board, match_groups=()).match_groups == ()
    with pytest.raises(ValueError, match="tuning group 'bundle' spans length-match groups 'a' and 'b'"):
        replace(board, match_groups=(NetMatchGroup("a", LANES[:2], 1), NetMatchGroup("b", LANES[2:], 1)))
    with pytest.raises(ValueError, match="must agree on tuning_group"):
        replace(board, net_routing_rules=tuple(replace(rule, tuning_group=None) if rule.net == "LANE_A_N" else rule
                                               for rule in board.net_routing_rules))

"""D-PHY plan R10: S-shaped (two-sided) serpentines for length-match tuning."""
from __future__ import annotations

from dataclasses import replace
import json

import pytest

from pcbir import (
    BoardOutline, CopperLayer, NetRoutingRule, PhysicalBoard, Point, PrototypePhysicalOptions, RouteKind,
    TrackSegment, TuningStyle, compile_source, nm_from_mm, physical_board_digest, prototype_physicalize,
    route_critical_nets, route_global, verify_match_groups,
)
from pcbir.critical import _track_length
from pcbir.critical_tuning import (MATCH_TUNING_LEG_LIMIT, UnitTuner, _leg_loss, _window_capacity,
                                   match_tuning_line, serpentine_path)
from pcbir.geometry import segment_distance_squared
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.syntax import CopperScriptError

from test_length_match_tuning import (TWO_LANES, _hard, _keepout, _lane, _lane_offsets, _lanes_board, _length,
                                      _sharp_bends)

WIDTH, GAP = nm_from_mm("0.14"), nm_from_mm("0.26")
SPACING = WIDTH + GAP
AMPLITUDE = nm_from_mm("0.35")
CLEARANCE = nm_from_mm("0.52")
LAYER = CopperLayer.FRONT
EMPTY = PhysicalBoard("Empty", BoardOutline.rectangle(60, 60), {}, (), ())
PAIRS = {
    "along x": (TrackSegment("A", Point.mm(5, 10), Point.mm(45, 10), WIDTH, LAYER),
                TrackSegment("B", Point.mm(5, "9.6"), Point.mm(45, "9.6"), WIDTH, LAYER)),
    # Members in opposite directions along y.
    "reversed": (TrackSegment("A", Point.mm(30, 50), Point.mm(30, 10), WIDTH, LAYER),
                 TrackSegment("B", Point.mm("30.4", 10), Point.mm("30.4", 50), WIDTH, LAYER)),
}


def _tuner(tracks, board=EMPTY, nets=("A", "B"), spacing=SPACING, **options) -> UnitTuner:
    return UnitTuner(board, RoutingClearanceIndex(board), tracks, nets, spacing, AMPLITUDE, CLEARANCE,
                     style=TuningStyle.SERPENTINE, **options)


def _paths(tracks, nets=("A", "B")) -> dict[str, list[TrackSegment]]:
    return {net: [t for t in tracks if t.net == net] for net in nets}


def _offsets(path, line: int, horizontal: bool) -> list[int]:
    """Signed distance of every corner from the original line."""
    return [(p.y_nm if horizontal else p.x_nm) - line for t in path for p in (t.start, t.end)]


@pytest.mark.parametrize("shape", PAIRS)
def test_serpentine_legs_alternate_sides_and_add_exact_equal_lengths(shape) -> None:
    tracks = PAIRS[shape]
    tuner = _tuner(tracks)
    # The leg pitch is the lane width plus the default leg gap, the larger of
    # 3 x width and the clearance.
    assert tuner.windows[0].pitch == SPACING + WIDTH + CLEARANCE
    for added in (nm_from_mm("1.5"), nm_from_mm("4.1"), nm_from_mm(7)):
        plan = tuner.plan(added)
        assert plan.reason == "" and plan.style == "serpentine" and plan.bumps == 0
        assert plan.legs == len(plan.amplitudes_nm) + 1
        assert all(nm_from_mm("0.234") < height <= AMPLITUDE for height in plan.amplitudes_nm)
        assert max(plan.amplitudes_nm) - min(plan.amplitudes_nm) <= 1    # levelled
        paths = _paths(plan.tracks)
        for net, path in paths.items():
            # Each member gains exactly the length asked for, stays continuous
            # and turns by 45 degrees at most.
            assert _track_length(tuple(path)) == _length(tracks, net) + added
            assert all(a.end == b.start for a, b in zip(path, path[1:]))
            assert _sharp_bends(path) == []
        # The members keep the pair spacing through every leg and corner.
        assert min(segment_distance_squared(a.start, a.end, b.start, b.end)
                   for a in paths["A"] for b in paths["B"]) == SPACING ** 2
        for first, second in (("A", "B"), ("B", "A")):
            assert all(SPACING ** 2 <= offset < (SPACING + 2) ** 2
                       for offset in _lane_offsets(paths[first], paths[second]))
        # The tops alternate sides: the trace crosses its original line, and
        # never rises more than the amplitude limit off it on either side.
        horizontal = shape == "along x"
        line = tracks[0].start.y_nm if horizontal else tracks[0].start.x_nm
        offsets = _offsets(paths["A"], line, horizontal)
        assert min(offsets) < 0 < max(offsets)
        assert max(abs(offset) for offset in offsets) == max(plan.amplitudes_nm)


def test_each_leg_adds_its_length_less_one_leg_loss_for_both_members() -> None:
    # Serpentines of one to five tops, at uneven heights: every member gains
    # 2 x the sum of heights less (tops + 1) leg losses, in integer nanometres.
    pitch = SPACING + WIDTH + CLEARANCE
    for heights in ((300_000,), (300_000, 280_001), (250_017, 333_333, 300_000),
                    (240_000, 350_000, 240_000, 350_000, 299_999)):
        sides = [1 if index % 2 == 0 else -1 for index in range(len(heights))]
        # Every leg is at least 240 um long: up to (240 um - d) // 2.
        for chamfer in (1, 1_000, 2_843):
            expected = 2 * sum(heights) - (len(heights) + 1) * _leg_loss(chamfer, SPACING)
            for member in (1, -1):
                corners = serpentine_path(nm_from_mm(10), pitch, SPACING, sides, heights, member, chamfer)
                points = [Point(nm_from_mm(5), 0), *(Point(x, y) for x, y in corners), Point(nm_from_mm(40), 0)]
                path = tuple(TrackSegment("A", a, b, WIDTH, LAYER) for a, b in zip(points, points[1:]) if a != b)
                assert _track_length(path) - nm_from_mm(35) == expected
    # A single net (no partner) loses 2 x (2c - round(c sqrt 2)) per leg.
    assert _leg_loss(7_000, 0) == 2 * (14_000 - 9_899)


def test_a_single_net_snakes_about_its_own_line() -> None:
    track = (TrackSegment("A", Point.mm(5, 10), Point.mm(45, 10), WIDTH, LAYER),)
    tuner = _tuner(track, nets=("A",), spacing=0)
    assert tuner.windows[0].pitch == WIDTH + CLEARANCE
    plan = tuner.plan(nm_from_mm(5))
    assert plan.style == "serpentine"
    (path,) = _paths(plan.tracks, ("A",)).values()
    assert _track_length(tuple(path)) == nm_from_mm(45)
    assert _sharp_bends(path) == []
    assert sorted({sign for sign in (1, -1) for offset in _offsets(path, nm_from_mm(10), True)
                   if offset * sign > 0}) == [-1, 1]


def test_serpentines_fall_back_to_one_sided_bumps() -> None:
    # A wall at the clearance below the pair leaves room on one side only:
    # no serpentine fits, so the unit gets R3's one-sided bumps upward.
    walled = replace(EMPTY, copper_keepouts=(_keepout("wall", 4, 7, 46, "9.33"),))
    tracks = PAIRS["along x"]
    tuner = _tuner(tracks, walled)
    assert tuner.windows == []
    plan = tuner.plan(nm_from_mm(1))
    assert plan.reason == "" and plan.style == "bumps" and plan.legs == 0 and plan.bumps
    assert min(_offsets(_paths(plan.tracks)["B"], nm_from_mm("9.6"), True)) >= 0
    assert plan.tracks == UnitTuner(walled, RoutingClearanceIndex(walled), tracks, ("A", "B"), SPACING,
                                    AMPLITUDE, CLEARANCE).tune(nm_from_mm(1))[0]
    # Too little length for tops tall enough to have 45-degree corners on
    # both members also falls back to bumps.
    small = _tuner(tracks).plan(nm_from_mm("0.2"))
    assert small.style == "bumps" and small.bumps
    # A wall on one side over part of the run: the serpentine takes the free
    # stretch, another run keeps bumps.
    partly = replace(EMPTY, copper_keepouts=(_keepout("wall", 4, 7, 25, "9.33"),))
    plan = _tuner(tracks, partly).plan(nm_from_mm(3))
    assert plan.style == "serpentine"
    assert all(t.start.x_nm > nm_from_mm(25) for t in plan.tracks if t.start.y_nm < nm_from_mm("9.6"))


def test_the_leg_count_is_bounded() -> None:
    tuner = _tuner(PAIRS["along x"], leg_limit=6, bump_limit=3)
    # Six legs hold five tops: their full capacity fits, more does not (nor
    # in the three bumps of the fallback).
    five = _window_capacity(tuner.windows[0], (AMPLITUDE,) * 5)
    plan = tuner.plan(five - five % 2)
    assert plan.reason == "" and plan.legs == 6
    plan = tuner.plan(five + 2)
    assert plan.tracks is None
    assert "the limit of 3 bumps; serpentine: insufficient tuning room" in plan.reason
    assert plan.reason.endswith("in 6 leg(s), the limit of 6 legs")
    assert MATCH_TUNING_LEG_LIMIT == 32


def test_a_declared_leg_gap_sets_the_pitch() -> None:
    tuner = _tuner(PAIRS["along x"], leg_gap_nm=nm_from_mm("0.8"))
    assert tuner.windows[0].pitch == SPACING + WIDTH + nm_from_mm("0.8")
    plan = tuner.plan(nm_from_mm(3))
    legs = sorted({t.start.x_nm for t in _paths(plan.tracks)["A"] if t.start.x_nm == t.end.x_nm})
    # Facing legs of one member are the gap plus one width apart (centres).
    assert min(b - a for a, b in zip(legs, legs[1:])) == nm_from_mm("0.8") + WIDTH


def _serpentine_lanes(**options) -> PhysicalBoard:
    board = _lanes_board(TWO_LANES, amplitude=AMPLITUDE, **options)
    return replace(board, net_routing_rules=tuple(
        replace(rule, tuning_style=TuningStyle.SERPENTINE) for rule in board.net_routing_rules))


def test_group_tuning_uses_serpentines_clear_of_neighbours_under_native_drc() -> None:
    # LANE_B is 4 mm short; walls 0.5 mm off both sides of its 0.2/0.2 pair.
    board = _serpentine_lanes(keepouts=(_keepout("above", 6, "14.8", 28, 17), _keepout("below", 6, 11, 28, "13.2")))
    result = route_critical_nets(board, route_global(board))
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned", tuning.reason
    assert tuning.skew_after_nm <= tuning.max_skew_nm and not _hard(result.board)
    (unit,) = tuning.units
    assert (unit.nets, unit.stage, unit.style, unit.bumps) == (("LANE_B_N", "LANE_B_P"), "final", "serpentine", 0)
    assert unit.legs == len(unit.amplitudes_nm) + 1 == tuning.legs
    lane = _lane(result.board.tracks, "LANE_B")
    # Pair skew is unchanged and the copper stays between the walls.
    assert _length(lane, "LANE_B_N") == _length(lane, "LANE_B_P")
    assert all(nm_from_mm("13.3") <= p.y_nm <= nm_from_mm("14.7") for t in lane for p in (t.start, t.end))
    (check,) = verify_match_groups(result.board)
    assert check.status == "pass"
    document = json.loads(result.to_json())["match_tuning"][0]
    assert document["legs"] == unit.legs
    assert document["units"] == [{"nets": ["LANE_B_N", "LANE_B_P"], "stage": "final", "style": "serpentine",
                                  "added_length_nm": unit.added_length_nm, "bumps": 0, "legs": unit.legs,
                                  "amplitudes_nm": list(unit.amplitudes_nm)}]
    line = match_tuning_line(tuning)
    assert f"LANE_B_N/LANE_B_P serpentine {unit.legs} legs up to" in line
    # Deterministic.
    again = route_critical_nets(board, route_global(board))
    assert again == result and again.to_json() == result.to_json()


def test_bumps_style_reports_and_routes_as_before() -> None:
    board = _lanes_board(TWO_LANES)
    explicit = replace(board, net_routing_rules=tuple(
        replace(rule, tuning_style=TuningStyle.BUMPS) for rule in board.net_routing_rules))
    assert explicit == board
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    (tuning,) = result.match_tuning
    assert tuning.status == "tuned" and tuning.units and not tuning.reported_units
    assert "units" not in json.loads(result.to_json())["match_tuning"][0]
    # A serpentine declared on nets outside any match group changes nothing.
    alone = replace(board, match_groups=())
    serpentine = replace(alone, net_routing_rules=tuple(
        replace(rule, tuning_style=TuningStyle.SERPENTINE) for rule in alone.net_routing_rules))
    first, second = route_critical_nets(alone, guides), route_critical_nets(serpentine, guides)
    assert (first.locked_tracks, first.nets, first.routing_fingerprint) == (
        second.locked_tracks, second.nets, second.routing_fingerprint)


# --- Language ----------------------------------------------------------------


def _source(properties: str, partner: str = 'tuning_style = "serpentine";') -> str:
    return f"""board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        constraint routing(CKP) {{
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm;
            {properties}
        }}
        constraint routing(CKN) {{
            kind = differential; partner = CKP; width = 0.14mm; pair_gap = 0.26mm; {partner}
        }}
    }}"""


JLC = PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer")


def test_tuning_properties_lower_to_the_rule_and_bind_the_digest_only_when_declared() -> None:
    board = prototype_physicalize(compile_source(_source(
        'tuning_style = "serpentine"; tuning_spacing = 0.6mm;')), JLC)
    rule = next(rule for rule in board.net_routing_rules if rule.net == "CKP")
    assert (rule.tuning_style, rule.tuning_spacing_nm) == (TuningStyle.SERPENTINE, nm_from_mm("0.6"))
    plain = prototype_physicalize(compile_source(_source("", partner="")), JLC)
    assert {rule.tuning_style for rule in plain.net_routing_rules} == {TuningStyle.BUMPS}
    bumps = prototype_physicalize(compile_source(_source('tuning_style = "bumps";', partner="")), JLC)
    assert physical_board_digest(bumps) == physical_board_digest(plain)
    assert physical_board_digest(board) != physical_board_digest(plain)


@pytest.mark.parametrize("properties, message", [
    ('tuning_style = "zigzag";', "must be \"bumps\" or \"serpentine\""),
    ("tuning_spacing = 0.5mm;", "requires tuning_style = \"serpentine\""),
    ('tuning_style = "serpentine"; tuning_spacing = 0mm;', "'tuning_spacing' must be positive"),
    ('tuning_style = "serpentine"; tuning_spacing = 0.5;', "'tuning_spacing' must be a length"),
])
def test_invalid_tuning_properties_fail_with_locations(properties: str, message: str) -> None:
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(_source(properties), "lanes.copper")
    assert error.value.code == "CMP110"
    assert error.value.location.filename == "lanes.copper"


def test_pair_members_must_agree_on_the_tuning_style() -> None:
    with pytest.raises(ValueError, match="must agree on tuning_style"):
        prototype_physicalize(compile_source(_source("", partner='tuning_style = "serpentine";')), JLC)
    with pytest.raises(ValueError, match="tuning spacing requires"):
        NetRoutingRule("A", RouteKind.CLOCK, tuning_spacing_nm=nm_from_mm("0.5"))
    with pytest.raises(ValueError):
        NetRoutingRule("A", RouteKind.CLOCK, tuning_style="zigzag")

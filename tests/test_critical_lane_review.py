"""D-PHY plan D6: the routed critical-lane review of the route and preflight reports."""
from __future__ import annotations

from dataclasses import replace
import json
from math import sqrt
from pathlib import Path
import subprocess
import sys

from pcbir import (
    BoardOutline, CopperLayer, CopperZone, FootprintPad, NetRoutingRule, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, RouteKind,
    Size, TrackSegment, Via, nm_from_mm,
)
from pcbir.critical import CriticalNetResult, CriticalRoutingResult, CriticalRoutingStatus
from pcbir.critical_review import critical_lane_review, lane_review_line
from pcbir.critical_tuning import MatchTuningResult
from pcbir.physical import NetMatchGroup, PadReference, PadShape

ROOT = Path(__file__).resolve().parents[1]
WIDTH = nm_from_mm("0.2")


def _track(net: str, start: tuple[str, str], end: tuple[str, str]) -> TrackSegment:
    return TrackSegment(net, Point.mm(*start), Point.mm(*end), WIDTH, CopperLayer.FRONT)


def _review_board() -> PhysicalBoard:
    """Two pairs on F.Cu (0.2 mm wide, 0.2 mm gap) and a single-ended net.

    Pair A runs straight from J1 to J2 at y = 10 / 10.4 mm with 1 mm breakout
    regions around its lands (cut at x = 6 and 30 mm). Pair B runs at
    y = 10.9 / 11.3 mm over x = 10..20 mm, 0.3 mm edge to edge from A_N, then
    bends 90 degrees; B_N is 0.8 mm shorter. C (a clock) bends 45 degrees and
    ends on a via. S is a signal net next to J1's breakout; GND owns a zone.
    """
    lands = PhysicalFootprint("test/pair-lands", (
        FootprintPad("1", Point.mm(0, 0), Size.mm("0.2", "0.2"), shape=PadShape.CIRCLE),
        FootprintPad("2", Point.mm(0, "0.4"), Size.mm("0.2", "0.2"), shape=PadShape.CIRCLE),
    ), Size.mm(1, 1))
    pair = dict(width_nm=WIDTH, pair_gap_nm=nm_from_mm("0.2"))
    breakout = dict(breakout_length_nm=nm_from_mm(1), breakout_gap_nm=nm_from_mm("0.1"))
    rules = (
        NetRoutingRule("A_P", RouteKind.DIFFERENTIAL, differential_partner="A_N",
                       max_skew_nm=nm_from_mm("0.5"), **pair, **breakout),
        NetRoutingRule("A_N", RouteKind.DIFFERENTIAL, differential_partner="A_P",
                       max_skew_nm=nm_from_mm("0.5"), **pair, **breakout),
        NetRoutingRule("B_P", RouteKind.DIFFERENTIAL, differential_partner="B_N",
                       max_skew_nm=nm_from_mm("0.1"), **pair),
        NetRoutingRule("B_N", RouteKind.DIFFERENTIAL, differential_partner="B_P",
                       max_skew_nm=nm_from_mm("0.1"), **pair),
        NetRoutingRule("C", RouteKind.CLOCK, width_nm=WIDTH),
    )
    tracks = (
        _track("A_P", ("5", "10"), ("6", "10")), _track("A_P", ("6", "10"), ("30", "10")),
        _track("A_P", ("30", "10"), ("31", "10")),
        _track("A_N", ("5", "10.4"), ("6", "10.4")), _track("A_N", ("6", "10.4"), ("30", "10.4")),
        _track("A_N", ("30", "10.4"), ("31", "10.4")),
        _track("B_P", ("10", "10.9"), ("20", "10.9")), _track("B_P", ("20", "10.9"), ("20", "12.9")),
        _track("B_N", ("10", "11.3"), ("19.6", "11.3")), _track("B_N", ("19.6", "11.3"), ("19.6", "12.9")),
        _track("C", ("10", "16"), ("14", "16")), _track("C", ("14", "16"), ("17", "19")),
        _track("S", ("4", "9.5"), ("5.5", "9.5")),
        # 0.2 mm from A_P, but GND owns a zone and is ignored.
        _track("GND", ("8", "9.6"), ("9", "9.6")),
    )
    outline = PolygonWithHoles(PolygonRing((Point.mm(0, 0), Point.mm(40, 0), Point.mm(40, 25),
                                            Point.mm(0, 25))))
    return PhysicalBoard(
        "LaneReview", BoardOutline.rectangle(40, 25), {lands.name: lands},
        (Placement("J1", lands.name, Point.mm(5, 10)), Placement("J2", lands.name, Point.mm(31, 10))),
        (
            PhysicalNet("A_P", (PadReference("J1", "1"), PadReference("J2", "1"))),
            PhysicalNet("A_N", (PadReference("J1", "2"), PadReference("J2", "2"))),
            *(PhysicalNet(name, ()) for name in ("B_P", "B_N", "C", "S", "GND")),
        ),
        tracks=tracks,
        vias=(Via("C", Point.mm(17, 19), nm_from_mm("0.6"), nm_from_mm("0.3"),
                  CopperLayer.FRONT, CopperLayer.BACK),),
        net_routing_rules=rules,
        zones=(CopperZone("ground", "GND", (CopperLayer.BACK,), outline),),
        match_groups=(NetMatchGroup("lanes", ("A_P", "A_N", "C"), nm_from_mm(20)),),
    )


RESULTS = (
    CriticalNetResult(("A_N", "A_P"), True, 6, 0, (nm_from_mm(26),) * 2, 0),
    CriticalNetResult(("B_N", "B_P"), True, 4, 0, (nm_from_mm("11.2"), nm_from_mm(12)), nm_from_mm("0.8")),
    CriticalNetResult(("C",), True, 2, 1, (8_242_641,), 0),
)
TUNING = (MatchTuningResult("lanes", nm_from_mm(20), "within_limit", 17_757_359, 17_757_359, ()),)


def _minimum(distance: int, neighbour: str, at: tuple[str, str], kind: str = "track") -> dict[str, object]:
    return {"distance_nm": distance, "neighbour": neighbour, "object": kind, "pad": None,
            "layer": "F.Cu", "at_nm": [nm_from_mm(at[0]), nm_from_mm(at[1])]}


def test_review_reports_lengths_skew_spacing_coupling_and_bends() -> None:
    review = critical_lane_review(_review_board(), RESULTS, TUNING)
    assert review["search_radius_nm"] == nm_from_mm(1)
    assert review["coupling_factor"] == 2 and review["sharp_bend_degrees"] == 45
    assert review["ignored_zone_nets"] == ["GND"]
    nets = {item["net"]: item for item in review["nets"]}
    assert list(nets) == ["A_N", "A_P", "B_N", "B_P", "C"]

    # Lengths are tracks only; C's via barrel is not counted.
    assert {net: item["routed_length_nm"] for net, item in nets.items()} == {
        "A_N": nm_from_mm(26), "A_P": nm_from_mm(26), "B_N": nm_from_mm("11.2"),
        "B_P": nm_from_mm(12), "C": nm_from_mm(4) + 4_242_641}
    assert {net: item["via_count"] for net, item in nets.items()} == {
        "A_N": 0, "A_P": 0, "B_N": 0, "B_P": 0, "C": 1}
    assert all(item["layers"] == ["F.Cu"] for item in nets.values())
    assert {net: item["partner"] for net, item in nets.items()} == {
        "A_N": "A_P", "A_P": "A_N", "B_N": "B_P", "B_P": "B_N", "C": None}
    assert [net for net, item in nets.items() if item["breakout"]] == ["A_N", "A_P"]

    assert review["pairs"] == [
        {"nets": ["A_N", "A_P"], "lengths_nm": [nm_from_mm(26)] * 2, "skew_nm": 0,
         "max_skew_nm": nm_from_mm("0.5"), "status": "pass"},
        {"nets": ["B_N", "B_P"], "lengths_nm": [nm_from_mm("11.2"), nm_from_mm(12)],
         "skew_nm": nm_from_mm("0.8"), "max_skew_nm": nm_from_mm("0.1"), "status": "fail"},
    ]
    (group,) = review["match_groups"]
    assert group == {
        "id": "lanes", "max_skew_nm": nm_from_mm(20), "skew_nm": 17_757_359, "status": "pass",
        "members": [
            {"net": "A_P", "length_nm": nm_from_mm(26), "routed": True, "shortfall_nm": 0},
            {"net": "A_N", "length_nm": nm_from_mm(26), "routed": True, "shortfall_nm": 0},
            {"net": "C", "length_nm": 8_242_641, "routed": True, "shortfall_nm": 17_757_359},
        ],
        "tuning_status": "within_limit",
    }

    # A_P: S is 0.3 mm from the J1 breakout piece and 0.5071 mm (diagonally)
    # from the channel; B_P is 0.7 mm away. The partner and GND are ignored.
    assert nets["A_P"]["spacing"] == {
        "inside_breakout": {"critical": None, "signal": _minimum(300_000, "S", ("5", "10"))},
        "outside_breakout": {"critical": _minimum(700_000, "B_P", ("10", "10")),
                             "signal": _minimum(round(sqrt(2) * 500_000) - 200_000, "S", ("6", "10"))},
    }
    assert nets["A_N"]["spacing"] == {
        "inside_breakout": {"critical": None, "signal": _minimum(700_000, "S", ("5", "10.4"))},
        "outside_breakout": {"critical": _minimum(300_000, "B_P", ("10", "10.4")),
                             "signal": _minimum(round(sqrt(1.06) * 1_000_000) - 200_000, "S", ("6", "10.4"))},
    }
    # Without breakout properties, all copper is outside; S is beyond 1 mm.
    assert nets["B_P"]["spacing"] == {
        "inside_breakout": {"critical": None, "signal": None},
        "outside_breakout": {"critical": _minimum(300_000, "A_N", ("10", "10.9")), "signal": None},
    }
    assert nets["B_N"]["spacing"]["outside_breakout"] == {
        "critical": _minimum(700_000, "A_N", ("10", "11.3")), "signal": None}
    assert nets["C"]["spacing"] == {region: {"critical": None, "signal": None}
                                    for region in ("inside_breakout", "outside_breakout")}

    # Coupled: closer than 2 x 0.2 mm to the other pair, outside the breakout.
    # A_N's channel is within reach of B_P's 10 mm run plus sqrt(0.6^2 - 0.5^2)
    # mm past each end; B_P couples over its run and 0.1 mm of its bend leg.
    assert {net: (item["coupling_threshold_nm"], item["coupled_length_nm"], item["coupled_nets"])
            for net, item in nets.items()} == {
        "A_N": (400_000, round(nm_from_mm(10) + 2 * sqrt(600_000 ** 2 - 500_000 ** 2)), ["B_P"]),
        "A_P": (400_000, 0, []),
        "B_N": (400_000, 0, []),
        "B_P": (400_000, nm_from_mm("10.1"), ["A_N"]),
        "C": (None, None, []),
    }

    # Straight joins are 0 degrees; B turns 90 degrees, C 45 (not sharp).
    assert nets["A_P"]["bends"] == {"sharpest_degrees": 0.0, "sharp_count": 0, "sharp": []}
    assert nets["B_P"]["bends"] == {"sharpest_degrees": 90.0, "sharp_count": 1, "sharp": [
        {"at_nm": [nm_from_mm(20), nm_from_mm("10.9")], "layer": "F.Cu", "degrees": 90.0}]}
    assert nets["B_N"]["bends"]["sharp"] == [
        {"at_nm": [nm_from_mm("19.6"), nm_from_mm("11.3")], "layer": "F.Cu", "degrees": 90.0}]
    assert nets["C"]["bends"] == {"sharpest_degrees": 45.0, "sharp_count": 0, "sharp": []}


def test_review_json_shape() -> None:
    review = critical_lane_review(_review_board(), RESULTS, TUNING)
    assert json.loads(json.dumps(review)) == review
    assert set(review) == {"search_radius_nm", "coupling_factor", "sharp_bend_degrees",
                           "ignored_zone_nets", "nets", "pairs", "match_groups"}
    lane_keys = {"net", "group", "connected", "routed_length_nm", "layer_lengths_nm", "layers",
                 "via_count", "estimated_delay_ps", "effective_permittivity",
                 "delay_evidence_grade", "delay_reason"}
    for item in review["nets"]:
        assert set(item) == lane_keys | {"partner", "breakout", "spacing", "coupling_threshold_nm",
                                         "coupled_length_nm", "coupled_nets", "bends"}
        assert set(item["spacing"]) == {"inside_breakout", "outside_breakout"}
        for region in item["spacing"].values():
            assert set(region) == {"critical", "signal"}
            for minimum in region.values():
                assert minimum is None or set(minimum) == {"distance_nm", "neighbour", "object",
                                                           "pad", "layer", "at_nm"}
        assert set(item["bends"]) == {"sharpest_degrees", "sharp_count", "sharp"}
    assert all(set(pair) == {"nets", "lengths_nm", "skew_nm", "max_skew_nm", "status"}
               for pair in review["pairs"])
    assert set(review["match_groups"][0]) == {"id", "max_skew_nm", "skew_nm", "status", "members",
                                              "tuning_status"}
    # Results select the reviewed nets; neighbour classes come from the rules.
    partial = critical_lane_review(_review_board(), RESULTS[:1])
    assert [item["net"] for item in partial["nets"]] == ["A_N", "A_P"]
    assert partial["nets"] == review["nets"][:2]
    empty = critical_lane_review(_review_board(), ())
    assert empty["nets"] == [] and empty["pairs"] == []
    assert empty["match_groups"][0]["tuning_status"] is None


def test_review_names_pad_neighbours_and_ignores_pads_without_a_net() -> None:
    board = _review_board()
    # J3.2 (net T) is 0.4 mm from A_P's channel; J4's lands have no net and
    # one of them is 0.15 mm away.
    board = replace(board, placements=(*board.placements,
                                       Placement("J3", "test/pair-lands", Point.mm(12, 9)),
                                       Placement("J4", "test/pair-lands", Point.mm(16, "9.25"))),
                    nets=(*board.nets, PhysicalNet("T", (PadReference("J3", "2"),))))
    review = critical_lane_review(board, RESULTS)
    a_p = next(item for item in review["nets"] if item["net"] == "A_P")
    assert a_p["spacing"]["outside_breakout"]["signal"] == {
        "distance_nm": 400_000, "neighbour": "T", "object": "pad", "pad": "J3.2",
        "layer": "F.Cu", "at_nm": [nm_from_mm(12), nm_from_mm(10)]}


def test_console_line_summarises_the_review() -> None:
    review = critical_lane_review(_review_board(), RESULTS, TUNING)
    assert lane_review_line(review) == (
        "CRITICAL LANES: 5 nets; pairs=2, worst skew=0.800 mm, over max_skew=1; "
        "match groups=1, over max_skew=0; min spacing outside breakout: "
        "critical=0.300 mm (A_N to B_P), signal=0.507 mm (A_P to S); "
        "coupled length=20.763 mm; bends>45deg=2 (sharpest 90.0 deg)")
    single = critical_lane_review(_review_board(), RESULTS[2:])
    assert lane_review_line(single) == (
        "CRITICAL LANES: 1 nets; pairs=0, worst skew=n/a, over max_skew=0; "
        "match groups=1, over max_skew=0; min spacing outside breakout: "
        "critical=none within 1.000 mm, signal=none within 1.000 mm; "
        "coupled length=0.000 mm; bends>45deg=0 (sharpest 45.0 deg)")


def test_preflight_reports_the_review_of_the_critical_stage(tmp_path, monkeypatch, capsys) -> None:
    import pcbir.critical_preflight as preflight

    board = _review_board()
    routed = CriticalRoutingResult(CriticalRoutingStatus.SUCCESS, board, RESULTS, board.tracks,
                                   board.vias, "global", "critical", match_tuning=TUNING)
    monkeypatch.setattr(preflight, "route_critical_nets", lambda *args, **kwargs: routed)
    report = tmp_path / "report.json"
    preflight.main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
                    "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
                    "--report", str(report)])
    data = json.loads(report.read_text())
    expected = critical_lane_review(board, RESULTS, TUNING)
    assert data["critical_lane_review"] == expected
    assert lane_review_line(expected) in capsys.readouterr().out


def test_route_board_reports_the_review_of_the_exported_copper(tmp_path) -> None:
    source = tmp_path / "board.copper"
    source.write_text("""board Lanes {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component R2: RESISTOR { footprint = "0402"; }
        net DP { R1.1; R2.1; }
        net DN { R1.2; R2.2; }
        constraint routing(DP) {
            kind = differential; partner = DN; width = 0.2mm; pair_gap = 0.2mm; max_skew = 1mm;
        }
        constraint routing(DN) {
            kind = differential; partner = DP; width = 0.2mm; pair_gap = 0.2mm; max_skew = 1mm;
        }
    }
    """, encoding="utf-8")
    report = tmp_path / "route-report.json"
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "route-board", str(source), "--allow-proxy-footprints",
         "--layers", "2", "--fab-profile", "generic", "--candidates", "1", "--passes", "1",
         "--pitch-mm", "1", "--report", str(report)],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )
    assert result.returncode in {0, 1}, result.stdout + result.stderr
    review = json.loads(report.read_text(encoding="utf-8"))["critical_lane_review"]
    assert [item["net"] for item in review["nets"]] == ["DN", "DP"]
    (pair,) = review["pairs"]
    assert pair["nets"] == ["DN", "DP"] and pair["status"] == "pass"
    assert pair["lengths_nm"] == [item["routed_length_nm"] for item in review["nets"]]
    assert lane_review_line(review) in result.stdout.splitlines()

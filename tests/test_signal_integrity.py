"""Signal-integrity screening (D-PHY plan L2, L3, L5, L7)."""

from dataclasses import replace
from decimal import Decimal
import json

import pytest

from pcbir import (
    AnalysisStatus,
    CopperLayer,
    CopperZone,
    EvidenceGrade,
    PolygonRing,
    PolygonWithHoles,
    PrototypePhysicalOptions,
    TrackSegment,
    ZoneFillResult,
    compile_design_source,
    edge_coupled_microstrip_impedance,
    edge_coupled_stripline_impedance,
    effective_permittivity,
    layer_geometry,
    layer_group_report,
    layer_group_warnings,
    microstrip_impedance,
    nm_from_mm,
    Point,
    prototype_physicalize,
    reference_plane_warnings,
    return_path_continuity,
    return_path_coverage,
    return_path_report,
    screen_impedance,
    si_check,
    stripline_impedance,
)
from pcbir.cli import main


def mm(value: str) -> int:
    return nm_from_mm(value)


# --- closed-form screening ---------------------------------------------------


def test_jlcpcb_six_layer_outer_pair_screens_near_100_ohm_differential() -> None:
    # JLCPCB-style six-layer outer layer: 0.10 mm 3313 prepreg, er 4.1, 0.035 mm copper, 0.14/0.26 mm.
    single = microstrip_impedance(mm("0.14"), mm("0.035"), mm("0.10"), Decimal("4.1"))
    pair = edge_coupled_microstrip_impedance(mm("0.14"), mm("0.26"), mm("0.035"), mm("0.10"),
                                             Decimal("4.1"), target_ohms=Decimal(100),
                                             tolerance_ohms=Decimal(10))
    assert Decimal("51") < single.value < Decimal("53")
    assert Decimal("98") < pair.value < Decimal("102")
    assert pair.status is AnalysisStatus.PASS
    assert pair.evidence_grade is EvidenceGrade.SCREENING
    # A 0.10/0.30 mm geometry over the same prepreg screens well above 100 ohm.
    first = edge_coupled_microstrip_impedance(mm("0.10"), mm("0.30"), mm("0.035"), mm("0.10"), Decimal("4.1"))
    assert Decimal("113") < first.value < Decimal("118")


def test_coupling_lowers_differential_impedance_as_the_gap_closes() -> None:
    loose = edge_coupled_microstrip_impedance(mm("0.14"), mm("1.0"), mm("0.035"), mm("0.1"), Decimal("4.1"))
    tight = edge_coupled_microstrip_impedance(mm("0.14"), mm("0.1"), mm("0.035"), mm("0.1"), Decimal("4.1"))
    single = microstrip_impedance(mm("0.14"), mm("0.035"), mm("0.1"), Decimal("4.1")).value
    assert tight.value < loose.value < 2 * single
    assert any("outside the IPC-2141A" in note for note in loose.validity)
    with pytest.raises(ValueError, match="gap"):
        edge_coupled_microstrip_impedance(mm("0.14"), 0, mm("0.035"), mm("0.1"), Decimal("4.1"))


def test_symmetric_and_offset_stripline_estimates() -> None:
    symmetric = stripline_impedance(mm("0.1"), mm("0.0175"), mm("0.2"), mm("0.2"), Decimal("4.2"))
    offset = stripline_impedance(mm("0.1"), mm("0.0175"), mm("0.1"), mm("0.5"), Decimal("4.2"))
    assert Decimal("60") < symmetric.value < Decimal("63")
    assert "symmetric" in symmetric.claim_scope and "offset" in offset.claim_scope
    # A trace nearer one plane couples more strongly to it than a centred trace.
    centred = stripline_impedance(mm("0.1"), mm("0.0175"), mm("0.3"), mm("0.3"), Decimal("4.2"))
    assert offset.value < centred.value
    pair = edge_coupled_stripline_impedance(mm("0.1"), mm("0.2"), mm("0.0175"), mm("0.2"), mm("0.2"),
                                            Decimal("4.2"))
    assert pair.value < 2 * symmetric.value
    with pytest.raises(ValueError, match="too wide"):
        stripline_impedance(mm("2"), mm("0.0175"), mm("0.05"), mm("0.05"), Decimal("4.2"))


def test_return_path_coverage_reports_uncovered_segments(six_layer_board) -> None:
    board = six_layer_board
    zone = CopperZone("gnd", "GND", (CopperLayer.INTERNAL_1,),
                      PolygonWithHoles(PolygonRing(_rectangle(0, 0, 40, 30))))
    fill = ZoneFillResult("gnd", CopperLayer.INTERNAL_1, "a" * 64, "test", "1",
                          (PolygonWithHoles(PolygonRing(_rectangle(0, 0, 20, 30))),))
    covered = TrackSegment("CKP", Point.mm(2, 5), Point.mm(10, 5), mm("0.14"), CopperLayer.FRONT)
    exposed = TrackSegment("CKP", Point.mm(22, 5), Point.mm(30, 5), mm("0.14"), CopperLayer.FRONT)
    filled = replace(board, zones=(zone,), zone_fills=(fill,), tracks=(covered, exposed))
    result, uncovered = return_path_coverage(filled, "CKP", ("GND",))
    assert result.value == Decimal("0.5") and result.status is AnalysisStatus.FAIL
    assert uncovered == (exposed,)
    assert return_path_continuity(filled, "CKP", "GND") == result
    report = {item.net: item for item in return_path_report(filled)}
    assert report["CKP"].reference_nets == ("GND",)
    assert report["CKP"].to_dict()["uncovered_segments"] == [
        {"layer": "F.Cu", "start": [mm("22"), mm("5")], "end": [mm("30"), mm("5")], "width_nm": mm("0.14")}]
    assert report["CKN"].result.status is AnalysisStatus.INDETERMINATE  # not routed
    unfilled = replace(filled, zone_fills=())
    assert all(item.result.status is AnalysisStatus.INDETERMINATE for item in return_path_report(unfilled))


def _rectangle(x0, y0, x1, y1):
    return (Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1))


# --- board-level screening ---------------------------------------------------


STACKUP = """
    stackup {
        copper F.Cu { thickness = 0.035mm; }
        dielectric P1 { thickness = 0.1mm; er = 4.1; material = "3313"; }
        copper In1.Cu { thickness = 0.0175mm; }
        dielectric C1 { thickness = 0.55mm; er = 4.6; }
        copper In2.Cu { thickness = 0.0175mm; }
        dielectric P2 { thickness = 0.1mm; er = 4.1; }
        copper In3.Cu { thickness = 0.0175mm; }
        dielectric C2 { thickness = 0.55mm; er = 4.6; }
        copper In4.Cu { thickness = 0.0175mm; }
        dielectric P3 { thickness = 0.1mm; er = 4.1; }
        copper B.Cu { thickness = 0.035mm; }
    }
"""


def si_source(constraints: str, stackup: str = STACKUP) -> str:
    return f"""board Dphy {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        component R3: RESISTOR {{ footprint = "0402"; }}
        component R4: RESISTOR {{ footprint = "0402"; }}
        component R5: RESISTOR {{ footprint = "0402"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        net D0P {{ R3.1; R4.1; }}
        net D0N {{ R3.2; R4.2; }}
        net GND {{ R5.1; }}
        net RF {{ R5.2; }}
        mechanical {{
            outline rectangle {{ width = 40mm; height = 30mm; }}
            {stackup}
        }}
        {constraints}
    }}"""


PAIRS = """
    constraint copper_zone(GND) { layers = "In1.Cu,In4.Cu"; }
    constraint routing(CKP) { kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm;
        allowed_layers = "F.Cu"; target_impedance_ohms = 100; target_single_ended_ohms = 50; layer_group = "csi"; }
    constraint routing(CKN) { kind = differential; partner = CKP; width = 0.14mm; pair_gap = 0.26mm;
        allowed_layers = "F.Cu"; target_impedance_ohms = 100; target_single_ended_ohms = 50; layer_group = "csi"; }
    constraint routing(D0P) { kind = differential; partner = D0N; width = 0.10mm; pair_gap = 0.30mm;
        allowed_layers = "F.Cu,In2.Cu"; target_impedance_ohms = 100; impedance_tolerance_percent = 5;
        layer_group = "csi"; }
    constraint routing(D0N) { kind = differential; partner = D0P; width = 0.10mm; pair_gap = 0.30mm;
        allowed_layers = "F.Cu,In2.Cu"; target_impedance_ohms = 100; impedance_tolerance_percent = 5;
        layer_group = "csi"; }
    constraint length_match(CKP, CKN, D0P, D0N) { id = "csi-lanes"; max_skew = 1.5mm; }
"""

SIX = PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer")


@pytest.fixture
def six_layer_board():
    return prototype_physicalize(compile_design_source(si_source(PAIRS)), SIX)


def test_layer_geometry_follows_the_declared_stackup(six_layer_board) -> None:
    stackup = six_layer_board.stackup
    top = layer_geometry(stackup, CopperLayer.FRONT)
    assert (top.structure, top.dielectric_height_nm, top.relative_permittivity) == (
        "microstrip", mm("0.1"), Decimal("4.1"))
    inner = layer_geometry(stackup, CopperLayer.INTERNAL_2)
    assert (inner.structure, inner.height_above_nm, inner.height_below_nm) == (
        "stripline", mm("0.55"), mm("0.1"))
    assert inner.relative_permittivity == Decimal("4.523077")  # thickness-weighted
    bottom = layer_geometry(stackup, CopperLayer.BACK)
    assert bottom.height_above_nm == mm("0.1") and bottom.height_below_nm is None
    assert Decimal(1) < effective_permittivity(stackup, CopperLayer.FRONT, mm("0.14")) < Decimal("4.1")
    assert effective_permittivity(stackup, CopperLayer.INTERNAL_2, mm("0.1")) == inner.relative_permittivity


def test_impedance_screening_checks_every_allowed_layer(six_layer_board) -> None:
    estimates, warnings = screen_impedance(six_layer_board)
    by_key = {(item.nets, item.layer): item for item in estimates}
    clock = by_key[(("CKN", "CKP"), "F.Cu")]
    assert clock.status is AnalysisStatus.PASS
    assert Decimal("98") < clock.differential_ohms < Decimal("102")
    assert Decimal("51") < clock.single_ended_ohms < Decimal("53")
    assert (clock.target_differential_ohms, clock.target_single_ended_ohms, clock.tolerance_percent) == (
        100, 50, Decimal(10))
    lane_top = by_key[(("D0N", "D0P"), "F.Cu")]
    lane_inner = by_key[(("D0N", "D0P"), "In2.Cu")]
    assert lane_top.status is AnalysisStatus.FAIL and lane_top.target_single_ended_ohms is None
    assert lane_inner.structure == "offset_stripline"
    assert [(item.code, item.nets, item.layers) for item in warnings] == [
        ("SI-IMPEDANCE", ("D0N", "D0P"), ("F.Cu",))]
    assert "differential estimate 115.5 ohm is outside 100 ohm ±5%" in warnings[0].message


def test_single_ended_targets_and_missing_stackup_are_reported() -> None:
    board = prototype_physicalize(compile_design_source(si_source("""
        constraint routing(RF) { kind = rf_feed; width = 0.2mm; allowed_layers = "F.Cu";
            target_impedance_ohms = 50; }
        constraint routing(CKP) { kind = clock; width = 0.14mm; allowed_layers = "F.Cu";
            target_single_ended_ohms = 50; }
    """)), SIX)
    estimates, warnings = screen_impedance(board)
    rf = next(item for item in estimates if item.nets == ("RF",))
    assert rf.differential_ohms is None and rf.target_single_ended_ohms == 50
    assert rf.status is AnalysisStatus.FAIL  # 0.2 mm over 0.1 mm screens near 40 ohm
    assert next(item for item in estimates if item.nets == ("CKP",)).status is AnalysisStatus.PASS
    assert [item.nets for item in warnings] == [("RF",)]

    bare = prototype_physicalize(compile_design_source(si_source(PAIRS, stackup="")), SIX)
    estimates, warnings = screen_impedance(bare)
    assert estimates == ()
    assert [(item.code, item.nets) for item in warnings] == [
        ("SI-NO-STACKUP", ("CKN", "CKP")), ("SI-NO-STACKUP", ("D0N", "D0P"))]
    assert "100 ohm differential, 50 ohm single-ended" in warnings[0].message


def test_reference_plane_adjacency_warnings() -> None:
    constraints = """
        constraint copper_zone(GND) { layers = "In1.Cu"; }
        constraint routing(CKP) { kind = differential; partner = CKN; pair_gap = 0.2mm;
            allowed_layers = "F.Cu,In3.Cu,B.Cu"; }
        constraint routing(CKN) { kind = differential; partner = CKP; pair_gap = 0.2mm;
            allowed_layers = "F.Cu,In3.Cu,B.Cu"; }
        constraint routing(D0P) { kind = clock; allowed_layers = "In2.Cu"; }
        constraint routing(RF) { kind = general; allowed_layers = "B.Cu"; }
    """
    board = prototype_physicalize(compile_design_source(si_source(constraints)), SIX)
    warnings = reference_plane_warnings(board)
    assert [(item.code, item.nets, item.layers) for item in warnings] == [
        ("SI-NO-REFERENCE-PLANE", ("CKN", "CKP"), ("In3.Cu", "B.Cu"))]
    assert "In3.Cu (adjacent: In2.Cu, In4.Cu)" in warnings[0].message
    # A plane on In4.Cu covers both In3.Cu and B.Cu.
    zones = replace(board.zones[0], id="gnd2", layers=(CopperLayer.INTERNAL_4,))
    assert reference_plane_warnings(replace(board, zones=(*board.zones, zones))) == ()


def test_layer_groups_pre_and_post_route(six_layer_board) -> None:
    warnings = layer_group_warnings(six_layer_board)
    assert [(item.code, item.nets) for item in warnings] == [
        ("SI-LAYER-GROUP", ("CKN", "CKP", "D0N", "D0P"))]
    assert "CKN: F.Cu; CKP: F.Cu; D0N: F.Cu,In2.Cu; D0P: F.Cu,In2.Cu" in warnings[0].message
    width = mm("0.14")
    routed = replace(six_layer_board, tracks=(
        TrackSegment("CKP", Point.mm(2, 2), Point.mm(12, 2), width, CopperLayer.FRONT),
        TrackSegment("D0P", Point.mm(2, 6), Point.mm(4, 6), width, CopperLayer.FRONT),
        TrackSegment("D0P", Point.mm(4, 6), Point.mm(14, 6), width, CopperLayer.INTERNAL_2),
    ))
    reports, split = layer_group_report(routed)
    members = {member.net: member for member in reports[0].members}
    assert members["CKP"].routed_layers == ("F.Cu",) and members["CKP"].main_layer == "F.Cu"
    assert members["D0P"].routed_layers == ("F.Cu", "In2.Cu") and members["D0P"].main_layer == "In2.Cu"
    assert members["CKN"].main_layer is None
    assert not reports[0].consistent
    assert [(item.code, item.layers) for item in split] == [("SI-LAYER-GROUP-SPLIT", ("F.Cu", "In2.Cu"))]
    single = prototype_physicalize(compile_design_source(si_source(
        'constraint routing(CKP) { layer_group = "lonely"; }')), SIX)
    assert "single member" in layer_group_warnings(single)[0].message


def test_si_check_report_is_deterministic_screening(six_layer_board) -> None:
    report = si_check(six_layer_board)
    assert report.stackup_declared and report.thickness_nm == mm("1.54")
    assert [item.code for item in report.warnings] == ["SI-IMPEDANCE", "SI-LAYER-GROUP"]
    assert report.match_groups[0].status == "incomplete"
    document = json.loads(report.to_json())
    assert document["schema"] == "copperscript-si-check/v0.1"
    assert document["evidence_grade"] == "screening"
    assert document["match_groups"][0]["id"] == "csi-lanes"
    assert document["layer_groups"] == [{"name": "csi", "nets": ["CKN", "CKP", "D0N", "D0P"]}]
    assert report.to_json() == si_check(six_layer_board).to_json()
    lines = report.lines()
    assert lines[0].startswith("SI screening for Dphy: 6 copper layers, declared stack-up 1.54 mm")
    assert lines[-1] == "SI check: 2 warning(s)"


def test_si_check_command(tmp_path, capsys) -> None:
    path = tmp_path / "board.copper"
    path.write_text(si_source(PAIRS), encoding="utf-8")
    arguments = [str(path), "--allow-proxy-footprints", "--layers", "6", "--fab-profile", "jlcpcb-six-layer"]
    assert main(["si-check", *arguments]) == 0  # warnings never fail the command
    output = capsys.readouterr().out
    assert "IMPEDANCE CKN,CKP F.Cu microstrip w=0.14mm g=0.26mm: diff 99.7 ohm" in output
    assert "WARNING SI-IMPEDANCE: D0N/D0P on F.Cu" in output
    assert "MATCH GROUP csi-lanes: 4 nets, max skew 1.5 mm" in output
    assert main(["si-check", *arguments, "--json"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert [item["code"] for item in document["warnings"]] == ["SI-IMPEDANCE", "SI-LAYER-GROUP"]
    assert main(["si-check", str(path), "--allow-proxy-footprints", "--layers", "4",
                 "--fab-profile", "jlcpcb-four-layer"]) == 2
    assert "stackup declares 6 copper layers" in capsys.readouterr().out
    broken = tmp_path / "broken.copper"
    broken.write_text(si_source("constraint length_match(CKP) { max_skew = 1mm; }"), encoding="utf-8")
    assert main(["si-check", str(broken), "--allow-proxy-footprints", "--layers", "6"]) == 2
    assert "CMP111" in capsys.readouterr().out

"""Routing-intent language for D-PHY routing (plan L4 length matching, L5 layer
groups, L6 breakout properties) and DRC-LENGTH-MATCH verification."""

from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    DesignRules,
    FootprintPad,
    NetMatchGroup,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    PrototypePhysicalOptions,
    RouteKind,
    Size,
    TrackSegment,
    compile_source,
    nm_from_mm,
    normalize_constraints,
    physical_board_digest,
    prototype_physicalize,
    run_physical_drc,
    verify_match_groups,
)
from pcbir.syntax import CopperScriptError


def board(constraints: str) -> str:
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
        {constraints}
    }}"""


JLC = PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer")


# --- L6 breakout + L2/L5 routing properties ---------------------------------


def test_signal_integrity_routing_properties_lower_to_the_rule() -> None:
    physical = prototype_physicalize(compile_source(board("""
        constraint routing(CKP) {
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm; clearance = 0.3mm;
            target_impedance_ohms = 100; target_single_ended_ohms = 50; impedance_tolerance_percent = 7.5;
            layer_group = "csi-src";
            breakout_length = 1.2mm; breakout_width = 0.1mm; breakout_gap = 0.15mm; breakout_clearance = 0.1mm;
        }""")), JLC)
    rule = physical.net_routing_rules[0]
    assert rule.target_single_ended_ohms == 50
    assert rule.impedance_tolerance_percent == Decimal("7.5")
    assert rule.effective_impedance_tolerance_percent == Decimal("7.5")
    assert rule.layer_group == "csi-src"
    assert (rule.breakout_length_nm, rule.breakout_width_nm, rule.breakout_gap_nm,
            rule.breakout_clearance_nm) == (nm_from_mm("1.2"), nm_from_mm("0.1"),
                                            nm_from_mm("0.15"), nm_from_mm("0.1"))
    plain = replace(rule, layer_group=None, breakout_length_nm=None, breakout_width_nm=None,
                    breakout_gap_nm=None, breakout_clearance_nm=None)
    assert plain.effective_impedance_tolerance_percent == Decimal("7.5")
    assert replace(plain, impedance_tolerance_percent=None).effective_impedance_tolerance_percent == 10
    # New intent is digest-bound only when declared.
    assert physical_board_digest(physical) != physical_board_digest(
        replace(physical, net_routing_rules=(replace(plain, impedance_tolerance_percent=None,
                                                     target_single_ended_ohms=None),)))


@pytest.mark.parametrize("properties, message", [
    ("target_impedance_ohms = 100; target_single_ended_ohms = 50.5;", "positive integer"),
    ("target_impedance_ohms = 100; target_single_ended_ohms = 0;", "positive integer"),
    ("target_impedance_ohms = 100; impedance_tolerance_percent = 0;", "greater than 0 and below 100"),
    ("target_impedance_ohms = 100; impedance_tolerance_percent = 100;", "greater than 0 and below 100"),
    ("target_impedance_ohms = 100; impedance_tolerance_percent = 5mm;", "must be a number"),
    ("impedance_tolerance_percent = 5;", "requires 'target_impedance_ohms'"),
    ('layer_group = "";', "nonempty name"),
    ("breakout_width = 0.1mm;", "require a positive 'breakout_length'"),
    ("breakout_length = 1mm;", "requires 'breakout_width'"),
    ("breakout_length = 0mm; breakout_width = 0.1mm;", "'breakout_length' must be positive"),
    ("breakout_length = 1mm; breakout_width = 0.1;", "'breakout_width' must be a length"),
    ("breakout_length = 1mm; breakout_width = 0.2mm;", "may only relax 'width'"),
    ("breakout_length = 1mm; breakout_gap = 0.3mm;", "may only relax 'pair_gap'"),
    ("breakout_length = 1mm; breakout_clearance = 0.4mm;", "may only relax 'clearance'"),
])
def test_invalid_signal_integrity_properties_fail_with_locations(properties: str, message: str) -> None:
    source = board(f"""
        constraint routing(CKP) {{
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm; clearance = 0.3mm;
            {properties}
        }}""")
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(source, "lanes.copper")
    assert error.value.code == "CMP110"
    assert error.value.location.filename == "lanes.copper"
    assert error.value.location.line == 12  # the routing constraint declaration


def test_breakout_gap_requires_a_pair_gap() -> None:
    with pytest.raises(CopperScriptError, match="requires a differential 'pair_gap'"):
        compile_source(board("constraint routing(CKP) { breakout_length = 1mm; breakout_gap = 0.1mm; }"))


def test_breakout_values_relax_board_defaults_but_respect_fabrication_minimums() -> None:
    # No declared width/clearance: the board defaults are the effective profile.
    relaxed = prototype_physicalize(compile_source(board(
        "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.1mm; breakout_clearance = 0.09mm; }"
    )), JLC)
    assert relaxed.net_routing_rules[0].breakout_width_nm == nm_from_mm("0.1")
    with pytest.raises(ValueError, match="breakout width is below the board minimum track width"):
        prototype_physicalize(compile_source(board(
            "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.1mm; }")))
    with pytest.raises(ValueError, match="breakout clearance may only relax"):
        prototype_physicalize(compile_source(board(
            "constraint routing(CKP) { breakout_length = 1mm; breakout_clearance = 0.1mm; }")), JLC)
    with pytest.raises(ValueError, match="breakout width may only relax"):
        prototype_physicalize(compile_source(board(
            "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.21mm; }")), JLC)


def test_net_routing_rule_validates_signal_intent_directly() -> None:
    base = NetRoutingRule("A", RouteKind.DIFFERENTIAL, differential_partner="B",
                          width_nm=nm_from_mm("0.14"), pair_gap_nm=nm_from_mm("0.26"))
    with pytest.raises(ValueError, match="breakout gap may only relax"):
        replace(base, breakout_length_nm=1, breakout_gap_nm=nm_from_mm("0.3"))
    with pytest.raises(ValueError, match="requires a positive breakout_length"):
        replace(base, breakout_width_nm=nm_from_mm("0.1"))
    with pytest.raises(ValueError, match="impedance tolerance requires"):
        replace(base, impedance_tolerance_percent=Decimal(5))
    with pytest.raises(ValueError, match="must agree"):
        NetRoutingRule("A", RouteKind.RF_FEED, target_impedance_ohms=50, target_single_ended_ohms=45)
    assert NetRoutingRule("A", RouteKind.RF_FEED, target_impedance_ohms=50,
                          target_single_ended_ohms=50).target_single_ended_ohms == 50


# --- L4 length matching ------------------------------------------------------


def test_length_match_lowers_to_a_net_match_group() -> None:
    electrical = compile_source(board("""
        constraint length_match(CKP, CKN, D0P, D0N) { id = "csi-src-lanes"; max_skew = 1.5mm; }
    """))
    physical = prototype_physicalize(electrical)
    assert physical.match_groups == (
        NetMatchGroup("csi-src-lanes", ("CKP", "CKN", "D0P", "D0N"), nm_from_mm("1.5")),)
    # Net targets are not mistaken for omitted placement targets.
    assert not physical.metadata.get("omitted_constraint_targets")
    normalized = normalize_constraints(electrical.constraints)[0]
    assert (normalized.id, normalized.property, normalized.domain, normalized.verifier,
            normalized.consumers) == ("csi-src-lanes", "route.length_match", "routing",
                                      "DRC-LENGTH-MATCH", ("physical_drc",))


def test_length_match_without_id_gets_a_deterministic_id() -> None:
    physical = prototype_physicalize(compile_source(board(
        "constraint length_match(CKP, CKN) { max_skew = 0.15mm; }")))
    assert physical.match_groups[0].id == "length_match:0"


@pytest.mark.parametrize("declaration, message", [
    ("constraint length_match(CKP) { max_skew = 1mm; }", "at least two nets"),
    ("constraint length_match(CKP, CKP) { max_skew = 1mm; }", "more than once"),
    ("constraint length_match(CKP, R1.1) { max_skew = 1mm; }", "nets, not pins"),
    ("constraint length_match(CKP, CKN) { max_skew = 1mm; tolerance = 1mm; }", "unknown length_match parameter 'tolerance'"),
    ("constraint length_match(CKP, CKN) { id = \"x\"; }", "requires 'max_skew'"),
    ("constraint length_match(CKP, CKN) { max_skew = 1; }", "must be a length"),
    ("constraint length_match(CKP, CKN) { max_skew = -1mm; }", "must be positive"),
])
def test_invalid_length_match_fails_with_a_location(declaration: str, message: str) -> None:
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(board(declaration), "lanes.copper")
    assert error.value.code == "CMP111"
    assert error.value.location.filename == "lanes.copper"


@pytest.mark.parametrize("declarations, message", [
    ("constraint length_match(CKP, NOPE) { max_skew = 1mm; }", "unknown net 'NOPE'"),
    ("constraint length_match(CKP, CKN) { max_skew = 1mm; }"
     "constraint length_match(CKN, D0P) { max_skew = 1mm; }", "'CKN' already belongs"),
    ("constraint length_match(CKP, CKN) { id = \"g\"; max_skew = 1mm; }"
     "constraint length_match(D0P, D0N) { id = \"g\"; max_skew = 1mm; }", "duplicate length_match id 'g'"),
])
def test_length_match_net_semantics_fail_with_the_constraint_location(declarations: str, message: str) -> None:
    electrical = compile_source(board(declarations), "lanes.copper")
    with pytest.raises(ValueError, match=rf"lanes.copper:\d+:\d+: .*{message}"):
        prototype_physicalize(electrical)


def test_physical_board_rejects_inconsistent_match_groups() -> None:
    physical = prototype_physicalize(compile_source(board("")))
    with pytest.raises(ValueError, match="unknown net"):
        replace(physical, match_groups=(NetMatchGroup("g", ("CKP", "X"), 1),))
    with pytest.raises(ValueError, match="belongs to length-match groups"):
        replace(physical, match_groups=(NetMatchGroup("g", ("CKP", "CKN"), 1),
                                        NetMatchGroup("h", ("CKN", "D0P"), 1)))
    with pytest.raises(ValueError, match="at least two nets"):
        NetMatchGroup("g", ("CKP",), 1)


def _routed_lanes(lengths_mm: dict[str, str], max_skew_mm: str) -> PhysicalBoard:
    footprint = PhysicalFootprint("test/pad", (FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),),
                                  Size.mm(1, 1))
    placements, nets, tracks = [], [], []
    for row, (net, length) in enumerate(sorted(lengths_mm.items())):
        y = Decimal(3 + 3 * row)
        end = Decimal(2) + Decimal(length)
        placements += [Placement(f"{net}_A", footprint.name, Point.mm(2, y)),
                       Placement(f"{net}_B", footprint.name, Point.mm(end, y))]
        nets.append(PhysicalNet(net, (PadReference(f"{net}_A", "1"), PadReference(f"{net}_B", "1"))))
        tracks.append(TrackSegment(net, Point.mm(2, y), Point.mm(end, y), nm_from_mm("0.2"),
                                   CopperLayer.FRONT))
    return PhysicalBoard(
        "Lanes", BoardOutline.rectangle(30, 20), {footprint.name: footprint}, tuple(placements),
        tuple(nets), tracks=tuple(tracks), metadata={"detailed_routing": "complete"},
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.15")),
        match_groups=(NetMatchGroup("lanes", tuple(sorted(lengths_mm)), nm_from_mm(max_skew_mm)),),
    )


def test_length_match_verification_reports_member_lengths_and_group_skew() -> None:
    routed = _routed_lanes({"CKP": "10", "D0P": "11", "D1P": "10.4"}, "1.5")
    result = verify_match_groups(routed)[0]
    assert [(member.net, member.length_nm, member.routed) for member in result.members] == [
        ("CKP", nm_from_mm(10), True), ("D0P", nm_from_mm(11), True), ("D1P", nm_from_mm("10.4"), True)]
    assert (result.skew_nm, result.status) == (nm_from_mm(1), "pass")
    assert result.to_dict()["members"][0]["shortfall_nm"] == nm_from_mm(1)
    report = run_physical_drc(routed)
    assert not [item for item in report.findings if item.code == "DRC-LENGTH-MATCH"]
    coverage = next(item for item in report.coverage if item.check == "length_match")
    assert coverage.status.value == "executed" and coverage.required


def test_group_skew_over_the_limit_is_a_hard_drc_failure() -> None:
    routed = _routed_lanes({"CKP": "10", "D0P": "12"}, "1.5")
    report = run_physical_drc(routed)
    finding = next(item for item in report.findings if item.code == "DRC-LENGTH-MATCH")
    assert finding.severity.value == "error"
    assert (finding.required_nm, finding.measured_nm) == (nm_from_mm("1.5"), nm_from_mm(2))
    assert finding.nets == ("CKP", "D0P")
    assert finding.objects == ("match_group:lanes",)
    assert report.decision.value == "fail"


def test_incompletely_routed_groups_are_not_judged() -> None:
    routed = _routed_lanes({"CKP": "10", "D0P": "14"}, "1.5")
    partial = replace(routed, tracks=routed.tracks[:1])
    result = verify_match_groups(partial)[0]
    assert (result.status, result.skew_nm) == ("incomplete", None)
    assert [member.routed for member in result.members] == [True, False]
    assert not [item for item in run_physical_drc(partial).findings if item.code == "DRC-LENGTH-MATCH"]


def test_boards_without_groups_report_length_match_not_applicable() -> None:
    routed = replace(_routed_lanes({"CKP": "10", "D0P": "12"}, "1.5"), match_groups=())
    report = run_physical_drc(routed)
    coverage = next(item for item in report.coverage if item.check == "length_match")
    assert coverage.status.value == "not_applicable"
    assert verify_match_groups(routed) == ()

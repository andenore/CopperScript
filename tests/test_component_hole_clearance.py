"""Component-scoped ``hole_clearance``: one footprint's own pads against its own NPTH holes."""

from __future__ import annotations

from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (
    BoardOutline,
    ComponentHoleClearance,
    ConstraintKind,
    DesignRules,
    FootprintPad,
    FootprintResolver,
    KiCadPcbBackend,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Size,
    compile_design_file,
    compile_design_source,
    normalize_constraints,
    physical_board_digest,
    prototype_physicalize,
    resolved_physicalize,
    run_physical_drc,
    write_kicad_project,
)
from pcbir.backends.kicad_project import kicad_export_digest
from pcbir.drc import placement_copper_findings
from pcbir.physical import CopperLayer, PadKind, PadShape, Point, TrackSegment, Via
from pcbir.syntax import CopperScriptError


ROOT = Path(__file__).parents[1]
PLAIN_FOOTPRINT = ROOT / "tests" / "fixtures" / "footprints" / "R_0402_Test.kicad_mod"
REASON = "Vendor land pattern: GND pads 0.194 mm from the receptacle's own plastic locating pegs"
PEG_RADIUS_NM = 325_000
PAD_NM = 600_000
BOARD_HOLE_CLEARANCE_NM = 250_000  # DesignRules default


# --- source and lowering -------------------------------------------------------

RECEPTACLE_MOD = """(footprint "J_Receptacle_Test"
  (layer "F.Cu")
  (attr smd)
  (pad "" np_thru_hole circle (at -2 0) (size 0.65 0.65) (drill 0.65) (layers "*.Cu" "*.Mask"))
  (pad "" np_thru_hole circle (at 2 0) (size 0.65 0.65) (drill 0.65) (layers "*.Cu" "*.Mask"))
  (pad "1" smd rect (at -2 -0.819) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
  (pad "2" smd rect (at 2 -0.819) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
  (fp_rect (start -3 -1.5) (end 3 1.5) (stroke (width 0.05) (type default)) (fill none) (layer "F.CrtYd"))
)
"""


def _source(constraints: str) -> str:
    return f"""board Receptacle {{
    use library "tiny";
    component J1: RESISTOR {{ value = 1kohm; footprint = "J_Receptacle_Test.kicad_mod"; }}
    component R1: RESISTOR {{ value = 1kohm; footprint = "{PLAIN_FOOTPRINT.name}"; }}
    net GND {{ J1.1; J1.2; R1.1; }}
    net SIG {{ R1.2; }}
{constraints}
}}
"""


VALID = f"""    constraint hole_clearance(J1) {{
        clearance = 0.19mm;
        reason = "{REASON}";
    }}"""


def _library(directory: Path) -> Path:
    (directory / "J_Receptacle_Test.kicad_mod").write_text(RECEPTACLE_MOD, encoding="utf-8")
    shutil.copyfile(PLAIN_FOOTPRINT, directory / PLAIN_FOOTPRINT.name)
    return directory


def _physicalize(tmp_path: Path, source: str) -> PhysicalBoard:
    design = compile_design_source(source, str(tmp_path / "board.copper"))
    return resolved_physicalize(design, FootprintResolver(_library(tmp_path)))


def _line(source: str, text: str, occurrence: int = 0) -> int:
    lines = [index for index, line in enumerate(source.splitlines(), 1) if text in line]
    return lines[occurrence]


def test_valid_constraint_lowers_to_the_physical_board(tmp_path: Path) -> None:
    source = _source(VALID)
    design = compile_design_source(source, "board.copper")
    constraint = next(item for item in design.electrical.constraints
                      if item.kind is ConstraintKind.HOLE_CLEARANCE)
    assert constraint.targets == ("J1",)
    assert set(constraint.parameters) == {"clearance", "reason"}

    physical = _physicalize(tmp_path, source)
    assert physical.component_hole_clearances == (ComponentHoleClearance("J1", 190_000, REASON),)
    assert physical.rules.minimum_hole_clearance_nm == BOARD_HOLE_CLEARANCE_NM
    assert not physical.metadata.get("omitted_constraint_targets")
    # The scoped rule is board data bound by the signoff digest.
    assert physical_board_digest(physical) != physical_board_digest(
        replace(physical, component_hole_clearances=()))
    assert physical_board_digest(physical) != physical_board_digest(replace(
        physical, component_hole_clearances=(ComponentHoleClearance("J1", 200_000, REASON),)))

    normalized = normalize_constraints(design.electrical.constraints)[0]
    assert (normalized.property, normalized.domain, normalized.verifier, normalized.consumers) == (
        "copper.hole_clearance", "physical", "DRC-HOLE-CLEARANCE",
        ("physicalizer", "physical_drc", "kicad_rules"))


def test_proxy_footprints_have_no_holes_and_keep_the_board_rule() -> None:
    physical = prototype_physicalize(compile_design_source(_source(VALID)))
    assert physical.component_hole_clearances == ()


@pytest.mark.parametrize("declaration, message", [
    ("constraint hole_clearance() { clearance = 0.19mm; reason = \"r\"; }", "exactly one component"),
    ("constraint hole_clearance(J1, R1) { clearance = 0.19mm; reason = \"r\"; }", "exactly one component"),
    ("constraint hole_clearance(J1.1) { clearance = 0.19mm; reason = \"r\"; }", "not a pin"),
    ("constraint hole_clearance(J1) { reason = \"r\"; }", "requires 'clearance'"),
    ("constraint hole_clearance(J1) { clearance = 0.19; reason = \"r\"; }", "positive length"),
    ("constraint hole_clearance(J1) { clearance = 0mm; reason = \"r\"; }", "positive length"),
    ("constraint hole_clearance(J1) { clearance = -0.1mm; reason = \"r\"; }", "positive length"),
    ("constraint hole_clearance(J1) { clearance = 1V; reason = \"r\"; }", "positive length"),
    ("constraint hole_clearance(J1) { clearance = 0.19mm; }", "requires 'reason'"),
    ("constraint hole_clearance(J1) { clearance = 0.19mm; reason = \"  \"; }", "nonempty string"),
    ("constraint hole_clearance(J1) { clearance = 0.19mm; reason = 3; }", "nonempty string"),
    ("constraint hole_clearance(J1) { clearance = 0.19mm; reason = \"r\"; width = 1mm; }",
     "unknown hole_clearance parameter 'width'"),
])
def test_invalid_declarations_fail_with_a_location(declaration: str, message: str) -> None:
    source = _source("    " + declaration)
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_design_source(source, "receptacle.copper")
    assert error.value.code == "CMP112"
    assert error.value.location.filename == "receptacle.copper"
    assert error.value.location.line == _line(source, "hole_clearance")


def test_unknown_component_fails_with_a_location() -> None:
    source = _source("    constraint hole_clearance(J9) { clearance = 0.19mm; reason = \"r\"; }")
    with pytest.raises(CopperScriptError, match="'J9' is not a component") as error:
        compile_design_source(source, "receptacle.copper")
    assert error.value.code == "CMP113"
    assert error.value.location.line == _line(source, "hole_clearance")


def test_duplicate_constraint_fails_at_the_second_declaration() -> None:
    source = _source(VALID + "\n    constraint hole_clearance(J1) { clearance = 0.2mm; reason = \"again\"; }")
    with pytest.raises(CopperScriptError, match="'J1' already has a hole_clearance") as error:
        compile_design_source(source, "receptacle.copper")
    assert error.value.code == "CMP114"
    assert error.value.location.line == _line(source, "hole_clearance", 1)


@pytest.mark.parametrize("constraints, code, message", [
    # R1's footprint has only copper pads.
    ("constraint hole_clearance(R1) { clearance = 0.19mm; reason = \"r\"; }", "CMP115",
     "'R1' has no footprint with non-plated holes"),
    ("constraint hole_clearance(J1) { clearance = 0.3mm; reason = \"r\"; }", "CMP116",
     "may only relax"),
])
def test_footprint_semantics_fail_with_the_constraint_location(
        tmp_path: Path, constraints: str, code: str, message: str) -> None:
    source = _source("    " + constraints)
    line = _line(source, "hole_clearance")
    with pytest.raises(ValueError, match=rf"board.copper:{line}:\d+: {code}: .*{message}"):
        _physicalize(tmp_path, source)


def test_component_without_a_footprint_fails(tmp_path: Path) -> None:
    # The part has no footprint, so the component is omitted from the board.
    source = _source("""    component V1: VOLTAGE_SOURCE;
    constraint hole_clearance(V1) { clearance = 0.19mm; reason = "r"; }""")
    with pytest.raises(ValueError, match=r"board.copper:\d+:\d+: CMP115: component 'V1'"):
        _physicalize(tmp_path, source)


def _package(tmp_path: Path, module_constraint: str, board_constraint: str) -> Path:
    parts = tmp_path / "parts"
    parts.mkdir()
    (parts / "J_Receptacle_Test.kicad_mod").write_text(RECEPTACLE_MOD, encoding="utf-8")
    (parts / "port.copper").write_text(f"""module Port {{
    use library "tiny";
    port A: passive;
    component J: RESISTOR {{ value = 1kohm; footprint = "J_Receptacle_Test.kicad_mod"; }}
    net A {{ J.1; J.2; port.A; }}
    {module_constraint}
}}
""", encoding="utf-8")
    board = tmp_path / "board.copper"
    board.write_text(f"""board Hierarchy {{
    import p "./parts";
    module OUT: p.Port;
    net X {{ OUT.A; }}
    {board_constraint}
}}
""", encoding="utf-8")
    return board


@pytest.mark.parametrize("module_constraint, board_constraint", [
    ("", f"constraint hole_clearance(OUT/J) {{ clearance = 0.19mm; reason = \"{REASON}\"; }}"),
    (f"constraint hole_clearance(J) {{ clearance = 0.19mm; reason = \"{REASON}\"; }}", ""),
])
def test_module_component_paths_lower_to_the_flattened_reference(
        tmp_path: Path, module_constraint: str, board_constraint: str) -> None:
    board = _package(tmp_path, module_constraint, board_constraint)
    physical = resolved_physicalize(compile_design_file(board), FootprintResolver(tmp_path))
    assert physical.component_hole_clearances == (ComponentHoleClearance("OUT/J", 190_000, REASON),)


@pytest.mark.parametrize("module_constraint, board_constraint, code, message", [
    ("", "constraint hole_clearance(OUT/K) { clearance = 0.19mm; reason = \"r\"; }",
     "CMP113", "'OUT/K' is not a component"),
    ("", "constraint hole_clearance(NOPE/J) { clearance = 0.19mm; reason = \"r\"; }",
     "CMP113", "'NOPE/J' is not a component"),
    ("constraint hole_clearance(J) { clearance = 0.19mm; reason = \"r\"; }",
     "constraint hole_clearance(OUT/J) { clearance = 0.19mm; reason = \"r\"; }",
     "CMP114", "'OUT/J' already has a hole_clearance"),
])
def test_module_component_paths_are_resolved_at_compile_time(
        tmp_path: Path, module_constraint: str, board_constraint: str, code: str, message: str) -> None:
    board = _package(tmp_path, module_constraint, board_constraint)
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_design_file(board)
    assert error.value.code == code
    assert Path(error.value.location.filename).name == "board.copper"
    assert error.value.location.line == 5


# --- physical DRC ----------------------------------------------------------------


def _receptacle(gap_nm: int) -> PhysicalFootprint:
    offset = PEG_RADIUS_NM + gap_nm + PAD_NM // 2
    peg = Size(2 * PEG_RADIUS_NM, 2 * PEG_RADIUS_NM)
    pad = Size(PAD_NM, PAD_NM)
    return PhysicalFootprint("test/receptacle", (
        FootprintPad("", Point.mm(-2, 0), peg, kind=PadKind.NON_PLATED_THROUGH_HOLE,
                     shape=PadShape.CIRCLE, drill=peg),
        FootprintPad("", Point.mm(2, 0), peg, kind=PadKind.NON_PLATED_THROUGH_HOLE,
                     shape=PadShape.CIRCLE, drill=peg),
        FootprintPad("1", Point(-2_000_000, -offset), pad, shape=PadShape.RECTANGLE),
        FootprintPad("2", Point(-2_000_000, offset), pad, shape=PadShape.RECTANGLE),
        FootprintPad("3", Point(2_000_000, -offset), pad, shape=PadShape.RECTANGLE),
        FootprintPad("4", Point(2_000_000, offset), pad, shape=PadShape.RECTANGLE),
        FootprintPad("5", Point(0, -offset), pad, shape=PadShape.RECTANGLE),
    ), Size.mm(6, 3))


def receptacle_board(gap_nm: int = 194_000) -> PhysicalBoard:
    """J1 at (10, 10) mm with pegs at x = 8 and 12 mm; neighbours 0.2 mm from them."""
    offset = PEG_RADIUS_NM + gap_nm + PAD_NM // 2
    receptacle = _receptacle(gap_nm)
    single = PhysicalFootprint("test/pad", (
        FootprintPad("1", Point(0, 0), Size(PAD_NM, PAD_NM), shape=PadShape.RECTANGLE),), Size.mm(1, 1))
    mounting = PhysicalFootprint("test/mounting", (
        FootprintPad("", Point(0, 0), Size.mm(1, 1), kind=PadKind.NON_PLATED_THROUGH_HOLE,
                     shape=PadShape.CIRCLE, drill=Size.mm(1, 1)),), Size.mm(1, 1))
    return PhysicalBoard(
        "HoleRules", BoardOutline.rectangle(20, 20),
        {item.name: item for item in (receptacle, single, mounting)},
        (Placement("J1", receptacle.name, Point.mm(10, 10)),
         # R1's pad and H1's hole are 0.2 mm from J1's right peg and pad 1.
         Placement("R1", single.name, Point(12_000_000 + PEG_RADIUS_NM + 200_000 + PAD_NM // 2, 10_000_000)),
         Placement("H1", mounting.name, Point(8_000_000, 10_000_000 - offset - PAD_NM // 2 - 200_000 - 500_000))),
        (PhysicalNet("GND", tuple(PadReference("J1", n) for n in "1234")),
         PhysicalNet("SIG", (PadReference("J1", "5"),)),
         PhysicalNet("SIG_R", (PadReference("R1", "1"),)),
         PhysicalNet("SIG_T", ()), PhysicalNet("SIG_V", ())),
        # A foreign-net and an own-net (GND) track, and a via, each 0.2 mm from a peg.
        tracks=(TrackSegment("SIG_T", Point.mm(7.375, 9.6), Point.mm(7.375, 10.4), 200_000, CopperLayer.FRONT),
                TrackSegment("GND", Point.mm(8.625, 9.6), Point.mm(8.625, 10.4), 200_000, CopperLayer.FRONT)),
        vias=(Via("SIG_V", Point.mm(11.175, 10), 600_000, 300_000),),
        metadata={"detailed_routing": "complete"},
    )


def _scoped(board: PhysicalBoard, clearance_nm: int = 190_000) -> PhysicalBoard:
    return replace(board, component_hole_clearances=(ComponentHoleClearance("J1", clearance_nm, REASON),))


OWN_PAIRS = {("pad:J1.1:2", "hole:J1.:0"), ("pad:J1.2:3", "hole:J1.:0"),
             ("pad:J1.3:4", "hole:J1.:1"), ("pad:J1.4:5", "hole:J1.:1")}
# Every other neighbour is 0.2 mm from a hole and keeps the 0.25 mm board rule.
BOARD_RULE_PAIRS = {("pad:J1.1:2", "hole:H1.:0"), ("pad:R1.1:0", "hole:J1.:1"),
                    ("track:0", "hole:J1.:0"), ("track:1", "hole:J1.:0"), ("via:0", "hole:J1.:1")}


def _hole_findings(board: PhysicalBoard):
    return [item for item in run_physical_drc(board).findings if item.code == "DRC-HOLE-CLEARANCE"]


def test_without_the_constraint_the_pads_fail_the_board_rule() -> None:
    findings = _hole_findings(receptacle_board())
    assert {item.objects for item in findings} == OWN_PAIRS | BOARD_RULE_PAIRS


def test_scoped_rule_passes_own_pads_and_records_each_relaxation() -> None:
    report = run_physical_drc(_scoped(receptacle_board()))
    findings = [item for item in report.findings if item.code == "DRC-HOLE-CLEARANCE"]
    # Tracks (foreign and own net), the via, R1's pad and J1's pad next to H1 still fail.
    assert {item.objects for item in findings} == BOARD_RULE_PAIRS
    assert all("scoped" not in item.message for item in findings)
    assert [(item.component, item.objects, item.nets, item.required_nm, item.relaxed_nm,
             item.measured_nm, item.reason) for item in report.hole_clearance_relaxations] == [
        ("J1", pair, ("GND",), BOARD_HOLE_CLEARANCE_NM, 190_000, 194_000, REASON)
        for pair in sorted(OWN_PAIRS)]
    coverage = {item.check: item for item in report.coverage}
    assert coverage["component_hole_clearance"].required
    document = json.loads(report.to_json())
    assert document["hole_clearance_relaxations"][0] == {
        "component": "J1", "objects": ["pad:J1.1:2", "hole:J1.:0"], "nets": ["GND"],
        "required_nm": BOARD_HOLE_CLEARANCE_NM, "relaxed_nm": 190_000, "measured_nm": 194_000,
        "reason": REASON}
    # Editor placement checks share the same rule.
    assert {item.objects for item in placement_copper_findings(_scoped(receptacle_board()))
            if item.code == "DRC-HOLE-CLEARANCE"} == BOARD_RULE_PAIRS


def test_a_pad_closer_than_the_scoped_value_still_fails() -> None:
    report = run_physical_drc(_scoped(receptacle_board(gap_nm=185_000)))
    scoped = [item for item in report.findings
              if item.code == "DRC-HOLE-CLEARANCE" and item.objects in OWN_PAIRS]
    assert len(scoped) == 4
    assert all("checked against the scoped hole_clearance of J1" in item.message for item in scoped)
    assert {(item.required_nm, item.measured_nm) for item in scoped} == {(190_000, 185_000)}
    assert report.hole_clearance_relaxations == ()


def test_pads_meeting_the_board_rule_are_not_listed() -> None:
    report = run_physical_drc(_scoped(receptacle_board(gap_nm=BOARD_HOLE_CLEARANCE_NM)))
    assert report.hole_clearance_relaxations == ()
    assert not any(item.objects in OWN_PAIRS for item in report.findings)
    assert "component_hole_clearance" in {item.check for item in report.coverage}


def test_tracks_vias_and_other_pads_at_the_scoped_distance_keep_the_board_rule() -> None:
    board = _scoped(receptacle_board())
    left, right = 8_000_000 - PEG_RADIUS_NM - 190_000, 8_000_000 + PEG_RADIUS_NM + 190_000
    tracks = (  # 0.19 mm either side of the left peg: a foreign net and J1's own GND net
        TrackSegment("SIG_T", Point(left - 100_000, 9_600_000), Point(left - 100_000, 10_400_000),
                     200_000, CopperLayer.FRONT),
        TrackSegment("GND", Point(right + 100_000, 9_600_000), Point(right + 100_000, 10_400_000),
                     200_000, CopperLayer.FRONT),
    )
    # A via and R1's pad 0.19 mm either side of the right peg.
    vias = (Via("SIG_V", Point(12_000_000 - PEG_RADIUS_NM - 190_000 - 300_000, 10_000_000), 600_000, 300_000),)
    moved = replace(board, tracks=tracks, vias=vias, placements=tuple(
        replace(item, position=Point(12_000_000 + PEG_RADIUS_NM + 190_000 + PAD_NM // 2, 10_000_000))
        if item.reference == "R1" else item for item in board.placements))
    findings = _hole_findings(moved)
    assert {("track:0", "hole:J1.:0"), ("track:1", "hole:J1.:0"), ("via:0", "hole:J1.:1"),
            ("pad:R1.1:0", "hole:J1.:1")} <= {item.objects for item in findings}
    assert not {item.objects for item in findings} & OWN_PAIRS


def test_physical_board_validates_scoped_rules() -> None:
    board = receptacle_board()
    rule = ComponentHoleClearance("J1", 190_000, REASON)
    with pytest.raises(ValueError, match="more than one hole clearance"):
        replace(board, component_hole_clearances=(rule, rule))
    with pytest.raises(ValueError, match="unknown placement"):
        replace(board, component_hole_clearances=(ComponentHoleClearance("J9", 190_000, REASON),))
    with pytest.raises(ValueError, match="no non-plated holes"):
        replace(board, component_hole_clearances=(ComponentHoleClearance("R1", 190_000, REASON),))
    with pytest.raises(ValueError, match="may only relax"):
        replace(board, component_hole_clearances=(ComponentHoleClearance("J1", 260_000, REASON),))
    with pytest.raises(ValueError, match="may only relax"):
        replace(_scoped(board), rules=DesignRules(minimum_hole_clearance_nm=150_000))
    with pytest.raises(ValueError, match="requires a reason"):
        ComponentHoleClearance("J1", 190_000, " ")
    with pytest.raises(ValueError, match="must be positive"):
        ComponentHoleClearance("J1", 0, REASON)


# Goldens computed before hole_clearance existed: boards without the
# constraint must keep byte-identical DRC reports, digests and KiCad exports.
GOLDEN_BOARD_DIGEST = "e81641e915f7bb4c2d432820c42f6713050039262c0bd94782b64cdd57130b06"
GOLDEN_REPORT_SHA256 = "4611290aeac812375105452d0717065d7fe70bd3a42cb19c37356a67f11e293e"
GOLDEN_KICAD_EXPORT_DIGEST = "f6c051483c55022ffd2fff6c0b491c1cfe782748e04982c62443aa5494f2d54b"


def test_boards_without_scoped_rules_are_byte_identical() -> None:
    board = receptacle_board()
    report = run_physical_drc(board)
    assert physical_board_digest(board) == GOLDEN_BOARD_DIGEST
    assert sha256(report.to_json().encode()).hexdigest() == GOLDEN_REPORT_SHA256
    assert "hole_clearance_relaxations" not in report.to_json()
    assert "component_hole_clearance" not in {item.check for item in report.coverage}
    manifest = KiCadPcbBackend().generate(board)
    assert kicad_export_digest(manifest) == GOLDEN_KICAD_EXPORT_DIGEST
    assert not any(item.name.endswith(".kicad_dru") for item in manifest.artifacts)


def test_scoped_results_are_deterministic(tmp_path: Path) -> None:
    first = run_physical_drc(_scoped(receptacle_board()))
    second = run_physical_drc(_scoped(receptacle_board()))
    assert first == second and first.to_json() == second.to_json()
    one = _physicalize(tmp_path, _source(VALID))
    two = _physicalize(tmp_path, _source(VALID))
    assert one == two and physical_board_digest(one) == physical_board_digest(two)
    assert KiCadPcbBackend().generate(_scoped(receptacle_board())) == \
        KiCadPcbBackend().generate(_scoped(receptacle_board()))


# --- KiCad export ----------------------------------------------------------------

EXPECTED_RULES = f"""(version 1)
# Generated by CopperScript from hole_clearance constraints; do not edit.
# J1: {REASON}
(rule "J1 hole clearance"
  (constraint hole_clearance (min 0.19mm))
  (condition "A.memberOfFootprint('J1') && B.memberOfFootprint('J1') && (A.Pad_Type == 'NPTH, mechanical' || B.Pad_Type == 'NPTH, mechanical')"))
"""


def test_kicad_export_writes_a_same_stem_custom_rules_file(tmp_path: Path) -> None:
    plain = KiCadPcbBackend().generate(receptacle_board())
    manifest = KiCadPcbBackend().generate(_scoped(receptacle_board()))
    rules = next(item for item in manifest.artifacts if item.name.endswith(".kicad_dru"))
    assert rules.name == "HoleRules.kicad_dru"
    assert rules.content == EXPECTED_RULES
    # Board Setup keeps the board value; only the rules file is added.
    assert [item for item in manifest.artifacts if item is not rules] == list(plain.artifacts)
    assert any("J1: 0.19 mm" in warning for warning in manifest.warnings)

    output = tmp_path / "chosen.kicad_pcb"
    paths = write_kicad_project(manifest, output)
    assert output.with_suffix(".kicad_dru") in paths
    assert output.with_suffix(".kicad_dru").read_text(encoding="utf-8") == EXPECTED_RULES
    # A later export without scoped rules removes the generated file but keeps hand-written ones.
    write_kicad_project(plain, output)
    assert not output.with_suffix(".kicad_dru").exists()
    output.with_suffix(".kicad_dru").write_text("(version 1)\n", encoding="utf-8")
    write_kicad_project(plain, output)
    assert output.with_suffix(".kicad_dru").read_text(encoding="utf-8") == "(version 1)\n"


def test_kicad_rules_use_the_exported_reference_and_require_it_to_be_unique() -> None:
    board = _scoped(receptacle_board())
    nested = replace(board, placements=tuple(
        replace(item, reference="OUT/J1") if item.reference == "J1" else item for item in board.placements),
        nets=tuple(replace(net, pads=tuple(PadReference("OUT/J1", pad.pad) if pad.component == "J1" else pad
                                           for pad in net.pads)) for net in board.nets),
        component_hole_clearances=(ComponentHoleClearance("OUT/J1", 190_000, REASON),))
    rules = next(item for item in KiCadPcbBackend().generate(nested).artifacts
                 if item.name.endswith(".kicad_dru")).content
    assert "memberOfFootprint('OUT_J1')" in rules and "# OUT/J1: " in rules
    clash = replace(nested, placements=tuple(
        replace(item, reference="OUT_J1") if item.reference == "R1" else item for item in nested.placements),
        nets=tuple(replace(net, pads=tuple(PadReference("OUT_J1", pad.pad) if pad.component == "R1" else pad
                                           for pad in net.pads)) for net in nested.nets))
    with pytest.raises(ValueError, match="'OUT_J1' is not unique"):
        KiCadPcbBackend().generate(clash)


def _kicad_hole_violations(cli: str, board: PhysicalBoard, directory: Path) -> set[tuple[str, str]]:
    """Run native KiCad DRC; return (copper item, hole owner) for each hole violation."""
    output = directory / "board.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), output)
    report = directory / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--format", "json", "--severity-all",
                             "--output", str(report), str(output)],
                            capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, result.stdout + result.stderr
    found = set()
    for violation in json.loads(report.read_text(encoding="utf-8"))["violations"]:
        if violation["type"] != "hole_clearance":
            continue
        descriptions = [item["description"] for item in violation["items"]]
        index = next(i for i, text in enumerate(descriptions) if text.startswith("NPTH pad of "))
        copper = descriptions[1 - index]
        found.add((copper.split(" on ")[0], descriptions[index].removeprefix("NPTH pad of ")))
    return found


KICAD_BOARD_RULE = {("Pad 1 [GND] of J1", "H1"), ("Pad 1 [SIG_R] of R1", "J1"),
                    ("Track [SIG_T]", "J1"), ("Track [GND]", "J1"), ("Via [SIG_V]", "J1")}
KICAD_OWN = {(f"Pad {number} [GND] of J1", "J1") for number in "1234"}


def test_native_kicad_drc_agrees_with_the_scoped_rule(tmp_path: Path) -> None:
    cli = shutil.which("kicad-cli")
    if not cli:
        pytest.skip("native DRC agreement requires KiCad CLI")
    (tmp_path / "plain").mkdir()
    (tmp_path / "scoped").mkdir()
    (tmp_path / "tight").mkdir()
    assert _kicad_hole_violations(cli, receptacle_board(), tmp_path / "plain") == KICAD_OWN | KICAD_BOARD_RULE
    # The custom rule relaxes only J1's pads against J1's pegs, below the
    # 0.25 mm Board Setup value; tracks, the via, R1 and H1 keep 0.25 mm.
    assert _kicad_hole_violations(cli, _scoped(receptacle_board()), tmp_path / "scoped") == KICAD_BOARD_RULE
    # A scoped value above the measured 0.194 mm is enforced by both checkers.
    tight = _scoped(receptacle_board(), clearance_nm=195_000)
    assert _kicad_hole_violations(cli, tight, tmp_path / "tight") == KICAD_OWN | KICAD_BOARD_RULE
    assert {item.objects for item in _hole_findings(tight)} == OWN_PAIRS | BOARD_RULE_PAIRS

"""Declared requirements must be checked independently of optimistic board status."""
from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir import compile_source, prototype_physicalize
from pcbir.drc import run_physical_drc
from pcbir.physical import CopperLayer, Point, TrackSegment, nm_from_mm
from pcbir.placement import transformed_pad_position
from pcbir.requirement_verification import verify_requirements


def design(declarations=""):
    source = compile_source('''board Verification {
        use library "tiny";
        component A: RESISTOR;
        component B: RESISTOR;
        net N { A.1; B.1; }
        net M { A.2; B.2; }
    ''' + declarations + "}", filename="requirements.copper")
    physical = prototype_physicalize(source)
    physical = replace(physical, placements=tuple(replace(p, position=Point.mm(10 if p.reference == "A" else 20, 10))
                                                  for p in physical.placements))
    poses = {p.reference: p for p in physical.placements}
    tracks = tuple(TrackSegment(net, *(transformed_pad_position(physical, poses[ref], pad) for ref in ("A", "B")),
                               nm_from_mm("0.25"), CopperLayer.FRONT) for net, pad in (("N", "1"), ("M", "2")))
    physical = replace(physical, tracks=tracks,
                       metadata={**physical.metadata, "detailed_routing": "complete"})
    return source, physical


def row(source, physical, **kwargs):
    return verify_requirements(source, physical, **kwargs)["requirements"][0]


@pytest.mark.parametrize("kind,limit,status", [("max_distance", "10mm", "pass"),
    ("max_distance", "9mm", "fail"), ("min_distance", "10mm", "pass"), ("min_distance", "11mm", "fail")])
def test_actual_pin_distance_and_source_origin(kind, limit, status):
    source, physical = design(f'constraint {kind}(A.1, B.1) {{ id = "bypass"; distance = {limit}; }}')
    result = row(source, physical)
    assert result["status"] == status
    assert result["origins"][0].startswith("requirements.copper:")
    assert result["measurements"][0]["actual_nm"] == 10_000_000


def test_distance_uses_exact_squared_comparison_not_rounded_display():
    source, physical = design('constraint max_distance(A, B) { distance = 0.000001mm; }')
    a, b = physical.placements
    physical = replace(physical, tracks=(), placements=(a, replace(b, position=Point(a.position.x_nm+1, a.position.y_nm+1))))
    result = row(source, physical, stage="placement")
    assert result["status"] == "fail"
    assert result["measurements"][0]["actual_nm"] == 1


@pytest.mark.parametrize("declaration,status", [
    ('constraint fixed_placement(A) { x = 10mm; y = 10mm; rotation = 0; }', "pass"),
    ('constraint fixed_placement(A) { x = 11mm; y = 10mm; }', "fail"),
    ('constraint allowed_orientations(A) { values = "90,180"; }', "fail"),
    ('constraint align(A, B) { axis = y; tolerance = 0mm; }', "pass"),
    ('constraint placement_region(A) { x = 8mm; y = 8mm; width = 4mm; height = 4mm; }', "pass"),
    ('constraint placement_region(A) { x = 10mm; y = 10mm; width = 1mm; height = 1mm; }', "fail"),
    ('constraint keepout(A) { x = 8mm; y = 8mm; width = 4mm; height = 4mm; }', "fail"),
])
def test_individual_placement_geometry(declaration, status):
    source, physical = design(declaration)
    assert row(source, physical, stage="placement")["status"] == status


@pytest.mark.parametrize("mode,status,complete", [("assume", "assumed", True),
    ("external", "blocked", False), ("prefer", "fail", True), ("require", "fail", False)])
def test_assumptions_advisories_and_external_obligations(mode, status, complete):
    source, physical = design(f'constraint max_distance(A, B) {{ distance = 1mm; mode = {mode}; }}')
    report = verify_requirements(source, physical)
    assert report["requirements"][0]["status"] == status
    assert report["complete"] is complete


def test_original_source_limit_is_not_hidden_by_a_weaker_final_rule():
    source, physical = design('constraint routing(N) { id = "strict"; width = 0.3mm; }')
    physical = replace(physical, net_routing_rules=(replace(physical.net_routing_rules[0], width_nm=200_000),))
    report = verify_requirements(source, physical, drc=run_physical_drc(physical))
    assert report["requirements"][0]["status"] == "fail"


def test_valid_routing_is_not_an_impedance_pass():
    source, physical = design('constraint routing(N) { target_impedance_ohms = 50; }')
    result = row(source, physical)
    assert result["status"] == "blocked" and "target_impedance_ohms" in result["detail"]


def test_missing_connectivity_does_not_pass_length_or_via_limits():
    source, physical = design('constraint routing(N) { max_length = 20mm; max_vias = 1; }')
    assert row(source, replace(physical, tracks=()))["status"] == "blocked"


def test_length_and_skew_misses_warn_in_drc_but_still_fail_their_requirements():
    from pcbir.drc import DrcSeverity
    # The 10 mm tracks are connected copper: a 5 mm limit is a review warning
    # in DRC (no open, no geometry error), but the declared limit still fails.
    source, physical = design('constraint routing(N) { max_length = 5mm; }')
    drc = run_physical_drc(physical)
    length = [f for f in drc.findings if f.code == "DRC-MAX-LENGTH"]
    assert length and all(f.severity is DrcSeverity.WARNING for f in length)
    result = row(source, physical)
    assert result["status"] == "fail" and "maximum routed length" in result["detail"]
    assert row(*design('constraint routing(N) { max_length = 20mm; }'))["status"] == "pass"
    # A 2 mm detour on M against a 1 mm group skew limit: the same split.
    source, physical = design('constraint length_match(N, M) { max_skew = 1mm; }')
    assert physical.match_groups
    m = physical.tracks[1]
    up = Point(m.start.x_nm, m.start.y_nm + nm_from_mm(1))
    over = Point(m.end.x_nm, m.end.y_nm + nm_from_mm(1))
    detour = (replace(m, end=up), replace(m, start=up, end=over), replace(m, start=over))
    longer = replace(physical, tracks=(physical.tracks[0], *detour))
    match = [f for f in run_physical_drc(longer).findings if f.code == "DRC-LENGTH-MATCH"]
    assert match and all(f.severity is DrcSeverity.WARNING for f in match)
    assert row(source, longer)["status"] == "fail"
    assert row(source, physical)["status"] == "pass"


def test_stale_drc_and_geometry_bindings():
    source, physical = design('constraint routing(N) { width = 0.2mm; }')
    drc = run_physical_drc(physical)
    moved = replace(physical, tracks=(replace(physical.tracks[0], width_nm=300_000),))
    report = verify_requirements(source, moved, drc=drc)
    assert report["status"] == "blocked"
    assert "different physical geometry" in report["requirements"][0]["detail"]
    assert report["board_digest"] != verify_requirements(source, physical)["board_digest"]


def test_duplicate_ids_and_unregistered_verifier_cannot_pass():
    source, physical = design('constraint max_distance(A, B) { id = "limit"; distance = 20mm; }')
    duplicated = replace(source, constraints=(*source.constraints, *source.constraints))
    assert all(r["status"] == "blocked" for r in verify_requirements(duplicated, physical)["requirements"])
    custom = replace(source, constraints=(replace(source.constraints[0], consumers=("pretend",), verifier="PASS"),))
    assert row(custom, physical)["status"] == "blocked"


def test_placement_cannot_claim_route_stage_verification():
    source, physical = design('constraint routing(N) { max_length = 20mm; }')
    assert row(source, physical, stage="placement")["status"] == "blocked"


def test_length_match_checks_actual_connected_tracks():
    source, physical = design('constraint length_match(N, M) { max_skew = 1mm; }')
    assert row(source, physical)["status"] == "pass"
    assert row(source, replace(physical, tracks=()))["status"] == "blocked"


def test_native_zone_evidence_must_match_final_geometry_and_preserved_settings():
    from pcbir.backends import KiCadPcbBackend
    from pcbir.backends.kicad_project import kicad_export_digest
    from pcbir.drc import physical_board_digest
    from pcbir.plane_verify import PlaneVerification
    source, physical = design('constraint copper_zone(N) { layers = "F.Cu"; inset = 1mm; }')
    assert row(source, physical)["status"] == "blocked"
    # Unit evidence fixture exercises exact source/export binding; actual native
    # process evidence is tested independently by the acceptance runner.
    native = PlaneVerification(True, physical_board_digest(physical),
        kicad_export_digest(KiCadPcbBackend().generate(physical)), "filled", "report", "10.0.6", 0, 0, 0, ())
    assert row(source, physical, native=native)["status"] == "pass"
    moved = replace(physical, tracks=(replace(physical.tracks[0], width_nm=300_000), *physical.tracks[1:]))
    assert row(source, moved, native=native)["status"] == "blocked"
    changed_zone = replace(physical, zones=(replace(physical.zones[0], priority=1),))
    assert row(source, changed_zone, native=native)["status"] == "fail"
    assert row(source, physical, native=replace(native, passed=False, other_violation_count=1))["status"] == "fail"


def test_hierarchical_constraints_retain_qualified_targets_and_origins():
    from pathlib import Path
    from pcbir import compile_file
    source = compile_file(Path(__file__).parents[1] / "examples/hierarchical_board/board.copper")
    physical = prototype_physicalize(source)
    report = verify_requirements(source, physical, stage="placement")
    assert any("PWR/" in target for r in report["requirements"] for target in r["targets"])
    assert all(r["origins"] for r in report["requirements"])


def test_unknown_placement_parameter_and_missing_physical_target_are_blocked():
    source, physical = design('constraint max_distance(A, B) { distance = 20mm; }')
    source = replace(source, constraints=(replace(source.constraints[0],
        parameters={**source.constraints[0].parameters, "imaginary_check": "pass"}),))
    assert row(source, physical)["status"] == "blocked"
    source = replace(source, constraints=(replace(source.constraints[0], targets=("MISSING", "B")),))
    assert row(source, physical)["status"] == "blocked"

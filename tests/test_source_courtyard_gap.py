"""Explicit extra courtyard policy preserves source intent and stricter floors."""
from dataclasses import replace
from decimal import Decimal
import pytest

from pcbir import CopperScriptError, compile_source, compile_file, prototype_physicalize
from pcbir.clusters import footprint_geometry_digest
from pcbir.constraint_coverage import ConstraintMode
from pcbir.model import Constraint, ConstraintKind
from pcbir.physical import (
    BoardOutline, ComponentPlacementRule, PhysicalBoard, PhysicalFootprint,
    Placement, PlacementTarget, Point, RigidPlacementCluster, RigidPlacementMember,
    Size, Stackup, component_pair_courtyard_gap_nm,
)
from pcbir.quantities import Length
from pcbir.serializer import board_to_dict


def source(constraints=""):
    return '''board Generic {
    use library "tiny";
    component A: RESISTOR { value = 10 ohm; footprint = "Test/Chip"; }
    component B: RESISTOR { value = 10 ohm; footprint = "Test/Chip"; }
    component C: RESISTOR { value = 10 ohm; footprint = "Test/Chip"; }
    ''' + constraints + '\n}'


def physical(constraints=""):
    return prototype_physicalize(compile_source(source(constraints)))


def test_absent_default_and_explicit_caller_fallback_remain_distinct():
    board = physical()
    assert component_pair_courtyard_gap_nm(board, "A", "B") == 500000
    assert component_pair_courtyard_gap_nm(board, "A", "B", fallback_nm=200000) == 200000
    assert not board.placement_rules


def test_explicit_zero_and_member_max_do_not_relax_unannotated_neighbour():
    board = physical('constraint courtyard_gap(A, B) { additional = 0 mm; }')
    assert component_pair_courtyard_gap_nm(board, "A", "B") == 0
    assert component_pair_courtyard_gap_nm(board, "A", "C") == 500000
    board = physical('''constraint courtyard_gap(A) { additional = 0.1 mm; }
                       constraint courtyard_gap(B) { additional = 0.3 mm; }''')
    assert component_pair_courtyard_gap_nm(board, "A", "B") == 300000
    assert component_pair_courtyard_gap_nm(board, "B", "C", fallback_nm=200000) == 300000


@pytest.mark.parametrize("reverse", [False, True])
def test_region_and_repeated_policy_max_merge_without_erasing_fixed_intent(reverse):
    declarations = [
        'constraint courtyard_gap(A, B) { additional = 0.2 mm; }',
        '''constraint placement_region(A, B) {
            name = "bank"; x = 2 mm; y = 2 mm; width = 20 mm; height = 20 mm;
            side = front; courtyard_gap = 0.1 mm; }''',
        'constraint courtyard_gap(A) { additional = 0 mm; }',
        'constraint fixed_placement(A) { x = 5 mm; y = 5 mm; rotation = 90; side = front; }',
        'constraint allowed_orientations(A) { values = "0,90"; }',
    ]
    if reverse:
        declarations.reverse()
    board = physical('\n'.join(declarations))
    rules = {r.reference: r for r in board.placement_rules}
    assert rules["A"].additional_courtyard_gap_nm == rules["B"].additional_courtyard_gap_nm == 200000
    assert rules["A"].fixed_position == Point.mm(5, 5)
    assert rules["A"].fixed_rotation_degrees == 90
    assert rules["A"].allowed_orientations == (Decimal(0), Decimal(90))
    assert rules["A"].region == rules["B"].region == "bank"


def test_region_without_gap_does_not_erase_explicit_policy():
    board = physical('''constraint courtyard_gap(A) { additional = 0 mm; }
        constraint placement_region(A) { name="r"; x=0 mm; y=0 mm; width=20 mm; height=20 mm; }''')
    assert board.placement_rules[0].additional_courtyard_gap_nm == 0


def test_quantity_metadata_roundtrip_retains_exact_value_and_origins():
    electrical = compile_source(source('''constraint courtyard_gap(A, B) {
        additional = 0.125 mm; id = "assembly.bank"; mode = external;
        consumers = "placement,physical_drc"; verifier = "DRC-PLACEMENT"; }'''), "policy.copper")
    declaration = electrical.constraints[0]
    assert declaration.parameters["additional"].base_value == Decimal("0.000125")
    assert declaration.constraint_id == "assembly.bank" and declaration.mode is ConstraintMode.EXTERNAL
    serialized = board_to_dict(electrical)["constraints"][0]
    assert serialized["origins"] == list(declaration.origins)
    assert serialized["parameters"]["additional"]["base_value"] == "0.000125"
    assert {r.additional_courtyard_gap_nm for r in prototype_physicalize(electrical).placement_rules} == {125000}


@pytest.mark.parametrize("declaration", [
    'constraint courtyard_gap(A) { additional = -0.1 mm; }',
    'constraint courtyard_gap(A) { additional = 0.0000001 mm; }',
    'constraint courtyard_gap(A) { additional = 1 V; }',
    'constraint courtyard_gap(A) { additional = 0; }',
    'constraint courtyard_gap(A) { }',
    'constraint courtyard_gap(A) { additional = 0 mm; verified = true; }',
    'constraint courtyard_gap() { additional = 0 mm; }',
    'constraint courtyard_gap(A.1) { additional = 0 mm; }',
    'constraint courtyard_gap(Missing) { additional = 0 mm; }',
    'constraint courtyard_gap(A) { additional = 0 mm; mode = prefer; }',
    'constraint courtyard_gap(A) { additional = 0 mm; mode = target; }',
    'constraint courtyard_gap(A) { additional = 0 mm; mode = assume; }',
    'constraint placement_region(A) { x=0 mm;y=0 mm;width=10 mm;height=10 mm;courtyard_gap=-1 mm; }',
    'constraint placement_region(A) { x=0 mm;y=0 mm;width=10 mm;height=10 mm;courtyard_gap=0 mm;mode=prefer; }',
])
def test_invalid_source_policy_is_located_and_rejected(declaration):
    with pytest.raises(CopperScriptError) as caught:
        compile_source(source(declaration), "bad-policy.copper")
    assert caught.value.code == "CMP123"
    assert str(caught.value).startswith("bad-policy.copper:")


@pytest.mark.parametrize("bad", [-1, 0.5, True])
def test_typed_member_and_fallback_require_nonnegative_integer_nm(bad):
    with pytest.raises(ValueError):
        ComponentPlacementRule("A", additional_courtyard_gap_nm=bad)
    with pytest.raises(ValueError):
        component_pair_courtyard_gap_nm(physical(), "A", "B", fallback_nm=bad)


@pytest.mark.parametrize("parameters,targets,mode", [
    ({"additional": Length.of("-1", "mm")}, ("A",), ConstraintMode.REQUIRE),
    ({"additional": Length.of("0.0000001", "mm")}, ("A",), ConstraintMode.REQUIRE),
    ({"additional": Length.of("0", "mm")}, ("Missing",), ConstraintMode.REQUIRE),
    ({"additional": Length.of("0", "mm")}, ("A.1",), ConstraintMode.REQUIRE),
    ({"additional": Length.of("0", "mm")}, ("A",), ConstraintMode.PREFER),
])
def test_direct_electrical_ir_cannot_bypass_policy_validation(parameters, targets, mode):
    electrical = compile_source(source())
    constraint = Constraint(ConstraintKind.COURTYARD_GAP, targets, parameters, mode=mode)
    with pytest.raises(ValueError):
        prototype_physicalize(replace(electrical, constraints=(constraint,)))


def test_audited_rigid_internal_contract_survives_and_explicit_members_only_tighten():
    footprint = PhysicalFootprint("chip", (), Size.mm(1, 0.5))
    digest = footprint_geometry_digest(footprint)
    poses = tuple(Placement(ref, "chip", Point.mm(x, 5)) for ref, x in [("A", 5), ("B", 7), ("C", 10)])
    cluster = RigidPlacementCluster("audited", PlacementTarget("A"), (
        RigidPlacementMember("A", "chip", digest, Point.mm(0, 0)),
        RigidPlacementMember("B", "chip", digest, Point.mm(2, 0)),
    ), "manufacturer layout", internal_clearance_nm=100000)
    board = PhysicalBoard("Rigid", BoardOutline.rectangle(20, 20), {"chip": footprint}, poses, (), Stackup(),
                          rigid_clusters=(cluster,))
    assert component_pair_courtyard_gap_nm(board, "A", "B") == 100000
    assert component_pair_courtyard_gap_nm(board, "A", "C") == 500000
    changed = replace(board, placement_rules=(ComponentPlacementRule("A", additional_courtyard_gap_nm=0),
                                             ComponentPlacementRule("B", additional_courtyard_gap_nm=300000)))
    assert component_pair_courtyard_gap_nm(changed, "A", "B") == 300000
    assert changed.rigid_clusters == board.rigid_clusters


def test_resolver_rejects_unknown_or_same_component():
    for first, second in [("A", "A"), ("A", "Unknown")]:
        with pytest.raises(ValueError):
            component_pair_courtyard_gap_nm(physical(), first, second)


def test_new_immutable_policy_snapshot_does_not_reuse_old_rules_or_members():
    baseline = physical()
    assert component_pair_courtyard_gap_nm(baseline, "A", "B") == 500000
    updated = replace(baseline, placement_rules=(
        ComponentPlacementRule("A", additional_courtyard_gap_nm=0),
        ComponentPlacementRule("B", additional_courtyard_gap_nm=100000),
    ))
    assert component_pair_courtyard_gap_nm(updated, "A", "B") == 100000
    assert component_pair_courtyard_gap_nm(baseline, "A", "B") == 500000
    removed = replace(updated, placements=tuple(p for p in updated.placements if p.reference != "C"))
    with pytest.raises(ValueError):
        component_pair_courtyard_gap_nm(removed, "A", "C")


def test_module_source_and_host_policy_merge_on_actual_qualified_members(tmp_path):
    package = tmp_path / "parts"
    package.mkdir()
    (package / "pair.copper").write_text('''module Pair {
        use library "tiny";
        component X: RESISTOR { value=10 ohm; footprint="Test/Chip"; }
        component Y: RESISTOR { value=10 ohm; footprint="Test/Chip"; }
        constraint courtyard_gap(X, Y) { additional=0.1 mm; }
    }''')
    path = tmp_path / "board.copper"
    path.write_text('''board Generic {
        import p "./parts";
        module BANK: p.Pair;
        constraint courtyard_gap(BANK/X) { additional=0.3 mm; }
    }''')
    board = prototype_physicalize(compile_file(path))
    assert {r.reference: r.additional_courtyard_gap_nm for r in board.placement_rules} == {
        "BANK/X": 300000, "BANK/Y": 100000,
    }
    assert component_pair_courtyard_gap_nm(board, "BANK/X", "BANK/Y") == 300000


def test_renamed_source_targets_follow_identity_not_designator_patterns():
    text = source('constraint courtyard_gap(A, B) { additional=0.1 mm; }')
    for old, new in [("A", "Q17"), ("B", "DECOUPLER"), ("C", "REMOTE")]:
        text = text.replace(old + ":", new + ":").replace("(" + old, "(" + new).replace(", " + old + ")", ", " + new + ")")
    board = prototype_physicalize(compile_source(text))
    assert component_pair_courtyard_gap_nm(board, "Q17", "DECOUPLER") == 100000
    assert component_pair_courtyard_gap_nm(board, "Q17", "REMOTE") == 500000

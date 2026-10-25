import csv
import json
from dataclasses import replace
from pathlib import Path

import pytest

from pcbir.assembly import (
    AssemblyError, Offer, check_assembly, electrical_digest, load_lock, lock_to_json, snapshot, write_candidate_bom,
    write_hand_assembly_list, write_jlcpcb_bom, write_ordering_list,
)
from pcbir.cli import main
from pcbir.compiler import compile_file
from pcbir.model import Board, ComponentInstance, Constraint, ConstraintKind, ModuleDefinition, PartDefinition
from pcbir.quantities import Resistance
from pcbir.serializer import board_to_json

ROOT = Path(__file__).parents[1]


def board_fixture() -> Board:
    part = PartDefinition("R", {}, manufacturer="Example Manufacturer", footprints=("Generic:R",))
    excluded = PartDefinition("M", {}, assembled=False)
    return Board("TestBoard", {"R": part, "M": excluded}, (
        ComponentInstance("R1", "R", Resistance.of(10, "kohm")),
        ComponentInstance("R2", "R", Resistance.of(10, "kohm")),
        ComponentInstance("M1", "M"),
    ), ())


def reviewed_lock(board: Board):
    draft = snapshot(board)
    return replace(draft, selections=tuple(replace(
        entry, mpn="EXAMPLE-10K-0603-1P", supplier="jlcpcb", supplier_part="C123", reviewed=True,
    ) for entry in draft.selections))


def codes(report):
    return {issue["code"] for issue in report["issues"]}


def test_snapshot_is_deterministic_and_excludes_nonassembled():
    board = board_fixture()
    lock = snapshot(board)
    assert [e.reference for e in lock.selections] == ["R1", "R2"]
    assert lock_to_json(lock) == lock_to_json(snapshot(board))
    result = check_assembly(board, lock)
    assert codes(result) == {"UNRESOLVED_SELECTION", "REVIEW_REQUIRED"}
    assert result["availability"] == "not_checked"
    assert result["manufacturing_ready"] is False


def test_complete_selection_roundtrip_and_grouped_bom(tmp_path):
    board = board_fixture()
    lock = reviewed_lock(board)
    path = tmp_path / "assembly.lock"
    path.write_text(lock_to_json(lock), encoding="utf-8")
    assert load_lock(path) == lock
    assert check_assembly(board, lock)["passed"]
    output = tmp_path / "build" / "bom.csv"
    write_jlcpcb_bom(board, lock, output)
    with output.open(newline="", encoding="utf-8") as stream:
        rows = list(csv.DictReader(stream))
    assert len(rows) == 1
    assert rows[0]["Designator"] == "R1,R2"
    assert rows[0]["LCSC Part #"] == "C123"
    assert rows[0]["MPN"] == "EXAMPLE-10K-0603-1P"
    assert rows[0]["Comment"] == "10 kohm"


def test_stale_lock_detects_value_library_and_board_changes():
    board = board_fixture()
    lock = reviewed_lock(board)
    changed_value = replace(board, components=(replace(board.components[0], value=Resistance.of(20, "kohm")), *board.components[1:]))
    changed_library = replace(board, library={**board.library, "R": replace(board.library["R"], metadata={"revision": "new"})})
    for changed in (changed_value, changed_library):
        assert "STALE_LOCK" in codes(check_assembly(changed, lock))
    assert "BOARD_MISMATCH" in codes(check_assembly(replace(board, name="Other"), lock))


@pytest.mark.parametrize("hierarchical", [False, True])
def test_digest_ignores_only_constraint_diagnostic_origins(hierarchical):
    constraint = Constraint(
        ConstraintKind.FIXED_PLACEMENT, ("R1",), {"x": 10, "y": 20},
        origins=(r"C:\checkout\board.copper:95:16",),
    )
    board = board_fixture()
    if hierarchical:
        module = ModuleDefinition("Unit", {}, {}, (), (), (), constraints=(constraint,))
        board = replace(board, module_definitions={"Unit": module})

        def with_constraint(value):
            return replace(board, module_definitions={"Unit": replace(module, constraints=(value,))})
    else:
        board = replace(board, constraints=(constraint,))

        def with_constraint(value):
            return replace(board, constraints=(value,))

    relocated = with_constraint(replace(constraint, origins=("/home/runner/project/board.copper:120:3",)))
    lock = reviewed_lock(board)
    assert electrical_digest(board) == electrical_digest(relocated)
    assert "STALE_LOCK" not in codes(check_assembly(relocated, lock))
    assert board_to_json(board) != board_to_json(relocated)
    assert constraint.origins == (r"C:\checkout\board.copper:95:16",)
    for changed in (
        replace(constraint, parameters={"x": 11, "y": 20}),
        replace(constraint, targets=("R2",)),
        replace(constraint, constraint_id="different"),
    ):
        assert "STALE_LOCK" in codes(check_assembly(with_constraint(changed), lock))


def test_digest_preserves_user_properties_named_origins():
    board = board_fixture()
    changed = replace(board, components=(
        replace(board.components[0], properties={"origins": "user-defined fact"}),
        *board.components[1:],
    ))
    assert electrical_digest(board) != electrical_digest(changed)


def test_coverage_unknown_duplicates_and_mismatches():
    board = board_fixture()
    lock = reviewed_lock(board)
    entry = lock.selections[0]
    changed = replace(lock, selections=(entry, entry, replace(entry, reference="UNKNOWN"),
        replace(entry, reference="M1"), replace(lock.selections[1], part="Wrong", footprint="Wrong", manufacturer="Wrong")))
    assert codes(check_assembly(board, changed)) >= {
        "DUPLICATE_SELECTION", "UNKNOWN_REFERENCE", "NON_ASSEMBLED_REFERENCE",
        "PART_MISMATCH", "FOOTPRINT_MISMATCH", "MANUFACTURER_MISMATCH", "CONFLICTING_OFFER",
    }
    missing = replace(lock, selections=lock.selections[:1])
    assert "MISSING_SELECTION" in codes(check_assembly(board, missing))


@pytest.mark.parametrize("code", ["C0", "123", "C0123", "C-123", "c123"])
def test_invalid_jlcpcb_code(code):
    board = board_fixture()
    lock = reviewed_lock(board)
    changed = replace(lock, selections=tuple(replace(e, supplier_part=code) for e in lock.selections))
    assert "INVALID_SUPPLIER_PART" in codes(check_assembly(board, changed))


def test_export_refuses_unreviewed_incomplete_and_non_jlcpcb_without_writing(tmp_path):
    board = board_fixture()
    full = reviewed_lock(board)
    locks = (snapshot(board), replace(full, selections=full.selections[:1]),
             replace(full, selections=tuple(replace(e, reviewed=False) for e in full.selections)),
             replace(full, selections=tuple(replace(e, supplier="other") for e in full.selections)))
    output = tmp_path / "bom.csv"
    for lock in locks:
        with pytest.raises(AssemblyError):
            write_jlcpcb_bom(board, lock, output)
        assert not output.exists()


def test_loader_rejects_unknown_fields_duplicate_json_and_wrong_types(tmp_path):
    document = json.loads(lock_to_json(snapshot(board_fixture())))
    invalid = []
    invalid.append({**document, "stock": 100})
    invalid.append({**document, "selections": [{**document["selections"][0], "reviewed": "false"}]})
    invalid.append({**document, "selections": [{**document["selections"][0], "mpn": 123}]})
    invalid.append({**document, "selections": [{**document["selections"][0], "mpn": "  unknown "}]})
    invalid.append({**document, "electrical_sha256": "bad"})
    invalid.append({**document, "selections": [{"reference": "R1"}]})
    path = tmp_path / "assembly.lock"
    for data in invalid:
        path.write_text(json.dumps(data), encoding="utf-8")
        with pytest.raises(AssemblyError):
            load_lock(path)
    path.write_text('{"schema": "one", "schema": "two"}', encoding="utf-8")
    with pytest.raises(AssemblyError, match="duplicate JSON"):
        load_lock(path)


def test_hierarchy_uses_qualified_references_without_mutating_ir():
    board = compile_file(ROOT / "examples/hierarchical_board/board.copper")
    before = tuple(board.components)
    lock = snapshot(board)
    assert "PWR/U1" in {e.reference for e in lock.selections}
    assert board.components == before
    assert "PWR/U1" not in {c.ref for c in board.components}


def test_cli_snapshot_never_overwrites_and_partial_check_fails(tmp_path, capsys):
    board = ROOT / "examples/valid_board/board.copper"
    path = tmp_path / "assembly.lock"
    assert main(["assembly", "snapshot", str(board), "-o", str(path)]) == 0
    original = path.read_bytes()
    assert main(["assembly", "snapshot", str(board), "-o", str(path)]) == 2
    assert path.read_bytes() == original
    report_path = tmp_path / "build/assembly.json"
    assert main(["assembly", "check", str(board), "--lock", str(path), "--report", str(report_path)]) == 1
    report = json.loads(report_path.read_text())
    assert report["passed"] is False
    assert report["exact_selection_count"] == 0
    assert "availability not checked" in capsys.readouterr().out


def test_cli_erc_error_prevents_snapshot(tmp_path):
    path = tmp_path / "assembly.lock"
    assert main(["assembly", "snapshot", str(ROOT / "examples/invalid_board/board.copper"), "-o", str(path)]) == 1
    assert not path.exists()


def test_bom_api_checks_erc_independently(tmp_path):
    from pcbir.model import Endpoint, Net
    board = board_fixture()
    broken = replace(board, nets=(Net("BAD", (Endpoint("UNKNOWN", "1"),)),))
    with pytest.raises(AssemblyError, match="ERC failed"):
        write_jlcpcb_bom(broken, reviewed_lock(broken), tmp_path / "bom.csv")
    assert not (tmp_path / "bom.csv").exists()


def mixed_board() -> Board:
    """One part the assembler stocks, one it does not, one deliberately unfitted."""
    stocked = PartDefinition("R", {}, manufacturer="Example Manufacturer", footprints=("Generic:R",))
    specialist = PartDefinition("U", {}, manufacturer="Specialist Semiconductor", footprints=("Generic:QFN",))
    return Board("MixedBoard", {"R": stocked, "U": specialist}, (
        ComponentInstance("R1", "R", Resistance.of(10, "kohm")),
        ComponentInstance("U1", "U"),
        ComponentInstance("R9", "R", Resistance.of(10, "kohm")),
    ), ())


def mixed_lock(board: Board):
    draft = snapshot(board)
    selections = []
    for entry in draft.selections:
        if entry.reference == "U1":          # stocked nowhere; hand-placed from consigned stock
            selections.append(replace(entry, mpn="SPECIAL-1234", reviewed=True, populate="consigned",
                                      supplier="digikey", supplier_part="123-SPECIAL-1234-ND",
                                      source_url="https://example.com/special-1234"))
        elif entry.reference == "R9":        # fitted by nobody
            selections.append(replace(entry, reviewed=True, populate="none"))
        else:
            selections.append(replace(entry, mpn="EXAMPLE-10K-0603-1P", supplier="jlcpcb",
                                      supplier_part="C123", reviewed=True))
    return replace(draft, selections=tuple(selections))


def test_consigned_and_unfitted_selections_pass_without_a_supplier_offer():
    board = mixed_board()
    report = check_assembly(board, mixed_lock(board))
    assert report["passed"], report["issues"]
    assert report["supplier_assembled_count"] == 1
    assert report["consigned"] == ["U1"] and report["not_fitted"] == ["R9"]


def test_bom_carries_only_supplier_assembled_components(tmp_path):
    board = mixed_board()
    output = tmp_path / "bom.csv"
    write_jlcpcb_bom(board, mixed_lock(board), output)
    with output.open(newline="", encoding="utf-8") as stream:
        rows = list(csv.DictReader(stream))
    # U1 and R9 must not reach the assembler: the CPL is built from these
    # designators, so a stray row would ask it to place an unsupplied part.
    assert [row["Designator"] for row in rows] == ["R1"]


def test_consigned_still_needs_a_part_identity_but_no_supplier():
    board = mixed_board()
    lock = mixed_lock(board)
    vague = replace(lock, selections=tuple(
        replace(e, mpn=None) if e.reference == "U1" else e for e in lock.selections))
    assert "UNRESOLVED_SELECTION" in codes(check_assembly(board, vague))
    message = next(i["message"] for i in check_assembly(board, vague)["issues"]
                   if i["code"] == "UNRESOLVED_SELECTION")
    assert "consigned" in message


def test_bom_refuses_a_foreign_supplier_on_a_supplier_assembled_part(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    foreign = replace(lock, selections=tuple(
        replace(e, supplier="other-supplier") if e.reference == "R1" else e for e in lock.selections))
    with pytest.raises(AssemblyError, match="supplier-assembled"):
        write_jlcpcb_bom(board, foreign, tmp_path / "bom.csv")


def test_bom_refuses_when_nothing_is_supplier_assembled(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    none_ordered = replace(lock, selections=tuple(
        replace(e, populate="consigned") if e.ordered else e for e in lock.selections))
    with pytest.raises(AssemblyError, match="nothing to export"):
        write_jlcpcb_bom(board, none_ordered, tmp_path / "bom.csv")


def test_populate_mode_is_validated():
    with pytest.raises(AssemblyError, match="populate must be one of"):
        snapshot(mixed_board()).selections[0].__class__(
            reference="R1", part="R", footprint="Generic:R", populate="maybe")


def test_v0_1_locks_load_and_mean_supplier_assembly(tmp_path):
    board = board_fixture()
    lock = reviewed_lock(board)
    legacy = json.loads(lock_to_json(lock))
    legacy["schema"] = "copperscript-assembly-lock/v0.1"
    for entry in legacy["selections"]:
        del entry["populate"]
    path = tmp_path / "assembly.lock"
    path.write_text(json.dumps(legacy, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    assert load_lock(path) == lock
    assert all(entry.ordered for entry in load_lock(path).selections)


def test_hand_assembly_list_is_exactly_what_the_supplier_bom_omits(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    bom, hand = tmp_path / "bom.csv", tmp_path / "hand.csv"
    write_jlcpcb_bom(board, lock, bom)
    assert write_hand_assembly_list(board, lock, hand) == 2
    with hand.open(newline="", encoding="utf-8") as stream:
        rows = list(csv.DictReader(stream))
    assert {(row["Action"], row["Designator"]) for row in rows} == {
        ("hand-assemble", "U1"), ("do not fit", "R9")}
    consigned = next(row for row in rows if row["Designator"] == "U1")
    assert (consigned["Manufacturer"], consigned["MPN"], consigned["Quantity"]) == (
        "Specialist Semiconductor", "SPECIAL-1234", "1")
    # Where to buy it travels with the part, so the list is an order sheet.
    assert (consigned["Supplier"], consigned["Supplier Part"], consigned["URL"]) == (
        "digikey", "123-SPECIAL-1234-ND", "https://example.com/special-1234")
    with bom.open(newline="", encoding="utf-8") as stream:
        supplied = {ref for row in csv.DictReader(stream) for ref in row["Designator"].split(",")}
    # Together the two files cover every populated component, with no overlap.
    assert supplied | {"U1", "R9"} == {e.reference for e in lock.selections}
    assert not supplied & {"U1", "R9"}



def test_ordering_list_covers_every_selection_with_its_sourcing(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    sheet = tmp_path / "ordering.csv"
    assert write_ordering_list(board, lock, sheet) == len(lock.selections)
    with sheet.open(newline="", encoding="utf-8") as stream:
        rows = list(csv.DictReader(stream))
    listed = {ref: row for row in rows for ref in row["Designator"].split(",")}
    assert set(listed) == {e.reference for e in lock.selections}
    assert sum(int(row["Quantity"]) for row in rows) == len(lock.selections)
    for entry in lock.selections:
        row = listed[entry.reference]
        assert (row["MPN"], row["Supplier"], row["Supplier Part"], row["URL"]) == (
            entry.mpn or "", entry.supplier or "", entry.supplier_part or "", entry.source_url or "")
        assert row["Assembly"] == {"supplier": "supplier-assembled", "consigned": "hand-assemble",
                                   "none": "do not fit"}[entry.populate]
    unreviewed = replace(lock, selections=tuple(replace(e, reviewed=False) for e in lock.selections))
    with pytest.raises(AssemblyError, match="ordering list refused"):
        write_ordering_list(board, unreviewed, tmp_path / "refused.csv")


def test_alternate_offers_roundtrip_only_in_v0_3_and_fill_one_column_per_distributor(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    plain = json.loads(lock_to_json(lock))
    assert plain["schema"] == "copperscript-assembly-lock/v0.2"
    assert all("alternates" not in entry for entry in plain["selections"])
    stocked = Offer("DigiKey", "EX-10K-ND", "https://example.com/ex-10k")
    lock = replace(lock, selections=tuple(
        replace(e, alternates=(stocked,)) if e.reference == "R1" else e for e in lock.selections))
    path = tmp_path / "assembly.lock"
    path.write_text(lock_to_json(lock), encoding="utf-8")
    assert json.loads(path.read_text())["schema"] == "copperscript-assembly-lock/v0.3"
    assert load_lock(path) == lock
    assert check_assembly(board, lock)["passed"]
    sheet = tmp_path / "ordering.csv"
    write_ordering_list(board, lock, sheet)
    with sheet.open(newline="", encoding="utf-8") as stream:
        rows = {row["Designator"]: row for row in csv.DictReader(stream)}
    # The primary supplier is unchanged; the DigiKey column also carries U1's
    # primary digikey offer, matched without regard to case.
    assert (rows["R1"]["Supplier"], rows["R1"]["Supplier Part"]) == ("jlcpcb", "C123")
    assert (rows["R1"]["DigiKey Part"], rows["R1"]["DigiKey URL"]) == ("EX-10K-ND", "https://example.com/ex-10k")
    assert rows["U1"]["DigiKey Part"] == "123-SPECIAL-1234-ND"
    assert rows["R9"]["DigiKey Part"] == ""
    bom = tmp_path / "bom.csv"
    write_jlcpcb_bom(board, lock, bom)
    assert "EX-10K-ND" not in bom.read_text(encoding="utf-8")
    downgraded = json.loads(path.read_text())
    downgraded["schema"] = "copperscript-assembly-lock/v0.2"
    path.write_text(json.dumps(downgraded), encoding="utf-8")
    with pytest.raises(AssemblyError, match="alternates require"):
        load_lock(path)


def test_alternate_offers_are_checked_like_the_primary_offer():
    board = mixed_board()
    lock = mixed_lock(board)
    def with_alternates(*offers):
        return replace(lock, selections=tuple(
            replace(e, alternates=offers) if e.reference == "R1" else e for e in lock.selections))
    assert codes(check_assembly(board, with_alternates(Offer("JLCPCB", "C123")))) == {"DUPLICATE_OFFER"}
    assert codes(check_assembly(board, with_alternates(Offer("jlcpcb", "123")))) == {"INVALID_SUPPLIER_PART"}
    assert codes(check_assembly(board, with_alternates(Offer("digikey", "123-SPECIAL-1234-ND")))) == {
        "CONFLICTING_OFFER"}
    with pytest.raises(AssemblyError, match="supplier_part must be"):
        Offer("digikey", " ")

def test_hand_assembly_list_refuses_an_unreviewed_lock(tmp_path):
    board = mixed_board()
    lock = mixed_lock(board)
    unreviewed = replace(lock, selections=tuple(replace(e, reviewed=False) for e in lock.selections))
    with pytest.raises(AssemblyError, match="hand-assembly list refused"):
        write_hand_assembly_list(board, unreviewed, tmp_path / "hand.csv")


def hierarchical_board() -> Board:
    part = PartDefinition("R", {}, manufacturer="Example Manufacturer", footprints=("Generic:R",))
    return Board("Hier", {"R": part}, (
        ComponentInstance("BUCK/R1", "R", Resistance.of(10, "kohm")),
        ComponentInstance("BUCK/R2", "R", Resistance.of(10, "kohm")),
        ComponentInstance("R3", "R", Resistance.of(10, "kohm")),
    ), ())


def test_bom_and_hand_list_use_the_board_designator_not_the_source_path(tmp_path):
    # The KiCad export flattens BUCK/R1 to BUCK_R1, and the assembler matches the
    # BOM against the CPL and silkscreen by that designator.
    board = hierarchical_board()
    lock = reviewed_lock(board)
    lock = replace(lock, selections=tuple(
        replace(e, populate="consigned") if e.reference == "BUCK/R2" else e for e in lock.selections))
    bom, hand = tmp_path / "bom.csv", tmp_path / "hand.csv"
    write_jlcpcb_bom(board, lock, bom)
    write_hand_assembly_list(board, lock, hand)
    with bom.open(newline="", encoding="utf-8") as stream:
        assert [r["Designator"] for r in csv.DictReader(stream)] == ["BUCK_R1,R3"]
    with hand.open(newline="", encoding="utf-8") as stream:
        assert [r["Designator"] for r in csv.DictReader(stream)] == ["BUCK_R2"]


def test_references_that_flatten_to_one_designator_are_refused(tmp_path):
    part = PartDefinition("R", {}, manufacturer="Example Manufacturer", footprints=("Generic:R",))
    board = Board("Clash", {"R": part}, (
        ComponentInstance("A/B", "R", Resistance.of(1, "kohm")),
        ComponentInstance("A_B", "R", Resistance.of(1, "kohm")),
    ), ())
    with pytest.raises(AssemblyError, match="both become board designator A_B"):
        write_jlcpcb_bom(board, reviewed_lock(board), tmp_path / "bom.csv")


def test_candidate_bundle_preserves_unreviewed_flags_and_strict_export(tmp_path):
    board = board_fixture()
    lock = replace(reviewed_lock(board), selections=tuple(
        replace(e, reviewed=False) for e in reviewed_lock(board).selections))
    data = write_candidate_bom(board, lock, tmp_path / 'candidate')
    assert not data['passed'] and not data['ordering_approved']
    assert not data['qualified_release'] and data['reviewed_count'] == 0
    assert all(not e.reviewed for e in lock.selections)
    assert (tmp_path / 'candidate/bom-candidate-NOT-APPROVED.csv').is_file()
    assert json.loads((tmp_path / 'candidate/assembly-review.json').read_text()) == data
    with pytest.raises(AssemblyError):
        write_jlcpcb_bom(board, lock, tmp_path / 'production.csv')
    with pytest.raises(FileExistsError):
        write_candidate_bom(board, lock, tmp_path / 'candidate')


@pytest.mark.parametrize('change', ['missing', 'stale', 'unresolved'])
def test_candidate_refuses_bad_selection_identity_without_output(tmp_path, change):
    board = board_fixture()
    lock = reviewed_lock(board)
    if change == 'missing':
        lock = replace(lock, selections=lock.selections[:1])
    elif change == 'stale':
        lock = replace(lock, electrical_sha256='0' * 64)
    elif change == 'unresolved':
        lock = snapshot(board)
    with pytest.raises(AssemblyError):
        write_candidate_bom(board, lock, tmp_path / 'candidate')
    assert not (tmp_path / 'candidate').exists()


def test_mixed_supplier_candidate_keeps_jlc_bom_and_cpl_subset_exact(tmp_path):
    board = board_fixture()
    original = reviewed_lock(board)
    lock = replace(original, selections=(original.selections[0],
        replace(original.selections[1], supplier='digikey', supplier_part='EXACT-CT-ND')))
    positions = tmp_path / 'native.csv'
    positions.write_text('Ref,PosX,PosY,Rot,Side\nR1,1,-2,90,top\nR2,3,-4,180,bottom\n')
    out = tmp_path / 'candidate'
    data = write_candidate_bom(board, lock, out, native_positions=positions)
    assert data['jlcpcb_references'] == ['R1']
    assert data['other_supplier_references'] == ['R2']
    for name in ('bom-candidate-NOT-APPROVED.csv', 'cpl-candidate-NOT-APPROVED.csv'):
        with (out / name).open(newline='') as stream:
            assert {r['Designator'] for r in csv.DictReader(stream)} == {'R1'}
    with (out / 'all-suppliers-NOT-APPROVED.csv').open(newline='') as stream:
        rows = list(csv.DictReader(stream))
    assert rows[1]['Supplier Part'] == 'EXACT-CT-ND'
    assert {r['Designator'] for r in rows} == {'R1', 'R2'}
    with pytest.raises(AssemblyError, match='jlcpcb offers'):
        write_jlcpcb_bom(board, lock, tmp_path / 'production.csv')
    missing = tmp_path / 'missing.csv'
    missing.write_text('Ref,PosX,PosY,Rot,Side\nR1,1,-2,90,top\n')
    with pytest.raises(AssemblyError, match='candidate CPL rejected'):
        write_candidate_bom(board, lock, tmp_path / 'incomplete', native_positions=missing)


def test_other_supplier_only_candidate_does_not_emit_jlc_files(tmp_path):
    board = board_fixture()
    lock = replace(reviewed_lock(board), selections=tuple(
        replace(e, supplier='lcsc') for e in reviewed_lock(board).selections))
    out = tmp_path / 'candidate'
    data = write_candidate_bom(board, lock, out)
    assert data['jlcpcb_references'] == []
    assert data['other_supplier_references'] == ['R1', 'R2']
    assert (out / 'all-suppliers-NOT-APPROVED.csv').is_file()
    assert not (out / 'bom-candidate-NOT-APPROVED.csv').exists()


def test_hierarchical_bom_uses_native_references_without_changing_lock(tmp_path):
    board = board_fixture()
    board = replace(board, components=(replace(board.components[0], ref='BANK/R1'), *board.components[1:]))
    lock = reviewed_lock(board)
    path = tmp_path / 'bom.csv'
    write_jlcpcb_bom(board, lock, path)
    with path.open(newline='', encoding='utf-8') as stream:
        rows = list(csv.DictReader(stream))
    assert rows[0]['Designator'] == 'BANK_R1,R2'
    assert lock.selections[0].reference == 'BANK/R1'
    data = write_candidate_bom(board, lock, tmp_path / 'candidate')
    assert data['native_reference_map']['BANK/R1'] == 'BANK_R1'


def test_native_reference_collision_refuses_bom_and_candidate(tmp_path):
    board = board_fixture()
    board = replace(board, components=(replace(board.components[0], ref='BANK/R1'),
                                       replace(board.components[1], ref='BANK_R1')))
    lock = reviewed_lock(board)
    with pytest.raises(AssemblyError, match='both become board designator'):
        write_jlcpcb_bom(board, lock, tmp_path / 'bom.csv')
    with pytest.raises(AssemblyError, match='both become board designator'):
        write_candidate_bom(board, lock, tmp_path / 'candidate')
    assert not (tmp_path / 'bom.csv').exists() and not (tmp_path / 'candidate').exists()


def test_candidate_cpl_matches_native_references_and_excludes_dnp(tmp_path):
    from hashlib import sha256
    board = board_fixture()
    lock = reviewed_lock(board)
    positions = tmp_path / 'native.csv'
    positions.write_text('Ref,PosX,PosY,Rot,Side\nR1,1,-2,90,top\nR2,3,-4,180,bottom\nM1,5,-6,0,top\n')
    data = write_candidate_bom(board, lock, tmp_path / 'candidate', native_positions=positions)
    with (tmp_path / 'candidate/cpl-candidate-NOT-APPROVED.csv').open(newline='') as stream:
        rows = list(csv.DictReader(stream))
    assert {r['Designator'] for r in rows} == {'R1', 'R2'}
    assert rows[1]['Rotation'] == '180' and rows[1]['Layer'] == 'Bottom'
    assert data['native_positions_sha256'] == sha256(positions.read_bytes()).hexdigest()
    assert 'not_approved' in data['placement_review']
    assert not data['ordering_approved']


def test_candidate_cpl_missing_native_reference_never_writes_completion_report(tmp_path):
    board = board_fixture()
    positions = tmp_path / 'native.csv'
    positions.write_text('Ref,PosX,PosY,Rot,Side\nR1,1,-2,90,top\n')
    with pytest.raises(AssemblyError, match='candidate CPL rejected'):
        write_candidate_bom(board, reviewed_lock(board), tmp_path / 'candidate', native_positions=positions)
    assert not (tmp_path / 'candidate/assembly-review.json').exists()
    assert not (tmp_path / 'candidate/cpl-candidate-NOT-APPROVED.csv').exists()

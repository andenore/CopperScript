import csv
import json
from dataclasses import replace
from pathlib import Path

import pytest

from pcbir.assembly import (
    AssemblyError, check_assembly, electrical_digest, load_lock, lock_to_json, snapshot, write_jlcpcb_bom,
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
    board = compile_file(ROOT / "examples/hierarchical_board.copper")
    before = tuple(board.components)
    lock = snapshot(board)
    assert "PWR/U1" in {e.reference for e in lock.selections}
    assert board.components == before
    assert "PWR/U1" not in {c.ref for c in board.components}


def test_cli_snapshot_never_overwrites_and_partial_check_fails(tmp_path, capsys):
    board = ROOT / "examples/valid_board.copper"
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
    assert main(["assembly", "snapshot", str(ROOT / "examples/invalid_board.copper"), "-o", str(path)]) == 1
    assert not path.exists()


def test_bom_api_checks_erc_independently(tmp_path):
    from pcbir.model import Endpoint, Net
    board = board_fixture()
    broken = replace(board, nets=(Net("BAD", (Endpoint("UNKNOWN", "1"),)),))
    with pytest.raises(AssemblyError, match="ERC failed"):
        write_jlcpcb_bom(broken, reviewed_lock(broken), tmp_path / "bom.csv")
    assert not (tmp_path / "bom.csv").exists()

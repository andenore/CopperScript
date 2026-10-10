from hashlib import sha256
import json
from pathlib import Path

import pytest

from pcbir.cam_audit import audit_cam, reconcile_native_ipc, load_cam_evidence_index
from pcbir.cam_qualification import (parse_ipcd356, NormalizedTestNet, TestPoint, TestVia,
    PyGerberAdapter, CamCorpusCase, ToolIdentity, CamQualificationProfile, run_cam_qualification_matrix)
from pcbir.cli import main
from pcbir.manufacturing import CommandResult
from pcbir.manufacturing_files import export_manufacturing_files
from test_manufacturing_files import pcb_fixture, fake_kicad


def native_export(command, cwd):
    result = fake_kicad(command, cwd)
    if command[1:4] == ("pcb", "export", "gerbers"):
        output = Path(command[command.index("-o") + 1])
        for path in output.glob("*.gbr"):
            layer = path.stem.split("round-", 1)[1]
            function = {"F_Cu": "Copper,L1,Top", "B_Cu": "Copper,L2,Bot",
                        "F_Mask": "Soldermask,Top", "B_Mask": "Soldermask,Bot",
                        "F_Paste": "Paste,Top", "B_Paste": "Paste,Bot",
                        "F_Silkscreen": "Legend,Top", "B_Silkscreen": "Legend,Bot",
                        "Edge_Cuts": "Profile,NP"}[layer]
            path.write_text(f"%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,{function}*%\nM02*\n", encoding="ascii")
    elif command[1:4] == ("pcb", "export", "drill"):
        output = Path(command[command.index("-o") + 1])
        (output / "round-PTH.drl").write_text("M48\n; #@! TF.FileFunction,Plated,1,2,PTH\nMETRIC\nT1C0.300\n%\nT1\nX1.0Y-2.0\nM30\n")
    elif command[1:4] == ("pcb", "export", "ipcd356"):
        output = Path(command[command.index("-o") + 1])
        output.write_text("P  CODE 00\nP  UNITS CUST 0\n"
                          "327SIGNAL           J1    -1          A01X+001181Y-002362X0236Y0236R000S2\n999\n")
    return result


def package(tmp_path):
    return export_manufacturing_files(pcb_fixture(tmp_path), tmp_path / "package",
                                     kicad_cli=Path("fixture"), skip_independent_cam=True, runner=native_export)


def by_id(result, identifier):
    return next(item for item in result["checks"] if item["id"] == identifier)


def test_real_artifact_path_runs_and_missing_independent_geometry_stays_incomplete(tmp_path):
    root = package(tmp_path)
    result = audit_cam(root)
    assert result["status"] == "incomplete"
    assert not result["qualified_release"]
    for identifier in ("cam.layers", "cam.checksums", "cam.native-binding", "cam.native-drc", "cam.drill-parse", "cam.netlist-parse"):
        assert by_id(result, identifier)["status"] == "pass"
    for identifier in ("cam.independent-parsers", "cam.copper-connectivity", "cam.fabrication-geometry"):
        assert by_id(result, identifier)["status"] == "incomplete"


def test_changed_exported_bytes_fail_checksum_even_if_second_parser_missing(tmp_path):
    root = package(tmp_path)
    next((root / "gerbers").glob("*.gbr")).write_text("wrong artwork")
    assert audit_cam(root)["status"] == "fail"


def test_unhashed_extra_file_is_not_silently_accepted(tmp_path):
    root = package(tmp_path)
    (root / "wrong-layer.gbr").write_text("bad extra layer")
    assert by_id(audit_cam(root), "cam.checksums")["status"] == "fail"


def test_native_round_drill_multiset_reconciles_and_duplicate_fails(tmp_path):
    root = package(tmp_path)
    native = {"drills": [{"plated": True, "diameter_nm": 300000, "position": [1000000, 2000000]}],
              "contacts": [{"net": "SIGNAL", "component": "J1", "pad": "1", "position": [2999740, 5999480]}],
              "vias": [], "unsupported": []}
    result = audit_cam(root, native=native)
    assert by_id(result, "cam.drill-reconciliation")["status"] == "pass"
    assert by_id(result, "cam.contact-reconciliation")["status"] == "pass"
    native["drills"] *= 2
    assert by_id(audit_cam(root, native=native), "cam.drill-reconciliation")["status"] == "fail"


def test_unsupported_slots_are_incomplete_not_pass(tmp_path):
    root = package(tmp_path)
    result = audit_cam(root, native={"unsupported": ["slot"]})
    assert by_id(result, "cam.native-reconciliation")["status"] == "incomplete"


def test_cli_writes_new_report_and_exits_nonzero_for_incomplete(tmp_path, monkeypatch):
    root = package(tmp_path)
    monkeypatch.setattr("pcbir.cam_audit.available_adapters", lambda **kwargs: ())
    output = tmp_path / "audit.json"
    assert main(["audit-cam", str(root), "--report", str(output)]) == 1
    assert json.loads(output.read_text())["status"] == "incomplete"
    assert main(["audit-cam", str(root), "--report", str(output)]) == 2
    assert main(["audit-cam", str(root), "--report", str(root / "audit.json")]) == 2


def test_ipcd356_six_character_reference_without_whitespace_before_separator(tmp_path):
    path = tmp_path / "native.d356"
    path.write_text("P  UNITS CUST 0\n327PWR/BAT          PWR_J_-1          A01X+026575Y-026611X0512Y1772R000S2\n999\n")
    parsed = parse_ipcd356(path)
    assert parsed.points[0].component == "PWR_J_"
    assert parsed.points[0].pad == "1"
    path.write_text("327PWR/BAT          PWR_J_-1          A01X+026575Y-026611X0512Y1772R000S2\n999\n")
    with pytest.raises(ValueError, match="unit"):
        parse_ipcd356(path)


def test_alias_partition_handles_truncated_references_but_rejects_merged_nets():
    native = {"contacts": [
        {"net": "very.long.source.signal.a", "component": "PWR_C_ONE", "pad": "1", "position": [1000, 2000]},
        {"net": "very.long.source.signal.b", "component": "PWR_C_TWO", "pad": "1", "position": [9000, 2000]},
        {"net": "very.long.source.signal.a", "component": "PWR_C_ONE", "pad": "2", "position": [2000, 2000]}], "vias": []}
    parsed = NormalizedTestNet((TestPoint("ALIAS_A", "PWR_C_", "1", 1000, -2000),
                               TestPoint("ALIAS_B", "PWR_C_", "1", 9000, -2000),
                               TestPoint("ALIAS_A", "PWR_C_", "2", 2000, -2000)))
    assert all(item["status"] == "pass" for item in reconcile_native_ipc(native, parsed))
    merged = NormalizedTestNet(tuple(TestPoint("MERGED", p.component, p.pad, p.x_nm, p.y_nm) for p in parsed.points))
    assert reconcile_native_ipc(native, merged)[-1]["status"] == "fail"
    split = NormalizedTestNet((*parsed.points[:2], TestPoint("WRONG", "PWR_C_", "2", 2000, -2000)))
    assert reconcile_native_ipc(native, split)[-1]["status"] == "fail"


def test_expanded_positive_negative_pygerber_corpus_does_not_fake_second_parser():
    adapter = PyGerberAdapter()
    missing = ToolIdentity("missing-independent-parser", "fixture", "1" * 64)
    root = Path(__file__).parent / "fixtures/cam"
    cases = tuple(CamCorpusCase(path.stem, path, not path.stem.startswith("negative"))
                  for path in sorted(root.glob("*.gbr")))
    profile = CamQualificationProfile("corpus-regression", "source-pinned profile still required", "source-pinned profile still required",
                                      (adapter.identity, missing))
    result = run_cam_qualification_matrix(profile, cases, (adapter,))
    assert result.status.value == "incomplete"
    assert len(result.cells) == len(cases)
    assert all(cell.passed for cell in result.cells), result.cells


def test_cam_external_evidence_retains_bytes_and_rejects_changed_report(tmp_path):
    root = package(tmp_path)
    baseline = audit_cam(root)
    artifact = tmp_path / "review.txt"
    artifact.write_text("fixture reviewed report")
    document = {"schema": "copperscript-engineering-evidence/v0.1", "id": "cam.copper-connectivity",
                "stage": "design", "inputs_sha256": baseline["inputs_sha256"], "status": "pass",
                "reviewed_by": "fixture", "report_path": "review.txt", "report_sha256": sha256(artifact.read_bytes()).hexdigest(),
                "settings_sha256": "1" * 64, "model_sha256": "2" * 64,
                "tool": {"name": "fixture", "version": "fixture", "identity_sha256": "3" * 64}}
    (tmp_path / "evidence.json").write_text(json.dumps(document))
    index = tmp_path / "index.json"
    index.write_text(json.dumps({"schema": "copperscript-cam-evidence-index/v0.1",
                                "results": {"cam.copper-connectivity": "evidence.json"}}))
    external = load_cam_evidence_index(index)
    result = audit_cam(root, external=external)
    assert by_id(result, "cam.copper-connectivity")["status"] == "pass"
    assert result["status"] == "incomplete"  # Other mandatory gates not supplied.
    assert result["evidence_artifacts"]["cam.copper-connectivity"]["report_sha256"] == document["report_sha256"]
    artifact.write_text("changed report")
    with pytest.raises(ValueError, match="mismatch"):
        load_cam_evidence_index(index)


def test_ipc_auxiliary_origin_is_not_confused_with_absolute_drill_origin():
    native = {"contacts": [{"net": "A", "component": "U1", "pad": "1", "position": [10000, 20000]}],
              "vias": [{"net": "A", "drill_nm": 300000, "position": [20000, 30000]}], "ipc_origin_nm": [5000, 10000]}
    parsed = NormalizedTestNet((TestPoint("ALIAS", "U1", "1", 5000, -10000),),
                               (TestVia("ALIAS", 15000, -20000, 300000),))
    assert all(item["status"] == "pass" for item in reconcile_native_ipc(native, parsed))

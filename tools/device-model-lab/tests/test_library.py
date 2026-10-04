import shutil
import json
from pathlib import Path
import pytest

from copperscript_stm32g0 import core
from pcbir.devicegen import check_generated, load_bundle, render_bundle, validate_bundle
from copperscript_stm32g0.cubemx import CubeMXError, ingest, packet, reconcile
from copperscript_stm32g0.compatibility import build_report, markdown

FIXTURE_CUBEMX = Path(__file__).parent / "fixtures" / "cubemx"


def test_exact_upstream_validation_and_cross_references():
    bundle = load_bundle(core.BUNDLE)
    assert validate_bundle(bundle) == ()
    assert [row["name"] for row in bundle.pads] == ["PA0", "PA1", "PC0", "PC1"]
    assert [row["bond"] for row in bundle.parts[0].pins] == ["PA0", "PA1", "PC0", "PC1"]
    assert core.evidence_errors() == []


def test_evidence_mismatch_is_rejected(tmp_path, monkeypatch):
    evidence = tmp_path / "evidence.jsonl"
    evidence.write_text(
        core.EVIDENCE.read_text(encoding="utf-8").replace(
            '"fact_id":"pin.pa0.number","subject":"PA0","field":"package_pin","value":25',
            '"fact_id":"pin.pa0.number","subject":"PA0","field":"package_pin","value":99',
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(core, "EVIDENCE", evidence)
    assert "PA0.package_pin does not match pins.csv" in core.evidence_errors()


def test_deterministic_upstream_copper_output():
    expected = render_bundle(load_bundle(core.BUNDLE))
    assert expected == {path.name: path.read_text(encoding="utf-8") for path in core.OUTPUT.glob("*.copper")}


def test_incomplete_scope_blocks_publication():
    report = core.coverage()
    assert report["complete_coverage"] is False
    assert report["production_publishable"] is False


def test_unresolved_blocks_upstream_validation(tmp_path):
    bundle_dir = tmp_path / "bundle"
    shutil.copytree(core.BUNDLE, bundle_dir)
    pads = bundle_dir / "pads.csv"
    pads.write_text(pads.read_text(encoding="utf-8").replace("PA0,digital", "PA0,?"), encoding="utf-8")
    assert validate_bundle(load_bundle(bundle_dir))


def test_upstream_check_matches_committed_outputs():
    assert check_generated(load_bundle(core.BUNDLE), core.OUTPUT) == ()


def test_coverage_is_conspicuous():
    assert core.coverage() == {
        "orderable_part": "STM32G0B1CBT6",
        "package": "LQFP48",
        "verified_package_pins": 4,
        "covered_pads": 4,
        "peripheral_signals": 0,
        "mux_options": 0,
        "scope": "proof-of-concept",
        "complete_coverage": False,
        "production_publishable": False,
        "unresolved_facts": 0,
        "illustrative_facts": 0,
    }


def test_cubemx_namespace_agnostic_pin_and_signal_extraction(tmp_path):
    first = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx", tmp_path / "extract.json")
    assert first["package"] == "LQFP48"
    assert [pin["name"] for pin in first["pins"] if pin["name"].startswith(("PA", "PC"))] == ["PC0", "PC1", "PA0", "PA1"]
    assert first["pins"][-1]["signals"][0]["name"] == "SPI1_SCK/I2S1_CK"
    assert first["cube_mx_version"] == "6.12.0"
    assert len(first["source_manifest"]["files"]) == 4
    assert (tmp_path / "extract.json").exists()
    manifest = json.loads((tmp_path / "extract.manifest.json").read_text(encoding="utf-8"))
    assert manifest["database_version"] == "6.1.0"
    assert manifest["files"] == first["source_manifest"]["files"]


def test_cubemx_referenced_ip_files_are_hashed_and_reconcile_exactly():
    artifact = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx")
    report = reconcile(artifact, core.BUNDLE, core.EVIDENCE, ["PA0", "PA1", "PC0", "PC1"])
    assert report["exact_agreement"] is True
    assert report["errors"] == []
    assert {entry["path"] for entry in artifact["source_manifest"]["files"]} == {
        "db/mcu/families.xml",
        "db/mcu/IP/GPIO-gpiog0_v1_0_Cube_Modes.xml",
        "db/mcu/IP/GPIO-gpiog0_v1_0_Cube_Parameters.xml",
        "db/mcu/STM32G0B1CBTx.xml",
    }


def test_cubemx_extraction_is_deterministic(tmp_path):
    left = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx", tmp_path / "left.json")
    right = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx", tmp_path / "right.json")
    assert left == right
    assert (tmp_path / "left.json").read_text(encoding="utf-8") == (tmp_path / "right.json").read_text(encoding="utf-8")


def test_cubemx_mismatch_blocks_reconciliation():
    artifact = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx")
    next(pin for pin in artifact["pins"] if pin["name"] == "PA0")["position"] = "99"
    report = reconcile(artifact, core.BUNDLE, core.EVIDENCE, ["PA0"])
    assert report["exact_agreement"] is False
    assert any("conflict" in error for error in report["errors"])
    assert report["conflicts"]


def test_core_validation_wires_cubemx_gate(monkeypatch):
    original = core.cubemx_ingest

    def mismatching_ingest(*args, **kwargs):
        artifact = original(*args, **kwargs)
        artifact["pins"][0]["position"] = "99"
        return artifact

    monkeypatch.setattr(core, "cubemx_ingest", mismatching_ingest)
    with pytest.raises(RuntimeError, match="conflict"):
        core.validate(FIXTURE_CUBEMX, "STM32G0B1CBTx")


def test_cubemx_bounded_packet_contains_only_requested_pins():
    artifact = ingest(FIXTURE_CUBEMX, "STM32G0B1CBTx")
    bounded = packet(artifact, ["PC0"])
    assert [pin["name"] for pin in bounded["pins"]] == ["PC0"]
    assert "PA0" not in json.dumps(bounded)
    assert "I2C3_SCL" in json.dumps(bounded)


def test_cross_vendor_report_is_deterministic_and_source_backed():
    first = build_report()
    second = build_report()
    assert first == second
    assert [case["orderable_part"] for case in first["cases"]] == [
        "nRF52840-QIAA", "CYUSB4014-FCAXI", "AD4134BCPZ", "OPA2197ID"
    ]
    assert all(case["validation_errors"] == [] for case in first["cases"])
    assert all(case["production_publishable"] is False for case in first["cases"])
    assert "unrepresentable" in markdown(first)


def test_cross_vendor_report_detects_required_model_gaps():
    report = build_report()
    gaps = {(concept["id"], concept["classification"]) for case in report["cases"] for concept in case["concepts"]}
    assert ("closed_part_kind", "unrepresentable") in gaps
    assert ("differential_pair_grouping", "unrepresentable") in gaps
    assert ("wildcard_pin_routing", "expansion-risk") in gaps
    assert ("repeated_functional_units", "unrepresentable") in gaps


def test_cross_vendor_evidence_preserves_mode_aliases_and_package_rules():
    report = build_report()
    cases = {case["id"]: case for case in report["cases"]}
    assert cases["cyusb4014-fcaxi"]["fact_count"] == 7
    assert cases["ad4134bcpz"]["fact_count"] == 8
    fx10 = (core.ROOT / "data/case-studies/cyusb4014-fcaxi/evidence.jsonl").read_text(encoding="utf-8")
    adc = (core.ROOT / "data/case-studies/ad4134bcpz/evidence.jsonl").read_text(encoding="utf-8")
    assert '"ball":"B1","signal":"P0D7P"' in fx10
    assert '"B1":"P0D15"' in fx10
    assert '"field":"required_connection","value":"AGND5"' in adc
    assert '"field":"supply_ranges"' in adc


def test_cubemx_rejects_reference_outside_database(tmp_path):
    fixture = tmp_path / "cubemx"
    shutil.copytree(FIXTURE_CUBEMX, fixture)
    mcu = fixture / "db" / "mcu" / "STM32G0B1CBTx.xml"
    mcu.write_text(
        mcu.read_text(encoding="utf-8").replace(
            "</Mcu>", '<IP Ref="../../outside.xml" /></Mcu>'
        ),
        encoding="utf-8",
    )
    (fixture / "outside.xml").write_text("<outside />", encoding="utf-8")
    with pytest.raises(CubeMXError, match="escapes CubeMX db/mcu"):
        ingest(fixture, "STM32G0B1CBTx")

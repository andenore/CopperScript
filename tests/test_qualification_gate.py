import json
from pathlib import Path

import pytest

from pcbir.qualification import digest_json
from pcbir.qualification_gate import qualification_gate
from pcbir.cam_audit import audit_cam
from pcbir.engineering_qualification import assess_engineering, context_check
from test_cam_audit import package


def setup_reports(tmp_path):
    root = package(tmp_path)
    plan = tmp_path / "plan.json"
    contract = tmp_path / "contract.json"
    contract.write_text(json.dumps({"schema": "copperscript-engineering-contract/v0.1", "id": "fixture",
        "kind": "rf", "matching": "unresolved", "sources": [], "requirements": [
            {"id": identifier, "stage": stage, "algorithm": "external", "description": "fixture qualification"}
            for identifier, stage in (("matching-review", "design"), ("impedance", "simulation"), ("prototype-rf", "bench"))]}))
    source = tmp_path / "source.copper"
    source.write_text("// fixture source")
    plan.write_text(json.dumps({"schema": "copperscript-engineering-plan/v0.1", "context": {},
        "input_files": {"source": "source.copper"}, "contracts": [{"scope": "fixture", "path": "contract.json"}]}))
    cam = tmp_path / "cam.json"
    engineering = tmp_path / "engineering.json"
    cam.write_text(json.dumps(audit_cam(root)))
    engineering.write_text(json.dumps(assess_engineering(root / "round.kicad_pcb", plan)))
    return root, plan, cam, engineering


def test_incomplete_reports_cannot_pass_checkpoint(tmp_path):
    values = setup_reports(tmp_path)
    result = qualification_gate(*values)
    assert result["status"] == "incomplete"
    assert result["qualified_release"] is False


def test_pair_screen_is_auxiliary_and_algorithm_bound(tmp_path):
    root, plan, cam, engineering = setup_reports(tmp_path)
    document = json.loads(plan.read_text())
    document["differential_pair_screens"] = [dict(id="usb.geometry", positive_net="DP", negative_net="DM",
                                                maximum_search_gap_nm=400000)]
    plan.write_text(json.dumps(document))
    result = assess_engineering(root / "round.kicad_pcb", plan)
    assert "pair_geometry.py" in result["inputs"]["algorithms"]
    assert result["screening"][0]["status"] == "incomplete"
    assert "usb.geometry" not in {item["id"] for item in result["checks"]}
    assert result["qualified_release"] is False


def test_pair_and_microstrip_screen_ids_cannot_collide(tmp_path):
    root, plan, _, _ = setup_reports(tmp_path)
    document = json.loads(plan.read_text())
    document["microstrip_screens"] = [dict(id="same")]
    document["differential_pair_screens"] = [dict(id="same")]
    plan.write_text(json.dumps(document))
    with pytest.raises(ValueError, match="duplicate auxiliary"):
        assess_engineering(root / "round.kicad_pcb", plan)


@pytest.mark.parametrize("changed", ["source", "native", "plan", "contract"])
def test_changed_inputs_invalidate_prior_reports(tmp_path, changed):
    root, plan, cam, engineering = setup_reports(tmp_path)
    path = {"source": tmp_path / "source.copper", "native": root / "round.kicad_pcb",
            "plan": plan, "contract": tmp_path / "contract.json"}[changed]
    path.write_text(path.read_text() + "\n")
    result = qualification_gate(root, plan, cam, engineering)
    assert result["status"] == "fail"


def test_claimed_pass_with_incomplete_checks_is_rejected(tmp_path):
    root, plan, cam, engineering = setup_reports(tmp_path)
    document = json.loads(cam.read_text())
    document["status"] = "pass"
    cam.write_text(json.dumps(document))
    with pytest.raises(ValueError, match="aggregate"):
        qualification_gate(root, plan, cam, engineering)


def test_report_input_digest_cannot_be_reused_after_mutation(tmp_path):
    root, plan, cam, engineering = setup_reports(tmp_path)
    document = json.loads(engineering.read_text())
    document["inputs"]["pcb_sha256"] = "0" * 64
    engineering.write_text(json.dumps(document))
    with pytest.raises(ValueError, match="digest"):
        qualification_gate(root, plan, cam, engineering)


def source():
    return {"url": "https://example.com/fixture", "revision": "fixture", "locator": "fixture", "sha256": "a" * 64}


def test_review_flag_alone_is_not_stackup_qualification():
    assert context_check("stackup", {"reviewed": True, "source": "guess"}, None)["status"] == "incomplete"
    stackup = {"reviewed": True, "source": source(), "selected_profile": "fixture", "finished_via_plating_nm": 20000,
               "physical_layers": [{"kind": "copper", "name": "F.Cu", "thickness_nm": 35000},
                                   {"kind": "dielectric", "thickness_nm": 1530000, "relative_permittivity": 4.1, "loss_tangent": 0.02},
                                   {"kind": "copper", "name": "B.Cu", "thickness_nm": 35000}]}
    native = {"layers": ["F.Cu", "B.Cu"], "board_thickness_nm": 1600000}
    assert context_check("stackup", stackup, native)["status"] == "pass"
    native["board_thickness_nm"] = 1200000
    assert context_check("stackup", stackup, native)["status"] == "fail"


def test_operating_envelope_requires_real_named_scenarios():
    envelope = {"reviewed": True, "source": source(), "scenarios": []}
    assert context_check("operating_envelope", envelope, None)["status"] == "incomplete"
    envelope["scenarios"] = [{"id": "fixture", "minimum_input_v": 3, "maximum_input_v": 4.2,
                               "peak_current_a": 1, "rms_current_a": 2, "ambient_c": 25}]
    assert context_check("operating_envelope", envelope, None)["status"] == "fail"

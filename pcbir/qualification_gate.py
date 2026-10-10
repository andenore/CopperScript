"""Revalidate current-input bindings before accepting engineering reports.

This checkpoint is not a production release API, purchase or BOM approval. It
requires all engineering gates while preserving independent assembly gates.
"""
from pathlib import Path

from .qualification import load_json, hash_file, digest_json, aggregate, check, report, SCHEMA
from .engineering_qualification import load_contract
from .stackup_qualification import engineering_input_files

CAM_REQUIRED = {"cam.checksums", "cam.native-binding", "cam.native-drc", "cam.layers", "cam.drill-parse",
                "cam.netlist-parse", "cam.drill-reconciliation", "cam.contact-reconciliation",
                "cam.via-reconciliation", "cam.net-partition", "cam.parser-corpus", "cam.parser-agreement",
                "cam.copper-connectivity", "cam.fabrication-geometry"}


def _document(path, kind, required):
    document = load_json(path)
    if document.get("schema") != SCHEMA or document.get("kind") != kind:
        raise ValueError("wrong qualification report schema/kind")
    checks = document.get("checks", [])
    ids = [item["id"] for item in checks]
    if len(set(ids)) != len(ids) or any(item.get("status") not in {"pass", "fail", "incomplete"} for item in checks):
        raise ValueError("invalid/duplicate qualification checks")
    if document.get("inputs_sha256") != digest_json(document["inputs"]):
        raise ValueError("qualification report input digest mismatch")
    if document.get("status") != aggregate(checks):
        raise ValueError("qualification report aggregate mismatch")
    missing = sorted(required - set(ids))
    findings = [check(f"{kind}.report", document["status"], "Reported gate results")]
    if missing:
        findings.append(check(f"{kind}.coverage", "incomplete", "Mandatory checks absent", missing=missing))
    return document, findings


def _algorithm_findings(kind, inputs):
    findings = []
    module = Path(__file__).parent
    for name, expected in inputs.get("algorithms", {}).items():
        if Path(name).name != name:
            raise ValueError("invalid algorithm binding")
        findings.append(check(f"{kind}.algorithm.{name}", "pass" if hash_file(module / name) == expected else "fail",
                              "Algorithm bytes must match the report"))
    if "native_probe_sha256" in inputs:
        for key, path in (("native_probe_sha256", module / "native_qualification_probe.py"),
                          ("native_overlay_sha256", module / "editor/native_overlay.py")):
            findings.append(check(f"{kind}.{key}", "pass" if hash_file(path) == inputs.get(key) else "fail", "Native reader binding"))
    return findings


def qualification_gate(directory: Path, plan_path: Path, cam_path: Path, engineering_path: Path) -> dict:
    directory = directory.resolve()
    plan = load_json(plan_path)
    required = {"engineering.stackup", "engineering.operating_envelope"}
    for entry in plan["contracts"]:
        required.add(f"{entry['scope']}.provenance")
        contract = load_contract(plan_path.parent / entry["path"])
        required.update(f"{entry['scope']}.{item['id']}" for item in contract["requirements"])
    cam, checks = _document(cam_path, "cam", CAM_REQUIRED)
    engineering, engineering_checks = _document(engineering_path, "rf-power", required)
    checks += engineering_checks
    paths = tuple(path for path in directory.rglob("*") if path.is_file())
    if any(path.is_symlink() for path in directory.rglob("*")):
        raise ValueError("symlinked manufacturing package")
    actual = {path.relative_to(directory).as_posix(): hash_file(path) for path in paths}
    checks.append(check("cam.artifact-binding", "pass" if actual == cam["inputs"].get("artifacts") else "fail",
                        "Final package bytes must match CAM evidence"))
    checks.append(check("cam.algorithm-binding", "pass" if hash_file(Path(__file__).with_name("cam_audit.py")) ==
                        cam["inputs"].get("algorithm_sha256") else "fail", "CAM runner binding"))
    checks += _algorithm_findings("cam", cam["inputs"])
    if cam.get("evidence_index_path"):
        checks.append(check("cam.evidence-index", "pass" if hash_file(Path(cam["evidence_index_path"])) ==
                            cam.get("evidence_index_sha256") else "fail", "External evidence selection index binding"))
    for identifier, binding in cam.get("evidence_artifacts", {}).items():
        matching = hash_file(Path(binding["document_path"])) == binding["document_sha256"] and \
            hash_file(Path(binding["report_path"])) == binding["report_sha256"]
        checks.append(check(f"cam.evidence.{identifier}", "pass" if matching else "fail", "External CAM evidence byte binding"))
    if any(item["id"] in {"cam.copper-connectivity", "cam.fabrication-geometry"} and item["status"] == "pass" and
           item["id"] not in cam.get("evidence_artifacts", {}) for item in cam["checks"]):
        checks.append(check("cam.external-binding", "fail", "Passed external CAM checks need retained report bytes"))
    pcbs = [path for path in paths if path.parent == directory and path.suffix == ".kicad_pcb"]
    inputs = engineering["inputs"]
    checks.append(check("engineering.native-binding", "pass" if len(pcbs) == 1 and hash_file(pcbs[0]) == inputs.get("pcb_sha256") else "fail",
                        "RF/power evidence must bind the SAME exported native board as CAM"))
    checks.append(check("engineering.plan-binding", "pass" if hash_file(plan_path) == inputs.get("plan_sha256") else "fail", "Operating/binding plan identity"))
    file_hashes = engineering_input_files(plan_path, plan)
    contract_hashes = {entry["scope"]: hash_file(plan_path.parent / entry["path"]) for entry in plan["contracts"]}
    checks.append(check("engineering.file-bindings", "pass" if file_hashes == inputs.get("files") else "fail", "Parts/source/selection inputs"))
    checks.append(check("engineering.contract-bindings", "pass" if contract_hashes == inputs.get("contracts") else "fail", "Reusable library requirements"))
    evidence_hashes = {}
    for identifier, relative in plan.get("evidence", {}).items():
        document = load_json(plan_path.parent / relative)
        evidence_hashes[identifier] = {"document_sha256": hash_file(plan_path.parent / relative),
                                      "report_sha256": hash_file(plan_path.parent / document["report_path"])}
    checks.append(check("engineering.evidence-bindings", "pass" if evidence_hashes == engineering.get("evidence_artifacts", {}) else "fail",
                        "Reviewed external evidence/report bytes must remain unchanged"))
    checks += _algorithm_findings("engineering", inputs)
    return report("engineering-checkpoint", {"cam_report_sha256": hash_file(cam_path),
                   "engineering_report_sha256": hash_file(engineering_path), "plan_sha256": hash_file(plan_path),
                   "gate_algorithm_sha256": hash_file(Path(__file__))}, checks)

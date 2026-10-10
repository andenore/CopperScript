"""Content-bound engineering evidence. A check pass is not production approval."""
from __future__ import annotations

from hashlib import sha256
import json
from pathlib import Path
import platform

SCHEMA = "copperscript-qualification/v0.1"
MAX_BYTES = 64 * 1024 * 1024


def load_json(path: Path) -> dict:
    if path.is_symlink() or path.stat().st_size > MAX_BYTES:
        raise ValueError(f"unsafe or oversized qualification input: {path}")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result
    value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique,
                       parse_constant=lambda value: (_ for _ in ()).throw(
                           ValueError(f"nonfinite JSON value: {value}")))
    if not isinstance(value, dict):
        raise ValueError("qualification document must be an object")
    return value


def digest_json(value) -> str:
    return sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                             allow_nan=False).encode()).hexdigest()


def hash_file(path: Path) -> str:
    if not path.is_file() or path.is_symlink() or path.stat().st_size > MAX_BYTES:
        raise ValueError(f"unsafe, missing or oversized input: {path}")
    return sha256(path.read_bytes()).hexdigest()


def check(identifier: str, status: str, detail: str, *, stage="design", **metrics) -> dict:
    if status not in {"pass", "fail", "incomplete"}:
        raise ValueError("invalid qualification status")
    return dict(id=identifier, status=status, stage=stage, detail=detail, metrics=metrics)


def aggregate(checks: list[dict]) -> str:
    if any(item["status"] == "fail" for item in checks):
        return "fail"
    if not checks or any(item["status"] != "pass" for item in checks):
        return "incomplete"
    return "pass"


def report(kind: str, inputs: dict, checks: list[dict], *, tools=()) -> dict:
    return {"schema": SCHEMA, "kind": kind, "status": aggregate(checks),
            "qualified_release": False,
            "qualification_boundary": "Engineering checks only; not BOM/order authorization or hardware certification",
            "inputs": inputs, "inputs_sha256": digest_json(inputs), "checks": checks,
            "tools": [{"name": "Python", "version": platform.python_version()}, *tools]}


def external_evidence(identifier: str, stage: str, inputs: dict,
                      evidence: dict | None) -> dict:
    """Validate a reviewed, externally supplied result against all current inputs.

    This verifies binding/completeness, not the truth or independence of a human
    attestation. Tool versions, model/settings and report bytes are mandatory.
    Never execute commands or load executables named by an evidence document.
    """
    if evidence is None:
        return check(identifier, "incomplete", "External evidence not supplied", stage=stage)
    required = {"schema", "id", "stage", "inputs_sha256", "status", "reviewed_by",
                "report_sha256", "tool", "settings_sha256", "model_sha256"}
    if not required <= evidence.keys() or evidence.get("schema") != "copperscript-engineering-evidence/v0.1":
        return check(identifier, "incomplete", "Incomplete engineering evidence contract", stage=stage)
    if evidence["id"] != identifier or evidence["stage"] != stage:
        return check(identifier, "fail", "Evidence scope or stage mismatch", stage=stage)
    if evidence["inputs_sha256"] != digest_json(inputs):
        return check(identifier, "fail", "Evidence is stale: inputs changed", stage=stage)
    for key in ("report_sha256", "settings_sha256", "model_sha256"):
        value = evidence[key]
        if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
            return check(identifier, "incomplete", f"Missing valid {key}", stage=stage)
    tool = evidence["tool"]
    if not isinstance(tool, dict) or not all(tool.get(key) for key in ("name", "version", "identity_sha256")):
        return check(identifier, "incomplete", "Unpinned evidence tool", stage=stage)
    identity = tool["identity_sha256"]
    if not isinstance(identity, str) or len(identity) != 64 or any(c not in "0123456789abcdef" for c in identity):
        return check(identifier, "incomplete", "Invalid evidence tool hash", stage=stage)
    if not isinstance(evidence["reviewed_by"], str) or not evidence["reviewed_by"].strip():
        return check(identifier, "incomplete", "External result needs accountable review", stage=stage)
    if evidence["status"] not in {"pass", "fail", "incomplete"}:
        return check(identifier, "incomplete", "Unknown external result status", stage=stage)
    return check(identifier, evidence["status"], "Reviewed external attestation (not independently verified)",
                 stage=stage, reviewer=evidence["reviewed_by"], report_sha256=evidence["report_sha256"])

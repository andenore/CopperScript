from __future__ import annotations

import csv
import json
import os
import subprocess
import sys
from pathlib import Path

from .cubemx import CubeMXError, ingest as cubemx_ingest, reconcile as cubemx_reconcile

ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "data" / "bundles" / "stm32g0b1"
OUTPUT = ROOT / "generated"
REQUESTS = ROOT / "data" / "requests.json"
EVIDENCE = ROOT / "data" / "evidence.jsonl"
SOURCES = ROOT / "data" / "sources.json"


def devicegen_env() -> dict[str, str]:
    env = os.environ.copy()
    source = env.get("COPPERSCRIPT_SOURCE")
    if source:
        env["PYTHONPATH"] = str(Path(source)) + os.pathsep + env.get("PYTHONPATH", "")
    return env


def devicegen(*args: str, capture: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "pcbir.devicegen", *args],
        cwd=ROOT,
        env=devicegen_env(),
        text=True,
        capture_output=capture,
        check=False,
    )


def _json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def facts() -> list[dict[str, object]]:
    return [
        json.loads(line)
        for line in EVIDENCE.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _rows(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def evidence_errors() -> list[str]:
    source_ids = {source["id"] for source in _json(SOURCES)["sources"]}
    evidence = facts()
    errors: list[str] = []
    seen: set[object] = set()
    by_field: dict[tuple[object, object], dict[str, object]] = {}
    for fact in evidence:
        for field in ("fact_id", "subject", "field", "value", "status", "source_id", "locator"):
            if field not in fact:
                errors.append(f"evidence fact missing {field}: {fact.get('fact_id', '<unknown>')}")
        fact_id = fact.get("fact_id")
        if fact_id in seen:
            errors.append(f"duplicate evidence fact_id: {fact_id}")
        seen.add(fact_id)
        if fact.get("source_id") not in source_ids:
            errors.append(f"unknown evidence source_id: {fact.get('source_id')}")
        if fact.get("status") not in {"verified", "inferred", "unresolved", "illustrative"}:
            errors.append(f"invalid evidence status: {fact.get('status')}")
        if (fact.get("status") == "unresolved") != (fact.get("value") == "?"):
            errors.append(f"unresolved evidence must use ?: {fact_id}")
        by_field[(fact.get("subject"), fact.get("field"))] = fact

    part_dir = BUNDLE / "parts" / "stm32g0b1cbt6"
    for pin in _rows(part_dir / "pins.csv"):
        for field, column in (("package_pin", "number"), ("bond", "bond")):
            fact = by_field.get((pin["name"], field))
            if fact is None or str(fact.get("value")) != pin[column]:
                errors.append(f"{pin['name']}.{field} does not match pins.csv")
    for pad in _rows(BUNDLE / "pads.csv"):
        for field in ("domains", "directions", "drive_modes"):
            fact = by_field.get((pad["name"], field))
            if fact is None or str(fact.get("value")) != pad[field]:
                errors.append(f"{pad['name']}.{field} does not match pads.csv")
    return errors


def validate(cubemx_root: str | Path | None = None, cubemx_identity: str = "STM32G0B1CBTx") -> None:
    errors = evidence_errors()
    if errors:
        raise RuntimeError("\n".join(errors))
    if cubemx_root:
        artifact = cubemx_ingest(cubemx_root, cubemx_identity)
        report = cubemx_reconcile(artifact, BUNDLE, EVIDENCE, [row["name"] for row in _rows(BUNDLE / "parts" / "stm32g0b1cbt6" / "pins.csv")])
        if report["errors"]:
            raise RuntimeError("\n".join(report["errors"]))
    result = devicegen("validate", str(BUNDLE), capture=True)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)


def generate(cubemx_root: str | Path | None = None, cubemx_identity: str = "STM32G0B1CBTx") -> None:
    validate(cubemx_root, cubemx_identity)
    result = devicegen("generate", str(BUNDLE), "--out-dir", str(OUTPUT), capture=True)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)


def check() -> None:
    result = devicegen("check", str(BUNDLE), "--out-dir", str(OUTPUT), capture=True)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)


def coverage() -> dict[str, object]:
    request = _json(REQUESTS)["requests"][0]
    evidence = facts()
    complete = bool(request["complete_coverage"])
    unresolved = sum(f["status"] == "unresolved" for f in evidence)
    illustrative = sum(f["status"] == "illustrative" for f in evidence)
    return {
        "orderable_part": request["orderable_part"],
        "package": request["package"],
        "verified_package_pins": sum(f["field"] == "package_pin" and f["status"] == "verified" for f in evidence),
        "covered_pads": len(_rows(BUNDLE / "pads.csv")),
        "peripheral_signals": len(_rows(BUNDLE / "peripherals.csv")),
        "mux_options": len(_rows(BUNDLE / "mux.csv")),
        "scope": request["scope"],
        "complete_coverage": complete,
        "production_publishable": complete and not unresolved and not illustrative and not evidence_errors(),
        "unresolved_facts": unresolved,
        "illustrative_facts": illustrative,
    }

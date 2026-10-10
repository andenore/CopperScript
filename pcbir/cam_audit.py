"""Audit the actual final-native manufacturing package, without rewriting it."""
from __future__ import annotations

from collections import Counter
from dataclasses import asdict
from pathlib import Path
import re
import subprocess

from .cam_qualification import (CamQualificationProfile, CamCorpusCase, ToolIdentity,
    PyGerberAdapter, GerbvSubprocessAdapter, run_cam_qualification_matrix,
    qualify_cam_artifacts, parse_xnc, parse_ipcd356)
from .qualification import check, hash_file, load_json, report, external_evidence, digest_json


def probe_native(pcb: Path, kicad_python: Path) -> dict:
    # The interpreter is an explicit CLI setting, never from an untrusted profile.
    probe = Path(__file__).with_name("native_qualification_probe.py")
    before = hash_file(pcb)
    try:
        result = subprocess.run((str(kicad_python), str(probe), str(pcb.resolve())),
                                capture_output=True, text=True, timeout=60, check=False)
    except subprocess.TimeoutExpired as exc:
        raise ValueError("native probe timed out; no evidence granted") from exc
    if result.returncode:
        raise ValueError(result.stderr.strip() or "native geometry probe failed")
    if hash_file(pcb) != before:
        raise ValueError("native PCB changed while extracting evidence")
    import json
    return json.loads(result.stdout)


def load_cam_profile(path: Path):
    data = load_json(path)
    if data.get("schema") != "copperscript-cam-profile/v0.1":
        raise ValueError("unknown CAM profile schema")
    profile = CamQualificationProfile(data["id"], data["gerber_spec_revision"], data["xnc_spec_revision"],
                                       tuple(ToolIdentity(**item) for item in data["tools"]))
    cases = []
    for item in data["corpus"]:
        entry = dict(item)
        if type(entry.get("expect_parse")) is not bool:
            raise ValueError("CAM corpus expect_parse must be boolean")
        expected = entry.pop("sha256")
        relative = Path(entry.pop("path"))
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("CAM corpus paths must be profile-local")
        source = path.parent / relative
        if hash_file(source) != expected:
            raise ValueError(f"CAM corpus hash mismatch: {source}")
        if "bounds_nm" in entry:
            entry["bounds_nm"] = tuple(entry["bounds_nm"])
        cases.append(CamCorpusCase(path=source, **entry))
    return profile, tuple(cases)


def available_adapters(*, gerbv: Path | None = None, gerbv_version: str | None = None):
    adapters = []
    try:
        adapters.append(PyGerberAdapter())
    except ImportError:
        pass
    if gerbv is not None:
        if not gerbv_version:
            raise ValueError("explicit pinned gerbv version is required")
        adapters.append(GerbvSubprocessAdapter(gerbv, gerbv_version))
    return tuple(adapters)


def load_cam_evidence_index(path: Path):
    index = load_json(path)
    if index.get("schema") != "copperscript-cam-evidence-index/v0.1":
        raise ValueError("unknown CAM evidence index")
    results, artifacts = {}, {}
    for identifier, relative in index["results"].items():
        if identifier not in {"cam.copper-connectivity", "cam.fabrication-geometry"}:
            raise ValueError("unsupported CAM external evidence scope")
        document_path = path.parent / relative
        evidence = load_json(document_path)
        artifact_path = path.parent / evidence["report_path"]
        if hash_file(artifact_path) != evidence["report_sha256"]:
            raise ValueError("external CAM report bytes mismatch")
        results[identifier] = evidence
        artifacts[identifier] = {"document_path": str(document_path.resolve()), "document_sha256": hash_file(document_path),
                                 "report_path": str(artifact_path.resolve()), "report_sha256": hash_file(artifact_path)}
    return {"results": results, "artifacts": artifacts, "index_sha256": hash_file(path), "index_path": str(path.resolve())}


def _match_multiset(expected, actual, *, tolerance_nm, coordinates, label):
    """Coordinate quantization tolerance only; preserve every duplicate contact."""
    remaining = list(actual)
    missing = []
    for wanted in expected:
        found = next((index for index, got in enumerate(remaining)
                      if wanted[:-coordinates] == got[:-coordinates] and
                      all(abs(a - b) <= tolerance_nm for a, b in zip(wanted[-coordinates:], got[-coordinates:]))), None)
        if found is None:
            missing.append(wanted)
        else:
            remaining.pop(found)
    return check(label, "fail" if missing or remaining else "pass", "Native/exported multiset reconciliation",
                 missing_count=len(missing), unexpected_count=len(remaining),
                 missing=missing[:20], unexpected=remaining[:20])


def reconcile_native_ipc(native, parsed):
    """Compare contact/via identities and the complete net equivalence relation.

    IPC signal names may be exporter aliases. Do not guess their truncation
    algorithm: recover correspondence from unique physical contacts and require
    a bijection of expected nets and exported aliases across pads AND vias.
    """
    relationships = []
    results = []
    origin = native.get("ipc_origin_nm", [0, 0])
    position = lambda item: [item["position"][0] - origin[0], item["position"][1] - origin[1]]
    for kind, expected, actual, tolerance_fields in (
        ("contact", [(p["net"], p["component"][:6], p["pad"][:4], *position(p)) for p in native["contacts"]],
         [(p.net, p.component, p.pad, p.x_nm, -p.y_nm) for p in parsed.points], 2),
        ("via", [(p["net"], p["drill_nm"], *position(p)) for p in native["vias"]],
         [(p.net, p.drill_nm, p.x_nm, -p.y_nm) for p in parsed.vias], 3),
    ):
        remaining, defects = list(actual), []
        for wanted in expected:
            matches = [index for index, got in enumerate(remaining)
                       if wanted[1:-tolerance_fields] == got[1:-tolerance_fields] and
                       all(abs(a - b) <= 1270 for a, b in zip(wanted[-tolerance_fields:], got[-tolerance_fields:]))]
            if not matches:
                defects.append(f"missing {wanted}")
                continue
            if len({remaining[index][0] for index in matches}) != 1:
                defects.append(f"ambiguous signal alias at {wanted}")
                continue
            got = remaining.pop(matches[0])
            relationships.append((wanted[0], got[0]))
        defects.extend(f"unexpected {item}" for item in remaining)
        results.append(check(f"cam.{kind}-reconciliation", "fail" if defects else "pass",
                             "Full physical contact multiset; coordinate quantization only", defects=defects[:20],
                             defect_count=len(defects), contact_count=len(expected)))
    expected_to_alias, alias_to_expected = {}, {}
    for expected, alias in relationships:
        expected_to_alias.setdefault(expected, set()).add(alias)
        alias_to_expected.setdefault(alias, set()).add(expected)
    splits = [key for key, values in expected_to_alias.items() if len(values) != 1]
    merges = [key for key, values in alias_to_expected.items() if len(values) != 1]
    wrong_nc = [(expected, alias) for expected, alias in relationships if (expected == "N/C") != (alias == "N/C")]
    results.append(check("cam.net-partition", "fail" if splits or merges or wrong_nc else "pass",
                         "Pad/via net partition preserves exporter aliases; NOT copper extraction proof",
                         split_source_nets=splits, merged_aliases=merges, wrong_unconnected=wrong_nc,
                         net_count=len(expected_to_alias)))
    return results


def audit_cam(directory: Path, *, profile=None, cases=(), adapters=(), native=None,
              external: dict | None = None) -> dict:
    directory = directory.resolve()
    checks = []
    inputs = {"algorithm_sha256": hash_file(Path(__file__)), "artifacts": {}}
    inputs["algorithms"] = {name: hash_file(Path(__file__).with_name(name)) for name in
                            ("qualification.py", "cam_qualification.py", "cam_geometry.py")}
    children = sorted(directory.rglob("*"))
    if any(path.is_symlink() for path in children):
        return report("cam", inputs, [check("cam.files", "fail", "Package contains symlinks")])
    files = [path for path in children if path.is_file()]
    names = [path.relative_to(directory).as_posix() for path in files]
    if len(set(name.casefold() for name in names)) != len(names):
        return report("cam", inputs, [check("cam.files", "fail", "Case-colliding paths")])
    inputs["artifacts"] = {name: hash_file(path) for name, path in zip(names, files)}
    declared = {}
    for line in (directory / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match or match[2] in declared:
            raise ValueError("malformed/duplicate package checksum")
        declared[match[2]] = match[1]
    differences = [name for name, digest in declared.items() if inputs["artifacts"].get(name) != digest]
    differences += sorted(set(names) - set(declared) - {"SHA256SUMS", "manufacturing-package.zip"})
    checks.append(check("cam.checksums", "fail" if differences else "pass", "Package checksum coverage", mismatches=differences))
    manifest = load_json(directory / "manifest.json")
    if manifest.get("schema") != "copperscript-manufacturing-files/v0.1":
        raise ValueError("expected final-native manufacturing package")
    pcbs = [path for path in files if path.parent == directory and path.suffix == ".kicad_pcb"]
    checks.append(check("cam.native-binding", "pass" if len(pcbs) == 1 and
                        hash_file(pcbs[0]) == manifest.get("exported_pcb_sha256") else "fail",
                        "Exported native board must match package manifest"))
    drc = load_json(directory / "drc.json")
    clean = all(key in drc and isinstance(drc[key], list) and not drc[key] for key in ("violations", "unconnected_items"))
    checks.append(check("cam.native-drc", "pass" if clean else "fail", "Saved native DRC report; not independent CAM"))
    functions = []
    gerbers = [path for path in files if path.suffix.lower() == ".gbr"]
    malformed = []
    for path in gerbers:
        text = path.read_text(encoding="ascii")
        matches = re.findall(r"%TF\.FileFunction,([^*]+)\*%", text)
        if len(matches) != 1 or "%MOMM*%" not in text or not text.rstrip().endswith("M02*"):
            malformed.append(path.name)
        functions.extend(matches)
    count = len(manifest["copper_layers"])
    expected = [f"Copper,L{i},{'Top' if i == 1 else 'Bot' if i == count else 'Inr'}"
                for i in range(1, count + 1)]
    expected += ["Soldermask,Top", "Soldermask,Bot", "Paste,Top", "Paste,Bot",
                 "Legend,Top", "Legend,Bot", "Profile,NP"]
    checks.append(check("cam.layers", "pass" if not malformed and Counter(functions) == Counter(expected) else "fail",
                        "Exact X2 layer inventory (structural, not geometry)", functions=sorted(functions), malformed=malformed))
    programs, drill_errors = [], []
    for path in files:
        if path.suffix.lower() != ".drl":
            continue
        text = path.read_text(encoding="ascii")
        metadata = re.findall(r"TF\.FileFunction,(Plated|NonPlated),(\d+),(\d+),", text)
        if len(metadata) != 1 or metadata[0][1:] != ("1", str(count)):
            drill_errors.append(f"unsupported/incorrect drill span: {path.name}")
            continue
        try:
            programs.append(parse_xnc(path, plated=metadata[0][0] == "Plated"))
        except ValueError as exc:
            drill_errors.append(str(exc))
    checks.append(check("cam.drill-parse", "fail" if drill_errors or not programs else "pass", "Metric through-drill subset",
                        errors=drill_errors, hits=sum(len(p.hits) for p in programs), slots=sum(len(p.slots) for p in programs)))
    netlists = [path for path in files if path.suffix.lower() == ".d356"]
    parsed = None
    try:
        if len(netlists) != 1:
            raise ValueError("exactly one manufacturing netlist required")
        parsed = parse_ipcd356(netlists[0])
        if not parsed.points:
            raise ValueError("manufacturing netlist contains no contacts")
        checks.append(check("cam.netlist-parse", "pass", "IPC-D-356 supported contact records", contacts=len(parsed.points), vias=len(parsed.vias)))
    except ValueError as exc:
        checks.append(check("cam.netlist-parse", "fail", str(exc)))
    if native is None:
        checks.append(check("cam.native-reconciliation", "incomplete", "Explicit KiCad Python probe needed"))
    elif native.get("unsupported") or any(program.slots for program in programs):
        checks.append(check("cam.native-reconciliation", "incomplete", "Native reconciliation subset does not support these objects",
                            unsupported=native.get("unsupported", [])))
    else:
        wanted = [(d["plated"], d["diameter_nm"], *d["position"]) for d in native["drills"]]
        got = [(p.plated, h.diameter_nm, h.x_nm, -h.y_nm) for p in programs for h in p.hits]
        checks.append(_match_multiset(wanted, got, tolerance_nm=1000, coordinates=2, label="cam.drill-reconciliation"))
        if parsed is not None:
            checks.extend(reconcile_native_ipc(native, parsed))
    tools = [asdict(adapter.identity) for adapter in adapters]
    inputs["tools"] = tools
    if native is not None:
        inputs["native_probe_sha256"] = hash_file(Path(__file__).with_name("native_qualification_probe.py"))
        inputs["native_geometry_sha256"] = digest_json(native)
        inputs["native_overlay_sha256"] = hash_file(Path(__file__).parent / "editor/native_overlay.py")
    if profile is None:
        checks.append(check("cam.independent-parsers", "incomplete", "Pinned two-parser profile and positive/negative corpus required"))
    else:
        matrix = run_cam_qualification_matrix(profile, tuple(cases), tuple(adapters))
        qualification = qualify_cam_artifacts(directory, profile, tuple(adapters))
        inputs["profile"] = asdict(profile)
        inputs["corpus"] = matrix.corpus_hashes
        checks.append(check("cam.parser-corpus", matrix.status.value, "Pinned independent parser corpus", findings=matrix.findings))
        checks.append(check("cam.parser-agreement", qualification.status.value,
                            "Parse/render agreement only; NOT electrical copper proof", findings=qualification.findings))
    for identifier in ("cam.copper-connectivity", "cam.fabrication-geometry"):
        checks.append(external_evidence(identifier, "design", inputs, (external or {}).get("results", external or {}).get(identifier)))
    result = report("cam", inputs, checks, tools=tools)
    result["evidence_artifacts"] = (external or {}).get("artifacts", {})
    result["evidence_index_sha256"] = (external or {}).get("index_sha256")
    result["evidence_index_path"] = (external or {}).get("index_path")
    return result

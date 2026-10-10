"""Contract-driven RF/power/interface assessment; reusable scoped requirements.

Companion JSON contracts are deliberately not new Copper language syntax.
External simulation/bench results remain reviewed attestations, not certification.
"""
from __future__ import annotations

from pathlib import Path
from math import hypot

from .power_integrity import CALCULATIONS, number
from .qualification import check, report, hash_file, load_json, external_evidence, digest_json
from .rf_geometry import reference_coverage
from .pair_geometry import differential_pair_inventory
from .route_path_geometry import routed_path_inventory
from .stackup_qualification import (engineering_input_files, resolve_stackup_context,
                                   supplier_stackup_check, outer_microstrip_inventory)

REQUIRED = {
    "rf": {"matching-review", "impedance", "prototype-rf"},
    "power": {"component-ratings", "current-capacity", "layout-review", "transient", "prototype-power"},
    "interface": {"operating-mode", "layout-review", "impedance", "signal-integrity", "prototype-interface"},
}
ALGORITHMS = {"external", "reference_coverage", "calculation", "range", "route_width", "route_length", "placement_distance"}


def context_check(key, context, native):
    identifier = f"engineering.{key}"
    if key == "stackup" and isinstance(context, dict) and context.get("schema") == "copperscript-fabrication-stackup/v0.1":
        return supplier_stackup_check(context, native)
    if not isinstance(context, dict) or context.get("reviewed") is not True or _missing(context):
        return check(identifier, "incomplete", "Reviewed board-specific input required")
    source = context.get("source")
    if not isinstance(source, dict) or not all(source.get(field) for field in ("url", "revision", "locator", "sha256")):
        return check(identifier, "incomplete", "Input provenance needs URL/revision/locator/hash")
    if not isinstance(source["sha256"], str) or len(source["sha256"]) != 64 or any(c not in "0123456789abcdef" for c in source["sha256"]):
        return check(identifier, "fail", "Invalid input provenance hash")
    try:
        if key == "stackup":
            if not context.get("selected_profile") or not context.get("physical_layers"):
                return check(identifier, "incomplete", "Select exact fabrication profile and physical stackup")
            physical = context["physical_layers"]
            copper = []
            total = 0
            previous = None
            for layer in physical:
                if layer["kind"] not in {"copper", "dielectric"} or layer["kind"] == previous:
                    raise ValueError("stackup must alternate copper/dielectric layers")
                previous = layer["kind"]
                thickness = number(layer["thickness_nm"], "layer thickness")
                total += thickness
                if layer["kind"] == "copper":
                    copper.append(layer["name"])
                else:
                    number(layer["relative_permittivity"], "relative_permittivity", minimum=1)
                    if number(layer["loss_tangent"], "loss_tangent", allow_zero=True) >= 1:
                        raise ValueError("loss tangent must be below one")
            number(context["finished_via_plating_nm"], "finished via plating")
            if physical[0]["kind"] != "copper" or physical[-1]["kind"] != "copper" or len(set(copper)) != len(copper):
                raise ValueError("invalid copper stackup endpoints/order")
            if native is None:
                return check(identifier, "incomplete", "Native copper order/thickness comparison unavailable")
            if copper != native["layers"] or abs(total - native["board_thickness_nm"]) > 1:
                raise ValueError("supplier stackup differs from actual native layer order/thickness")
        else:
            scenarios = context.get("scenarios")
            if not isinstance(scenarios, list) or not scenarios:
                return check(identifier, "incomplete", "Define explicit source/load/temperature scenarios")
            ids = [scenario["id"] for scenario in scenarios]
            if len(set(ids)) != len(ids) or not all(isinstance(value, str) and value for value in ids):
                raise ValueError("invalid operating scenario IDs")
            for scenario in scenarios:
                low = number(scenario["minimum_input_v"], "minimum_input_v")
                high = number(scenario["maximum_input_v"], "maximum_input_v")
                peak = number(scenario["peak_current_a"], "peak_current_a", allow_zero=True)
                rms = number(scenario["rms_current_a"], "rms_current_a", allow_zero=True)
                number(scenario["ambient_c"], "ambient_c", minimum=-273.15, allow_zero=True)
                if low > high or rms > peak:
                    raise ValueError("inverted input range or RMS greater than peak")
    except (KeyError, TypeError, ValueError) as exc:
        return check(identifier, "fail", f"Invalid board-specific context: {exc}")
    return check(identifier, "pass", "Reviewed, structured board-specific input; external qualification still required")


def load_contract(path: Path) -> dict:
    contract = load_json(path)
    if contract.get("schema") != "copperscript-engineering-contract/v0.1" or contract.get("kind") not in REQUIRED:
        raise ValueError(f"unknown engineering contract: {path}")
    if not isinstance(contract.get("id"), str) or not contract["id"]:
        raise ValueError("contract needs an ID")
    requirements = contract.get("requirements", [])
    ids = [item["id"] for item in requirements]
    if len(set(ids)) != len(ids) or not REQUIRED[contract["kind"]] <= set(ids):
        raise ValueError("contract omits mandatory requirements or duplicates IDs")
    if any(item.get("algorithm") not in ALGORITHMS or item.get("stage") not in {"design", "simulation", "bench"}
           for item in requirements):
        raise ValueError("unsupported requirement algorithm/stage")
    if contract["kind"] == "rf" and contract.get("matching") not in {"required", "integrated", "unresolved"}:
        raise ValueError("RF contract must explicitly classify matching")
    if contract["kind"] == "interface" and (not isinstance(contract.get("protocol"), str) or not contract["protocol"].strip()):
        raise ValueError("interface contract must identify its protocol")
    for item in requirements:
        if not isinstance(item.get("description"), str) or not item["description"].strip():
            raise ValueError("requirement must explain its qualification boundary")
        # These cannot be satisfied by one simple geometric or arithmetic check.
        if item["id"] in REQUIRED[contract["kind"]] and item["algorithm"] != "external":
            raise ValueError("mandatory engineering qualification requires reviewed external evidence")
        expected_stage = "bench" if item["id"].startswith("prototype-") else "simulation" if item["id"] in {"impedance", "transient", "signal-integrity"} else "design"
        if item["id"] in REQUIRED[contract["kind"]] and item["stage"] != expected_stage:
            raise ValueError("mandatory qualification evidence has wrong stage")
    return contract


def _missing(value):
    return value is None or isinstance(value, dict) and any(_missing(item) for item in value.values()) or \
        isinstance(value, list) and any(_missing(item) for item in value)


def _range(identifier, values, limits):
    if not isinstance(limits, dict) or not limits:
        raise ValueError("numerical checks require explicit limits")
    violations = []
    for metric, bounds in limits.items():
        if metric not in values or not isinstance(bounds, dict) or not bounds or set(bounds) - {"minimum", "maximum"}:
            raise ValueError("invalid metric/bound")
        value = values[metric]
        number(value, metric, minimum=-1e300, allow_zero=True)
        for direction, bound in bounds.items():
            number(bound, f"{metric}.{direction}", minimum=-1e300, allow_zero=True)
            if direction == "minimum" and value < bound or direction == "maximum" and value > bound:
                violations.append(f"{metric} {direction} {bound}")
        if "minimum" in bounds and "maximum" in bounds and bounds["minimum"] > bounds["maximum"]:
            raise ValueError("inverted numerical bounds")
    return check(identifier, "fail" if violations else "pass", "Explicit-input numerical screening",
                 values=values, violations=violations)


def evaluate_requirement(identifier, requirement, binding, native, inputs, evidence):
    algorithm, stage = requirement["algorithm"], requirement["stage"]
    if algorithm == "external":
        return external_evidence(identifier, stage, inputs, evidence)
    if not isinstance(binding, dict) or _missing(binding):
        return check(identifier, "incomplete", "Numerical/geometry bindings unresolved", stage=stage)
    if not binding.get("basis"):
        return check(identifier, "incomplete", "A source locator or engineering derivation is required", stage=stage)
    if algorithm == "reference_coverage":
        if native is None:
            return check(identifier, "incomplete", "Native filled-copper probe unavailable")
        return reference_coverage(native, identifier=identifier, **{key: binding[key] for key in
                                  ("net", "reference_net", "reference_layer", "margin_nm")})
    if algorithm == "calculation":
        calculation = binding["calculation"]
        if calculation not in CALCULATIONS:
            raise ValueError("unknown power calculation")
        values = CALCULATIONS[calculation](**binding["parameters"])
        if not isinstance(values, dict):
            values = {"resistance_ohms": values}
        return _range(identifier, values, binding["limits"])
    if algorithm == "range":
        return _range(identifier, binding["values"], binding["limits"])
    if native is None:
        return check(identifier, "incomplete", "Native geometry unavailable")
    if algorithm == "placement_distance":
        # Deliberately component-origin distance, never a pin-relative substitute.
        poses = {pose["reference"]: pose["position"] for pose in native["poses"]}
        if any(reference not in poses for reference in binding["references"]) or len(binding["references"]) != 2:
            raise ValueError("placement check requires exactly two existing references")
        a, b = (poses[reference] for reference in binding["references"])
        return _range(identifier, {"origin_distance_nm": hypot(a[0] - b[0], a[1] - b[1])}, binding["limits"])
    tracks = [track for track in native["tracks"] if track["net"] == binding["net"]]
    if not tracks:
        return check(identifier, "incomplete", "Net has no supported routed track segments")
    if algorithm == "route_width":
        # Check every segment, including pad neckdowns; no default exceptions.
        return _range(identifier, {"minimum_track_width_nm": min(track["width_nm"] for track in tracks)}, binding["limits"])
    if algorithm == "route_length":
        # Sum of all net segments is an upper bound, NOT a pin-to-pin path solver.
        return _range(identifier, {"total_track_length_nm": sum(hypot(t["start"][0] - t["end"][0],
                    t["start"][1] - t["end"][1]) for t in tracks)}, binding["limits"])
    raise ValueError("unsupported engineering algorithm")


def assess_engineering(pcb: Path, plan_path: Path, *, native=None) -> dict:
    plan = load_json(plan_path)
    if plan.get("schema") != "copperscript-engineering-plan/v0.1":
        raise ValueError("unknown engineering plan schema")
    entries = plan.get("contracts", [])
    if not entries or len({entry["scope"] for entry in entries}) != len(entries):
        raise ValueError("plan requires unique, nonempty contract scopes")
    inputs = {"pcb_sha256": hash_file(pcb), "plan_sha256": hash_file(plan_path),
              "algorithms": {name: hash_file(Path(__file__).with_name(name)) for name in
                             ("engineering_qualification.py", "qualification.py", "rf_geometry.py", "power_integrity.py", "engineering.py", "stackup_qualification.py", "pair_geometry.py", "route_path_geometry.py")},
              "context": resolve_stackup_context(plan_path, plan),
              "files": engineering_input_files(plan_path, plan), "contracts": {}}
    contracts = []
    for entry in entries:
        path = plan_path.parent / entry["path"]
        contract = load_contract(path)
        inputs["contracts"][entry["scope"]] = hash_file(path)
        contracts.append((entry, contract))
    checks = []
    for key in ("stackup", "operating_envelope"):
        checks.append(context_check(key, inputs["context"].get(key), native))
    tools = []
    if native is not None:
        inputs["native_geometry_sha256"] = digest_json(native)
        inputs["native_probe_sha256"] = hash_file(Path(__file__).with_name("native_qualification_probe.py"))
        inputs["native_overlay_sha256"] = hash_file(Path(__file__).parent / "editor/native_overlay.py")
        tools = [{"name": "KiCad", "version": native["kicad_version"]}]
        try:
            import shapely
            tools.append({"name": "Shapely", "version": shapely.__version__, "GEOS": shapely.geos_version_string})
        except ImportError:
            checks.append(check("engineering.geometry-tool", "incomplete", "Optional vector geometry dependency unavailable"))
        inputs["geometry_tools"] = tools
    evidence_artifacts = {}
    for entry, contract in contracts:
        sources = contract.get("sources", [])
        source_complete = bool(sources) and all(isinstance(source, dict) and all(source.get(key)
                                   for key in ("url", "revision", "locator", "sha256"))
                                   and str(source["url"]).startswith("https://")
                                   and isinstance(source["sha256"], str) and len(source["sha256"]) == 64
                                   and all(c in "0123456789abcdef" for c in source["sha256"]) for source in sources)
        checks.append(check(f"{entry['scope']}.provenance", "pass" if source_complete else "incomplete",
                            "Exact reference revision/locator/content hash required"))
        bindings = entry.get("bindings", {})
        for requirement in contract["requirements"]:
            identifier = f"{entry['scope']}.{requirement['id']}"
            evidence_path = plan.get("evidence", {}).get(identifier)
            evidence = load_json(plan_path.parent / evidence_path) if evidence_path else None
            if evidence is not None:
                # Evidence body and separately retained report bytes must agree.
                artifact_path = evidence.get("report_path")
                if not artifact_path or hash_file(plan_path.parent / artifact_path) != evidence.get("report_sha256"):
                    checks.append(check(identifier, "fail", "External report bytes missing or mismatched", stage=requirement["stage"]))
                    continue
                evidence_artifacts[identifier] = {"document_sha256": hash_file(plan_path.parent / evidence_path),
                                                  "report_sha256": hash_file(plan_path.parent / artifact_path)}
            try:
                checks.append(evaluate_requirement(identifier, requirement, bindings.get(requirement["id"]), native, inputs, evidence))
            except (KeyError, TypeError, ValueError) as exc:
                checks.append(check(identifier, "fail", f"Invalid requirement binding: {exc}", stage=requirement["stage"]))
    screens = plan.get("microstrip_screens", [])
    pair_screens = plan.get("differential_pair_screens", [])
    path_screens = plan.get("route_path_screens", [])
    screening = []
    screen_ids = [item["id"] for item in screens + pair_screens + path_screens]
    if len(set(screen_ids)) != len(screen_ids) or set(screen_ids) & {item["id"] for item in checks}:
        raise ValueError("duplicate auxiliary screen IDs")
    for screen in screens:
        try:
            screening.append(outer_microstrip_inventory(native, inputs["context"].get("stackup", {}),
                          identifier=screen["id"], net=screen["net"], reference_layer=screen["reference_layer"], reference_net=screen["reference_net"]))
        except (KeyError, TypeError, ValueError) as exc:
            screening.append(check(screen["id"], "fail", f"Invalid microstrip screen: {exc}"))
    for screen in pair_screens:
        try:
            screening.append(differential_pair_inventory(native, inputs["context"].get("stackup", {}),
                **{key: screen[key] for key in ("positive_net", "negative_net", "maximum_search_gap_nm")},
                identifier=screen["id"]))
        except (KeyError, TypeError, ValueError) as exc:
            screening.append(check(screen["id"], "fail", f"Invalid pair screen: {exc}"))
    for screen in path_screens:
        try:
            screening.append(routed_path_inventory(native, inputs["context"].get("stackup", {}),
                identifier=screen["id"], **{key: screen[key] for key in ("net", "terminals", "return_net")}))
        except (KeyError, TypeError, ValueError) as exc:
            screening.append(check(screen["id"], "fail", f"Invalid path screen: {exc}"))
    result = report("rf-power", inputs, checks, tools=tools)
    # Auxiliary screening cannot approve or replace mandatory solver evidence.
    result["screening"] = screening
    result["evidence_artifacts"] = evidence_artifacts
    return result

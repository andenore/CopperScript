"""Checked local SPICE model bundles, explicit terminal mapping, and provenance."""
from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from .model import SimulationError, object_fields, read_json, required, strings, text


@dataclass(frozen=True, slots=True)
class ModelBundle:
    model_id: str
    kind: str
    name: str
    terminals: tuple[str, ...]
    pin_map: Mapping[str, str]
    ignored_pins: tuple[str, ...]
    supported_parts: tuple[str, ...]
    analyses: tuple[str, ...]
    versions: tuple[int, ...]
    entrypoint: str
    assets: tuple[tuple[str, str], ...]
    manifest: str

    def __post_init__(self):
        object.__setattr__(self, "pin_map", MappingProxyType(dict(self.pin_map)))


def load_registry(path: Path | None) -> dict[str, ModelBundle]:
    if path is None:
        return {}
    raw = object_fields(read_json(path), {"schema_version", "models"}, "model registry")
    if type(raw.get("schema_version")) is not int or raw["schema_version"] != 1:
        raise SimulationError("model registry requires schema_version: 1")
    entries = required(raw, "models", "model registry")
    if not isinstance(entries, dict):
        raise SimulationError("registry models must be an object")
    bundles = {}
    for model_id, value in sorted(entries.items()):
        text(model_id, "model ID")
        d = object_fields(value, {"kind", "name", "terminals", "pin_map", "ignored_pins", "supported_parts", "analyses",
                                  "ngspice_versions", "entrypoint", "files", "source_url", "license", "assumptions", "dialect"}, model_id)
        kind = required(d, "kind", model_id)
        if kind not in {"subcircuit", "diode", "bjt", "mosfet"}:
            raise SimulationError(f"model {model_id}: unsupported model kind")
        name = required(d, "name", model_id)
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            raise SimulationError(f"model {model_id}: invalid SPICE model name")
        if d.get("dialect", "native") != "native":
            raise SimulationError(f"model {model_id}: only qualified native ngspice models are supported")
        terminals = strings(required(d, "terminals", model_id), model_id + ".terminals")
        if not terminals or (kind != "subcircuit" and len(terminals) != {"diode": 2, "bjt": 3, "mosfet": 4}[kind]):
            raise SimulationError(f"model {model_id}: incorrect terminal count")
        standard_order = {"diode": ("A", "K"), "bjt": ("C", "B", "E"), "mosfet": ("D", "G", "S", "B")}.get(kind)
        if standard_order and tuple(t.upper() for t in terminals) != standard_order:
            raise SimulationError(f"model {model_id}: {kind} terminals must follow native card order {', '.join(standard_order)}")
        pin_map = required(d, "pin_map", model_id)
        if not isinstance(pin_map, dict) or set(pin_map) != set(terminals) or any(not isinstance(v, str) for v in pin_map.values()):
            raise SimulationError(f"model {model_id}: pin_map must bind every model terminal")
        analyses = strings(required(d, "analyses", model_id), model_id + ".analyses")
        if not analyses or set(analyses) - {"op", "ac", "transient"}:
            raise SimulationError(f"model {model_id}: invalid analysis capabilities")
        versions = required(d, "ngspice_versions", model_id)
        if not isinstance(versions, list) or not versions or any(type(v) is not int or v < 1 for v in versions):
            raise SimulationError(f"model {model_id}: ngspice_versions must list qualified major versions")
        parts = strings(required(d, "supported_parts", model_id), model_id + ".supported_parts")
        if not parts:
            raise SimulationError(f"model {model_id}: supported_parts must be explicit")
        text(required(d, "license", model_id), "model license")
        strings(d.get("assumptions", []), "model assumptions")
        files = required(d, "files", model_id)
        if not isinstance(files, list) or not files:
            raise SimulationError(f"model {model_id}: expected a nonempty files list")
        content_by_path = {}
        names = {}
        file_manifest = []
        for file in files:
            f = object_fields(file, {"path", "sha256"}, "model file")
            relative = text(required(f, "path", model_id), "model file path")
            full = (path.parent / relative).resolve()
            if not full.is_relative_to(path.parent.resolve()) or full in content_by_path:
                raise SimulationError(f"model {model_id}: file escapes registry directory or is repeated")
            expected = required(f, "sha256", model_id)
            if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
                raise SimulationError("model files require lowercase SHA-256 hashes")
            try:
                original = full.read_bytes()
                content = original.decode("utf-8-sig")
            except (OSError, UnicodeError) as exc:
                raise SimulationError(f"cannot read model file {relative}: {exc}") from exc
            if hashlib.sha256(original).hexdigest() != expected:
                raise SimulationError(f"model {model_id}: SHA-256 mismatch for {relative}")
            content_by_path[full] = content
            # Keep each source path distinct: identical bytes may resolve relative includes differently.
            location_hash = hashlib.sha256(relative.encode()).hexdigest()[:12]
            names[full] = f"models/{expected}-{location_hash}.lib"
            file_manifest.append({"path": relative, "sha256": expected, "artifact": names[full]})
        entry = (path.parent / text(required(d, "entrypoint", model_id), "entrypoint")).resolve()
        if entry not in names:
            raise SimulationError(f"model {model_id}: entrypoint is not a checked file")
        rewritten = {}
        edges = {}
        for full, content in content_by_path.items():
            lines = []
            edges[full] = []
            for line in content.splitlines():
                stripped = line.strip()
                if not stripped.startswith("*") and re.search(r"\b(?:file|filename)\s*=|\bpwlfile\s*\(", stripped, re.I):
                    raise SimulationError(f"model {model_id}: external waveform data files are unsupported; all model inputs must be checked assets")
                if stripped.startswith("."):
                    command = stripped.split()[0].lower()
                    if command not in {".model", ".subckt", ".ends", ".param", ".func", ".include", ".inc"}:
                        raise SimulationError(f"model {model_id}: unsupported model directive {command}; control scripts are not model data")
                    if command in {".include", ".inc"}:
                        match = re.fullmatch(r"\.(?:include|inc)\s+(?:\"([^\"]+)\"|'([^']+)'|([^\s]+))\s*", stripped, re.I)
                        if not match:
                            raise SimulationError(f"model {model_id}: invalid include")
                        include = (full.parent / next(v for v in match.groups() if v is not None)).resolve()
                        if include not in names:
                            raise SimulationError(f"model {model_id}: include is not in the checked file bundle")
                        edges[full].append(include)
                        line = f'.include "{names[include]}"'
                lines.append(line)
            rewritten[names[full]] = "\n".join(lines) + "\n"
        visited = set()
        def visit(node, stack=()):
            if node in stack:
                raise SimulationError(f"model {model_id}: cyclic include")
            if node not in visited:
                for child in edges[node]:
                    visit(child, (*stack, node))
                visited.add(node)
        visit(entry)
        declarations = "\n".join(content_by_path[p] for p in visited)
        if kind == "subcircuit":
            declaration = re.search(rf"(?im)^\s*\.subckt\s+{re.escape(name)}\s+([^\r\n]+)", declarations)
            if not declaration:
                raise SimulationError(f"model {model_id}: subcircuit declaration is missing")
            actual = re.split(r"\s+(?:params:|\w+=)", declaration[1], maxsplit=1, flags=re.I)[0].split()
            if tuple(t.lower() for t in actual) != tuple(t.lower() for t in terminals):
                raise SimulationError(f"model {model_id}: terminal order differs from .subckt declaration")
        else:
            declaration = re.search(rf"(?im)^\s*\.model\s+{re.escape(name)}\s+([A-Za-z]+)", declarations)
            allowed = {"diode": {"d"}, "bjt": {"npn", "pnp"}, "mosfet": {"nmos", "pmos"}}[kind]
            if not declaration or declaration[1].lower() not in allowed:
                raise SimulationError(f"model {model_id}: missing or incompatible .model declaration")
        import json
        manifest = json.dumps({"id": model_id, **d, "files": file_manifest, "registry_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}, sort_keys=True)
        bundles[model_id] = ModelBundle(model_id, kind, name, terminals, pin_map,
            strings(d.get("ignored_pins", []), "ignored_pins"), parts, analyses, tuple(versions), names[entry],
            tuple(sorted((names[p], rewritten[names[p]]) for p in visited)), manifest)
    return bundles

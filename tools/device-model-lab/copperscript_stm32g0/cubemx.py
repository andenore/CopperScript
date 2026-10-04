"""Small, namespace-agnostic STM32CubeMX XML ingestion and reconciliation layer."""

from __future__ import annotations

import hashlib
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Iterable


class CubeMXError(ValueError):
    pass


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1].casefold()


def _attr(element: ET.Element, *names: str) -> str:
    wanted = {name.casefold() for name in names}
    for key, value in element.attrib.items():
        if key.rsplit("}", 1)[-1].casefold() in wanted and value:
            return value.strip()
    return ""


def _split_modes(value: str) -> list[str]:
    return [item for item in re.split(r"[|;,\s]+", value) if item]


def _mcu_dir(root: str | Path) -> Path:
    candidate = Path(root).resolve()
    if candidate.name.casefold() == "mcu":
        return candidate
    nested = candidate / "db" / "mcu"
    if nested.is_dir():
        return nested
    raise CubeMXError(f"CubeMX db/mcu directory not found below {candidate}")


def _read_xml(path: Path) -> ET.Element:
    try:
        return ET.parse(path).getroot()
    except (OSError, ET.ParseError) as exc:
        raise CubeMXError(f"cannot parse XML {path}: {exc}") from exc


def _find_xml(directory: Path, identity: str) -> Path:
    exact = {path.name.casefold(): path for path in directory.glob("*.xml")}
    for name in (f"{identity}.xml", f"{identity.lower()}.xml"):
        if name.casefold() in exact:
            return exact[name.casefold()]
    matches = sorted(path for path in directory.glob("*.xml") if identity.casefold() in path.stem.casefold())
    if matches:
        return matches[0]
    raise CubeMXError(f"CubeMX MCU XML not found for {identity} in {directory}")


def _source_ref(value: str, base: Path, mcu_dir: Path) -> Path | None:
    if not value.casefold().endswith(".xml"):
        return None
    normalized = value.replace("\\", "/").lstrip("/")
    for candidate in (base / normalized, mcu_dir / normalized, mcu_dir / "IP" / Path(normalized).name):
        if candidate.is_file():
            resolved = candidate.resolve()
            if not resolved.is_relative_to(mcu_dir.resolve()):
                raise CubeMXError(f"XML reference escapes CubeMX db/mcu: {value!r}")
            return resolved
    return None


def _ip_xml(element: ET.Element, mcu_dir: Path) -> list[Path]:
    if _local(element.tag) != "ip":
        return []
    name = _attr(element, "name")
    version = _attr(element, "version")
    config = _attr(element, "configFile")
    stems = [stem for stem in (config, f"{name}-{version}" if name and version else "") if stem]
    ip_dir = mcu_dir / "IP"
    matches: set[Path] = set()
    for stem in stems:
        basename = Path(stem.replace("\\", "/")).name
        for path in ip_dir.glob(f"{basename}*.xml"):
            matches.add(path.resolve())
    return sorted(matches)


def _referenced_xml(root: ET.Element, base: Path, mcu_dir: Path) -> list[Path]:
    result: set[Path] = set()
    for element in root.iter():
        result.update(_ip_xml(element, mcu_dir))
        for key, value in element.attrib.items():
            local_key = key.rsplit("}", 1)[-1].casefold()
            if local_key in {"ref", "file", "path", "source", "config", "configfile", "ip", "filename"}:
                resolved = _source_ref(value, base, mcu_dir)
                if resolved:
                    result.add(resolved)
    return sorted(result)


def _pin_signal(element: ET.Element) -> list[dict[str, object]]:
    signals: list[dict[str, object]] = []
    modes = _split_modes(_attr(element, "IOModes", "ioModes", "ioMode"))
    for child in element.iter():
        if child is element or _local(child.tag) not in {"signal", "pinsignal"}:
            continue
        name = _attr(child, "name", "signal", "function")
        child_modes = _split_modes(_attr(child, "IOModes", "ioModes", "ioMode"))
        if name:
            signals.append({"name": name, "io_modes": child_modes or modes})
    return signals


def _extract_pins(root: ET.Element) -> list[dict[str, object]]:
    pins: list[dict[str, object]] = []
    for element in root.iter():
        if _local(element.tag) != "pin":
            continue
        name = _attr(element, "name", "pinName")
        position = _attr(element, "position", "number", "pinNumber")
        if name and position:
            pins.append({"name": name, "position": position, "type": _attr(element, "type", "pinType"), "signals": _pin_signal(element)})
    return sorted(pins, key=lambda pin: (int(pin["position"]) if str(pin["position"]).isdigit() else 10**9, str(pin["name"])))


def _find_value(root: ET.Element, *names: str) -> str:
    wanted = {name.casefold() for name in names}
    for element in root.iter():
        for key, value in element.attrib.items():
            if key.rsplit("}", 1)[-1].casefold() in wanted and value:
                return value.strip()
    return ""


def ingest(root: str | Path, identity: str, output: str | Path | None = None) -> dict[str, object]:
    mcu_dir = _mcu_dir(root)
    source_root = mcu_dir.parent.parent if mcu_dir.name.casefold() == "mcu" and mcu_dir.parent.name.casefold() == "db" else mcu_dir
    families_path = mcu_dir / "families.xml"
    consumed: set[Path] = set()
    family_root = _read_xml(families_path) if families_path.is_file() else None
    if family_root is not None:
        consumed.add(families_path.resolve())
    mcu_path = _find_xml(mcu_dir, identity)
    mcu_root = _read_xml(mcu_path)
    consumed.add(mcu_path.resolve())
    pending = _referenced_xml(mcu_root, mcu_path.parent, mcu_dir)
    while pending:
        ref = pending.pop(0)
        if ref in consumed:
            continue
        consumed.add(ref)
        ref_root = _read_xml(ref)
        pending.extend(path for path in _referenced_xml(ref_root, ref.parent, mcu_dir) if path not in consumed)
    files = []
    for path in sorted(consumed):
        try:
            relative = path.relative_to(source_root).as_posix()
        except ValueError:
            relative = path.name
        files.append({"path": relative, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    artifact = {
        "schema": "copperscript-stm32g0-cubemx/v0.1",
        "identity": identity,
        "device": _find_value(mcu_root, "name", "mcu", "device") or identity,
        "family": _find_value(mcu_root, "family", "familyName"),
        "package": _find_value(mcu_root, "package", "packageName"),
        "cube_mx_version": _find_value(family_root, "version", "cubeVersion") if family_root is not None else "",
        "database_version": _find_value(family_root, "databaseVersion", "dbVersion") if family_root is not None else "",
        "pins": _extract_pins(mcu_root),
        "source_manifest": {"mcu_directory": "db/mcu", "files": files},
    }
    if output:
        destination = Path(output)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(artifact, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        manifest = {
            "schema": "copperscript-stm32g0-cubemx-source-manifest/v0.1",
            "identity": artifact["identity"],
            "cube_mx_version": artifact["cube_mx_version"],
            "database_version": artifact["database_version"],
            "files": artifact["source_manifest"]["files"],
        }
        destination.with_name(destination.stem + ".manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    return artifact


def _facts(path: Path) -> dict[tuple[str, str], dict[str, object]]:
    result = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            fact = json.loads(line)
            result[(str(fact.get("subject")), str(fact.get("field")))] = fact
    return result


def _csv_rows(path: Path) -> list[dict[str, str]]:
    import csv
    with path.open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def reconcile(artifact: dict[str, object], bundle: Path, evidence: Path, requested_pins: Iterable[str]) -> dict[str, object]:
    requested = list(requested_pins)
    xml_pins = {str(pin["name"]): pin for pin in artifact.get("pins", [])}
    part_pins = {row["name"]: row for row in _csv_rows(bundle / "parts" / "stm32g0b1cbt6" / "pins.csv")}
    pads = {row["name"] for row in _csv_rows(bundle / "pads.csv")}
    facts = _facts(evidence)
    errors: list[str] = []
    conflicts: list[dict[str, object]] = []
    expected_package = "LQFP48"
    actual_package = str(artifact.get("package", ""))
    if actual_package.replace("-", "").casefold() != expected_package.casefold():
        errors.append(f"CubeMX package {actual_package!r} does not match requested {expected_package!r}")
    for name in requested:
        xml_pin = xml_pins.get(name)
        bundle_pin = part_pins.get(name)
        if xml_pin is None:
            errors.append(f"CubeMX is missing requested pin {name}")
            continue
        if bundle_pin is None or name not in pads or bundle_pin.get("bond") != name:
            errors.append(f"normalized bundle is missing bond/pad {name}")
            continue
        xml_position = str(xml_pin.get("position"))
        bundle_position = str(bundle_pin.get("number"))
        evidence_fact = facts.get((name, "package_pin"))
        evidence_position = str(evidence_fact.get("value")) if evidence_fact else ""
        if xml_position != bundle_position:
            errors.append(f"CubeMX/bundle conflict for {name}: XML {xml_position}, bundle {bundle_position}")
        if not evidence_fact or evidence_fact.get("status") != "verified":
            errors.append(f"missing verified evidence for {name}.package_pin")
        elif evidence_position != xml_position:
            conflict = {"pin": name, "cubemx": xml_position, "datasheet_evidence": evidence_position}
            conflicts.append(conflict)
            errors.append(f"CubeMX/datasheet conflict for {name}: XML {xml_position}, evidence {evidence_position}")
    return {"identity": artifact.get("identity"), "requested_pins": requested, "exact_agreement": not errors, "errors": errors, "conflicts": conflicts, "production_publishable": False}


def packet(artifact: dict[str, object], pins: Iterable[str]) -> dict[str, object]:
    selected = set(pins)
    return {"schema": "copperscript-stm32g0-cubemx-packet/v0.1", "identity": artifact.get("identity"), "package": artifact.get("package"), "pins": [pin for pin in artifact.get("pins", []) if pin.get("name") in selected]}

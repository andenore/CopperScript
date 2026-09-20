"""Table-driven generation of CopperScript device and part packages.

The normalized bundle format is intentionally compact: JSON stores metadata
and CSV stores repetitive tables. This keeps datasheet extraction work small
and lets deterministic code perform validation and source rendering.
"""

from __future__ import annotations

import argparse
import csv
from dataclasses import dataclass
import io
import json
from pathlib import Path
import re
from typing import Mapping, Sequence

from .model import PartKind, PinCapability, PinType, UnpoweredBehavior


BUNDLE_SCHEMA = "copperscript-device-bundle/v0.1"
PACKET_SCHEMA = "copperscript-device-work-packet/v0.1"
UNKNOWN = "?"

PAD_FIELDS = (
    "name",
    "capabilities",
    "role",
    "power_domain",
    "unpowered",
    "voltage_min",
    "voltage_max",
)
PERIPHERAL_FIELDS = ("peripheral", "kind", "signal", "type", "required")
MUX_FIELDS = ("pad", "peripheral", "signal", "selector", "resource", "setting")
PIN_FIELDS = (
    "name",
    "number",
    "bond",
    "capabilities",
    "role",
    "voltage_min",
    "voltage_max",
)
SOURCE_FIELDS = ("document", "revision", "location", "url", "checksum")


class DeviceGenerationError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class PartBundle:
    directory: Path
    manifest: Mapping[str, object]
    pins: tuple[Mapping[str, str], ...]


@dataclass(frozen=True, slots=True)
class DeviceBundle:
    directory: Path
    manifest: Mapping[str, object]
    pads: tuple[Mapping[str, str], ...]
    peripherals: tuple[Mapping[str, str], ...]
    mux: tuple[Mapping[str, str], ...]
    parts: tuple[PartBundle, ...]

    @property
    def device(self) -> Mapping[str, object]:
        value = self.manifest.get("device")
        return value if isinstance(value, dict) else {}


def load_bundle(path: str | Path) -> DeviceBundle:
    directory = Path(path).resolve()
    manifest = _read_json(directory / "device.json")
    part_entries = manifest.get("parts", [])
    if not isinstance(part_entries, list):
        raise DeviceGenerationError("device.json: parts must be a list")
    parts: list[PartBundle] = []
    for index, entry in enumerate(part_entries):
        if not isinstance(entry, str):
            raise DeviceGenerationError(f"parts[{index}] must be a directory name")
        part_directory = (directory / entry).resolve()
        if not part_directory.is_relative_to(directory):
            raise DeviceGenerationError(f"part directory escapes bundle: {entry!r}")
        parts.append(
            PartBundle(
                directory=part_directory,
                manifest=_read_json(part_directory / "part.json"),
                pins=_read_csv(part_directory / "pins.csv", PIN_FIELDS),
            )
        )
    return DeviceBundle(
        directory=directory,
        manifest=manifest,
        pads=_read_csv(directory / "pads.csv", PAD_FIELDS),
        peripherals=_read_csv(directory / "peripherals.csv", PERIPHERAL_FIELDS),
        mux=_read_csv(directory / "mux.csv", MUX_FIELDS),
        parts=tuple(parts),
    )


def validate_bundle(bundle: DeviceBundle) -> tuple[str, ...]:
    errors: list[str] = []
    if bundle.manifest.get("schema") != BUNDLE_SCHEMA:
        errors.append(f"device.json: schema must be {BUNDLE_SCHEMA!r}")
    device = bundle.device
    device_name = _required_json_string(device, "name", "device", errors)
    _output_name(device, "device", errors)
    _validate_string_map(device.get("metadata", {}), "device.metadata", errors)
    _validate_source(device.get("source", {}), "device.source", errors)

    resources = _string_list(device.get("resources", []), "device.resources", errors)
    resource_names = set(resources)
    _duplicates(resources, "device resource", errors)

    domains = device.get("power_domains", [])
    domain_names: set[str] = set()
    domain_supply_pads: dict[str, tuple[str, ...]] = {}
    if not isinstance(domains, list):
        errors.append("device.power_domains must be a list")
    else:
        for index, domain in enumerate(domains):
            label = f"device.power_domains[{index}]"
            if not isinstance(domain, dict):
                errors.append(f"{label} must be an object")
                continue
            name = _required_json_string(domain, "name", label, errors)
            pads = _string_list(domain.get("supply_pads", []), f"{label}.supply_pads", errors)
            if name in domain_names:
                errors.append(f"duplicate power domain {name!r}")
            domain_names.add(name)
            domain_supply_pads[name] = tuple(pads)

    capability_values = {item.value for item in PinCapability}
    unpowered_values = {item.value for item in UnpoweredBehavior}
    pad_names: set[str] = set()
    for index, row in enumerate(bundle.pads, 2):
        label = f"pads.csv:{index}"
        _reject_unknown_marker(row, label, errors)
        name = _required_cell(row, "name", label, errors)
        capabilities = _pipe_values(
            _required_cell(row, "capabilities", label, errors)
        )
        _validate_choices(capabilities, capability_values, f"{label}: capabilities", errors)
        if name in pad_names:
            errors.append(f"{label}: duplicate pad {name!r}")
        pad_names.add(name)
        power_domain = row.get("power_domain", "")
        if power_domain and power_domain not in domain_names:
            errors.append(f"{label}: unknown power domain {power_domain!r}")
        unpowered = row.get("unpowered", "")
        if unpowered and unpowered not in unpowered_values:
            errors.append(f"{label}: unknown unpowered behavior {unpowered!r}")
        _validate_voltage(row.get("voltage_min", ""), f"{label}: voltage_min", errors)
        _validate_voltage(row.get("voltage_max", ""), f"{label}: voltage_max", errors)

    for domain, supplies in domain_supply_pads.items():
        for pad in supplies:
            if pad not in pad_names:
                errors.append(f"power domain {domain!r} references unknown pad {pad!r}")

    signal_types = {item.value for item in PinType}
    peripheral_signals: set[tuple[str, str]] = set()
    peripheral_kinds: dict[str, str] = {}
    for index, row in enumerate(bundle.peripherals, 2):
        label = f"peripherals.csv:{index}"
        _reject_unknown_marker(row, label, errors)
        peripheral = _required_cell(row, "peripheral", label, errors)
        kind = _required_cell(row, "kind", label, errors)
        signal = _required_cell(row, "signal", label, errors)
        signal_type = _required_cell(row, "type", label, errors)
        _validate_choices((signal_type,), signal_types, f"{label}: type", errors)
        required = row.get("required", "") or "true"
        if required not in {"true", "false"}:
            errors.append(f"{label}: required must be true or false")
        key = (peripheral, signal)
        if key in peripheral_signals:
            errors.append(f"{label}: duplicate peripheral signal {peripheral}.{signal}")
        peripheral_signals.add(key)
        previous_kind = peripheral_kinds.setdefault(peripheral, kind)
        if previous_kind != kind:
            errors.append(f"{label}: peripheral {peripheral!r} has inconsistent kinds")

    mux_keys: set[tuple[str, str, str]] = set()
    for index, row in enumerate(bundle.mux, 2):
        label = f"mux.csv:{index}"
        _reject_unknown_marker(row, label, errors)
        pad = _required_cell(row, "pad", label, errors)
        peripheral = _required_cell(row, "peripheral", label, errors)
        signal = _required_cell(row, "signal", label, errors)
        _required_cell(row, "selector", label, errors)
        if pad not in pad_names:
            errors.append(f"{label}: unknown pad {pad!r}")
        if (peripheral, signal) not in peripheral_signals:
            errors.append(f"{label}: unknown peripheral signal {peripheral}.{signal}")
        resource = row.get("resource", "")
        setting = row.get("setting", "")
        if bool(resource) != bool(setting):
            errors.append(f"{label}: resource and setting must be provided together")
        if resource and resource not in resource_names:
            errors.append(f"{label}: unknown resource {resource!r}")
        key = (pad, peripheral, signal)
        if key in mux_keys:
            errors.append(f"{label}: duplicate mux option {pad}:{peripheral}.{signal}")
        mux_keys.add(key)

    part_names: list[str] = []
    output_names = [device.get("output")]
    for part in bundle.parts:
        errors.extend(_validate_part(part, device_name, pad_names, capability_values))
        name = part.manifest.get("name")
        if isinstance(name, str):
            part_names.append(name)
        output_names.append(part.manifest.get("output"))
    _duplicates(part_names, "part", errors)
    _duplicates(
        [name for name in output_names if isinstance(name, str)],
        "generated output",
        errors,
    )
    return tuple(errors)


def render_bundle(bundle: DeviceBundle) -> Mapping[str, str]:
    errors = validate_bundle(bundle)
    if errors:
        raise DeviceGenerationError("\n".join(errors))
    device_output = str(bundle.device["output"])
    result = {device_output: _render_device(bundle)}
    for part in bundle.parts:
        result[str(part.manifest["output"])] = _render_part(bundle, part)
    return result


def write_bundle(bundle: DeviceBundle, output_directory: str | Path) -> tuple[Path, ...]:
    destination = Path(output_directory)
    destination.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for name, content in render_bundle(bundle).items():
        path = destination / name
        path.write_text(content, encoding="utf-8")
        written.append(path)
    return tuple(written)


def check_generated(bundle: DeviceBundle, output_directory: str | Path) -> tuple[str, ...]:
    destination = Path(output_directory)
    differences: list[str] = []
    for name, expected in render_bundle(bundle).items():
        path = destination / name
        if not path.exists():
            differences.append(f"missing generated file: {path}")
        elif path.read_text(encoding="utf-8") != expected:
            differences.append(f"generated file is stale: {path}")
    return tuple(differences)


def summarize_bundle(bundle: DeviceBundle) -> Mapping[str, object]:
    return {
        "schema": BUNDLE_SCHEMA,
        "device": bundle.device.get("name", ""),
        "pads": len(bundle.pads),
        "peripheral_signals": len(bundle.peripherals),
        "mux_options": len(bundle.mux),
        "parts": {
            str(part.manifest.get("name", part.directory.name)): len(part.pins)
            for part in bundle.parts
        },
        "unknown_cells": sum(
            _unknown_count(rows)
            for rows in (
                bundle.pads,
                bundle.peripherals,
                bundle.mux,
                *(part.pins for part in bundle.parts),
            )
        ),
        "validation_errors": len(validate_bundle(bundle)),
    }


def build_work_packet(
    bundle: DeviceBundle,
    section: str,
    *,
    part_name: str | None = None,
    match: str | None = None,
    missing_only: bool = False,
) -> str:
    fields: tuple[str, ...]
    rows: tuple[Mapping[str, str], ...]
    allowed: Mapping[str, object] = {}
    if section == "pads":
        fields, rows = PAD_FIELDS, bundle.pads
        allowed = {
            "capabilities": [item.value for item in PinCapability],
            "unpowered": [item.value for item in UnpoweredBehavior],
        }
    elif section == "peripherals":
        fields, rows = PERIPHERAL_FIELDS, bundle.peripherals
        allowed = {"type": [item.value for item in PinType]}
    elif section == "mux":
        fields, rows = MUX_FIELDS, bundle.mux
    elif section == "pins":
        if not part_name:
            raise DeviceGenerationError("--part is required for the pins section")
        selected = next(
            (part for part in bundle.parts if part.manifest.get("name") == part_name),
            None,
        )
        if selected is None:
            raise DeviceGenerationError(f"unknown part {part_name!r}")
        fields, rows = PIN_FIELDS, selected.pins
        allowed = {"capabilities": [item.value for item in PinCapability]}
    else:
        raise DeviceGenerationError(f"unknown packet section {section!r}")

    filtered = rows
    if match:
        needle = match.casefold()
        filtered = tuple(
            row for row in filtered if needle in " ".join(row.values()).casefold()
        )
    if missing_only:
        filtered = tuple(row for row in filtered if UNKNOWN in row.values())
    header = {
        "schema": PACKET_SCHEMA,
        "device": bundle.device.get("name", ""),
        "source": bundle.device.get("source", {}),
        "section": section,
        "part": part_name,
        "allowed": allowed,
        "unknown_marker": UNKNOWN,
        "rows": len(filtered),
    }
    output = io.StringIO()
    output.write(json.dumps(header, ensure_ascii=False, separators=(",", ":")))
    output.write("\n")
    writer = csv.DictWriter(output, fieldnames=fields, lineterminator="\n")
    writer.writeheader()
    writer.writerows({field: row.get(field, "") for field in fields} for row in filtered)
    return output.getvalue()


def _validate_part(
    part: PartBundle,
    device_name: str,
    pad_names: set[str],
    capability_values: set[str],
) -> tuple[str, ...]:
    errors: list[str] = []
    manifest = part.manifest
    label = f"{part.directory.name}/part.json"
    _required_json_string(manifest, "name", label, errors)
    kind = _required_json_string(manifest, "kind", label, errors)
    if kind and kind not in {item.value for item in PartKind}:
        errors.append(f"{label}: unknown part kind {kind!r}")
    declared_device = _required_json_string(manifest, "device", label, errors)
    if declared_device and declared_device != device_name:
        errors.append(
            f"{label}: device {declared_device!r} does not match {device_name!r}"
        )
    _required_json_string(manifest, "footprint", label, errors)
    _output_name(manifest, label, errors)
    _validate_source(manifest.get("source", {}), f"{label}.source", errors)
    pin_names: set[str] = set()
    pin_numbers: set[str] = set()
    for index, row in enumerate(part.pins, 2):
        row_label = f"{part.directory.name}/pins.csv:{index}"
        _reject_unknown_marker(row, row_label, errors)
        name = _required_cell(row, "name", row_label, errors)
        number = _required_cell(row, "number", row_label, errors)
        bonds = _pipe_values(_required_cell(row, "bond", row_label, errors))
        if name in pin_names:
            errors.append(f"{row_label}: duplicate pin name {name!r}")
        if number in pin_numbers:
            errors.append(f"{row_label}: duplicate pin number {number!r}")
        pin_names.add(name)
        pin_numbers.add(number)
        for bond in bonds:
            if bond not in pad_names:
                errors.append(f"{row_label}: bond references unknown pad {bond!r}")
        capabilities = _pipe_values(row.get("capabilities", ""))
        _validate_choices(
            capabilities, capability_values, f"{row_label}: capabilities", errors
        )
        _validate_voltage(row.get("voltage_min", ""), f"{row_label}: voltage_min", errors)
        _validate_voltage(row.get("voltage_max", ""), f"{row_label}: voltage_max", errors)
    if not part.pins:
        errors.append(f"{part.directory.name}/pins.csv: at least one pin is required")
    return tuple(errors)


def _render_device(bundle: DeviceBundle) -> str:
    device = bundle.device
    lines = _generated_header(bundle.directory.name, device.get("header", []))
    lines.append(f"device {device['name']} {{")
    for name, value in _string_map(device.get("metadata", {})).items():
        lines.append(f"    {name} = {_quote(value)};")
    _append_source(lines, device.get("source", {}), 4)
    if len(lines) > 1:
        lines.append("")
    for domain in device.get("power_domains", []):
        supplies = ",".join(domain["supply_pads"])
        lines.extend(
            [
                f"    power_domain {domain['name']} {{",
                f"        supply_pads = {_quote(supplies)};",
                "    }",
                "",
            ]
        )
    for row in bundle.pads:
        lines.append(f"    pad {row['name']} {{")
        _append_if(lines, "capabilities", row.get("capabilities", "").replace("|", ","), 8, quoted=True)
        _append_if(lines, "role", row.get("role", ""), 8)
        _append_if(lines, "power_domain", row.get("power_domain", ""), 8)
        _append_if(lines, "unpowered", row.get("unpowered", ""), 8)
        _append_if(lines, "voltage_min", row.get("voltage_min", ""), 8)
        _append_if(lines, "voltage_max", row.get("voltage_max", ""), 8)
        lines.extend(["    }", ""])
    peripheral_order: list[str] = []
    grouped: dict[str, list[Mapping[str, str]]] = {}
    for row in bundle.peripherals:
        if row["peripheral"] not in grouped:
            peripheral_order.append(row["peripheral"])
            grouped[row["peripheral"]] = []
        grouped[row["peripheral"]].append(row)
    for peripheral in peripheral_order:
        rows = grouped[peripheral]
        lines.append(f"    peripheral {peripheral}: {rows[0]['kind']} {{")
        for row in rows:
            required = row.get("required", "") or "true"
            suffix = ";" if required == "true" else " { required = false; }"
            lines.append(f"        signal {row['signal']}: {row['type']}{suffix}")
        lines.extend(["    }", ""])
    for resource in device.get("resources", []):
        lines.append(f"    resource {resource};")
    if device.get("resources"):
        lines.append("")
    for row in bundle.mux:
        prefix = f"    mux {row['pad']}: {row['peripheral']}.{row['signal']}"
        if row.get("resource"):
            lines.extend(
                [
                    f"{prefix} {{",
                    f"        selector = {_quote(row['selector'])};",
                    f"        resource = {row['resource']};",
                    f"        setting = {row['setting']};",
                    "    }",
                ]
            )
        else:
            lines.append(f"{prefix} {{ selector = {_quote(row['selector'])}; }}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def _render_part(bundle: DeviceBundle, part: PartBundle) -> str:
    manifest = part.manifest
    lines = _generated_header(bundle.directory.name, manifest.get("header", []))
    lines.extend(
        [
            f"part {manifest['name']} {{",
            f"    kind = {manifest['kind']};",
        ]
    )
    if manifest.get("manufacturer"):
        lines.append(f"    manufacturer = {_quote(str(manifest['manufacturer']))};")
    lines.extend(
        [
            f"    footprint = {_quote(str(manifest['footprint']))};",
            f"    device = {manifest['device']};",
        ]
    )
    _append_source(lines, manifest.get("source", {}), 4)
    lines.append("")
    for row in part.pins:
        lines.append(f"    pin {row['name']} {{")
        lines.append(f"        number = {_quote(row['number'])};")
        _append_if(lines, "bond", row.get("bond", "").replace("|", ","), 8)
        _append_if(lines, "capabilities", row.get("capabilities", "").replace("|", ","), 8, quoted=True)
        _append_if(lines, "role", row.get("role", ""), 8)
        _append_if(lines, "voltage_min", row.get("voltage_min", ""), 8)
        _append_if(lines, "voltage_max", row.get("voltage_max", ""), 8)
        lines.extend(["    }", ""])
    if lines[-1] == "":
        lines.pop()
    lines.append("}")
    return "\n".join(lines) + "\n"


def _generated_header(bundle_name: str, header: object) -> list[str]:
    lines = [
        f"// Generated from device-data/{bundle_name}; do not edit by hand.",
        "// Regenerate with: python tools/devicegen.py generate <bundle> --out-dir <package>",
    ]
    if isinstance(header, list):
        lines.extend(f"// {item}" for item in header if isinstance(item, str))
    return [*lines, ""]


def _append_source(lines: list[str], source: object, indent: int) -> None:
    if not isinstance(source, dict):
        return
    prefix = " " * indent
    for field in SOURCE_FIELDS:
        value = source.get(field)
        if isinstance(value, str) and value:
            lines.append(f"{prefix}source_{field} = {_quote(value)};")


def _append_if(
    lines: list[str],
    name: str,
    value: str,
    indent: int,
    *,
    quoted: bool = False,
) -> None:
    if value:
        rendered = _quote(value) if quoted else value
        lines.append(f"{' ' * indent}{name} = {rendered};")


def _read_json(path: Path) -> Mapping[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise DeviceGenerationError(f"cannot read {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise DeviceGenerationError(f"{path} must contain a JSON object")
    return value


def _read_csv(path: Path, expected_fields: tuple[str, ...]) -> tuple[Mapping[str, str], ...]:
    try:
        with path.open(encoding="utf-8", newline="") as handle:
            reader = csv.DictReader(handle)
            actual = tuple(reader.fieldnames or ())
            if actual != expected_fields:
                raise DeviceGenerationError(
                    f"{path}: expected CSV header {','.join(expected_fields)}"
                )
            rows: list[Mapping[str, str]] = []
            for line, row in enumerate(reader, 2):
                if None in row or any(value is None for value in row.values()):
                    raise DeviceGenerationError(
                        f"{path}:{line}: row does not match the CSV header"
                    )
                rows.append({key: value.strip() for key, value in row.items()})
            return tuple(rows)
    except OSError as exc:
        raise DeviceGenerationError(f"cannot read {path}: {exc}") from exc


def _required_json_string(
    value: Mapping[str, object], key: str, label: str, errors: list[str]
) -> str:
    item = value.get(key)
    if not isinstance(item, str) or not item or item == UNKNOWN:
        errors.append(f"{label}.{key} must be a non-empty string")
        return ""
    return item


def _required_cell(
    row: Mapping[str, str], key: str, label: str, errors: list[str]
) -> str:
    value = row.get(key, "")
    if not value or value == UNKNOWN:
        errors.append(f"{label}: {key} is required")
        return ""
    return value


def _output_name(value: Mapping[str, object], label: str, errors: list[str]) -> None:
    output = _required_json_string(value, "output", label, errors)
    if output and (Path(output).name != output or not output.endswith(".copper")):
        errors.append(f"{label}.output must be a .copper file name without directories")


def _validate_string_map(value: object, label: str, errors: list[str]) -> None:
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and isinstance(item, str) for key, item in value.items()
    ):
        errors.append(f"{label} must be an object containing string values")


def _validate_source(value: object, label: str, errors: list[str]) -> None:
    if value in ({}, None):
        return
    if not isinstance(value, dict):
        errors.append(f"{label} must be an object")
        return
    unknown = sorted(set(value) - set(SOURCE_FIELDS))
    if unknown:
        errors.append(f"{label} has unknown field {unknown[0]!r}")
    for field, item in value.items():
        if not isinstance(item, str) or item == UNKNOWN:
            errors.append(f"{label}.{field} must be a resolved string")


def _string_list(value: object, label: str, errors: list[str]) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item and item != UNKNOWN for item in value
    ):
        errors.append(f"{label} must be a list of resolved strings")
        return ()
    return tuple(value)


def _string_map(value: object) -> Mapping[str, str]:
    return value if isinstance(value, dict) else {}


def _duplicates(values: Sequence[str], label: str, errors: list[str]) -> None:
    seen: set[str] = set()
    for value in values:
        if value in seen:
            errors.append(f"duplicate {label} {value!r}")
        seen.add(value)


def _reject_unknown_marker(
    row: Mapping[str, str], label: str, errors: list[str]
) -> None:
    fields = [name for name, value in row.items() if value == UNKNOWN]
    if fields:
        errors.append(f"{label}: unresolved fields: {', '.join(fields)}")


def _pipe_values(value: str) -> tuple[str, ...]:
    return tuple(item.strip() for item in value.split("|") if item.strip())


def _validate_choices(
    values: Sequence[str], allowed: set[str], label: str, errors: list[str]
) -> None:
    for value in values:
        if value and value not in allowed:
            errors.append(f"{label}: unknown value {value!r}")


def _validate_voltage(value: str, label: str, errors: list[str]) -> None:
    if value and not re.fullmatch(r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:V|mV)", value):
        errors.append(f"{label} must be a V or mV quantity")


def _unknown_count(rows: Sequence[Mapping[str, str]]) -> int:
    return sum(value == UNKNOWN for row in rows for value in row.values())


def _quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="devicegen", description="Generate CopperScript device packages from compact tables"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    for command in ("summary", "validate"):
        child = subparsers.add_parser(command)
        child.add_argument("bundle", type=Path)
    generate = subparsers.add_parser("generate")
    generate.add_argument("bundle", type=Path)
    generate.add_argument("--out-dir", required=True, type=Path)
    check = subparsers.add_parser("check")
    check.add_argument("bundle", type=Path)
    check.add_argument("--out-dir", required=True, type=Path)
    packet = subparsers.add_parser("packet")
    packet.add_argument("bundle", type=Path)
    packet.add_argument("--section", choices=("pads", "peripherals", "mux", "pins"), required=True)
    packet.add_argument("--part")
    packet.add_argument("--match")
    packet.add_argument("--missing-only", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        bundle = load_bundle(args.bundle)
        if args.command == "summary":
            print(json.dumps(summarize_bundle(bundle), ensure_ascii=False, separators=(",", ":")))
            return 0
        if args.command == "validate":
            errors = validate_bundle(bundle)
            if errors:
                for error in errors:
                    print(f"ERROR: {error}")
                return 1
            print(f"OK: {bundle.device.get('name', '')} device bundle is valid")
            return 0
        if args.command == "generate":
            for path in write_bundle(bundle, args.out_dir):
                print(f"Generated {path}")
            return 0
        if args.command == "check":
            differences = check_generated(bundle, args.out_dir)
            if differences:
                for difference in differences:
                    print(f"ERROR: {difference}")
                return 1
            print(f"OK: generated sources are current in {args.out_dir}")
            return 0
        if args.command == "packet":
            print(
                build_work_packet(
                    bundle,
                    args.section,
                    part_name=args.part,
                    match=args.match,
                    missing_only=args.missing_only,
                ),
                end="",
            )
            return 0
    except (DeviceGenerationError, OSError) as exc:
        print(f"ERROR: {exc}")
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

"""Offline, explicit procurement selections; never infer substitutes or stock."""

from __future__ import annotations

import csv
import hashlib
import json
import re
from dataclasses import asdict, dataclass, fields
from pathlib import Path

from .elaborate import elaborate
from .erc import check, has_errors
from .model import Board, ComponentInstance, PartDefinition
from .quantities import Quantity
from .serializer import board_to_dict

SCHEMA = "copperscript-assembly-lock/v0.1"


class AssemblyError(ValueError):
    """Malformed selection data or an attempt to export an incomplete BOM."""


@dataclass(frozen=True, slots=True)
class AssemblySelection:
    reference: str
    part: str
    footprint: str | None
    manufacturer: str | None = None
    mpn: str | None = None
    supplier: str | None = None
    supplier_part: str | None = None
    source_url: str | None = None
    reviewed: bool = False

    def __post_init__(self) -> None:
        for name in ("reference", "part"):
            _text(getattr(self, name), name)
        for name in ("footprint", "manufacturer", "mpn", "supplier", "supplier_part", "source_url"):
            value = getattr(self, name)
            if value is not None:
                _text(value, name)
        if type(self.reviewed) is not bool:
            raise AssemblyError("reviewed must be a boolean")

    @property
    def exact(self) -> bool:
        return all((self.manufacturer, self.mpn, self.supplier, self.supplier_part, self.footprint))


@dataclass(frozen=True, slots=True)
class AssemblyLock:
    board: str
    electrical_sha256: str
    selections: tuple[AssemblySelection, ...]

    def __post_init__(self) -> None:
        _text(self.board, "board")
        if not isinstance(self.electrical_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", self.electrical_sha256):
            raise AssemblyError("electrical_sha256 must be a lowercase SHA-256 digest")
        object.__setattr__(self, "selections", tuple(self.selections))


def _text(value: object, name: str) -> None:
    if not isinstance(value, str) or not value.strip() or value != value.strip() or any(ord(c) < 32 for c in value):
        raise AssemblyError(f"{name} must be a nonempty, trimmed string without control characters")


def electrical_digest(board: Board) -> str:
    """Bind design semantics and library provenance, not diagnostic locations.

    Constraint origins identify source lines for diagnostics, so their absolute
    checkout paths must not invalidate selections on another machine. Strip
    only those fields; evidence references and arbitrary properties stay bound.
    """
    document = board_to_dict(board)
    for unit in (document, *document["module_definitions"]):
        for constraint in unit["constraints"]:
            constraint.pop("origins", None)
    canonical = json.dumps(document, indent=2, sort_keys=True) + "\n"
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _components(board: Board) -> dict[str, tuple[ComponentInstance, PartDefinition]]:
    flat = elaborate(board)
    result = {}
    for component in flat.components:
        if component.ref in result:
            raise AssemblyError(f"duplicate component reference: {component.ref}")
        part = flat.library.get(component.part)
        if part is None:
            raise AssemblyError(f"unknown part: {component.part}")
        result[component.ref] = (component, part)
    return result


def _footprint(component: ComponentInstance, part: PartDefinition) -> str | None:
    return component.footprint or (part.footprints[0] if part.footprints else None)


def snapshot(board: Board) -> AssemblyLock:
    return AssemblyLock(board.name, electrical_digest(board), tuple(
        AssemblySelection(ref, component.part, _footprint(component, part), part.manufacturer)
        for ref, (component, part) in sorted(_components(board).items()) if part.assembled
    ))


def lock_to_json(lock: AssemblyLock) -> str:
    return json.dumps({"schema": SCHEMA, **asdict(lock)}, indent=2, sort_keys=True) + "\n"


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise AssemblyError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def load_lock(path: str | Path) -> AssemblyLock:
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
        if not isinstance(data, dict) or set(data) != {"schema", "board", "electrical_sha256", "selections"}:
            raise AssemblyError("expected schema, board, electrical_sha256 and selections fields only")
        if data["schema"] != SCHEMA or not isinstance(data["selections"], list):
            raise AssemblyError("unsupported assembly schema or invalid selections list")
        names = {f.name for f in fields(AssemblySelection)}
        entries = []
        for entry in data["selections"]:
            if not isinstance(entry, dict) or set(entry) - names:
                raise AssemblyError("selection must be an object with known fields only")
            entries.append(AssemblySelection(**entry))
        return AssemblyLock(data["board"], data["electrical_sha256"], tuple(entries))
    except (json.JSONDecodeError, TypeError) as exc:
        raise AssemblyError(f"invalid assembly lock: {exc}") from exc


def check_assembly(board: Board, lock: AssemblyLock) -> dict[str, object]:
    components = _components(board)
    required = {ref for ref, (_, part) in components.items() if part.assembled}
    issues = []

    def issue(code: str, reference: str | None, message: str) -> None:
        issues.append({"code": code, "reference": reference, "message": message})

    if lock.board != board.name:
        issue("BOARD_MISMATCH", None, "lock belongs to a different board")
    if lock.electrical_sha256 != electrical_digest(board):
        issue("STALE_LOCK", None, "compiled electrical IR changed; review and regenerate the snapshot")
    seen = set()
    offers = {}
    exact = set()
    reviewed = set()
    for entry in sorted(lock.selections, key=lambda e: e.reference):
        ref = entry.reference
        if ref in seen:
            issue("DUPLICATE_SELECTION", ref, "component has more than one selection")
        seen.add(ref)
        if ref not in components:
            issue("UNKNOWN_REFERENCE", ref, "selection does not name a board component")
            continue
        component, part = components[ref]
        if not part.assembled:
            issue("NON_ASSEMBLED_REFERENCE", ref, "library part is explicitly non-assembled")
            continue
        if entry.part != component.part:
            issue("PART_MISMATCH", ref, "selected library part differs from the board")
        if entry.footprint != _footprint(component, part):
            issue("FOOTPRINT_MISMATCH", ref, "selected footprint differs from the board")
        if part.manufacturer and entry.manufacturer != part.manufacturer:
            issue("MANUFACTURER_MISMATCH", ref, "selection must use the library's manufacturer identity")
        if not entry.exact:
            issue("UNRESOLVED_SELECTION", ref, "manufacturer, full MPN, supplier, supplier part and footprint are required")
        else:
            exact.add(ref)
        if not entry.reviewed:
            issue("REVIEW_REQUIRED", ref, "package, ratings, mapping and footprint have not been approved")
        else:
            reviewed.add(ref)
        if entry.supplier == "jlcpcb" and entry.supplier_part and not re.fullmatch(r"C[1-9][0-9]*", entry.supplier_part):
            issue("INVALID_SUPPLIER_PART", ref, "JLCPCB codes must be C followed by a positive integer")
        if entry.supplier and entry.supplier_part:
            key = (entry.supplier, entry.supplier_part)
            identity = (entry.manufacturer, entry.mpn)
            if key in offers and offers[key] != identity:
                issue("CONFLICTING_OFFER", ref, "one supplier code is assigned to different manufacturer parts")
            offers[key] = identity
    for ref in sorted(required - seen):
        issue("MISSING_SELECTION", ref, "populated component has no assembly selection")
    return {
        "schema": "copperscript-assembly-report/v0.1", "board": board.name,
        "passed": not issues, "component_count": len(required),
        "exact_selection_count": len(exact), "reviewed_count": len(reviewed),
        "availability": "not_checked", "manufacturing_ready": False, "issues": issues,
    }


def write_jlcpcb_bom(board: Board, lock: AssemblyLock, path: str | Path) -> None:
    """Write only a fully reviewed selection BOM, not an ordering certificate."""
    if has_errors(check(board)):
        raise AssemblyError("ERC failed; BOM export refused")
    if not check_assembly(board, lock)["passed"]:
        raise AssemblyError("assembly selections failed; BOM export refused")
    if any(e.supplier != "jlcpcb" for e in lock.selections):
        raise AssemblyError("JLCPCB BOM export requires jlcpcb offers for every populated component")
    components = _components(board)
    groups = {}
    for entry in lock.selections:
        component, _ = components[entry.reference]
        value = (f"{format(component.value.value.normalize(), 'f')} {component.value.display_unit}"
                 if isinstance(component.value, Quantity)
                 else str(component.value) if component.value is not None else entry.mpn)
        key = (value, entry.footprint, entry.supplier_part, entry.manufacturer, entry.mpn)
        groups.setdefault(key, []).append(entry.reference)
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("Comment", "Designator", "Footprint", "LCSC Part #", "Manufacturer", "MPN"))
        for (value, footprint, code, manufacturer, mpn), references in sorted(groups.items()):
            writer.writerow((value, ",".join(sorted(references)), footprint, code, manufacturer, mpn))

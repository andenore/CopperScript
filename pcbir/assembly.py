"""Offline, explicit procurement selections; never infer substitutes or stock."""

from __future__ import annotations

import csv
import hashlib
import json
import re
from dataclasses import asdict, dataclass, fields
from pathlib import Path

from .backends.kicad_pcb import kicad_reference
from .elaborate import elaborate
from .erc import check, has_errors
from .model import Board, ComponentInstance, PartDefinition
from .serializer import board_to_dict

SCHEMA = "copperscript-assembly-lock/v0.2"
# v0.3 adds optional per-selection `alternates`; a lock is written as v0.3 only
# when it records any, so locks without them stay readable by v0.2 tools.
ALTERNATES_SCHEMA = "copperscript-assembly-lock/v0.3"
# v0.1 locks predate `populate` and load unchanged: every selection defaults to
# supplier assembly, which is exactly what those locks meant.
READABLE_SCHEMAS = (ALTERNATES_SCHEMA, SCHEMA, "copperscript-assembly-lock/v0.1")

# How a populated component reaches the board. Only "supplier" components are
# ordered and placed by the assembler, so only they belong in its BOM/CPL.
POPULATE_MODES = ("supplier", "consigned", "none")


class AssemblyError(ValueError):
    """Malformed selection data or an attempt to export an incomplete BOM."""


@dataclass(frozen=True, slots=True)
class Offer:
    """Another place to buy the selected manufacturer part; never what is fitted."""
    supplier: str
    supplier_part: str
    source_url: str | None = None

    def __post_init__(self) -> None:
        for name in ("supplier", "supplier_part"):
            _text(getattr(self, name), name)
        if self.source_url is not None:
            _text(self.source_url, "source_url")


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
    populate: str = "supplier"
    # Same manufacturer part from other distributors (spares, quotes, hand
    # builds). The assembler's BOM still uses only supplier/supplier_part.
    alternates: tuple[Offer, ...] = ()

    def __post_init__(self) -> None:
        for name in ("reference", "part"):
            _text(getattr(self, name), name)
        object.__setattr__(self, "alternates", tuple(self.alternates))
        if not all(isinstance(offer, Offer) for offer in self.alternates):
            raise AssemblyError("alternates must be supplier offers")
        for name in ("footprint", "manufacturer", "mpn", "supplier", "supplier_part", "source_url"):
            value = getattr(self, name)
            if value is not None:
                _text(value, name)
        if type(self.reviewed) is not bool:
            raise AssemblyError("reviewed must be a boolean")
        if self.populate not in POPULATE_MODES:
            raise AssemblyError(f"populate must be one of {', '.join(POPULATE_MODES)}")

    @property
    def ordered(self) -> bool:
        """The assembler sources and places this component."""
        return self.populate == "supplier"

    @property
    def exact(self) -> bool:
        """Identity required for this component's populate mode.

        A consigned component is still fitted, so it needs a full part identity
        for whoever hand-places it, but no supplier offer. A component that is
        not fitted needs neither.
        """
        if self.populate == "none":
            return True
        identity = (self.manufacturer, self.mpn, self.footprint)
        if self.populate == "consigned":
            return all(identity)
        return all((*identity, self.supplier, self.supplier_part))


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
    document = asdict(lock)
    for entry in document["selections"]:
        if not entry["alternates"]:
            del entry["alternates"]
        for offer in entry.get("alternates", ()):
            if offer["source_url"] is None:
                del offer["source_url"]
    schema = ALTERNATES_SCHEMA if any(entry.alternates for entry in lock.selections) else SCHEMA
    return json.dumps({"schema": schema, **document}, indent=2, sort_keys=True) + "\n"


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
        if data["schema"] not in READABLE_SCHEMAS or not isinstance(data["selections"], list):
            raise AssemblyError("unsupported assembly schema or invalid selections list")
        names = {f.name for f in fields(AssemblySelection)}
        entries = []
        for entry in data["selections"]:
            if not isinstance(entry, dict) or set(entry) - names:
                raise AssemblyError("selection must be an object with known fields only")
            if "alternates" in entry:
                offers = entry["alternates"]
                if data["schema"] != ALTERNATES_SCHEMA or not isinstance(offers, list) or not all(
                        isinstance(offer, dict) and not set(offer) - {f.name for f in fields(Offer)}
                        for offer in offers):
                    raise AssemblyError(f"alternates require {ALTERNATES_SCHEMA} and supplier offer objects")
                entry = {**entry, "alternates": tuple(Offer(**offer) for offer in offers)}
            entries.append(AssemblySelection(**entry))
        return AssemblyLock(data["board"], data["electrical_sha256"], tuple(entries))
    except (json.JSONDecodeError, TypeError) as exc:
        raise AssemblyError(f"invalid assembly lock: {exc}") from exc


_UNRESOLVED = {
    "supplier": "manufacturer, full MPN, supplier, supplier part and footprint are required",
    "consigned": "a consigned component still requires manufacturer, full MPN and footprint",
    "none": "",
}


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
    populate: dict[str, set[str]] = {mode: set() for mode in POPULATE_MODES}
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
        if entry.populate != "none" and part.manufacturer and entry.manufacturer != part.manufacturer:
            issue("MANUFACTURER_MISMATCH", ref, "selection must use the library's manufacturer identity")
        if not entry.exact:
            issue("UNRESOLVED_SELECTION", ref, _UNRESOLVED[entry.populate])
        else:
            exact.add(ref)
        populate[entry.populate].add(ref)
        if not entry.reviewed:
            issue("REVIEW_REQUIRED", ref, "package, ratings, mapping and footprint have not been approved")
        else:
            reviewed.add(ref)
        if entry.alternates and not (entry.manufacturer and entry.mpn):
            issue("UNRESOLVED_SELECTION", ref, "alternate offers require the manufacturer and full MPN they sell")
        listed = [(entry.supplier, entry.supplier_part)] if entry.supplier and entry.supplier_part else []
        listed += [(offer.supplier, offer.supplier_part) for offer in entry.alternates]
        if len({(supplier.casefold(), part) for supplier, part in listed}) != len(listed):
            issue("DUPLICATE_OFFER", ref, "one supplier code is listed more than once")
        for supplier, supplier_part in listed:
            if supplier == "jlcpcb" and not re.fullmatch(r"C[1-9][0-9]*", supplier_part):
                issue("INVALID_SUPPLIER_PART", ref, "JLCPCB codes must be C followed by a positive integer")
            key = (supplier, supplier_part)
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
        # Consigned and not-fitted components are absent from the supplier
        # BOM/CPL by design; report them so the omission is never silent.
        "supplier_assembled_count": len(populate["supplier"]),
        "consigned_count": len(populate["consigned"]),
        "not_fitted_count": len(populate["none"]),
        "consigned": sorted(populate["consigned"]), "not_fitted": sorted(populate["none"]),
        "availability": "not_checked", "manufacturing_ready": False, "issues": issues,
    }


def _designators(entries) -> dict[str, str]:
    """Board designator for each selection, as placed and printed on the PCB.

    Assemblers match the BOM, the CPL and the silkscreen by designator, and the
    KiCad export flattens hierarchical source references (BUCK_1V2/C_AVIN ->
    BUCK_1V2_C_AVIN). Using the source form would leave every hierarchical
    component unmatched against its own placement. Two references that flatten
    to the same designator would be indistinguishable to the assembler, so that
    is refused rather than silently merged.
    """
    result: dict[str, str] = {}
    owner: dict[str, str] = {}
    for entry in entries:
        designator = kicad_reference(entry.reference)
        if designator in owner and owner[designator] != entry.reference:
            raise AssemblyError(f"references {owner[designator]} and {entry.reference} "
                                f"both become board designator {designator}")
        owner[designator] = entry.reference
        result[entry.reference] = designator
    return result


def write_jlcpcb_bom(board: Board, lock: AssemblyLock, path: str | Path) -> None:
    """Write the fully reviewed, supplier-assembled selections; not an order.

    Consigned and not-fitted components are deliberately absent. The CPL is
    built from this file's designators, so omitting them here preserves the
    mandatory BOM/CPL reference equality and keeps the assembler from being
    asked to place a part nobody supplied it. Whoever fits the consigned parts
    works from the assembly report, which lists them by reference.
    """
    if has_errors(check(board)):
        raise AssemblyError("ERC failed; BOM export refused")
    if not check_assembly(board, lock)["passed"]:
        raise AssemblyError("assembly selections failed; BOM export refused")
    ordered = _supplier_assembled(lock, "JLCPCB BOM export")
    _write_bom_rows(board, ordered, _designators(lock.selections), Path(path))


def write_candidate_bom(board: Board, lock: AssemblyLock, directory: str | Path,
                        *, native_positions: str | Path | None = None) -> dict:
    """Prepare an explicitly unapproved review bundle without changing any gate.

    Only REVIEW_REQUIRED can remain unresolved. Missing/stale/conflicting
    selections and ERC errors still refuse output. The production BOM API is
    unchanged; this candidate is for human review, not an ordering certificate.
    Like the production BOM it covers supplier-assembled selections only, so
    its CPL matches the parts the assembler is actually asked to place.
    """
    if has_errors(check(board)):
        raise AssemblyError("ERC failed; candidate BOM refused")
    diagnostics = check_assembly(board, lock)
    if any(i['code'] != 'REVIEW_REQUIRED' for i in diagnostics['issues']):
        raise AssemblyError("candidate selections are inconsistent or incomplete")
    ordered = _supplier_assembled(lock, "candidate BOM")
    # Resolved before the directory exists: a designator collision leaves no output.
    designators = _designators(lock.selections)
    reference_map = {entry.reference: designators[entry.reference] for entry in ordered}
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=False)
    _write_bom_rows(board, ordered, designators, directory / 'bom-candidate-NOT-APPROVED.csv')
    diagnostics.update(purpose='human_review_only', qualified_release=False,
                       ordering_approved=False, electrical_sha256=lock.electrical_sha256,
                       native_reference_map=reference_map)
    if native_positions is not None:
        from .manufacturing_files import ManufacturingFilesError, write_jlcpcb_cpl
        native_positions = Path(native_positions)
        before = hashlib.sha256(native_positions.read_bytes()).hexdigest()
        try:
            write_jlcpcb_cpl(native_positions, directory / 'cpl-candidate-NOT-APPROVED.csv',
                             references=set(reference_map.values()))
        except ManufacturingFilesError as exc:
            raise AssemblyError(f'candidate CPL rejected: {exc}') from exc
        if hashlib.sha256(native_positions.read_bytes()).hexdigest() != before:
            raise AssemblyError('native positions changed during candidate review export')
        diagnostics['native_positions_sha256'] = before
        diagnostics['placement_review'] = 'not_approved; source/pose/pin-1/rotation review required'
    (directory / 'assembly-review.json').write_text(
        json.dumps(diagnostics, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    (directory / 'README.txt').write_text(
        'UNAPPROVED ASSEMBLY REVIEW CANDIDATE\n'
        'Do not submit this bundle as an approved order.\n'
        'Review exact packages/ratings/pin maps, supplier stock and final CPL.\n'
        'Selection flags and production BOM gates have not been changed.\n', encoding='utf-8')
    return diagnostics


def _supplier_assembled(lock: AssemblyLock, purpose: str) -> list[AssemblySelection]:
    ordered = [entry for entry in lock.selections if entry.ordered]
    if any(entry.supplier != "jlcpcb" for entry in ordered):
        raise AssemblyError(
            f"{purpose} requires jlcpcb offers for every supplier-assembled component; "
            'mark the others populate="consigned" or populate="none"')
    if not ordered:
        raise AssemblyError("no supplier-assembled components; nothing to export")
    return ordered


def _write_bom_rows(board: Board, entries, designators: dict[str, str], output: Path) -> None:
    components = _components(board)
    groups = {}
    for entry in entries:
        component, _ = components[entry.reference]
        value = str(component.value) if component.value is not None else entry.mpn
        key = (value, entry.footprint, entry.supplier_part, entry.manufacturer, entry.mpn)
        groups.setdefault(key, []).append(designators[entry.reference])
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("Comment", "Designator", "Footprint", "LCSC Part #", "Manufacturer", "MPN"))
        for (value, footprint, code, manufacturer, mpn), references in sorted(groups.items()):
            writer.writerow((value, ",".join(sorted(references)), footprint, code, manufacturer, mpn))


def _short_footprint(footprint: str | None) -> str:
    """Library-qualified or path-like footprint ids, reduced to the footprint name."""
    if not footprint:
        return ""
    name = footprint.rsplit("/", 1)[-1].rsplit(":", 1)[-1]
    return name.removesuffix(".kicad_mod")


def write_hand_assembly_list(board: Board, lock: AssemblyLock, path: str | Path) -> int:
    """Write every component the supplier will not place; return the count.

    The companion to write_jlcpcb_bom, which omits these by design. Consigned
    components must be bought and fitted by hand; not-fitted components are
    listed too, so their empty footprints read as intended rather than as an
    assembly defect.
    """
    if not check_assembly(board, lock)["passed"]:
        raise AssemblyError("assembly selections failed; hand-assembly list refused")
    designators = _designators(lock.selections)
    groups: dict[tuple[str, ...], list[str]] = {}
    for entry in lock.selections:
        if entry.ordered:
            continue
        action = "hand-assemble" if entry.populate == "consigned" else "do not fit"
        # Sourcing is optional on consigned selections; when recorded it is
        # what the person buying the parts needs, so carry it through.
        key = (action, entry.manufacturer or "", entry.mpn or "", _short_footprint(entry.footprint),
               entry.supplier or "", entry.supplier_part or "", entry.source_url or "")
        groups.setdefault(key, []).append(designators[entry.reference])
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("Action", "Designator", "Quantity", "Manufacturer", "MPN", "Footprint",
                         "Supplier", "Supplier Part", "URL"))
        for (action, manufacturer, mpn, footprint, supplier, part, url), references in sorted(groups.items()):
            writer.writerow((action, ",".join(sorted(references)), len(references),
                             manufacturer, mpn, footprint, supplier, part, url))
    return sum(len(references) for references in groups.values())


def write_ordering_list(board: Board, lock: AssemblyLock, path: str | Path) -> int:
    """Write every selection with its sourcing, whoever fits it; return the count.

    The supplier BOM and the hand-assembly list each cover one side of the
    split. Buying, quoting and re-ordering spares need the whole board in one
    sheet, so this lists supplier-assembled, consigned and not-fitted
    selections together, with the assembly route of each.
    """
    if not check_assembly(board, lock)["passed"]:
        raise AssemblyError("assembly selections failed; ordering list refused")
    components = _components(board)
    designators = _designators(lock.selections)
    routes = {"supplier": "supplier-assembled", "consigned": "hand-assemble", "none": "do not fit"}
    # One column pair per alternate distributor, also filled where that
    # distributor is the primary supplier, so a single-source order reads
    # straight down one column.
    distributors: dict[str, str] = {}
    for offer in sorted((offer for entry in lock.selections for offer in entry.alternates),
                        key=lambda offer: offer.supplier):
        distributors.setdefault(offer.supplier.casefold(), offer.supplier)
    groups: dict[tuple[str, ...], list[str]] = {}
    for entry in lock.selections:
        component, _ = components[entry.reference]
        value = str(component.value) if component.value is not None else entry.mpn or ""
        offers = {offer.supplier.casefold(): offer for offer in entry.alternates}
        if entry.supplier and entry.supplier_part:
            offers.setdefault(entry.supplier.casefold(), Offer(entry.supplier, entry.supplier_part, entry.source_url))
        columns = tuple(field for name in sorted(distributors) for field in (
            offers[name].supplier_part if name in offers else "",
            offers[name].source_url or "" if name in offers else ""))
        key = (routes[entry.populate], value, entry.manufacturer or "", entry.mpn or "",
               _short_footprint(entry.footprint), entry.supplier or "", entry.supplier_part or "",
               entry.source_url or "", *columns)
        groups.setdefault(key, []).append(designators[entry.reference])
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("Designator", "Quantity", "Value", "Manufacturer", "MPN", "Footprint",
                         "Assembly", "Supplier", "Supplier Part", "URL",
                         *(f"{distributors[name]} {label}" for name in sorted(distributors)
                           for label in ("Part", "URL"))))
        for (route, value, manufacturer, mpn, footprint, supplier, part, url, *columns), references in sorted(
                groups.items(), key=lambda item: (sorted(item[1]), item[0])):
            writer.writerow((",".join(sorted(references)), len(references), value, manufacturer, mpn,
                             footprint, route, supplier, part, url, *columns))
    return sum(len(references) for references in groups.values())

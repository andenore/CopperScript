"""Compact source-derived discovery; library data is never executed or qualified.

Indexing parses declarations, not the compiler's expanded board/library IR.
Inspection retrieves one definition with explicit pagination and source bindings.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, fields, is_dataclass
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import re

from .lexer import tokenize
from .packages import (
    PackageResolver, _inventory_checksum, _module_inventory, find_manifest, read_manifest,
)
from .parser import parse
from .quantities import decimal_text
from .syntax import CopperScriptError, Document, RawQuantity, SourceLocation

INDEX_SCHEMA = "copperscript.library-index/v1"
PAGE_SCHEMA = "copperscript.library-context/v1"
KINDS = ("part", "device", "module", "board_profile")
SECTIONS = ("overview", "interfaces", "requirements", "composition", "properties", "notes",
            "dependencies", "declarations", "source")
_INTERFACES = {"PinDecl", "PadDecl", "PortDecl", "PeripheralDecl", "InterfaceDecl",
               "UnitDecl", "PowerDomainDecl", "SignalGroupDecl", "ModeGroupDecl", "PadSetDecl"}
_REQUIREMENTS = {"ConstraintDecl", "RouteRuleDecl", "SupplyDecl", "PowerStateDecl"}
_COMPOSITION = {"ComponentDecl", "ModuleInstanceDecl", "NetDecl", "ConfigurationDecl",
                "MuxDecl", "MechanicalProfileUseDecl", "MechanicalItemDecl", "MechanicalDecl"}
_CORE_PROPERTIES = {"category", "manufacturer", "assembled", "mpn", "description", "device",
                    "footprint", "footprints", "value", "source_document", "source_revision",
                    "source_location", "source_url", "source_checksum"}


def _json(value: object) -> str:
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"), allow_nan=False)


def _digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _value(value: object) -> object:
    if isinstance(value, RawQuantity):
        return f"{decimal_text(Decimal(value.value))} {value.unit}"
    if isinstance(value, SourceLocation):
        return {"line": value.line, "column": value.column}
    if is_dataclass(value):
        return {f.name: _value(getattr(value, f.name)) for f in fields(value) if f.name != "location"}
    if isinstance(value, dict):
        return {k: _value(v) for k, v in value.items()}
    if isinstance(value, (tuple, list)):
        return [_value(v) for v in value]
    return value


def _is_requirement(declaration: object) -> bool:
    return (type(declaration).__name__ in _REQUIREMENTS or
            type(declaration).__name__ == "ComponentDecl" and
            any(k in declaration.attributes for k in ("role", "decouples")))


@dataclass
class Catalog:
    index: dict
    documents: dict[str, Document]
    source_text: dict[str, str]
    resolver: PackageResolver | None = None
    roots: tuple[Path, ...] = ()


def _notes(text: str) -> list[dict]:
    """Extract lexer-skipped comments; comment markers inside strings are data."""
    notes, previous = [], 0
    for token in tokenize(text):
        gap = text[previous:token.location.offset]
        for match in re.finditer(r"(?:#|//)([^\r\n]*)", gap):
            offset = previous + match.start()
            notes.append({"type": "SourceNote", "line": text.count("\n", 0, offset) + 1,
                          "text": match.group(1).strip()})
        previous = token.end_offset
    return notes


def _provider(root: Path, module: str, version: str, expected_digest: str | None = None) -> Catalog:
    inventory = _module_inventory(root)
    checksum = _inventory_checksum(inventory)
    if expected_digest is not None and checksum != expected_digest:
        raise ValueError(f"library changed during discovery: {module}")
    entries, diagnostics, documents, texts = [], [], {}, {}
    for item in inventory:
        if not item.path.endswith(".copper"):
            continue
        path = root / item.path
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != item.sha256:
            raise ValueError(f"library changed during discovery: {module}/{item.path}")
        try:
            text = data.decode("utf-8")
            document = parse(text, str(path))
        except (CopperScriptError, UnicodeError) as exc:
            diagnostics.append({"source": f"{module}/{item.path}", "code": getattr(exc, "code", "ENCODING"),
                                "detail": str(exc)[:300]})
            continue
        if document.kind not in KINDS:
            continue
        directory = Path(item.path).parent.as_posix()
        package = module if directory == "." else f"{module}/{directory}"
        selector = f"{package}::{document.name}"
        properties = {d.name: _value(d.value) for d in document.declarations
                      if type(d).__name__ in {"PartPropertyDecl", "DevicePropertyDecl"}}
        capabilities = sorted({getattr(d, "kind", getattr(d, "pin_type", ""))
                               for d in document.declarations
                               if type(d).__name__ in {"PeripheralDecl", "InterfaceDecl", "PortDecl"}} - {""})
        entry = {"selector": selector, "kind": document.kind, "symbol": document.name,
                 "package": package, "provider": module, "version": version,
                 "content_digest": checksum, "source": item.path,
                 "source_sha256": "sha256:" + item.sha256, "line": document.location.line,
                 "metadata": {k: str(properties[k])[:160] for k in sorted(_CORE_PROPERTIES & properties.keys())},
                 "metadata_is_preview": True,
                 "capabilities": capabilities[:8], "capabilities_omitted": max(0, len(capabilities) - 8),
                 "counts": {"declarations": len(document.declarations), "imports": len(document.imports),
                            "interfaces": sum(type(d).__name__ in _INTERFACES for d in document.declarations),
                            "requirements": sum(_is_requirement(d) for d in document.declarations)}}
        # File identity also handles duplicate exports; inspection refuses ambiguity.
        key = f"{module}/{item.path}"
        documents[key], texts[key] = document, text
        entries.append(entry)
    duplicates = Counter((e["selector"], e["kind"]) for e in entries)
    for selector, kind in sorted(k for k, n in duplicates.items() if n > 1):
        diagnostics.append({"source": selector, "code": "DUPLICATE_EXPORT",
                            "detail": f"multiple {kind} definitions; repair this library before use"})
    # Detect mutation of any asset, not merely the sources read above.
    if _inventory_checksum(_module_inventory(root)) != checksum:
        raise ValueError(f"library changed during discovery: {module}")
    return Catalog({"schema": INDEX_SCHEMA, "providers": [{"module": module, "version": version,
                    "content_digest": checksum}], "entries": sorted(entries, key=lambda e: (e["selector"], e["kind"], e["source"])),
                    "diagnostics": diagnostics, "complete": not diagnostics}, documents, texts, roots=(root,))


def index_library(root: Path) -> Catalog:
    """Build a portable index for one library; no lock or source writes."""
    root = root.resolve()
    manifest = read_manifest(root / "copper.mod")
    return _provider(root, manifest.path, "workspace")


def project_catalog(project: Path, *, locked: bool = True, offline: bool = True,
                    providers: tuple[str, ...] = ()) -> Catalog:
    """Search exactly the host project's declared dependencies, never sibling repos.

    Cross-library imports keep the compiler's policy: declare their versions in
    the host manifest. This does not implement automatic transitive selection.
    """
    manifest_path = find_manifest(project.resolve())
    if manifest_path is None:
        raise ValueError("library discovery requires a project copper.mod")
    manifest = read_manifest(manifest_path)
    unknown = set(providers) - manifest.requirements.keys()
    if unknown:
        raise ValueError(f"provider not declared in project: {sorted(unknown)[0]}")
    resolver = PackageResolver(manifest, locked=locked, offline=offline)
    catalog = Catalog({"schema": INDEX_SCHEMA, "providers": [], "entries": [],
                       "diagnostics": [], "complete": True}, {}, {}, resolver)
    for module in sorted(set(providers or manifest.requirements)):
        resolved = resolver.resolve(module, SourceLocation(str(manifest_path), 0, 1, 1))
        provider = _provider(resolved.directory, module, resolved.version, resolved.checksum)
        catalog.index["providers"].extend(provider.index["providers"])
        # Mirror longest-prefix resolution: nested module imports shadow a parent.
        catalog.index["entries"].extend(e for e in provider.index["entries"]
                                        if resolver._matching_module(e["package"]) == module)
        catalog.index["diagnostics"].extend(provider.index["diagnostics"])
        catalog.documents.update(provider.documents)
        catalog.source_text.update(provider.source_text)
        catalog.roots += provider.roots
    catalog.index["complete"] = not catalog.index["diagnostics"]
    catalog.index["entries"].sort(key=lambda e: (e["selector"], e["kind"], e["source"]))
    return catalog


def _page(header: dict, rows: list, *, offset: int, limit: int, max_chars: int) -> dict:
    if type(offset) is not int or offset < 0 or type(limit) is not int or not 1 <= limit <= 100:
        raise ValueError("offset must be nonnegative; limit must be 1..100")
    if type(max_chars) is not int or not 2048 <= max_chars <= 65536:
        raise ValueError("max_chars must be 2048..65536")
    if offset > len(rows):
        raise ValueError("offset exceeds result count")
    result = {"schema": PAGE_SCHEMA, **header, "offset": offset, "total": len(rows), "items": [],
              "next_offset": None, "omitted": len(rows) - offset, "page_complete": False,
              "evidence_level": "source_declarations_only"}
    if len(_json(result)) > max_chars - 600:
        raise ValueError("identity/header exceeds output budget; increase max_chars")
    for i in range(offset, min(len(rows), offset + limit)):
        row = rows[i]
        trial = {**result, "items": result["items"] + [row]}
        if len(_json(trial)) > max_chars - 300:
            if result["items"]:
                break
            # A single oversized declaration must not make pagination stall.
            row = {"record_index": i, "needs_source": True, "record_chars": len(_json(row)),
                   "type": row.get("type", "entry"), "line": row.get("line"),
                   "detail": "record exceeds page budget; inspect its bound source or increase max_chars"}
        result["items"].append(row)
    consumed = offset + len(result["items"])
    result.update(next_offset=consumed if consumed < len(rows) else None,
                  omitted=len(rows) - consumed,
                  page_complete=consumed == len(rows) and not any(r.get("needs_source") for r in result["items"]))
    return result


def search(catalog: Catalog, query: str = "", *, kind: str | None = None, offset: int = 0,
           limit: int = 5, max_chars: int = 8000) -> dict:
    if kind is not None and kind not in KINDS:
        raise ValueError("unsupported definition kind")
    if len(query) > 512:
        raise ValueError("query must be at most 512 characters")
    terms = query.casefold().split()
    rows = []
    for entry in catalog.index["entries"]:
        document = catalog.documents[f"{entry['provider']}/{entry['source']}"]
        haystack = (entry["selector"] + " " + entry["kind"] + " " + _json(_value(document))).casefold()
        if (kind is None or entry["kind"] == kind) and all(t in haystack for t in terms):
            rows.append(entry)
    def relevance(entry: dict) -> tuple:
        symbol, metadata = entry["symbol"].casefold(), _json(entry["metadata"]).casefold()
        score = sum(30 if t == symbol else 10 if t in symbol else 5 if t in metadata else 1 for t in terms)
        return (-score, entry["selector"], entry["kind"], entry["source"])
    rows.sort(key=relevance)
    return _page({"action": "search", "query": query, "discovery_complete": catalog.index["complete"],
                  "diagnostic_count": len(catalog.index["diagnostics"]),
                  "next_step": "library inspect SELECTOR --kind KIND; library diagnostics for discovery failures"},
                 rows, offset=offset, limit=limit, max_chars=max_chars)


def _record(declaration: object, index: int) -> dict:
    return {"record_index": index, "type": type(declaration).__name__.removesuffix("Decl"),
            "line": declaration.location.line, **_value(declaration)}


def inspect(catalog: Catalog, selector: str, *, kind: str | None = None, section: str = "overview",
            offset: int = 0, limit: int = 20, max_chars: int = 8000,
            expected_digest: str | None = None, members: tuple[str, ...] = ()) -> dict:
    if section not in SECTIONS:
        raise ValueError("unsupported inspection section")
    if members and section in {"source", "notes"}:
        raise ValueError("member selection applies to declarations, not source/notes")
    if len(members) > 32 or any(not isinstance(m, str) or not m or len(m) > 128 for m in members):
        raise ValueError("members must be 1..128 character names; at most 32 selections")
    matches = [e for e in catalog.index["entries"] if e["selector"] == selector and (kind is None or e["kind"] == kind)]
    if not matches:
        raise ValueError("definition not found; use library search with its provider/package")
    if len(matches) != 1:
        raise ValueError("ambiguous definition; supply --kind or repair duplicate exports")
    entry = matches[0]
    if expected_digest is not None and expected_digest != entry["content_digest"]:
        raise ValueError("library content changed since selection; search again before inspection")
    key = f"{entry['provider']}/{entry['source']}"
    document = catalog.documents[key]
    declarations = list(document.declarations)
    rows = []
    if section == "source":
        rows = [{"type": "SourceLine", "line": n, "text": line}
                for n, line in enumerate(catalog.source_text[key].splitlines(), 1)]
    elif section == "dependencies":
        rows = [{"type": "BuiltinLibrary", "name": name} for name in document.libraries]
        for declaration in document.imports:
            row = _record(declaration, len(rows))
            try:
                resolved = catalog.resolver.resolve(declaration.path, declaration.location) if catalog.resolver else None
                if resolved:
                    row.update(status="resolved", package=resolved.import_path, provider=resolved.module_path,
                               version=resolved.version, content_digest=resolved.checksum)
                else:
                    row.update(status="not_resolved", detail="inspect through a consuming project's declared dependencies")
            except CopperScriptError as exc:
                row.update(status="blocked", code=exc.code, detail=str(exc)[:300])
            rows.append(row)
        # Parts can inherit a device contract; this is not expanded into overview.
        for declaration in declarations:
            if type(declaration).__name__ == "PartPropertyDecl" and declaration.name == "device":
                rows.append({"type": "DeviceContract", "name": str(declaration.value),
                             "selector": f"{entry['package']}::{declaration.value}",
                             "detail": "inspect the selected device contract; inheritance is not expanded here"})
    elif section == "notes":
        rows = _notes(catalog.source_text[key])
    else:
        if section == "overview":
            rows.extend(_notes(catalog.source_text[key]))
        for i, declaration in enumerate(declarations):
            t = type(declaration).__name__
            property_decl = t in {"PartPropertyDecl", "DevicePropertyDecl"}
            selected = (section == "declarations" or
                        section == "interfaces" and t in _INTERFACES or
                        section == "requirements" and _is_requirement(declaration) or
                        section == "composition" and t in _COMPOSITION or
                        section == "properties" and property_decl or
                        section == "overview" and (t in _INTERFACES or _is_requirement(declaration) or
                                                   property_decl and declaration.name in _CORE_PROPERTIES))
            if selected:
                rows.append(_record(declaration, i))
        if section == "overview":
            # Source caveats and obligations precede potentially large pin tables.
            def priority(row: dict) -> int:
                if row["type"] == "SourceNote":
                    return 0
                declaration = declarations[row["record_index"]]
                if _is_requirement(declaration):
                    return 1
                return 2 if row["type"] in {"PartProperty", "DeviceProperty"} else 3
            rows.sort(key=priority)
    # Comments may contain essential caveats: retain them, explicitly paginated.
    comments = _notes(catalog.source_text[key])
    section_total = len(rows)
    unmatched = set(members)
    if members:
        requested = {m.casefold(): m for m in members}
        filtered = []
        for row in rows:
            names = {str(row[k]).casefold() for k in ("name", "ref", "kind") if k in row}
            names.update(str(t).casefold() for t in row.get("targets", []))
            if "decouples" in row.get("attributes", {}):
                names.add(str(row["attributes"]["decouples"]).casefold())
            matching = names & requested.keys()
            if matching:
                filtered.append(row)
                unmatched.difference_update(m for m in members if m.casefold() in matching)
        rows = filtered
    identity = {k: v for k, v in entry.items() if k not in
                {"metadata", "metadata_is_preview", "capabilities", "capabilities_omitted", "counts"}}
    header = {"action": "inspect", "identity": identity, "section": section,
              "available_sections": list(SECTIONS), "comment_count": len(comments),
              "section_total": section_total, "unselected": section_total - len(rows),
              "requested_members": list(members), "unmatched_members": sorted(unmatched),
              "view_scope": "selected source definition; inherited/imported definitions are not expanded",
              "next_step": "read requirements, properties and notes; declarations/source preserve all declared data; dependencies identifies imported contracts"}
    return _page(header, rows, offset=offset, limit=limit, max_chars=max_chars)


def diagnostics(catalog: Catalog, **page_options: int) -> dict:
    return _page({"action": "diagnostics", "discovery_complete": catalog.index["complete"]},
                 catalog.index["diagnostics"], **page_options)


def render_page(page: dict) -> str:
    """Compact JSON is also the default text format: deterministic and bounded."""
    return _json(page)


def write_index(catalog: Catalog, output: Path) -> None:
    """Export an index outside the library to avoid self-referential digests.

    Consumers should regenerate against pinned sources; exported indexes are
    descriptive snapshots and never substitutes for source/electrical evidence.
    """
    if any(output.resolve().is_relative_to(root) for root in catalog.roots):
        raise ValueError("index output must stay outside indexed library roots")
    if output.exists():
        raise ValueError("index output must be a new file")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(_json(catalog.index) + "\n")

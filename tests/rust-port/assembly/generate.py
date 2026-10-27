"""Regenerate the assembly-lock fixtures of the Rust comparison corpus.

    python tests/rust-port/assembly/generate.py

For every board below it writes, under ``tests/rust-port/assembly/<name>/``:

- ``unreviewed.lock``: the snapshot as ``copper assembly snapshot`` writes it;
- ``reviewed.lock``: every selection resolved and reviewed, with consigned,
  not-fitted and alternate-offer selections mixed in (schema v0.3 when any
  alternates are present);
- ``candidate.lock``: resolved but partly unreviewed, with one non-JLCPCB
  supplier, so only ``candidate-bom`` accepts it;
- ``positions.csv``: a KiCad position export naming every component;
- ``bom.csv``: the JLCPCB BOM of ``reviewed.lock`` when that lock passes.

``errors/`` holds hand-written variants of the valid board's reviewed lock,
each breaking one rule. Locks bind the compiled electrical IR, so a source
change makes them stale; rerun this script after deliberate board changes.
The Python implementation is the oracle, so this uses it directly.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import replace
from pathlib import Path

from pcbir.assembly import Offer, check_assembly, lock_to_json, snapshot, write_jlcpcb_bom, _components
from pcbir.backends.kicad_pcb import kicad_reference
from pcbir.loader import load_board

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
BOARDS = {
    "valid_board": "examples/valid_board/board.copper",
    "hierarchical_board": "examples/hierarchical_board/board.copper",
    "cm4_baseboard": "examples/cm4_baseboard/board.copper",
    "nrf52_coin_cell": "examples/nrf52_coin_cell/board.copper",
    "full_vertical": "examples/full_vertical/board.copper",
    "round_led_ring": "examples/round_led_ring/board.copper",
    "tps62130a_macro": "examples/tps62130a_macro/board.copper",
    "device_model_showcase": "examples/device_model_showcase/board.copper",
    "collision": "tests/rust-port/assembly/boards/collision.copper",
    "dnp": "tests/rust-port/assembly/boards/dnp/board.copper",
}


def _code(*parts: object) -> str:
    digest = hashlib.sha256("|".join(map(str, parts)).encode()).hexdigest()
    return f"C{int(digest[:8], 16) % 900000 + 100000}"


def _resolved(board, draft, *, reviewed, candidate: bool):
    components = _components(board)
    entries = []
    for index, entry in enumerate(draft.selections):
        component, part = components[entry.reference]
        if entry.footprint is None:
            entries.append(replace(entry, populate="none", reviewed=reviewed(index)))
            continue
        manufacturer = entry.manufacturer or "Generic Passives"
        value = str(component.value) if component.value is not None else ""
        mpn = "-".join(text for text in (part.name.rsplit(".", 1)[-1], value.replace(" ", ""),
                                         entry.footprint.rsplit(":", 1)[-1].rsplit("/", 1)[-1]) if text)
        code = _code(manufacturer, mpn)
        resolved = replace(entry, manufacturer=manufacturer, mpn=mpn, supplier="jlcpcb", supplier_part=code,
                           source_url=f"https://jlcpcb.com/partdetail/{code}" if index % 3 == 0 else None,
                           reviewed=reviewed(index))
        if index % 6 == 4:
            resolved = replace(resolved, populate="consigned", supplier=None, supplier_part=None,
                               source_url=f"https://example.com/parts/{mpn}")
        elif index % 9 == 7:
            resolved = replace(resolved, populate="none")
        elif candidate and index % 5 == 2:
            resolved = replace(resolved, supplier="lcsc", supplier_part=code)
        if index % 4 == 1:
            resolved = replace(resolved, alternates=(
                Offer("Mouser", f"M-{code[1:]}", f"https://example.com/mouser/{code[1:]}"),
                Offer("digikey", f"D{code[1:]}")))
        entries.append(resolved)
    return replace(draft, selections=tuple(entries))


def _positions(board) -> str:
    rows = ["Ref,Val,Package,PosX,PosY,Rot,Side"]
    for index, (reference, (component, part)) in enumerate(sorted(_components(board).items())):
        value = str(component.value) if component.value is not None else part.name
        rotation = ("0.0000", "90.0000", "180.0000", "-90.0000")[index % 4]
        side = "bottom" if index % 5 == 3 else "top"
        rows.append(f'"{kicad_reference(reference)}","{value}","{part.name}",'
                    f"{10 + index * 2.5:.4f},{-20 - index * 1.25:.4f},{rotation},{side}")
    return "\n".join(rows) + "\n"


def main() -> None:
    for name, source in BOARDS.items():
        board = load_board(ROOT / source, locked=True, offline=True)
        directory = HERE / name
        directory.mkdir(exist_ok=True)
        draft = snapshot(board)
        (directory / "unreviewed.lock").write_text(lock_to_json(draft), encoding="utf-8")
        reviewed = _resolved(board, draft, reviewed=lambda index: True, candidate=False)
        (directory / "reviewed.lock").write_text(lock_to_json(reviewed), encoding="utf-8")
        candidate = _resolved(board, draft, reviewed=lambda index: index % 2 == 0, candidate=True)
        (directory / "candidate.lock").write_text(lock_to_json(candidate), encoding="utf-8")
        (directory / "positions.csv").write_text(_positions(board), encoding="utf-8", newline="")
        bom = directory / "bom.csv"
        bom.unlink(missing_ok=True)
        if check_assembly(board, reviewed)["passed"]:
            try:
                write_jlcpcb_bom(board, reviewed, bom)
            except ValueError:
                pass
    _errors()


def _errors() -> None:
    """One broken rule per lock, from the valid board's reviewed lock."""
    base = json.loads((HERE / "valid_board" / "reviewed.lock").read_text(encoding="utf-8"))
    directory = HERE / "errors"
    directory.mkdir(exist_ok=True)

    def write(name: str, document) -> None:
        text = document if isinstance(document, str) else json.dumps(document, indent=2, sort_keys=True) + "\n"
        (directory / f"{name}.lock").write_text(text, encoding="utf-8")

    def variant(name: str, change) -> None:
        document = json.loads(json.dumps(base))
        change(document)
        write(name, document)

    selections = lambda d: d["selections"]  # noqa: E731
    first = lambda d: d["selections"][0]  # noqa: E731
    variant("stale", lambda d: d.update(electrical_sha256="0" * 64))
    variant("board-mismatch", lambda d: d.update(board="OtherBoard"))
    variant("unknown-reference", lambda d: selections(d).append({**first(d), "reference": "R99"}))
    variant("duplicate-selection", lambda d: selections(d).append(dict(first(d))))
    variant("missing-selection", lambda d: selections(d).pop())
    variant("part-mismatch", lambda d: first(d).update(part="RESISTOR"))
    variant("footprint-mismatch", lambda d: first(d).update(footprint="0805"))
    variant("manufacturer-mismatch", lambda d: [e.update(manufacturer="Someone Else") for e in selections(d)
                                                if e["reference"] == "U2"])
    variant("unresolved", lambda d: first(d).update(mpn=None))
    variant("unresolved-consigned", lambda d: first(d).update(populate="consigned", mpn=None))
    variant("unreviewed", lambda d: first(d).update(reviewed=False))
    variant("invalid-supplier-part", lambda d: first(d).update(supplier_part="C0123"))
    def conflicting(d) -> None:
        offered = [e for e in selections(d) if e["supplier"] == "jlcpcb"]
        other = next(e for e in offered if e["mpn"] != offered[0]["mpn"])
        other["supplier_part"] = offered[0]["supplier_part"]
    variant("conflicting-offer", conflicting)
    variant("duplicate-offer", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"), first(d).update(
        alternates=[{"supplier": "JLCPCB", "supplier_part": first(d)["supplier_part"]}])))
    variant("alternates-without-mpn", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"), first(d).update(
        populate="none", mpn=None, alternates=[{"supplier": "Mouser", "supplier_part": "M-1"}])))
    variant("other-supplier", lambda d: first(d).update(supplier="lcsc"))
    variant("all-consigned", lambda d: [e.update(populate="consigned") for e in selections(d)])
    variant("none-fitted", lambda d: [e.update(populate="none") for e in selections(d)])
    variant("v0.1", lambda d: (d.update(schema="copperscript-assembly-lock/v0.1"),
                               [(e.pop("populate"), e.pop("alternates", None)) for e in selections(d)]))
    # Malformed documents.
    variant("unknown-schema", lambda d: d.update(schema="copperscript-assembly-lock/v9"))
    variant("extra-field", lambda d: d.update(comment="x"))
    variant("selections-not-list", lambda d: d.update(selections={}))
    variant("selection-not-object", lambda d: selections(d).append("R1"))
    variant("unknown-selection-field", lambda d: first(d).update(notes="x"))
    variant("alternates-v0.2", lambda d: (d.update(schema="copperscript-assembly-lock/v0.2"),
                                          first(d).update(alternates=[{"supplier": "a", "supplier_part": "b"}])))
    variant("alternates-not-list", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"),
                                              first(d).update(alternates={})))
    variant("offer-unknown-field", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"),
                                              first(d).update(alternates=[{"supplier": "a", "x": 1}])))
    variant("offer-missing-part", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"),
                                             first(d).update(alternates=[{"supplier": "a"}])))
    variant("offer-untrimmed", lambda d: (d.update(schema="copperscript-assembly-lock/v0.3"),
                                          first(d).update(alternates=[{"supplier": " a", "supplier_part": "b"}])))
    variant("missing-reference", lambda d: first(d).pop("reference"))
    variant("missing-three", lambda d: [first(d).pop(key) for key in ("reference", "part", "footprint")])
    variant("missing-two", lambda d: [first(d).pop(key) for key in ("part", "footprint")])
    variant("reference-number", lambda d: first(d).update(reference=7))
    variant("part-empty", lambda d: first(d).update(part=""))
    variant("mpn-control", lambda d: first(d).update(mpn="A\tB"))
    variant("mpn-untrimmed", lambda d: first(d).update(mpn="A "))
    variant("reviewed-string", lambda d: first(d).update(reviewed="yes"))
    variant("populate-unknown", lambda d: first(d).update(populate="hand"))
    variant("board-empty", lambda d: d.update(board=""))
    variant("digest-uppercase", lambda d: d.update(electrical_sha256="A" * 64))
    variant("digest-short", lambda d: d.update(electrical_sha256="abc"))
    dnp = json.loads((HERE / "dnp" / "reviewed.lock").read_text(encoding="utf-8"))
    dnp["selections"].append({**dnp["selections"][0], "reference": "C_DNP", "part": "p.DNP"})
    write("dnp-non-assembled", dnp)
    many = json.loads((HERE / "full_vertical" / "reviewed.lock").read_text(encoding="utf-8"))
    for entry in many["selections"]:
        entry["populate"] = "consigned" if entry["populate"] == "supplier" else entry["populate"]
    write("full-vertical-consigned", many)
    write("not-json", "{not json\n")
    write("empty", "")
    write("list", "[]\n")
    write("trailing-data", json.dumps(base) + "\n{}\n")
    write("duplicate-key", '{"schema": "a", "schema": "b"}\n')
    write("duplicate-nested-key", json.dumps(base, indent=2).replace('"reviewed": true', '"reviewed": true, "reviewed": true', 1))
    write("bom-prefix", "﻿" + json.dumps(base) + "\n")
    write("crlf-error", '{\r\n  "schema": \r\n}\r\n')


if __name__ == "__main__":
    main()

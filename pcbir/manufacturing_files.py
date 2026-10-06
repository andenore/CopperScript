"""Native KiCad manufacturing files, explicitly without independent CAM signoff.

This consumes the final native board, not a reconstructed physical IR: filled
copper and native edits must survive unchanged. The strict qualified release
API in manufacturing.py remains separate.
"""

from __future__ import annotations

import csv
from decimal import Decimal, InvalidOperation
from hashlib import sha256
import json
from pathlib import Path
import shutil
import tempfile
from zipfile import ZipFile, ZIP_DEFLATED, ZipInfo

from .importers.kicad_mod import _parse_sexpr, _children, _first, _atom
from .manufacturing import CommandRunner, _subprocess_runner, _run


class ManufacturingFilesError(ValueError):
    """Native DRC or export inventory checks failed."""


def _number(value: str) -> str:
    try:
        result = Decimal(value)
        if not result.is_finite():
            raise InvalidOperation
    except InvalidOperation as exc:
        raise ManufacturingFilesError(f"invalid placement number: {value!r}") from exc
    return format(result, "f")


def write_jlcpcb_cpl(native_csv: Path, output: Path, *, references: set[str]) -> None:
    """Rename KiCad columns; never guess supplier-specific rotation offsets.

    Native millimetres, positive-CCW rotation and common unmirrored front/back
    coordinates are preserved. Exact BOM/CPL reference equality is mandatory.
    """
    with native_csv.open(encoding="utf-8-sig", newline="") as stream:
        reader = csv.DictReader(stream)
        if not {"Ref", "PosX", "PosY", "Rot", "Side"} <= set(reader.fieldnames or ()):
            raise ManufacturingFilesError("unexpected KiCad position CSV columns")
        rows = list(reader)
    selected = []
    seen = set()
    for row in rows:
        ref = row["Ref"]
        if ref in seen:
            raise ManufacturingFilesError(f"duplicate position reference: {ref}")
        seen.add(ref)
        if ref not in references:
            continue  # Explicit non-populated parts do not belong in assembly CPL.
        if row["Side"] not in {"top", "bottom"}:
            raise ManufacturingFilesError(f"unknown placement side: {row['Side']}")
        selected.append((ref, _number(row["PosX"]), _number(row["PosY"]),
                         _number(row["Rot"]), row["Side"].title()))
    if seen & references != references:
        raise ManufacturingFilesError("BOM references missing from position export: "
                                      + ", ".join(sorted(references - seen)))
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("Designator", "Mid X", "Mid Y", "Rotation", "Layer"))
        writer.writerows(sorted(selected))


def _bom_references(path: Path) -> set[str]:
    with path.open(encoding="utf-8-sig", newline="") as stream:
        reader = csv.DictReader(stream)
        required = {"Comment", "Designator", "Footprint", "LCSC Part #"}
        if not required <= set(reader.fieldnames or ()):
            raise ManufacturingFilesError("BOM is not a JLCPCB selection CSV")
        refs = set()
        for row in reader:
            if any(not row.get(key, "").strip() for key in required):
                raise ManufacturingFilesError("BOM contains an incomplete selection")
            for ref in row["Designator"].split(","):
                ref = ref.strip()
                if not ref or ref in refs:
                    raise ManufacturingFilesError(f"empty or duplicate BOM reference: {ref}")
                refs.add(ref)
    if not refs:
        raise ManufacturingFilesError("BOM contains no populated components")
    return refs


def _zip_files(output: Path, files: list[Path], root: Path) -> None:
    with ZipFile(output, "w", compression=ZIP_DEFLATED) as archive:
        for path in sorted(files):
            info = ZipInfo(path.relative_to(root).as_posix(), (1980, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, path.read_bytes())


def export_manufacturing_files(
    pcb: Path, output: Path, *, kicad_cli: Path,
    skip_independent_cam: bool = False, bom: Path | None = None,
    replace_existing: bool = False,
    runner: CommandRunner | None = None,
) -> Path:
    """Stage, native-refill/DRC, export, inventory-check and publish atomically.

    Existing generated output requires explicit replacement; its previous
    contents are retained as a sibling backup. Circular outlines/slots remain KiCad's responsibility.
    This is file generation, not independent CAM qualification or an order.
    """
    if not skip_independent_cam:
        raise ManufacturingFilesError("explicit --skip-independent-cam acknowledgement required")
    pcb = pcb.resolve()
    if output.is_symlink():
        raise ManufacturingFilesError("output directory must not be a symlink")
    output = output.resolve()
    if output.exists():
        if not replace_existing:
            raise FileExistsError(f"manufacturing directory already exists: {output}")
        manifest = output / "manifest.json"
        if not manifest.is_file() or json.loads(manifest.read_text(encoding="utf-8")).get("schema") != "copperscript-manufacturing-files/v0.1":
            raise ManufacturingFilesError("replacement requires a CopperScript manufacturing-files directory")
    if pcb.is_relative_to(output) or bom is not None and bom.resolve().is_relative_to(output):
        raise ManufacturingFilesError("input PCB/BOM must be outside the output directory")
    root = _parse_sexpr(pcb.read_text(encoding="utf-8"), str(pcb))
    if root[0] != "kicad_pcb":
        raise ManufacturingFilesError("expected a native .kicad_pcb board")
    layers_node = _first(root, "layers")
    copper = tuple(str(item[1]) for item in (layers_node or [])[1:]
                   if isinstance(item, list) and len(item) > 1 and str(item[1]).endswith(".Cu"))
    if not copper or not any(
        _atom(_first(item, "layer"), 1) == "Edge.Cuts"
        for tag in ("gr_circle", "gr_line", "gr_arc", "gr_poly", "gr_rect") for item in _children(root, tag)
    ):
        raise ManufacturingFilesError("board needs copper layers and an outline")
    layers = (*copper, "F.Mask", "B.Mask", "F.Silkscreen", "B.Silkscreen",
              "F.Paste", "B.Paste", "Edge.Cuts")
    refs = _bom_references(bom) if bom else None
    run = runner or _subprocess_runner
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}-", dir=output.parent))
    try:
        native = stage / pcb.name
        shutil.copy2(pcb, native)
        project = pcb.with_suffix(".kicad_pro")
        if not project.is_file():
            raise ManufacturingFilesError("matching .kicad_pro required to retain design rules")
        shutil.copy2(project, native.with_suffix(".kicad_pro"))
        # Exported component-scoped custom rules belong to the same project.
        if pcb.with_suffix(".kicad_dru").is_file():
            shutil.copy2(pcb.with_suffix(".kicad_dru"), native.with_suffix(".kicad_dru"))
        for path in (pcb.parent / "fp-lib-table", pcb.parent / "CopperScript.pretty"):
            if path.is_dir():
                shutil.copytree(path, stage / path.name)
            elif path.is_file():
                shutil.copy2(path, stage / path.name)
        result = run((str(kicad_cli), "version"), stage)
        if result.returncode or not result.stdout.strip():
            raise ManufacturingFilesError("cannot query KiCad version")
        version = result.stdout.strip()
        drc = stage / "drc.json"
        _run(run, (str(kicad_cli), "pcb", "drc", "--refill-zones", "--save-board",
                   "--format", "json", "--severity-all", "--exit-code-violations",
                   "-o", str(drc), str(native)), stage, "run native DRC")
        report = json.loads(drc.read_text(encoding="utf-8"))
        if any(key not in report or not isinstance(report[key], list) or report[key]
               for key in ("violations", "unconnected_items")):
            raise ManufacturingFilesError("native DRC report must contain zero violations and opens")
        gerbers, drills = stage / "gerbers", stage / "drill"
        gerbers.mkdir()
        drills.mkdir()
        _run(run, (str(kicad_cli), "pcb", "export", "gerbers", "--layers", ",".join(layers),
                   "--no-protel-ext", "--subtract-soldermask", "-o", str(gerbers), str(native)),
             stage, "export Gerbers")
        _run(run, (str(kicad_cli), "pcb", "export", "drill", "--format", "excellon",
                   "--excellon-units", "mm", "--drill-origin", "absolute", "--excellon-separate-th",
                   "--generate-report", "-o", str(drills), str(native)),
             stage, "export drills")
        netlist = stage / "board.d356"
        _run(run, (str(kicad_cli), "pcb", "export", "ipcd356", "-o", str(netlist), str(native)),
             stage, "export manufacturing netlist")
        artwork = list(gerbers.glob("*.gbr"))
        drill_files = list(drills.glob("*.drl"))
        if len(artwork) != len(layers) or not drill_files or not netlist.is_file():
            raise ManufacturingFilesError("missing manufacturing outputs")
        files = [*artwork, *drill_files, netlist]
        if any(not file.stat().st_size for file in files):
            raise ManufacturingFilesError("empty manufacturing output")
        native_pos = stage / "positions-kicad.csv"
        _run(run, (str(kicad_cli), "pcb", "export", "pos", "--format", "csv", "--units", "mm",
                   "--side", "both", "--exclude-dnp", "-o", str(native_pos), str(native)),
             stage, "export positions")
        if refs is not None:
            shutil.copy2(bom, stage / "bom.csv")
            write_jlcpcb_cpl(native_pos, stage / "cpl.csv", references=refs)
        _zip_files(stage / "gerbers-drill.zip", [*artwork, *drill_files], stage)
        document = {
            "schema": "copperscript-manufacturing-files/v0.1", "kicad_version": version,
            "input_pcb_sha256": sha256(pcb.read_bytes()).hexdigest(),
            "exported_pcb_sha256": sha256(native.read_bytes()).hexdigest(),
            "native_drc": "passed", "independent_cam": "skipped_by_request",
            "qualified_release": False, "supplier_availability": "not_checked",
            "assembly_bom_included": bom is not None, "copper_layers": list(copper),
            "placement_convention": "KiCad mm, common origin, Y up, no bottom X mirror, native CCW rotations",
            "assembly_preview_review": "required; verify centroid, pin 1, polarity and supplier rotation",
        }
        (stage / "manifest.json").write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        paths = sorted(path for path in stage.rglob("*") if path.is_file())
        (stage / "SHA256SUMS").write_text("".join(
            f"{sha256(path.read_bytes()).hexdigest()}  {path.relative_to(stage).as_posix()}\n" for path in paths
        ), encoding="utf-8")
        _zip_files(stage / "manufacturing-package.zip", [*paths, stage / "SHA256SUMS"], stage)
        backup = None
        if output.exists():
            backup = Path(tempfile.mkdtemp(prefix=f".{output.name}-previous-", dir=output.parent))
            backup.rmdir()  # Our new empty directory; previous outputs are never deleted.
            output.rename(backup)
        try:
            stage.rename(output)
        except OSError:
            if backup is not None:
                backup.rename(output)
            raise
        return output
    finally:
        if stage.exists():
            shutil.rmtree(stage)  # Only our exact newly-created staging directory.

"""Write a complete generated PCB project without changing global KiCad setup."""

from hashlib import sha256
import json
from pathlib import Path, PurePosixPath

from .base import ArtifactManifest
from .kicad_pcb import KICAD_RULES_MARKER, VIA_PROCESS_GENERATOR, VIA_PROCESS_SCHEMA


def kicad_export_digest(manifest: ArtifactManifest) -> str:
    """Bind verification to every exported asset, including library lookup."""
    document = [(item.name, sha256(item.content.encode("utf-8")).hexdigest())
                for item in manifest.artifacts]
    return sha256(json.dumps(document, separators=(",", ":")).encode()).hexdigest()


def write_kicad_project(manifest: ArtifactManifest, output: Path) -> tuple[Path, ...]:
    """Write PCB, same-stem project, library table and canonical footprints.

    The PCB, project, custom-rules file and via-process sidecar follow a
    user-selected output stem. Generated library assets use project-relative
    paths and may be shared by exports in one directory. Unrelated library
    tables are never replaced or merged. A same-stem rules file left by an
    earlier CopperScript export is removed when this export has none, so it
    can never relax KiCad DRC for a board that no longer declares it. Generated
    process sidecars are likewise removed when the new export needs none;
    unrelated process files are preserved, or reject a conflicting export.
    """
    output = Path(output)
    if manifest.backend != "kicad-pcb" or len(manifest.artifacts) < 3:
        raise ValueError("a complete KiCad PCB artifact manifest is required")
    root = output.parent.resolve()
    destinations = []
    for index, artifact in enumerate(manifest.artifacts):
        relative = PurePosixPath(artifact.name)
        if (not relative.parts or relative.is_absolute()
                or any(part in {"..", "."} for part in relative.parts)
                or "\\" in artifact.name or ":" in artifact.name):
            raise ValueError(f"unsafe generated artifact path: {artifact.name!r}")
        if index == 0:
            path = output
        elif index == 1:
            path = output.with_suffix(".kicad_pro")
        elif relative.suffix == ".kicad_dru" and len(relative.parts) == 1:
            path = output.with_suffix(".kicad_dru")
        elif relative.name.endswith(".via-process.json") and len(relative.parts) == 1:
            path = output.with_suffix(".via-process.json")
        else:
            path = root / Path(*relative.parts)
        if not path.resolve().is_relative_to(root):
            raise ValueError(f"artifact escapes output directory: {artifact.name!r}")
        destinations.append(path)
    if len({str(path.resolve()).casefold() for path in destinations}) != len(destinations):
        raise ValueError("generated artifact destinations collide")
    table = next((item for item in manifest.artifacts if item.name == "fp-lib-table"), None)
    if table is None:
        raise ValueError("KiCad export omitted its footprint library table")
    existing_table = root / "fp-lib-table"
    if existing_table.exists() and existing_table.read_bytes() != table.content.encode("utf-8"):
        raise FileExistsError("refusing to overwrite an unrelated fp-lib-table; export to a separate build directory")
    rules = output.with_suffix(".kicad_dru")
    stale_rules = (rules not in destinations and rules.is_file()
                   and KICAD_RULES_MARKER in rules.read_text(encoding="utf-8", errors="replace"))
    process = output.with_suffix(".via-process.json")
    generated_process = process.is_file() and _is_generated_via_process(process)
    if process.exists() and process in destinations and not generated_process:
        raise FileExistsError("refusing to overwrite an unrelated via-process sidecar")
    # Check every path before writing anything. No recursive deletion of stale
    # assets: older content-addressed files may belong to another board export.
    # Only generated same-stem rules/process files are removed; user files stay.
    if stale_rules:
        rules.unlink()
    if generated_process and process not in destinations:
        process.unlink()
    for artifact, path in zip(manifest.artifacts, destinations, strict=True):
        path.parent.mkdir(parents=True, exist_ok=True)
        content = artifact.content
        if path.suffix == ".kicad_pro":
            project = json.loads(content)
            project["meta"]["filename"] = path.name
            content = json.dumps(project, indent=2, sort_keys=True) + "\n"
        path.write_bytes(content.encode("utf-8"))
    return tuple(destinations)


def _is_generated_via_process(path: Path) -> bool:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, UnicodeError):
        return False
    return (isinstance(document, dict) and document.get("schema") == VIA_PROCESS_SCHEMA
            and document.get("generated_by") == VIA_PROCESS_GENERATOR)

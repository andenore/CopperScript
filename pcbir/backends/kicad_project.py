"""Write a complete generated PCB project without changing global KiCad setup."""

from hashlib import sha256
import json
from pathlib import Path, PurePosixPath

from .base import ArtifactManifest
from .kicad_pcb import KICAD_RULES_MARKER


def kicad_export_digest(manifest: ArtifactManifest) -> str:
    """Bind verification to every exported asset, including library lookup."""
    document = [(item.name, sha256(item.content.encode("utf-8")).hexdigest())
                for item in manifest.artifacts]
    return sha256(json.dumps(document, separators=(",", ":")).encode()).hexdigest()


def write_kicad_project(manifest: ArtifactManifest, output: Path) -> tuple[Path, ...]:
    """Write PCB, same-stem project, library table and canonical footprints.

    Only the PCB, the project and an optional custom-rules file follow a
    user-selected output stem. Generated library assets use project-relative
    paths and may be shared by exports in one directory. Unrelated library
    tables are never replaced or merged. A same-stem rules file left by an
    earlier CopperScript export is removed when this export has none, so it
    can never relax KiCad DRC for a board that no longer declares it.
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
    # Check every path before writing anything. No recursive deletion of stale
    # assets: older content-addressed files may belong to another board export.
    # Only a generated same-stem rules file is removed; hand-written ones stay.
    if stale_rules:
        rules.unlink()
    for artifact, path in zip(manifest.artifacts, destinations, strict=True):
        path.parent.mkdir(parents=True, exist_ok=True)
        content = artifact.content
        if path.suffix == ".kicad_pro":
            project = json.loads(content)
            project["meta"]["filename"] = path.name
            content = json.dumps(project, indent=2, sort_keys=True) + "\n"
        path.write_bytes(content.encode("utf-8"))
    return tuple(destinations)

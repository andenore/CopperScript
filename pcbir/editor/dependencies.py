"""Initial-open dependency preparation, separate from offline editing."""
from pathlib import Path

from ..packages import prepare_dependencies
from ..parser import parse
from ..packages import find_manifest
from .source import SourceSnapshot


def dependency_context(source, project=None):
    """Keep a managed definition attached to the consuming project's pins."""
    source = Path(source).resolve()
    if project is not None:
        manifest = find_manifest(Path(project).resolve())
        if manifest is None:
            raise ValueError('the selected --project has no copper.mod')
        return manifest
    parts = source.parts
    for index, part in enumerate(parts[:-2]):
        if part.casefold() == '.copper-cache' and parts[index + 1] == 'pkg':
            manifest = find_manifest(Path(*parts[:index]))
            if manifest:
                return manifest
    return source


def prepare_editor_dependencies(source, text=None, *, offline=False, locked=False, project=None,
                                progress=None, **settings):
    """Prepare explicit manifest requirements for any Copper document kind.

    Whole-module inventories include source packages, footprints and macro assets.
    No source is rewritten and no manifest/version is inferred from an URL. The
    caller must lower the selected document with locked/offline resolution after
    this succeeds. Syntax is checked before fetching. Unsaved edits never alter
    dependency selection, which remains explicit in the manifest.
    """
    source = Path(source)
    if source.suffix.lower() == '.kicad_mod':
        raise ValueError('A .kicad_mod file is a KiCad footprint. Open the hard macro .copper definition, '
                         'or open its board and select Open definition.')
    snapshot = SourceSnapshot(source.read_bytes() if text is None else
                              text.encode("utf-8") if isinstance(text, str) else text, str(source))
    document = parse(snapshot.text, str(source))
    if document.kind not in {"board", "macro", "board_profile"}:
        raise ValueError("physical editor supports board, macro and board_profile documents")
    context = dependency_context(source, project if document.kind != 'board' else None)
    return prepare_dependencies(context, offline=offline, locked=locked, progress=progress)

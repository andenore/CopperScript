"""Generic locked/offline editor physicalization shared by document hosts."""
from pathlib import Path

from ..compiler import compile_design_source
from ..erc import check, has_errors
from ..footprints import FootprintResolver
from ..hard_macros import apply_hard_macro_scene
from ..packages import find_manifest
from ..physicalize import prototype_physicalize, resolved_physicalize
from ..placement_templates import apply_placement_templates
from .session import EditorSession
from .source import SourceSnapshot
from .transactions import SourceWorkspace


def document_session(source, reader, *, footprint_roots=(), layers=None, fab_profile="generic",
                     templates=None, macros=(), allow_proxy=False, overlay=None, project=None):
    source = Path(source).resolve()
    from ..parser import parse
    raw = reader() if reader else source.read_bytes()
    text = SourceSnapshot(raw, str(source)).text
    document = parse(text, str(source))
    if document.kind in {"macro", "board_profile"}:
        if overlay or templates or macros:
            raise ValueError("board overlays and placement assets require a board document")
        from .definitions import DefinitionSession
        return DefinitionSession(source, reader=reader, footprint_roots=footprint_roots, project=project)
    if document.kind != "board":
        raise ValueError("physical editor supports board, macro and board_profile documents")
    design = compile_design_source(text, str(source), locked=True, offline=True)
    diagnostics = check(design.electrical)
    if has_errors(diagnostics):
        raise ValueError("source fails ERC: " + "; ".join(map(str, diagnostics)))
    from .configuration import physical_options

    def build(candidate):
        options = physical_options(candidate, layers=layers, fab_profile=fab_profile)
        board = (prototype_physicalize(candidate, options) if allow_proxy else resolved_physicalize(candidate,
            FootprintResolver(base_directory=source.parent, search_roots=tuple(Path(p).resolve() for p in footprint_roots),
                              locked=True, offline=True), options))
        if templates:
            board = apply_placement_templates(board, Path(templates), locked=True, offline=True)
        for macro in macros:
            board = apply_hard_macro_scene(board, Path(macro), locked=True, offline=True)
        return board

    board = build(design)
    manifest = find_manifest(source.parent)
    paths = [Path(p) for p in (templates, *macros) if p]
    if manifest:
        paths += [manifest, manifest.parent / "copper.lock"]
    paths += [Path(fp.metadata["source_path"]) for fp in board.footprints.values() if fp.metadata.get("source_path")]
    from ..macro_bindings import source_macro_input_paths
    paths += list(source_macro_input_paths(board))
    workspace = SourceWorkspace(source, design, build, input_paths=tuple(paths), source_reader=reader)
    return EditorSession(board, source, workspace=workspace, overlay=overlay)

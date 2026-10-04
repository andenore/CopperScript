"""Generic locked/offline editor physicalization shared by document hosts."""
from pathlib import Path

from ..compiler import compile_design_source
from ..erc import check, has_errors
from ..footprints import FootprintResolver
from ..hard_macros import apply_hard_macro_scene
from ..packages import find_manifest
from ..physicalize import PrototypePhysicalOptions, prototype_physicalize, resolved_physicalize
from ..placement_templates import apply_placement_templates
from .session import EditorSession
from .source import SourceSnapshot
from .transactions import SourceWorkspace


def document_session(source, reader, *, footprint_roots=(), layers=2, fab_profile="generic",
                     templates=None, macros=(), allow_proxy=False):
    source = Path(source).resolve()
    design = compile_design_source(SourceSnapshot(reader(), str(source)).text, str(source), locked=True, offline=True)
    diagnostics = check(design.electrical)
    if has_errors(diagnostics):
        raise ValueError("source fails ERC: " + "; ".join(map(str, diagnostics)))
    options = PrototypePhysicalOptions(copper_layers=layers, fabrication_profile=fab_profile)

    def build(candidate):
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
    workspace = SourceWorkspace(source, design, build, input_paths=tuple(paths), source_reader=reader)
    return EditorSession(board, source, workspace=workspace)

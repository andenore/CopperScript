"""Initial-open dependency preparation, separate from offline editing."""
from pathlib import Path

from ..packages import prepare_dependencies
from ..parser import parse
from .source import SourceSnapshot


def prepare_editor_dependencies(source, text=None, *, offline=False, locked=False,
                                progress=None, **settings):
    """Prepare explicit manifest requirements for any Copper document kind.

    Whole-module inventories include source packages, footprints and macro assets.
    No source is rewritten and no manifest/version is inferred from an URL. The
    caller must lower the selected document with locked/offline resolution after
    this succeeds. Syntax is checked before fetching. Unsaved edits never alter
    dependency selection, which remains explicit in the manifest.
    """
    source = Path(source)
    snapshot = SourceSnapshot(source.read_bytes() if text is None else
                              text.encode("utf-8") if isinstance(text, str) else text, str(source))
    document = parse(snapshot.text, str(source))
    if document.kind not in {"board", "macro", "board_profile"}:
        raise ValueError("physical editor supports board, macro and board_profile documents")
    return prepare_dependencies(Path(source), offline=offline, locked=locked, progress=progress)

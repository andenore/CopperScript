"""Load CopperScript source files into the typed board IR."""

from __future__ import annotations

from pathlib import Path

from .compiler import compile_design_file
from .model import Board
from .design import Design
from .syntax import CopperScriptError


class BoardLoadError(Exception):
    pass


def load_board(
    path: str | Path, *, locked: bool = False, offline: bool = False
) -> Board:
    return load_design(path, locked=locked, offline=offline).electrical


def load_design(path: str | Path, *, locked: bool = False, offline: bool = False) -> Design:
    source_path = Path(path).resolve()
    if not source_path.is_file():
        raise BoardLoadError(f"board file does not exist: {source_path}")
    if source_path.suffix != ".copper":
        raise BoardLoadError(f"unsupported source format {source_path.suffix!r}; expected .copper")
    try:
        return compile_design_file(source_path, locked=locked, offline=offline)
    except CopperScriptError as exc:
        raise BoardLoadError(str(exc)) from exc

"""Import external PCB library formats into CopperScript physical IR."""

from .kicad_mod import (
    FootprintImportResult,
    KiCadModImportError,
    load_kicad_mod,
    parse_kicad_mod,
)

__all__ = [
    "FootprintImportResult",
    "KiCadModImportError",
    "load_kicad_mod",
    "parse_kicad_mod",
]

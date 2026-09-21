"""Resolve external footprint references into normalized physical IR.

Resolution is intentionally explicit and deterministic.  CopperScript does
not inspect a user's KiCad installation or silently select the first matching
system footprint.  Direct ``.kicad_mod`` paths are relative to the board
source directory; KiCad ``Library:Footprint`` identifiers are searched only in
the roots supplied by the caller.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from pathlib import Path

from .importers import FootprintImportResult, KiCadModImportError, load_kicad_mod


class FootprintResolutionError(ValueError):
    """A footprint reference cannot be resolved safely and unambiguously."""


@dataclass(frozen=True, slots=True)
class FootprintResolver:
    """Resolve CopperScript footprint references from explicit local roots."""

    base_directory: Path
    search_roots: tuple[Path, ...] = ()
    strict: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "base_directory", self.base_directory.resolve())
        object.__setattr__(
            self,
            "search_roots",
            tuple(Path(root).resolve() for root in self.search_roots),
        )

    def resolve(self, reference: str) -> FootprintImportResult:
        """Load one reference and preserve the reference as its physical ID."""

        if not reference or "\x00" in reference:
            raise FootprintResolutionError("footprint reference cannot be empty")
        candidates = self._candidate_paths(reference)
        matches = tuple(path for path in candidates if path.is_file())
        if not matches:
            searched = ", ".join(str(path) for path in candidates[:6])
            suffix = f"; searched {searched}" if searched else ""
            raise FootprintResolutionError(
                f"cannot resolve footprint {reference!r}{suffix}"
            )
        if len(matches) > 1:
            joined = ", ".join(str(path) for path in matches)
            raise FootprintResolutionError(
                f"footprint {reference!r} is ambiguous: {joined}"
            )

        path = matches[0]
        try:
            imported = load_kicad_mod(path, strict=self.strict)
        except KiCadModImportError as exc:
            raise FootprintResolutionError(str(exc)) from exc

        expected_name = _reference_name(reference)
        if imported.footprint.name != expected_name:
            raise FootprintResolutionError(
                f"footprint reference {reference!r} resolved to {path}, but the file "
                f"declares {imported.footprint.name!r}; expected {expected_name!r}"
            )

        metadata = dict(imported.footprint.metadata)
        metadata.update(
            {
                "resolved_reference": reference,
                "source_footprint_name": imported.footprint.name,
            }
        )
        footprint = replace(
            imported.footprint,
            name=reference,
            source_library_id=reference,
            metadata=metadata,
        )
        return FootprintImportResult(
            footprint=footprint,
            warnings=imported.warnings,
            source_version=imported.source_version,
        )

    def _candidate_paths(self, reference: str) -> tuple[Path, ...]:
        roots = (self.base_directory, *self.search_roots)
        reference_path = Path(reference)
        candidates: list[Path] = []

        if reference_path.is_absolute():
            if reference_path.suffix.casefold() != ".kicad_mod":
                raise FootprintResolutionError(
                    "absolute footprint references must name a .kicad_mod file"
                )
            candidates.append(reference_path)
        elif (
            reference_path.suffix.casefold() == ".kicad_mod"
            or "/" in reference
            or "\\" in reference
        ):
            if reference_path.suffix.casefold() != ".kicad_mod":
                raise FootprintResolutionError(
                    "path-like footprint references must name a .kicad_mod file"
                )
            candidates.extend(root / reference_path for root in roots)
        elif ":" in reference:
            library, name = reference.split(":", 1)
            if not library or not name or ":" in name:
                raise FootprintResolutionError(
                    f"invalid KiCad footprint identifier {reference!r}"
                )
            for root in roots:
                candidates.append(root / f"{library}.pretty" / f"{name}.kicad_mod")
                candidates.append(root / library / f"{name}.kicad_mod")
        else:
            for root in roots:
                candidates.append(root / f"{reference}.kicad_mod")
                if root.is_dir():
                    candidates.extend(
                        directory / f"{reference}.kicad_mod"
                        for directory in sorted(root.glob("*.pretty"))
                        if directory.is_dir()
                    )

        unique: dict[str, Path] = {}
        for candidate in candidates:
            resolved = candidate.resolve()
            unique.setdefault(str(resolved).casefold(), resolved)
        return tuple(unique.values())


def _reference_name(reference: str) -> str:
    if ":" in reference and not Path(reference).is_absolute():
        return reference.split(":", 1)[1]
    path = Path(reference)
    return path.stem if path.suffix.casefold() == ".kicad_mod" else reference

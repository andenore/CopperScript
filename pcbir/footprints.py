"""Resolve external footprint references into normalized physical IR.

Managed URLs and manifest library bindings use the existing package cache and
inventory lock. Explicit local files/roots remain available for development;
no system installation or floating remote revision is selected implicitly.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from pathlib import Path

from .importers import FootprintImportResult, KiCadModImportError, load_kicad_mod
from .packages import PackageResolver, ResolvedAsset, ResolvedPackage, find_manifest, read_manifest
from .syntax import CopperScriptError, SourceLocation


class FootprintResolutionError(ValueError):
    """A footprint reference cannot be resolved safely and unambiguously."""


@dataclass(frozen=True, slots=True)
class FootprintResolver:
    """Resolve managed dependencies and explicit local footprint sources."""

    base_directory: Path
    search_roots: tuple[Path, ...] = ()
    strict: bool = False
    locked: bool = False
    offline: bool = False
    _packages: PackageResolver | None = field(default=None, init=False, repr=False, compare=False)
    _verified_modules: dict[str, ResolvedPackage] = field(default_factory=dict, init=False, repr=False, compare=False)

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
        try:
            target = self.managed_reference(reference)
            asset = None
            resolution = "local"
            if target:
                candidates = (() if reference.startswith(("github.com/", "gitlab.com/", "https://")) or self.locked else
                              self._candidate_paths(reference, roots=self.search_roots))
                matches = tuple(p for p in candidates if p.is_file())
                if matches:
                    resolution = "local_override"
                else:
                    asset = self._asset(target)
                    candidates = matches = (asset.path,)
                    resolution = "managed"
            else:
                candidates = self._candidate_paths(reference)
                matches = tuple(path for path in candidates if path.is_file())
                if not matches:
                    manifest_path = find_manifest(self.base_directory)
                    if manifest_path:
                        packages = self._package_resolver()
                        for module in packages.manifest.requirements:
                            package = self._module(module)
                            root = package.directory / "footprints"
                            managed = self._candidate_paths(reference, roots=(root,))
                            matches += tuple(p for p in managed if p.is_file() and p not in matches)
                        if len(matches) == 1:
                            path = matches[0]
                            owner = next((p for p in self._verified_modules.values() if path.is_relative_to(p.directory)), None)
                            if owner:
                                asset = self._asset(owner.module_path + "/" + path.relative_to(owner.directory).as_posix())
                                resolution = "managed"
        except (CopperScriptError, OSError) as exc:
            raise FootprintResolutionError(str(exc)) from exc
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
        if asset and imported.footprint.metadata.get("source_sha256") != asset.sha256:
            raise FootprintResolutionError(f"footprint changed after module verification: {reference!r}")

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
                "resolution": resolution,
            }
        )
        if target:
            metadata["managed_reference"] = target
        if asset:
            metadata.update({"source_asset": asset.identity, "module_path": asset.module_path,
                             "module_version": asset.version, "module_checksum": asset.checksum})
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

    def managed_reference(self, reference: str) -> str | None:
        """Return a managed asset identity without fetching any dependency."""
        if "://" in reference and not reference.startswith("https://"):
            raise FootprintResolutionError("managed footprint URLs must use HTTPS")
        if reference.startswith(("github.com/", "gitlab.com/", "https://")):
            if not reference.lower().endswith(".kicad_mod"):
                raise FootprintResolutionError("managed footprint references must name a .kicad_mod file")
            return reference.removeprefix("https://")
        if ":" in reference and not Path(reference).is_absolute() and "/" not in reference and "\\" not in reference:
            library, name = reference.split(":", 1)
            if not library or not name or ":" in name or name in {".", ".."}:
                raise FootprintResolutionError(f"invalid KiCad footprint identifier {reference!r}")
            manifest_path = find_manifest(self.base_directory)
            if manifest_path:
                binding = read_manifest(manifest_path).footprint_libraries.get(library)
                if binding:
                    return f"{binding}/{name}.kicad_mod"
        return None

    def _package_resolver(self) -> PackageResolver:
        if self._packages is None:
            resolver = PackageResolver.for_source(self.base_directory, SourceLocation(str(self.base_directory), 0, 1, 1),
                                                  locked=self.locked, offline=self.offline)
            object.__setattr__(self, "_packages", resolver)
        return self._packages

    def _module(self, module: str) -> ResolvedPackage:
        if module not in self._verified_modules:
            self._verified_modules[module] = self._package_resolver().resolve(module, SourceLocation(str(self.base_directory), 0, 1, 1))
        return self._verified_modules[module]

    def _asset(self, reference: str) -> ResolvedAsset:
        return self._package_resolver().resolve_asset(reference, SourceLocation(str(self.base_directory), 0, 1, 1), self._verified_modules)

    def _candidate_paths(self, reference: str, *, roots: tuple[Path, ...] | None = None) -> tuple[Path, ...]:
        roots = (self.base_directory, *self.search_roots) if roots is None else roots
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
    if reference.startswith(("github.com/", "gitlab.com/", "https://")):
        return reference.rsplit("/", 1)[-1][:-len(".kicad_mod")]
    if ":" in reference and not Path(reference).is_absolute():
        return reference.split(":", 1)[1]
    path = Path(reference)
    return path.stem if path.suffix.casefold() == ".kicad_mod" else reference


def prepare_footprint_dependencies(board, resolver: FootprintResolver) -> tuple[str, ...]:
    """Prepare selected managed geometry without requiring unrelated local files."""
    from .elaborate import elaborate
    flat = elaborate(board)
    references = sorted({c.footprint or flat.library[c.part].footprints[0]
                         for c in flat.components if c.footprint or flat.library[c.part].footprints})
    managed = []
    for reference in references:
        if resolver.managed_reference(reference):
            resolver.resolve(reference)
            managed.append(reference)
    return tuple(managed)

"""Go-style package manifests and deterministic dependency resolution.

Package import paths are stable source identities. Versions and local
development replacements live in ``copper.mod``; complete content inventories
are recorded in ``copper.lock``. Remote modules are fetched as Git repositories
into a project-local cache and CopperScript never executes package code.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

from .syntax import CopperScriptError, SourceLocation


@dataclass(frozen=True, slots=True)
class ModuleManifest:
    path: str
    requirements: dict[str, str]
    replacements: dict[str, str]
    filename: Path

    @property
    def root(self) -> Path:
        return self.filename.parent


@dataclass(frozen=True, slots=True)
class ResolvedPackage:
    import_path: str
    module_path: str
    version: str
    directory: Path
    checksum: str


@dataclass(frozen=True, slots=True)
class LockedFile:
    path: str
    size: int
    sha256: str


@dataclass(frozen=True, slots=True)
class LockedModule:
    module_path: str
    version: str
    source: str
    checksum: str
    files: tuple[LockedFile, ...]


LOCK_SCHEMA = "copperscript.package-lock/v1"


def find_manifest(start: Path) -> Path | None:
    """Return the nearest ``copper.mod`` at or above *start*."""

    directory = start if start.is_dir() else start.parent
    for candidate_root in (directory, *directory.parents):
        candidate = candidate_root / "copper.mod"
        if candidate.is_file():
            return candidate
    return None


def read_manifest(path: Path) -> ModuleManifest:
    requirements: dict[str, str] = {}
    replacements: dict[str, str] = {}
    module_path: str | None = None
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        _manifest_error(path, 1, f"cannot read manifest: {exc}")

    for line_number, raw_line in enumerate(lines, 1):
        line = raw_line.split("//", 1)[0].split("#", 1)[0].strip()
        if not line:
            continue
        fields = line.split()
        if fields[0] == "module" and len(fields) == 2:
            if module_path is not None:
                _manifest_error(path, line_number, "duplicate module directive")
            module_path = fields[1]
        elif fields[0] == "require" and len(fields) == 3:
            if fields[1] in requirements:
                _manifest_error(path, line_number, f"duplicate requirement {fields[1]!r}")
            requirements[fields[1]] = fields[2]
        elif fields[0] == "replace" and len(fields) == 4 and fields[2] == "=>":
            if fields[1] in replacements:
                _manifest_error(path, line_number, f"duplicate replacement {fields[1]!r}")
            replacements[fields[1]] = fields[3]
        else:
            _manifest_error(path, line_number, f"invalid directive: {raw_line.strip()}")

    if module_path is None:
        _manifest_error(path, 1, "manifest must declare its module path")
    unknown_replacements = sorted(set(replacements) - set(requirements))
    if unknown_replacements:
        _manifest_error(
            path,
            1,
            f"replacement has no matching requirement: {unknown_replacements[0]!r}",
        )
    return ModuleManifest(module_path, requirements, replacements, path.resolve())


class PackageResolver:
    """Resolve package paths declared by one project manifest."""

    def __init__(
        self,
        manifest: ModuleManifest,
        *,
        locked: bool = False,
        offline: bool = False,
    ):
        self.manifest = manifest
        self.cache_root = manifest.root / ".copper-cache" / "pkg"
        self.locked = locked
        self.offline = offline
        self.lock_path = manifest.root / "copper.lock"
        self._lock_entries = _read_lock(self.lock_path)

    @classmethod
    def for_source(
        cls,
        source: Path,
        location: SourceLocation,
        *,
        locked: bool = False,
        offline: bool = False,
    ) -> "PackageResolver":
        manifest_path = find_manifest(source)
        if manifest_path is None:
            raise CopperScriptError(
                "PKG001",
                "package imports require a copper.mod in this directory or a parent",
                location,
            )
        return cls(read_manifest(manifest_path), locked=locked, offline=offline)

    def resolve(self, import_path: str, location: SourceLocation) -> ResolvedPackage:
        module_path = self._matching_module(import_path)
        if module_path is None:
            raise CopperScriptError(
                "PKG002",
                f"no requirement in copper.mod provides package {import_path!r}",
                location,
            )
        version = self.manifest.requirements[module_path]
        replacement = self.manifest.replacements.get(module_path)
        key = (module_path, version)
        if self.locked and key not in self._lock_entries:
            raise CopperScriptError(
                "PKG010",
                f"locked resolution requires {module_path}@{version} in copper.lock",
                location,
            )
        is_local_replacement = replacement is not None
        if is_local_replacement:
            module_root = Path(replacement)
            if not module_root.is_absolute():
                module_root = self.manifest.root / module_root
            module_root = module_root.resolve()
            if not module_root.is_dir():
                raise CopperScriptError(
                    "PKG003",
                    f"replacement directory does not exist: {module_root}",
                    location,
                )
        else:
            module_root = self._fetch_git_module(module_path, version, location)

        suffix = import_path[len(module_path) :].lstrip("/")
        package_dir = (module_root / suffix).resolve()
        if not package_dir.is_dir() or not package_dir.is_relative_to(module_root):
            raise CopperScriptError(
                "PKG004",
                f"package {import_path!r} does not exist in module {module_path!r}",
                location,
            )
        try:
            files = _module_inventory(module_root)
        except (OSError, ValueError) as exc:
            raise CopperScriptError(
                "PKG012", f"cannot inventory {module_path}@{version}: {exc}", location
            ) from exc
        checksum = _inventory_checksum(files)
        source = f"replace:{replacement}" if replacement is not None else f"git:https://{module_path}.git"
        self._verify_or_record_lock(
            LockedModule(module_path, version, source, checksum, files), location
        )
        return ResolvedPackage(import_path, module_path, version, package_dir, checksum)

    def _matching_module(self, import_path: str) -> str | None:
        matches = [
            module
            for module in self.manifest.requirements
            if import_path == module or import_path.startswith(f"{module}/")
        ]
        return max(matches, key=len) if matches else None

    def _fetch_git_module(
        self, module_path: str, version: str, location: SourceLocation
    ) -> Path:
        if not module_path.startswith("github.com/"):
            raise CopperScriptError(
                "PKG005",
                "v0.1 remote fetching supports github.com module paths only; use replace for other sources",
                location,
            )
        if not re.fullmatch(r"github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", module_path):
            raise CopperScriptError("PKG005", "invalid GitHub module path", location)
        if not version or version.startswith("-"):
            raise CopperScriptError("PKG006", "invalid Git revision", location)
        cache_key = hashlib.sha256(f"{module_path}@{version}".encode()).hexdigest()[:20]
        destination = self.cache_root / cache_key
        if destination.is_symlink():
            raise CopperScriptError("PKG012", "package cache roots may not be symlinks", location)
        if destination.is_dir():
            return destination.resolve()
        if self.offline:
            raise CopperScriptError(
                "PKG011",
                f"offline resolution cache miss for {module_path}@{version}",
                location,
            )
        self.cache_root.mkdir(parents=True, exist_ok=True)
        temporary = Path(tempfile.mkdtemp(prefix=f"{cache_key}-", dir=self.cache_root))
        try:
            # Exact commit requirements are immutable and cannot be passed to
            # clone --branch. Disable host checkout conversions: the lock pins
            # repository bytes, not a platform-specific CRLF representation.
            completed = subprocess.run(
                [
                    "git",
                    "-c", "core.autocrlf=false",
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    *([] if re.fullmatch(r"[0-9a-fA-F]{40}", version) else ["--branch", version]),
                    f"https://{module_path}.git",
                    str(temporary),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            if completed.returncode != 0:
                detail = completed.stderr.strip() or "git clone failed"
                raise CopperScriptError("PKG006", detail, location)
            if re.fullmatch(r"[0-9a-fA-F]{40}", version):
                for arguments in (("fetch", "--quiet", "--depth", "1", "origin", version),
                                  ("checkout", "--quiet", "--detach", version)):
                    completed = subprocess.run(["git", "-c", "core.autocrlf=false",
                        "-C", str(temporary), *arguments], capture_output=True, text=True)
                    if completed.returncode:
                        raise CopperScriptError("PKG006", completed.stderr.strip(), location)
            temporary.replace(destination)
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)
        return destination.resolve()

    def _verify_or_record_lock(self, entry: LockedModule, location: SourceLocation) -> None:
        key = (entry.module_path, entry.version)
        expected = self._lock_entries.get(key)
        declared = {(module, version) for module, version in self.manifest.requirements.items()}
        if expected == entry and (self.locked or set(self._lock_entries).issubset(declared)):
            return
        if self.locked:
            expected_digest = expected.checksum if expected else "missing"
            raise CopperScriptError("PKG008", f"lock mismatch for {entry.module_path}@{entry.version}: "
                f"expected {expected_digest}, got {entry.checksum}", location)
        self._lock_entries[key] = entry
        self._lock_entries = {k:v for k,v in self._lock_entries.items() if k in declared}
        try:
            _write_lock(self.lock_path, self._lock_entries)
        except OSError as exc:
            raise CopperScriptError("PKG007", f"cannot write copper.lock: {exc}", location) from exc


def resolve_module_asset(source: Path, import_path: str, *, locked: bool = True,
                         offline: bool = False) -> Path:
    """Resolve data through the same revision/inventory lock as .copper parts.

    Accept Go-style paths or https GitHub paths, never execute downloaded data.
    Paths must stay within the selected module, and symlinks are rejected by
    the normal full inventory validation.
    """
    import_path = import_path.removeprefix("https://")
    if "\\" in import_path or any(p in {".", "..", ""} for p in import_path.split("/")):
        raise CopperScriptError("PKG004", "invalid module asset path", SourceLocation(str(source),0,1,1))
    location = SourceLocation(str(source),0,1,1)
    resolver = PackageResolver.for_source(Path(source), location, locked=locked, offline=offline)
    parent, _, name = import_path.rpartition("/")
    package = resolver.resolve(parent, location)
    asset = (package.directory / name).resolve()
    if not asset.is_relative_to(package.directory) or not asset.is_file():
        raise CopperScriptError("PKG004", f"module asset does not exist: {import_path}", location)
    return asset


def resolve_module_root(source: Path, module_path: str, *, locked: bool = True,
                        offline: bool = False) -> Path:
    """Public managed-cache lookup; callers never need a sibling checkout."""
    location = SourceLocation(str(source),0,1,1)
    return PackageResolver.for_source(Path(source),location,locked=locked,offline=offline).resolve(
        module_path.removeprefix("https://").removesuffix(".git"), location).directory

_LOCKED_SUFFIXES = frozenset({
    ".copper", ".kicad_mod", ".step", ".stp", ".wrl", ".json", ".csv"
})
_IGNORED_PARTS = frozenset({".git", ".copper-cache", "__pycache__", ".pytest_cache"})


def _module_inventory(root: Path) -> tuple[LockedFile, ...]:
    inventory: list[LockedFile] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        relative = path.relative_to(root)
        if any(part in _IGNORED_PARTS for part in relative.parts):
            continue
        if path.is_symlink():
            raise ValueError(f"package assets may not be symlinks: {relative.as_posix()}")
        if not path.is_file():
            continue
        if path.name != "copper.mod" and path.suffix.casefold() not in _LOCKED_SUFFIXES:
            continue
        data = path.read_bytes()
        inventory.append(
            LockedFile(relative.as_posix(), len(data), hashlib.sha256(data).hexdigest())
        )
    return tuple(inventory)


def _inventory_checksum(files: tuple[LockedFile, ...]) -> str:
    digest = hashlib.sha256()
    for item in files:
        digest.update(item.path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(str(item.size).encode("ascii"))
        digest.update(b"\0")
        digest.update(item.sha256.encode("ascii"))
        digest.update(b"\0")
    return f"sha256:{digest.hexdigest()}"


def _read_lock(path: Path) -> dict[tuple[str, str], LockedModule]:
    if not path.exists():
        return {}
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
        if document.get("schema") != LOCK_SCHEMA or not isinstance(document.get("modules"), list):
            raise ValueError("unsupported schema")
        entries: dict[tuple[str, str], LockedModule] = {}
        for raw in document["modules"]:
            files = tuple(
                LockedFile(item["path"], int(item["size"]), item["sha256"])
                for item in raw["files"]
            )
            entry = LockedModule(
                raw["module"], raw["version"], raw["source"], raw["checksum"], files
            )
            key = (entry.module_path, entry.version)
            if key in entries:
                raise ValueError(f"duplicate module {entry.module_path}@{entry.version}")
            entries[key] = entry
        return entries
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
        raise CopperScriptError(
            "PKG007", f"cannot read copper.lock: {exc}", SourceLocation(str(path), 0, 1, 1)
        ) from exc


def _write_lock(path: Path, entries: dict[tuple[str, str], LockedModule]) -> None:
    modules = []
    for key in sorted(entries):
        entry = entries[key]
        modules.append({
            "module": entry.module_path,
            "version": entry.version,
            "source": entry.source,
            "checksum": entry.checksum,
            "files": [
                {"path": item.path, "size": item.size, "sha256": item.sha256}
                for item in entry.files
            ],
        })
    content = json.dumps({"schema": LOCK_SCHEMA, "modules": modules}, indent=2, sort_keys=True)
    path.write_text(content + "\n", encoding="utf-8")


def _manifest_error(path: Path, line: int, message: str):
    raise CopperScriptError("PKG009", message, SourceLocation(str(path), 0, line, 1))

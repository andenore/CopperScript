"""Go-style package manifests and deterministic dependency resolution.

Package import paths are stable source identities. Versions and local
development replacements live in ``copper.mod``; content hashes are recorded
in ``copper.sum``. Remote modules are fetched as Git repositories into a
project-local cache and CopperScript never executes package code.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
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

    def __init__(self, manifest: ModuleManifest):
        self.manifest = manifest
        self.cache_root = manifest.root / ".copper-cache" / "pkg"

    @classmethod
    def for_source(cls, source: Path, location: SourceLocation) -> "PackageResolver":
        manifest_path = find_manifest(source)
        if manifest_path is None:
            raise CopperScriptError(
                "PKG001",
                "package imports require a copper.mod in this directory or a parent",
                location,
            )
        return cls(read_manifest(manifest_path))

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
        checksum = _module_checksum(module_root)
        # Match Go's development ergonomics: a local replacement is mutable
        # source and is not locked in the sum file. Its current hash is still
        # retained in IR provenance. Downloaded module content is immutable.
        if not is_local_replacement:
            self._verify_or_record_sum(module_path, version, checksum, location)
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
        cache_key = hashlib.sha256(f"{module_path}@{version}".encode()).hexdigest()[:20]
        destination = self.cache_root / cache_key
        if destination.is_dir():
            return destination.resolve()
        self.cache_root.mkdir(parents=True, exist_ok=True)
        temporary = Path(tempfile.mkdtemp(prefix=f"{cache_key}-", dir=self.cache_root))
        try:
            completed = subprocess.run(
                [
                    "git",
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--branch",
                    version,
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
            temporary.replace(destination)
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)
        return destination.resolve()

    def _verify_or_record_sum(
        self,
        module_path: str,
        version: str,
        checksum: str,
        location: SourceLocation,
    ) -> None:
        sum_path = self.manifest.root / "copper.sum"
        entries: dict[tuple[str, str], str] = {}
        if sum_path.exists():
            try:
                lines = sum_path.read_text(encoding="utf-8").splitlines()
            except OSError as exc:
                raise CopperScriptError("PKG007", f"cannot read copper.sum: {exc}", location) from exc
            for line_number, line in enumerate(lines, 1):
                if not line.strip():
                    continue
                fields = line.split()
                if len(fields) != 3:
                    raise CopperScriptError(
                        "PKG007", f"invalid copper.sum line {line_number}", location
                    )
                entries[(fields[0], fields[1])] = fields[2]

        key = (module_path, version)
        expected = entries.get(key)
        if expected is not None and expected != checksum:
            raise CopperScriptError(
                "PKG008",
                f"checksum mismatch for {module_path}@{version}: expected {expected}, got {checksum}",
                location,
            )
        if expected is None:
            entries[key] = checksum
            content = "".join(
                f"{module} {entry_version} {entry_checksum}\n"
                for (module, entry_version), entry_checksum in sorted(entries.items())
            )
            try:
                sum_path.write_text(content, encoding="utf-8")
            except OSError as exc:
                raise CopperScriptError("PKG007", f"cannot write copper.sum: {exc}", location) from exc


def _module_checksum(root: Path) -> str:
    digest = hashlib.sha256()
    files = sorted(
        (
            path
            for path in root.rglob("*")
            if path.is_file()
            and ".git" not in path.relative_to(root).parts
            and ".copper-cache" not in path.relative_to(root).parts
            and (path.suffix == ".copper" or path.name == "copper.mod")
        ),
        key=lambda path: path.relative_to(root).as_posix(),
    )
    for path in files:
        relative = path.relative_to(root).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return f"sha256:{digest.hexdigest()}"


def _manifest_error(path: Path, line: int, message: str):
    raise CopperScriptError("PKG009", message, SourceLocation(str(path), 0, line, 1))

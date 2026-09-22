"""Fail-closed independent CAM qualification contracts and evidence."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from hashlib import sha256
from pathlib import Path
from typing import Protocol


class CamGateStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"
    INCOMPLETE = "incomplete"
    REVIEW_REQUIRED = "review_required"


@dataclass(frozen=True, slots=True)
class ToolIdentity:
    name: str
    version: str
    executable_sha256: str

    def __post_init__(self) -> None:
        if not self.name or not self.version or len(self.executable_sha256) != 64:
            raise ValueError("CAM tools require an exact name, version, and SHA-256")


@dataclass(frozen=True, slots=True)
class CamQualificationProfile:
    id: str
    gerber_spec_revision: str
    xnc_spec_revision: str
    required_tool_identities: tuple[ToolIdentity, ...]
    maximum_file_bytes: int = 100_000_000


@dataclass(frozen=True, slots=True)
class NormalizedCamLayer:
    file_function: str
    file_polarity: str
    units: str
    bounds_nm: tuple[int, int, int, int]
    topology_digest: str


class CamToolAdapter(Protocol):
    identity: ToolIdentity

    def parse_gerber(self, path: Path) -> NormalizedCamLayer: ...


@dataclass(frozen=True, slots=True)
class CamQualificationEvidence:
    status: CamGateStatus
    artifact_hashes: tuple[tuple[str, str], ...]
    tool_identities: tuple[ToolIdentity, ...]
    findings: tuple[str, ...]


def qualify_cam_artifacts(directory: Path, profile: CamQualificationProfile,
                          adapters: tuple[CamToolAdapter, ...]) -> CamQualificationEvidence:
    """Check immutable bytes with every pinned independent adapter.

    Geometry comparison is adapter-neutral: each parser must agree on function,
    polarity, units, bounds, and a topology digest.  Unsupported or missing
    required tools produce INCOMPLETE, never a pass.
    """
    findings: list[str] = []
    paths = tuple(sorted((p for p in directory.rglob("*") if p.is_file()),
                         key=lambda p: p.relative_to(directory).as_posix().casefold()))
    relative = [p.relative_to(directory).as_posix() for p in paths]
    if len({name.casefold() for name in relative}) != len(relative):
        return CamQualificationEvidence(CamGateStatus.FAIL, (), (), ("case-colliding artifact paths",))
    hashes: list[tuple[str, str]] = []
    for path, name in zip(paths, relative):
        if path.is_symlink():
            findings.append(f"symlink is not allowed: {name}")
            continue
        data = path.read_bytes()
        if len(data) > profile.maximum_file_bytes:
            findings.append(f"artifact exceeds size limit: {name}")
        hashes.append((name, sha256(data).hexdigest()))
    expected = {item: item for item in profile.required_tool_identities}
    actual = {adapter.identity: adapter for adapter in adapters}
    missing = tuple(sorted((item for item in expected if item not in actual), key=lambda i: (i.name, i.version)))
    if missing:
        findings.extend(f"required CAM tool unavailable: {item.name} {item.version}" for item in missing)
        return CamQualificationEvidence(CamGateStatus.INCOMPLETE, tuple(hashes), tuple(actual), tuple(findings))
    gerbers = tuple(path for path in paths if path.suffix.casefold() in {".gbr", ".ger"})
    if not gerbers:
        findings.append("no Gerber artwork")
        return CamQualificationEvidence(CamGateStatus.FAIL, tuple(hashes), tuple(actual), tuple(findings))
    for path in gerbers:
        parsed: list[NormalizedCamLayer] = []
        for identity in profile.required_tool_identities:
            try:
                parsed.append(actual[identity].parse_gerber(path))
            except Exception as exc:  # adapters are an isolation boundary
                findings.append(f"{identity.name} could not parse {path.name}: {exc}")
        if parsed and any(item != parsed[0] for item in parsed[1:]):
            findings.append(f"independent parser disagreement: {path.name}")
    status = CamGateStatus.PASS if not findings else CamGateStatus.FAIL
    return CamQualificationEvidence(status, tuple(hashes), profile.required_tool_identities, tuple(findings))

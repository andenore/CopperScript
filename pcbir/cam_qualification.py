"""Fail-closed independent CAM qualification contracts and evidence."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from hashlib import sha256
from pathlib import Path
from io import BytesIO
import re
import subprocess
import sys
import tempfile
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


@dataclass(frozen=True, slots=True)
class DrillHit:
    tool: str
    diameter_nm: int
    x_nm: int
    y_nm: int


@dataclass(frozen=True, slots=True)
class NormalizedDrillProgram:
    hits: tuple[DrillHit, ...]
    units: str


class PyGerberAdapter:
    """Pinned in-process adapter; use subprocess isolation in release runners."""

    def __init__(self) -> None:
        import pygerber
        self.identity = ToolIdentity("PyGerber", pygerber.__version__,
                                     sha256(Path(sys.executable).read_bytes()).hexdigest())

    def parse_gerber(self, path: Path) -> NormalizedCamLayer:
        from pygerber.gerberx3.api import v2
        parsed = v2.GerberFile.from_file(path).parse(on_parser_error=v2.OnParserErrorEnum.Raise)
        info = parsed.get_info()
        output = BytesIO()
        parsed.render_raster(output, dpmm=100, image_format=v2.ImageFormatEnum.PNG,
                             pixel_format=v2.PixelFormatEnum.RGBA)
        text = path.read_text(encoding="ascii", errors="strict")
        function = re.search(r"%TF\.FileFunction,([^*]+)\*%", text)
        polarity = re.search(r"%TF\.FilePolarity,([^*]+)\*%", text)
        unit = "mm" if "%MOMM*%" in text else "inch" if "%MOIN*%" in text else "unknown"
        scale = 1_000_000
        return NormalizedCamLayer(
            function.group(1) if function else "",
            polarity.group(1) if polarity else "",
            unit,
            tuple(round(float(value) * scale) for value in (
                info.min_x_mm, info.min_y_mm, info.max_x_mm, info.max_y_mm)),
            sha256(output.getvalue()).hexdigest(),
        )


class GerbvSubprocessAdapter:
    """Independent libgerbv CLI image oracle with exact executable identity."""

    def __init__(self, executable: Path, version: str):
        executable = executable.resolve(strict=True)
        self.executable = executable
        self.identity = ToolIdentity("libgerbv", version,
                                     sha256(executable.read_bytes()).hexdigest())

    def parse_gerber(self, path: Path) -> NormalizedCamLayer:
        from PIL import Image
        with tempfile.TemporaryDirectory(prefix="copper-gerbv-") as temporary:
            output = Path(temporary) / "layer.png"
            completed = subprocess.run(
                (str(self.executable), "-x", "png", "--dpi=2540", "-o", str(output), str(path)),
                text=True, capture_output=True, timeout=60, check=False,
            )
            if completed.returncode or not output.is_file():
                raise RuntimeError(completed.stderr.strip() or "gerbv render failed")
            image = Image.open(output).convert("RGBA")
            alpha = image.getchannel("A")
            bbox = alpha.getbbox()
            if bbox is None:
                bounds_nm = (0, 0, 0, 0)
            else:
                bounds_nm = tuple(value * 10_000 for value in bbox)
            topology = sha256(image.tobytes()).hexdigest()
        text = path.read_text(encoding="ascii", errors="strict")
        function = re.search(r"%TF\.FileFunction,([^*]+)\*%", text)
        polarity = re.search(r"%TF\.FilePolarity,([^*]+)\*%", text)
        unit = "mm" if "%MOMM*%" in text else "inch" if "%MOIN*%" in text else "unknown"
        return NormalizedCamLayer(function.group(1) if function else "",
                                  polarity.group(1) if polarity else "", unit,
                                  bounds_nm, topology)


def parse_xnc(path: Path) -> NormalizedDrillProgram:
    """Parse the strict decimal metric XNC subset emitted by the release profile."""
    text = path.read_text(encoding="ascii", errors="strict")
    if "M48" not in text or "M30" not in text or "METRIC" not in text:
        raise ValueError("XNC requires M48, explicit METRIC units, and M30")
    tools = {match.group(1): round(float(match.group(2)) * 1_000_000)
             for match in re.finditer(r"^T(\d+)C([0-9.]+)$", text, re.MULTILINE)}
    current: str | None = None
    hits: list[DrillHit] = []
    for line in text.splitlines():
        tool = re.fullmatch(r"T(\d+)", line)
        if tool:
            current = tool.group(1)
            if current not in tools:
                raise ValueError(f"XNC selects undefined tool T{current}")
            continue
        coordinate = re.fullmatch(r"X(-?[0-9.]+)Y(-?[0-9.]+)", line)
        if coordinate:
            if current is None:
                raise ValueError("XNC coordinate occurs before tool selection")
            hits.append(DrillHit(current, tools[current],
                                 round(float(coordinate.group(1)) * 1_000_000),
                                 round(float(coordinate.group(2)) * 1_000_000)))
    return NormalizedDrillProgram(tuple(sorted(hits, key=lambda item: (item.x_nm, item.y_nm,
                                                                        item.diameter_nm, item.tool))), "mm")


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

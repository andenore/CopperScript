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
from collections import Counter

from .physical import PadKind, PhysicalBoard
from .placement import transformed_local_point, transformed_pad_position


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
class CamCorpusCase:
    id: str
    path: Path
    expect_parse: bool
    file_function: str | None = None
    file_polarity: str | None = None
    units: str | None = None
    bounds_nm: tuple[int, int, int, int] | None = None
    bounds_tolerance_nm: int = 10_000


@dataclass(frozen=True, slots=True)
class CamMatrixCell:
    case_id: str
    tool: ToolIdentity
    passed: bool
    detail: str = ""


@dataclass(frozen=True, slots=True)
class CamQualificationMatrix:
    status: CamGateStatus
    corpus_hashes: tuple[tuple[str, str], ...]
    required_tools: tuple[ToolIdentity, ...]
    cells: tuple[CamMatrixCell, ...]
    findings: tuple[str, ...] = ()

    @property
    def passed(self) -> bool:
        return self.status is CamGateStatus.PASS


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
    plated: bool | None = None


@dataclass(frozen=True, slots=True)
class CamReconciliation:
    passed: bool
    findings: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class TestPoint:
    net: str
    component: str
    pad: str
    x_nm: int
    y_nm: int


@dataclass(frozen=True, slots=True)
class TestVia:
    net: str
    x_nm: int
    y_nm: int
    drill_nm: int


@dataclass(frozen=True, slots=True)
class NormalizedTestNet:
    points: tuple[TestPoint, ...]
    vias: tuple[TestVia, ...] = ()


class PyGerberAdapter:
    """Pinned in-process adapter; use subprocess isolation in release runners."""

    def __init__(self) -> None:
        import pygerber
        package_root = Path(pygerber.__file__).resolve().parent
        digest = sha256()
        digest.update(Path(sys.executable).read_bytes())
        for source in sorted(package_root.rglob("*.py"),
                             key=lambda item: item.relative_to(package_root).as_posix()):
            digest.update(source.relative_to(package_root).as_posix().encode("utf-8"))
            digest.update(b"\0")
            digest.update(source.read_bytes())
            digest.update(b"\0")
        self.identity = ToolIdentity("PyGerber", pygerber.__version__, digest.hexdigest())

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
    """Independent libgerbv parse/re-export oracle with exact CLI identity.

    Gerbv re-exports RS-274X, then the same normalizer used for the original
    Gerber renders both files. This avoids comparing unlike PNG encodings or
    treating a raster's pixel origin as a physical coordinate origin.
    """

    def __init__(self, executable: Path, version: str):
        executable = executable.resolve(strict=True)
        self.executable = executable
        self.identity = ToolIdentity("libgerbv", version,
                                     sha256(executable.read_bytes()).hexdigest())

    def parse_gerber(self, path: Path) -> NormalizedCamLayer:
        with tempfile.TemporaryDirectory(prefix="copper-gerbv-") as temporary:
            output = Path(temporary) / "layer.gbr"
            completed = subprocess.run(
                (str(self.executable), "-x", "rs274x", "-o", str(output), str(path)),
                text=True, capture_output=True, timeout=60, check=False,
            )
            if completed.returncode or not output.is_file():
                raise RuntimeError(completed.stderr.strip() or "gerbv re-export failed")
            normalized = PyGerberAdapter().parse_gerber(output)
        text = path.read_text(encoding="ascii", errors="strict")
        function = re.search(r"%TF\.FileFunction,([^*]+)\*%", text)
        polarity = re.search(r"%TF\.FilePolarity,([^*]+)\*%", text)
        unit = "mm" if "%MOMM*%" in text else "inch" if "%MOIN*%" in text else "unknown"
        return NormalizedCamLayer(function.group(1) if function else "",
                                  polarity.group(1) if polarity else "", unit,
                                  normalized.bounds_nm, normalized.topology_digest)


def run_cam_qualification_matrix(
    profile: CamQualificationProfile,
    cases: tuple[CamCorpusCase, ...],
    adapters: tuple[CamToolAdapter, ...],
) -> CamQualificationMatrix:
    """Run a deterministic positive/negative corpus for every pinned adapter."""

    findings: list[str] = []
    if not cases or not any(case.expect_parse for case in cases) or not any(
        not case.expect_parse for case in cases
    ):
        findings.append("qualification corpus requires positive and negative cases")
    if len({case.id for case in cases}) != len(cases):
        findings.append("qualification corpus case IDs must be unique")
    if len({identity.name for identity in profile.required_tool_identities}) < 2:
        findings.append("qualification requires at least two distinct CAM tools")
    corpus_hashes: list[tuple[str, str]] = []
    for case in sorted(cases, key=lambda item: item.id):
        if not case.path.is_file() or case.path.is_symlink():
            findings.append(f"corpus case unavailable or symlinked: {case.id}")
            continue
        if case.path.stat().st_size > profile.maximum_file_bytes:
            findings.append(f"corpus case exceeds size limit: {case.id}")
            continue
        data = case.path.read_bytes()
        corpus_hashes.append((case.id, sha256(data).hexdigest()))
    expected = set(profile.required_tool_identities)
    present = {adapter.identity for adapter in adapters}
    missing = expected - present
    for identity in sorted(missing, key=lambda item: (item.name, item.version)):
        findings.append(f"required CAM tool unavailable: {identity.name} {identity.version}")
    cells: list[CamMatrixCell] = []
    parsed_by_case: dict[str, list[tuple[ToolIdentity, NormalizedCamLayer]]] = {}
    for adapter in sorted(
        (item for item in adapters if item.identity in expected),
        key=lambda item: (
            item.identity.name,
            item.identity.version,
            item.identity.executable_sha256,
        ),
    ):
        for case in sorted(cases, key=lambda item: item.id):
            if case.id not in dict(corpus_hashes):
                continue
            try:
                parsed = adapter.parse_gerber(case.path)
            except Exception as exc:  # adapter isolation boundary
                if case.expect_parse:
                    cells.append(
                        CamMatrixCell(case.id, adapter.identity, False, f"unexpected rejection: {exc}")
                    )
                else:
                    cells.append(CamMatrixCell(case.id, adapter.identity, True))
                continue
            if not case.expect_parse:
                cells.append(
                    CamMatrixCell(case.id, adapter.identity, False, "malformed case was accepted")
                )
                continue
            mismatches: list[str] = []
            for name, expected, actual in (
                ("file function", case.file_function, parsed.file_function),
                ("file polarity", case.file_polarity, parsed.file_polarity),
                ("units", case.units, parsed.units),
            ):
                if expected is not None and expected != actual:
                    mismatches.append(f"{name}: expected {expected!r}, got {actual!r}")
            if parsed.bounds_nm[2] <= parsed.bounds_nm[0] or parsed.bounds_nm[3] <= parsed.bounds_nm[1]:
                mismatches.append("non-positive rendered extent")
            if case.bounds_nm is not None and any(
                abs(expected - actual) > case.bounds_tolerance_nm
                for expected, actual in zip(case.bounds_nm, parsed.bounds_nm)
            ):
                mismatches.append(
                    f"bounds: expected {case.bounds_nm!r}, got {parsed.bounds_nm!r}"
                )
            if not parsed.topology_digest:
                mismatches.append("missing rendered topology")
            cells.append(
                CamMatrixCell(case.id, adapter.identity, not mismatches, "; ".join(mismatches))
            )
            parsed_by_case.setdefault(case.id, []).append((adapter.identity, parsed))
    for case_id, results in sorted(parsed_by_case.items()):
        if len(results) < 2:
            continue
        baseline = results[0][1]
        for identity, parsed in results[1:]:
            if (parsed.file_function, parsed.file_polarity, parsed.units) != (
                baseline.file_function, baseline.file_polarity, baseline.units
            ):
                findings.append(
                    f"independent CAM metadata disagreement: {case_id}, {identity.name}"
                )
    if any(not cell.passed for cell in cells):
        status = CamGateStatus.FAIL
    elif any(not finding.startswith("required CAM tool unavailable:") for finding in findings):
        status = CamGateStatus.FAIL
    elif missing:
        status = CamGateStatus.INCOMPLETE
    else:
        status = CamGateStatus.PASS
    return CamQualificationMatrix(
        status,
        tuple(corpus_hashes),
        profile.required_tool_identities,
        tuple(cells),
        tuple(findings),
    )


def parse_xnc(path: Path, *, plated: bool | None = None) -> NormalizedDrillProgram:
    """Parse the strict decimal metric XNC subset emitted by the release profile."""
    text = path.read_text(encoding="ascii", errors="strict")
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines or lines[0] != "M48" or lines[-1] != "M30":
        raise ValueError("XNC requires M48 header and final M30")
    tools: dict[str, int] = {}
    current: str | None = None
    hits: list[DrillHit] = []
    in_header = True
    metric = False
    for line in lines[1:-1]:
        if line.startswith(";"):
            continue
        if in_header:
            if line == "%":
                in_header = False
                continue
            if line.startswith("METRIC") and re.fullmatch(r"METRIC(?:,(?:TZ|LZ))?", line):
                metric = True
                continue
            tool_definition = re.fullmatch(r"T(\d+)C(\d+(?:\.\d+)?)", line)
            if tool_definition:
                diameter = round(float(tool_definition.group(2)) * 1_000_000)
                if diameter <= 0 or tool_definition.group(1) in tools:
                    raise ValueError(f"invalid or duplicate XNC tool: {line}")
                tools[tool_definition.group(1)] = diameter
                continue
            if line == "FMAT,2":
                continue
            raise ValueError(f"unsupported XNC header command: {line}")
        if line in {"G90", "G05"}:
            continue
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
            continue
        raise ValueError(f"unsupported XNC body command: {line}")
    if in_header or not metric:
        raise ValueError("XNC requires a closed header and explicit METRIC units")
    return NormalizedDrillProgram(tuple(sorted(hits, key=lambda item: (item.x_nm, item.y_nm,
                                                                        item.diameter_nm, item.tool))), "mm", plated)


def reconcile_drills(board: PhysicalBoard,
                     programs: tuple[NormalizedDrillProgram, ...]) -> CamReconciliation:
    """Compare the normalized hit multiset to pads and vias in signed physical IR."""
    expected: Counter[tuple[int, int, int, bool | None]] = Counter()
    findings: list[str] = []
    for hole in board.mechanical_holes:
        expected[(hole.position.x_nm, hole.position.y_nm, hole.diameter_nm, False)] += 1
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        for pad in footprint.pads:
            if pad.drill is None:
                continue
            if pad.drill.width_nm != pad.drill.height_nm:
                findings.append(f"slot reconciliation requires routed XNC support: {placement.reference}.{pad.number}")
                continue
            # Each physical land owns its drill, including repeated/empty numbers.
            point = transformed_local_point(placement, pad.position)
            plated = pad.kind is PadKind.THROUGH_HOLE
            expected[(point.x_nm, point.y_nm, pad.drill.width_nm, plated)] += 1
    for via in board.vias:
        expected[(via.position.x_nm, via.position.y_nm, via.drill_nm, True)] += 1
    actual: Counter[tuple[int, int, int, bool | None]] = Counter()
    for program in programs:
        for hit in program.hits:
            # KiCad Excellon uses Cartesian-up Y; board IR uses PCB Y-down.
            actual[(hit.x_nm, -hit.y_nm, hit.diameter_nm, program.plated)] += 1
    if expected != actual:
        for item, count in sorted((expected - actual).items()):
            findings.append(f"missing drill hit {item} x{count}")
        for item, count in sorted((actual - expected).items()):
            findings.append(f"unexpected drill hit {item} x{count}")
    return CamReconciliation(not findings, tuple(findings))


def parse_ipcd356(path: Path) -> NormalizedTestNet:
    text = path.read_text(encoding="ascii", errors="strict")
    if not text.rstrip().endswith("999"):
        raise ValueError("IPC-D-356 has no 999 end record")
    scale_nm = 2_540 if "P  UNITS CUST 0" in text else 10_000
    points: list[TestPoint] = []
    vias: list[TestVia] = []
    pad_pattern = re.compile(
        r"^(?:317|327)(?P<net>\S+)\s+(?P<component>\S+)\s+-(?P<pad>\S+)"
        r".*?X(?P<x>[+-]\d+)Y(?P<y>[+-]\d+)"
    )
    via_pattern = re.compile(
        r"^317(?P<net>\S+)\s+VIA\s+MD(?P<drill>\d+)PA\d+"
        r"X(?P<x>[+-]\d+)Y(?P<y>[+-]\d+)"
    )
    for line in text.splitlines():
        if not line.startswith(("317", "327")):
            continue
        via_match = via_pattern.match(line)
        if via_match:
            vias.append(TestVia(
                via_match.group("net"), int(via_match.group("x")) * scale_nm,
                int(via_match.group("y")) * scale_nm,
                int(via_match.group("drill")) * scale_nm,
            ))
            continue
        pad_match = pad_pattern.match(line)
        if pad_match:
            points.append(TestPoint(
                pad_match.group("net"), pad_match.group("component"),
                pad_match.group("pad"), int(pad_match.group("x")) * scale_nm,
                int(pad_match.group("y")) * scale_nm,
            ))
            continue
        raise ValueError(f"unsupported IPC-D-356 contact record: {line[:24]}")
    return NormalizedTestNet(
        tuple(sorted(points, key=lambda item: (
            item.net, item.component, item.pad, item.x_nm, item.y_nm))),
        tuple(sorted(vias, key=lambda item: (item.net, item.x_nm, item.y_nm, item.drill_nm))),
    )


def reconcile_test_net(board: PhysicalBoard,
                       parsed: NormalizedTestNet) -> CamReconciliation:
    placement_by_ref = {placement.reference: placement for placement in board.placements}
    expected = {}
    for net in board.nets:
        for pad in net.pads:
            placement = placement_by_ref[pad.component]
            position = transformed_pad_position(board, placement, pad.pad)
            expected[(pad.component, pad.pad)] = (net.name, position.x_nm, position.y_nm)
    # KiCad writes unconnected physical pads as N/C records. They do not
    # represent source nets, but a connected source pad marked N/C still fails.
    actual = {(point.component, point.pad): point for point in parsed.points
              if point.net != "N/C"}
    findings: list[str] = []
    if len(actual) != sum(point.net != "N/C" for point in parsed.points):
        findings.append("duplicate IPC-D-356 component/pad records")
    for identity, (net, x_nm, y_nm) in sorted(expected.items()):
        if identity not in actual:
            findings.append(f"missing IPC-D-356 point {identity[0]}.{identity[1]}")
            continue
        point = actual[identity]
        if point.net != net:
            findings.append(f"IPC-D-356 point {identity[0]}.{identity[1]} maps to {point.net!r}, expected {net!r}")
        # CUST 0 resolution is 0.0001 inch = 2540 nm. KiCad's IPC Y axis is
        # Cartesian-up while physical IR/KiCad PCB coordinates are Y-down.
        if abs(point.x_nm - x_nm) > 1_270 or abs(point.y_nm + y_nm) > 1_270:
            findings.append(f"IPC-D-356 point {identity[0]}.{identity[1]} has incorrect position")
    for identity in sorted(set(actual) - set(expected)):
        findings.append(f"unexpected IPC-D-356 point {identity[0]}.{identity[1]}")
    # Equivalence-relation check catches merged source nets even when names are truncated.
    grouped: dict[str, set[str]] = {}
    for identity, point in actual.items():
        if identity in expected:
            grouped.setdefault(point.net, set()).add(expected[identity][0])
    for parsed_net, source_nets in sorted(grouped.items()):
        if len(source_nets) > 1:
            findings.append(f"IPC-D-356 net {parsed_net!r} merges source nets {sorted(source_nets)}")
    unmatched_vias = list(parsed.vias)
    for via in board.vias:
        match = next((point for point in unmatched_vias
                      if point.net == via.net
                      and abs(point.x_nm - via.position.x_nm) <= 1_270
                      and abs(point.y_nm + via.position.y_nm) <= 1_270
                      and abs(point.drill_nm - via.drill_nm) <= 1_270), None)
        if match is None:
            findings.append(f"missing IPC-D-356 via on {via.net} at {via.position.x_nm},{via.position.y_nm}")
        else:
            unmatched_vias.remove(match)
    for via in unmatched_vias:
        findings.append(f"unexpected IPC-D-356 via on {via.net} at {via.x_nm},{via.y_nm}")
    return CamReconciliation(not findings, tuple(findings))


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
        if path.stat().st_size > profile.maximum_file_bytes:
            findings.append(f"artifact exceeds size limit: {name}")
            continue
        data = path.read_bytes()
        hashes.append((name, sha256(data).hexdigest()))
    gerbers = tuple(path for path in paths if path.suffix.casefold() in {".gbr", ".ger"})
    if not gerbers:
        findings.append("no Gerber artwork")
    if len({item.name for item in profile.required_tool_identities}) < 2:
        findings.append("qualification requires at least two distinct CAM tools")
    if findings:
        return CamQualificationEvidence(CamGateStatus.FAIL, tuple(hashes), (), tuple(findings))
    expected = {item: item for item in profile.required_tool_identities}
    actual = {adapter.identity: adapter for adapter in adapters}
    missing = tuple(sorted((item for item in expected if item not in actual), key=lambda i: (i.name, i.version)))
    if missing:
        findings.extend(f"required CAM tool unavailable: {item.name} {item.version}" for item in missing)
        return CamQualificationEvidence(CamGateStatus.INCOMPLETE, tuple(hashes), tuple(actual), tuple(findings))
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

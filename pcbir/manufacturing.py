"""Gated KiCad manufacturing export with independent structural CAM checks."""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from typing import Callable

from .backends import KiCadPcbBackend
from .drc import DrcCompleteness, DrcDecision, SignoffToken, physical_board_digest
from .physical import PhysicalBoard
from .process_drc import ProcessDrcReport


@dataclass(frozen=True, slots=True)
class CommandResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


CommandRunner = Callable[[tuple[str, ...], Path], CommandResult]


@dataclass(frozen=True, slots=True)
class ManufacturingProfile:
    name: str = "kicad-gerber-x2-two-layer"
    qualified_kicad_major: int = 10
    gerber_layers: tuple[str, ...] = (
        "F.Cu",
        "B.Cu",
        "F.Mask",
        "B.Mask",
        "F.Silkscreen",
        "B.Silkscreen",
        "Edge.Cuts",
    )
    allow_signoff_waivers: bool = False
    require_ipcd356: bool = True
    required_file_function_prefixes: tuple[str, ...] = (
        "Copper,L1,Top",
        "Copper,L2,Bot",
        "Soldermask,Top",
        "Soldermask,Bot",
        "Profile",
    )
    require_process_drc: bool = False


@dataclass(frozen=True, slots=True)
class CamFinding:
    code: str
    message: str
    artifact: str | None = None


@dataclass(frozen=True, slots=True)
class CamVerificationReport:
    passed: bool
    findings: tuple[CamFinding, ...]
    gerber_count: int
    drill_count: int
    ipcd356_count: int

    def to_json(self) -> str:
        return json.dumps(
            {
                "schema": "copperscript-cam-verification/v0.1",
                "passed": self.passed,
                "gerber_count": self.gerber_count,
                "drill_count": self.drill_count,
                "ipcd356_count": self.ipcd356_count,
                "findings": [
                    {"code": item.code, "message": item.message, "artifact": item.artifact}
                    for item in self.findings
                ],
            },
            indent=2,
            sort_keys=True,
        ) + "\n"


@dataclass(frozen=True, slots=True)
class ManufacturingRelease:
    directory: Path
    manifest: Path
    checksums: Path
    board_file: Path
    drc_report: Path
    cam_report: CamVerificationReport
    kicad_version: str


def build_manufacturing_release(
    board: PhysicalBoard,
    signoff: SignoffToken,
    output_directory: Path,
    *,
    kicad_cli: Path,
    profile: ManufacturingProfile | None = None,
    runner: CommandRunner | None = None,
    process_report: ProcessDrcReport | None = None,
) -> ManufacturingRelease:
    """Create a release atomically; no output is published unless every gate passes."""

    profile = profile or ManufacturingProfile()
    runner = runner or _subprocess_runner
    _validate_release_gate(board, signoff, profile, process_report)
    output_directory = output_directory.resolve()
    if output_directory.exists():
        raise FileExistsError(f"release directory already exists: {output_directory}")
    output_directory.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{output_directory.name}-", dir=output_directory.parent))
    try:
        version_result = runner((str(kicad_cli), "--version"), stage)
        _require_success(version_result, "query KiCad version")
        version = version_result.stdout.strip().splitlines()[0]
        major_match = re.match(r"(\d+)", version)
        if major_match is None or int(major_match.group(1)) != profile.qualified_kicad_major:
            raise RuntimeError(
                f"KiCad {version!r} is not qualified by profile {profile.name!r}; "
                f"expected major {profile.qualified_kicad_major}"
            )

        manifest = KiCadPcbBackend().generate(board)
        board_artifact = manifest.artifacts[0]
        board_path = stage / board_artifact.name
        board_path.write_text(board_artifact.content, encoding="utf-8")
        drc_path = stage / "kicad-drc.json"
        drc_command = [
                str(kicad_cli),
                "pcb",
                "drc",
                "--format",
                "json",
                "--severity-all",
                "--exit-code-violations",
                "--output",
                str(drc_path),
                str(board_path),
        ]
        if board.zones:
            drc_command[-1:-1] = ["--refill-zones", "--save-board"]
        _run(
            runner,
            tuple(drc_command),
            stage,
            "run KiCad DRC",
        )
        gerber_directory = stage / "gerbers"
        drill_directory = stage / "drill"
        gerber_directory.mkdir()
        drill_directory.mkdir()
        _run(
            runner,
            (
                str(kicad_cli),
                "pcb",
                "export",
                "gerbers",
                "--output",
                str(gerber_directory),
                "--layers",
                ",".join(profile.gerber_layers),
                "--no-protel-ext",
                str(board_path),
            ),
            stage,
            "export Gerber X2 artwork",
        )
        _run(
            runner,
            (
                str(kicad_cli),
                "pcb",
                "export",
                "drill",
                "--output",
                str(drill_directory),
                "--format",
                "excellon",
                "--excellon-units",
                "mm",
                "--excellon-separate-th",
                str(board_path),
            ),
            stage,
            "export Excellon drill files",
        )
        ipcd_path = stage / f"{board_path.stem}.d356"
        if profile.require_ipcd356:
            _run(
                runner,
                (
                    str(kicad_cli),
                    "pcb",
                    "export",
                    "ipcd356",
                    "--output",
                    str(ipcd_path),
                    str(board_path),
                ),
                stage,
                "export IPC-D-356 netlist",
            )

        cam = verify_cam_directory(stage, profile)
        (stage / "cam-verification.json").write_text(cam.to_json(), encoding="utf-8")
        if not cam.passed:
            summary = "; ".join(item.message for item in cam.findings)
            raise RuntimeError(f"independent CAM verification failed: {summary}")

        artifact_paths = tuple(
            sorted(
                (
                    item
                    for item in stage.rglob("*")
                    if item.is_file()
                    and item.name not in {"release-manifest.json", "SHA256SUMS"}
                ),
                key=lambda item: item.relative_to(stage).as_posix(),
            )
        )
        release_document = {
            "schema": "copperscript-manufacturing-release/v0.1",
            "profile": profile.name,
            "kicad_version": version,
            "board_digest": signoff.board_digest,
            "signoff_token_digest": signoff.token_digest,
            "signoff_decision": signoff.decision.value,
            "zone_fill": {
                "required": bool(board.zones),
                "engine": "KiCad" if board.zones else None,
                "engine_version": version if board.zones else None,
                "saved_refilled_board": bool(board.zones),
            },
            "process_drc": None if process_report is None else {
                "fabrication": process_report.fabrication.value,
                "stencil": process_report.stencil.value,
                "assembly": process_report.assembly.value,
                "passed": process_report.passed,
            },
            "artifacts": [
                {
                    "path": item.relative_to(stage).as_posix(),
                    "size": item.stat().st_size,
                    "sha256": _file_digest(item),
                }
                for item in artifact_paths
            ],
        }
        manifest_path = stage / "release-manifest.json"
        manifest_path.write_text(json.dumps(release_document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        checksum_paths = (*artifact_paths, manifest_path)
        checksums_path = stage / "SHA256SUMS"
        checksums_path.write_text(
            "".join(
                f"{_file_digest(item)}  {item.relative_to(stage).as_posix()}\n"
                for item in checksum_paths
            ),
            encoding="ascii",
        )
        stage.replace(output_directory)
        return ManufacturingRelease(
            output_directory,
            output_directory / "release-manifest.json",
            output_directory / "SHA256SUMS",
            output_directory / board_path.name,
            output_directory / drc_path.name,
            cam,
            version,
        )
    except Exception:
        if stage.exists():
            shutil.rmtree(stage)
        raise


def verify_cam_directory(
    directory: Path, profile: ManufacturingProfile | None = None
) -> CamVerificationReport:
    """Independently parse structural invariants in Gerber, drill and netlist outputs."""

    profile = profile or ManufacturingProfile()
    findings: list[CamFinding] = []
    gerbers = tuple(sorted((directory / "gerbers").glob("*.gbr")))
    drills = tuple(sorted((directory / "drill").glob("*.drl")))
    ipcd = tuple(sorted(directory.glob("*.d356")))
    functions: set[str] = set()
    for path in gerbers:
        text = path.read_text(encoding="ascii", errors="replace")
        function = re.search(r"%TF\.FileFunction,([^*]+)\*%", text)
        for code, condition, message in (
            ("CAM-GERBER-FORMAT", "%FS" in text, "missing Gerber coordinate format"),
            ("CAM-GERBER-UNITS", "%MOMM*%" in text, "Gerber units are not millimetres"),
            ("CAM-GERBER-X2", function is not None, "missing Gerber X2 FileFunction attribute"),
            ("CAM-GERBER-POLARITY", function is not None and (function.group(1).startswith("Profile") or "%TF.FilePolarity," in text), "missing Gerber X2 polarity attribute"),
            ("CAM-GERBER-END", "M02*" in text, "missing Gerber end-of-file command"),
        ):
            if not condition:
                findings.append(CamFinding(code, message, path.relative_to(directory).as_posix()))
        if function is not None:
            value = function.group(1)
            if value in functions:
                findings.append(CamFinding("CAM-GERBER-DUPLICATE-FUNCTION", f"duplicate FileFunction {value!r}", path.relative_to(directory).as_posix()))
            functions.add(value)
    for prefix in profile.required_file_function_prefixes:
        if not any(value.startswith(prefix) for value in functions):
            findings.append(CamFinding("CAM-GERBER-FUNCTION-MISSING", f"no Gerber declares required FileFunction prefix {prefix!r}"))
    if not drills:
        findings.append(CamFinding("CAM-DRILL-MISSING", "no Excellon drill file was generated"))
    for path in drills:
        text = path.read_text(encoding="ascii", errors="replace")
        if "M48" not in text or "METRIC" not in text or "M30" not in text:
            findings.append(CamFinding("CAM-DRILL-STRUCTURE", "invalid or non-metric Excellon structure", path.relative_to(directory).as_posix()))
    if profile.require_ipcd356:
        if len(ipcd) != 1:
            findings.append(CamFinding("CAM-IPCD356-COUNT", f"expected one IPC-D-356 netlist, found {len(ipcd)}"))
        elif "999" not in ipcd[0].read_text(encoding="ascii", errors="replace"):
            findings.append(CamFinding("CAM-IPCD356-END", "IPC-D-356 netlist has no end record", ipcd[0].relative_to(directory).as_posix()))
    return CamVerificationReport(not findings, tuple(findings), len(gerbers), len(drills), len(ipcd))


def _validate_release_gate(board: PhysicalBoard, signoff: SignoffToken,
                           profile: ManufacturingProfile,
                           process_report: ProcessDrcReport | None) -> None:
    if signoff.board_digest != physical_board_digest(board):
        raise ValueError("signoff token does not match the exact board geometry")
    if signoff.completeness is not DrcCompleteness.COMPLETE:
        raise ValueError("manufacturing export requires complete physical DRC coverage")
    allowed = {DrcDecision.PASS}
    if profile.allow_signoff_waivers:
        allowed.add(DrcDecision.PASS_WITH_WAIVERS)
    if signoff.decision not in allowed:
        raise ValueError(f"signoff decision {signoff.decision.value!r} is not releasable")
    if board.metadata.get("prototype_footprints") == "true":
        raise ValueError("manufacturing export rejects proxy footprints")
    if board.metadata.get("detailed_routing") != "complete":
        raise ValueError("manufacturing export requires completed detailed routing")
    if profile.require_process_drc:
        if process_report is None:
            raise ValueError("manufacturing profile requires fabrication/stencil/assembly DRC")
        if not process_report.passed:
            raise ValueError("fabrication/stencil/assembly DRC is not fully passed")


def _subprocess_runner(command: tuple[str, ...], cwd: Path) -> CommandResult:
    completed = subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=False)
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def _run(runner: CommandRunner, command: tuple[str, ...], cwd: Path, action: str) -> None:
    _require_success(runner(command, cwd), action)


def _require_success(result: CommandResult, action: str) -> None:
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        raise RuntimeError(f"failed to {action}: {detail}")


def _file_digest(path: Path) -> str:
    return sha256(path.read_bytes()).hexdigest()

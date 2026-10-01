"""KiCad-backed evidence for actual filled-plane connectivity.

The zone declaration and provisional pad escapes are never treated as proof of
electrical continuity. This check refills a disposable exported board and
parses KiCad's independent connectivity/DRC report.
"""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
import json
from pathlib import Path
import re
import tempfile

from .backends import KiCadPcbBackend
from .drc import physical_board_digest
from .manufacturing import CommandRunner, _subprocess_runner
from .physical import PhysicalBoard


@dataclass(frozen=True, slots=True)
class PlaneVerification:
    passed: bool
    board_digest: str
    export_digest: str
    filled_board_digest: str
    report_digest: str
    kicad_version: str
    unconnected_count: int
    island_count: int
    other_violation_count: int
    findings: tuple[str, ...]

    def matches(self, board: PhysicalBoard) -> bool:
        """Reject evidence reused after any source-board geometry change."""
        if self.board_digest != physical_board_digest(board):
            return False
        pcb, project = KiCadPcbBackend().generate(board).artifacts[:2]
        current_export = sha256(
            pcb.content.encode("utf-8") + project.content.encode("utf-8")
        ).hexdigest()
        return self.export_digest == current_export

    def zone_connectivity_verified(self, board: PhysicalBoard) -> bool:
        """Separate connected fill from overall signoff, bound to exact export.

        Library-copy/lookup findings remain signoff failures, but do not erase
        the independent zero-open observation. Any other violation, island,
        malformed count, or stale source/export prevents reconciliation.
        """
        library_types = {"lib_footprint_issues", "lib_footprint_mismatch"}
        return (
            self.unconnected_count == self.island_count == 0
            and self.other_violation_count == len(self.findings)
            and all(item.split(":", 1)[0] in library_types for item in self.findings)
            and self.matches(board)
        )

    def to_json(self) -> str:
        return json.dumps({
            "schema": "copperscript-plane-verification/v0.1",
            "passed": self.passed,
            "board_digest": self.board_digest,
            "export_digest": self.export_digest,
            "filled_board_digest": self.filled_board_digest,
            "report_digest": self.report_digest,
            "kicad_version": self.kicad_version,
            "unconnected_count": self.unconnected_count,
            "island_count": self.island_count,
            "other_violation_count": self.other_violation_count,
            "findings": list(self.findings),
        }, indent=2, sort_keys=True) + "\n"


def verify_filled_planes(
    board: PhysicalBoard,
    *,
    kicad_cli: Path,
    runner: CommandRunner | None = None,
    qualified_major: int = 10,
) -> PlaneVerification:
    """Refill a private KiCad board and require zero DRC/connectivity findings."""

    runner = runner or _subprocess_runner
    with tempfile.TemporaryDirectory(prefix="copperscript-plane-") as directory:
        stage = Path(directory)
        version_result = runner((str(kicad_cli), "--version"), stage)
        if version_result.returncode:
            raise RuntimeError(f"KiCad version query failed: {version_result.stderr}")
        version = version_result.stdout.strip().splitlines()[0]
        match = re.match(r"(\d+)", version)
        if match is None or int(match.group(1)) != qualified_major:
            raise RuntimeError(
                f"KiCad {version!r} is not the qualified major {qualified_major}"
            )
        manifest = KiCadPcbBackend().generate(board)
        if len(manifest.artifacts) < 2:
            raise RuntimeError("KiCad backend omitted the same-stem project")
        pcb_artifact, project_artifact = manifest.artifacts[:2]
        pcb_path = stage / pcb_artifact.name
        project_path = stage / project_artifact.name
        # Preserve the backend's exact UTF-8 bytes on Windows as well as Unix.
        pcb_path.write_bytes(pcb_artifact.content.encode("utf-8"))
        project_path.write_bytes(project_artifact.content.encode("utf-8"))
        export_digest = sha256(pcb_path.read_bytes() + project_path.read_bytes()).hexdigest()
        report_path = stage / "drc.json"
        command = [str(kicad_cli), "pcb", "drc", "--format", "json",
                   "--severity-all", "--output", str(report_path)]
        if board.zones:
            command += ["--refill-zones", "--save-board"]
        command.append(str(pcb_path))
        result = runner(tuple(command), stage)
        if result.returncode or not report_path.is_file():
            raise RuntimeError(
                "KiCad refill/DRC failed: "
                + (result.stderr.strip() or result.stdout.strip() or "no report")
            )
        report_bytes = report_path.read_bytes()
        try:
            report = json.loads(report_bytes)
        except (ValueError, UnicodeDecodeError) as exc:
            raise RuntimeError("KiCad produced an invalid JSON DRC report") from exc
        violations = report.get("violations")
        unconnected = report.get("unconnected_items")
        if not isinstance(violations, list) or not isinstance(unconnected, list):
            raise RuntimeError("KiCad DRC report lacks violations/unconnected_items")
        islands = [item for item in violations if _violation_type(item) == "isolated_copper"]
        others = [item for item in violations if _violation_type(item) != "isolated_copper"]
        findings = tuple(
            f"{_violation_type(item)}: {item.get('description', '')}"
            for item in (*unconnected, *islands, *others)
        )
        return PlaneVerification(
            not findings,
            physical_board_digest(board), export_digest,
            sha256(pcb_path.read_bytes()).hexdigest(),
            sha256(report_bytes).hexdigest(), version,
            len(unconnected), len(islands), len(others), findings,
        )


def _violation_type(item: object) -> str:
    if not isinstance(item, dict):
        raise RuntimeError("malformed KiCad DRC finding")
    return str(item.get("type", "unknown"))

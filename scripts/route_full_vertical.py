"""Run the complete full-vertical draft-routing workflow into ignored build/.

From the checkout: uv run --no-sync python scripts/route_full_vertical.py
Requires the committed copper.lock and KiCad footprints. CopperLib is fetched
from its pinned URL into the verified package cache; no manual checkout needed.
No relocking, fabrication-rule relaxation or manufacturing export is performed.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import time

from pcbir.profiling import phase_summary, profiled_command, profile_summary


REPOSITORY = Path(__file__).resolve().parents[1]


def _provenance(repository: Path) -> dict:
    def git(*arguments):
        try:
            result = subprocess.run(["git", *arguments], cwd=repository,
                                    capture_output=True, text=True, encoding="utf-8", errors="replace",
                                    timeout=10, check=False)
            return result.stdout.strip() if result.returncode == 0 else None
        except (OSError, subprocess.TimeoutExpired):
            return None
    diff = git("diff", "HEAD", "--no-ext-diff")
    return {"python": sys.version, "executable": sys.executable, "platform": platform.platform(),
            "logical_cpus": os.cpu_count(), "processor": platform.processor(),
            "git_commit": git("rev-parse", "HEAD"),
            "tracked_changes": git("status", "--porcelain", "--untracked-files=no"),
            "tracked_diff_sha256": hashlib.sha256(diff.encode("utf-8")).hexdigest() if diff is not None else None,
            "input_sha256": {name: hashlib.sha256((repository / name).read_bytes()).hexdigest()
                             for name in ("copper.lock", "examples/full_vertical_board.copper",
                                          "examples/full_vertical_placement_templates.json")}}


def _save_performance(output: Path, mode: str, exit_code: int) -> dict:
    """Telemetry failure must not replace the actual router exit status."""
    result = {"mode": mode, "status": "disabled" if mode == "none" else "missing",
              "process_interrupted": exit_code == 130}
    try:
        if (output / "routing.log").is_file():
            (output / "phase-timings.json").write_text(
                json.dumps(phase_summary(output / "routing.log"), indent=2) + "\n", encoding="utf-8")
            result["phase_timings"] = "phase-timings.json"
        if mode == "cprofile" and (output / "routing.prof").is_file():
            summary, report = profile_summary(output / "routing.prof")
            (output / "profile-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
            (output / "profile-summary.txt").write_text(report, encoding="utf-8")
            result.update(status="saved", raw="routing.prof", summary="profile-summary.json",
                          text="profile-summary.txt")
    except Exception as exc:
        result.update(status="error", error=str(exc))
        print(f"PERFORMANCE REPORT ERROR: {exc}", file=sys.stderr)
    return result


def _default_kicad_cli() -> Path:
    found = shutil.which("kicad-cli")
    if found:
        return Path(found)
    return Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "KiCad/10.0/bin/kicad-cli.exe"


def _default_footprints(cli: Path) -> Path:
    configured = os.environ.get("KICAD10_FOOTPRINT_DIR")
    if configured:
        return Path(configured)
    bundled = cli.resolve().parent.parent / "share/kicad/footprints"
    return bundled if bundled.is_dir() else Path("/usr/share/kicad/footprints")


def routing_command(repository: Path, output: Path, cli: Path,
                    footprints: Path, copperlib_footprints: Path,
                    *, dependency_expansions: int = 2,
                    incremental_placement: bool = True,
                    include_placement_templates: bool = True) -> list[str]:
    """Keep the reviewed workflow explicit; paths are never shell-expanded."""
    command = [
        sys.executable, "-u", "-m", "copperscript", "route-board",
        str(repository / "examples/full_vertical_board.copper"),
        "--locked", "--offline", "--layers", "6", "--fab-profile", "jlcpcb-six-layer",
        "--footprint-root", str(footprints), "--footprint-root", str(copperlib_footprints),
        "--candidates", "1", "--placement-candidate", "candidate-01",
        "--feedback-iterations", "1", "--router-iterations", "5", "--critical-feedback-trials", "0",
        "--pitch-mm", "1", "--passes", "2", "--search-budget", "20000", "--progress",
        "--soft-ripup", "--fanout", "--constrained-pins-first", "--progressive-guides",
        "--repair-budget-multiplier", "10",
        "--plane-contact-radius-mm", "5", "--zone-escape-trials", "4", "--zone-local-ripup-trials", "6",
        "--zone-dependency-expansions", str(dependency_expansions),
        "--verify-plane-fill", str(cli),
        "--report", str(output / "route-report.json"), "-o", str(output / "board.kicad_pcb"),
    ]
    if include_placement_templates:
        command.extend(("--placement-templates", str(repository / "examples/full_vertical_placement_templates.json")))
    if not incremental_placement:
        command.append("--no-incremental-placement-repair")
    return command


def _run_logged(command: list[str], repository: Path, log: Path) -> int:
    environment = {**os.environ, "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"}
    with log.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(command, cwd=repository, env=environment,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   text=True, encoding="utf-8", errors="replace")
        try:
            assert process.stdout is not None
            for line in process.stdout:
                print(line, end="", flush=True)
                stream.write(line)
                stream.flush()
            return process.wait()
        except BaseException:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            raise
        finally:
            if process.stdout is not None:
                process.stdout.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--kicad-cli", type=Path, default=_default_kicad_cli())
    parser.add_argument("--kicad-footprints", type=Path, help="KiCad footprint-library directory")
    parser.add_argument("--copperlib-footprints", type=Path,
                        help="optional explicit footprint override (default: locked URL library cache)")
    parser.add_argument("--offline", action="store_true", help="require an already populated package cache")
    parser.add_argument("--output-dir", type=Path,
                        help="new/empty directory inside repository build/ (default: timestamped run)")
    parser.add_argument("--dry-run", action="store_true", help="validate paths and print command; write nothing")
    parser.add_argument("--profile", choices=("cprofile", "none"), default="cprofile",
                        help="function profiling (default: cprofile); none for uninstrumented benchmarks")
    parser.add_argument("--zone-dependency-expansions", type=int, choices=range(9), default=2,
                        help="local blocker-cone expansion rounds (default: 2; 0 for comparison)")
    parser.add_argument("--no-incremental-placement-repair", action="store_true",
                        help="compare using full-pipeline placement trials")
    args = parser.parse_args(argv)
    cli = args.kicad_cli.resolve()
    footprints = (args.kicad_footprints or _default_footprints(cli)).resolve()
    from pcbir.packages import resolve_module_root
    from pcbir.syntax import CopperScriptError
    try:
        copperlib = (args.copperlib_footprints.resolve() if args.copperlib_footprints else
            resolve_module_root(REPOSITORY / "examples/full_vertical_board.copper",
                "github.com/andenore/CopperLib",locked=True,offline=args.offline or args.dry_run) / "footprints")
    except (OSError,ValueError,CopperScriptError) as exc:
        print(f"SETUP ERROR: {exc}",file=sys.stderr)
        return 2
    started = datetime.now(timezone.utc)
    output = args.output_dir or Path("build/full-vertical") / started.strftime("%Y%m%dT%H%M%S%fZ")
    if not output.is_absolute():
        output = REPOSITORY / output
    output = output.resolve()
    build = (REPOSITORY / "build").resolve()
    try:
        if not output.is_relative_to(build):
            raise ValueError("--output-dir must stay inside the repository build/ directory")
        for required in (cli, REPOSITORY / "copper.lock",
                         REPOSITORY / "examples/full_vertical_board.copper",
                         REPOSITORY / "examples/full_vertical_placement_templates.json"):
            if not required.is_file():
                raise ValueError(f"missing file: {required}; see the README checkout/setup instructions")
        for required in (footprints, copperlib):
            if not required.is_dir():
                raise ValueError(f"missing footprint directory: {required}; supply a footprint-path override")
        if output.exists() and (not output.is_dir() or any(output.iterdir())):
            raise ValueError(f"output directory is not empty: {output}; select a new directory")
    except (OSError, ValueError) as exc:
        print(f"SETUP ERROR: {exc}", file=sys.stderr)
        return 2
    command = routing_command(REPOSITORY, output, cli, footprints, copperlib,
                              dependency_expansions=args.zone_dependency_expansions,
                              incremental_placement=not args.no_incremental_placement_repair)
    routing_arguments = command
    if args.profile == "cprofile":
        command = profiled_command(command, output / "routing.prof")
    print(f"Output: {output}", flush=True)
    print(subprocess.list2cmdline(command) if os.name == "nt" else shlex.join(command), flush=True)
    if args.dry_run:
        return 0
    manifest = {"started_utc": started.isoformat(), "repository": str(REPOSITORY),
                "command": command, "routing_command": routing_arguments,
                "profiling": {"mode": args.profile, "status": "pending"},
                "provenance": _provenance(REPOSITORY), "status": "running", "exit_code": None}
    manifest_path = output / "run.json"
    tick = time.perf_counter()
    exit_code = 2
    try:
        output.mkdir(parents=True, exist_ok=True)
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        exit_code = _run_logged(command, REPOSITORY, output / "routing.log")
    except KeyboardInterrupt:
        exit_code = 130
        print("Routing interrupted; partial artifacts are not a completed result.", file=sys.stderr)
    except OSError as exc:
        manifest["error"] = str(exc)
        print(f"RUN ERROR: {exc}", file=sys.stderr)
    finally:
        manifest["profiling"] = _save_performance(output, args.profile, exit_code)
        status = {0: "passed", 1: "unmet_gates", 130: "interrupted"}.get(exit_code, "error")
        manifest.update(finished_utc=datetime.now(timezone.utc).isoformat(),
                        elapsed_seconds=round(time.perf_counter() - tick, 3),
                        status=status, exit_code=exit_code)
        if output.is_dir():
            try:
                manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
            except OSError as exc:
                print(f"MANIFEST ERROR: {exc}", file=sys.stderr)
                exit_code = 2
    print(f"Routing exit code: {exit_code}; outputs: {output}")
    if exit_code == 1:
        print("Draft has unmet routing/DRC gates; inspect route-report.json. This is not production signoff.")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

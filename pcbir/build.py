"""Generic profiled routing, saved fill and verification for shared Make rules.

Board sources, library identities, geometry and routing presets belong to the
project, not this runner. Native DRC success is not manufacturing signoff.
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

from .compiler import compile_design_file
from .profiling import phase_summary, profiled_command, profile_summary
from .syntax import CopperScriptError


def default_kicad_cli() -> Path:
    configured = os.environ.get("KICAD_CLI") or shutil.which("kicad-cli")
    return Path(configured or Path(os.environ.get("ProgramFiles", "C:/Program Files"))
                / "KiCad/10.0/bin/kicad-cli.exe")


def default_footprints(cli: Path) -> Path:
    configured = os.environ.get("KICAD10_FOOTPRINT_DIR")
    bundled = cli.resolve().parent.parent / "share/kicad/footprints"
    return Path(configured) if configured else (
        bundled if bundled.is_dir() else Path("/usr/share/kicad/footprints"))


def routing_command(source: Path, output: Path, cli: Path, footprints: Path,
                    options: list[str], *, locked: bool = True) -> list[str]:
    reserved = {"-o", "--output", "--report", "--verify-plane-fill", "--locked", "--offline"}
    if any(arg.split("=", 1)[0] in reserved or arg.startswith("-o") and not arg.startswith("--")
           for arg in options):
        raise ValueError("output, report, verification and resolution flags are controlled by the build runner")
    command = [sys.executable, "-u", "-m", "copperscript", "route-board", str(source),
        *(["--locked"] if locked else []), "--offline", "--footprint-root", str(footprints), *options,
        "--verify-plane-fill", str(cli), "--report", str(output / "route-report.json"),
        "-o", str(output / "board.kicad_pcb")]
    from .cli import _parser
    _parser().parse_args(command[4:])
    return command


def completed(output: Path) -> bool:
    """Fail closed: subprocess exit alone is not completion evidence."""
    try:
        route = json.loads((output / "route-report.json").read_text(encoding="utf-8"))
        native = json.loads((output / "kicad-drc.json").read_text(encoding="utf-8"))
        return (route["status"] == "pass" and route["erc_pass"] is True
            and route["routing_complete"] is True
            and route["connectivity"]["native_fill_verified"] is True
            and route["connectivity"]["unrouted_ordinary_nets"] == []
            and native["violations"] == [] and native["unconnected_items"] == []
            and (output / "board.kicad_pcb").is_file())
    except (OSError, ValueError, KeyError, TypeError):
        return False


def run_logged(command: list[str], cwd: Path, log: Path) -> int:
    with log.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, encoding="utf-8", errors="replace",
            env={**os.environ, "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"})
        try:
            assert process.stdout is not None
            for line in process.stdout:
                print(line, end="", flush=True)
                stream.write(line)
                stream.flush()
            return process.wait()
        except BaseException as exc:
            # A CI process-group interrupt also reaches the profiled child.
            # Let its finally block save statistics before terminating it.
            if isinstance(exc, KeyboardInterrupt):
                try:
                    process.wait(timeout=10)
                    raise
                except subprocess.TimeoutExpired:
                    pass
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


def save_performance(output: Path, mode: str, exit_code: int) -> dict:
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


def provenance(project: Path, source: Path, options: list[str]) -> dict:
    paths = {source}
    # Hash project-owned sources and manifests, excluding generated/cache trees.
    # External package inventories are authenticated by the committed lock.
    for directory, subdirs, files in os.walk(project):
        subdirs[:] = [name for name in subdirs if name not in {
            "build", "dist", ".git", ".venv", "venv", ".copper-cache", "__pycache__"}]
        paths.update(Path(directory) / name for name in files
                     if name.endswith((".copper", ".mk")) or name in {"copper.mod", "copper.lock", "Makefile"})
    for index, value in enumerate(options):
        if value in {"--hard-macro", "--placement-templates"}:
            paths.add(Path(options[index + 1]).resolve())
        elif value.startswith(("--hard-macro=", "--placement-templates=")):
            paths.add(Path(value.split("=", 1)[1]).resolve())
    def git(*args):
        try:
            result = subprocess.run(["git", *args], cwd=project, capture_output=True,
                text=True, encoding="utf-8", errors="replace", timeout=10)
            return result.stdout.strip() if result.returncode == 0 else None
        except (OSError, subprocess.TimeoutExpired):
            return None
    diff = git("diff", "HEAD", "--no-ext-diff")
    return {"python": sys.version, "executable": sys.executable, "platform": platform.platform(),
        "logical_cpus": os.cpu_count(), "processor": platform.processor(),
        "build_runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "git_commit": git("rev-parse", "HEAD"),
        "tracked_changes": git("status", "--porcelain", "--untracked-files=no"),
        "tracked_diff_sha256": hashlib.sha256(diff.encode()).hexdigest() if diff is not None else None,
        "input_sha256": {str(p.relative_to(project)) if p.is_relative_to(project) else str(p):
            hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)}}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--project-root", type=Path, default=Path.cwd())
    parser.add_argument("--kicad-cli", type=Path, default=default_kicad_cli())
    parser.add_argument("--kicad-footprints", type=Path)
    parser.add_argument("--locked", action="store_true")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--output-dir", type=Path, help="new/empty directory below project build/")
    parser.add_argument("--output-root", type=Path, help="timestamped runs below this project build/ directory")
    parser.add_argument("--profile", choices=("cprofile", "none"), default="cprofile")
    parser.add_argument("--dry-run", action="store_true", help="validate cached inputs; write nothing")
    raw = list(sys.argv[1:] if argv is None else argv)
    separator = raw.index("--") if "--" in raw else len(raw)
    args = parser.parse_args(raw[:separator])
    options = raw[separator + 1:]
    started = datetime.now(timezone.utc)
    project, source = args.project_root.resolve(), args.source.resolve()
    cli = Path(shutil.which(str(args.kicad_cli)) or args.kicad_cli).resolve()
    footprints = (args.kicad_footprints or default_footprints(cli)).resolve()
    base = (args.output_root or project / "build" / source.stem).resolve()
    output = (args.output_dir or base / started.strftime("%Y%m%dT%H%M%S%fZ")).resolve()
    try:
        build = project / "build"
        if output == build or not output.is_relative_to(build):
            raise ValueError("output must stay below project build/")
        if not source.is_relative_to(project):
            raise ValueError("source must be inside --project-root")
        if output.exists() and (not output.is_dir() or any(output.iterdir())):
            raise ValueError(f"output is not empty: {output}")
        if not cli.is_file() or not footprints.is_dir():
            raise ValueError("KiCad CLI or footprint directory missing; supply explicit paths")
        route = routing_command(source, output, cli, footprints, options, locked=args.locked)
        # Resolve this board's actual imports through its nearest manifest. No
        # hardcoded library, sibling checkout, relocking or executed vendor code.
        compile_design_file(source, locked=args.locked, offline=args.offline or args.dry_run)
        inputs = provenance(project, source, options)
    except (OSError, ValueError, CopperScriptError) as exc:
        print(f"SETUP ERROR: {exc}", file=sys.stderr)
        return 2
    command = profiled_command(route, output / "routing.prof") if args.profile == "cprofile" else route
    fill = [str(cli), "pcb", "drc", "--refill-zones", "--save-board", "--severity-all",
            "--format", "json", "--output", str(output / "kicad-drc.json"), str(output / "board.kicad_pcb")]
    print(f"Output: {output}", flush=True)
    for item in (command, fill):
        print(subprocess.list2cmdline(item) if os.name == "nt" else shlex.join(item), flush=True)
    if args.dry_run:
        return 0
    manifest = {"started_utc": started.isoformat(), "status": "running", "command": command,
                "routing_command": route, "fill_command": fill, "fabrication_ready": False, "provenance": inputs}
    tick, code = time.perf_counter(), 2
    owned = False
    try:
        output.mkdir(parents=True, exist_ok=True)
        manifest_path = output / "run.json"
        # Claim the run exclusively even if two callers validate the same empty
        # RUN_DIR simultaneously. Never overwrite the other caller's evidence.
        with manifest_path.open("x", encoding="utf-8") as stream:
            owned = True
            stream.write(json.dumps(manifest, indent=2) + "\n")
        code = run_logged(command, Path.cwd(), output / "routing.log")
        if code == 0:
            code = run_logged(fill, Path.cwd(), output / "fill.log")
            if code == 0 and not completed(output):
                code = 1
            if code == 0:
                manifest["filled_board_sha256"] = hashlib.sha256((output / "board.kicad_pcb").read_bytes()).hexdigest()
                manifest["native_drc_sha256"] = hashlib.sha256((output / "kicad-drc.json").read_bytes()).hexdigest()
    except KeyboardInterrupt:
        code = 130
    except (OSError, ValueError) as exc:
        manifest["error"] = str(exc)
        code = 2
    finally:
        manifest["profiling"] = save_performance(output, args.profile, code) if owned else {"status": "not_started"}
        manifest.update(status={0: "passed", 1: "unmet_gates", 130: "interrupted"}.get(code, "error"),
                        exit_code=code, elapsed_seconds=round(time.perf_counter() - tick, 3),
                        finished_utc=datetime.now(timezone.utc).isoformat())
        if owned:
            try:
                (output / "run.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
            except OSError as exc:
                print(f"MANIFEST ERROR: {exc}", file=sys.stderr)
                code = 2
    print(f"Build exit code: {code}; outputs: {output}")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

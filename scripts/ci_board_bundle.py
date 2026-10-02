"""Trusted CI: locked URL dependencies, bounded complete routing, draft bundles.

No manufacturing export, rule relaxation, relocking or dry-run routing.
Preparation fetches the pinned library into the managed package cache; routing
then consumes it offline. No sibling/manual CopperLib checkout is required.
All generated files stay below build/; subprocesses never use a shell.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import zipfile

ROOT = Path(__file__).resolve().parents[1]
LAYERS = ("F.Cu", "In1.Cu", "In2.Cu", "In3.Cu", "In4.Cu", "B.Cu")
LIBRARY_MODULE = "github.com/andenore/CopperLib"
WORKER = """import runpy, signal, sys
def interrupt(*args): raise KeyboardInterrupt()
if hasattr(signal, 'SIGBREAK'): signal.signal(signal.SIGBREAK, interrupt)
module = sys.argv[1]
sys.argv = [module, *sys.argv[2:]]
runpy.run_module(module, run_name='__main__')
"""


def save(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(repository: Path, *arguments: str) -> str:
    return subprocess.check_output(["git", *arguments], cwd=repository,
                                   text=True, encoding="utf-8", timeout=30).strip()


def build_path(path: Path) -> Path:
    result = path.resolve()
    if not result.is_relative_to((ROOT / "build").resolve()):
        raise ValueError("CI output must stay inside this repository's build/")
    return result


def library_module(*, offline: bool):
    """Resolve the public module through the same locked package API as imports."""
    from pcbir.packages import PackageResolver
    from pcbir.syntax import SourceLocation
    source = ROOT / "examples/full_vertical_board.copper"
    location = SourceLocation(str(source), 0, 1, 1)
    resolver = PackageResolver.for_source(source, location, locked=True, offline=offline)
    if LIBRARY_MODULE in resolver.manifest.replacements:
        raise ValueError("CI requires CopperLib through its URL, not a local replacement")
    result = resolver.resolve(LIBRARY_MODULE, location)
    if not re.fullmatch(r"[0-9a-f]{40}", result.version):
        raise ValueError("CI requires CopperLib pinned by full Git commit in copper.mod")
    if git(result.directory, "rev-parse", "HEAD") != result.version:
        raise ValueError("managed library cache HEAD differs from the locked Git revision")
    return result


def prepare(output: Path) -> None:
    from pcbir.compiler import compile_file
    pins = json.loads((ROOT / ".github/board-toolchain.json").read_text())
    lock_path = ROOT / "copper.lock"
    original = lock_path.read_bytes()
    output.mkdir(parents=True, exist_ok=True)
    (output / "committed-copper.lock").write_bytes(original)
    # The resolver fetches only the pinned revision and verifies every lock byte.
    # Nothing here canonicalizes, refreshes or modifies the authoritative lock.
    library = library_module(offline=False)
    compile_file(ROOT / "examples/full_vertical_board.copper", locked=True, offline=True)
    compile_file(ROOT / "examples/nrf52_coin_cell.copper", locked=True, offline=True)
    if lock_path.read_bytes() != original:
        raise ValueError("locked preparation unexpectedly changed copper.lock")
    save(output / "provenance.json", {
        "schema": "copperscript-ci-provenance/v1", "toolchain": pins,
        "commit": git(ROOT, "rev-parse", "HEAD"), "copperlib_commit": library.version,
        "library_source": f"https://{library.module_path}.git", "library_checksum": library.checksum,
        "python": sys.version, "platform": platform.platform(),
        "lock_sha256": hashlib.sha256(original).hexdigest(), "lock_modified": False,
        "github": {key: os.environ.get(key) for key in (
            "GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_REF", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")},
        "inputs": {name: sha(ROOT / name) for name in (
            "examples/full_vertical_board.copper", "examples/full_vertical_placement_templates.json",
            "examples/nrf52_coin_cell.copper", "examples/nrf_antenna_hard_macro.json")}})


def run_logged(command: list[str], output: Path, seconds: float,
               *, grace_seconds: float = 45) -> dict:
    """Interrupt the router's process group, allowing cProfile finally to save.

    Forced termination is a fallback, reported honestly; it may lose profiles.
    """
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    result = {"command": command, "timed_out": False, "forced_termination": False,
              "started_utc": datetime.now(timezone.utc).isoformat()}
    flags = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if os.name == "nt" else {"start_new_session": True}
    with (output / "ci-process.log").open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
            env={**os.environ, "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"}, **flags)
        def drain():
            assert process.stdout is not None
            for line in process.stdout:
                stream.write(line)
                stream.flush()
                print(line, end="", flush=True)
        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        try:
            result["exit_code"] = process.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            result["timed_out"] = True
            try:
                if os.name == "nt":
                    process.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    os.killpg(process.pid, signal.SIGINT)
                process.wait(timeout=grace_seconds)
            except (OSError, subprocess.TimeoutExpired):
                result["forced_termination"] = True
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                                   capture_output=True, timeout=30, check=False)
                else:
                    os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=30)
            result["exit_code"] = 124
        finally:
            reader.join(timeout=5)
            if process.stdout is not None:
                process.stdout.close()
    result["elapsed_seconds"] = round(time.monotonic() - started, 3)
    save(output / "ci-process.json", result)
    return result


def route(output: Path, cli: Path, footprints: Path, minutes: int,
          include_nrf: bool) -> None:
    spec = importlib.util.spec_from_file_location("full_vertical_runner", ROOT / "scripts/route_full_vertical.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    from pcbir.profiling import profiled_command
    board_dir = output / "full-vertical"
    if board_dir.exists():
        raise ValueError("route output already exists; do not mix CI runs")
    library = library_module(offline=True).directory
    arguments = runner.routing_command(ROOT, board_dir, cli, footprints, library / "footprints")
    wrapped = profiled_command(arguments, board_dir / "routing.prof")
    command = [wrapped[0], "-u", "-c", WORKER, "pcbir.profiling", *wrapped[4:]]
    result = run_logged(command, board_dir, minutes * 60)
    shutil.copy2(board_dir / "ci-process.log", board_dir / "routing.log")
    result["profiling"] = runner._save_performance(board_dir, "cprofile",
        130 if result["timed_out"] else result["exit_code"])
    result["provenance"] = runner._provenance(ROOT)
    result["routing_command"] = arguments
    result["status"] = ("timeout" if result["timed_out"] else
                        {0: "passed", 1: "unmet_gates"}.get(result["exit_code"], "error"))
    save(board_dir / "run.json", result)
    if include_nrf:
        run_logged([sys.executable, "-u", "-c", WORKER, "pcbir.nrf52_example", "--route", "--offline",
            "--kicad-cli", str(cli),
            "--footprint-root", str(footprints),
            "--output-dir", str(output / "nrf52-coin-cell")], output / "nrf52-coin-cell", 20 * 60)


def inspect(output: Path, cli: Path) -> list[dict]:
    """Refill a separate inspection copy; never change the delivered router input.

    Copy its project/local library so DRC uses generated rules, not global state.
    SVGs are direct KiCad layer exports, not illustrative generated graphics.
    """
    results = []
    for source in sorted(output.glob("*/*.kicad_pcb")):
        review = source.parent / "inspection"
        review.mkdir(exist_ok=True)
        board = review / source.name
        shutil.copy2(source, board)
        for suffix in (".kicad_pro", ".kicad_dru"):
            project = source.with_suffix(suffix)
            if project.exists():
                shutil.copy2(project, board.with_suffix(suffix))
        for sibling in source.parent.iterdir():
            if sibling.name == "fp-lib-table":
                shutil.copy2(sibling, review / sibling.name)
            elif sibling.is_dir() and sibling.suffix == ".pretty":
                shutil.copytree(sibling, review / sibling.name, dirs_exist_ok=True)
        commands = [([str(cli), "pcb", "drc", "--format", "json", "--refill-zones", "--save-board",
                      "--exit-code-violations", "-o", str(review / "kicad-drc.json"), str(board)], 180)]
        for layer in LAYERS:
            commands.append(([str(cli), "pcb", "export", "svg", "--layers", f"{layer},Edge.Cuts",
                              "--mode-single", "--fit-page-to-board", "--exclude-drawing-sheet",
                              "-o", str(review / (layer.replace(".", "_") + ".svg")), str(board)], 30))
        for index, (command, seconds) in enumerate(commands):
            try:
                result = subprocess.run(command, cwd=review, capture_output=True, text=True,
                                        encoding="utf-8", errors="replace", timeout=seconds, check=False)
                item = {"command": command, "exit_code": result.returncode,
                        "stdout": result.stdout, "stderr": result.stderr}
            except (OSError, subprocess.TimeoutExpired) as exc:
                item = {"command": command, "error": str(exc)}
            save(review / f"command-{index:02d}.json", item)
        drc = review / "kicad-drc.json"
        try:
            report = json.loads(drc.read_text(encoding="utf-8")) if drc.exists() else None
        except (ValueError, OSError) as exc:
            save(review / "drc-read-error.json", {"error": str(exc)})
            report = None
        if not isinstance(report, dict) or not all(isinstance(report.get(key), list)
                for key in ("violations", "unconnected_items")):
            report = None
        drc_result = json.loads((review / "command-00.json").read_text(encoding="utf-8"))
        results.append({"board": source.relative_to(output).as_posix(),
                        "drc_exit_code": drc_result.get("exit_code"),
                        "geometry_violations": len(report.get("violations", [])) if report else None,
                        "unconnected_items": len(report.get("unconnected_items", [])) if report else None,
                        "library_findings": len(report.get("schematic_parity", [])) if report else None})
    return results


def package(output: Path, destination: Path, inspections: list[dict]) -> Path:
    """Package every generated project/report/preview, including partial failures.

    Entries and timestamps are stable, symlinks forbidden, hashes cover all bytes.
    No run is called manufacturing-ready based only on routing exit status.
    """
    if destination == output or destination.is_relative_to(output):
        raise ValueError("archive destination may not be inside the artifact input tree")
    output.mkdir(parents=True, exist_ok=True)
    if not (output / "provenance.json").exists():
        save(output / "provenance.json", {"setup_completed": False,
            "github_sha": os.environ.get("GITHUB_SHA"), "github_ref": os.environ.get("GITHUB_REF"),
            "note": "Setup failed before complete source/toolchain provenance could be recorded."})
    save(output / "inspection-summary.json", inspections)
    disclaimer = ("INSPECTION DRAFT — NOT FOR MANUFACTURE\n\n"
        "Complete routing was attempted, not guaranteed. Inspect per-board ci-process.json,\n"
        "route-report.json, logs and inspection/kicad-drc.json for failures, timeouts,\n"
        "unconnected items and violations. Missing board/report means no completed result.\n"
        "RF/USB/battery and fabrication qualification are not implied by zero DRC.\n"
        "No ungated Gerber/drill manufacturing release is generated by this workflow.\n"
        "Keep each .kicad_pcb with its .kicad_pro, fp-lib-table and .pretty directory.\n")
    (output / "READ-ME-FIRST.txt").write_text(disclaimer, encoding="utf-8")
    files = sorted(path for path in output.rglob("*") if path.is_file())
    if any(path.is_symlink() for path in output.rglob("*")):
        raise ValueError("artifact trees may not contain symlinks")
    entries = [{"path": p.relative_to(output).as_posix(), "sha256": sha(p), "size": p.stat().st_size}
               for p in files if p.name != "artifact-manifest.json"]
    save(output / "artifact-manifest.json", {"schema": "copperscript-inspection-bundle/v1",
         "fabrication_ready": False, "inspection_results": inspections, "files": entries})
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / "board-inspection-drafts.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as stream:
        for path in sorted(p for p in output.rglob("*") if p.is_file()):
            info = zipfile.ZipInfo(path.relative_to(output).as_posix(), (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            stream.writestr(info, path.read_bytes())
    (destination / "RELEASE-NOTES.md").write_text(
        "# CopperScript board inspection snapshot\n\nThese are **experimental drafts, not production files**. "
        "See the ZIP's READ-ME-FIRST.txt, provenance, routing logs and independent per-board DRC. "
        "Incomplete/failed/timed-out attempts retain available diagnostics.\n\n"
        "Includes KiCad projects/local footprints, layer SVGs, reports, profiling and source distributions "
        "when their build stages produced them. No manufacturing qualification is claimed.\n",
        encoding="utf-8")
    checksums = "".join(f"{sha(p)}  {p.name}\n" for p in sorted(destination.iterdir())
                        if p.is_file() and p.name != "SHA256SUMS.txt")
    (destination / "SHA256SUMS.txt").write_text(checksums, encoding="utf-8")
    return archive


def gates_passed(output: Path) -> bool:
    """Conservative inspection gates; never a fabrication signoff token."""
    runs = sorted(output.glob("*/ci-process.json"))
    try:
        summary = json.loads((output / "inspection-summary.json").read_text())
        if not runs or not summary or len(runs) != len(summary):
            return False
        for path in runs:
            run = json.loads(path.read_text())
            if run["exit_code"] != 0 or run["timed_out"]:
                return False
            route_report = json.loads((path.parent / "route-report.json").read_text())
            if route_report.get("status") != "pass" or not route_report.get("erc_pass"):
                return False
        return all(item["drc_exit_code"] == 0 and item["geometry_violations"] == 0
                   and item["unconnected_items"] == 0 for item in summary)
    except (ValueError, OSError, KeyError):
        return False


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("prepare", "route", "sources", "package", "check"))
    parser.add_argument("--output", type=Path, default=ROOT / "build/ci-routing")
    parser.add_argument("--dist", type=Path, default=ROOT / "build/ci-dist")
    parser.add_argument("--kicad-cli", type=Path, default=Path(os.environ.get("KICAD_CLI", "kicad-cli")))
    parser.add_argument("--footprints", type=Path, default=Path(os.environ.get("KICAD10_FOOTPRINT_DIR", "/usr/share/kicad/footprints")))
    parser.add_argument("--minutes", type=int, default=270)
    parser.add_argument("--include-nrf", action="store_true")
    args = parser.parse_args(argv)
    output = build_path(args.output)
    try:
        if args.stage == "prepare":
            prepare(output)
        elif args.stage == "route":
            if not 1 <= args.minutes <= 270:
                raise ValueError("routing budget must be 1..270 minutes")
            route(output, args.kicad_cli.resolve(), args.footprints.resolve(),
                  args.minutes, args.include_nrf)
        elif args.stage == "sources":
            destination = build_path(args.dist)
            destination.mkdir(parents=True, exist_ok=True)
            library = library_module(offline=True)
            subprocess.run(["git", "archive", "--format=zip",
                f"--output={destination / 'copperlib-sources.zip'}", library.version],
                cwd=library.directory, check=True, timeout=120)
        elif args.stage == "package":
            package(output, build_path(args.dist), inspect(output, args.kicad_cli.resolve()))
        else:
            if not gates_passed(output):
                print("Inspection gates failed or results are incomplete; published files are drafts.")
                return 1
    except Exception as exc:
        save(output / f"ci-{args.stage}-error.json", {"error": str(exc), "fabrication_ready": False})
        print(f"CI {args.stage} ERROR: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

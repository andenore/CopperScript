"""Route the CM4 carrier, save filled copper, and check that exact KiCad board.

Run from the checkout: uv run --no-sync python scripts/route_cm4.py
Outputs and profiling stay in ignored build/. This is not manufacturing signoff.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

import route_full_vertical as workflow
from pcbir.packages import resolve_module_root
from pcbir.profiling import profiled_command
from pcbir.syntax import CopperScriptError

REPOSITORY = Path(__file__).resolve().parents[1]
SOURCE = Path("examples/cm4_baseboard/board.copper")


def routing_command(repository: Path, output: Path, cli: Path, footprints: Path) -> list[str]:
    return [sys.executable, "-u", "-m", "copperscript", "route-board",
        str(repository / SOURCE), "--locked", "--offline", "--layers", "4",
        "--fab-profile", "jlcpcb-four-layer", "--footprint-root", str(footprints),
        "--candidates", "1", "--placement-candidate", "candidate-00",
        "--feedback-iterations", "1", "--router-iterations", "5",
        "--pitch-mm", "1", "--passes", "2", "--search-budget", "20000", "--progress",
        "--soft-ripup", "--fanout", "--fanout-maze", "--constrained-pins-first",
        "--progressive-guides", "--repair-budget-multiplier", "10",
        "--plane-contact-radius-mm", "5", "--verify-plane-fill", str(cli),
        "--report", str(output / "route-report.json"), "-o", str(output / "board.kicad_pcb")]


def completed(output: Path) -> bool:
    """Fail closed: a process exit alone is not evidence of completed routing."""
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kicad-cli", type=Path, default=workflow._default_kicad_cli())
    parser.add_argument("--kicad-footprints", type=Path)
    parser.add_argument("--offline", action="store_true", help="require a populated locked library cache")
    parser.add_argument("--output-dir", type=Path, help="new/empty directory below repository build/")
    parser.add_argument("--profile", choices=("cprofile", "none"), default="cprofile")
    parser.add_argument("--dry-run", action="store_true", help="validate and print commands; write nothing")
    args = parser.parse_args(argv)
    started = datetime.now(timezone.utc)
    cli = Path(shutil.which(str(args.kicad_cli)) or args.kicad_cli).resolve()
    footprints = (args.kicad_footprints or workflow._default_footprints(cli)).resolve()
    output = args.output_dir or Path("build/cm4-baseboard") / started.strftime("%Y%m%dT%H%M%S%fZ")
    output = (REPOSITORY / output).resolve()
    try:
        build = (REPOSITORY / "build").resolve()
        if output == build or not output.is_relative_to(build):
            raise ValueError("output must be a new/empty directory below repository build/")
        if output.exists() and (not output.is_dir() or any(output.iterdir())):
            raise ValueError(f"output is not empty: {output}")
        for path in (cli, REPOSITORY / SOURCE, REPOSITORY / SOURCE.parent / "copper.lock"):
            if not path.is_file():
                raise ValueError(f"missing required file: {path}")
        if not footprints.is_dir():
            raise ValueError(f"missing KiCad footprints: {footprints}")
        # Fetch only the manifest's pinned URL; never substitute a sibling checkout.
        resolve_module_root(REPOSITORY / SOURCE, "github.com/andenore/CopperLib",
                            locked=True, offline=args.offline or args.dry_run)
    except (OSError, ValueError, CopperScriptError) as exc:
        print(f"SETUP ERROR: {exc}", file=sys.stderr)
        return 2

    route = routing_command(REPOSITORY, output, cli, footprints)
    command = profiled_command(route, output / "routing.prof") if args.profile == "cprofile" else route
    fill = [str(cli), "pcb", "drc", "--refill-zones", "--save-board", "--severity-all",
            "--format", "json", "--output", str(output / "kicad-drc.json"),
            str(output / "board.kicad_pcb")]
    print(f"Output: {output}", flush=True)
    print(subprocess.list2cmdline(command), flush=True)
    print(subprocess.list2cmdline(fill), flush=True)
    if args.dry_run:
        return 0

    inputs = [SOURCE, SOURCE.parent / "copper.mod", SOURCE.parent / "copper.lock",
              SOURCE.parent / "mechanics/carrier.copper", Path("scripts/route_cm4.py")]
    manifest = {"started_utc": started.isoformat(), "status": "running", "command": command,
                "routing_command": route, "fill_command": fill, "fabrication_ready": False,
                "input_sha256": {str(p): hashlib.sha256((REPOSITORY / p).read_bytes()).hexdigest()
                                 for p in inputs}}
    tick = time.perf_counter()
    code = 2
    output.mkdir(parents=True, exist_ok=True)
    manifest_path = output / "run.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    try:
        code = workflow._run_logged(command, REPOSITORY, output / "routing.log")
        if code == 0:
            # route-board verifies disposable filled copper. Persist the fills and
            # check the actual deliverable, retaining every severity (no waivers).
            code = workflow._run_logged(fill, REPOSITORY, output / "fill.log")
            if code == 0 and not completed(output):
                code = 1
            if code == 0:
                manifest["filled_board_sha256"] = hashlib.sha256(
                    (output / "board.kicad_pcb").read_bytes()).hexdigest()
                manifest["native_drc_sha256"] = hashlib.sha256(
                    (output / "kicad-drc.json").read_bytes()).hexdigest()
    except KeyboardInterrupt:
        code = 130
    except (OSError, ValueError) as exc:
        manifest["error"] = str(exc)
        code = 2
    finally:
        manifest["profiling"] = workflow._save_performance(output, args.profile, code)
        manifest.update(status={0: "passed", 1: "unmet_gates", 130: "interrupted"}.get(code, "error"),
                        exit_code=code, elapsed_seconds=round(time.perf_counter() - tick, 3),
                        finished_utc=datetime.now(timezone.utc).isoformat())
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"CM4 routing exit code: {code}; outputs: {output}")
    return code


if __name__ == "__main__":
    raise SystemExit(main())

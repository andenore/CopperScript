"""Isolated ngspice execution, coverage gates, data exports, and visual reports."""
from __future__ import annotations

import csv
import hashlib
import json
import math
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from ..backends.ngspice import NgspiceBackend, circuit_manifest
from ..model import Board
from ..serializer import board_to_json
from .measure import evaluate, validate_dimensions
from .model import SimulationError, SimulationPlan
from .raw import parse_raw
from .report import render_report, require_reporting
from .result import normalize, represented
from .select import select_circuit


def save_json(path: Path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")


def _configuration(folder: Path) -> dict:
    init = folder / "initialization"
    init.mkdir(parents=True, exist_ok=True)
    (init / "spinit").write_text("* CopperScript controlled initialization; native models only.\nset num_threads=1\n", encoding="ascii")
    (init / ".spiceinit").write_text("* CopperScript controlled user initialization.\n", encoding="ascii")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("SPICE_", "NGSPICE_"))}
    env.update({"SPICE_SCRIPTS": str(init), "SPICE_USERINIT_DIR": str(init), "SPICE_ASCIIRAWFILE": "1", "LC_NUMERIC": "C"})
    package_root = str(Path(__file__).resolve().parents[2])
    env["PYTHONPATH"] = package_root + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    return env


def _execute(command: list[str], cwd: Path, timeout: float):
    options = {"cwd": cwd, "env": _configuration(cwd), "text": True, "encoding": "utf-8", "errors": "replace",
               "stdout": subprocess.PIPE, "stderr": subprocess.STDOUT}
    if os.name == "nt":
        options["creationflags"] = subprocess.CREATE_NO_WINDOW
    else:
        options["start_new_session"] = True
    try:
        process = subprocess.Popen(command, **options)
    except OSError as exc:
        raise SimulationError(f"cannot start ngspice: {exc}") from exc
    timed_out = False
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        if os.name == "nt":
            process.kill()
        else:
            os.killpg(process.pid, signal.SIGKILL)
        output, _ = process.communicate()
    return process.returncode, output, timed_out


def resolve_engine(executable: str | Path | None = None) -> tuple[Path, bool]:
    requested = str(executable or os.environ.get("NGSPICE", "ngspice"))
    resolved = shutil.which(requested)
    path = Path(resolved or requested).expanduser().resolve()
    if not path.is_file():
        raise SimulationError("ngspice was not found; install it or use --ngspice /path/to/ngspice (a Windows ngspice.dll is also supported)")
    return path, path.suffix.lower() == ".dll"


def _command(engine: Path, shared: bool, *, version=False):
    if shared:
        return [sys.executable, "-m", "pcbir.simulation.shared_worker", str(engine), *( ["--version"] if version else [])]
    return [str(engine), "--version"] if version else [str(engine), "-n", "-b", "-r", "run.raw", "-o", "run.log", "deck.cir"]


def engine_identity(engine: Path, shared: bool, timeout: float) -> dict:
    with tempfile.TemporaryDirectory(prefix="copperscript-engine-") as directory:
        code, output, timed_out = _execute(_command(engine, shared, version=True), Path(directory), min(timeout, 15))
    match = re.search(r"ngspice[-\s]+(\d+)(?:\.\d+)?", output, re.I)
    if code or timed_out or not match:
        raise SimulationError("could not identify ngspice version: " + output[-1500:])
    return {"engine": "ngspice", "major_version": int(match[1]), "mode": "isolated_shared_library" if shared else "batch",
            "path": str(engine), "sha256": hashlib.sha256(engine.read_bytes()).hexdigest(), "version_output": output.strip(),
            "initialization": "controlled spinit and .spiceinit; no inherited SPICE environment; native dialect"}


def export_simulation(board: Board, plan: SimulationPlan, output: Path) -> Path:
    circuits = [select_circuit(board, plan.for_case(case)) for case in plan.cases]
    for circuit in circuits:
        validate_dimensions(circuit.plan)
    manifest = NgspiceBackend(plan).generate(board)
    _prepare_output(output)
    _write_manifest(manifest, output)
    return output


def _prepare_output(output):
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        raise SimulationError(f"output directory must be empty: {output}; choose a new directory to avoid stale results")
    output.mkdir(parents=True, exist_ok=True)


def _write_manifest(manifest, output):
    for artifact in manifest.artifacts:
        path = output / artifact.name
        if not path.resolve().is_relative_to(output.resolve()):
            raise SimulationError("artifact path escapes output directory")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(artifact.content, encoding="utf-8", newline="\n")


def _export_waveforms(data, output, formats):
    folder = output / "data" / data.analysis.name / data.case
    folder.mkdir(parents=True, exist_ok=True)
    document = data.to_dict()
    for trace in data.traces.values():
        entry = document["traces"][trace.name]
        entry["magnitude"] = list(represented(trace, "magnitude"))
        entry["magnitude_dB"] = list(represented(trace, "dB")) if trace.unit == "1" else None
        entry["phase_deg"] = list(represented(trace, "phase")) if data.analysis.kind == "ac" else None
        entry["phase_unwrapped_deg"] = list(represented(trace, "phase", unwrap=True)) if data.analysis.kind == "ac" else None
        entry["undefined_dB"] = [v is not None and abs(v) == 0 for v in trace.values] if trace.unit == "1" else None
    if "json" in formats:
        save_json(folder / "waveforms.json", document)
    if "csv" in formats:
        header = [f"{data.axis_name}[{data.axis_unit}]"]
        for name, trace in data.traces.items():
            header += [f"{name}.real[{trace.unit}]", f"{name}.imaginary[{trace.unit}]", f"{name}.valid"]
            if data.analysis.kind == "ac":
                header += [f"{name}.magnitude[{trace.unit}]", f"{name}.phase[deg]", f"{name}.phase_unwrapped[deg]"]
                if trace.unit == "1":
                    header += [f"{name}.magnitude[dB]"]
        with (folder / "waveforms.csv").open("w", encoding="utf-8", newline="") as stream:
            writer = csv.writer(stream); writer.writerow(header)
            representations = {n: {rep: represented(t, rep, unwrap=rep == "phase_unwrapped") if rep != "phase_unwrapped" else represented(t, "phase", unwrap=True)
                                  for rep in ("magnitude", "phase", "phase_unwrapped", "dB")} for n, t in data.traces.items()}
            for index, coordinate in enumerate(data.axis):
                row = [coordinate]
                for name, trace in data.traces.items():
                    value = trace.values[index]
                    row += [value.real if value is not None else None, value.imag if value is not None else None, value is not None]
                    if data.analysis.kind == "ac":
                        row += [representations[name][r][index] for r in ("magnitude", "phase", "phase_unwrapped")]
                        if trace.unit == "1":
                            row += [representations[name]["dB"][index]]
                writer.writerow(row)


def run_simulation(board: Board, plan: SimulationPlan, output: str | Path, *, ngspice: str | Path | None = None,
                   timeout: float = 60, source_path: Path | None = None) -> dict:
    if not math.isfinite(timeout) or timeout <= 0:
        raise SimulationError("timeout must be positive and finite")
    output = Path(output).resolve()
    circuits = {case: select_circuit(board, plan.for_case(case)) for case in plan.cases}
    for circuit in circuits.values():
        validate_dimensions(circuit.plan)
    require_reporting()
    engine, shared = resolve_engine(ngspice)
    identity = engine_identity(engine, shared, timeout)
    for circuit in circuits.values():
        for model in circuit.models:
            if identity["major_version"] not in model.versions:
                raise SimulationError(f"model {model.model_id}: ngspice {identity['major_version']} is not in its qualified versions {model.versions}")
    manifest = NgspiceBackend(plan).generate(board)
    _prepare_output(output)
    _write_manifest(manifest, output)
    started = datetime.now(timezone.utc).isoformat()
    design = board_to_json(board)
    provenance = {"schema": "copperscript-simulation/v1", "started_utc": started, "engine": identity,
                  "board": board.name, "design_sha256": hashlib.sha256(design.encode()).hexdigest(),
                  "plan_sha256": hashlib.sha256(plan.document.encode()).hexdigest(),
                  "dependencies": [{"module_path": d.module_path, "version": d.version, "checksum": d.checksum} for d in board.dependencies],
                  "cases": {case: circuit_manifest(circuit) for case, circuit in circuits.items()}}
    if source_path:
        provenance["source"] = {"path": str(source_path.resolve()), "sha256": hashlib.sha256(source_path.read_bytes()).hexdigest()}
    save_json(output / "provenance.json", provenance)
    waveforms = []
    runs = []
    measurements = []
    formats = plan.data.get("outputs", {}).get("data", ["csv", "json"])
    for case, circuit in circuits.items():
        for analysis in circuit.plan.analyses:
            folder = output / "cases" / case / analysis.name
            command = _command(engine, shared)
            record = {"case": case, "analysis": analysis.name, "kind": analysis.kind, "status": "failed", "command": command}
            try:
                code, console, timed_out = _execute(command, folder, timeout)
                (folder / "console.log").write_text(console, encoding="utf-8")
                if not (folder / "run.log").exists():
                    (folder / "run.log").write_text(console, encoding="utf-8")
                record.update({"exit_code": code, "timed_out": timed_out})
                if timed_out or code:
                    raise SimulationError("simulator timed out" if timed_out else f"simulator exited with status {code}")
                try:
                    content = (folder / "run.raw").read_text(encoding="utf-8")
                except (OSError, UnicodeError) as exc:
                    raise SimulationError(f"missing or unreadable raw output: {exc}") from exc
                plots = parse_raw(content)
                if len(plots) != 1:
                    raise SimulationError("expected exactly one analysis in each simulator run")
                data = normalize(circuit, analysis, case, plots[0])
                waveforms.append(data)
                _export_waveforms(data, output, formats)
                measurements.extend(evaluate(c, data) for c in circuit.plan.data.get("checks", []) if c.get("analysis", analysis.name) == analysis.name)
                record["status"] = "completed"
                record["samples"] = len(data.axis)
                record["diagnostics"] = list(data.diagnostics)
            except (SimulationError, OSError) as exc:
                record["error"] = str(exc)
                for check in circuit.plan.data.get("checks", []):
                    if check.get("analysis", analysis.name) == analysis.name:
                        measurements.append({"name": check["name"], "case": case, "analysis": analysis.name, "probe": check["probe"],
                            "operation": check["operation"], "representation": check.get("representation", "real"),
                            "unit": "unknown", "value": None, "status": "incomplete", "reason": str(exc)})
            runs.append(record)
            save_json(folder / "process.json", record)
    status = ("failed" if any(r["status"] != "completed" for r in runs) or any(m["status"] == "failed" for m in measurements)
              else "incomplete" if any(m["status"] == "incomplete" for m in measurements)
              else "passed" if measurements else "completed")
    if any(data.diagnostics for data in waveforms) and status in {"passed", "completed"}:
        status = "incomplete"
    summary = {"schema": "copperscript-simulation/v1", "name": plan.name, "status": status, "runs": runs,
               "output_status": "complete", "report": str(output / "report.html"), "provenance": provenance}
    save_json(output / "measurements.json", measurements)
    save_json(output / "requirements.json", {"status": status, "checks": measurements})
    try:
        render_report(plan, waveforms, measurements, summary, output)
    except Exception as exc:
        summary["output_status"] = "failed"
        summary["output_error"] = str(exc)
        summary["status"] = "incomplete" if status in {"passed", "completed"} else status
        (output / "report.html").write_text("<!doctype html><meta charset='utf-8'><h1>Simulation report incomplete</h1><p>Graph rendering failed. Numerical data and logs are retained.</p>", encoding="utf-8")
    save_json(output / "summary.json", summary)
    artifacts = []
    for path in sorted(output.rglob("*")):
        if path.is_file():
            artifacts.append({"path": path.relative_to(output).as_posix(), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "bytes": path.stat().st_size})
    save_json(output / "artifacts.json", {"files": artifacts})
    return summary

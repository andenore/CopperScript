"""Benchmark ladder: route a fixed set of boards and compare completion and time.

Usage::

    copper bench bench/ladder.json --out build/bench/<name>
    copper bench bench/ladder.json --only valid,cm4 --baseline bench/baselines/legacy.json

The ladder file lists boards with their project directory, source file and
``route-board`` options. Project paths are relative to the ladder file, and
``${VAR}`` expands from the environment, for example
``${COPPER_BOARDS}/CopperAssetTracker``. A board whose project directory is
missing is reported as ``skipped``, so the ladder runs wherever the optional
consumer checkouts are absent.

Each run writes ``<out>/<board>/route-report.json``, ``board.kicad_pcb`` and
``route.log``. The aggregate goes to ``<out>/results.json`` and
``<out>/results.md``. Numbers come from the report's pipeline-independent
``summary`` block (see :mod:`pcbir.route_summary`).
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time


SCHEMA = "copperscript-bench/v1"
# A candidate regresses when it loses completion, gains errors or is this much slower.
SLOWDOWN_LIMIT = 1.25
SLOWDOWN_FLOOR_SECONDS = 30.0


def load_ladder(path: Path) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    boards = data.get("boards") if isinstance(data, dict) else None
    if not isinstance(boards, list) or not boards:
        raise ValueError(f"{path}: expected a non-empty 'boards' list")
    names = set()
    result = []
    for entry in boards:
        name = entry.get("name")
        if not name or name in names:
            raise ValueError(f"{path}: every board needs a unique name")
        names.add(name)
        project = Path(os.path.expandvars(entry.get("project", ".")))
        if not project.is_absolute():
            project = (path.parent / project).resolve()
        result.append({
            "name": name,
            "project": project,
            "source": entry.get("source", "board.copper"),
            "args": [os.path.expandvars(str(item)) for item in entry.get("args", [])],
            "timeout_s": float(entry.get("timeout_s", 1800)),
            "native": bool(entry.get("native", False)),
            "tags": list(entry.get("tags", [])),
        })
    return result


def run_board(board: dict, out: Path, *, kicad_cli: str | None = None,
              python: str = sys.executable, extra_args: tuple[str, ...] = ()) -> dict:
    directory = out / board["name"]
    record = {"name": board["name"], "project": str(board["project"]), "tags": board["tags"]}
    source = board["project"] / board["source"]
    if not source.is_file():
        return {**record, "status": "skipped", "reason": f"missing {source}"}
    directory.mkdir(parents=True, exist_ok=False)
    report = directory / "route-report.json"  # what `copper issues RUN_DIR` reads
    command = [python, "-m", "copperscript", "route-board", board["source"], *board["args"], *extra_args,
               "--report", str(report), "-o", str(directory / "board.kicad_pcb")]
    if kicad_cli and board["native"]:
        command += ["--verify-plane-fill", kicad_cli]
    started = time.perf_counter()
    status = "completed"
    with (directory / "route.log").open("w", encoding="utf-8") as log:
        log.write(" ".join(command) + "\n")
        log.flush()
        try:
            code = subprocess.run(command, cwd=board["project"], stdout=log, stderr=subprocess.STDOUT,
                                  timeout=board["timeout_s"]).returncode
        except subprocess.TimeoutExpired:
            code, status = None, "timeout"
    elapsed = round(time.perf_counter() - started, 1)
    record.update(exit_code=code, elapsed_s=elapsed, command=command[2:])
    if status == "timeout":
        return {**record, "status": status}
    if code not in (0, 1) or not report.is_file():
        return {**record, "status": "error", "reason": _last_line(directory / "route.log")}
    document = json.loads(report.read_text(encoding="utf-8"))
    summary = document.get("summary")
    if summary is None:
        return {**record, "status": "error", "reason": "report has no summary block"}
    return {**record, "status": status, "route_status": document.get("status"), "summary": summary,
            "phases": _phase_seconds(document)}


def _phase_seconds(document: dict) -> dict:
    timings = document.get("timings") or {}
    return {key: value for key, value in timings.items() if isinstance(value, (int, float))}


def _last_line(path: Path) -> str:
    try:
        lines = [line.strip() for line in path.read_text(encoding="utf-8", errors="replace").splitlines()
                 if line.strip()]
    except OSError:
        return ""
    return lines[-1][:300] if lines else ""


def _row(result: dict) -> dict:
    summary = result.get("summary") or {}
    signal = summary.get("signal", {})
    return {
        "status": result["status"],
        "completion": signal.get("completion"),
        "opens": signal.get("opens"),
        "connections": signal.get("connections"),
        "plane_opens": summary.get("plane", {}).get("estimated_opens",
                                                    summary.get("plane", {}).get("explicit_opens")),
        "drc_errors": summary.get("drc", {}).get("errors"),
        "native_unconnected": summary.get("native", {}).get("unconnected"),
        "vias": summary.get("copper", {}).get("vias"),
        "length_mm": summary.get("copper", {}).get("track_length_mm"),
        "elapsed_s": result.get("elapsed_s"),
    }


def compare(baseline: dict, candidate: dict) -> list[str]:
    """Return human-readable regressions of ``candidate`` against ``baseline``."""

    old = {item["name"]: _row(item) for item in baseline.get("results", [])}
    problems = []
    for item in candidate.get("results", []):
        name, new = item["name"], _row(item)
        before = old.get(name)
        if before is None or before["status"] == "skipped" or new["status"] == "skipped":
            continue
        if new["status"] != "completed":
            if before["status"] == "completed":
                problems.append(f"{name}: run {new['status']} (baseline completed)")
            continue
        if before["status"] != "completed":
            continue
        if (new["completion"] or 0) < (before["completion"] or 0):
            problems.append(f"{name}: completion {before['completion']} -> {new['completion']}")
        if (new["drc_errors"] or 0) > (before["drc_errors"] or 0):
            problems.append(f"{name}: exact DRC errors {before['drc_errors']} -> {new['drc_errors']}")
        if (before["native_unconnected"] is not None and new["native_unconnected"] is not None
                and new["native_unconnected"] > before["native_unconnected"]):
            problems.append(f"{name}: native unconnected {before['native_unconnected']} -> {new['native_unconnected']}")
        slow = before["elapsed_s"] or 0
        if (new["elapsed_s"] or 0) > max(slow * SLOWDOWN_LIMIT, slow + SLOWDOWN_FLOOR_SECONDS):
            problems.append(f"{name}: time {slow}s -> {new['elapsed_s']}s")
    return problems


def markdown(document: dict, problems: list[str] | None = None) -> str:
    lines = [f"# Benchmark ladder: {document['label']}", "",
             f"CopperScript `{document.get('copperscript_commit') or 'unknown'}`, "
             f"{document['started_utc']}", "",
             "| Board | Status | Completion | Opens / connections | Plane opens (est.) | DRC errors | Native unconnected | Vias | Length mm | Time s |",
             "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for item in document["results"]:
        row = _row(item)
        def cell(value):
            return "–" if value is None else str(value)
        opens = "–" if row["opens"] is None else f"{row['opens']} / {row['connections']}"
        lines.append(f"| {item['name']} | {row['status']} | {cell(row['completion'])} | {opens} | "
                     f"{cell(row['plane_opens'])} | {cell(row['drc_errors'])} | {cell(row['native_unconnected'])} | "
                     f"{cell(row['vias'])} | {cell(row['length_mm'])} | {cell(row['elapsed_s'])} |")
    if problems is not None:
        lines += ["", "## Regressions against baseline", ""]
        lines += [f"- {item}" for item in problems] or ["None."]
    return "\n".join(lines) + "\n"


def _commit() -> str | None:
    root = Path(__file__).resolve().parent.parent
    try:
        return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=root, capture_output=True,
                              text=True, timeout=10).stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="copper bench", description=__doc__.split("\n\n")[0])
    parser.add_argument("ladder", type=Path)
    parser.add_argument("--out", type=Path, required=True, help="new directory for this run")
    parser.add_argument("--only", help="comma-separated board names")
    parser.add_argument("--tags", help="comma-separated tags; run boards having any of them")
    parser.add_argument("--label", default=None)
    parser.add_argument("--baseline", type=Path, help="results.json to compare against")
    parser.add_argument("--kicad-cli", help="enable native refill/DRC for boards marked native")
    parser.add_argument("--python", default=sys.executable,
                        help="interpreter whose CopperScript runs the boards (default: this one)")
    parser.add_argument("--extra-arg", action="append", default=[], dest="extra_args",
                        help="append one route-board argument to every board (repeatable)")
    args = parser.parse_args(argv)
    boards = load_ladder(args.ladder)
    if args.only:
        wanted = set(args.only.split(","))
        unknown = wanted - {board["name"] for board in boards}
        if unknown:
            parser.error(f"unknown boards: {', '.join(sorted(unknown))}")
        boards = [board for board in boards if board["name"] in wanted]
    if args.tags:
        tags = set(args.tags.split(","))
        boards = [board for board in boards if tags & set(board["tags"])]
    args.out.mkdir(parents=True, exist_ok=True)
    document = {"schema": SCHEMA, "label": args.label or args.out.name,
                "started_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "copperscript_commit": _commit(), "ladder": str(args.ladder), "results": []}
    for board in boards:
        print(f"[bench] {board['name']} ...", flush=True)
        result = run_board(board, args.out, kicad_cli=args.kicad_cli, python=args.python,
                           extra_args=tuple(args.extra_args))
        row = _row(result)
        print(f"[bench] {board['name']}: {result['status']} completion={row['completion']} "
              f"opens={row['opens']} drc={row['drc_errors']} time={row['elapsed_s']}s", flush=True)
        document["results"].append(result)
        (args.out / "results.json").write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    problems = None
    if args.baseline:
        problems = compare(json.loads(args.baseline.read_text(encoding="utf-8")), document)
    (args.out / "results.md").write_text(markdown(document, problems), encoding="utf-8")
    print((args.out / "results.md").read_text(encoding="utf-8"))
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())

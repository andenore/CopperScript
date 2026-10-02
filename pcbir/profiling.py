"""Operational profiling helpers, independent of IR, routing and signoff."""
from __future__ import annotations

import argparse
import cProfile
import io
import json
from pathlib import Path
import pstats
import runpy
import sys


def profiled_command(command: list[str], output: Path) -> list[str]:
    """Wrap a Python module launch without changing its arguments/exit status."""
    if command[1:4] != ["-u", "-m", "copperscript"]:
        raise ValueError("expected the unbuffered CopperScript module command")
    return [command[0], "-u", "-m", "pcbir.profiling", "--output", str(output),
            "--module", command[3], "--", *command[4:]]


def profile_summary(profile: Path, *, limit: int = 50) -> tuple[dict, str]:
    """Bounded JSON and text views; raw stats retain the complete call graph."""
    stream = io.StringIO()
    stats = pstats.Stats(str(profile), stream=stream)
    rows = [{"file": file, "line": line, "function": function,
             "primitive_calls": primitive, "calls": calls,
             "self_seconds": own, "cumulative_seconds": cumulative}
            for (file, line, function), (primitive, calls, own, cumulative, _) in stats.stats.items()]
    def ranked(field):
        return sorted(rows, key=lambda row: (-row[field], row["file"], row["line"], row["function"]))[:limit]
    result = {"backend": "cProfile", "total_calls": stats.total_calls,
              "primitive_calls": stats.prim_calls, "total_self_seconds": stats.total_tt,
              "top_self": ranked("self_seconds"), "top_cumulative": ranked("cumulative_seconds"),
              "note": "Cumulative times overlap; do not sum them. Instrumented times are not benchmarks."}
    stream.write("SELF TIME (excludes callees)\n")
    stats.sort_stats(pstats.SortKey.TIME).print_stats(limit)
    stream.write("CUMULATIVE TIME (includes callees; overlapping)\n")
    stats.sort_stats(pstats.SortKey.CUMULATIVE).print_stats(limit)
    return result, stream.getvalue()


def phase_summary(log: Path) -> dict:
    """Pair streamed phase events; retain unfinished spans instead of inventing durations."""
    active: dict[str, list[dict]] = {}
    intervals = []
    malformed = 0
    events = 0
    with log.open(encoding="utf-8") as stream:
        for line in stream:
            if not line.startswith("PROGRESS "):
                continue
            try:
                item = json.loads(line[len("PROGRESS "):])
                phase, event, elapsed = item["phase"], item["event"], item["elapsed_seconds"]
                if not isinstance(phase, str) or not isinstance(elapsed, (int, float)):
                    raise ValueError("invalid progress event")
            except (ValueError, TypeError, KeyError):
                malformed += 1
                continue
            events += 1
            if event == "started":
                active.setdefault(phase, []).append(item)
            elif event == "finished" and active.get(phase):
                start = active[phase].pop()
                intervals.append({"phase": phase, "start_seconds": start["elapsed_seconds"],
                                  "finish_seconds": elapsed,
                                  "duration_seconds": round(elapsed - start["elapsed_seconds"], 3),
                                  "started_details": start.get("details", {}),
                                  "finished_details": item.get("details", {})})
    return {"events": events, "malformed_events": malformed, "intervals": intervals,
            "unfinished": [item for stack in active.values() for item in stack],
            "note": "Inclusive elapsed spans overlap across nested phases; do not sum all intervals. "
                    "Phases without start/finish pairs have no duration. Not signoff evidence."}


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Profile a module while preserving SystemExit.")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--module", required=True)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    remaining = args.arguments[1:] if args.arguments[:1] == ["--"] else args.arguments
    sys.argv = [args.module, *remaining]
    profiler = cProfile.Profile()
    try:
        profiler.runcall(runpy.run_module, args.module, run_name="__main__", alter_sys=True)
    finally:
        # The stdlib cProfile CLI swallows SystemExit. Do not use it here:
        # exit 1 must still mean unmet gates. Force-killed processes may save nothing.
        try:
            profiler.dump_stats(str(args.output))
        except Exception as exc:
            print(f"PROFILE ERROR: {exc}", file=sys.stderr)


if __name__ == "__main__":
    main()

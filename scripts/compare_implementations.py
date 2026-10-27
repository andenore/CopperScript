"""Run the same commands on the Python CLI and the Rust binary and compare outputs.

The Python CLI (``python -m copperscript``) is the oracle for the Rust rewrite
(docs/rust-port-plan.md). The corpus, ``scripts/compare_corpus.json``, lists
suites of commands and the milestone that rewrites each one. Suites up to the
corpus's ``accepted_milestone`` are gated: every case must produce identical
output, or a difference recorded in docs/rust-port-differences.md.

    python scripts/compare_implementations.py --rust target/release/copper
    python scripts/compare_implementations.py --list --suite all
    python scripts/compare_implementations.py --suite ladder --oracle-when-pending --report build/compare/ladder.json

Each case runs both implementations in turn with the same working directory
and the same output directory, so paths in their output agree. The harness
compares the exit status, stdout, stderr and every output file byte for byte
(a suite may narrow this), and records both run times. A Rust command that
reports it is not implemented yet is ``pending``; its oracle run is skipped
unless ``--oracle-when-pending`` asks for the Python timing anyway.

The ``ladder`` suite expands ``bench/ladder.json`` (via ``pcbir.ladder``) into
one ``route-board`` case per board: this is the ladder timing hook.
"""

from __future__ import annotations

import argparse
import difflib
import fnmatch
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "scripts" / "compare_corpus.json"
DIFFERENCES = ROOT / "docs" / "rust-port-differences.md"
REPORT_SCHEMA = "copperscript.compare-report/v1"
CORPUS_SCHEMA = "copperscript.compare-corpus/v1"
# Must match crates/copper/src/commands.rs.
NOT_IMPLEMENTED_EXIT = 3
NOT_IMPLEMENTED_MARKER = b"not implemented in the Rust rewrite"
CHANNELS = ("exit", "stdout", "stderr", "files")
DEFAULT_TIMEOUT_S = 600.0
# Deterministic oracle runs: UTF-8 streams (the Python 3.15 default) and a fixed hash seed.
ENVIRONMENT = {"PYTHONUTF8": "1", "PYTHONHASHSEED": "0"}


@dataclass
class Case:
    id: str
    suite: str
    milestone: str
    args: list[str]
    cwd: Path
    timeout_s: float
    compare: tuple[str, ...]
    files: tuple[str, ...] | None = None
    json_keys: dict[str, list[str]] = field(default_factory=dict)
    skip_reason: str | None = None


def milestone_number(name: str) -> int:
    match = re.fullmatch(r"M(\d+)", name)
    if not match:
        raise ValueError(f"milestone must look like M0, M1, ...: {name!r}")
    return int(match.group(1))


def _declares(path: Path, kind: str) -> bool:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return False
    return re.search(rf"^\s*{re.escape(kind)}\s+[A-Za-z_]", text, re.MULTILINE) is not None


def _inputs(patterns: list[str], declares: str | None) -> list[str]:
    found = set()
    for pattern in patterns:
        for match in glob.glob(str(ROOT / pattern), recursive=True):
            path = Path(match)
            if path.is_file() and (declares is None or _declares(path, declares)):
                found.add(path.relative_to(ROOT).as_posix())
    return sorted(found)


def _expand(template: list[str], values: dict[str, object]) -> list[str]:
    args = []
    for item in template:
        if item.startswith("{") and item.endswith("}") and isinstance(values.get(item[1:-1]), list):
            args.extend(values[item[1:-1]])
            continue
        for key, value in values.items():
            if isinstance(value, str):
                item = item.replace("{" + key + "}", value)
        args.append(item)
    return args


def _ladder_cases(suite: dict, base: dict, kicad_cli: str | None) -> list[Case]:
    sys.path.insert(0, str(ROOT))
    from pcbir.ladder import load_ladder

    cases = []
    for board in load_ladder(ROOT / suite["ladder"]):
        args = ["route-board", board["source"], *board["args"],
                "--report", "{out}/route-report.json", "-o", "{out}/board.kicad_pcb"]
        if kicad_cli and board["native"]:
            args += ["--verify-plane-fill", kicad_cli]
        project = Path(board["project"])
        skip = None if (project / board["source"]).is_file() else f"missing {project / board['source']}"
        cases.append(Case(id=f"ladder/{board['name']}", args=args, cwd=project, timeout_s=board["timeout_s"],
                          skip_reason=skip, **base))
    return cases


def load_corpus(path: Path, kicad_cli: str | None = None) -> tuple[dict, list[Case]]:
    corpus = json.loads(path.read_text(encoding="utf-8"))
    if corpus.get("schema") != CORPUS_SCHEMA:
        raise ValueError(f"{path}: expected schema {CORPUS_SCHEMA}")
    boards = corpus.get("boards", {})
    cases: list[Case] = []
    for suite in corpus["suites"]:
        name, milestone = suite["name"], suite["milestone"]
        milestone_number(milestone)
        base = {"suite": name, "milestone": milestone, "compare": tuple(suite.get("compare", CHANNELS)),
                "files": tuple(suite["files"]) if "files" in suite else None, "json_keys": suite.get("json", {})}
        timeout = float(suite.get("timeout_s", DEFAULT_TIMEOUT_S))
        if "ladder" in suite:
            cases.extend(_ladder_cases(suite, base, kicad_cli))
            continue
        for case in suite.get("cases", []):
            case_id = case.get("id") or f"{name}/" + " ".join(case["args"][1:] or ["(default)"])
            cases.append(Case(id=case_id, args=list(case["args"]), cwd=ROOT,
                              timeout_s=float(case.get("timeout_s", timeout)), **base))
        for source in _inputs(suite.get("inputs", []), suite.get("declares")):
            cases.append(Case(id=f"{name}/{source}", args=_expand(suite["args"], {"input": source}), cwd=ROOT,
                              timeout_s=timeout, **base))
        selected = suite.get("boards")
        if selected:
            names = sorted(boards) if selected == "*" else selected
            for board in names:
                values = {"input": boards[board]["source"], "physical": boards[board].get("physical", [])}
                cases.append(Case(id=f"{name}/{board}", args=_expand(suite["args"], values), cwd=ROOT,
                                  timeout_s=timeout, **base))
    seen = set()
    for case in cases:
        if case.id in seen:
            raise ValueError(f"{path}: duplicate case id {case.id!r}")
        seen.add(case.id)
    return corpus, cases


def load_differences(path: Path) -> list[dict]:
    """Rows of the recorded-differences table: id, case and both result digests."""
    entries = []
    if not path.is_file():
        return entries
    for line in path.read_text(encoding="utf-8").splitlines():
        cells = [cell.strip().strip("`") for cell in line.strip().strip("|").split("|")]
        if len(cells) >= 6 and re.fullmatch(r"D-\d{4}", cells[0]):
            entries.append({"id": cells[0], "case": cells[1], "python": cells[2], "rust": cells[3],
                            "milestone": cells[4], "reason": cells[5]})
    return entries


def _sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _collect_files(directory: Path) -> dict[str, bytes]:
    return {path.relative_to(directory).as_posix(): path.read_bytes()
            for path in sorted(directory.rglob("*")) if path.is_file()}


def _json_at(data: bytes, keys: list[str]):
    try:
        value = json.loads(data.decode("utf-8"))
        for key in keys:
            value = value[key]
        return value
    except (UnicodeDecodeError, ValueError, KeyError, TypeError, IndexError):
        return None


@dataclass
class Run:
    exit: int | None
    seconds: float
    stdout: bytes
    stderr: bytes
    files: dict[str, bytes]
    timed_out: bool = False

    def pending(self) -> bool:
        return self.exit == NOT_IMPLEMENTED_EXIT and NOT_IMPLEMENTED_MARKER in self.stderr


def run_one(command: list[str], case: Case, out: Path, environment: dict[str, str]) -> Run:
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    args = _expand(case.args, {"out": str(out)})
    started = time.perf_counter()
    try:
        completed = subprocess.run([*command, *args], cwd=case.cwd, capture_output=True, env=environment,
                                   timeout=case.timeout_s)
        code, stdout, stderr, timed_out = completed.returncode, completed.stdout, completed.stderr, False
    except subprocess.TimeoutExpired as expired:
        code, stdout, stderr, timed_out = None, expired.stdout or b"", expired.stderr or b"", True
    seconds = round(time.perf_counter() - started, 3)
    return Run(code, seconds, stdout, stderr, _collect_files(out), timed_out)


def observed(run: Run, case: Case) -> dict:
    """The compared parts of a run, as digests."""
    parts: dict[str, object] = {}
    if "exit" in case.compare:
        parts["exit"] = run.exit
    if "stdout" in case.compare:
        parts["stdout"] = _sha256(run.stdout)
    if "stderr" in case.compare:
        parts["stderr"] = _sha256(run.stderr)
    if "files" in case.compare:
        names = sorted(run.files) if case.files is None else list(case.files)
        parts["files"] = {name: _sha256(run.files[name]) if name in run.files else None for name in names}
    for name, keys in case.json_keys.items():
        value = _json_at(run.files[name], keys) if name in run.files else None
        parts[f"json:{name}#{'.'.join(keys)}"] = _sha256(json.dumps(value, sort_keys=True).encode())
    return parts


def result_digest(parts: dict) -> str:
    return _sha256(json.dumps(parts, sort_keys=True, separators=(",", ":")).encode())


def _excerpt(name: str, expected: bytes, actual: bytes, limit: int = 20) -> list[str]:
    try:
        left, right = expected.decode("utf-8").splitlines(), actual.decode("utf-8").splitlines()
    except UnicodeDecodeError:
        return [f"  {name}: binary content differs ({len(expected)} vs {len(actual)} bytes)"]
    diff = list(difflib.unified_diff(left, right, f"python/{name}", f"rust/{name}", lineterm="", n=1))
    if not diff and expected != actual:
        diff = [f"  {name}: same lines, different bytes (line endings or trailing newline)"]
    return ["  " + line for line in diff[:limit]] + (["  ..."] if len(diff) > limit else [])


def _differences(python: Run, rust: Run, case: Case) -> tuple[list[str], list[str]]:
    left, right = observed(python, case), observed(rust, case)
    names, excerpt = [], []
    for key in sorted(set(left) | set(right)):
        if left.get(key) == right.get(key):
            continue
        if key == "files":
            for name in sorted(set(left[key]) | set(right[key])):
                if left[key].get(name) != right[key].get(name):
                    names.append(f"files:{name}")
                    excerpt += _excerpt(name, python.files.get(name, b""), rust.files.get(name, b""))
        else:
            names.append(key)
            if key in ("stdout", "stderr"):
                excerpt += _excerpt(key, getattr(python, key), getattr(rust, key))
            elif key == "exit":
                excerpt.append(f"  exit: python {python.exit}, rust {rust.exit}")
    return names, excerpt


def _summary(run: Run | None) -> dict | None:
    if run is None:
        return None
    return {"exit": run.exit, "seconds": run.seconds, "timed_out": run.timed_out,
            "stdout_sha256": _sha256(run.stdout), "stderr_sha256": _sha256(run.stderr),
            "files": {name: _sha256(data) for name, data in run.files.items()}}


def compare_case(case: Case, rust_command: list[str], python_command: list[str], work: Path,
                 environment: dict[str, str], differences: list[dict], oracle_when_pending: bool,
                 keep: Path | None) -> tuple[dict, list[str]]:
    record = {"id": case.id, "suite": case.suite, "milestone": case.milestone, "args": case.args,
              "cwd": case.cwd.relative_to(ROOT).as_posix() if case.cwd.is_relative_to(ROOT) else str(case.cwd)}
    if case.skip_reason:
        return {**record, "status": "skipped", "reason": case.skip_reason}, []
    slug = re.sub(r"[^A-Za-z0-9._-]+", "_", case.id)
    out = work / slug / "out"
    rust = run_one(rust_command, case, out, environment)
    stash = work / slug / "rust"
    shutil.move(str(out), stash)
    if rust.pending() and not oracle_when_pending:
        return {**record, "status": "pending", "rust": _summary(rust),
                "reason": rust.stderr.decode("utf-8", "replace").strip()}, []
    python = run_one(python_command, case, out, environment)
    shutil.move(str(out), work / slug / "python")
    record.update(python=_summary(python), rust=_summary(rust))
    if python.timed_out or python.exit is None:
        return {**record, "status": "error", "reason": "the Python oracle timed out"}, []
    if rust.pending():
        return {**record, "status": "pending", "reason": rust.stderr.decode("utf-8", "replace").strip()}, []
    digests = {"python": result_digest(observed(python, case)), "rust": result_digest(observed(rust, case))}
    record["digests"] = digests
    if digests["python"] == digests["rust"]:
        return {**record, "status": "equal"}, []
    names, excerpt = _differences(python, rust, case)
    record["differences"] = names
    for entry in differences:
        if entry["case"] == case.id and entry["python"] == digests["python"] and entry["rust"] == digests["rust"]:
            return {**record, "status": "accepted", "accepted_by": entry["id"]}, []
    if keep:
        for name, run in (("python", python), ("rust", rust)):
            target = keep / slug / name
            target.mkdir(parents=True, exist_ok=True)
            (target / "stdout").write_bytes(run.stdout)
            (target / "stderr").write_bytes(run.stderr)
            for file_name, data in run.files.items():
                (target / "files" / file_name).parent.mkdir(parents=True, exist_ok=True)
                (target / "files" / file_name).write_bytes(data)
    return {**record, "status": "different"}, excerpt


def _commit() -> str | None:
    try:
        return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True,
                              timeout=10).stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def markdown(report: dict) -> str:
    lines = [f"# Rust/Python comparison ({report['platform']}, commit {report.get('commit') or 'unknown'})", "",
             f"Gate: suites up to {report['gate_milestone']}. "
             f"Result: {'pass' if report['passed'] else 'FAIL'}.", "",
             "| Suite | Milestone | Gated | Equal | Accepted | Different | Pending | Skipped | Error | Python s | Rust s |",
             "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for suite in report["suites"]:
        counts = suite["counts"]
        lines.append(f"| {suite['name']} | {suite['milestone']} | {'yes' if suite['gated'] else 'no'} | "
                     + " | ".join(str(counts.get(key, 0)) for key in
                                  ("equal", "accepted", "different", "pending", "skipped", "error"))
                     + f" | {suite['python_seconds']:.2f} | {suite['rust_seconds']:.2f} |")
    ladder = [case for case in report["cases"] if case["suite"] == "ladder"]
    if ladder:
        lines += ["", "## Ladder timing", "", "| Board | Status | Python s | Rust s |", "|---|---|---:|---:|"]
        for case in ladder:
            python = (case.get("python") or {}).get("seconds")
            rust = (case.get("rust") or {}).get("seconds")
            lines.append(f"| {case['id'].split('/', 1)[1]} | {case['status']} | "
                         f"{'-' if python is None else python} | {'-' if rust is None else rust} |")
    bad = [case for case in report["cases"] if case["status"] in ("different", "error") or
           (case["gated"] and case["status"] == "pending")]
    if bad:
        lines += ["", "## Cases needing attention", ""]
        for case in bad:
            detail = ", ".join(case.get("differences", [])) or case.get("reason", "")
            digests = case.get("digests")
            lines.append(f"- `{case['id']}` ({case['status']}): {detail}"
                         + (f"; python `{digests['python']}`, rust `{digests['rust']}`" if digests else ""))
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    default_rust = ROOT / "target" / "release" / ("copper.exe" if os.name == "nt" else "copper")
    parser.add_argument("--rust", type=Path, default=default_rust, help=f"Rust binary (default {default_rust})")
    parser.add_argument("--python", default=sys.executable, help="interpreter whose CopperScript is the oracle")
    parser.add_argument("--corpus", type=Path, default=CORPUS)
    parser.add_argument("--differences", type=Path, default=DIFFERENCES)
    parser.add_argument("--suite", action="append", default=[],
                        help="suite to run (repeatable; 'all' for every suite); default: the gated suites")
    parser.add_argument("--case", action="append", default=[], help="glob on case ids (repeatable)")
    parser.add_argument("--milestone", help="gate suites up to this milestone (default: the corpus's accepted one)")
    parser.add_argument("--list", action="store_true", help="print the enumerated cases and stop")
    parser.add_argument("--report", type=Path, help="write the JSON report here")
    parser.add_argument("--markdown", type=Path, help="write the Markdown summary here")
    parser.add_argument("--keep", type=Path, help="keep both outputs of every differing case under this directory")
    parser.add_argument("--oracle-when-pending", action="store_true",
                        help="run the Python oracle even when the Rust command is not implemented (timing)")
    parser.add_argument("--kicad-cli", help="pass --verify-plane-fill to native ladder boards")
    args = parser.parse_args(argv)

    corpus, cases = load_corpus(args.corpus, args.kicad_cli)
    gate = args.milestone or corpus["accepted_milestone"]
    gate_number = milestone_number(gate)
    suites = {case.suite for case in cases}
    selected = set(args.suite) if args.suite and "all" not in args.suite else (suites if args.suite else
               {case.suite for case in cases if milestone_number(case.milestone) <= gate_number})
    unknown = selected - suites
    if unknown:
        parser.error(f"unknown suites: {', '.join(sorted(unknown))}")
    cases = [case for case in cases if case.suite in selected and
             (not args.case or any(fnmatch.fnmatch(case.id, pattern) for pattern in args.case))]
    if args.list:
        for case in cases:
            gated = "gated" if milestone_number(case.milestone) <= gate_number else "     "
            print(f"{case.milestone} {gated} {case.id}  ->  copper {' '.join(case.args)}"
                  + (f"  [skipped: {case.skip_reason}]" if case.skip_reason else ""))
        print(f"{len(cases)} cases in {len({case.suite for case in cases})} suites")
        return 0

    rust_binary = args.rust.resolve()
    if os.name == "nt" and rust_binary.suffix.lower() != ".exe" and not rust_binary.exists():
        rust_binary = rust_binary.with_suffix(".exe")
    if not rust_binary.is_file():
        parser.error(f"Rust binary not found: {rust_binary} (cargo build --release)")
    environment = {**os.environ, **ENVIRONMENT}
    differences = load_differences(args.differences)
    started = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    records = []
    with tempfile.TemporaryDirectory(prefix="copper-compare-") as temporary:
        work = Path(temporary)
        for case in cases:
            record, excerpt = compare_case(case, [str(rust_binary)], [args.python, "-m", "copperscript"], work,
                                           environment, differences, args.oracle_when_pending, args.keep)
            record["gated"] = milestone_number(case.milestone) <= gate_number
            records.append(record)
            timing = " ".join(f"{name} {record[name]['seconds']}s" for name in ("python", "rust")
                              if record.get(name))
            print(f"{record['status']:<9} {case.id}" + (f"  ({timing})" if timing else ""), flush=True)
            for line in excerpt:
                print(line)
    suite_rows = []
    for name in dict.fromkeys(record["suite"] for record in records):
        rows = [record for record in records if record["suite"] == name]
        counts: dict[str, int] = {}
        for record in rows:
            counts[record["status"]] = counts.get(record["status"], 0) + 1
        seconds = {impl: round(sum((record.get(impl) or {}).get("seconds") or 0 for record in rows), 3)
                   for impl in ("python", "rust")}
        suite_rows.append({"name": name, "milestone": rows[0]["milestone"], "gated": rows[0]["gated"],
                           "counts": counts, "python_seconds": seconds["python"], "rust_seconds": seconds["rust"]})
    failing = [record for record in records if record["status"] in ("different", "error")
               or (record["gated"] and record["status"] == "pending")]
    report = {"schema": REPORT_SCHEMA, "started_utc": started, "commit": _commit(), "platform": sys.platform,
              "python": sys.version.split()[0], "rust_binary": str(rust_binary), "corpus": str(args.corpus),
              "gate_milestone": gate, "passed": not failing, "suites": suite_rows, "cases": records}
    summary = markdown(report)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    if args.markdown:
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        args.markdown.write_text(summary, encoding="utf-8")
    print()
    print(summary, end="")
    return 0 if not failing else 1


if __name__ == "__main__":
    raise SystemExit(main())

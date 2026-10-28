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
(a suite may narrow this), and records both run times. The Rust binary writes
LF line endings on every platform; on Windows the Python outputs' CRLF is read
as LF before comparing (and the Rust outputs' too, for formats such as CSV
that use CRLF on every platform). ``{root}`` in case arguments is the
checkout, for paths a command resolves from another working directory. A Rust command that
reports it is not implemented yet is ``pending``; its oracle run is skipped
unless ``--oracle-when-pending`` asks for the Python timing anyway.

The ``ladder`` suite expands ``bench/ladder.json`` (via ``pcbir.ladder``) into
one ``route-board`` case per board: this is the ladder timing hook.

Python results are cached under ``build/oracle-cache``, keyed by everything an
oracle run can depend on (``oracle_digest``: the non-Rust tree, the
interpreter and its packages, the platform and checkout path) and by the case.
A change to the Rust side alone therefore reruns only the binary;
``--no-oracle-cache`` runs Python for every case. Ladder cases always run it.
One comparison runs per checkout at a time (``build/compare.lock``), because a
run writes seeded mutations beside their sources.

A suite's ``json`` option compares parts of a JSON output file instead of
(or besides) its bytes, mapping the file name to one key path
(``["summary"]``), several key paths (``[["status"], ["drc"]]``), or
``{"without": [paths]}``: the whole document with those paths removed, for
documents carrying wall time. A ``"*"`` step in a path stands for every item
of a list (or value of an object).

A case may name a client transcript as its ``stdin`` (the ``lsp`` suites):
JSON lines, each a message to ``send`` (framed here with Content-Length),
``raw`` bytes, or ``wait`` for N complete messages on stdout, so debounced
server notifications arrive in a deterministic order. Strings may use
``{root_uri}``, and in a suite with ``inputs``, ``{input}``, ``{input_uri}``
and ``{input_text}``. The transcript's digest is part of the oracle cache key.
LSP output is binary on both implementations (CRLF headers by protocol), so
its stdout is compared without the Windows CRLF reading.

A case may name an ``http`` client transcript for a server that prints its
loopback URL (``edit-mechanical``): JSON lines of ``get``/``post`` requests,
replayed in order once the URL appears; the responses are written to
``{out}/http.jsonl`` with the random port and capability token masked (in
stdout too), then the server is terminated. See ``_run_http``.

A suite or case may ``stage`` inputs: a map of output-relative names to
repository paths (files or directories) copied into the fresh output directory
before each implementation runs, for commands that rewrite their inputs in
place (``editor-overlay`` binds the overlay into the sibling ``run.json``;
the editor saves source). Both implementations then start from identical
copies and the staged tree is compared like any other output.
"""

from __future__ import annotations

import argparse
import concurrent.futures
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
import threading
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
    json_keys: dict[str, list] = field(default_factory=dict)
    skip_reason: str | None = None
    # (repository-relative source, seed): run on a seeded mutation of the source.
    mutation: tuple[str, int] | None = None
    # A client transcript piped to stdin, and the suite input it is about.
    stdin: str | None = None
    stdin_input: str | None = None
    # Output-relative name -> repository path copied into the output directory first.
    stage: dict[str, str] = field(default_factory=dict)
    # An HTTP client transcript for a server that prints its URL (the editor).
    http: str | None = None


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


_TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*|\d+(?:\.\d+)?")


def mutate_source(text: str, seed: int) -> str:
    """A deterministic small edit that keeps most of the source valid.

    Line deletions, duplications and swaps, and identifier or number swaps
    reach the compiler's semantic checks rather than only the parser.
    """
    import random
    rng = random.Random(seed)
    lines = text.split("\n")
    kind = rng.choice(["delete", "duplicate", "swap", "identifier", "number"])
    if kind in {"delete", "duplicate", "swap"} and len(lines) > 2:
        index = rng.randrange(1, len(lines) - 1)
        if kind == "delete":
            del lines[index]
        elif kind == "duplicate":
            lines.insert(index, lines[index])
        else:
            other = rng.randrange(1, len(lines) - 1)
            lines[index], lines[other] = lines[other], lines[index]
        return "\n".join(lines)
    tokens = list(_TOKEN.finditer(text))
    wanted = [m for m in tokens if (m.group()[0].isdigit()) == (kind == "number")]
    if len(wanted) < 2:
        return text[:-1]
    target, replacement = rng.choice(wanted), rng.choice(wanted).group()
    return text[:target.start()] + replacement + text[target.end():]


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
                "files": tuple(suite["files"]) if "files" in suite else None, "json_keys": suite.get("json", {}),
                "stage": dict(suite.get("stage", {}))}
        timeout = float(suite.get("timeout_s", DEFAULT_TIMEOUT_S))
        if "ladder" in suite:
            cases.extend(_ladder_cases(suite, base, kicad_cli))
            continue
        for case in suite.get("cases", []):
            case_id = case.get("id") or f"{name}/" + " ".join(case["args"][1:] or ["(default)"])
            # A case may narrow the compared channels, e.g. to exit status
            # for a usage error, whose argparse wording is not reproduced.
            channels = {"compare": tuple(case["compare"])} if "compare" in case else {}
            stage = {"stage": {**base["stage"], **case["stage"]}} if "stage" in case else {}
            cases.append(Case(id=case_id, args=list(case["args"]), cwd=ROOT,
                              timeout_s=float(case.get("timeout_s", timeout)), stdin=case.get("stdin"),
                              http=case.get("http"), **{**base, **channels, **stage}))
        for source in _inputs(suite.get("inputs", []), suite.get("declares")):
            stdin = suite.get("stdin")
            cases.append(Case(id=f"{name}/{source}", args=_expand(suite["args"], {"input": source}), cwd=ROOT,
                              timeout_s=timeout, stdin=stdin, stdin_input=source if stdin else None, **base))
            for number in range(int(suite.get("mutations", 0))):
                seed = int(hashlib.sha256(f"{source}#{number}".encode()).hexdigest()[:12], 16)
                mutated = (Path(source).parent / f".compare-mutation-{number}{Path(source).suffix}").as_posix()
                cases.append(Case(id=f"{name}/{source}#m{number}", args=_expand(suite["args"], {"input": mutated}),
                                  cwd=ROOT, timeout_s=timeout, mutation=(source, seed), stdin=stdin,
                                  stdin_input=mutated if stdin else None, **base))
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
        return _json_path(json.loads(data.decode("utf-8")), keys)
    except (UnicodeDecodeError, ValueError, KeyError, TypeError, IndexError):
        return None


def _json_path(value, keys: list[str]):
    for index, key in enumerate(keys):
        if key == "*":
            items = value.values() if isinstance(value, dict) else value
            return [_json_path(item, keys[index + 1:]) for item in items]
        value = value[key]
    return value


def _json_without(value, keys: list[str]):
    """``value`` with the item at ``keys`` removed (``"*"`` over every item)."""
    if not keys or not isinstance(value, (dict, list)):
        return value
    key, rest = keys[0], keys[1:]
    if key == "*":
        if isinstance(value, dict):
            return {name: _json_without(item, rest) for name, item in value.items()}
        return [_json_without(item, rest) for item in value]
    if not isinstance(value, dict) or key not in value:
        return value
    if not rest:
        return {name: item for name, item in value.items() if name != key}
    return {**value, key: _json_without(value[key], rest)}


def _json_parts(name: str, data: bytes | None, spec) -> dict[str, object]:
    """The compared digests of one ``json`` entry: see the module documentation."""
    if isinstance(spec, dict):
        try:
            value = json.loads(data.decode("utf-8")) if data is not None else None
        except (UnicodeDecodeError, ValueError):
            value = None
        for keys in spec["without"]:
            value = _json_without(value, keys)
        label = ",".join(".".join(keys) for keys in spec["without"])
        return {f"json:{name}#without:{label}": _sha256(json.dumps(value, sort_keys=True).encode())}
    paths = spec if spec and isinstance(spec[0], list) else [spec]
    return {f"json:{name}#{'.'.join(keys)}":
            _sha256(json.dumps(_json_at(data, keys) if data is not None else None, sort_keys=True).encode())
            for keys in paths}


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

    def with_lf(self, stdout: bool = True) -> "Run":
        """The run with `\r\n` as `\n`: the Rust binary writes LF text on every
        platform, while Python's text mode writes CRLF on Windows. LSP stdout
        (``stdout=False``) is binary framing with CRLF headers on both sides."""
        lf = lambda data: data.replace(b"\r\n", b"\n")
        return Run(self.exit, self.seconds, lf(self.stdout) if stdout else self.stdout, lf(self.stderr),
                   {name: lf(data) for name, data in self.files.items()}, self.timed_out)


_PLACEHOLDER = re.compile(r"\{(root_uri|input|input_uri|input_text)\}")


def transcript_steps(case: Case) -> list[bytes | int]:
    """The case's client transcript: bytes to write, or a count of complete
    server messages to wait for."""
    values = {"root_uri": ROOT.as_uri()}
    if case.stdin_input:
        path = ROOT / case.stdin_input
        values.update(input=case.stdin_input, input_uri=path.resolve().as_uri())
        with path.open(encoding="utf-8", newline="") as stream:
            values["input_text"] = stream.read()

    def expand(value):
        if isinstance(value, str):
            return _PLACEHOLDER.sub(lambda match: values[match.group(1)], value)
        if isinstance(value, list):
            return [expand(item) for item in value]
        if isinstance(value, dict):
            return {key: expand(item) for key, item in value.items()}
        return value

    steps: list[bytes | int] = []
    for line in (ROOT / case.stdin).read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        step = expand(json.loads(line))
        if "send" in step:
            body = json.dumps(step["send"], ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            steps.append(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        elif "raw" in step:
            steps.append(step["raw"].encode("utf-8"))
        else:
            steps.append(int(step["wait"]))
    return steps


def _complete_messages(data: bytes) -> int:
    count, index = 0, 0
    while (split := data.find(b"\r\n\r\n", index)) >= 0:
        match = re.search(rb"Content-Length: *(\d+)", data[index:split], re.IGNORECASE)
        end = split + 4 + int(match.group(1)) if match else split + 4
        if end > len(data):
            break
        count, index = count + 1, end
    return count


def _run_transcript(argv: list[str], case: Case, environment: dict[str, str]) -> tuple:
    """Feed the transcript, waiting where it says so; (exit, stdout, stderr, timed_out)."""
    deadline = time.monotonic() + case.timeout_s
    process = subprocess.Popen(argv, cwd=case.cwd, env=environment, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = {"stdout": bytearray(), "stderr": bytearray()}

    def drain(name: str) -> None:
        stream = getattr(process, name)
        while chunk := stream.read1(65536):
            output[name].extend(chunk)

    readers = [threading.Thread(target=drain, args=(name,), daemon=True) for name in output]
    for reader in readers:
        reader.start()
    timed_out = False
    try:
        for step in transcript_steps(case):
            if isinstance(step, int):
                while _complete_messages(bytes(output["stdout"])) < step and process.poll() is None:
                    if time.monotonic() >= deadline:
                        # A wait the server never satisfies is a failed run, not a slow one.
                        timed_out = True
                        raise TimeoutError(f"waiting for message {step}")
                    time.sleep(0.01)
                continue
            process.stdin.write(step)
            process.stdin.flush()
    except TimeoutError:
        process.kill()
    except OSError:
        pass
    try:
        process.stdin.close()
    except OSError:
        pass
    try:
        process.wait(timeout=max(0.1, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        timed_out = True
    for reader in readers:
        reader.join()
    return (None if timed_out else process.returncode), bytes(output["stdout"]), bytes(output["stderr"]), timed_out


_EDITOR_URL = re.compile(rb"http://127\.0\.0\.1:(\d+)/#token=([A-Za-z0-9_-]+)")


def _mask_http(value, port: str, token: str):
    """Replace the server's random port and token; drop placement job timings."""
    if isinstance(value, str):
        return value.replace(token, "{token}").replace(f"127.0.0.1:{port}", "127.0.0.1:{port}")
    if isinstance(value, list):
        return [_mask_http(item, port, token) for item in value]
    if isinstance(value, dict):
        masked = {key: _mask_http(item, port, token) for key, item in value.items()}
        if isinstance(masked.get("elapsed_seconds"), (int, float)) and "input_revision" in masked:
            masked["elapsed_seconds"] = None
            masked["profile"] = None
        return masked
    return value


def _run_http(argv: list[str], case: Case, out: Path, environment: dict[str, str]) -> tuple:
    """Start a server that prints its URL, replay ``case.http`` and stop it.

    JSON lines, each a request: ``{"get": PATH}`` or ``{"post": PATH}`` with a
    ``json`` value or ``raw`` body text, optional ``headers`` (added, or
    replacing the defaults: ``Host``, ``X-Copper-Token`` and for a body
    ``Content-Type: application/json``; a null value removes one), and
    ``"token": false`` to send no capability. Each response is recorded in
    ``{out}/http.jsonl`` as its status, ``Content-Type``, ``Cache-Control``
    and JSON body (other bodies by SHA-256), with the random port and token
    masked; stdout masks them too. The server is then terminated.
    """
    import http.client
    deadline = time.monotonic() + case.timeout_s
    process = subprocess.Popen(argv, cwd=case.cwd, env=environment, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = {"stdout": bytearray(), "stderr": bytearray()}

    def drain(name: str) -> None:
        stream = getattr(process, name)
        while chunk := stream.read1(65536):
            output[name].extend(chunk)

    readers = [threading.Thread(target=drain, args=(name,), daemon=True) for name in output]
    for reader in readers:
        reader.start()
    timed_out, port, token, records, revision = False, None, None, [], 0

    def with_revision(value):
        if value == "{revision}":
            return revision
        if isinstance(value, list):
            return [with_revision(item) for item in value]
        if isinstance(value, dict):
            return {key: with_revision(item) for key, item in value.items()}
        return value

    while process.poll() is None and time.monotonic() < deadline:
        match = _EDITOR_URL.search(bytes(output["stdout"]))
        if match:
            port, token = match.group(1).decode(), match.group(2).decode()
            break
        time.sleep(0.01)
    if port is None and process.poll() is None:
        timed_out = True
    if port is not None:
        for line in (ROOT / case.http).read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            step = json.loads(line)
            method = "GET" if "get" in step else "POST"
            path = step.get("get", step.get("post"))
            body = (json.dumps(with_revision(step["json"])).encode("utf-8") if "json" in step
                    else step["raw"].encode("utf-8") if "raw" in step else None)
            headers = {"Host": f"127.0.0.1:{port}"}
            if step.get("token", True):
                headers["X-Copper-Token"] = token
            if body is not None:
                headers["Content-Type"] = "application/json"
            for key, value in step.get("headers", {}).items():
                if value is None:
                    headers.pop(key, None)
                else:
                    headers[key] = value.replace("{port}", port)
            connection = http.client.HTTPConnection("127.0.0.1", int(port),
                                                    timeout=max(1.0, deadline - time.monotonic()))
            try:
                connection.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
                for key, value in headers.items():
                    connection.putheader(key, value)
                if body is not None:
                    connection.putheader("Content-Length", str(len(body)))
                connection.endheaders(body)
                response = connection.getresponse()
                data = response.read()
                record = {"status": response.status, "content_type": response.getheader("Content-Type"),
                          "cache_control": response.getheader("Cache-Control")}
                try:
                    parsed = json.loads(data)
                    record["json"] = _mask_http(parsed, port, token)
                    if isinstance(parsed, dict):
                        latest = parsed.get("scene") or parsed.get("preview") or parsed
                        if isinstance(latest, dict) and isinstance(latest.get("revision"), int):
                            revision = latest["revision"]
                except ValueError:
                    record["sha256"] = _sha256(data)
            except (OSError, http.client.HTTPException) as exc:
                record = {"error": type(exc).__name__}
            finally:
                connection.close()
            records.append(record)
            if time.monotonic() >= deadline:
                timed_out = True
                break
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=max(1.0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        timed_out = True
    for reader in readers:
        reader.join()
    (out / "http.jsonl").write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in records),
                                    encoding="utf-8", newline="\n")
    stdout = bytes(output["stdout"])
    if port is not None:
        stdout = stdout.replace(token.encode(), b"{token}").replace(f"127.0.0.1:{port}".encode(), b"127.0.0.1:{port}")
    # A terminated server's exit status is the signal, the same for both.
    return (None if timed_out else process.returncode), stdout, bytes(output["stderr"]), timed_out


def run_one(command: list[str], case: Case, out: Path, environment: dict[str, str]) -> Run:
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    for name, source in sorted(case.stage.items()):
        target = out / name
        if (ROOT / source).is_dir():
            shutil.copytree(ROOT / source, target)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / source, target)
    args = _expand(case.args, {"out": str(out), "root": str(ROOT)})
    started = time.perf_counter()
    if case.http:
        code, stdout, stderr, timed_out = _run_http([*command, *args], case, out, environment)
        return Run(code, round(time.perf_counter() - started, 3), stdout, stderr, _collect_files(out), timed_out)
    if case.stdin:
        code, stdout, stderr, timed_out = _run_transcript([*command, *args], case, environment)
        return Run(code, round(time.perf_counter() - started, 3), stdout, stderr, _collect_files(out), timed_out)
    try:
        completed = subprocess.run([*command, *args], cwd=case.cwd, capture_output=True, env=environment,
                                   timeout=case.timeout_s)
        code, stdout, stderr, timed_out = completed.returncode, completed.stdout, completed.stderr, False
    except subprocess.TimeoutExpired as expired:
        code, stdout, stderr, timed_out = None, expired.stdout or b"", expired.stderr or b"", True
    seconds = round(time.perf_counter() - started, 3)
    return Run(code, seconds, stdout, stderr, _collect_files(out), timed_out)


# Paths whose content cannot change a Python oracle result.
_RUST_ONLY = ("crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "docs", ".github",
              "scripts/compare_corpus.json")


def oracle_digest(python: str) -> str:
    """Everything a Python oracle run can depend on, except the case itself.

    The tracked non-Rust tree (with the content of local edits and untracked
    files), the interpreter and its installed distributions, the platform and
    the checkout path, which absolute paths in messages expose. Package caches
    are content-addressed, so their directory names already pin their bytes.
    """
    exclude = [f":(exclude){path}" for path in _RUST_ONLY]
    git = lambda *args: subprocess.run(["git", *args, "--", ".", *exclude], cwd=ROOT, capture_output=True,
                                       check=True).stdout
    digest = hashlib.sha256(git("ls-files", "-s"))
    changed = git("ls-files", "-m", "-o", "--exclude-standard").decode("utf-8").splitlines()
    for name in sorted(set(changed)):
        path = ROOT / name
        digest.update(name.encode() + b"\0" + (path.read_bytes() if path.is_file() else b"<deleted>"))
    versions = subprocess.run([python, "-c", "import sys, importlib.metadata as m; print(sys.version); "
                               "print(sorted(f'{d.metadata[\"Name\"]}=={d.version}' for d in m.distributions()))"],
                              capture_output=True, check=True).stdout
    digest.update(versions + f"{sys.platform}|{ROOT}|{sorted(ENVIRONMENT.items())}".encode())
    return digest.hexdigest()


class OracleCache:
    """Python oracle results by case, valid while `oracle_digest` is unchanged."""

    def __init__(self, directory: Path, digest: str):
        self.directory = directory / digest[:20]
        self.hits = 0

    def _path(self, case: Case) -> Path:
        parts = [case.id, case.args, str(case.cwd), case.timeout_s, case.mutation, sorted(case.stage.items())]
        if case.stdin:
            parts.append(_sha256((ROOT / case.stdin).read_bytes()))
        if case.http:
            parts.append(_sha256((ROOT / case.http).read_bytes()))
        key = json.dumps(parts)
        return self.directory / hashlib.sha256(key.encode()).hexdigest()

    def load(self, case: Case) -> Run | None:
        path = self._path(case)
        try:
            meta = json.loads((path / "run.json").read_text(encoding="utf-8"))
            files = {name: (path / "files" / name).read_bytes() for name in meta["files"]}
            run = Run(meta["exit"], meta["seconds"], (path / "stdout").read_bytes(), (path / "stderr").read_bytes(),
                      files, meta["timed_out"])
        except (OSError, ValueError, KeyError):
            return None
        self.hits += 1
        return run

    def store(self, case: Case, run: Run) -> None:
        path = self._path(case)
        partial = path.with_name(path.name + f".{os.getpid()}.{threading.get_ident()}")
        shutil.rmtree(partial, ignore_errors=True)
        (partial / "files").mkdir(parents=True)
        (partial / "stdout").write_bytes(run.stdout)
        (partial / "stderr").write_bytes(run.stderr)
        for name, data in run.files.items():
            (partial / "files" / name).parent.mkdir(parents=True, exist_ok=True)
            (partial / "files" / name).write_bytes(data)
        (partial / "run.json").write_text(json.dumps({"exit": run.exit, "seconds": run.seconds, "files": sorted(run.files),
                                                      "timed_out": run.timed_out}), encoding="utf-8")
        shutil.rmtree(path, ignore_errors=True)
        try:
            partial.rename(path)
        except OSError:
            shutil.rmtree(partial, ignore_errors=True)


class CheckoutLock:
    """One comparison per checkout: a run writes seeded mutations beside their
    sources (package caches included), which another run would read."""

    def __init__(self, path: Path):
        self.path = path

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            descriptor = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            raise SystemExit(f"another comparison is running in this checkout ({self.path}); "
                             "delete the file if it is stale") from None
        os.write(descriptor, str(os.getpid()).encode())
        os.close(descriptor)
        return self

    def __exit__(self, *exc):
        self.path.unlink(missing_ok=True)


def oracle_crash(stderr: bytes) -> bytes | None:
    """The final exception line of an uncaught Python exception, else None.

    A traceback names absolute paths and source lines, so its digest differs
    per checkout and platform; the exception line alone is stable, which lets
    a reviewed difference for an oracle crash match on every platform.
    """
    marker = b"Traceback (most recent call last):"
    if marker not in stderr:
        return None
    lines = [line for line in stderr.splitlines() if line.strip()]
    return b"python crash: " + (lines[-1].strip() if lines else b"")


def observed(run: Run, case: Case) -> dict:
    """The compared parts of a run, as digests."""
    parts: dict[str, object] = {}
    if "exit" in case.compare:
        parts["exit"] = run.exit
    if "stdout" in case.compare:
        parts["stdout"] = _sha256(run.stdout)
    if "stderr" in case.compare:
        crash = oracle_crash(run.stderr)
        parts["stderr"] = _sha256(run.stderr) if crash is None else crash.decode("utf-8", "replace")
    if "files" in case.compare:
        names = sorted(run.files) if case.files is None else list(case.files)
        parts["files"] = {name: _sha256(run.files[name]) if name in run.files else None for name in names}
    for name, spec in case.json_keys.items():
        parts.update(_json_parts(name, run.files.get(name), spec))
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
            "stdout_head": run.stdout[:160].decode("utf-8", "replace"),
            "stdout_sha256": _sha256(run.stdout), "stderr_sha256": _sha256(run.stderr),
            "files": {name: _sha256(data) for name, data in run.files.items()}}


def compare_case(case: Case, rust_command: list[str], python_command: list[str], work: Path,
                 environment: dict[str, str], differences: list[dict], oracle_when_pending: bool,
                 keep: Path | None, cache: OracleCache | None = None) -> tuple[dict, list[str]]:
    record = {"id": case.id, "suite": case.suite, "milestone": case.milestone, "args": case.args,
              "cwd": case.cwd.relative_to(ROOT).as_posix() if case.cwd.is_relative_to(ROOT) else str(case.cwd)}
    if case.skip_reason:
        return {**record, "status": "skipped", "reason": case.skip_reason}, []
    slug = re.sub(r"[^A-Za-z0-9._-]+", "_", case.id)
    out = work / slug / "out"
    mutated = None
    if case.mutation is not None:
        source, seed = case.mutation
        original = (ROOT / source).read_text(encoding="utf-8")
        target = case.stdin_input or next(arg for arg in case.args if Path(arg).name.startswith(".compare-mutation-"))
        mutated = ROOT / target
        mutated.write_bytes(mutate_source(original, seed).encode("utf-8"))
    try:
        return _compare_runs(case, record, rust_command, python_command, work, slug, out, environment,
                             differences, oracle_when_pending, keep, cache)
    finally:
        if mutated is not None:
            mutated.unlink(missing_ok=True)


def _compare_runs(case, record, rust_command, python_command, work, slug, out, environment, differences,
                  oracle_when_pending, keep, cache):
    rust = run_one(rust_command, case, out, environment)
    stash = work / slug / "rust"
    shutil.move(str(out), stash)
    if rust.pending() and not oracle_when_pending:
        return {**record, "status": "pending", "rust": _summary(rust),
                "reason": rust.stderr.decode("utf-8", "replace").strip()}, []
    python = cache.load(case) if cache else None
    if python is None:
        python = run_one(python_command, case, out, environment)
        if cache and not python.timed_out:
            cache.store(case, python)
    else:
        out.mkdir(parents=True, exist_ok=True)
    if os.name == "nt":
        # Formats that end lines with CRLF everywhere (CSV) do so in both.
        python, rust = python.with_lf(stdout=not case.stdin), rust.with_lf(stdout=not case.stdin)
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
    parser.add_argument("--oracle-cache", type=Path, default=ROOT / "build" / "oracle-cache",
                        help="reuse Python oracle results while its sources and environment are unchanged")
    parser.add_argument("--no-oracle-cache", action="store_true", help="always run the Python oracle")
    parser.add_argument("--print-oracle-digest", action="store_true",
                        help="print the oracle cache digest (for a CI cache key) and stop")
    parser.add_argument("--jobs", type=int, default=1,
                        help="cases to run at once (default 1; timings are only comparable with 1)")
    args = parser.parse_args(argv)

    if args.print_oracle_digest:
        print(oracle_digest(args.python))
        return 0
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
    results: dict[int, dict] = {}
    printing = threading.Lock()
    # Cases on one example's tree run in one group, in order: a seeded
    # mutation is written next to its source, where a sibling package import
    # of the same tree would otherwise inventory it.
    groups: dict[str, list[int]] = {}
    for index, case in enumerate(cases):
        tree = next((arg for arg in (*case.args, case.stdin_input or "") if arg.startswith(("examples/", "tests/"))),
                    None)
        key = "/".join(Path(tree).parts[:2]) if tree else case.id
        groups.setdefault(key, []).append(index)
    with CheckoutLock(ROOT / "build" / "compare.lock"):
        # Ladder timings measure the oracle itself, so its cases always run it.
        cache = None if args.no_oracle_cache else OracleCache(args.oracle_cache, oracle_digest(args.python))
        # A fixed work directory per checkout: output paths appear in messages,
        # so a cached oracle result must name the same directory as a fresh run.
        work = Path(tempfile.gettempdir()) / f"copper-compare-{hashlib.sha256(str(ROOT).encode()).hexdigest()[:12]}"
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir(parents=True)
        try:

            def run_group(indexes: list[int]) -> None:
                for index in indexes:
                    case = cases[index]
                    record, excerpt = compare_case(case, [str(rust_binary)], [args.python, "-m", "copperscript"], work,
                                                   environment, differences, args.oracle_when_pending, args.keep,
                                                   None if case.suite == "ladder" else cache)
                    record["gated"] = milestone_number(case.milestone) <= gate_number
                    results[index] = record
                    timing = " ".join(f"{name} {record[name]['seconds']}s" for name in ("python", "rust")
                                      if record.get(name))
                    with printing:
                        print(f"{record['status']:<9} {case.id}" + (f"  ({timing})" if timing else ""), flush=True)
                        for line in excerpt:
                            print(line)

            with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
                for future in [pool.submit(run_group, indexes) for indexes in groups.values()]:
                    future.result()
        finally:
            shutil.rmtree(work, ignore_errors=True)
    records = [results[index] for index in range(len(cases))]
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
    if cache:
        print(f"Oracle cache: {cache.hits} of {len(records)} cases reused ({cache.directory})")
    print(summary, end="")
    return 0 if not failing else 1


if __name__ == "__main__":
    raise SystemExit(main())

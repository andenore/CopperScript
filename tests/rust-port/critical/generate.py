"""Regenerate the critical-stage input checkpoints of the replay-critical suites.

Runs the Python CLI's ``route-board BOARD --routing-profile quick
--checkpoint-dir DIR`` on every board with critical routing rules and keeps
the ``critical-inputs.state.json`` it writes as ``NAME.state.json`` here.
The rest of the routing run is not needed, so it is stopped as soon as that
checkpoint exists (see README.md). A board whose run ends without writing
one is reported and skipped.

``bad/`` holds inputs derived from those checkpoints for the error paths,
``derived/`` valid inputs for result paths no checkpoint reaches: each is a
small, documented edit of a real state, re-digested so that it reaches the
check it is about (``derive()``).

    python tests/rust-port/critical/generate.py [NAME ...]
    python tests/rust-port/critical/generate.py --derived
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
WORK = ROOT / "build" / "critical-fixtures"
ENVIRONMENT = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONUTF8": "1"}
PROXY = ["--locked", "--offline", "--allow-proxy-footprints", "--no-check"]
SIX = ["--layers", "6", "--fab-profile", "jlcpcb-six-layer"]
FOUR = ["--layers", "4", "--fab-profile", "jlcpcb-four-layer"]
TIMEOUT_S = 1800

# name: (board, route-board options before --routing-profile quick --checkpoint-dir)
BOARDS = {
    "routing": ("tests/rust-port/export-kicad-pcb/routing.copper", ["--allow-proxy-footprints", "--no-check", *SIX]),
    "si-dphy": ("tests/rust-port/si-check/dphy.copper", [*PROXY, *SIX]),
    "si-four-layer": ("tests/rust-port/si-check/four_layer.copper", [*PROXY, *FOUR]),
    "si-layer-groups": ("tests/rust-port/si-check/layer_groups.copper", [*PROXY, *SIX]),
    "si-no-stackup": ("tests/rust-port/si-check/no_stackup.copper", [*PROXY, *SIX]),
    "si-reference-planes": ("tests/rust-port/si-check/reference_planes.copper", [*PROXY, *SIX]),
    "si-single-ended": ("tests/rust-port/si-check/single_ended.copper", [*PROXY, *SIX]),
    "si-two-layer": ("tests/rust-port/si-check/two_layer.copper", [*PROXY, "--layers", "2"]),
    "full-vertical": ("examples/full_vertical/board.copper", ["--locked", "--no-check", *SIX]),
    "nrf-antenna-macro": ("examples/nrf_antenna_macro/board.copper", ["--locked", "--offline", "--no-check", *SIX]),
}


def checkpoint(name: str, board: str, options: list[str]) -> Path | None:
    work = WORK / name
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    target = work / "critical-inputs.state.json"
    command = [sys.executable, "-m", "copperscript", "route-board", board, *options,
               "--routing-profile", "quick", "--checkpoint-dir", str(work),
               "--report", str(work / "route-report.json")]
    log = (work / "route-board.log").open("w", encoding="utf-8")
    process = subprocess.Popen(command, cwd=ROOT, env=ENVIRONMENT, stdout=log, stderr=subprocess.STDOUT)
    started = time.monotonic()
    try:
        # save_state writes the file in one call: wait until it is complete.
        while process.poll() is None and time.monotonic() - started < TIMEOUT_S:
            if target.is_file() and target.read_bytes().endswith(b"\n"):
                break
            time.sleep(1)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        log.close()
    if not (target.is_file() and target.read_bytes().endswith(b"\n")):
        print(f"{name}: no critical-inputs checkpoint (exit {process.returncode}); see {work / 'route-board.log'}")
        return None
    kept = HERE / f"{name}.state.json"
    shutil.copyfile(target, kept)
    print(f"{name}: {kept.relative_to(ROOT)} ({kept.stat().st_size} bytes)")
    return kept


VALID = ROOT / "tests" / "rust-port" / "reports" / "states" / "critical-inputs.state.json"
BAD = HERE / "bad"


def _digest(value: object) -> str:
    body = json.dumps(value, sort_keys=True, separators=(",", ":"))
    return "sha256:" + hashlib.sha256(body.encode()).hexdigest()


def _state(document: dict, value: object) -> str:
    """``document`` with ``value`` and its digest, written as save_state writes."""
    rest = {"content_digest": _digest(value), "kind": document["kind"], "metadata": document["metadata"],
            "value": value}
    return '{"schema":"copperscript.state/v1",' + json.dumps(rest, sort_keys=True, separators=(",", ":"))[1:] + "\n"


def _entry(value: dict, key: str) -> dict:
    return next(item for name, item in value["$map"] if name == key)


def _rule(board: dict, net: str) -> dict:
    return next(rule["fields"] for rule in board["fields"]["net_routing_rules"]["$tuple"]
                if rule["fields"]["net"] == net)


def derive() -> None:
    """Write the derived error-path inputs under ``bad/``."""
    BAD.mkdir(exist_ok=True)
    valid = json.loads(VALID.read_text(encoding="utf-8"))
    routing = json.loads((HERE / "routing.state.json").read_text(encoding="utf-8"))
    macro = json.loads((HERE / "nrf-antenna-macro.state.json").read_text(encoding="utf-8"))
    files: dict[str, str | bytes] = {
        "not-a-state.json": '{"schema": "copperscript.state/v0", "value": null}\n',
        "invalid-json.state.json": '{"schema": "copperscript.state/v1",\n',
        "empty.state.json": "",
        "not-utf8.state.json": b'{"schema": "\xff"}\n',
        "digest-mismatch.state.json": VALID.read_text(encoding="utf-8").replace('"content_digest":"sha256:3',
                                                                               '"content_digest":"sha256:4', 1),
        "foreign-type.state.json": _state(valid, {"$type": "os:system", "fields": {}}),
        "plain-number.state.json": _state(valid, 42),
    }
    # The dict without its global route.
    value = copy.deepcopy(valid["value"])
    value["$map"] = [entry for entry in value["$map"] if entry[0] != "global_route"]
    files["no-global-route.state.json"] = _state(valid, value)
    # A board field PhysicalBoard does not declare (a StateError).
    value = copy.deepcopy(valid["value"])
    _entry(value, "board")["fields"]["bogus"] = 1
    files["unknown-board-field.state.json"] = _state(valid, value)
    # An unknown copper layer in a global guide (an enum ValueError).
    value = copy.deepcopy(valid["value"])
    route = next(item for item in _entry(value, "global_route")["fields"]["routes"]["$tuple"]
                 if item["fields"]["segments"]["$tuple"])
    route["fields"]["segments"]["$tuple"][0]["fields"]["layer"]["value"] = "X.Cu"
    files["bad-guide-layer.state.json"] = _state(valid, value)
    # A pair rule whose partner names another net (the symmetric-rule result).
    value = copy.deepcopy(routing["value"])
    _rule(_entry(value, "board"), "D_P")["differential_partner"] = "GND"
    (HERE / "derived").mkdir(exist_ok=True)
    (HERE / "derived" / "asymmetric-pair.state.json").write_bytes(_state(routing, value).encode("utf-8"))
    # Hard-macro guides from another placement.
    value = copy.deepcopy(macro["value"])
    _entry(value, "global_route")["fields"]["placement_fingerprint"] = "0" * 64
    files["stale-macro-guides.state.json"] = _state(macro, value)
    # Package-access reservations (the FanoutResult path).
    value = copy.deepcopy(valid["value"])
    for entry in value["$map"]:
        if entry[0] == "reserved_accesses":
            entry[1] = {"$type": "pcbir.fanout:FanoutResult", "fields": {}}
    files["reserved-accesses.state.json"] = _state(valid, value)
    for name, content in files.items():
        path = BAD / name
        if isinstance(content, bytes):
            path.write_bytes(content)
        else:
            path.write_bytes(content.encode("utf-8"))
        print(f"derived {path.relative_to(ROOT)}")


def main(names: list[str]) -> int:
    if names == ["--derived"]:
        derive()
        return 0
    unknown = set(names) - set(BOARDS)
    if unknown:
        print("unknown boards: " + ", ".join(sorted(unknown)))
        return 2
    for name, (board, options) in BOARDS.items():
        if not names or name in names:
            checkpoint(name, board, options)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

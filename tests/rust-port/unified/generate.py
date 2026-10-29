"""Regenerate the unified-router input states of the Rust comparison corpus.

    python tests/rust-port/unified/generate.py [NAME ...]

For every board below it runs ``python -m copperscript route-board`` with
``--unified-save-inputs`` and keeps the saved state as
``tests/rust-port/unified/<name>.state.json`` (and the ball-grid fanout
sidecar ``<name>.state.json.bga.json`` when the run writes one). The route
itself is written to a scratch directory and discarded. Every run is
deterministic (no time budget; ``PYTHONHASHSEED=0``); a second run writes the
same bytes. Placement-feedback trial states (``.trial-N``) are dropped.
``malformed/`` holds hand-made variants of the valid board's
state (``malformed()``), each breaking one decoding rule with a recomputed
content digest unless the digest is the point. The exact commands are listed
in README.md. The Python
implementation is the oracle, so this uses it directly.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
PROXY = ["--locked", "--offline", "--allow-proxy-footprints"]
BOARDS = {
    "valid_board": ["examples/valid_board/board.copper", *PROXY],
    "routing": ["tests/rust-port/export-kicad-pcb/routing.copper", *PROXY],
    "placement": ["tests/rust-port/export-kicad-pcb/placement.copper", *PROXY],
    "six_layer": ["tests/rust-port/export-kicad-pcb/six_layer.copper", *PROXY, "--layers", "6",
                  "--fab-profile", "jlcpcb-six-layer"],
    "zones": ["tests/rust-port/export-kicad-pcb/zones.copper", *PROXY, "--layers", "4"],
    "layer_groups": ["tests/rust-port/si-check/layer_groups.copper", *PROXY, "--layers", "6",
                     "--fab-profile", "jlcpcb-six-layer"],
    "dphy": ["tests/rust-port/si-check/dphy.copper", *PROXY, "--layers", "6", "--fab-profile", "jlcpcb-six-layer"],
    # The benchmark ladder's arguments (bench/ladder.json); round_led_ring resolves footprints online.
    "round_led_ring": ["examples/round_led_ring/board.copper", "--locked", "--layers", "2",
                       "--fab-profile", "generic", "--candidates", "1", "--feedback-iterations", "1",
                       "--router-iterations", "5", "--fanout", "--plane-contact-radius-mm", "5"],
    "nrf52": ["examples/nrf52_coin_cell/board.copper", "--locked", "--offline", "--layers", "6",
              "--fab-profile", "jlcpcb-six-layer", "--width-mm", "50", "--height-mm", "40"],
    # A committed 2x2 via-in-pad array (real footprints from the board's own library).
    "via_array": ["tests/rust-port/export-kicad-pcb/via_array.copper", "--locked", "--offline", "--layers", "6",
                  "--fab-profile", "jlcpcb-six-layer"],
}


def generate(name: str) -> None:
    board, *options = BOARDS[name]
    state = HERE / f"{name}.state.json"
    for stale in (state, Path(f"{state}.bga.json"), *HERE.glob(f"{name}.state.json.trial-*")):
        stale.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        command = [sys.executable, "-m", "copperscript", "route-board", str(ROOT / board), *options,
                   "-o", str(Path(scratch) / "routed.kicad_pcb"), "--report", str(Path(scratch) / "report.json"),
                   "--unified-save-inputs", str(state)]
        environment = {**os.environ, "PYTHONUTF8": "1", "PYTHONHASHSEED": "0"}
        completed = subprocess.run(command, cwd=scratch, env=environment, capture_output=True, text=True)
        if not state.exists():
            sys.exit(f"{name}: route-board saved no state (exit {completed.returncode})\n"
                     f"{completed.stdout[-2000:]}{completed.stderr[-2000:]}")
    # Placement-feedback trials save their own inputs beside the state; the
    # corpus keeps the state of the selected placement only.
    for trial in HERE.glob(f"{name}.state.json.trial-*"):
        trial.unlink()
    print(f"{name}: {state.relative_to(ROOT).as_posix()} (route-board exit {completed.returncode})")


def _write(path: Path, document: dict, *, digest: bool = True) -> None:
    if digest:
        body = json.dumps(document["value"], sort_keys=True, separators=(",", ":"))
        document["content_digest"] = "sha256:" + hashlib.sha256(body.encode()).hexdigest()
    path.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n", encoding="utf-8")


def _entry(document: dict, key: str) -> list:
    return next(item for item in document["value"]["$map"] if item[0] == key)


def malformed() -> None:
    """Variants of ``valid_board.state.json`` that each break one rule."""
    source = json.loads((HERE / "valid_board.state.json").read_text(encoding="utf-8"))
    target = HERE / "malformed"
    shutil.rmtree(target, ignore_errors=True)
    target.mkdir()
    nets = [net["fields"]["name"] for net in _entry(source, "board")[1]["fields"]["nets"]["$tuple"]]

    def variant(name: str, change, *, digest: bool = True) -> None:
        document = copy.deepcopy(source)
        change(document)
        _write(target / f"{name}.state.json", document, digest=digest)

    def options(document: dict) -> dict:
        return _entry(document, "options")[1]["fields"]

    def board(document: dict) -> dict:
        return _entry(document, "board")[1]["fields"]

    variant("digest-mismatch", lambda d: options(d).update(maximum_iterations=7), digest=False)
    variant("wrong-schema", lambda d: d.update(schema="copperscript.state/v0"))
    variant("option-unknown-field", lambda d: options(d).update(bogus_option=1))
    variant("option-zero-iterations", lambda d: options(d).update(maximum_iterations=0))
    variant("option-text-pitch", lambda d: options(d).update(pitch_nm="wide"))
    variant("option-bad-fine-regions", lambda d: options(d).update(fine_regions="everywhere"))
    variant("board-missing-name", lambda d: board(d).pop("name"))
    variant("board-unknown-field", lambda d: board(d).update(colour="green"))
    variant("board-bad-layer", lambda d: board(d)["tracks"].update({"$tuple": [
        {"$type": "pcbir.physical:TrackSegment", "fields": {
            "net": nets[0], "layer": {"$enum": "pcbir.physical:CopperLayer", "value": "X.Cu"},
            "start": {"$type": "pcbir.physical:Point", "fields": {"x_nm": 0, "y_nm": 0}},
            "end": {"$type": "pcbir.physical:Point", "fields": {"x_nm": 1000000, "y_nm": 0}},
            "width_nm": 200000}}]}))
    variant("missing-options", lambda d: d["value"].update({"$map": [
        item for item in d["value"]["$map"] if item[0] != "options"]}))
    # Valid variants: Python routes them (the replay suites narrow their output).
    valid = HERE / "variants"
    shutil.rmtree(valid, ignore_errors=True)
    valid.mkdir()
    for name, change in (
        ("auto-fine-regions", lambda d: options(d).update(fine_regions="auto")),
        ("selection", lambda d: _entry(d, "only_nets").__setitem__(1, {"$frozenset": nets[:2]})),
    ):
        document = copy.deepcopy(source)
        change(document)
        _write(valid / f"{name}.state.json", document)
    via_arrays(target, valid)
    (target / "not-json.state.json").write_text("{\"schema\": \"copperscript.state/v1\",\n", encoding="utf-8")
    print(f"malformed: {len(list(target.iterdir()))} variants")


def via_arrays(target: Path, valid: Path) -> None:
    """Variants of ``via_array.state.json``: its via-in-pad rule and array vias."""
    source = json.loads((HERE / "via_array.state.json").read_text(encoding="utf-8"))

    def board(document: dict) -> dict:
        return _entry(document, "board")[1]["fields"]

    def rule(**fields):
        return lambda d: board(d)["via_in_pad_rules"]["$tuple"][0]["fields"].update(fields)

    for name, change in (
        ("via-array-process", rule(process="wet")),
        ("via-array-rows-only", rule(columns=None)),
        ("via-array-zero-rows", rule(rows=0, columns=0)),
        ("via-array-bool-rows", rule(rows=True)),
        ("via-array-pitch-only", rule(rows=None, columns=None, pitch_nm=500000)),
        ("via-array-close-pitch", rule(rows=3, columns=3, pitch_nm=400000)),
        ("via-array-misfit", rule(rows=1, columns=3, pitch_nm=1500000)),
    ):
        document = copy.deepcopy(source)
        change(document)
        _write(target / f"{name}.state.json", document)
    # Valid: the committed array copper is missing, or one site is.
    def without_array(count):
        def change(document):
            vias = board(document)["vias"]["$tuple"]
            capped = [index for index, via in enumerate(vias) if via["fields"]["finish"] == "filled-capped"]
            for index in reversed(capped[:count]):
                del vias[index]
        return change
    for name, change in (("via-array-missing", without_array(4)), ("via-array-partial", without_array(1))):
        document = copy.deepcopy(source)
        change(document)
        _write(valid / f"{name}.state.json", document)


if __name__ == "__main__":
    names = sys.argv[1:] or [*BOARDS, "malformed"]
    for name in names:
        malformed() if name == "malformed" else generate(name)

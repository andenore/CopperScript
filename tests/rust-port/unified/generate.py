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


def _bga_nested():
    """A 5x5 0.75 mm ball array fanned out to a ring of pads, 4 layers: the
    default fine_regions="arrays" routes it on a nested lattice."""
    from test_unified_nested_routing import _bga_board
    return _bga_board()


def _bga_power():
    """The 6x6 0.8 mm ball grid of test_escape_plan with ball 1 on a B.Cu GND
    plane and decoupled rails N8 and N29 (each with a capacitor): the inputs
    of the ball-grid power islands and the ball-field via sites."""
    from dataclasses import replace
    from pcbir import FootprintPad, PadReference, PhysicalFootprint, PhysicalNet, Placement, Point, Size
    from pcbir.physical import DecouplingLink
    from test_escape_plan import ball_grid
    board = ball_grid(6, "0.45", plane=frozenset({1}))
    cap = PhysicalFootprint("cap", (FootprintPad("1", Point.mm("-0.5", 0), Size.mm("0.5", "0.6")),
                                    FootprintPad("2", Point.mm("0.5", 0), Size.mm("0.5", "0.6"))), Size.mm(2, 1))
    caps = {"N8": ("C1", Point.mm(17, 9)), "N29": ("C2", Point.mm(17, 15))}
    nets = []
    for net in board.nets:
        if net.name in caps:
            net = replace(net, pads=(*net.pads, PadReference(caps[net.name][0], "1")))
        elif net.name == "GND":
            net = replace(net, pads=(*net.pads, *(PadReference(ref, "2") for ref, _ in caps.values())))
        nets.append(net)
    links = tuple(DecouplingLink(PadReference(ref, "1"), PadReference("U", net[1:]), PadReference(ref, "2"),
                                 net, "GND") for net, (ref, _) in caps.items())
    return replace(board, footprints={**board.footprints, "cap": cap},
                   placements=(*board.placements, *(Placement(ref, "cap", at) for ref, at in caps.values())),
                   nets=tuple(nets), decoupling_links=links)


# Hand-built boards (the test suite's ball-grid boards): their states hold
# the board, its global route and the default options.
SYNTHETIC = {"bga_nested": _bga_nested, "bga_power": _bga_power}


def synthetic(name: str) -> None:
    sys.path.insert(0, str(ROOT / "tests"))
    from dataclasses import replace
    from pcbir import GlobalRouterOptions, nm_from_mm, route_global
    from pcbir.state_codec import save_state
    from pcbir.unified_router import UnifiedRouterOptions
    board = SYNTHETIC[name]()
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    state = HERE / f"{name}.state.json"
    save_state(str(state), {
        "board": board, "global_route": global_route, "options": UnifiedRouterOptions(),
        "fanout_accesses": None, "fanout_created_vias": None, "fanout_created_tracks": None,
        "only_nets": None, "required_accesses": None,
    }, note="route_unified inputs")
    print(f"{name}: {state.relative_to(ROOT).as_posix()} (hand-built board)")


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
    (target / "not-json.state.json").write_text("{\"schema\": \"copperscript.state/v1\",\n", encoding="utf-8")
    print(f"malformed: {len(list(target.iterdir()))} variants")


if __name__ == "__main__":
    names = sys.argv[1:] or [*BOARDS, *SYNTHETIC, "malformed"]
    for name in names:
        malformed() if name == "malformed" else synthetic(name) if name in SYNTHETIC else generate(name)

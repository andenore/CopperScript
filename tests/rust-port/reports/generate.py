"""Regenerate the run fixtures of the M5 report suites (see README.md).

Runs the Python CLI (`python -m copperscript`) on small example boards and
keeps the artefacts `copper issues`, `copper view` and `copper summarize`
read. Report branches the small boards never reach (native findings, global
and critical failures, length budgets, escape findings, plane estimates,
replay summaries, layout gates) are added as derived reports next to a real
run, located on that run's real pads and tracks. Everything is deterministic:
the same checkout regenerates the same bytes.

    python tests/rust-port/reports/generate.py
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
RUNS = HERE / "runs"
WORK = ROOT / "build" / "report-fixtures"
ENVIRONMENT = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONUTF8": "1"}
COMMON = ["--locked", "--offline", "--allow-proxy-footprints"]
ROUTE = ["--routing-profile", "quick"]

# name: (command, board, extra options, files kept)
GENERATED = {
    "valid-pass": ("route-board", "examples/valid_board/board.copper", ROUTE, ("route-report.json", "board.kicad_pcb")),
    "valid-clearance": ("route-board", "examples/valid_board/board.copper",
                        [*ROUTE, "--width-mm", "40", "--height-mm", "30"], ("route-report.json", "board.kicad_pcb")),
    "valid-stopped": ("route-board", "examples/valid_board/board.copper", [*ROUTE, "--stop-after", "placement"],
                      ("route-report.json", "board.kicad_pcb")),
    "hierarchical": ("route-board", "examples/hierarchical_board/board.copper", ROUTE,
                     ("route-report.json", "board.kicad_pcb")),
    "outline": ("route-board", "examples/mechanical_outline/board.copper", ROUTE,
                ("route-report.json", "board.kicad_pcb")),
    "margin-error": ("route-board", "examples/valid_board/board.copper", [*ROUTE, "--width-mm", "6", "--height-mm", "6"],
                     ("route-report.error.json",)),
    "ring-error": ("route-board", "examples/round_led_ring/board.copper", ROUTE,
                   ("route-report.error.json", "route-report.error.state.json")),
    "layout": ("plan-layout", "examples/valid_board/board.copper", [], ("layout-report.json", "board.kicad_pcb")),
    "checkpoint": ("route-board", "examples/valid_board/board.copper", [*ROUTE, "--checkpoint-dir", "{work}/states"],
                   ("route-report.json",)),
}
STATES = ("final-board.state.json", "placement.state.json", "critical-inputs.state.json")


def copper(*args: str) -> None:
    completed = subprocess.run([sys.executable, "-m", "copperscript", *args], cwd=ROOT, env=ENVIRONMENT,
                               capture_output=True, text=True)
    print(f"python -m copperscript {' '.join(args)}  -> exit {completed.returncode}")


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def generate_runs() -> None:
    if WORK.exists():
        shutil.rmtree(WORK)
    for name, (command, board, extra, keep) in GENERATED.items():
        work = WORK / name
        report = "layout-report.json" if command == "plan-layout" else "route-report.json"
        options = [option.replace("{work}", work.relative_to(ROOT).as_posix()) for option in extra]
        copper(command, board, *COMMON, *options, "--report", (work / report).relative_to(ROOT).as_posix(),
               "-o", (work / "board.kicad_pcb").relative_to(ROOT).as_posix())
        target = RUNS / name
        target.mkdir(parents=True, exist_ok=True)
        for file in keep:
            shutil.copyfile(work / file, target / file)
    states = HERE / "states"
    states.mkdir(exist_ok=True)
    for file in STATES:
        shutil.copyfile(WORK / "checkpoint" / "states" / file, states / file)
    shutil.rmtree(RUNS / "checkpoint")


def pads(board: Path) -> dict[str, list]:
    sys.path.insert(0, str(ROOT))
    from pcbir.scene import scene_from_kicad_pcb
    scene = scene_from_kicad_pcb(board)
    return {"pads": [(p.ref, p.pad, p.net, p.center) for p in scene.pads],
            "tracks": [(t.net, t.layer, t.start, t.end) for t in scene.tracks]}


def nm(value: float) -> int:
    return round(value * 1_000_000)


def derived_reports() -> None:
    """Reports for the branches the small boards do not reach, on valid-clearance's geometry."""
    run = RUNS / "valid-clearance"
    base = json.loads((run / "route-report.json").read_text(encoding="utf-8"))
    geometry = pads(run / "board.kicad_pcb")
    by_ref = {(ref, pad): (net, center) for ref, pad, net, center in geometry["pads"]}
    gnd = [(ref, pad, center) for ref, pad, net, center in geometry["pads"] if net == "GND"]
    sda = [(ref, pad, center) for ref, pad, net, center in geometry["pads"] if net == "I2C_SDA"]
    token = base["drc"]["token"]["board_digest"]

    def item(ref, pad, layers=("F.Cu",), kind="pad"):
        net, center = by_ref[(ref, pad)]
        return {"component": ref, "pad": pad, "net": net, "kind": kind, "layers": list(layers),
                "positions_nm": [[nm(center[0]), nm(center[1])]]}

    def loose(kind, x, y, net="GND", layers=("B.Cu",)):
        return {"kind": kind, "net": net, "layers": list(layers), "positions_nm": [[nm(x), nm(y)], [nm(x) + 400000, nm(y)]]}

    # Native findings: gaps grouped by net and place, other findings by type.
    filled_source = (run / "board.kicad_pcb").read_text(encoding="utf-8")
    x0, y0 = gnd[0][2]
    zone = (f'  (zone (net 0) (net_name "GND") (layer "B.Cu") (uuid "00000000-0000-0000-0000-00000000f111")\n'
            f'    (polygon (pts (xy {x0 - 3} {y0 - 3}) (xy {x0 + 3} {y0 - 3}) (xy {x0 + 3} {y0 + 3}) (xy {x0 - 3} {y0 + 3})))\n'
            f'    (filled_polygon (layer "B.Cu") (pts (xy {x0 - 2.5} {y0 - 2.5}) (xy {x0 + 2.5} {y0 - 2.5}) '
            f'(xy {x0 + 2.5} {y0 + 2.5}) (xy {x0 - 2.5} {y0 + 2.5})))\n  )\n')
    filled = filled_source.rstrip().removesuffix(")") + zone + ")\n"
    native_dir = run / "native"
    native_dir.mkdir(exist_ok=True)
    (native_dir / "filled.kicad_pcb").write_text(filled, encoding="utf-8")
    digest = hashlib.sha256((native_dir / "filled.kicad_pcb").read_bytes()).hexdigest()
    native_findings = [
        {"type": "unconnected_items", "description": "Missing connection between items",
         "items": [item(*gnd[0][:2]), item(*gnd[1][:2])]},
        {"type": "unconnected_items", "description": "Missing connection between items",
         "items": [item(*gnd[1][:2]), item(*gnd[2][:2])]},
        {"type": "unconnected_items", "description": "Missing connection between items",
         "items": [item(*sda[0][:2]), item(*sda[1][:2])]},
        {"type": "clearance", "description": "Clearance violation (netclass 'Default' clearance 0.2000 mm)",
         "items": [item(*gnd[0][:2]), loose("track", x0 + 0.3, y0)]},
        {"type": "clearance", "description": "Clearance violation (netclass 'Default' clearance 0.2000 mm)",
         "items": [loose("via", x0 - 0.5, y0 + 0.5, layers=("F.Cu", "B.Cu"))]},
        {"category": "dangling", "description": "Track has unconnected end", "items": [{"kind": "track", "net": "GND"}]},
        {"description": "Board has a malformed outline", "items": []},
        "not a finding",
    ]
    native = {**base, "status": "fail", "routing_complete": False, "plane_verification": {
        "passed": False, "board_digest": token, "artifact_directory": "native", "filled_board_digest": digest,
        "unconnected_count": 3, "other_violation_count": 2, "island_count": 0, "report_digest": "sha256:" + "ab" * 32,
        "native_findings": native_findings,
        "findings": ["Missing connection between items"] * 3 + ["Clearance violation", "Track has unconnected end"]}}
    write_json(run / "native.json", native)

    # A closed native verification that resolves deferred explicit-copper findings.
    deferred = ["GND"]
    closed = {**base, "status": "pass", "routing_complete": True,
              "route_geometry": {**base["route_geometry"], "zone_nets_deferred": deferred},
              "drc": {**base["drc"], "findings": [
                  {"code": "DRC-OPEN-NET", "severity": "error", "nets": ["GND"], "message": "GND is open by explicit copper"},
                  {"code": "DRC-ROUTE-INCOMPLETE", "severity": "error", "nets": [], "message": "routing incomplete"},
                  {"code": "DRC-MAX-LENGTH", "severity": "warning", "nets": ["I2C_SDA"], "message": "I2C_SDA too long",
                   "measured_nm": 30500000, "required_nm": 25000000}]},
              "plane_verification": {"passed": True, "board_digest": token, "unconnected_count": 0,
                                     "other_violation_count": 0, "island_count": 0, "findings": [],
                                     "report_digest": "sha256:" + "cd" * 32}}
    write_json(run / "closed.json", closed)
    write_json(run / "unbound.json", {**closed, "plane_verification": {**closed["plane_verification"], "board_digest": "x"}})

    # Global, critical, detailed, budget, plane-estimate, escape and placement branches.
    ordinary = [f"NET{i}" for i in range(8)]
    tracks = geometry["tracks"]
    routes = [
        {"net": "I2C_SDA", "connected": False, "diagnostics": ["no pin access for U2.46", "blocked near U3.4"]},
        {"net": "I2C_SCL", "connected": False, "diagnostics": ["no pin access for U2.45"]},
        {"net": "V3V3", "connected": False, "deferred_to_zone": True, "diagnostics": ["zone"]},
        *({"net": net, "connected": False, "diagnostics": ["capacity exhausted"]} for net in ordinary),
        {"net": "GND", "connected": False},
        {"net": "VBUS", "connected": True},
        "not a route",
    ]
    failing = {**base, "status": "fail", "routing_complete": False, "stopped_after": None,
               "global": {**base["global"], "routes": routes},
               "critical": {**base["critical"], "nets": [
                   {"nets": ["I2C_SDA", "I2C_SCL"], "connected": False, "diagnostics": ["pair blocked at U2"],
                    "local_preparation": {"status": "blocked", "reason": "terminal escape crosses U2.45"},
                    "repairs": [{"ripped_up": ["V3V3"], "search_states": 812, "pair_budget_exhausted": True},
                                {"ripped_up": [], "search_states": 3}, "skipped"]},
                   {"nets": ["VBUS"], "connected": False, "diagnostics": [],
                    "local_preparation": {"status": "unsupported"}},
                   {"nets": ["GND"], "connected": True, "diagnostics": ["fine"]}]},
               "detailed": {**base["detailed"], "nets": [
                   {"net": "I2C_SDA", "connected": False, "diagnostics": ["search budget exhausted"]},
                   {"net": "I2C_SCL", "connected": False, "diagnostics": ["search budget exhausted"]},
                   {"net": "V3V3", "connected": False, "diagnostics": []},
                   *({"net": net, "connected": False, "diagnostics": ["no legal via site"]} for net in ordinary),
                   {"net": "GND", "connected": True, "diagnostics": []}]},
               "drc": {**base["drc"], "findings": [
                   *base["drc"]["findings"],
                   {"code": "DRC-OPEN-NET", "severity": "error", "nets": ["I2C_SDA", "I2C_SCL"], "message": "open"},
                   {"code": "DRC-OPEN-NET", "severity": "error", "nets": ["V3V3"], "message": "open"},
                   {"code": "DRC-TRACK-CLEARANCE", "severity": "error", "nets": [tracks[0][0]], "layers": [tracks[0][1]],
                    "objects": ["track:0", "via:0", "track:9999", "pad:C1.1"], "message": "track too close"},
                   {"code": "DRC-TRACK-CLEARANCE", "severity": "error", "nets": ["NOT_THIS_NET"], "layers": [],
                    "objects": ["track:1", "via:1"], "message": "mismatching objects get no marker"},
                   {"code": "DRC-MAX-LENGTH", "severity": "warning", "nets": ["I2C_SDA"], "message": "I2C_SDA exceeds its budget",
                    "measured_nm": 31250000, "required_nm": 25000000},
                   {"code": "DRC-DIFF-SKEW", "severity": "warning", "nets": ordinary, "message": "skew"},
                   {"code": "DRC-LENGTH-MATCH", "severity": "warning", "nets": ["I2C_SCL"], "message": "match",
                    "measured_nm": 1.5, "required_nm": 2},
                   {"code": "DRC-OTHER", "severity": "warning", "nets": ["GND"], "message": "ignored"},
                   "not a finding"]},
               "summary": {**base["summary"], "plane": {"estimated_open_pads": {
                   "GND": [f"{ref}.{pad}" for ref, pad, _ in gnd], "V3V3": ["U2.24", "U3.1", "C1.1", "R1.1", "R2.1",
                                                                          "U1.2", "U2.36", "U2.48"]}}},
               "escape": {**base["escape"], "pairs": [
                   {"components": ["U2", "U3"], "crossing_pairs": 2, "crossing_nets": ["I2C_SDA", "I2C_SCL"],
                    "layer_moves": ["I2C_SCL"]},
                   {"components": ["U1", "C1"], "crossing_pairs": 0, "crossing_nets": []}],
                   "clusters": [{"component": "U2", "overflow": 2, "inner_signal_pins": ["U2.45", "U2.46", "U2.99"],
                                 "channels": [{"layer": "F.Cu", "demand": 5, "capacity": 3},
                                              {"layer": "B.Cu", "demand": 1, "capacity": 4}, "x"]},
                                {"component": "U3", "overflow": 0}]},
               "placement_findings": [
                   {"code": "REMOTE_FIXED_PART", "status": "warn", "subject": "U3", "message": "U3 is fixed far away"},
                   {"code": "CORRIDOR_PART", "status": "pass", "subject": "R1", "message": "fine"},
                   {"status": "warn", "message": "no subject"}],
               "requirements": {**base["requirements"], "status": "fail", "complete": False,
                                "counts": {"fail": 1, "blocked": 1, "pass": 1}, "requirements": [
                   *base["requirements"]["requirements"],
                   {"id": "max_distance:1", "status": "fail", "detail": "C1 is 4.2 mm from U2.VDD (limit 3 mm)",
                    "origins": ["board.copper:107:16", "board.copper:108:16", "lib.copper:4:1", "lib.copper:9:1"],
                    "measurements": [{"actual_nm": 4200000, "limit_nm": 3000000}] * 5},
                   {"id": "impedance:0", "status": "blocked", "detail": "no field solver",
                    "origins": "board.copper:120:5", "measurements": []}]},
               "routing_quality": {**base["routing_quality"], "decoupling": [
                   {"surface_connected": True}, {"surface_connected": False}, {}]},
               "fanout": {"late_bypass_completion": {
                   "status": "partial", "passes": 2, "maximum_passes": 3, "selected_links": ["a", "b", "c"],
                   "completed_links": ["a"], "remaining_links": ["b", "c"], "added_tracks": 4, "added_vias": 1,
                   "reason": "remaining links have no legal via site"}},
               "gates": [{"stage": "route", "status": "fail", "summary": "routing incomplete"},
                         {"stage": "verify", "status": "not_run", "summary": ""},
                         {"stage": "place", "status": "pass", "summary": "placed"}]}
    write_json(run / "failing.json", failing)

    # replay-unified's summary: its unrouted table stands in for detailed nets.
    replay = {"schema": "copperscript-route-summary/v1", "state": "trial.state.json", "seconds": 1.5,
              "selected_nets": ["I2C_SDA", "I2C_SCL"], "routing_complete": False,
              "unrouted": {"I2C_SDA": ["blocked by U3"], "I2C_SCL": ["blocked by U3"], "V3V3": "not a list"}}
    write_json(run / "replay.json", replay)

    # A rejected run directory without route-report.json: the report is found by schema.
    renamed = RUNS / "renamed"
    renamed.mkdir(exist_ok=True)
    write_json(renamed / "a-notes.json", {"note": "not a CopperScript record"})
    write_json(renamed / "board.editor-intent.json", {"schema": "copperscript-editor-intent/v1"})
    write_json(renamed / "b.summary.json", {"schema": "copperscript-route-board/v0.1"})
    write_json(renamed / "routed.json", failing)


def reference_boards() -> None:
    """A rotated and nudged copy of a run's board, for reference overlays."""
    boards = HERE / "boards"
    boards.mkdir(exist_ok=True)
    text = (RUNS / "valid-clearance" / "board.kicad_pcb").read_text(encoding="utf-8")
    marker = '(property "Reference" "U2"'
    start = text.rindex("(footprint", 0, text.index(marker))
    at = text.index("(at ", start)
    end = text.index(")", at)
    x, y = text[at + 4:end].split()[:2]
    rotated = text[:at] + f"(at {x} {y} 90" + text[end:]
    (boards / "reference-rotated.kicad_pcb").write_text(rotated, encoding="utf-8")


def malformed_reports() -> None:
    """Reports `summarize` and `issues` refuse, each in its own way."""
    bad = HERE / "bad"
    bad.mkdir(exist_ok=True)
    texts = {
        "duplicate-key.json": '{"schema": "copperscript-route-board/v0.1", "drc": {"findings": [], "findings": []}}\n',
        "nan.json": '{"schema": "copperscript-route-board/v0.1", "routing_quality": {"track_length_nm": NaN}}\n',
        "infinity.json": '{"schema": "copperscript-route-board/v0.1", "x": [-Infinity]}\n',
        "syntax.json": '{"schema": "copperscript-route-board/v0.1",\n "drc": {"findings": [}\n}\n',
        "list.json": '[{"schema": "copperscript-route-board/v0.1"}]\n',
        "empty.json": "",
        "unsupported.json": '{"schema": "copperscript-route-board/v0.2"}\n',
        "unhashable-schema.json": '{"schema": ["copperscript-route-board/v0.1"]}\n',
        "wrong-types.json": '{"schema": "copperscript-route-board/v0.1", "requirements": [], "drc": {"findings": [1]}}\n',
        "connectivity.json": '{"schema": "copperscript-route-board/v0.1", "connectivity": [1, 2]}\n',
        "native-counter.json": '{"schema": "copperscript-route-board/v0.1", "plane_verification": '
                               '{"passed": false, "findings": {"gap": 2, "short": 1}}}\n',
        "native-items.json": '{"schema": "copperscript-route-board/v0.1", "plane_verification": '
                             '{"passed": false, "findings": [["not", "text"]]}}\n',
        "mixed-codes.json": '{"schema": "copperscript-route-board/v0.1", "drc": {"findings": ['
                            '{"severity": "error", "code": "B"}, {"severity": "error", "code": 7}, '
                            '{"severity": "error", "code": "A"}]}}\n',
        "track-length.json": '{"schema": "copperscript-route-board/v0.1", "routing_quality": '
                             '{"track_length_nm": [1], "track_count": 1, "via_count": 0}}\n',
        "float-length.json": '{"schema": "copperscript-route-board/v0.1", "routing_quality": '
                             '{"track_length_nm": 0.1, "track_count": 1, "via_count": 0, "decoupling": [{"surface_connected": 0.5}]}}\n',
        "late-rejected.json": '{"schema": "copperscript-layout-report/v0.1", "fanout": {"late_bypass_completion": '
                              '{"status": "rejected", "passes": 0, "maximum_passes": 2}}}\n',
    }
    for name, text in texts.items():
        (bad / name).write_text(text, encoding="utf-8")
    (bad / "bom.json").write_bytes(b"\xef\xbb\xbf" + texts["unsupported.json"].encode())
    (bad / "utf16.json").write_bytes(texts["unsupported.json"].encode("utf-16"))
    (bad / "latin1.json").write_bytes(b'{"schema": "caf\xe9"}\n')


def main() -> None:
    generate_runs()
    derived_reports()
    reference_boards()
    malformed_reports()


if __name__ == "__main__":
    main()

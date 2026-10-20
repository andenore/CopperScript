"""Preserve the selected geometry and identities across diagnostic iterations."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from pcbir.backends import KiCadPcbBackend
from pcbir.workbench import load_run, load_state

ROOT = Path(__file__).parents[1]
PCB = b'(kicad_pcb (version 20240108) (layers (0 "F.Cu" signal) (31 "B.Cu" signal)))\n'


def _native_report(directory, artifact_directory, content=PCB):
    report = directory / "route-report.json"
    report.write_text(json.dumps({
        "schema": "copperscript-route-board/v0.1",
        "plane_verification": {
            "artifact_directory": str(artifact_directory),
            "filled_board_digest": hashlib.sha256(content).hexdigest(),
        },
    }), encoding="utf-8")
    return report


@pytest.mark.parametrize("location", [
    "route-report.native-test/verification-001",
    "custom-evidence/verification-001",
    "custom-evidence/native-cleanup",
])
@pytest.mark.parametrize("relative", [False, True])
def test_filled_view_uses_report_selected_artifact_and_digest(tmp_path, location, relative, monkeypatch):
    accepted = tmp_path / location
    rejected = tmp_path / "route-report.native-zzz" / "verification-002"
    for directory in (accepted, rejected):
        directory.mkdir(parents=True)
    selected = accepted / "board.kicad_pcb"
    selected.write_bytes(PCB)
    (accepted / "aaa-unrelated.kicad_pcb").write_bytes(PCB + b"; unrelated\n")
    (rejected / "board.kicad_pcb").write_bytes(PCB + b"; rejected\n")
    report = _native_report(tmp_path, location if relative else accepted)
    # Relative artifact paths belong to the report, not the caller's directory.
    monkeypatch.chdir(ROOT)
    for target in (tmp_path, report):
        run = load_run(target)
        assert run.filled_board_path == selected
        assert run.scene(filled=True).source == str(selected)


@pytest.mark.parametrize("damage", ["changed", "missing", "no_digest", "no_directory", "no_verification"])
def test_filled_view_does_not_substitute_another_trial(tmp_path, damage):
    accepted = tmp_path / "evidence" / "verification-001"
    rejected = tmp_path / "route-report.native-zzz" / "verification-002"
    accepted.mkdir(parents=True)
    rejected.mkdir(parents=True)
    selected = accepted / "board.kicad_pcb"
    selected.write_bytes(PCB)
    # Even matching bytes in an unselected directory are not authority to use it.
    (rejected / "board.kicad_pcb").write_bytes(PCB)
    report = _native_report(tmp_path, accepted)
    document = json.loads(report.read_text())
    if damage == "changed":
        selected.write_bytes(PCB + b"; modified after verification\n")
    elif damage == "missing":
        selected.unlink()
    elif damage == "no_digest":
        del document["plane_verification"]["filled_board_digest"]
    elif damage == "no_directory":
        del document["plane_verification"]["artifact_directory"]
    else:
        del document["plane_verification"]
    report.write_text(json.dumps(document), encoding="utf-8")
    assert load_run(tmp_path).filled_board_path is None


@pytest.mark.parametrize("stage", ["placement", "package-preflight", "package-access"])
def test_stopped_cli_exports_exact_stage_copper_without_final_processing(tmp_path, stage, monkeypatch):
    import pcbir.cli as cli
    import pcbir.route_cleanup as cleanup

    source = tmp_path / "board.copper"
    source.write_text("""board StopProbe {
        use library "tiny";
        component R1: RESISTOR { value = 10kohm; }
        component R2: RESISTOR { value = 10kohm; }
        net GND { R1.1; R2.1; }
        no_connect R1.2, R2.2;
        supply GND { voltage = 0V; external = true; }
        constraint copper_zone(GND) { layers = "B.Cu"; inset = 0.5mm; pad_connection = solid; }
    }""", encoding="utf-8")
    original = cli.run_routing_pipeline
    completed = []

    def forbidden(*args, **kwargs):
        pytest.fail("stopped run entered final copper processing or native verification")

    def observe_pipeline(*args, **kwargs):
        result = original(*args, **kwargs)
        completed.append(result)
        # Guards are installed after the selected stage legitimately finishes.
        for name in ("stitch_zone_pads", "stitch_duplicate_pads", "run_physical_drc",
                     "improve_filled_copper", "verify_filled_planes"):
            monkeypatch.setattr(cli, name, forbidden)
        monkeypatch.setattr(cleanup, "trim_dangling_overhangs", forbidden)
        return result

    monkeypatch.setattr(cli, "run_routing_pipeline", observe_pipeline)
    report = tmp_path / "route-report.json"
    output = tmp_path / "board.kicad_pcb"
    code = cli.main([
        "route-board", str(source), "--allow-proxy-footprints", "--stop-after", stage,
        "--candidates", "1", "--checkpoint-dir", str(tmp_path / "checkpoint"),
        "--report", str(report), "-o", str(output), "--verify-plane-fill", "must-not-run",
    ])
    assert code == 1
    result, = completed
    document = json.loads(report.read_text())
    assert document["stopped_after"] == stage
    assert document["final_copper_processing"] == "skipped_for_stopped_run"
    assert document["native_verification"] == "skipped_for_stopped_run"
    assert document["duplicate_pad_stitch"]["status"] == "not_run"
    assert document["status"] == "fail" and not document["routing_complete"]
    expected = next(a.content for a in KiCadPcbBackend().generate(result.board).artifacts
                    if a.name.endswith(".kicad_pcb"))
    if isinstance(expected, bytes):
        expected = expected.decode("utf-8")
    assert output.read_text(encoding="utf-8") == expected
    if stage == "placement":
        saved = load_state(tmp_path / "checkpoint" / "placement.state.json")
        assert saved.board.tracks == result.board.tracks == ()
        assert saved.board.vias == result.board.vias == ()


def test_checkpoint_keys_are_canonical_across_processes_and_sensitive_to_changes():
    program = """
import json
import os
from dataclasses import replace
from pathlib import Path
from pcbir.loader import load_design
from pcbir.physicalize import prototype_physicalize, PrototypePhysicalOptions
from pcbir.placement import PlacementPlannerOptions
from pcbir.iteration_checkpoints import placement_key
board = prototype_physicalize(load_design(Path("examples/valid_board/board.copper")), PrototypePhysicalOptions())
refs = frozenset(("U1", "U2", "U3", "R1", "R2", "C1", "PS1"))
options = PlacementPlannerOptions(fixed_references=refs)
# Exercise nested mapping/set canonicalization as well as the dataclass field.
extra = {ref: frozenset(("F.Cu", "In1.Cu", "B.Cu")) for ref in refs}
def key(opts, data):
    return placement_key(board, opts, None, None, extra=data)["options"]
print(json.dumps([
    key(options, extra),
    key(replace(options, fixed_references=refs - {"U3"}), extra),
    key(options, ("F.Cu", "B.Cu")),
    key(options, ("B.Cu", "F.Cu")),
]))
"""
    results = []
    for seed in ("1", "2"):
        result = subprocess.run([sys.executable, "-c", program], text=True, capture_output=True,
                                cwd=ROOT, env={**os.environ, "PYTHONHASHSEED": seed})
        assert result.returncode == 0, result.stderr
        results.append(json.loads(result.stdout))
    assert results[0] == results[1]
    original, changed, forward, backward = results[0]
    assert original != changed
    assert forward != backward

"""Fast failure-gate tests; the real four-layer route is an explicit build."""
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture
def workflow(monkeypatch):
    monkeypatch.syspath_prepend(str(ROOT / "scripts"))
    spec = importlib.util.spec_from_file_location("cm4_workflow", ROOT / "scripts/route_cm4.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def evidence(output):
    (output / "route-report.json").write_text(json.dumps({
        "status": "pass", "erc_pass": True, "routing_complete": True,
        "connectivity": {"native_fill_verified": True, "unrouted_ordinary_nets": []}}))
    (output / "kicad-drc.json").write_text(json.dumps({"violations": [], "unconnected_items": []}))
    (output / "board.kicad_pcb").write_text("saved filled board")


@pytest.mark.parametrize("failure", ["open", "warning", "missing", "malformed", "unrouted", "erc"])
def test_completion_requires_exact_native_and_router_evidence(workflow, tmp_path, failure):
    evidence(tmp_path)
    assert workflow.completed(tmp_path)
    native_path = tmp_path / "kicad-drc.json"
    native = json.loads(native_path.read_text())
    if failure == "open":
        native["unconnected_items"] = [{"description": "unconnected ground"}]
    elif failure == "warning":
        native["violations"] = [{"severity": "warning"}]
    elif failure == "missing":
        native.pop("unconnected_items")
    elif failure == "malformed":
        native = []
    else:
        route_path = tmp_path / "route-report.json"
        route = json.loads(route_path.read_text())
        if failure == "unrouted":
            route["connectivity"]["unrouted_ordinary_nets"] = ["V5"]
        else:
            route["erc_pass"] = False
        route_path.write_text(json.dumps(route))
    native_path.write_text(json.dumps(native))
    assert not workflow.completed(tmp_path)


def test_command_uses_real_locked_four_layer_board_without_waivers(workflow, tmp_path):
    command = workflow.routing_command(ROOT, tmp_path, Path("cli"), Path("footprints"))
    assert command[4:6] == ["route-board", str(ROOT / "examples/cm4_baseboard/board.copper")]
    assert "--locked" in command and "--offline" in command and "--fanout-maze" in command
    assert command[command.index("--layers") + 1] == "4"
    assert command[command.index("--fab-profile") + 1] == "jlcpcb-four-layer"
    assert "--ground-via-in-pad" not in command and "--allow-proxy-footprints" not in command


@pytest.mark.parametrize("router_code,fill_code,valid,expected", [
    (1, 0, True, 1), (0, 3, True, 3), (0, 0, False, 1), (0, 0, True, 0)])
def test_workflow_preserves_exit_and_checks_saved_fills(workflow, tmp_path, monkeypatch,
                                                       router_code, fill_code, valid, expected):
    monkeypatch.setattr(workflow, "REPOSITORY", tmp_path)
    inputs = [workflow.SOURCE, workflow.SOURCE.parent / "copper.mod",
              workflow.SOURCE.parent / "copper.lock",
              workflow.SOURCE.parent / "mechanics/carrier.copper", Path("scripts/route_cm4.py")]
    for relative in inputs:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("test input")
    cli = tmp_path / "cli"
    cli.write_text("test executable")
    footprints = tmp_path / "footprints"
    footprints.mkdir()
    monkeypatch.setattr(workflow, "resolve_module_root", lambda *args, **kwargs: tmp_path)
    calls = []
    output = tmp_path / "build/run"
    def run(command, repository, log):
        calls.append(command)
        if len(calls) == 1:
            assert "pcbir.profiling" in command  # enabled by default; preserves SystemExit
            return router_code
        assert all(flag in command for flag in ("--save-board", "--refill-zones", "--severity-all"))
        if valid:
            evidence(output)
        return fill_code
    monkeypatch.setattr(workflow.workflow, "_run_logged", run)
    assert workflow.main(["--kicad-cli", str(cli), "--kicad-footprints", str(footprints),
                          "--output-dir", "build/run"]) == expected
    assert len(calls) == (1 if router_code else 2)
    manifest = json.loads((output / "run.json").read_text())
    assert manifest["exit_code"] == expected
    assert (manifest["status"] == "passed") == (expected == 0)
    assert ("filled_board_sha256" in manifest) == (expected == 0)
    # A previous run's evidence must never be silently reused.
    assert workflow.main(["--kicad-cli", str(cli), "--kicad-footprints", str(footprints),
                          "--output-dir", "build/run"]) == 2

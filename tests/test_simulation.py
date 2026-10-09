"""Plan/coverage gates and analytic regression tests for simulation evidence."""
import copy
import csv
import hashlib
import json
import math
import os
import shutil
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from pcbir.backends.ngspice import NgspiceBackend, generate_deck
from pcbir.elaborate import elaborate
from copperscript.library_data import tiny_library
from pcbir.loader import load_board
from pcbir.model import Board, ComponentInstance, Endpoint, ModuleDefinition, ModuleInstance, Net, PackagePinDefinition, PartDefinition, PinType
from pcbir.quantities import Current, Frequency, Time, Voltage
from pcbir.serializer import board_to_json
from pcbir.simulation import SimulationError, load_plan, parse_plan, run_simulation
from pcbir.simulation.measure import evaluate, validate_dimensions
from pcbir.simulation.models import load_registry
from pcbir.simulation.ngspice import _execute, engine_identity, export_simulation, resolve_engine
from pcbir.simulation.raw import parse_raw
from pcbir.simulation.result import Trace, Waveforms, normalize, represented
from pcbir.simulation.select import select_circuit


ROOT = Path(__file__).parents[1]
EXAMPLES = ROOT / "examples" / "simulation"


@pytest.fixture
def board():
    return load_board(EXAMPLES / "rc_filter.copper")


def plan_data():
    data = json.loads((EXAMPLES / "rc_filter.json").read_text())
    data.pop("cases")
    data.pop("plots")
    data.pop("checks")
    return data


@pytest.fixture(scope="module")
def engine():
    requested = os.environ.get("NGSPICE") or shutil.which("ngspice")
    if not requested:
        if os.environ.get("COPPER_SIM_REQUIRED") == "1":
            pytest.fail("CI requires a qualified ngspice engine; simulator tests may not be skipped")
        pytest.skip("set NGSPICE to an executable or Windows ngspice.dll to run simulator integration tests")
    if os.environ.get("COPPER_SIM_REQUIRED") == "1":
        import importlib.util
        if any(importlib.util.find_spec(m) is None for m in ("matplotlib", "plotly")):
            pytest.fail("CI requires simulation reporting dependencies; tests may not be skipped")
    pytest.importorskip("matplotlib")
    pytest.importorskip("plotly")
    return requested


@pytest.fixture(scope="module")
def example_results(engine, tmp_path_factory):
    folder = tmp_path_factory.mktemp("simulation-examples")
    results = {}
    for name in ("rc_filter", "inrush"):
        results[name] = run_simulation(load_board(EXAMPLES / f"{name}.copper"), load_plan(EXAMPLES / f"{name}.json"), folder / name, ngspice=engine, timeout=30)
    return folder, results


def test_typed_plan_and_export_preserve_authoritative_ir(board, tmp_path):
    plan = parse_plan(plan_data())
    original = board_to_json(board)
    assert isinstance(plan.analyses[0].start, Frequency)
    assert isinstance(plan.analyses[1].max_step, Time)
    assert isinstance(plan.sources[0].dc, Voltage)
    with pytest.raises(TypeError):
        plan.values["C1"] = "1 F"
    detached = plan.data
    detached["name"] = "mutated"
    assert plan.name == "rc-filter"
    first = NgspiceBackend(plan).generate(board)
    assert first == NgspiceBackend(plan).generate(board)
    assert board_to_json(board) == original
    output = export_simulation(board, plan, tmp_path / "decks")
    deck = (output / "cases/nominal/frequency/deck.cir").read_text()
    assert ".ac dec 100" in deck
    capacitor = next(line for line in deck.splitlines() if line.startswith("ce"))
    assert float(capacitor.split()[-1]) == pytest.approx(100e-9)
    assert "1.0M" not in deck


@pytest.mark.parametrize("mutate", [
    lambda d: d.update(schema_version=True),
    lambda d: d.update(nam="typo"),
    lambda d: d["sources"]["input"].update(series_resistance="1 ms"),
    lambda d: d["sources"]["input"]["ac"].update(magnitude="0 V"),
    lambda d: d["sources"]["input"]["waveform"].update(duration="-1 us"),
    lambda d: d["analyses"]["frequency"].update(stop="1 Hz"),
    lambda d: d["analyses"]["frequency"].update(points_per_decade=True),
    lambda d: d["analyses"]["startup"].update(max_step="2 ms"),
    lambda d: d.update(initial_state={"method": "uic"}),
    lambda d: d["derived_signals"].update(loop={"kind": "ratio", "numerator": "loop", "denominator": "input_voltage"}),
    lambda d: d["probes"]["input_current"].update(positive_direction="mystery"),
    lambda d: d["sources"]["input"].update(dc="1e400 V"),
    lambda d: d.update(cases={"../../escape": {}}),
])
def test_plan_rejects_typos_units_invalid_excitation_and_unsafe_names(mutate):
    data = plan_data(); mutate(data)
    with pytest.raises(SimulationError):
        parse_plan(data)


def test_duplicate_json_keys_are_not_silently_overwritten(tmp_path):
    path = tmp_path / "plan.json"
    path.write_text('{"schema_version":1,"schema_version":1}')
    with pytest.raises(SimulationError, match="duplicate JSON key"):
        load_plan(path)


def test_dimensional_checks_fail_before_solving():
    data = plan_data()
    data["checks"] = [{"name": "wrong_units", "analysis": "startup", "probe": "output_voltage", "operation": "max", "maximum": "1 A"}]
    with pytest.raises(SimulationError, match="Voltage unit"):
        validate_dimensions(parse_plan(data))
    data["checks"][0].update(probe="gain", analysis="startup", maximum=2)
    with pytest.raises(SimulationError, match="unavailable"):
        validate_dimensions(parse_plan(data))


def test_missing_active_model_and_incorrect_passive_value_fail(board):
    malformed = replace(board, components=(*board.components, ComponentInstance("U1", "OPAMP_UNKNOWN")),
                        library={**board.library, "OPAMP_UNKNOWN": PartDefinition("OPAMP_UNKNOWN", {})})
    with pytest.raises(SimulationError, match="missing simulation model"):
        select_circuit(malformed, parse_plan(plan_data()))
    data = plan_data(); data["values"] = {"C1": "2 kohm"}
    with pytest.raises(SimulationError, match="Capacitance unit"):
        select_circuit(board, parse_plan(data))


def test_subset_boundaries_require_explicit_replacements(board):
    ps = ComponentInstance("PS", "VOLTAGE_SOURCE")
    nets = tuple(replace(n, endpoints=(*n.endpoints, Endpoint("PS", "OUT" if n.name == "VIN" else "GND"))) if n.name in {"VIN", "GND"} else n for n in board.nets)
    partial = replace(board, components=(*board.components, ps), library={**board.library, **tiny_library()}, nets=nets)
    data = plan_data(); data["circuit"] = {"components": ["R1", "C1"], "reference_node": "GND"}
    with pytest.raises(SimulationError, match="unexplained subset boundaries"):
        select_circuit(partial, parse_plan(data))
    data["sources"]["input"]["covers"] = ["PS.OUT", "PS.GND"]
    circuit = select_circuit(partial, parse_plan(data))
    assert len(circuit.boundaries) == 2
    data["sources"]["input"]["covers"] = ["PS.OUT"]
    data["circuit"]["open_boundaries"] = {"PS.GND": "external return pin intentionally omitted in this fixture"}
    assert len(select_circuit(partial, parse_plan(data)).boundaries) == 2


def test_module_selection_uses_elaborated_port_aliases(board):
    definition = ModuleDefinition("Filter", {"VIN": PinType.POWER_IN, "VOUT": PinType.OUTPUT, "GND": PinType.POWER_IN},
        board.library, board.components, (), tuple(replace(n, endpoints=(*n.endpoints, Endpoint("port", n.name))) for n in board.nets))
    hierarchical = Board("HierarchicalFilter", {}, (), (), module_instances=(ModuleInstance("FILTER", "Filter"),), module_definitions={"Filter": definition})
    data = plan_data(); data["circuit"] = {"modules": ["FILTER"], "reference_node": "FILTER.GND"}
    data["sources"]["input"]["positive"] = "FILTER.VIN"; data["sources"]["input"]["negative"] = "FILTER.GND"
    for probe in data["probes"].values():
        for key in ("positive", "negative"):
            if key in probe:
                probe[key] = "FILTER." + probe[key]
    original = board_to_json(hierarchical)
    circuit = select_circuit(hierarchical, parse_plan(data))
    assert {e.ref for e in circuit.elements} == {"FILTER/R1", "FILTER/C1"}
    assert circuit.node("FILTER.GND") == "0"
    assert circuit.node("FILTER.VIN") == circuit.node("FILTER/R1.1")
    assert board_to_json(hierarchical) == original


RAW = """Title: test
Plotname: AC Analysis
Flags: complex
No. Variables: 3
No. Points: 2
Variables:
 0 frequency frequency
 1 v(n000001) voltage
 2 v(n000002) voltage
Values:
0 10,0
 1,0
 .5,-.5

1 100,0
 1,0
 .1,-.2
"""


def test_ascii_raw_preserves_complex_data_and_rejects_truncation():
    plot = parse_raw(RAW)[0]
    assert plot.vectors["v(n000002)"][0] == .5 - .5j
    for malformed in (RAW.rsplit(".1,-.2", 1)[0], RAW.replace("1 100,0", "9 100,0"), RAW.replace(".1,-.2", "nan,0")):
        with pytest.raises(SimulationError):
            parse_raw(malformed)


def test_phase_unwrapping_resets_at_invalid_and_zero_samples():
    values = tuple(complex(math.cos(math.radians(a)), math.sin(math.radians(a))) for a in (179, -179))
    trace = Trace("phase", "1", (*values, None, values[1], 0j))
    assert represented(trace, "phase", unwrap=True) == pytest.approx((179, 181, None, -179, None))
    assert represented(Trace("zero", "1", (0j,)), "dB") == (None,)


def test_nonuniform_inrush_measurements_keep_narrow_peaks():
    from pcbir.simulation.model import Analysis
    analysis = Analysis("startup", "transient", Time.of(0, "s"), Time.of(1, "ms"), Time.of(1, "us"))
    data = Waveforms(analysis, "nominal", "time", "s", (0, .00049, .0005, .000501, .001), {"current": Trace("current", "A", (0j, 0j, 10+0j, 0j, 0j))})
    maximum = evaluate({"name": "peak", "probe": "current", "operation": "max", "maximum": "1 A"}, data)
    charge = evaluate({"name": "charge", "probe": "current", "operation": "integral", "maximum": "60 uC"}, data)
    assert maximum["status"] == "failed" and maximum["value"] == 10
    assert charge["status"] == "passed" and charge["value"] == pytest.approx(55e-6)
    outside = evaluate({"name": "outside", "probe": "current", "operation": "at", "at": "2 ms", "maximum": "1 A"}, data)
    assert outside["status"] == "incomplete"


def test_timeout_kills_the_simulator_process(tmp_path):
    code, output, timed_out = _execute([sys.executable, "-c", "import time; time.sleep(5)"], tmp_path, .1)
    assert timed_out and code != 0


def test_stale_output_is_never_reused(board, tmp_path):
    output = tmp_path / "old"; output.mkdir()
    marker = output / "run.raw"; marker.write_text("stale")
    with pytest.raises(SimulationError, match="empty"):
        export_simulation(board, parse_plan(plan_data()), output)
    assert marker.read_text() == "stale"


def model_fixture(tmp_path):
    child = tmp_path / "gain.lib"
    child.write_bytes(b".subckt GAIN10 INP INN VCC VEE OUT\nEGAIN OUT VEE INP INN 10\n.ends GAIN10\n")
    root = tmp_path / "root.lib"; root.write_bytes(b'.include "gain.lib"\n')
    entry = {"kind": "subcircuit", "name": "GAIN10", "terminals": ["INP", "INN", "VCC", "VEE", "OUT"],
        "pin_map": {"INP": "PLUS", "INN": "MINUS", "VCC": "VP", "VEE": "VN", "OUT": "OUT"}, "supported_parts": ["TEST_AMP"],
        "analyses": ["op", "ac"], "ngspice_versions": [46, 47], "entrypoint": "root.lib",
        "files": [{"path": p.name, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in (root, child)],
        "license": "MIT", "assumptions": ["Educational ideal gain block; no supply clipping or startup model."]}
    registry = tmp_path / "models.json"; registry.write_text(json.dumps({"schema_version": 1, "models": {"test/gain10": entry}}))
    return registry, entry, child


def test_model_bundle_checks_hashes_includes_and_terminal_order(tmp_path):
    registry, entry, child = model_fixture(tmp_path)
    bundle = load_registry(registry)["test/gain10"]
    assert len(bundle.assets) == 2 and 'models/' in dict(bundle.assets)[bundle.entrypoint]
    child.write_bytes(child.read_bytes() + b"* changed\n")
    with pytest.raises(SimulationError, match="SHA-256 mismatch"):
        load_registry(registry)
    entry["files"][1]["sha256"] = hashlib.sha256(child.read_bytes()).hexdigest()
    entry["terminals"] = ["OUT", "INN", "VCC", "VEE", "INP"]
    registry.write_text(json.dumps({"schema_version": 1, "models": {"test/gain10": entry}}))
    with pytest.raises(SimulationError, match="terminal order"):
        load_registry(registry)


@pytest.mark.parametrize("payload", ['.control\nshell echo unwanted\n.endc\n', '.include "../outside.lib"\n'])
def test_model_control_scripts_and_unchecked_includes_are_rejected(tmp_path, payload):
    registry, entry, child = model_fixture(tmp_path)
    child.write_bytes(payload.encode())
    entry["files"][1]["sha256"] = hashlib.sha256(child.read_bytes()).hexdigest()
    registry.write_text(json.dumps({"schema_version": 1, "models": {"test/gain10": entry}}))
    with pytest.raises(SimulationError):
        load_registry(registry)


def test_real_examples_generate_checked_offline_graphs_and_complex_exports(example_results):
    folder, results = example_results
    assert all(s["status"] == "passed" and s["output_status"] == "complete" for s in results.values())
    rc = folder / "rc_filter"
    report = (rc / "report.html").read_text(encoding="utf-8")
    assert "Plotly.newPlot" in report and '<script src=' not in report
    assert "scrollZoom:true" in report and "high_capacitance" in report
    for relative in ("plots/frequency/bode.svg", "plots/frequency/bode.png", "plots/startup/startup.svg", "data/frequency/nominal/waveforms.csv"):
        assert (rc / relative).stat().st_size > 100
    data = json.loads((rc / "data/frequency/nominal/waveforms.json").read_text())
    frequency = data["axis"]["values"]
    values = data["traces"]["gain"]
    for f, real, imaginary in zip(frequency, values["real"], values["imaginary"]):
        expected = 1 / (1 + 1j * 2 * math.pi * f * 1000 * 100e-9)
        assert complex(real, imaginary) == pytest.approx(expected, rel=1e-7)
    with (rc / "data/frequency/nominal/waveforms.csv").open(newline="") as stream:
        csv_data = list(csv.DictReader(stream))
    assert float(csv_data[0]["gain.imaginary[1]"]) == pytest.approx(values["imaginary"][0])
    metrics = json.loads((rc / "measurements.json").read_text())
    nominal_bandwidth = next(m["value"] for m in metrics if m["name"] == "bandwidth" and m["case"] == "nominal")
    assert nominal_bandwidth == pytest.approx(1/(2*math.pi*1000*100e-9), rel=1e-3)
    inrush = json.loads((folder / "inrush/measurements.json").read_text())
    assert next(m["value"] for m in inrush if m["name"] == "peak_inrush" and m["case"] == "nominal") == pytest.approx(.5, rel=1e-4)
    assert next(m["value"] for m in inrush if m["name"] == "charging_charge" and m["case"] == "nominal") == pytest.approx(500e-6, rel=1e-4)


@pytest.mark.parametrize("sweep,resolution,points,stop", [("decade", "points_per_decade", 10, "1 kHz"), ("octave", "points_per_octave", 5, "2560 Hz"), ("linear", "points", 21, "1 kHz")])
def test_real_sweep_grids_and_gain_normalization(board, engine, tmp_path, sweep, resolution, points, stop):
    data = plan_data(); data["analyses"] = {"frequency": {"kind": "ac", "sweep": sweep, "start": "10 Hz", "stop": stop, resolution: points}}
    data["sources"]["input"]["ac"]["magnitude"] = "250 mV"
    result = run_simulation(board, parse_plan(data), tmp_path / "run", ngspice=engine)
    assert result["status"] == "completed"
    waves = json.loads((tmp_path / "run/data/frequency/nominal/waveforms.json").read_text())
    for f, real, imag in zip(waves["axis"]["values"], waves["traces"]["gain"]["real"], waves["traces"]["gain"]["imaginary"]):
        assert complex(real, imag) == pytest.approx(1 / (1 + 1j * 2 * math.pi * f * 1000 * 100e-9), rel=1e-7)


def test_real_vendor_style_pin_map_and_transitive_model_bundle(engine, tmp_path):
    registry, entry, child = model_fixture(tmp_path)
    pins = {n: PackagePinDefinition(n, number) for n, number in {"PLUS": "3", "MINUS": "2", "VP": "7", "VN": "4", "OUT": "6"}.items()}
    part = PartDefinition("TEST_AMP", pins, category="analog.amplifier")
    board = Board("ModelPinMap", {"TEST_AMP": part}, (ComponentInstance("U1", "TEST_AMP"),), (
        Net("VIN", (Endpoint("U1", "PLUS"),)), Net("GND", (Endpoint("U1", "MINUS"),)),
        Net("VP", (Endpoint("U1", "VP"),)), Net("VN", (Endpoint("U1", "VN"),)), Net("VOUT", (Endpoint("U1", "OUT"),))))
    data = {"schema_version": 1, "name": "model-pin-map", "circuit": {"all": True, "reference_node": "GND"},
        "models": {"registry": str(registry), "bindings": {"U1": "test/gain10"}},
        "sources": {"input": {"kind": "voltage", "positive": "VIN", "negative": "GND", "ac": {"magnitude": "1 V"}},
                    "positive_supply": {"kind": "voltage", "positive": "VP", "negative": "GND", "dc": "5 V"},
                    "negative_supply": {"kind": "voltage", "positive": "VN", "negative": "GND", "dc": "-5 V"}},
        "analysis": {"kind": "ac", "sweep": "linear", "start": "100 Hz", "stop": "10 kHz", "points": 10},
        "probes": {"vin": {"kind": "voltage", "positive": "VIN", "negative": "GND"}, "vout": {"kind": "voltage", "positive": "VOUT", "negative": "GND"}},
        "derived_signals": {"gain": {"kind": "ratio", "numerator": "vout", "denominator": "vin"}},
        "checks": [{"name": "gain", "probe": "gain", "operation": "min", "representation": "magnitude", "minimum": 9.99, "maximum": 10.01}]}
    result = run_simulation(board, parse_plan(data), tmp_path / "model-run", ngspice=engine)
    assert result["status"] == "passed"
    assert json.loads((tmp_path / "model-run/measurements.json").read_text())[0]["value"] == pytest.approx(10)


def test_real_diode_card_and_native_terminal_order(engine, tmp_path):
    library = tmp_path / "diode.lib"
    library.write_bytes(b".model TESTD D(Is=1e-12 N=1)\n")
    entry = {"kind": "diode", "name": "TESTD", "terminals": ["A", "K"],
        "pin_map": {"A": "ANODE", "K": "CATHODE"}, "supported_parts": ["TEST_DIODE"],
        "analyses": ["op"], "ngspice_versions": [46, 47], "entrypoint": "diode.lib",
        "files": [{"path": "diode.lib", "sha256": hashlib.sha256(library.read_bytes()).hexdigest()}], "license": "MIT"}
    registry = tmp_path / "models.json"
    registry.write_text(json.dumps({"schema_version": 1, "models": {"diode": entry}}))
    part = PartDefinition("TEST_DIODE", {"ANODE": PackagePinDefinition("ANODE", "1"), "CATHODE": PackagePinDefinition("CATHODE", "2")}, category="semiconductor.diode")
    circuit = Board("Diode", {"TEST_DIODE": part}, (ComponentInstance("D1", "TEST_DIODE"),), (
        Net("BIAS", (Endpoint("D1", "ANODE"),)), Net("GND", (Endpoint("D1", "CATHODE"),))))
    data = {"schema_version": 1, "name": "diode", "circuit": {"all": True, "reference_node": "GND"},
        "models": {"registry": str(registry), "bindings": {"D1": "diode"}},
        "sources": {"bias": {"kind": "voltage", "positive": "BIAS", "negative": "GND", "dc": "0.6 V"}},
        "analysis": {"kind": "op"}, "probes": {"current": {"kind": "source_current", "source": "bias"}},
        "checks": [{"name": "forward_current", "probe": "current", "operation": "min", "minimum": "5 mA", "maximum": "20 mA"}]}
    result = run_simulation(circuit, parse_plan(data), tmp_path / "diode-run", ngspice=engine)
    assert result["status"] == "passed"
    entry["terminals"] = ["K", "A"]
    registry.write_text(json.dumps({"schema_version": 1, "models": {"diode": entry}}))
    with pytest.raises(SimulationError, match="native card order"):
        load_registry(registry)


def test_real_failed_requirement_still_has_graphs(board, engine, tmp_path):
    data = plan_data(); data["analyses"] = {"startup": data["analyses"]["startup"]}; data["derived_signals"] = {}
    data["checks"] = [{"name": "deliberate_failure", "probe": "output_voltage", "operation": "max", "maximum": "0.1 V"}]
    result = run_simulation(board, parse_plan(data), tmp_path / "failed", ngspice=engine)
    assert result["status"] == "failed"
    assert (tmp_path / "failed/plots/startup/waveforms.png").is_file()


def test_real_operating_point(board, engine, tmp_path):
    data = plan_data()
    data["analyses"] = {"bias": {"kind": "op"}}
    data["derived_signals"] = {}
    data["sources"]["input"].pop("waveform")
    data["sources"]["input"]["dc"] = "1 V"
    data["checks"] = [{"name": "output_bias", "probe": "output_voltage", "operation": "min", "minimum": "0.999 V", "maximum": "1.001 V"}]
    result = run_simulation(board, parse_plan(data), tmp_path / "op", ngspice=engine)
    assert result["status"] == "passed"
    assert result["runs"][0]["samples"] == 1


def test_successful_exit_without_waveforms_is_a_failure(board, tmp_path, monkeypatch):
    import pcbir.simulation.ngspice as runner
    monkeypatch.setattr(runner, "engine_identity", lambda *a: {"major_version": 46})
    monkeypatch.setattr(runner, "_execute", lambda *a: (0, "reported success without results", False))
    monkeypatch.setattr(runner, "require_reporting", lambda: None)
    monkeypatch.setattr(runner, "render_report", lambda *a: None)
    result = run_simulation(board, parse_plan(plan_data()), tmp_path / "missing", ngspice=sys.executable)
    assert result["status"] == "failed"
    assert all("missing or unreadable raw" in r["error"] for r in result["runs"])


def test_commit_ci_requires_simulator_and_keeps_release_only_routing():
    workflow = (ROOT / ".github/workflows/board-routing.yml").read_text()
    test_job = workflow.split("  test:\n", 1)[1].split("  route:\n", 1)[0]
    route_job = workflow.split("  route:\n", 1)[1].split("  publish:\n", 1)[0]
    assert "COPPER_SIM_REQUIRED" in test_job and "id: ngspice" in test_job
    assert "matplotlib==3.10.6" in test_job and "plotly==6.3.0" in test_job
    assert "Simulation reports" in test_job
    assert test_job.count("if: ${{ always() && steps.ngspice.outcome == 'success' }}") == 2
    assert "run: python -m pytest" in test_job
    assert "startsWith(github.ref, 'refs/tags/v')" in route_job

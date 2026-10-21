"""Analog requirement gates and the independently bounded vendor-model fixture."""
import importlib.util
import json
import os
from pathlib import Path

import pytest

from pcbir.loader import load_board
from pcbir.model import Board, ComponentInstance, Endpoint, Net
from pcbir.quantities import Resistance
from pcbir.simulation import SimulationError, load_plan, parse_plan, run_simulation
from pcbir.simulation.result import Trace, Waveforms


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/simulation/analog_frontend"
spec = importlib.util.spec_from_file_location("analog_example_runner", EXAMPLE / "run.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


def test_settling_checks_detect_a_short_late_excursion():
    analysis = load_plan(EXAMPLE / "step.json").analyses[0]
    axis = (0, .0005, .001, .00101, .002, .00301, .0035, .003501, .0045, .005)
    values = (0, 0, 0, .1, .44, .4, .4, .4, .4, .4)
    waves = Waveforms(analysis, "nominal", "time", "s", axis,
                     {"output_signal": Trace("output_signal", "V", tuple(complex(v) for v in values))})
    metrics = runner.step_metrics(waves)
    assert metrics["overshoot_percent"] == pytest.approx(10)
    assert metrics["settled_error_percent"] == pytest.approx(0)
    changed = list(values)
    changed[6] = .42
    faulty = Waveforms(analysis, "nominal", "time", "s", axis,
                      {"output_signal": Trace("output_signal", "V", tuple(complex(v) for v in changed))})
    assert runner.step_metrics(faulty)["settled_error_percent"] == pytest.approx(5)


def test_relative_bandwidth_is_independent_of_absolute_gain():
    analysis = next(a for a in load_plan(EXAMPLE / "frequency.json").analyses if a.kind == "ac")
    import math
    axis = tuple(10 ** (1 + i / 100) for i in range(601))
    for gain in (2, 4):
        values = tuple(gain / (1 + 1j * f / 2000) for f in axis)
        waves = Waveforms(analysis, "corner", "frequency", "Hz", axis, {"gain": Trace("gain", "1", values)})
        result = runner.frequency_checks(waves)[0]
        assert result["status"] == "passed"
        assert result["value"] == pytest.approx(2000 * math.sqrt(1 + 2 * (.05 ** 2)), rel=1e-4)


def test_off_the_shelf_model_on_stiff_four_follower_fixture(tmp_path):
    """This validates model plumbing and a limited bias/AC fixture, not the board."""
    engine = os.environ.get("NGSPICE")
    if not engine:
        pytest.skip("set NGSPICE and prepare the vendor bundle for the off-the-shelf fixture")
    if not (EXAMPLE / "models/vendor/OPAx197.LIB").is_file():
        pytest.skip("run analog_frontend/prepare_model.py to test the separately fetched vendor model")
    pytest.importorskip("matplotlib")
    pytest.importorskip("plotly")
    original = load_board(EXAMPLE / "board.copper", locked=True, offline=True)
    amps = tuple(c for c in original.components if c.ref in {"U1", "U2"})
    loads = tuple(ComponentInstance(f"R{i}", "RESISTOR", value=Resistance.of(10, "kohm")) for i in range(4))
    nets = [Net("VP", tuple(Endpoint(c.ref, "VP") for c in amps)),
            Net("GND", tuple(Endpoint(c.ref, "VN") for c in amps) + tuple(Endpoint(c.ref, "2") for c in loads)),
            Net("VIN", tuple(Endpoint(c.ref, pin) for c in amps for pin in ("INA_P", "INB_P")))]
    for index, (amp, channel) in enumerate((c.ref, channel) for c in amps for channel in ("A", "B")):
        nets.append(Net(f"OUT{index}", (Endpoint(amp, "OUT" + channel), Endpoint(amp, "IN" + channel + "_N"), Endpoint(f"R{index}", "1"))))
    board = Board("VendorFourFollowers", original.library, (*amps, *loads), tuple(nets))
    plan = parse_plan({"schema_version": 1, "name": "vendor-followers", "circuit": {"all": True, "reference_node": "GND"},
        "models": {"registry": str(EXAMPLE / "models/registry.json"), "bindings": {"U1": "ti/opa2197-sboma34d", "U2": "ti/opa2197-sboma34d"}},
        "sources": {"supply": {"kind": "voltage", "positive": "VP", "negative": "GND", "dc": "5 V"},
                    "input": {"kind": "voltage", "positive": "VIN", "negative": "GND", "dc": "2.5 V", "ac": {"magnitude": "1 V"},
                              "waveform": {"kind": "pwl", "points": [["0 s", "2.5 V"], ["0.5 ms", "2.5 V"], ["0.51 ms", "2.6 V"]]}}},
        "analyses": {"bias": {"kind": "op"}, "frequency": {"kind": "ac", "sweep": "decade", "start": "10 Hz", "stop": "100 kHz", "points_per_decade": 20},
                     "step": {"kind": "transient", "stop": "2 ms", "max_step": "1 us"}},
        "probes": {**{f"output{i}": {"kind": "voltage", "positive": f"OUT{i}", "negative": "GND"} for i in range(4)},
                   "input_current": {"kind": "source_current", "source": "input"},
                   "supply_current": {"kind": "source_current", "source": "supply"}},
        "checks": [*({"name": f"dc_output{i}", "analysis": "bias", "probe": f"output{i}", "operation": "at", "at": 0, "minimum": "2.499 V", "maximum": "2.501 V"} for i in range(4)),
                   {"name": "input_bias_current", "analysis": "bias", "probe": "input_current", "operation": "at", "at": 0, "minimum": "-0.001 uA", "maximum": "0.001 uA"},
                   {"name": "supply_current", "analysis": "bias", "probe": "supply_current", "operation": "at", "at": 0, "minimum": "4.8 mA", "maximum": "5.2 mA"},
                   {"name": "ac_gain", "analysis": "frequency", "probe": "output0", "operation": "at", "at": "1 kHz", "representation": "magnitude", "minimum": "0.99 V", "maximum": "1.01 V"},
                   {"name": "late_step", "analysis": "step", "probe": "output0", "operation": "min", "window": ["1 ms", "2 ms"], "minimum": "2.599 V", "maximum": "2.601 V"}]})
    result = run_simulation(board, plan, tmp_path / "vendor output with spaces", ngspice=engine, timeout=30)
    assert result["status"] == "passed" and result["output_status"] == "complete"
    profile = result["provenance"]["engine"]["runtime_profile"]
    assert profile["dialect"] == "pspice" and {item["name"] for item in profile["code_models"]} == {"analog", "xtradev"}
    assert all(len(item["sha256"]) == 64 for item in profile["code_models"])
    output = tmp_path / "vendor output with spaces"
    assert runner.read_waveforms(output, plan, "nominal", "frequency").traces["output0"].values
    waveform_path = output / "data/frequency/nominal/waveforms.json"
    document = json.loads(waveform_path.read_text())
    document["traces"]["output0"]["real"][0] *= 2
    waveform_path.write_text(json.dumps(document))
    with pytest.raises(SimulationError, match="artifact inventory"):
        runner.read_waveforms(output, plan, "nominal", "frequency")

import json
from pathlib import Path

import pytest

from pcbir.power_integrity import (trace_resistance, via_resistance, series_path,
                                   reservoir_droop, converter_demand, thermal_estimate)
from pcbir.qualification import (aggregate, digest_json, external_evidence, load_json)
from pcbir.rf_geometry import reference_coverage, native_fill_polygon
from pcbir.engineering_qualification import load_contract, evaluate_requirement
from pcbir.cam_geometry import verify_vector_copper, rectangular_paste_area_ratio, minimum_polygon_web


def material():
    return dict(resistivity_ohm_m=1.724e-8, temperature_c=20,
                reference_temperature_c=20, temperature_coefficient_per_c=0.00393)


def rectangle(x1, y1, x2, y2):
    return {"outer": [[x1, y1], [x2, y1], [x2, y2], [x1, y2]], "holes": []}


def native():
    return {"layers": ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"],
            "tracks": [{"net": "RF", "layer": "F.Cu", "width_nm": 200,
                        "start": [1000, 2000], "end": [9000, 2000]}],
            "fills": [{"net": "GND", "layer": "In1.Cu", **rectangle(0, 0, 10000, 10000)}]}


def test_temperature_and_dimensions_affect_resistance():
    parameters = dict(length_m=0.1, width_m=0.001, thickness_m=35e-6, **material())
    expected = 1.724e-8 * 0.1 / (0.001 * 35e-6)
    assert trace_resistance(**parameters) == pytest.approx(expected)
    assert trace_resistance(**{**parameters, "width_m": 0.0005}) == pytest.approx(expected * 2)
    assert trace_resistance(**{**parameters, "temperature_c": 120}) == pytest.approx(expected * 1.393)
    assert via_resistance(length_m=0.0016, finished_hole_diameter_m=0.0003,
                          plating_thickness_m=20e-6, **material()) > 0


@pytest.mark.parametrize("bad", [0, -1, True, float("nan"), float("inf"), "0.001"])
def test_trace_dimensions_reject_invalid_inputs(bad):
    with pytest.raises(ValueError):
        trace_resistance(length_m=0.1, width_m=bad, thickness_m=35e-6, **material())


def test_series_path_checks_peak_rms_and_never_assumes_branch_topology():
    values = series_path(resistance_ohms=[0.1, 0.2], peak_current_a=2, rms_current_a=1)
    assert values["peak_drop_v"] == pytest.approx(0.6)
    assert values["rms_loss_w"] == pytest.approx(0.3)
    with pytest.raises(ValueError):
        series_path(resistance_ohms=[], peak_current_a=2, rms_current_a=1)
    with pytest.raises(ValueError):
        series_path(resistance_ohms=[1], peak_current_a=1, rms_current_a=2)


def test_droop_uses_effective_capacitance_and_esr():
    result = reservoir_droop(load_step_a=1, duration_s=0.001, effective_capacitance_f=0.001, esr_ohms=0.1)
    assert result["total_drop_v"] == pytest.approx(1.1)
    result = converter_demand(minimum_input_v=3, output_v=3.8, output_current_a=2,
                              minimum_efficiency=0.8, quiescent_current_a=0.001)
    assert result["input_current_a"] == pytest.approx(7.6 / 2.4 + 0.001)
    with pytest.raises(ValueError):
        converter_demand(minimum_input_v=3, output_v=3.8, output_current_a=2,
                         minimum_efficiency=1.1, quiescent_current_a=0)
    assert thermal_estimate(power_w=1.5, thermal_resistance_k_per_w=20, ambient_c=25,
                            maximum_c=85, model_source="fixture board model")["estimated_temperature_c"] == 55


def coverage(value, **kwargs):
    return reference_coverage(value, identifier="RF", net="RF", reference_net="GND",
                              reference_layer=kwargs.get("reference_layer", "In1.Cu"), margin_nm=0)


def test_reference_coverage_checks_whole_segment_not_midpoint():
    value = native()
    assert coverage(value)["status"] == "pass"
    value["fills"][0]["outer"] = rectangle(4000, 0, 6000, 10000)["outer"]
    assert coverage(value)["status"] == "fail"


def test_reference_holes_missing_fill_wrong_layer_and_overlapping_union():
    value = native()
    value["fills"][0]["holes"] = [rectangle(1200, 1900, 1400, 2100)["outer"]]
    assert coverage(value)["status"] == "fail"
    value["fills"] = []
    assert coverage(value)["status"] == "incomplete"
    value = native()
    assert coverage(value, reference_layer="In2.Cu")["status"] == "fail"
    value["fills"] = [{"net": "GND", "layer": "In1.Cu", **rectangle(0, 0, 6000, 10000)},
                      {"net": "GND", "layer": "In1.Cu", **rectangle(5000, 0, 10000, 10000)}]
    assert coverage(value)["status"] == "pass"


def test_invalid_reference_fill_is_not_healed():
    value = native()
    value["fills"][0]["outer"] = [[0, 0], [10000, 10000], [0, 10000], [10000, 0]]
    assert coverage(value)["status"] == "incomplete"


def test_native_retraced_slit_preserves_hole_exactly():
    from shapely.geometry import Point
    value = {"outer": [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0],
                       [3, 3], [3, 7], [7, 7], [7, 3], [3, 3], [0, 0]], "holes": []}
    shape = native_fill_polygon(value)
    assert shape.area == 84
    assert shape.covers(Point(1, 1))
    assert not shape.covers(Point(5, 5))


def evidence(inputs):
    return {"schema": "copperscript-engineering-evidence/v0.1", "id": "radio.impedance",
            "stage": "simulation", "inputs_sha256": digest_json(inputs), "status": "pass",
            "reviewed_by": "fixture reviewer", "report_sha256": "1" * 64,
            "settings_sha256": "2" * 64, "model_sha256": "3" * 64,
            "tool": {"name": "fixture solver", "version": "1.0", "identity_sha256": "4" * 64}}


def test_external_evidence_stale_scope_stage_and_missing_review():
    inputs = {"pcb": "a", "stackup": "b", "parts": "c"}
    item = evidence(inputs)
    verify = lambda data, context=inputs: external_evidence("radio.impedance", "simulation", context, data)
    assert verify(item)["status"] == "pass"
    assert verify(item, {**inputs, "stackup": "changed"})["status"] == "fail"
    assert verify({**item, "stage": "bench"})["status"] == "fail"
    assert verify({**item, "id": "other.impedance"})["status"] == "fail"
    assert verify({**item, "reviewed_by": ""})["status"] == "incomplete"
    assert verify(None)["status"] == "incomplete"
    assert aggregate([]) == "incomplete"


@pytest.mark.parametrize("content", ['{"a":1,"a":2}', '{"a":NaN}', '[]'])
def test_strict_json_rejects_duplicate_nonfinite_or_wrong_root(tmp_path, content):
    path = tmp_path / "input.json"
    path.write_text(content)
    with pytest.raises(ValueError):
        load_json(path)


def test_no_empty_or_weakened_rf_contract(tmp_path):
    path = tmp_path / "contract.json"
    contract = {"schema": "copperscript-engineering-contract/v0.1", "id": "RF", "kind": "rf", "matching": "required", "requirements": []}
    path.write_text(json.dumps(contract))
    with pytest.raises(ValueError):
        load_contract(path)


def test_missing_bound_operating_inputs_are_incomplete():
    requirement = {"algorithm": "calculation", "stage": "design"}
    assert evaluate_requirement("power", requirement, None, None, {}, None)["status"] == "incomplete"
    assert evaluate_requirement("power", requirement, {"effective_capacitance_f": None}, None, {}, None)["status"] == "incomplete"


def test_net_width_screen_catches_one_narrow_segment():
    value = native()
    value["tracks"].append({**value["tracks"][0], "width_nm": 20})
    requirement = {"algorithm": "route_width", "stage": "design"}
    binding = {"basis": "fixture requirement", "net": "RF", "limits": {"minimum_track_width_nm": {"minimum": 100}}}
    assert evaluate_requirement("width", requirement, binding, value, {}, None)["status"] == "fail"


def contact(net, layer, x1, x2):
    return {"net": net, "layer": layer, **rectangle(x1, 1, x2, 2)}


def vector(layers, contacts, links=()):
    return verify_vector_copper(layers, contacts, list(links), minimum_clearance_nm=2)


def test_vector_cam_detects_open_and_short_from_unlabelled_copper():
    contacts = [contact("A", "F", 1, 2), contact("A", "F", 8, 9)]
    assert vector({"F": [rectangle(0, 0, 10, 3)]}, contacts)[0]["status"] == "pass"
    assert vector({"F": [rectangle(0, 0, 4, 3), rectangle(6, 0, 10, 3)]}, contacts)[0]["status"] == "fail"
    contacts[1]["net"] = "B"
    assert vector({"F": [rectangle(0, 0, 10, 3)]}, contacts)[0]["status"] == "fail"


def test_vector_link_connects_layers_and_requires_complete_annuli():
    layers = {"F": [rectangle(0, 0, 10, 3)], "B": [rectangle(0, 0, 10, 3)]}
    contacts = [contact("A", "F", 1, 2), contact("A", "B", 8, 9)]
    link = {"surfaces": {"F": rectangle(3, 1, 4, 2), "B": rectangle(3, 1, 4, 2)}}
    assert vector(layers, contacts)[0]["status"] == "fail"
    assert vector(layers, contacts, [link])[0]["status"] == "pass"
    link["surfaces"]["B"] = rectangle(30, 1, 40, 2)
    assert vector(layers, contacts, [link])[0]["status"] == "fail"


def test_vector_spacing_and_unassigned_islands_are_not_passes():
    layers = {"F": [rectangle(0, 0, 4, 3), rectangle(5, 0, 10, 3)]}
    contacts = [contact("A", "F", 1, 2), contact("B", "F", 8, 9)]
    assert vector(layers, contacts)[1]["status"] == "fail"
    layers["F"].append(rectangle(20, 0, 30, 3))
    assert vector(layers, contacts)[0]["status"] == "incomplete"
    with pytest.raises(ValueError):
        vector({"F": [{"outer": [[0, 0], [10, 10], [0, 10], [10, 0]]}]}, contacts)


def test_mask_and_paste_screening():
    assert rectangular_paste_area_ratio(width_nm=1000, height_nm=1000, stencil_thickness_nm=100) == pytest.approx(2.5)
    assert minimum_polygon_web([rectangle(0, 0, 10, 10), rectangle(11, 0, 20, 10)], minimum_web_nm=2)["status"] == "fail"

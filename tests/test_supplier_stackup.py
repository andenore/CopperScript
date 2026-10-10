"""Supplier construction selection must not fabricate solver/minimum facts."""
import json

import pytest

from pcbir.stackup_qualification import (supplier_stackup_check, resolve_stackup_context,
                                        engineering_input_files, outer_microstrip_inventory)


def context():
    source = dict(url="https://supplier.example/stackup", revision="fixture", locator="table", sha256="a" * 64)
    return dict(reviewed=True, source=source, capabilities_source=source,
                selected_profile="fixture", nominal_finished_thickness_nm=200000,
                finished_thickness_tolerance_fraction=.1,
                copper_layers=["F.Cu", "B.Cu"], finished_via_plating_nm=None,
                physical_layers=[dict(kind="copper", name="F.Cu", thickness_nm=35000),
                                 dict(kind="dielectric", name="P1", thickness_nm=120000,
                                      relative_permittivity=4.1, loss_tangent=None),
                                 dict(kind="copper", name="B.Cu", thickness_nm=35000)])


def native():
    return dict(layers=["F.Cu", "B.Cu"], board_thickness_nm=200000,
                tracks=[dict(net="RF", layer="F.Cu", width_nm=180000, start=[0, 0], end=[1000000, 0])], fills=[])


def test_partial_supplier_facts_report_geometry_without_qualification():
    report = supplier_stackup_check(context(), native())
    assert report["status"] == "incomplete"
    assert report["metrics"]["construction_thickness_nm"] == 190000
    assert len(report["metrics"]["missing_inputs"]) == 2
    complete = context()
    complete["finished_via_plating_nm"] = 15000
    complete["physical_layers"][1]["loss_tangent"] = .02
    assert supplier_stackup_check(complete, native())["status"] == "pass"


@pytest.mark.parametrize("field,value", [("finished_thickness_tolerance_fraction", 1),
    ("nominal_finished_thickness_nm", 300000), ("finished_via_plating_nm", True)])
def test_invalid_supplier_dimensions_fail(field, value):
    selected = context()
    selected[field] = value
    assert supplier_stackup_check(selected, native())["status"] == "fail"


def test_average_plating_and_native_mismatch_never_pass():
    selected = context()
    selected["published_average_hole_plating_nm"] = 18000
    assert supplier_stackup_check(selected, native())["status"] == "incomplete"
    board = native()
    board["board_thickness_nm"] += 2
    assert supplier_stackup_check(selected, board)["status"] == "fail"
    board = native()
    board["layers"].reverse()
    assert supplier_stackup_check(selected, board)["status"] == "fail"


def test_profile_is_content_bound_and_cannot_be_overridden(tmp_path):
    profile = tmp_path / "profile.json"
    selected = context()
    selected["schema"] = "copperscript-fabrication-stackup/v0.1"
    profile.write_text(json.dumps(selected))
    plan = {"context": {"stackup": dict(profile_path="profile.json", reviewed=True)}}
    first = engineering_input_files(tmp_path / "plan.json", plan)
    assert resolve_stackup_context(tmp_path / "plan.json", plan)["stackup"]["selected_profile"] == "fixture"
    profile.write_text(json.dumps({**selected, "selected_profile": "changed"}))
    assert engineering_input_files(tmp_path / "plan.json", plan) != first
    plan["context"]["stackup"]["physical_layers"] = []
    with pytest.raises(ValueError, match="not overridden"):
        resolve_stackup_context(tmp_path / "plan.json", plan)


def test_inventory_uses_actual_widths_and_does_not_approve_impedance():
    board = native()
    board["fills"] = [dict(net="GND", layer="B.Cu", outer=[[-1000000,-1000000],[2000000,-1000000],[2000000,1000000],[-1000000,1000000]], holes=[])]
    board["tracks"].append({**board["tracks"][0], "width_nm": 250000})
    result = outer_microstrip_inventory(board, context(), identifier="width-screen", net="RF", reference_layer="B.Cu", reference_net="GND")
    assert result["status"] == "incomplete"
    entries = result["metrics"]["width_inventory"]
    assert entries[0]["estimated_single_ended_ohms"] > entries[1]["estimated_single_ended_ohms"]
    assert entries[0]["reference_coverage"]["status"] == "pass"


def test_no_reference_fill_has_only_hypothetical_formula_value():
    result = outer_microstrip_inventory(native(), context(), identifier="width", net="RF", reference_layer="B.Cu", reference_net="GND")
    item = result["metrics"]["width_inventory"][0]
    assert item["estimated_single_ended_ohms"] is None
    assert item["isolated_formula_ohms"] > 0
    assert item["reference_coverage"]["status"] == "incomplete"


def test_interior_microstrip_is_not_guessed():
    board, selected = native(), context()
    board["layers"] = selected["copper_layers"] = ["F.Cu", "In1.Cu", "B.Cu"]
    board["tracks"][0]["layer"] = "In1.Cu"
    result = outer_microstrip_inventory(board, selected, identifier="USB", net="RF", reference_layer="F.Cu", reference_net="GND")
    assert result["metrics"]["width_inventory"][0]["estimated_single_ended_ohms"] is None

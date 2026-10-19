from dataclasses import replace
from fractions import Fraction

import pytest

from pcbir import PhysicalNet, PadReference, Placement, Point, TrackSegment, CopperLayer
from pcbir.power_extract import CopperMaterial, PowerPort, PowerElectricalModel, screen_power_model
from test_power_geometry import board as source_board


def fixture():
    b = source_board()
    pads = (PadReference('GS', '1'), PadReference('GL', '1'))
    b = replace(b, placements=(*b.placements, Placement('GS', 'terminal', Point.mm(3, 8)),
        Placement('GL', 'terminal', Point.mm(15, 8))), nets=(*b.nets, PhysicalNet('RETURN', pads)), tracks=(
        TrackSegment('RAIL', Point.mm(3, 5), Point.mm(15, 5), 1000000, CopperLayer.FRONT),
        TrackSegment('RETURN', Point.mm(3, 8), Point.mm(15, 8), 1000000, CopperLayer.FRONT)))
    ports = (
        PowerPort('source', 'RAIL', PadReference('SRC', '1'), CopperLayer.FRONT, 0, '3.3'),
        PowerPort('load', 'RAIL', PadReference('LOAD', '1'), CopperLayer.FRONT, '0.1', minimum_voltage_v='3.29', local_return='return'),
        PowerPort('ground', 'RETURN', pads[0], CopperLayer.FRONT, 0, 0),
        PowerPort('return', 'RETURN', pads[1], CopperLayer.FRONT, '-0.1'),
    )
    model = PowerElectricalModel('RAIL', ports, (CopperMaterial(CopperLayer.FRONT, 35000, '1.68e-8', 20, 'analytic fixture'),),
                                 pitch_nm=250000, maximum_cells=4096)
    return b, model


def test_actual_copper_mesh_solves_supply_and_return_and_material_scaling():
    b, model = fixture()
    first = screen_power_model(b, model)
    assert first.status == 'dc_mesh_screen_pass'
    assert first.solution.maximum_kcl_residual_a <= 1e-9
    second = screen_power_model(b, replace(model, materials=(replace(model.materials[0], resistivity_ohm_m='3.36e-8'),)))
    n, _ = first.limits[0]
    ground = dict(first.references)[n]
    v1 = first.solution.voltages_v[n] - first.solution.voltages_v[ground]
    v2 = second.solution.voltages_v[n] - second.solution.voltages_v[ground]
    assert 3.3-v2 == pytest.approx(2*(3.3-v1), rel=1e-5)
    assert first.solution.voltages_v[ground] > 0
    assert 0.001 < 3.3-v1 < 0.003  # Both 12-square conductors plus port spreading.


def test_mesh_neck_and_hole_are_not_healed_or_treated_as_connected():
    b, model = fixture()
    # A sub-cell neck retains explicit copper continuity, but is unresolved at
    # this extraction pitch. This cannot be promoted to a zero-resistance join.
    b = replace(b, tracks=(replace(b.tracks[0], width_nm=100000), b.tracks[1]))
    screen = screen_power_model(b, model)
    assert screen.status == 'unresolved'
    assert 'missing_voltage_boundary' in screen.unresolved


def test_track_union_cannot_heal_a_drilled_void_in_the_extracted_sheet():
    from pcbir.physical import MechanicalHole
    b, model = fixture()
    b = replace(b, mechanical_holes=(MechanicalHole('cut', Point.mm(9, 5), 1500000),))
    result = screen_power_model(b, model)
    assert result.status == 'unresolved' and 'missing_voltage_boundary' in result.unresolved


def test_missing_current_return_material_and_bounds_are_visible():
    b, model = fixture()
    assert screen_power_model(b, replace(model, ports=model.ports[:1])).unresolved == ('terminal_current_obligations_missing',)
    assert screen_power_model(b, replace(model, ports=tuple(replace(p, local_return=None) if p.name == 'load' else p for p in model.ports))).unresolved == ('load_limits_or_local_returns_missing',)
    assert screen_power_model(b, replace(model, maximum_cells=1)).unresolved == ('mesh_cell_budget_exhausted',)
    assert screen_power_model(b, replace(model, materials=())).unresolved == ('electrical_inputs_missing',)


def test_voltage_failures_and_return_bounce_are_not_geometric_passes():
    b, model = fixture()
    ports = tuple(replace(p, minimum_voltage_v='3.2999') if p.name == 'load' else p for p in model.ports)
    assert screen_power_model(b, replace(model, ports=ports)).status == 'voltage_limit_failed'


def test_stale_native_evidence_and_float_inputs_rejected():
    from pcbir.plane_verify import PlaneVerification
    b, model = fixture()
    evidence = PlaneVerification(False, 'stale', 'stale', 'filled', 'report', 'test', 0, 0, 0, ())
    assert screen_power_model(b, model, evidence).unresolved == ('stale_or_unbound_native_evidence',)
    with pytest.raises(ValueError, match='exact'):
        replace(model.materials[0], resistivity_ohm_m=1.68e-8)


def test_two_mesh_refinement_requires_explicit_tolerance_and_finite_budget():
    from pcbir.power_extract import refine_power_model
    b, model = fixture()
    first = refine_power_model(b, model, None)
    assert first['status'] == 'refinement_tolerance_missing'
    refined = refine_power_model(b, replace(model, refinement_tolerance_v='0.0001'), None)
    assert refined['status'] == 'refinement_screen_pass'
    assert refined['fine']['pitch_nm'] == model.pitch_nm//2
    failed = refine_power_model(b, replace(model, maximum_cells=1), None)
    assert failed['status'] == 'refinement_unresolved'


def test_native_area_adoption_and_source_fragment_observation(tmp_path):
    import shutil
    from pcbir.power_geometry import PowerAreaOptions, plan_power_areas, apply_power_area_plan
    from pcbir.power_extract import assess_power_plan_fill
    from pcbir.plane_verify import verify_filled_planes
    cli = shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable')
    b = source_board()
    options = PowerAreaOptions(candidates_per_domain=1)
    plan = plan_power_areas(b, options)
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending'
    native = verify_filled_planes(routed, kicad_cli=cli, artifact_directory=tmp_path/'native')
    assert native.other_violation_count == 0 and native.unconnected_count == 0
    assert native.filled_regions
    assert assess_power_plan_fill(routed, plan, native, options)['status'] == 'unresolved'
    before = verify_filled_planes(b, kicad_cli=cli, artifact_directory=tmp_path/'before')
    observation = assess_power_plan_fill(routed, plan, native, options, baseline_evidence=before)
    assert observation['status'] == 'native_connected'
    assert observation['electrical_inputs_missing'] == ['RAIL']
    assert assess_power_plan_fill(b, plan, native, options)['status'] == 'stale_or_unbound_native_evidence'


def test_standard_pipeline_uses_plan_inside_placement_and_before_detail():
    from pcbir import (run_routing_pipeline, PlacementPlannerOptions, GlobalRouterOptions,
                       PlacementRoutingFeedbackOptions, DetailedRouterOptions)
    from pcbir.power_geometry import PowerAreaOptions
    b = source_board()
    result = run_routing_pipeline(b,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0,
                                                  fixed_references=frozenset(p.reference for p in b.placements)),
        global_options=GlobalRouterOptions(tile_size_nm=2000000),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        detailed_options=DetailedRouterOptions(pitch_nm=500000),
        power_area_options=PowerAreaOptions(candidates_per_domain=1))
    assert result.placement_and_global.power_area_plan.selected
    assert result.power_area_status == 'contact_routed_native_pending'
    assert result.power_area_plan.selected
    assert result.board.zones and result.board.placements == b.placements

"""Automatic alternatives retain exact ownership and never admit partial DRC."""
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir.copper_ownership import (begin_copper_ownership, append_copper_ownership,
    prospective_contact_uses, prepare_copper_trial, rebuild_copper_ownership)
from pcbir.drc import physical_board_digest
from pcbir.joint_preparation import (JointPreparationOptions, negotiate_preparation_contacts,
                                     joint_reservations_preserved, joint_routing_corridors)
from pcbir.physical import (CopperLayer, CopperZone, PadReference, PhysicalNet, Placement,
                            Point, PolygonRing, PolygonWithHoles, TrackSegment, Via)
from pcbir.plane import PlaneStitchOptions, propose_plane_contacts
from pcbir.plane_verify import verify_filled_planes
from pcbir.pin_escape import verified_fanout_path
from pcbir.routing_clearance import RoutingClearanceIndex
from test_escape_assignment import board, opts


def scene(renamed=False):
    source = board()
    ground = 'RETURN_Z' if renamed else 'GND'
    zone = CopperZone('reference', ground, (CopperLayer.BACK,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((1,1),(13,1),(13,13),(1,13))))))
    source = replace(source, placements=(*source.placements, Placement('G', 'one', Point.mm(3,9))),
        nets=(*source.nets, PhysicalNet(ground, (PadReference('G','1'),))), zones=(zone,))
    trace = TrackSegment(ground, Point.mm(3,9), Point.mm(3,7), 250000, CopperLayer.FRONT)
    via = Via(ground, Point.mm(3,7), 800000, 400000)
    original = replace(source, tracks=(trace,), vias=(via,))
    ledger = append_copper_ownership(begin_copper_ownership(source), source, original, 'contacts',
        uses=prospective_contact_uses(original, (PadReference('G','1'),)), trial_mutable=True)
    settings = opts(two_leg_escapes=False, refinement_step_nm=500000, surface_fallback=False)
    return source, original, ledger, settings


@pytest.mark.parametrize('renamed', [False, True])
def test_real_proposers_recover_a_blocked_signal_by_moving_only_its_contact(monkeypatch, renamed):
    import pcbir.fanout as fanout
    monkeypatch.setattr(fanout, '_candidates', lambda *a,**k: (Point.mm(3,7),))
    source, original, ledger, settings = scene(renamed)
    before = physical_board_digest(original)
    result = negotiate_preparation_contacts(original, ledger, settings, only_nets=frozenset({'A'}))
    assert result.baseline.pending_pads == (PadReference('U','1'),)
    assert result.status == 'geometry_proposal', result.report()
    assert result.board.placements == original.placements and result.board.zones == original.zones
    assert result.ownership.matches(result.board) and not result.ownership.frozen
    assert original.vias[0] not in result.board.vias
    assert all(t.net != 'A' for t in result.board.tracks)  # proposal remains separate
    assert verified_fanout_path(result.reservations, PadReference('U','1'), 'A', Point.mm(3,7),
                                RoutingClearanceIndex(result.reservations)) is not None
    assert joint_reservations_preserved(result, result.board)
    assert physical_board_digest(original) == before and ledger.matches(original)
    assert result == negotiate_preparation_contacts(original, ledger, settings, only_nets=frozenset({'A'}))
    # Ordinary obstacles introduced during later area growth invalidate the
    # signal reservations; the stage cannot silently freeze the lost access.
    conflict = replace(result.board, tracks=(*result.board.tracks, original.tracks[0]))
    assert not joint_reservations_preserved(result, conflict)


def test_fixed_unattributed_and_frozen_copper_never_become_movement_authority(monkeypatch):
    import pcbir.fanout as fanout
    monkeypatch.setattr(fanout, '_candidates', lambda *a,**k: (Point.mm(3,7),))
    _, original, ledger, settings = scene()
    for fixed in (begin_copper_ownership(original),
                  replace(ledger, occurrences=tuple(replace(o, requirements=()) for o in ledger.occurrences))):
        result = negotiate_preparation_contacts(original, fixed, settings, only_nets=frozenset({'A'}))
        assert result.board is original and result.status != 'geometry_proposal'
    with pytest.raises(ValueError, match='disposable'):
        negotiate_preparation_contacts(original, replace(ledger, frozen=True), settings)


def test_cumulative_site_candidate_and_dependency_bounds_return_the_original(monkeypatch):
    import pcbir.fanout as fanout
    monkeypatch.setattr(fanout, '_candidates', lambda *a,**k: (Point.mm(3,7),))
    _, original, ledger, settings = scene()
    for options in (JointPreparationOptions(maximum_generated_candidates=1),
                    JointPreparationOptions(maximum_occurrences=1),
                    JointPreparationOptions(maximum_site_queries=1)):
        result = negotiate_preparation_contacts(original, ledger, settings, options=options, only_nets=frozenset({'A'}))
        assert result.board is original and result.status != 'geometry_proposal'
        assert result.site_queries <= options.maximum_site_queries
        assert result.generated_candidates <= options.maximum_generated_candidates
        assert result.search_states <= options.assignment.maximum_search_states


def test_contact_witness_excludes_unrelated_branches_of_the_same_copper_component():
    _, original, _, _ = scene()
    branch = TrackSegment('GND', Point.mm(3,8), Point.mm(1,8), 250000, CopperLayer.FRONT)
    complete = replace(original, tracks=(*original.tracks, branch))
    use, = prospective_contact_uses(complete, (PadReference('G','1'),))
    assert use.tracks == original.tracks and use.vias == original.vias
    assert not prospective_contact_uses(complete, (PadReference('G','1'),), maximum_states=1)


def test_contact_alternatives_are_finite_fresh_and_use_the_existing_process():
    source, original, ledger, _ = scene()
    frame = prepare_copper_trial(original, ledger, ('via:0',))
    result = propose_plane_contacts(frame.held, PadReference('G','1'),
        PlaneStitchOptions(include_surface_zones=True), maximum_alternatives=4, maximum_sites=32)
    assert result.snapshot == physical_board_digest(source)
    assert 1 < len(result.choices) <= 4 and result.site_queries <= 32
    assert all(v is None or (v.finish == 'standard' and v.drill_nm == 400000) for _,v in result.choices)
    assert source == frame.held and source.via_in_pad_rules == ()


@pytest.mark.parametrize('existing_lead_in', [False, True])
def test_reused_via_keeps_its_actual_target_even_with_an_empty_proposal(existing_lead_in):
    from pcbir.joint_preparation import _alternatives
    from pcbir.joint_access import AccessRequirement, _materialize, _prove_requirement
    from types import SimpleNamespace
    source, original, _, settings = scene()
    via = replace(original.vias[0], position=Point.mm(3,'8.3'))
    original = replace(original, vias=(via,), tracks=(replace(original.tracks[0], end=via.position),))
    held = original if existing_lead_in else replace(original, tracks=())
    requirement = AccessRequirement('contact:GND:G.1', 'contact', 'GND', (PadReference('G','1'),),
                                   (CopperLayer.BACK,))
    frame = SimpleNamespace(held=held, ownership=begin_copper_ownership(held))
    remaining = dict(sites=128, candidates=8, bypass=8)
    choices, _ = _alternatives(frame, (requirement,), None, settings, JointPreparationOptions(), remaining)
    reused = tuple(c for c in choices if not c.vias and c.ports == ((PadReference('G','1'), original.vias[0].position),))
    assert reused
    assert not reused[0].tracks if existing_lead_in else reused[0].tracks
    for candidate in reused:
        physical = _materialize(held,(candidate,))
        assert _prove_requirement(physical, requirement, dict(candidate.ports), RoutingClearanceIndex(physical))


def test_bypass_alternatives_share_one_bounded_complete_proposal_search():
    from test_decoupling_crosslayer_paths import fixture
    from pcbir.decoupling_paths import propose_crosslayer_alternatives, validate_crosslayer_witness
    source, link = fixture()
    result = propose_crosslayer_alternatives(source, link, maximum_alternatives=4, maximum_proposals=8)
    assert 1 < len(result.choices) <= 4 and result.attempted <= 8
    assert all(p.witness is not None and not validate_crosslayer_witness(p.board,p.witness) for p in result.choices)
    assert source.tracks == source.vias == ()
    limited = propose_crosslayer_alternatives(source, link, maximum_alternatives=4, maximum_proposals=1)
    assert limited.attempted <= 1 and len(limited.choices) <= 1


def test_exact_replacement_rebases_indices_but_keeps_outside_source_ownership():
    source, original, ledger, _ = scene()
    frame = prepare_copper_trial(original, ledger, ('via:0',))
    via = replace(original.vias[0], position=Point.mm(3,8))
    trace = replace(original.tracks[0], end=via.position)
    candidate = replace(source, tracks=(trace,), vias=(via,))
    observed = rebuild_copper_ownership(frame, candidate, 'replacement',
        uses=prospective_contact_uses(candidate, (PadReference('G','1'),)))
    assert observed.matches(candidate) and observed.source_digest == ledger.source_digest
    assert all(o.producer == 'replacement' for o in observed.occurrences)
    with pytest.raises(ValueError, match='every released'):
        rebuild_copper_ownership(frame, candidate, 'replacement', uses=())


def test_native_recovered_contact_preserves_the_original_ground_connection(tmp_path, monkeypatch):
    import pcbir.fanout as fanout
    cli = shutil.which('kicad-cli')
    if not cli:
        pytest.skip('KiCad CLI unavailable')
    monkeypatch.setattr(fanout, '_candidates', lambda *a,**k: (Point.mm(3,7),))
    _, original, ledger, settings = scene()
    result = negotiate_preparation_contacts(original, ledger, settings, only_nets=frozenset({'A'}))
    assert result.status == 'geometry_proposal'
    before = verify_filled_planes(original, kicad_cli=Path(cli), artifact_directory=tmp_path/'before')
    after = verify_filled_planes(result.board, kicad_cli=Path(cli), artifact_directory=tmp_path/'after')
    from pcbir.power_extract import compare_power_fill_connectivity
    assert compare_power_fill_connectivity(original,before,result.board,after)['status']=='preserved'
    assert after.other_violation_count == 0


def test_planning_corridors_reserve_actual_tracks_and_complete_via_spans(monkeypatch):
    import pcbir.fanout as fanout
    from pcbir.power_geometry import PowerAreaOptions, _AreaClearance
    monkeypatch.setattr(fanout, '_candidates', lambda *a,**k: (Point.mm(3,7),))
    _, original, ledger, settings = scene()
    result = negotiate_preparation_contacts(original, ledger, settings, only_nets=frozenset({'A'}))
    corridors = joint_routing_corridors(result)
    assert corridors and any(set(c.layers)==set(original.stackup.copper_layers) for c in corridors)
    planning = _AreaClearance(result.board, PowerAreaOptions(protected_corridors=corridors))
    assert not planning.can_track('GND',Point.mm(3,9),Point.mm(3,7),250000,CopperLayer.FRONT)
    assert planning.can_track('GND',Point.mm(1,10),Point.mm(2,10),250000,CopperLayer.FRONT)
    assert result.board.copper_keepouts == original.copper_keepouts == ()
    with pytest.raises(ValueError,match='occurrence budget'):
        joint_routing_corridors(result,maximum_occurrences=1)


def test_known_ordinary_exits_remain_reserved_when_no_contact_move_is_needed():
    _, original, ledger, settings = scene()
    result = negotiate_preparation_contacts(original, ledger, settings, only_nets=frozenset({'B'}))
    assert result.status == 'not_needed'
    assert result.ports and joint_routing_corridors(result)
    assert joint_reservations_preserved(result,result.board)


def test_reserved_drill_and_annulus_enforce_the_real_gap_for_new_power_vias():
    from test_decoupling_crosslayer_paths import fixture
    from pcbir.power_geometry import PowerAreaOptions, _AreaClearance
    source, link = fixture()
    reserved = Via(link.return_net,Point.mm(20,15),450000,200000)
    candidate = Via(link.net,Point.mm('20.5',15),450000,200000)
    plain = _AreaClearance(source,PowerAreaOptions())
    exact = _AreaClearance(source,PowerAreaOptions(reserved_vias=(reserved,)))
    args=(candidate.net,candidate.position,candidate.size_nm,candidate.from_layer,candidate.to_layer,candidate.drill_nm)
    assert plain.can_via(*args)
    assert not exact.can_via(*args)  # 500 um centres need 550 um copper spacing.
    assert source.vias == ()


def test_seed_pool_reservation_refresh_is_source_bound_and_cumulative():
    from pcbir.power_geometry import PowerAreaOptions,plan_power_areas,refresh_power_area_reservations
    from test_power_preparation import fixture
    source, original_options = fixture()
    options=replace(original_options,maximum_conflict_trials=1)
    plan=plan_power_areas(source,options)
    refreshed=refresh_power_area_reservations(source,plan,options)
    assert refreshed.snapshot==plan.snapshot
    assert refreshed.allocation_states>=plan.allocation_states
    assert refreshed.allocation_states<=options.allocation_states
    assert len(refreshed.conflict_trials)<=options.maximum_conflict_trials
    with pytest.raises(ValueError,match='same-source'):
        refresh_power_area_reservations(replace(source,metadata={'changed':'true'}),plan,options)
    with pytest.raises(ValueError,match='unchanged work'):
        refresh_power_area_reservations(source,plan,replace(options,allocation_states=options.allocation_states+1))

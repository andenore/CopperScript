"""Native plane returns remain a separate, fresh, owned continuity ledger."""
from dataclasses import replace
from pathlib import Path
import shutil
import pytest

from pcbir.physical import (PhysicalBoard, BoardOutline, PhysicalFootprint, FootprintPad,
    PhysicalNet, PhysicalPowerDomain, Placement, Point, Size, PadReference, BoardSide,
    Stackup, CopperLayer, CopperZone, PolygonWithHoles, PolygonRing, DecouplingLink,
    ZoneConnection, TrackSegment)
from pcbir.fanout import FanoutOptions
from pcbir.power_geometry import PowerAreaOptions
from pcbir.power_preparation import (prepare_power_distribution, preparation_preserved,
                                      validate_power_preparation)
from pcbir.routing import route_global
from pcbir.plane_verify import verify_filled_planes


def fixture(*, data=False):
    footprint = PhysicalFootprint('two', (
        FootprintPad('1', Point.mm(-1, 0), Size.mm('.6', '.6')),
        FootprintPad('2', Point.mm(1, 0), Size.mm('.6', '.6')),
        *((FootprintPad('3', Point.mm(-1, '.8'), Size.mm('.4', '.4')), ) if data else ())),
        Size.mm(3, 3 if data else 1))
    rail = tuple(PadReference(r, '1') for r in ('J', 'U', 'C'))
    ground = tuple(PadReference(r, '2') for r in ('J', 'U', 'C'))
    link = DecouplingLink(net='RAIL', target=PadReference('U', '1'), capacitor=PadReference('C', '1'),
                          return_net='GND', return_pad=PadReference('C', '2'))
    board = PhysicalBoard('returns', BoardOutline.rectangle(30, 20), {'two': footprint},
        (Placement('J', 'two', Point.mm(4, 10)), Placement('U', 'two', Point.mm(15, 10)),
         Placement('C', 'two', Point.mm(15, 14), side=BoardSide.BACK)),
        (PhysicalNet('RAIL', rail), PhysicalNet('GND', ground),
         *((PhysicalNet('DATA', (PadReference('J', '3'), PadReference('U', '3'))), ) if data else ())),
        stackup=Stackup(copper_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                      CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        power_domains=(PhysicalPowerDomain('RAIL', rail, rail[:1]),),
        decoupling_links=(link,), zones=(CopperZone('ground', 'GND', (CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in ((1, 1), (29, 1), (29, 19), (1, 19))))),
            pad_connection=ZoneConnection.SOLID),))
    options = PowerAreaOptions(nets=('RAIL',), plane_return_nets=('RAIL',), candidates_per_domain=1,
                               maximum_layer_alternatives=1)
    return board, options


def verifier(tmp_path):
    executable = shutil.which('kicad-cli')
    if executable is None:
        pytest.skip('KiCad CLI unavailable')
    calls = []
    def verify(board, stage):
        calls.append(stage)
        return verify_filled_planes(board, kicad_cli=Path(executable),
            artifact_directory=tmp_path / f'{len(calls)}-{stage}')
    return verify, calls


def test_deferred_links_are_pending_without_explicit_or_native_false_pass():
    from pcbir.decoupling import route_decouplers
    board, options = fixture()
    routed = route_decouplers(board, fanout_options=FanoutOptions(plane_return_nets={'RAIL'}))
    assert routed.board is board and not routed.created_tracks and not routed.created_vias
    assert routed.pending == ((board.decoupling_links[0], 'native_plane_return_pending'),)
    preparation = prepare_power_distribution(board, route_global(board), options, FanoutOptions())
    assert not preparation.accepted and preparation.status == 'native_tool_required'
    assert preparation.board is board


def test_native_preparation_and_fresh_preservation_reject_changed_owners(tmp_path):
    board, options = fixture()
    verify, calls = verifier(tmp_path)
    prep = prepare_power_distribution(board, route_global(board), options, FanoutOptions(), native_verifier=verify)
    assert prep.accepted, prep.report()
    assert prep.ownership.matches(prep.board) and prep.ownership.frozen
    assert prep.ownership.uses
    assert all(not o['negotiable'] for o in prep.report()['copper_ownership']['occurrences'])
    from pcbir.copper_ownership import prepare_copper_trial
    with pytest.raises(ValueError, match='disposable'):
        prepare_copper_trial(prep.board, prep.ownership, ('via:0',))
    assert len(calls) == 2 and prep.evidence.matches(prep.board)
    assert preparation_preserved(prep, prep.board)
    assert not preparation_preserved(prep, replace(prep.board, tracks=prep.board.tracks[:-1]))
    assert not preparation_preserved(prep, replace(prep.board, placements=tuple(
        replace(p, position=Point.mm(16, 14)) if p.reference == 'C' else p for p in board.placements)))
    assert not preparation_preserved(prep, replace(prep.board, zones=()))
    # New physical copper invalidates the previous certificate. Refill also
    # catches a real dangling end instead of accepting its same-net label.
    added = replace(prep.board, tracks=(*prep.board.tracks,
        TrackSegment('GND', Point.mm(5, 10), Point.mm(6, 10), 250000, CopperLayer.FRONT)))
    assert validate_power_preparation(prep, added, prep.evidence, options)['status'] == 'fresh_native_evidence_required'
    assert validate_power_preparation(prep, added, verify(added, 'changed'), options)['status'] == 'power_preparation_regression'
    updated = replace(prep.board, metadata={**prep.board.metadata, 'detailed_routing': 'complete'})
    assert validate_power_preparation(prep, updated, verify(updated, 'refreshed'), options)['status'] == 'native_connected'


def test_opt_in_joint_stage_captures_a_private_snapshot_then_freezes_acceptance(tmp_path):
    board, options = fixture()
    options = replace(options, joint_contact_trials=1)
    verify, calls = verifier(tmp_path)
    snapshots = []
    prep = prepare_power_distribution(board, route_global(board), options, FanoutOptions(),
        native_verifier=verify, on_contact_snapshot=lambda b,o: snapshots.append((b,o)))
    assert prep.accepted and prep.joint_access.status == 'not_needed'
    assert len(snapshots) == 1 and snapshots[0][1].matches(snapshots[0][0])
    assert not snapshots[0][1].frozen
    assert prep.ownership.frozen and prep.joint_access.ownership.frozen
    assert calls == ['power-return-baseline', 'power-return-candidate']
    from pcbir.power_preparation import prepare_power_from_contacts
    resumed = prepare_power_from_contacts(prep.contact_snapshot, options, FanoutOptions(), native_verifier=verify)
    assert resumed.accepted and resumed.board == prep.board
    assert calls[-2:] == ['power-return-baseline', 'power-return-candidate']
    # A frozen accepted scene and changed source facts cannot be replayed as
    # fresh producer ownership. Rejection does not request a native trial.
    for snapshot in (
        replace(prep.contact_snapshot, ownership=prep.ownership, board=prep.board),
        replace(prep.contact_snapshot, source=replace(board, metadata={'changed':'source'})),
        replace(prep.contact_snapshot, source=replace(board, metadata={'changed':'source'}),
                ownership=replace(prep.contact_snapshot.ownership, source_digest='stale')),
        replace(prep.contact_snapshot, source=replace(board,
            placements=tuple(replace(p, position=Point.mm(5,10)) if p.reference=='J' else p for p in board.placements)))):
        count = len(calls)
        failed = prepare_power_from_contacts(snapshot, options, FanoutOptions(), native_verifier=verify)
        assert failed.status == 'stale_contact_snapshot' and len(calls) == count


@pytest.mark.parametrize('stale_joint_evidence', [False, True])
def test_joint_replacement_requires_extra_fresh_native_preservation(tmp_path, monkeypatch, stale_joint_evidence):
    from pcbir.joint_preparation import JointPreparationResult
    import pcbir.joint_preparation as joint
    board, options = fixture()
    options = replace(options, joint_contact_trials=1)
    verify, calls = verifier(tmp_path)
    # Isolate this transaction's native gate. Automatic producer movement has
    # its own real-geometry/native fixture in test_joint_preparation.py.
    def proposed(b, ownership, *_args, **_kwargs):
        return JointPreparationResult(b, ownership, 'geometry_proposal', reservations=b)
    monkeypatch.setattr(joint, 'negotiate_preparation_contacts', proposed)
    def native(b, stage):
        e = verify(b, stage)
        return replace(e, board_digest='stale') if stage == 'power-return-joint-contacts' and stale_joint_evidence else e
    prep = prepare_power_distribution(board, route_global(board), options, FanoutOptions(), native_verifier=native)
    assert calls[:2] == ['power-return-baseline','power-return-joint-contacts']
    if stale_joint_evidence:
        assert prep.status == 'joint_contact_native_rejected' and prep.board is board
        assert prep.joint_access_validation['status'] == 'fresh_baseline_required'
    else:
        assert prep.accepted and prep.joint_access_validation['status'] == 'preserved'
        assert calls[-1] == 'power-return-candidate'


@pytest.mark.parametrize('stale_feedback', [False, True])
def test_failed_area_feedback_is_native_gated_and_keeps_cumulative_work(tmp_path, monkeypatch, stale_feedback):
    import pcbir.power_geometry as geometry
    import pcbir.power_contact_feedback as feedback
    from pcbir.joint_preparation import JointPreparationResult
    from pcbir.power_geometry import PowerSearchFailure
    source, original_options = fixture()
    options = replace(original_options, area_contact_trials=1, maximum_conflict_trials=1)
    real_plan = geometry.plan_power_areas
    seen_options = []
    def planning(board, settings, **kwargs):
        seen_options.append(settings)
        result = real_plan(board,settings,**kwargs)
        if len(seen_options) == 1:
            return replace(result, selected_ids=(), allocation_states=2,
                conflict_trials=(('RAIL','held-domain','unresolved'),),
                search_failures=(PowerSearchFailure('RAIL',CopperLayer.FRONT,Point.mm(3,10),
                                                   Point.mm(14,10),250000,0,()),))
        return result
    monkeypatch.setattr(geometry,'plan_power_areas',planning)
    def proposed(board, ownership, *_args, **_kwargs):
        return JointPreparationResult(board,ownership,'geometry_proposal',reservations=board,dependency_trials=1)
    monkeypatch.setattr(feedback,'negotiate_power_area_contacts',proposed)
    verify, calls = verifier(tmp_path)
    def native(board, stage):
        result = verify(board,stage)
        return replace(result,board_digest='stale') if stale_feedback and 'area-contacts' in stage else result
    result = prepare_power_distribution(source,route_global(source),options,FanoutOptions(),native_verifier=native)
    assert calls[:2] == ['power-return-baseline','power-return-area-contacts-1']
    assert result.area_contact_trials[0]['allocation_states_spent'] == 2
    assert result.area_contact_trials[0]['regrowth_trials_spent'] == 1
    if stale_feedback:
        assert not result.accepted and result.board is source and len(seen_options) == 1
        assert result.area_contact_trials[0]['status'] == 'native_contact_preservation_rejected'
        assert result.area_contact_trials[0]['preservation']['status'] == 'fresh_baseline_required'
    else:
        assert result.accepted and len(seen_options) == 2
        assert seen_options[1].allocation_states == options.allocation_states-2
        assert seen_options[1].maximum_conflict_trials == 0
        assert result.area_contact_trials[0]['allocation_states_total'] <= options.allocation_states
        assert result.area_contact_trials[0]['regrowth_trials_total'] <= options.maximum_conflict_trials


def pipeline(board, options, verify):
    from pcbir.flow import run_routing_pipeline
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    from pcbir.package_access import PackageAccessOptions
    return run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(refinement_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        package_access_options=PackageAccessOptions(maximum_trials=0, maximum_pattern_trials=0),
        fanout_options=FanoutOptions(), power_area_options=options, power_native_verifier=verify)


def test_normal_pipeline_native_handoff_and_complete_boundary_rollback(tmp_path):
    board, options = fixture()
    verify, calls = verifier(tmp_path)
    result = pipeline(board, options, verify)
    assert result.power_preparation.accepted, result.power_preparation.report()
    assert result.package_access.ready
    assert result.power_preparation_validation['status'] == 'native_connected'
    assert calls == ['power-return-baseline', 'power-return-candidate',
                     'power-return-boundary', 'power-return-ordinary']
    # A fresh adapter returning mismatched source evidence cannot authorize
    # handoff. The complete appended ordinary/boundary proposal is rejected.
    def stale(candidate, stage):
        evidence = verify(candidate, stage)
        return replace(evidence, board_digest='changed') if stage == 'power-return-boundary' else evidence
    failed = pipeline(board, options, stale)
    assert failed.power_area_status == 'power_preparation_boundary_rejected'
    assert failed.board.tracks == failed.power_preparation.board.tracks
    assert failed.board.vias == failed.power_preparation.board.vias
    assert not failed.package_access.ready


@pytest.mark.parametrize('rejection_stage', ['power-return-boundary', 'power-return-ordinary'])
def test_boundary_rollback_restores_ordinary_owner_ledger(tmp_path, rejection_stage):
    board, options = fixture(data=True)
    verify, calls = verifier(tmp_path)
    seen = []
    def stale(candidate, stage):
        evidence = verify(candidate, stage)
        if stage == rejection_stage:
            seen.append(candidate)
            return replace(evidence, board_digest='changed')
        return evidence
    failed = pipeline(board, options, stale)
    assert failed.power_area_status == ('power_preparation_boundary_rejected'
        if rejection_stage == 'power-return-boundary' else 'power_preparation_ordinary_rejected')
    assert len(seen[0].tracks) > len(failed.power_preparation.board.tracks)
    if rejection_stage == 'power-return-boundary':
        assert not failed.fanout.created_tracks and not failed.fanout.created_vias
    assert not failed.package_access.ready
    assert failed.fanout.board.tracks == failed.board.tracks
    assert failed.fanout.board.vias == failed.board.vias


def test_route_board_native_return_policy_reports_final_certificate(tmp_path):
    import json
    from pcbir.cli import main
    cli = shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable')
    footprints = tmp_path / 'P.pretty'
    footprints.mkdir()
    for name, pads in (('two', [('1', -1, 0), ('2', 1, 0)]),
                       ('three', [('1', -1, 0), ('2', 1, 0), ('3', 0, -2)])):
        lines = [f'(footprint "{name}" (version 20240108) (generator pcbnew) (layer "F.Cu") (attr smd)']
        # Real fixture graphics prevent an inferred silk box along pad edges.
        lines += ['(fp_rect (start -1.5 -2.5) (end 1.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.Fab"))']
        lines += [f'(pad "{n}" smd rect (at {x} {y}) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))'
                  for n, x, y in pads]
        (footprints / f'{name}.kicad_mod').write_text('\n'.join((*lines, ')')), encoding='utf-8')
    source = tmp_path / 'board.copper'
    source.write_text('''board ReturnsCLI {
      silkscreen { branding { enabled = false; } }
      use library "tiny";
      component U: REGULATOR_3V3 { footprint = "P:three"; }
      component J: RESISTOR { footprint = "P:two"; }
      component C: CAPACITOR { footprint = "P:two"; role = decoupling; decouples = "U.IN"; }
      net RAIL { J.1; U.IN; C.1; }
      net GND { J.2; U.GND; C.2; }
      net OUT { U.OUT; }
      supply RAIL { voltage = 5 V; external = true; }
      supply GND { voltage = 0 V; external = true; }
      mechanical { outline rectangle { width = 30 mm; height = 20 mm; } }
      constraint fixed_placement(J) { x=4 mm; y=10 mm; rotation=0; side=front; }
      constraint fixed_placement(U) { x=15 mm; y=10 mm; rotation=0; side=front; }
      constraint fixed_placement(C) { x=15 mm; y=14 mm; rotation=0; side=back; }
      constraint copper_zone(GND) { id="ground"; layers="In1.Cu";
          x=1 mm; y=1 mm; width=28 mm; height=18 mm; pad_connection=solid; }
    }''', encoding='utf-8')
    config = tmp_path / 'power.json'
    config.write_text(json.dumps(dict(schema='copperscript-power-area/v0.1', nets=['RAIL'],
        plane_return_nets=['RAIL'], candidates_per_domain=1, maximum_layer_alternatives=1,
        external_boundaries=[dict(net='RAIL', terminal=dict(component='J', pad='1'), role='source',
                                  provenance='Synthetic external fixture source') ])), encoding='utf-8')
    report = tmp_path / 'route.json'
    code = main(['route-board', str(source), '--layers', '4', '--footprint-root', str(tmp_path),
        '--power-area-config', str(config), '--routing-profile', 'quick', '--package-access-trials', '0',
        '--zone-escape-trials', '0', '--zone-local-ripup-trials', '0', '--filled-copper-trials', '0',
        '--verify-plane-fill', cli, '--report', str(report), '-o', str(tmp_path/'draft.kicad_pcb')])
    assert code in (0, 1) and report.is_file()
    document = json.loads(report.read_text(encoding='utf-8'))
    assert document['power_area_planning']['return_preparation']['status'] == 'native_connected'
    assert document['power_area_planning']['return_preservation']['status'] == 'native_connected'

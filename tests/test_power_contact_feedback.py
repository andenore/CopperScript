"""Complete path witnesses drive bounded, atomic power/contact regeneration."""
from dataclasses import replace
import json

import pytest

from pcbir.drc import physical_board_digest
from pcbir.joint_preparation import JointPreparationOptions
from pcbir.power_contact_feedback import PowerContactFeedbackOptions, negotiate_power_area_contacts
from pcbir.power_geometry import PowerAreaOptions, PowerAreaPlan, PowerSearchFailure, PhysicalAreaCandidate
from pcbir.power_areas import PowerAreaCandidate
from pcbir.physical import CopperLayer, Point, TrackSegment
from pcbir.routing_clearance import RoutingClearanceIndex
from test_joint_preparation import scene


def fixture():
    _, board, ledger, settings = scene()
    failure = PowerSearchFailure('A', CopperLayer.FRONT, Point.mm(1,7), Point.mm(5,7),
        250000, 0, ('deliberately-untrusted-search-sample',))
    plan = PowerAreaPlan(physical_board_digest(board), '', 'unresolved', (), (), (), (),
                         source_board=board, search_failures=(failure,))
    return board, ledger, settings, plan


def run(board, ledger, settings, plan, **kwargs):
    return negotiate_power_area_contacts(board, ledger, plan, settings, PowerAreaOptions(), **kwargs)


def test_complete_path_replay_moves_only_exact_contact_owners_and_keeps_path_temporary():
    board, ledger, settings, plan = fixture()
    result = run(board, ledger, settings, plan)
    assert result.status == 'geometry_proposal', result.report()
    assert result.board.placements == board.placements and result.board.zones == board.zones
    assert result.ownership.matches(result.board) and not result.ownership.frozen
    assert result.board != board and board.vias[0] not in result.board.vias
    assert all(t.net != 'A' for t in result.board.tracks)
    proposal = next(t for t in result.trials if t['status'] == 'geometry_proposal')
    assert proposal['seeds'] and proposal['blockers'] and proposal['path']
    index = RoutingClearanceIndex(result.board)
    assert all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in proposal['path'])
    assert result.dependency_trials == 1
    assert board == plan.source_board and ledger.matches(board)
    assert result == run(board, ledger, settings, plan)
    assert json.loads(json.dumps(result.report()))['trials'][-1]['path']


@pytest.mark.parametrize('mutation', ['frozen', 'stale-ledger', 'stale-plan'])
def test_stale_or_frozen_ownership_cannot_authorize_replay(mutation):
    board, ledger, settings, plan = fixture()
    if mutation == 'frozen':
        ledger = replace(ledger, frozen=True)
    elif mutation == 'stale-ledger':
        ledger = replace(ledger, board_digest='old')
    else:
        plan = replace(plan, snapshot='old')
    with pytest.raises(ValueError):
        run(board, ledger, settings, plan)


@pytest.mark.parametrize('policy', ['fixed', 'unattributed', 'ambiguous'])
def test_ineligible_contacts_are_held_even_in_discovery(policy):
    board, ledger, settings, plan = fixture()
    values = dict(trial_mutable=False) if policy == 'fixed' else (
        dict(requirements=()) if policy == 'unattributed' else dict(ambiguous=True))
    held = replace(ledger, occurrences=tuple(replace(o, **values) for o in ledger.occurrences))
    result = run(board, held, settings, plan)
    assert result.board is board and result.ownership is held
    assert not result.dependency_trials


def test_failed_complete_regeneration_and_work_bounds_restore_exact_original():
    board, ledger, settings, plan = fixture()
    options = PowerContactFeedbackOptions(maximum_roots=1, maximum_path_states=1,
        joint=JointPreparationOptions(maximum_generated_candidates=1, maximum_occurrences=1))
    result = run(board, ledger, settings, plan, options=options)
    assert result.status != 'geometry_proposal' and result.board is board
    assert result.ownership is ledger and result.path_search_states <= 1
    assert result.dependency_trials <= options.joint.maximum_trials
    assert result.generated_candidates <= 1


def test_search_samples_do_not_release_an_unrelated_mutable_contact():
    board, ledger, settings, plan = fixture()
    clear = replace(plan.search_failures[0], start=Point.mm(8,3), end=Point.mm(12,3),
                    sampled_blockers=('via:0','track:0'))
    result = run(board, ledger, settings, replace(plan, search_failures=(clear,)))
    assert result.board is board and result.dependency_trials == 0
    assert result.trials[0]['status'] == 'no_contact_blockers'


def test_replayed_join_retains_its_exact_neighboring_area_prefix():
    board, ledger, settings, plan = fixture()
    wall = TrackSegment('B',Point.mm(4,3),Point.mm(4,11),250000,CopperLayer.FRONT)
    neighbor = PhysicalAreaCandidate(PowerAreaCandidate('neighbor','B',plan.snapshot,
        {'candidate:neighbor'},None,(),1),(wall,),(),CopperLayer.FRONT)
    failure = replace(plan.search_failures[0],held_candidate_ids=('neighbor',))
    held = replace(plan,candidates=(neighbor,),search_failures=(failure,))
    result = run(board,ledger,settings,held,options=PowerContactFeedbackOptions(maximum_path_states=8))
    assert result.board is board and not result.dependency_trials
    assert result.trials[0]['held_candidate_ids'] == ('neighbor',)
    assert result.trials[0]['status'] == 'no_complete_relaxed_path'
    for invalid in (replace(held, candidates=()), replace(held,candidates=(replace(neighbor,
            allocation=replace(neighbor.allocation,snapshot='old')),))):
        with pytest.raises(ValueError,match='same-source held'):
            run(board,ledger,settings,invalid)


def test_reservation_difference_preserves_multiplicity_after_contact_reordering():
    from collections import Counter
    from pcbir.copper_reservations import reserved_copper_additions
    board, _, _, _ = fixture()
    track = board.tracks[0]
    source = replace(board,tracks=(track,track))
    extra = replace(track,net='A',start=Point.mm(8,3),end=Point.mm(11,3))
    reordered = replace(source,tracks=(track,extra,track,track))
    tracks,vias = reserved_copper_additions(source,reordered)
    assert tracks == (track,extra) and not vias
    assert Counter((*source.tracks,*tracks)) == Counter(reordered.tracks)
    with pytest.raises(ValueError,match='lost a source copper occurrence'):
        reserved_copper_additions(source,replace(reordered,tracks=(track,extra)))


def test_discovery_holds_the_complete_oversized_shared_dependency_group():
    from pcbir.copper_ownership import CopperUse, record_copper_uses, prepare_copper_trial
    from pcbir.power_contact_feedback import _discovery_occurrences
    board, ledger, _, _ = fixture()
    old, = ledger.uses
    related = CopperUse('dependent-contact','contact',old.terminals,dependencies=(old.id,))
    shared = record_copper_uses(ledger,board,(related,))
    limits = JointPreparationOptions(maximum_obligations=1)
    assert not _discovery_occurrences(shared,limits)
    limits = replace(limits,maximum_obligations=2)
    eligible = _discovery_occurrences(shared,limits)
    assert eligible == {o.identity for o in shared.occurrences}
    frame = prepare_copper_trial(board,shared,('via:0',))
    assert set(frame.released) == eligible and len(frame.requirements) == 2


def test_native_contact_preservation_after_replay(tmp_path):
    from test_power_preparation import verifier
    from pcbir.power_extract import compare_power_fill_connectivity
    board, ledger, settings, plan = fixture()
    result = run(board, ledger, settings, plan)
    assert result.status == 'geometry_proposal'
    native, _ = verifier(tmp_path)
    before = native(board, 'before')
    after = native(result.board, 'after')
    assert compare_power_fill_connectivity(board,before,result.board,after)['status'] == 'preserved'
    assert after.other_violation_count == 0


def shared_branch_scene():
    from pcbir.copper_ownership import CopperUse, append_copper_ownership, begin_copper_ownership
    from pcbir.physical import CopperZone, PadReference, Placement, PolygonRing, PolygonWithHoles, Via
    from test_escape_assignment import board, opts
    source=board()
    source=replace(source,placements=tuple(replace(p,position=Point.mm(3,4)) if p.reference=='JA'
        else replace(p,position=Point.mm(10.5,10.5)) if p.reference=='U' else p for p in source.placements),
        zones=(CopperZone('reference','A',(CopperLayer.BACK,),PolygonWithHoles(
            PolygonRing(tuple(Point.mm(x,y) for x,y in ((1,1),(13,1),(13,13),(1,13)))))),))
    trunk=TrackSegment('A',Point.mm(3,4),Point.mm(10,4),250000,CopperLayer.FRONT)
    branch=TrackSegment('A',Point.mm(6,4),Point.mm(6,8),250000,CopperLayer.FRONT)
    via=Via('A',Point.mm(6,8),800000,400000)
    original=replace(source,tracks=(trunk,branch),vias=(via,Via('A',Point.mm(10,4),800000,400000)))
    uses=tuple(CopperUse(f'contact:shared:{i}','contact',(PadReference('JA','1'),),(trunk,branch),(via,)) for i in range(15))
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',uses=uses,trial_mutable=True)
    failure=PowerSearchFailure('B',CopperLayer.FRONT,Point.mm(4.5,6),Point.mm(7.5,6),250000,0,())
    plan=PowerAreaPlan(physical_board_digest(original),'','unresolved',(),(),(),(),
        source_board=original,search_failures=(failure,))
    return original,ledger,opts(refinement_step_nm=250000),plan


def test_shared_branch_repair_validates_more_obligations_than_it_regenerates():
    board,ledger,settings,plan=shared_branch_scene()
    config=PowerContactFeedbackOptions(maximum_roots=1,joint=JointPreparationOptions(
        maximum_trials=1,maximum_occurrences=1,maximum_obligations=1))
    result=run(board,ledger,settings,plan,options=config)
    assert result.status=='geometry_proposal',result.report()
    assert result.board.vias==board.vias and result.board.tracks[0]==board.tracks[0]
    assert result.ownership.matches(result.board) and len(result.requirements)==15
    assert result.trials[0]['repair']=='retained_track_junctions'
    assert result.dependency_trials==1 and result.path_search_states<=config.maximum_path_states
    assert ledger.matches(board) and all(t.net!='B' for t in result.board.tracks)
    assert result==run(board,ledger,settings,plan,options=config)


@pytest.mark.parametrize('bound',['states','obligations'])
def test_branch_work_or_validation_exhaustion_keeps_exact_original(bound):
    board,ledger,settings,plan=shared_branch_scene()
    options=(PowerContactFeedbackOptions(maximum_roots=1,maximum_path_states=1) if bound=='states' else
        PowerContactFeedbackOptions(maximum_roots=1,maximum_branch_obligations=1))
    result=run(board,ledger,settings,plan,options=options)
    assert result.status!='geometry_proposal' and result.board is board and result.ownership is ledger
    assert result.path_search_states<=options.maximum_path_states


def test_branch_failure_after_search_still_accounts_for_spent_work(monkeypatch):
    import pcbir.power_contact_feedback as feedback
    board,ledger,settings,plan=shared_branch_scene()
    def reject(*args,**kwargs):raise ValueError('injected_capture_rejection')
    monkeypatch.setattr(feedback,'rebuild_track_branch_ownership',reject)
    options=PowerContactFeedbackOptions(maximum_roots=1)
    result=run(board,ledger,settings,plan,options=options)
    assert result.board is board and result.ownership is ledger
    assert result.trials[0]['status']=='injected_capture_rejection'
    assert result.trials[0]['branch_path_search_states']>0
    assert result.path_search_states<=options.maximum_path_states


def test_shared_branch_native_refill_preserves_the_original_pad_groups(tmp_path):
    from test_power_preparation import verifier
    from pcbir.power_extract import compare_power_fill_connectivity
    board,ledger,settings,plan=shared_branch_scene()
    result=run(board,ledger,settings,plan,options=PowerContactFeedbackOptions(maximum_roots=1))
    assert result.status=='geometry_proposal',result.report()
    native,_=verifier(tmp_path)
    before=native(board,'shared-before')
    after=native(result.board,'shared-after')
    assert compare_power_fill_connectivity(board,before,result.board,after)['status']=='preserved'
    assert after.other_violation_count==0 and after.island_count==0

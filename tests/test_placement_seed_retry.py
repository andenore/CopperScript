"""Bounded attempts continue existing seeds; source errors remain fatal."""
from dataclasses import FrozenInstanceError, replace
import json
import pytest

import pcbir.placement as engine
from pcbir.layout import plan_placement
from pcbir.physical import (BoardOutline, ComponentPlacementRule, FootprintPad,
    PhysicalBoard, PhysicalFootprint, Placement, PlacementRegion, Point, Size)


def fixture():
    fp=PhysicalFootprint('unit',(FootprintPad('1',Point(0,0),Size.mm('.2','.2')),),Size.mm(1,1))
    poses=tuple(Placement(ref,fp.name,Point.mm(x,8)) for ref,x in (('ANCHOR',8),('FIRST',15),('SECOND',22)))
    return PhysicalBoard('SeedRetry',BoardOutline.rectangle(32,20),{fp.name:fp},poses,(),
        placement_rules=(ComponentPlacementRule('ANCHOR',fixed_position=poses[0].position,fixed_rotation_degrees=0),))


def options():
    return engine.PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,
        refinement_passes=0,escape_spacing_passes=0)


@pytest.mark.parametrize('budget', [0, -1, True, 1.5])
def test_invalid_explicit_seed_budget_is_rejected(budget):
    with pytest.raises(ValueError, match='seed count'):
        replace(options(), seed_count=budget)


def test_one_seed_budget_uses_real_legalization_and_keeps_fixed_pose():
    board = fixture(); opts = replace(options(), seed_count=1); events = []
    result = plan_placement(board, opts, progress=lambda *event: events.append(event))
    assert result.candidates[0].seed == 0
    assert [event for event in events if event[0] == 'analytical'] == [('analytical', 0, 1)]
    assert result.board.placements[0] == board.placements[0]
    assert engine.placement_solution_is_legal(board, {p.reference:p for p in result.board.placements}, opts)


def test_one_seed_failure_is_reported_without_promoting_partial_geometry(monkeypatch):
    calls = []
    def legalize(b, targets, source, o, seed, **kwargs):
        calls.append(seed)
        raise bounded('explicit budget exhausted', engine._fixed_placements(b, source, o))
    monkeypatch.setattr(engine, '_legalize', legalize)
    with pytest.raises(engine.PlacementAlgorithmError, match='explicit budget exhausted') as caught:
        engine.generate_placement_candidates(fixture(), replace(options(), seed_count=1))
    assert calls == [0]
    assert len(caught.value.failed_seed_attempts) == 1


def bounded(reason,placed):
    return engine._BoundedLegalizationFailure(reason,failed_reference='SECOND',accepted_placements=placed)


def test_first_bounded_failure_then_real_second_seed_and_report_evidence(monkeypatch):
    board=fixture();original=repr(board);opts=options();real=engine._legalize
    calls=[];events=[]
    def legalize(b,targets,source,o,seed,**kwargs):
        calls.append((seed,o.legalization_candidates,o.exact_repair_candidates,o.exact_repair_limit))
        if seed==0:raise bounded('synthetic exhausted mutable attempt',engine._fixed_placements(b,source,o))
        return real(b,targets,source,o,seed,**kwargs)
    monkeypatch.setattr(engine,'_legalize',legalize)
    plan=plan_placement(board,opts,progress=lambda *event:events.append(event))
    assert [call[0] for call in calls]==[0,1]
    assert all(call[1:]==(96,28,6) for call in calls)
    assert plan.candidates[0].seed==1
    assert engine.placement_solution_is_legal(board,{p.reference:p for p in plan.board.placements},opts)
    assert next(p for p in plan.board.placements if p.reference=='ANCHOR')==board.placements[0]
    failures=plan.candidates[0].statistics.failed_seed_attempts
    assert len(failures)==1 and failures[0].seed==0 and failures[0].accepted_count==1
    with pytest.raises(FrozenInstanceError):failures[0].seed=9
    assert ('legalization_failed',0,2) in events
    row=json.loads(plan.report.to_json())['candidates'][0]['statistics']['failed_seed_attempts'][0]
    assert row=={'seed':0,'failed_reference':'SECOND','accepted_count':1,'reason':'synthetic exhausted mutable attempt'}
    assert repr(board)==original


def test_later_failed_seed_is_visible_in_prior_successful_candidate(monkeypatch):
    board=fixture();opts=options();real=engine._legalize;calls=[]
    def legalize(b,targets,source,o,seed,**kwargs):
        calls.append(seed)
        if seed==1:raise bounded('later bounded failure',engine._fixed_placements(b,source,o))
        return real(b,targets,source,o,seed,**kwargs)
    monkeypatch.setattr(engine,'_legalize',legalize)
    candidates=engine.generate_placement_candidates(board,opts)
    assert calls==[0,1] and candidates[0].seed==0
    assert tuple(f.seed for f in candidates[0].statistics.failed_seed_attempts)==(1,)


def test_invalid_complete_seed_never_enters_copper_materialization(monkeypatch):
    board=fixture(); opts=options(); real=engine._legalize
    seen=[]
    def legalize(b,targets,source,o,seed,**kwargs):
        poses,repairs=real(b,targets,source,o,seed,**kwargs)
        if seed==0:
            poses["FIRST"]=replace(poses["FIRST"],position=poses["ANCHOR"].position)
        return poses,repairs
    import pcbir.decoupling as decoupling
    def bypass(b,poses,o):
        assert engine.placement_solution_is_legal(b,poses,o)
        seen.append(tuple(poses.values()))
        return poses,0
    monkeypatch.setattr(engine,"_legalize",legalize)
    monkeypatch.setattr(decoupling,"improve_decoupling_placement",bypass)
    result=engine.generate_placement_candidates(board,opts)
    assert len(seen)==1 and result[0].seed==1


def test_all_failed_attempts_keep_context_and_never_promote_partial_map(monkeypatch):
    board=fixture();original=repr(board);opts=options();calls=[];snapshots=[]
    def legalize(b,targets,source,o,seed,**kwargs):
        calls.append(seed)
        placed=engine._fixed_placements(b,source,o)
        if seed==1:placed['FIRST']=board.placements[1]
        error=bounded(f'reason for seed {seed}',placed)
        snapshots.append(error.accepted_placements)
        placed.clear()
        raise error
    monkeypatch.setattr(engine,'_legalize',legalize)
    with pytest.raises(engine.PlacementAlgorithmError,match='reason for seed 1') as caught:
        engine.generate_placement_candidates(board,opts)
    assert calls==[0,1]
    failures=caught.value.failed_seed_attempts
    assert [(f.seed,f.failed_reference,f.accepted_count) for f in failures]==[(0,'SECOND',1),(1,'SECOND',2)]
    assert snapshots[0]==(board.placements[0],)
    assert snapshots[1]==board.placements[:2]
    assert 'seed 0: SECOND after 1 accepted placements' in str(caught.value)
    assert 'seed 1: SECOND after 2 accepted placements' in str(caught.value)
    diagnostic=caught.value.diagnostic_board
    assert diagnostic is not None
    assert caught.value.diagnostic_components==('SECOND',)
    assert diagnostic.metadata['placement_diagnostic']=='rejected_geometry_not_a_checkpoint'
    assert diagnostic.placements[:2]==board.placements[:2]
    assert repr(board)==original


def test_actual_exhausted_region_raises_typed_failure_and_tries_both_existing_seeds():
    board=fixture();target=board.placements[1]
    impossible=replace(board,placements=(target,),regions=(PlacementRegion('tiny',BoardOutline.rectangle('.4','.4')),),
        placement_rules=(ComponentPlacementRule(target.reference,region='tiny'),))
    original=repr(impossible);events=[]
    with pytest.raises(engine.PlacementAlgorithmError,match='cannot legalize component') as caught:
        engine.generate_placement_candidates(impossible,options(),progress=lambda *event:events.append(event))
    assert [(f.seed,f.failed_reference,f.accepted_count) for f in caught.value.failed_seed_attempts]==[(0,'FIRST',0),(1,'FIRST',0)]
    assert [e[1] for e in events if e[0]=='legalization_failed']==[0,1]
    assert caught.value.diagnostic_components==('FIRST',)
    assert caught.value.diagnostic_board is not None
    assert repr(impossible)==original


def test_invalid_fixed_geometry_stays_fatal_on_first_attempt(monkeypatch):
    board=fixture();first=board.placements[0]
    collided=replace(board,placements=(first,replace(board.placements[1],position=first.position)),
        placement_rules=tuple(ComponentPlacementRule(ref,fixed_position=first.position,fixed_rotation_degrees=0)
                              for ref in ('ANCHOR','FIRST')))
    real=engine._legalize;calls=[]
    def observe(*args,**kwargs):calls.append(args[-1]);return real(*args,**kwargs)
    monkeypatch.setattr(engine,'_legalize',observe)
    with pytest.raises(engine.PlacementAlgorithmError,match='fixed component') as caught:
        engine.generate_placement_candidates(collided,options())
    assert calls==[] and caught.value.failed_seed_attempts==()


@pytest.mark.parametrize('error',(
    engine.PlacementAlgorithmError('cannot legalize component within local repair budget: unexpected source error'),
    ValueError('malformed source')))
def test_unexpected_error_is_not_retried_by_message_or_broad_exception_type(error,monkeypatch):
    calls=[]
    def unexpected(*args,**kwargs):calls.append(args[-1]);raise error
    monkeypatch.setattr(engine,'_legalize',unexpected)
    with pytest.raises(type(error),match=str(error)):
        engine.generate_placement_candidates(fixture(),options())
    assert calls==[0]


def test_empty_failure_evidence_keeps_legacy_report_shape():
    report=plan_placement(fixture(),options()).report
    assert all('failed_seed_attempts' not in row['statistics'] for row in json.loads(report.to_json())['candidates'])


def test_component_progress_does_not_change_placement_or_report():
    board=fixture();opts=replace(options(), refinement_passes=1);events=[]
    plain=plan_placement(board,opts)
    observed=plan_placement(board,opts,progress=lambda *event:events.append(event))
    assert observed.board == plain.board
    assert observed.report.to_json() == plain.report.to_json()
    assert any(stage.startswith('legalize:') for stage,_,_ in events)
    assert any(stage.startswith('refine:') for stage,_,_ in events)

"""Normal callback plumbing observes existing work without changing it."""
from dataclasses import replace
import json

import pytest

from pcbir import placement, routing, routeflow
from pcbir.progress import placement_progress
from test_progress import board


def test_adapter_preserves_raw_seed_indices_and_final_total_semantics():
    assert placement_progress(None) is None
    events=[];observe=placement_progress(lambda *event:events.append(event))
    for event in (('analytical',0,2),('legalization',0,2),('legalization_failed',0,2),
                  ('analytical',1,2),('refinement',1,2),('finished',2,2)):
        observe(*event)
    assert events[0]==('placement_seed','stage',{'stage':'analytical','seed':0,'seed_count':2})
    assert events[2]==('placement_seed','failed',{'stage':'legalization_failed','seed':0,'seed_count':2})
    assert events[-1]==('placement_candidates','finished',{'seed_count':2})
    assert not any(d.get('seed')==2 for _,_,d in events)


def test_global_real_result_and_work_order_are_identical_with_callback(monkeypatch):
    source=board();options=routing.GlobalRouterOptions(maximum_iterations=3)
    calls=[];events=[]
    for name in ('_build_graph','_pin_access_candidates','_route_iteration','_refine_overflowed_via_sites'):
        original=getattr(routing,name)
        def observe(*args,_name=name,_original=original,**kwargs):
            calls.append(_name);return _original(*args,**kwargs)
        monkeypatch.setattr(routing,name,observe)
    plain=routing.route_global(source,options);expected=calls[:];calls.clear()
    observed=routing.route_global(source,options,on_progress=lambda *event:events.append(event))
    assert observed==plain and observed.to_json()==plain.to_json() and calls==expected
    phases=[(p,e) for p,e,_ in events]
    assert phases[:5]==[('global_route','started'),('global_graph','started'),
        ('global_graph','finished'),('global_pin_access','started'),('global_pin_access','finished')]
    assert phases[-1]==('global_route','finished')
    starts=[d for p,e,d in events if (p,e)==('global_iteration','started')]
    finishes=[d for p,e,d in events if (p,e)==('global_iteration','finished')]
    assert len(starts)==len(finishes)==observed.metrics.iterations==1
    assert starts[0]=={'iteration':1,'maximum_iterations':3}
    assert events[-1][2]['total_overflow']==observed.metrics.total_overflow
    assert not any('fabrication_ready' in d for _,_,d in events)


def test_iteration_limit_improvement_and_layer_assignment_do_not_add_work(monkeypatch):
    source=board();original=routing._route_iteration;calls=[]
    def overflow(*args):
        result=original(*args);iteration=args[-1];calls.append(iteration)
        return replace(result,metrics=replace(result.metrics,total_overflow=(3 if iteration==1 else 2),
                       maximum_overflow=1,overfull_resource_count=1))
    monkeypatch.setattr(routing,'_route_iteration',overflow)
    monkeypatch.setattr(routing,'_refine_overflowed_via_sites',lambda b,g,*args:g)
    import pcbir.layer_assignment as assignment
    assigned=[]
    def preserve(b,g,r,*args):assigned.append(True);return r
    monkeypatch.setattr(assignment,'assign_ordinary_layers',preserve)
    options=routing.GlobalRouterOptions(maximum_iterations=3,layer_assignment_passes=1)
    plain=routing.route_global(source,options);expected=calls[:];calls.clear();assigned.clear()
    events=[]
    observed=routing.route_global(source,options,on_progress=lambda *event:events.append(event))
    assert observed==plain and calls==expected==[1,2,3] and assigned==[True]
    finishes=[d for p,e,d in events if (p,e)==('global_iteration','finished')]
    assert [d['improved'] for d in finishes]==[True,True,False]
    assert [d['total_overflow'] for d in finishes]==[3,2,2]
    assert events[-1]==('global_route','finished',{'status':'overflow','iterations':3,
                                               'unrouted_net_count':0,'total_overflow':2})


@pytest.mark.parametrize('function,unfinished',[('_build_graph','global_graph'),('_route_iteration','global_iteration')])
def test_interrupt_never_invents_a_finished_stage(monkeypatch,function,unfinished):
    def interrupt(*args,**kwargs):raise KeyboardInterrupt
    monkeypatch.setattr(routing,function,interrupt);events=[]
    with pytest.raises(KeyboardInterrupt):
        routing.route_global(board(),on_progress=lambda *event:events.append(event))
    phases=[(p,e) for p,e,_ in events]
    assert (unfinished,'started') in phases and (unfinished,'finished') not in phases
    assert ('global_route','finished') not in phases


def test_callback_exception_propagates_before_unstarted_work(monkeypatch):
    source=board();before=repr(source)
    def forbidden(*args):raise AssertionError('graph must not start after observer exception')
    monkeypatch.setattr(routing,'_build_graph',forbidden)
    def observe(phase,event,details):
        if (phase,event)==('global_graph','started'):raise RuntimeError('observer stopped')
    with pytest.raises(RuntimeError,match='observer stopped'):
        routing.route_global(source,on_progress=observe)
    assert repr(source)==before


def test_routeflow_reports_existing_seed_failure_without_extra_attempts(monkeypatch):
    from test_placement_seed_retry import fixture,options,bounded
    source=fixture();settings=options();original=placement._legalize;calls=[]
    def legalize(b,targets,poses,o,seed,**kwargs):
        calls.append((seed,o.legalization_candidates,o.exact_repair_candidates,o.exact_repair_limit))
        if seed==0:raise bounded('existing bounded mutable failure',placement._fixed_placements(b,poses,o))
        return original(b,targets,poses,o,seed,**kwargs)
    monkeypatch.setattr(placement,'_legalize',legalize)
    plain=routeflow.optimize_placement_for_routing(source,settings);expected=calls[:];calls.clear()
    events=[]
    observed=routeflow.optimize_placement_for_routing(source,settings,on_progress=lambda *e:events.append(e))
    assert observed==plain and calls==expected==[(0,96,28,6),(1,96,28,6)]
    failures=[d for p,e,d in events if (p,e)==('placement_seed','failed')]
    assert failures==[{'stage':'legalization_failed','seed':0,'seed_count':2}]
    assert ('placement_candidates','finished',{'seed_count':2}) in events
    assert len([1 for p,e,d in events if (p,e)==('global_route','started')])==2
    assert events[-1][0:2]==('placement_global_certification','finished')


def test_fatal_fixed_source_placement_keeps_original_error_without_retry_or_finish(monkeypatch):
    from test_placement_seed_retry import fixture,options
    from pcbir.physical import ComponentPlacementRule
    source=fixture();fixed=source.placements[0]
    source=replace(source,placements=(fixed,replace(source.placements[1],position=fixed.position)),
        placement_rules=tuple(ComponentPlacementRule(ref,fixed_position=fixed.position,fixed_rotation_degrees=0)
                              for ref in ('ANCHOR','FIRST')))
    before=repr(source);original=placement._legalize;calls=[]
    def legalize(*args,**kwargs):calls.append(args[-1]);return original(*args,**kwargs)
    def forbidden(*args,**kwargs):raise AssertionError('global routing must not follow fatal source error')
    monkeypatch.setattr(placement,'_legalize',legalize)
    monkeypatch.setattr(routeflow,'route_global',forbidden)
    with pytest.raises(placement.PlacementAlgorithmError,match='fixed component') as plain:
        routeflow.optimize_placement_for_routing(source,options())
    assert calls==[];events=[]  # Fully fixed faults fail before seed legalization.
    with pytest.raises(type(plain.value),match='fixed component') as observed:
        routeflow.optimize_placement_for_routing(source,options(),on_progress=lambda *e:events.append(e))
    assert str(observed.value)==str(plain.value) and observed.value.failed_seed_attempts==()
    assert calls==[] and repr(source)==before
    assert all(e!='finished' and p!='global_route' for p,e,_ in events)
    assert not any(e=='failed' for p,e,_ in events)


def test_routeflow_keeps_candidate_trials_and_final_certification_in_order(monkeypatch):
    source=board();original=routing.route_global;calls=[]
    def overflow(b,o,**kwargs):
        calls.append(b.name)
        result=original(b,o,**kwargs)
        return replace(result,status=routing.GlobalRoutingStatus.OVERFLOW,
                       metrics=replace(result.metrics,total_overflow=1))
    monkeypatch.setattr(routeflow,'route_global',overflow)
    monkeypatch.setattr(routeflow,'_movement_trials',lambda b,*args:(replace(b,name='trial-a'),replace(b,name='trial-b')))
    settings=placement.PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0)
    feedback=routeflow.PlacementRoutingFeedbackOptions(maximum_iterations=1,maximum_trials_per_iteration=2,
                                                       stagnation_limit=1)
    plain=routeflow.optimize_placement_for_routing(source,settings,None,feedback)
    expected=calls[:];calls.clear();events=[]
    observed=routeflow.optimize_placement_for_routing(source,settings,None,feedback,
                                                     on_progress=lambda *e:events.append(e))
    assert observed==plain and calls==expected==['progress','trial-a','trial-b','progress']
    assert observed.iterations[0].trials==2 and observed.accepted_moves==0
    assert [(d['iteration'],d['trial']) for p,e,d in events if (p,e)==('placement_global_trial','started')]==[(1,1),(1,2)]
    assert len([1 for p,e,d in events if (p,e)==('global_route','started')])==4
    assert events[-1][0:2]==('placement_global_certification','finished')


def test_zero_global_feedback_budget_skips_proposals_but_retains_failed_certification(monkeypatch):
    source = board(); original = routing.route_global; calls = []
    def overflow(b,o,**kwargs):
        calls.append(b.name)
        result = original(b,o,**kwargs)
        return replace(result, status=routing.GlobalRoutingStatus.OVERFLOW,
                       metrics=replace(result.metrics,total_overflow=1))
    def forbidden(*args,**kwargs):
        raise AssertionError('disabled feedback must not even generate disposable placements')
    monkeypatch.setattr(routeflow,'route_global',overflow)
    monkeypatch.setattr(routeflow,'_movement_trials',forbidden)
    options = placement.PlacementPlannerOptions(candidate_count=1, seed_count=1,
        analytical_iterations=0, refinement_passes=0, escape_spacing_passes=0)
    feedback = routeflow.PlacementRoutingFeedbackOptions(maximum_trials_per_iteration=0)
    result = routeflow.optimize_placement_for_routing(source,options,None,feedback)
    assert calls == ['progress','progress']  # Initial evaluation and final certification.
    assert result.accepted_moves == 0 and not result.iterations
    assert not result.full_route_certified


@pytest.mark.parametrize('value',(-1,True,1.5))
def test_invalid_global_feedback_budget(value):
    with pytest.raises(ValueError,match='trial budget'):
        routeflow.PlacementRoutingFeedbackOptions(maximum_trials_per_iteration=value)


def test_package_global_trial_forwards_same_callback_and_preserves_result(monkeypatch):
    import pcbir.fanout as fanout
    import pcbir.package_access as access
    from test_package_access import fixture
    from pcbir.physical import Point
    source,guides,settings,router=fixture()
    settings=replace(settings,surface_fallback=False);original=fanout._candidates
    def choices(position,center,options):
        return () if center==Point.mm(5,7) else original(position,center,options)
    monkeypatch.setattr(fanout,'_candidates',choices)
    baseline=access.preflight_package_access(source,guides,settings)
    options=access.PackageAccessOptions(maximum_trials=1,maximum_placement_probes=0)
    plain=access.improve_package_access(baseline,settings,options=options,global_options=router)
    calls=[];real=access.route_global;events=[]
    callback=lambda *e:events.append(e)
    def global_trial(*args,**kwargs):
        calls.append(kwargs.get('on_progress'));return real(*args,**kwargs)
    monkeypatch.setattr(access,'route_global',global_trial)
    observed=access.improve_package_access(baseline,settings,options=options,global_options=router,on_progress=callback)
    assert observed==plain and calls==[callback] and observed.accepted_moves==1
    phases=[(p,e) for p,e,_ in events]
    assert phases.index(('package_access_trial','started')) < phases.index(('global_iteration','started'))
    assert phases.index(('global_route','finished')) < phases.index(('package_access_trial','finished'))


def test_nested_progress_aggregation_keeps_one_pipeline_count_and_inclusive_spans(tmp_path):
    from pcbir import routing_benchmark as benchmark
    from pcbir.progress import emit
    from test_routing_benchmark import save_run
    target=tmp_path/'run';save_run(target);events=[]
    def observe(phase,event,details):
        events.append({'phase':phase,'event':event,'details':details,'elapsed_seconds':len(events)})
    emit(observe,'placement_global','started')
    routeflow.optimize_placement_for_routing(board(),placement.PlacementPlannerOptions(
        candidate_count=1,analytical_iterations=0,refinement_passes=0),on_progress=observe)
    emit(observe,'placement_global','finished')
    (target/'routing.log').write_text(''.join('PROGRESS '+json.dumps(e)+'\n' for e in events),encoding='utf-8')
    work=benchmark.summarize_run(target)['work']
    assert work['completed_pipeline_placement_stages']==1
    assert work['malformed_events']==0 and work['unfinished']==[]
    times=work['phase_inclusive_seconds']
    assert times['global_iteration']==2  # Two existing routes, one iteration each.
    assert times['global_route']>times['global_iteration']
    assert sum(times.values())>times['placement_global']  # Nested spans overlap.
    assert 'placement_seed' not in times and 'placement_candidates' not in times

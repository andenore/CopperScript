"""Measured contact ownership trials share the existing package-pattern bound."""
from dataclasses import replace
from types import SimpleNamespace
import pcbir.package_access as access
import pcbir.pair_terminals as terminals
import pcbir.contact_owner_regeneration as regeneration
from pcbir import route_global
from pcbir.critical import CriticalNetResult, CriticalRoutingResult, CriticalRoutingStatus
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.physical import TrackSegment, Point, CopperLayer
from pcbir.pair_terminals import PairTerminalPreparation
from test_local_pair_branches import fixture

def baseline():
    board = fixture()
    ordinary = route_fanout(board)
    guides = route_global(board)
    group = tuple(r.net for r in board.net_routing_rules)
    failed = CriticalNetResult(group,False,0,0,(),0,local_preparation=PairTerminalPreparation(
        status='blocked',paired_topology_supported=True))
    critical = CriticalRoutingResult(CriticalRoutingStatus.FAILED,board,(failed,),(),(),
        guides.routing_fingerprint,'fixture',0,0)
    return access.PackageAccessResult(board,guides,ordinary,critical,None,frozenset(group),0)

def test_contact_proposal_revalidates_all_owners_and_keeps_probe_disposable(monkeypatch):
    original = baseline()
    branch = tuple(TrackSegment(r.net,Point.mm(25,12),Point.mm(26,12),120000,CopperLayer.FRONT)
                   for r in original.source.net_routing_rules)
    def prepare(*args, report):
        report.searches=2;report.expanded_states=17
        return {},(branch[0],),(branch[1],),(),()
    monkeypatch.setattr(terminals,'prepare_pair_terminals',prepare)
    calls=[]
    def propose(board, fanout, group, tracks, vias, settings, **kwargs):
        assert board==original.source and fanout==original.fanout and tracks==branch
        assert kwargs['maximum_owner_nets']==8
        return original.fanout
    monkeypatch.setattr(regeneration,'regenerate_contact_owners',propose)
    def verify(board,guides,fanout,*args,**kwargs):
        assert not fanout.board.tracks
        assert kwargs['retained_critical']==original.critical
        assert kwargs['retained_accesses']==original.fanout
        calls.append('all_critical')
        return original.critical
    monkeypatch.setattr(access,'_critical_search',verify)
    improved=replace(original,failed_critical_nets=frozenset())
    def finish(*args):
        calls.append('planes_and_boundary')
        return improved
    monkeypatch.setattr(access,'_access_result',finish)
    work=access._SearchWork()
    result=access._contact_pattern_trial(original,original.source,FanoutOptions(),access.PackageAccessOptions(),
        None,None,6000,work,'screen')
    assert result.pattern_trials[0].outcome=='accepted'
    assert calls==['all_critical','planes_and_boundary']
    assert work.local_contact_searches==2 and work.local_contact_expanded_states==17
    assert result.board.tracks==original.board.tracks==()

def test_contact_failure_uses_one_slot_then_at_most_one_standard_pattern(monkeypatch):
    original=baseline()
    record=access.PackagePatternTrial(1,'contact_first',(),tuple(sorted(original.failed_critical_nets)),
                                    'contact_probe_blocked')
    calls=[]
    def first(*args):
        calls.append('contact')
        return replace(original,pattern_trials=(record,))
    monkeypatch.setattr(access,'_contact_pattern_trial',first)
    clean=replace(original.critical,nets=(),status=CriticalRoutingStatus.SUCCESS)
    monkeypatch.setattr(access,'_critical_search',lambda *args,**kwargs:clean)
    monkeypatch.setattr(access,'_failures',lambda *args:(frozenset(),0))
    def fanout(*args):
        calls.append('ordinary_proposal')
        return original.fanout
    monkeypatch.setattr(access,'route_fanout',fanout)
    options=access.PackageAccessOptions(maximum_pattern_trials=2)
    result=access._negotiate_patterns(original,original.source,FanoutOptions(),options,None,None)
    assert calls==['contact','ordinary_proposal']
    assert len(result.pattern_trials)==2 and [r.index for r in result.pattern_trials]==[1,2]
    assert result.fanout==original.fanout and result.critical==original.critical

def test_contact_failure_records_once_when_followup_is_ineligible(monkeypatch):
    original=baseline()
    record=access.PackagePatternTrial(1,'contact_first',(),tuple(sorted(original.failed_critical_nets)),
                                    'contact_probe_blocked')
    monkeypatch.setattr(access,'_contact_pattern_trial',lambda *args:replace(original,pattern_trials=(record,)))
    monkeypatch.setattr(access,'_critical_search',lambda *args,**kwargs:original.critical)
    monkeypatch.setattr(access,'_failures',lambda *args:(original.failed_critical_nets,0))
    result=access._negotiate_patterns(original,original.source,FanoutOptions(),access.PackageAccessOptions(),None,None)
    assert result.pattern_trials==(record,)

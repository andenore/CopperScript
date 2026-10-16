from dataclasses import replace
import pytest
from test_fanout_and_plane_verify import _dense_board
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.physical import PhysicalNet, PadReference, Placement, Point, TrackSegment, CopperLayer
from pcbir.released_exit_retry import complete_released_contact_exits
from pcbir.contact_owner_regeneration import _valid_exits, _hard_findings


def scene(name='SIGNAL', component='U1'):
    b=_dense_board()
    b=replace(b,placements=tuple(replace(p,reference=component) if p.reference=='U1' else p for p in b.placements)
        +(Placement('J2','terminal',Point.mm(16,15)),),
        nets=(PhysicalNet(name,(PadReference(component,'1'),PadReference('J1','1'))),
              PhysicalNet('HELD',(PadReference(component,'12'),PadReference('J2','1')))),
        tracks=(TrackSegment(name,Point.mm(17,17),Point.mm(18,17),150000,CopperLayer.FRONT),))
    held=route_fanout(b,only_nets=frozenset({'HELD'}))
    assert held.accesses and not held.pending_pads
    return b,replace(held,pending_pads=(PadReference(component,'1'),))


@pytest.mark.parametrize('name,component',[('SIGNAL','U1'),('RENAMED_BUS','EDGE_DEVICE')])
def test_real_retry_preserves_held_exit_and_source_occurrences(name,component):
    source,old=scene(name,component)
    result=complete_released_contact_exits(source,old,FanoutOptions())
    assert result.released_exit_retry.status=='completed'
    assert result.released_exit_retry.passes==result.released_exit_retry.maximum_passes==1
    assert not result.pending_pads
    assert _valid_exits(result.board,result)
    assert not _hard_findings(result.board)-_hard_findings(old.board)
    assert all(a is b for a,b in zip(old.board.tracks,result.board.tracks))
    assert all(a is b for a,b in zip(old.board.vias,result.board.vias))
    assert all(result.accesses[p]==v for p,v in old.accesses.items())
    assert old.pending_pads==(PadReference(component,'1'),)


@pytest.mark.parametrize('damage', ['equal_prefix_clone','source_change','missing_ledger',
    'unrelated_net','fake_anchor','duplicate_new_via','new_short'])
def test_rejected_retry_rolls_back_every_incumbent_owner(monkeypatch,damage):
    source,old=scene()
    good=route_fanout(old.board,only_nets=frozenset({'SIGNAL'}))
    assert good.created_tracks and good.created_vias
    if damage=='equal_prefix_clone':
        bad=replace(good,board=replace(good.board,tracks=(replace(good.board.tracks[0]),*good.board.tracks[1:])))
    elif damage=='source_change':
        bad=replace(good,board=replace(good.board,name='changed source'))
    elif damage=='missing_ledger':
        bad=replace(good,created_tracks=())
    elif damage=='unrelated_net':
        edge=TrackSegment('HELD',Point.mm(17,18),Point.mm(18,18),150000,CopperLayer.FRONT)
        bad=replace(good,board=replace(good.board,tracks=(*good.board.tracks,edge)),created_tracks=(*good.created_tracks,edge))
    elif damage=='fake_anchor':
        bad=replace(good,accesses={PadReference('U1','1'):Point.mm(1,1)})
    elif damage=='duplicate_new_via':
        via=good.created_vias[0]
        bad=replace(good,board=replace(good.board,vias=(*good.board.vias,via)),created_vias=(*good.created_vias,via))
    else:
        edge=TrackSegment('SIGNAL',Point.mm(10.75,9),Point.mm(10.75,11),150000,CopperLayer.FRONT)
        bad=replace(good,board=replace(good.board,tracks=(*good.board.tracks,edge)),created_tracks=(*good.created_tracks,edge))
    calls=[]
    def proposed(*args,**kwargs):calls.append(kwargs);return bad
    monkeypatch.setattr('pcbir.fanout.route_fanout',proposed)
    result=complete_released_contact_exits(source,old,FanoutOptions())
    assert len(calls)==1 and calls[0]['only_nets']==frozenset({'SIGNAL'})
    assert result.released_exit_retry.status=='rejected'
    assert replace(result,released_exit_retry=None)==old
    assert result.board is old.board
    assert result.created_tracks is old.created_tracks and result.created_vias is old.created_vias


def test_no_pending_does_not_call_router(monkeypatch):
    source,old=scene();old=replace(old,pending_pads=())
    monkeypatch.setattr('pcbir.fanout.route_fanout',lambda *a,**k:pytest.fail('unneeded retry'))
    result=complete_released_contact_exits(source,old,FanoutOptions())
    assert result.released_exit_retry.status=='not_needed'
    assert result.released_exit_retry.passes==0 and result.board is old.board


def test_invalid_incumbent_ledger_is_rejected_before_search(monkeypatch):
    source,old=scene();old=replace(old,added_track_count=999)
    monkeypatch.setattr('pcbir.fanout.route_fanout',lambda *a,**k:pytest.fail('stale owner'))
    result=complete_released_contact_exits(source,old,FanoutOptions())
    assert result.released_exit_retry.reason=='invalid_incumbent_ownership'
    assert result.board is old.board


def test_no_improvement_discards_generated_tail(monkeypatch):
    source,old=scene()
    good=route_fanout(old.board,only_nets=frozenset({'SIGNAL'}))
    monkeypatch.setattr('pcbir.fanout.route_fanout',lambda *a,**k:replace(good,accesses={}))
    result=complete_released_contact_exits(source,old,FanoutOptions())
    assert result.released_exit_retry.status=='unchanged'
    assert result.board is old.board


@pytest.mark.parametrize('released',(False,True))
def test_package_pattern_invokes_retry_only_after_actual_contact_release(monkeypatch,released):
    from pcbir.package_access import _full_ordinary_pattern
    from pcbir.plane import PlaneStitchResult
    from pcbir.plane_bypass_transfer import PlaneContactProposal
    source,old=scene()
    unused=TrackSegment('SIGNAL',Point.mm(18,18),Point.mm(19,18),150000,CopperLayer.BACK)
    frame=replace(source,tracks=(*source.tracks,unused) if released else source.tracks)
    plane=PlaneStitchResult(frame,(),(),int(released),0)
    def prepared(board,options):
        return replace(old,board=replace(board,tracks=(*board.tracks,*old.created_tracks),vias=(*board.vias,*old.created_vias)))
    monkeypatch.setattr('pcbir.package_access.route_fanout',prepared)
    result,reason=_full_ordinary_pattern(source,frame,FanoutOptions(),plane_contacts=PlaneContactProposal(source,plane))
    assert result is not None,reason
    assert unused not in result.board.tracks
    if released:
        assert result.released_exit_retry.status=='completed'
        assert not result.pending_pads
    else:
        assert result.released_exit_retry is None
        assert result.pending_pads==old.pending_pads

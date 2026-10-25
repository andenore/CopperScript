"""Small copper cuts keep junctions, all plated objects and broad obligations."""
from dataclasses import replace
import pytest

from pcbir.copper_branches import prepare_track_branch_trial, rebuild_track_branch_ownership
from pcbir.copper_ownership import (CopperUse, begin_copper_ownership, append_copper_ownership,
                                    prepare_copper_trial)
from pcbir.drc import explicit_copper_connectivity, physical_board_digest
from pcbir.physical import CopperLayer, PadReference, Point, TrackSegment, Via
from test_escape_assignment import board


def scene(count=15):
    source = board()
    trunk = TrackSegment('A',Point.mm(2,4),Point.mm(10,4),250000,CopperLayer.FRONT)
    branch = TrackSegment('A',Point.mm(6,4),Point.mm(6,8),250000,CopperLayer.FRONT)
    via = Via('A',Point.mm(6,8),800000,400000)
    after = replace(source,tracks=(trunk,branch),vias=(via,))
    uses = tuple(CopperUse(f'contact:{i}','contact',(PadReference('JA','1'),),
        (trunk,branch),(via,)) for i in range(count))
    ledger = append_copper_ownership(begin_copper_ownership(source),source,after,'contacts',uses=uses,trial_mutable=True)
    return after, ledger


def bent(track):
    points = (track.start,Point.mm(7,5),Point.mm(7,7),track.end)
    return tuple(replace(track,start=a,end=b) for a,b in zip(points,points[1:]))


def test_branch_cut_retains_shared_trunk_and_all_original_obligations():
    source,ledger = scene()
    with pytest.raises(ValueError,match='closure budget'):
        prepare_copper_trial(source,ledger,('track:1',),maximum_occurrences=1)
    frame = prepare_track_branch_trial(source,ledger,('track:1',),maximum_occurrences=1)
    assert frame.matches() and len(frame.requirements)==15
    assert frame.held.tracks==(source.tracks[0],) and frame.held.vias==source.vias
    assert ('track:1','track:0',Point.mm(6,4)) in frame.boundaries
    candidate,observed = rebuild_track_branch_ownership(frame,(bent(source.tracks[1]),))
    assert observed.matches(candidate) and not observed.frozen
    assert candidate.tracks[0]==source.tracks[0] and candidate.vias==source.vias
    assert {u.id for u in observed.uses}=={u.id for u in ledger.uses}
    assert all(source.tracks[1] not in u.tracks for u in observed.uses)
    assert all(set(candidate.tracks)<=set(u.tracks) for u in observed.uses)
    assert ledger.matches(source) and frame.original is source


def test_interior_t_is_an_explicit_retained_boundary_of_the_trunk():
    source,ledger=scene()
    frame=prepare_track_branch_trial(source,ledger,('track:0',))
    assert [(s.start,s.end) for s in frame.sections]==[
        (Point.mm(2,4),Point.mm(6,4)),(Point.mm(6,4),Point.mm(10,4))]
    paths=tuple((replace(source.tracks[0],start=s.start,end=s.end),) for s in frame.sections)
    after,owned=rebuild_track_branch_ownership(frame,paths)
    assert owned.matches(after) and source.tracks[1] in after.tracks


def test_crossing_on_another_layer_is_not_a_junction():
    source,ledger=scene()
    trunk=replace(source.tracks[0],layer=CopperLayer.BACK)
    changed=replace(source,tracks=(trunk,source.tracks[1]))
    uses=tuple(replace(u,tracks=changed.tracks) for u in ledger.uses)
    plain=replace(source,tracks=(),vias=())
    ledger=append_copper_ownership(begin_copper_ownership(plain),plain,changed,'contacts',uses=uses,trial_mutable=True)
    frame=prepare_track_branch_trial(changed,ledger,('track:1',))
    assert not any(b=='track:0' for a,b,p in frame.boundaries)


@pytest.mark.parametrize('fault',['stale','frozen','fixed','ambiguous','unknown','via','budget'])
def test_no_partial_authority_or_mutation_on_rejected_capture(fault):
    source,ledger=scene()
    before=physical_board_digest(source)
    seeds=('track:1',)
    kwargs={}
    if fault=='stale': ledger=replace(ledger,board_digest='stale')
    if fault=='frozen': ledger=replace(ledger,frozen=True)
    if fault in {'fixed','ambiguous','unknown'}:
        changes={'trial_mutable':False} if fault=='fixed' else ({'ambiguous':True} if fault=='ambiguous' else {'requirements':()})
        ledger=replace(ledger,occurrences=tuple(replace(o,**changes) if o.identity=='track:1' else o for o in ledger.occurrences))
    if fault=='via': seeds=('via:0',)
    if fault=='budget': seeds=('track:0','track:1');kwargs={'maximum_occurrences':1}
    with pytest.raises(ValueError): prepare_track_branch_trial(source,ledger,seeds,**kwargs)
    assert physical_board_digest(source)==before


@pytest.mark.parametrize('fault',['endpoint','width','layer','incomplete','forged','shortcut'])
def test_replacement_cannot_drop_boundaries_or_change_physical_intent(fault):
    source,ledger=scene()
    frame=prepare_track_branch_trial(source,ledger,('track:1',))
    paths=(bent(source.tracks[1]),)
    if fault=='endpoint': paths=((replace(source.tracks[1],start=Point.mm(7,4)),),)
    if fault=='width': paths=(tuple(replace(t,width_nm=150000) for t in paths[0]),)
    if fault=='layer': paths=(tuple(replace(t,layer=CopperLayer.BACK) for t in paths[0]),)
    if fault=='incomplete': paths=()
    if fault=='forged': frame=replace(frame,boundaries=())
    if fault=='shortcut':
        points=(Point.mm(6,4),Point.mm(8,4),Point.mm(8,6),Point.mm(6,8))
        paths=(tuple(replace(source.tracks[1],start=a,end=b) for a,b in zip(points,points[1:])),)
    with pytest.raises(ValueError): rebuild_track_branch_ownership(frame,paths)
    assert ledger.matches(source)


def test_required_array_vias_are_never_a_track_branch_cut():
    from test_pad_via_arrays import _array_board
    from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
    source=materialize_via_in_pad_arrays(_array_board(rows=2,columns=2,pitch_nm=500000))
    ledger=begin_copper_ownership(source)
    for i in range(4):
        with pytest.raises(ValueError): prepare_track_branch_trial(source,ledger,(f'via:{i}',))
    assert len(source.vias)==4


def test_off_center_pad_and_annulus_contacts_retain_the_actual_end_cap():
    original,old=scene(1)
    original=replace(original,placements=tuple(replace(p,position=Point.mm(6.1,8)) if p.reference=='JA'
        else p for p in original.placements),vias=(replace(original.vias[0],position=Point.mm(5.77,8)),))
    source=replace(original,tracks=(),vias=())
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',trial_mutable=True,
        uses=tuple(replace(u,vias=original.vias) for u in old.uses))
    frame=prepare_track_branch_trial(original,ledger,('track:1',))
    assert ('track:1','via:0',Point.mm(6,8)) in frame.boundaries
    assert any(b.startswith('pad:JA.') and p==Point.mm(6,8) for a,b,p in frame.boundaries)
    after,owned=rebuild_track_branch_ownership(frame,(bent(original.tracks[1]),))
    assert owned.matches(after) and after.vias==original.vias


def test_a_moved_path_cannot_lose_an_interior_plated_contact():
    original,old=scene(1)
    original=replace(original,vias=(*original.vias,Via('A',Point.mm(6,6),800000,400000)))
    source=replace(original,tracks=(),vias=())
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',trial_mutable=True,
        uses=tuple(replace(u,vias=original.vias) for u in old.uses))
    frame=prepare_track_branch_trial(original,ledger,('track:1',))
    assert len(frame.sections)==2 and frame.sections[0].end==Point.mm(6,6)
    with pytest.raises(ValueError,match='complete branch'):
        rebuild_track_branch_ownership(frame,(bent(original.tracks[1]),))


@pytest.mark.parametrize('field',['metadata','zone','placements'])
def test_changed_context_does_not_reuse_a_branch_snapshot(field):
    from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles
    original,ledger=scene(1)
    changed=(replace(original,metadata={'changed':True}) if field=='metadata' else
        replace(original,zones=(CopperZone('new','A',(CopperLayer.BACK,),
            PolygonWithHoles(PolygonRing(original.outline.vertices))),)) if field=='zone' else
        replace(original,placements=tuple(reversed(original.placements))))
    # Pose ordering alone is intentionally digest-neutral, but the exact frame
    # still binds its held board and source fields.
    frame=prepare_track_branch_trial(original,ledger,('track:1',))
    assert not replace(frame,original=changed).matches()


def test_degree_two_corners_move_but_physical_branch_anchors_stay():
    original,old=scene(1)
    old_branch=original.tracks[1]
    pieces=bent(old_branch)
    source=replace(original,tracks=(),vias=())
    original=replace(original,tracks=(original.tracks[0],*pieces))
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',trial_mutable=True,
        uses=tuple(replace(u,tracks=original.tracks) for u in old.uses))
    frame=prepare_track_branch_trial(original,ledger,('track:1','track:2','track:3'))
    assert len(frame.sections)==1 and len(frame.sections[0].occurrences)==3
    assert {frame.sections[0].start,frame.sections[0].end}=={old_branch.start,old_branch.end}
    straight=(replace(old_branch,start=frame.sections[0].start,end=frame.sections[0].end),)
    after,owned=rebuild_track_branch_ownership(frame,(straight,))
    assert owned.matches(after) and after.tracks==(original.tracks[0],*straight)
    assert all(t not in after.tracks for t in pieces)


def test_a_closed_selected_loop_is_not_silently_reduced_to_a_tree():
    original,old=scene(1)
    points=tuple(Point.mm(x,y) for x,y in ((4,10),(8,10),(8,12),(4,12),(4,10)))
    tracks=tuple(replace(original.tracks[1],start=a,end=b) for a,b in zip(points,points[1:]))
    source=replace(original,tracks=(),vias=())
    original=replace(source,tracks=tracks)
    ledger=append_copper_ownership(begin_copper_ownership(source),source,original,'contacts',trial_mutable=True,
        uses=tuple(replace(u,tracks=tracks,vias=()) for u in old.uses))
    with pytest.raises(ValueError,match='closed track branch'):
        prepare_track_branch_trial(original,ledger,tuple(f'track:{i}' for i in range(4)))
    assert ledger.matches(original)


def test_discovery_does_not_freeze_a_replaceable_corner():
    from pcbir.copper_branches import track_endpoint_guards
    original,_=scene(1)
    pieces=bent(original.tracks[1])
    original=replace(original,tracks=pieces)
    guards=track_endpoint_guards(original,{'track:0','track:1','track:2'})
    assert {t.start for t in guards}=={pieces[0].start,pieces[-1].end}


def test_fine_grid_can_enter_a_retained_junction_but_cannot_follow_the_trunk():
    from pcbir.copper_branches import RetainedJunctionClearance
    from pcbir.routing_clearance import RoutingClearanceIndex
    original,ledger=scene(1)
    frame=prepare_track_branch_trial(original,ledger,('track:1',))
    clearance=RetainedJunctionClearance(RoutingClearanceIndex(frame.held),'A',
        (Point.mm(6,4),Point.mm(6,8)))
    assert clearance.can_track('A',Point.mm(6.05,4.05),Point.mm(6.1,4.1),250000,CopperLayer.FRONT)
    assert not clearance.can_track('A',Point.mm(6,4),Point.mm(8,4),250000,CopperLayer.FRONT)
    assert not clearance.can_track('A',Point.mm(6.1,4.1),Point.mm(7,4.1),250000,CopperLayer.FRONT)


def test_unknown_multi_terminal_contact_contract_is_not_release_authority():
    original,ledger=scene(1)
    ledger=replace(ledger,uses=tuple(replace(u,terminals=(*u.terminals,PadReference('U','1'))) for u in ledger.uses))
    with pytest.raises(ValueError,match='unsupported'):
        prepare_track_branch_trial(original,ledger,('track:1',))

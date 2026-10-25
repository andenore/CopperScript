"""Original complete paths remain ownership evidence after canonical reselection."""
from dataclasses import fields, replace

import pytest

from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling import route_decouplers
from pcbir.decoupling_paths import refresh_crosslayer_witnesses, validate_crosslayer_witness
from pcbir.drc import physical_board_digest, run_physical_drc
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import _partial_ordinary_pattern, _verified_bypass_links
from pcbir.physical import (CopperLayer, FootprintPad, NetRoutingRule, PadReference,
                            PhysicalNet, Point, RouteKind, Size, TrackSegment)
from test_decoupling_crosslayer_paths import fixture


def shared_board(*, renamed=False, dual=False, reverse=False):
    board, link = fixture(renamed=renamed)
    chip = board.footprints['chip']
    chip = replace(chip, pads=(*chip.pads, FootprintPad('H', Point.mm(-1,2), Size.mm(.4,.4))))
    first = replace(board.placements[1], position=Point.mm(11,9))
    second = replace(first, reference='OTHER_CAP', position=Point.mm(9,12))
    other = replace(link, capacitor=PadReference(second.reference,'1'), return_pad=PadReference(second.reference,'2'))
    board = replace(board, footprints={**board.footprints,'chip':chip},
        placements=(board.placements[0],first,second), decoupling_links=(link,other),
        nets=(replace(board.nets[0],pads=(*board.nets[0].pads,other.capacitor)),
              replace(board.nets[1],pads=(*board.nets[1].pads,PadReference(link.target.component,'H'),other.return_pad))))
    if dual:
        # Same physical scene with the shared rail serving the return role.
        links=tuple(replace(l,capacitor=l.return_pad,return_pad=l.capacitor,
                            target=PadReference(l.target.component,'G'),net=l.return_net,return_net=l.net)
                    for l in board.decoupling_links)
        board=replace(board,decoupling_links=links)
    if reverse:
        board=replace(board,decoupling_links=tuple(reversed(board.decoupling_links)))
    return board


def route_outward_fixture(board):
    """Build the ownership fixture with explicit outward, 0.5 mm ray exits.

    These tests need an original path that a later canonical path supersedes.
    Keep that geometry independent of the production candidate ranking; real
    clearance, path, budget, admission and DRC checks still construct the result.
    """
    import pcbir.decoupling_paths as paths
    from pcbir.placement import transformed_local_point

    def outward(current, pose, pad, options):
        start = transformed_local_point(pose, pad.position)
        dx, dy = start.x_nm-pose.position.x_nm, start.y_nm-pose.position.y_nm
        if abs(dx) >= abs(dy):
            dx, dy = (1 if dx >= 0 else -1), 0
        else:
            dx, dy = 0, (1 if dy >= 0 else -1)
        for distance in range(500_000, options.maximum_radius_nm+1, 500_000):
            yield Point(start.x_nm+dx*distance, start.y_nm+dy*distance)

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(paths, '_package_candidates', outward)
        patch.setattr(paths, '_local_launch_sites', lambda *args: ())
        patch.setattr(paths, '_two_leg_candidates', lambda *args: ())
        return route_fanout(board)


@pytest.mark.parametrize('renamed', (False, True))
def test_compact_launch_search_keeps_routable_shared_bypass_alternatives(renamed):
    source = shared_board(renamed=renamed)
    result = route_fanout(source)
    assert not result.decoupling_pending
    assert len(result.decoupling_witnesses) == 2
    assert verified_bypass_reservations(source, result)[2] is None
    assert all(validate_crosslayer_witness(result.board, w) == ''
               for w in result.decoupling_witnesses)
    assert result.board.net_routing_rules is source.net_routing_rules
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value == 'error'
                and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'}]


def edges(witnesses, kind):
    return {id(s.edge) for w in witnesses for p in (w.supply_path,w.return_path)
            for s in p.steps if isinstance(s.edge,kind)}


@pytest.mark.parametrize('renamed',(False,True))
def test_original_adopted_path_covers_canonical_reselection_without_rerouting(renamed,monkeypatch):
    source=shared_board(renamed=renamed)
    result=route_outward_fixture(source)
    assert not result.decoupling_pending and len(result.decoupling_witnesses)==2
    paths,vias={},{}
    assert len(_verified_bypass_links(result.board,{l.net for l in source.decoupling_links},
               witness_paths=paths,witness_vias=vias))==2
    canonical_vias={id(v) for values in vias.values() for v in values}
    assert any(id(v) not in canonical_vias for v in result.decoupling_vias)
    assert all(id(t) in edges(result.decoupling_witnesses,TrackSegment) for t in result.decoupling_tracks)
    assert all(validate_crosslayer_witness(result.board,w)=='' for w in result.decoupling_witnesses)
    assert all(w.board_digest==physical_board_digest(result.board) for w in result.decoupling_witnesses)
    def forbidden(*args,**kwargs):raise AssertionError('admission must not route')
    monkeypatch.setattr('pcbir.decoupling.route_decouplers',forbidden)
    assert verified_bypass_reservations(source,result)==(result.decoupling_tracks,result.decoupling_vias,None)
    # The legacy no-record contract still rejects unsupported canonical coverage.
    assert verified_bypass_reservations(source,replace(result,decoupling_witnesses=()))[2]
    assert all(getattr(source,f.name)==getattr(result.board,f.name) for f in fields(source)
               if f.name not in {'tracks','vias'})
    assert not source.tracks and not source.vias


@pytest.mark.parametrize('dual,reverse',((False,False),(True,False),(True,True)))
def test_every_adopted_new_edge_is_covered_or_contribution_remains_pending(dual,reverse):
    source=shared_board(dual=dual,reverse=reverse)
    result=route_fanout(source)
    assert verified_bypass_reservations(source,result)[2] is None
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value=='error' and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]
    tracks,vias={},{}
    _verified_bypass_links(result.board,{l.net for l in source.decoupling_links},witness_paths=tracks,
                           witness_vias=vias,admitted_witnesses=result.decoupling_witnesses)
    assert {id(t) for t in result.decoupling_tracks}<={id(t) for ps in tracks.values() for t in ps}
    assert {id(v) for v in result.decoupling_vias}<={id(v) for ps in vias.values() for v in ps}


def test_first_complete_proposal_prunes_only_its_unwitnessed_suffix():
    from pcbir.decoupling_paths import propose_crosslayer_decoupling
    from pcbir.physical import Via
    source=shared_board(dual=True)
    first,second=source.decoupling_links
    initial=propose_crosslayer_decoupling(source,first)
    assert initial.witness is not None
    accepted=propose_crosslayer_decoupling(initial.board,second,maximum_proposals=1)
    assert accepted.witness is not None
    assert all(a is b for a,b in zip(initial.board.tracks,accepted.board.tracks))
    assert all(a is b for a,b in zip(initial.board.vias,accepted.board.vias))
    assert len(accepted.board.tracks)>=len(initial.board.tracks)
    assert len(accepted.board.vias)>=len(initial.board.vias)
    assert validate_crosslayer_witness(accepted.board,initial.witness)==''
    assert validate_crosslayer_witness(accepted.board,accepted.witness)==''
    assert {id(t) for t in accepted.created_tracks}<=edges((accepted.witness,),TrackSegment)
    assert {id(v) for v in accepted.created_vias}<=edges((accepted.witness,),Via)


@pytest.mark.parametrize('fault',('pose','edge_copy','missing_via','budget','floating','half_path','untyped',
                                 'missing_path','mutable_steps','untyped_node'))
def test_records_never_waive_current_source_original_edges_or_complete_return(fault):
    source=shared_board(); result=route_fanout(source); witness=result.decoupling_witnesses[0]
    if fault=='pose':
        source=replace(source,placements=tuple(replace(p,position=Point(p.position.x_nm+1,p.position.y_nm))
                                               if p==source.placements[-1] else p for p in source.placements))
        result=replace(result,board=replace(result.board,placements=source.placements))
    elif fault=='edge_copy':
        step=witness.supply_path.steps[0]
        forged=replace(witness,supply_path=replace(witness.supply_path,steps=(replace(step,edge=replace(step.edge)),*witness.supply_path.steps[1:])))
        result=replace(result,decoupling_witnesses=(forged,*result.decoupling_witnesses[1:]))
    elif fault=='missing_via':
        removed=result.created_vias[0]
        result=replace(result,board=replace(result.board,vias=tuple(v for v in result.board.vias if v is not removed)))
    elif fault=='budget':
        rules=tuple(replace(r,max_vias=0) for r in source.net_routing_rules)
        source=replace(source,net_routing_rules=rules);result=replace(result,board=replace(result.board,net_routing_rules=rules))
    elif fault=='floating':
        t=TrackSegment(source.decoupling_links[0].net,Point.mm(21,15),Point.mm(22,15),150000,result.created_tracks[0].layer)
        result=replace(result,board=replace(result.board,tracks=(*result.board.tracks,t)),
                       created_tracks=(*result.created_tracks,t),decoupling_tracks=(*result.decoupling_tracks,t),
                       added_track_count=result.added_track_count+1)
    elif fault=='half_path':
        result=replace(result,decoupling_witnesses=(replace(witness,return_path=replace(witness.return_path,steps=())),))
    elif fault=='missing_path':
        result=replace(result,decoupling_witnesses=(replace(witness,return_path=None),))
    elif fault=='mutable_steps':
        result=replace(result,decoupling_witnesses=(replace(witness,return_path=replace(witness.return_path,
                                                                                   steps=list(witness.return_path.steps))),))
    elif fault=='untyped_node':
        path=witness.return_path
        result=replace(result,decoupling_witnesses=(replace(witness,return_path=replace(path,
            steps=(replace(path.steps[0],end=object()),*path.steps[1:]))),))
    else:result=replace(result,decoupling_witnesses=(object(),))
    before=result.board
    assert verified_bypass_reservations(source,result)[2]
    assert result.board is before


def test_final_canonical_and_original_ledgers_are_distinct_fresh_snapshots():
    result=route_decouplers(shared_board())
    assert len(result.admitted_path_witnesses)==len(result.supply_return_witnesses)==2
    assert all(w.board_digest==physical_board_digest(result.board)
               for w in (*result.admitted_path_witnesses,*result.supply_return_witnesses))
    refreshed,reason=refresh_crosslayer_witnesses(result.board,result.admitted_path_witnesses)
    assert reason is None and refreshed==result.admitted_path_witnesses
    assert all(a.edge is b.edge for old,new in zip(result.admitted_path_witnesses,refreshed)
               for a,b in zip(old.supply_path.steps,new.supply_path.steps))


def test_regeneration_ownership_map_includes_exact_original_alternative_via():
    result=route_fanout(shared_board());paths,vias={},{}
    verified=_verified_bypass_links(result.board,{l.net for l in result.board.decoupling_links},
        witness_paths=paths,witness_vias=vias,admitted_witnesses=result.decoupling_witnesses)
    assert len(verified)==2
    assert {id(v) for v in result.decoupling_vias}<={id(v) for vs in vias.values() for v in vs}
    assert {id(t) for t in result.decoupling_tracks}<={id(t) for ts in paths.values() for t in ts}


def contact_scene(*, shared_return=False):
    board=shared_board()
    if shared_return:
        # An independent supply cohort borrows P as its declared return. Its
        # immutable source branch reaches the first cap's P land. The generous
        # explicit fixture budgets accommodate that deliberately long branch.
        a,c=board.decoupling_links
        pose=replace(board.placements[1],reference='INVERSE',position=Point.mm(6,5))
        other=replace(a,capacitor=PadReference('INVERSE','1'),return_pad=PadReference('INVERSE','2'),
                      target=PadReference(a.target.component,'G'),net=a.return_net,return_net=a.net)
        points=((11.5,9),(15,9),(15,3),(5.5,3),(5.5,5))
        branch=tuple(TrackSegment(a.net,Point.mm(*x),Point.mm(*y),150000,CopperLayer.BACK)
                     for x,y in zip(points,points[1:]))
        board=replace(board,tracks=branch,placements=(*board.placements,pose),
            decoupling_links=(a,other,c),
            net_routing_rules=tuple(replace(r,max_vias=4,max_length_nm=100_000_000)
                                    for r in board.net_routing_rules),
            nets=tuple(replace(n,pads=(*n.pads,other.capacitor if n.name==other.net else other.return_pad))
                       for n in board.nets))
    board=replace(board,nets=(*board.nets,PhysicalNet('PAIR_A',()),PhysicalNet('PAIR_B',())),
        net_routing_rules=(*board.net_routing_rules,
            *(NetRoutingRule(n,RouteKind.DIFFERENTIAL,width_nm=100000,pair_gap_nm=100000,
                             differential_partner=partner)
              for n,partner in (('PAIR_A','PAIR_B'),('PAIR_B','PAIR_A')))))
    baseline=route_outward_fixture(board) if shared_return else route_fanout(board)
    assert not baseline.decoupling_pending
    site=baseline.decoupling_witnesses[0].supply_path.vias[0].position
    obstacles=(TrackSegment('PAIR_A',Point(site.x_nm-50000,site.y_nm),Point(site.x_nm+50000,site.y_nm),
                            100000,CopperLayer.FRONT),
               TrackSegment('PAIR_B',Point.mm(2,2),Point.mm(2.1,2),100000,CopperLayer.FRONT))
    return board,baseline,obstacles


@pytest.mark.parametrize('operator',('partial','contact'))
def test_old_moving_cohort_path_shared_by_unselected_return_is_held_without_trial(monkeypatch,operator):
    import pcbir.contact_owner_regeneration as contact
    import pcbir.package_access as access
    board,baseline,obstacles=contact_scene(shared_return=True)
    old=baseline.decoupling_witnesses[0].supply_path.vias[0]
    unselected=baseline.decoupling_witnesses[1]
    assert unselected.link.net!=old.net and any(v is old for v in unselected.return_path.vias)
    # The moving P cohort's current canonical paths no longer select this via.
    paths,vias={},{}
    _verified_bypass_links(baseline.board,{old.net},witness_paths=paths,witness_vias=vias)
    assert all(v is not old for ps in vias.values() for v in ps)
    before=baseline.board
    def forbidden(*args,**kwargs):raise AssertionError('shared unselected owner must not be released')
    monkeypatch.setattr(access,'route_fanout',forbidden)
    monkeypatch.setattr(contact,'route_fanout',forbidden)
    if operator=='partial':
        answer=_partial_ordinary_pattern(baseline,board,replace(board,tracks=(*board.tracks,*obstacles)),
                                         FanoutOptions(),8,True)
        assert answer[0] is None and answer[-1]=='protected_ordinary_owner'
    else:
        assert contact.regenerate_contact_owners(board,baseline,('PAIR_A','PAIR_B'),obstacles,(),
                   FanoutOptions(),maximum_owner_nets=8) is None
    assert baseline.board is before and verified_bypass_reservations(board,baseline)[2] is None


@pytest.mark.parametrize('operator',('partial','contact'))
@pytest.mark.parametrize('corrupt',(False,True))
def test_selected_rebuild_merges_new_originals_or_rolls_back_completely(monkeypatch,operator,corrupt):
    import pcbir.contact_owner_regeneration as contact
    import pcbir.package_access as access
    board,baseline,obstacles=contact_scene()
    before=baseline.board
    calls=[]
    real=route_fanout
    def rebuild(*args,**kwargs):
        result=real(*args,**kwargs);calls.append(result)
        if corrupt:
            # Keep complete canonical paths and all ordinary geometry, but
            # corrupt the exact ORIGINAL source context being returned.
            records=tuple(replace(w,context_digest='stale') for w in result.decoupling_witnesses)
            return replace(result,decoupling_witnesses=records)
        return result
    monkeypatch.setattr(access,'route_fanout',rebuild)
    monkeypatch.setattr(contact,'route_fanout',rebuild)
    if operator=='partial':
        answer=_partial_ordinary_pattern(baseline,board,replace(board,tracks=(*board.tracks,*obstacles)),
                                         FanoutOptions(),8,True)
        proposed=answer[0]
    else:
        proposed=contact.regenerate_contact_owners(board,baseline,('PAIR_A','PAIR_B'),obstacles,(),
                   FanoutOptions(),maximum_owner_nets=8)
    assert len(calls)==1
    if corrupt:
        assert proposed is None
    else:
        assert proposed is not None and len(proposed.decoupling_witnesses)==2
        assert verified_bypass_reservations(board,proposed)[2] is None
        assert all(validate_crosslayer_witness(proposed.board,w)=='' for w in proposed.decoupling_witnesses)
        assert baseline.escaped_pads<=proposed.escaped_pads
        assert all(t not in proposed.board.tracks for t in obstacles)
    assert baseline.board is before and verified_bypass_reservations(board,baseline)[2] is None

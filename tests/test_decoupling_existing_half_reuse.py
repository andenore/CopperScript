"""Fresh complete halves survive a bounded missing-half target search."""
from dataclasses import replace

import pytest

import pcbir.decoupling_paths as paths
from pcbir.drc import run_physical_drc
from pcbir.physical import FootprintPad, PadReference, Point, Size
from test_decoupling_crosslayer_paths import fixture


def returned_scene(renamed=False):
    source,link=fixture(renamed=renamed)
    full=paths.propose_crosslayer_decoupling(source,link)
    assert full.witness is not None,full.reason
    extras=tuple(FootprintPad(f'GROUND_{i}' if renamed else f'H{i}',Point.mm(x-8,y-8),Size.mm(.4,.4))
                 for i,(x,y) in enumerate(((11,8),(12,8),(13,8),(14,8),(11,10),(12,10),(13,10),(14,10))))
    chip=replace(source.footprints['chip'],pads=(*source.footprints['chip'].pads,*extras),
        courtyard=tuple(Point.mm(x,y) for x,y in ((-1.5,-1.7),(6.5,-1.7),(6.5,3),(-1.5,3))))
    # One actual held through-via completes R; P remains unrouted. No flag or
    # installed-component internal connection is used as proof.
    returned=replace(full.board,footprints={**source.footprints,'chip':chip},
        tracks=tuple(t for t in full.board.tracks if t.net==link.return_net),
        vias=tuple(v for v in full.board.vias if v.net==link.return_net),
        nets=tuple(replace(n,pads=(*n.pads,*(PadReference(link.target.component,p.number) for p in extras)))
                   if n.name==link.return_net else n for n in source.nets),
        net_routing_rules=tuple(replace(r,max_vias=1) if r.net==link.return_net else r
                                for r in source.net_routing_rules))
    partial=paths.verify_crosslayer_decoupling(returned,link)
    assert partial.return_path is not None and partial.supply_path is None
    position=paths._land(returned,link.return_pad)[2].position
    targets=tuple(sorted(paths._return_targets(returned,link),key=lambda ref:(
        (paths._land(returned,ref)[2].position.x_nm-position.x_nm)**2+
        (paths._land(returned,ref)[2].position.y_nm-position.y_nm)**2,ref)))
    assert len(targets)==9 and targets[-1]==partial.return_path.end
    return returned,link,partial,targets,full


def hard_findings(board):
    return [f for f in run_physical_drc(board).findings
            if f.severity.value=='error' and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]


def observe_connections(monkeypatch):
    actual=paths._connections
    calls=[]
    def observe(board,start,end,net,options,launch_limit,site_limit,**kwargs):
        calls.append((start,end,net,launch_limit,site_limit))
        return actual(board,start,end,net,options,launch_limit,site_limit,**kwargs)
    monkeypatch.setattr(paths,'_connections',observe)
    return calls


@pytest.mark.parametrize('renamed',(False,True))
@pytest.mark.parametrize('target_limit',(1,8))
def test_existing_ninth_target_return_is_reused_without_new_return_copper(monkeypatch,renamed,target_limit):
    source,link,existing,_,_=returned_scene(renamed)
    calls=observe_connections(monkeypatch)
    assert not hard_findings(source)
    result=paths.propose_crosslayer_decoupling(source,link,maximum_return_candidates=target_limit,
                                              maximum_proposals=1)
    assert result.witness is not None,result.reason
    assert result.witness.return_target==existing.return_path.end
    assert len(result.created_tracks)==3 and len(result.created_vias)==1
    assert all(t.net==link.net for t in (*result.created_tracks,*result.created_vias))
    assert calls and {c[2] for c in calls}=={link.net}
    assert all(c[3:]==(8,64) for c in calls)
    assert all(any(t is current for current in result.witness.return_path.tracks) for t in existing.return_path.tracks)
    assert all(any(v is current for current in result.witness.return_path.vias) for v in existing.return_path.vias)
    assert result.board.tracks[:len(source.tracks)]==source.tracks and result.board.vias[:len(source.vias)]==source.vias
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    assert not hard_findings(result.board) and not paths._budget_reason(result.board,link.return_net)


@pytest.mark.parametrize('renamed',(False,True))
def test_explicit_target_is_binding_even_when_another_complete_return_exists(renamed):
    source,link,existing,targets,_=returned_scene(renamed)
    same=paths.propose_crosslayer_decoupling(source,link,return_target=existing.return_path.end)
    assert same.witness is not None and same.witness.return_target==existing.return_path.end
    other=paths.propose_crosslayer_decoupling(source,link,return_target=targets[0])
    assert other.witness is None and other.board is source and not other.created_tracks and not other.created_vias
    assert paths.verify_crosslayer_decoupling(source,link,return_target=targets[0]).return_path is None
    unknown=paths.propose_crosslayer_decoupling(source,link,return_target=PadReference(link.target.component,'ABSENT'))
    assert unknown.reason=='return_target_not_declared_on_target_component' and unknown.board is source


@pytest.mark.parametrize('target_limit',(1,3,8))
def test_absent_return_preserves_original_nearest_target_bound_and_complete_supply(monkeypatch,target_limit):
    source,link,_,targets,full=returned_scene()
    # Keep the genuinely generated complete supply, remove R, and author zero
    # return vias. Existing supply reuse and ordinary nearest-target enumeration
    # must remain unchanged; none of the R targets has a physical path.
    source=replace(source,tracks=tuple(t for t in full.board.tracks if t.net==link.net),
        vias=tuple(v for v in full.board.vias if v.net==link.net),
        net_routing_rules=tuple(replace(r,max_vias=0) if r.net==link.return_net else r for r in source.net_routing_rules))
    check=paths.verify_crosslayer_decoupling(source,link)
    assert check.supply_path is not None and check.return_path is None
    calls=observe_connections(monkeypatch)
    result=paths.propose_crosslayer_decoupling(source,link,maximum_return_candidates=target_limit)
    assert result.witness is None and result.board is source and not result.created_tracks and not result.created_vias
    assert tuple(c[0] for c in calls)==targets[:target_limit]
    assert {c[2] for c in calls}=={link.return_net}
    assert all(c[3:]==(8,64) for c in calls)
    assert result.supply_path is not None and result.return_path is None


@pytest.mark.parametrize('fault',('removed_via','removed_track','width','layers','length','via_budget'))
def test_invalid_old_return_is_not_trusted_or_rebound(fault):
    source,link,existing,_,_=returned_scene()
    target=existing.return_path.end
    if fault=='removed_via':source=replace(source,vias=())
    elif fault=='removed_track':source=replace(source,tracks=source.tracks[1:])
    else:
        def alter(rule):
            if rule.net!=link.return_net:return rule
            if fault=='width':return replace(rule,width_nm=250000)
            if fault=='layers':return replace(rule,allowed_layers=(rule.allowed_layers[0],))
            if fault=='length':return replace(rule,max_length_nm=1)
            return replace(rule,max_vias=0)
        source=replace(source,net_routing_rules=tuple(alter(r) for r in source.net_routing_rules))
    assert paths.verify_crosslayer_decoupling(source,link,return_target=target).return_path is None
    baseline_hard={f.fingerprint for f in hard_findings(source)}
    # Do not require a routable missing half to remain pending: a fresh repair
    # is legal, but it must return a newly complete current-board witness.
    result=paths.propose_crosslayer_decoupling(source,link,return_target=target,maximum_proposals=1)
    if result.witness is not None:
        assert paths.validate_crosslayer_witness(result.board,result.witness)==''
        assert not {f.fingerprint for f in hard_findings(result.board)}-baseline_hard
        if fault=='width':
            # Existing narrow source copper remains a visible baseline error;
            # the new complete path may only use currently valid width.
            assert baseline_hard
            assert all(t.width_nm>=250000 for t in result.witness.return_path.tracks)
    else:
        assert result.board is source and not result.created_tracks and not result.created_vias


def test_already_complete_input_keeps_original_copper_without_proposal_generation(monkeypatch):
    source,link,_,_,_=returned_scene()
    complete=paths.propose_crosslayer_decoupling(source,link)
    assert complete.witness is not None
    calls=observe_connections(monkeypatch)
    again=paths.propose_crosslayer_decoupling(complete.board,link)
    assert again.board is complete.board and not again.created_tracks and not again.created_vias and not calls
    assert paths.validate_crosslayer_witness(again.board,again.witness)==''


@pytest.mark.parametrize('bound',('maximum_return_candidates','maximum_proposals','maximum_path_states'))
def test_positive_computational_bounds_remain_required_with_complete_return(bound):
    source,link,_,_,_=returned_scene()
    with pytest.raises(ValueError,match='bounds must be positive'):
        paths.propose_crosslayer_decoupling(source,link,**{bound:0})

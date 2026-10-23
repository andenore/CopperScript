from dataclasses import replace
import pytest
from test_pair_vias import board_fixture
from pcbir import CopperLayer, route_global
from pcbir.physical import BoardSide, PadKind, PadReference, Size
from pcbir.pair_search import PairSearchStats, paired_candidates
from pcbir.pair_vias import paired_via_candidates
from pcbir.pair_terminals import terminal_copper_layers
from pcbir.critical import _route_pair, _validate_candidate


def hybrid(reverse=False, inner=False):
    b=board_fixture(True)
    fp=b.footprints['pair']
    plated=replace(fp,name='plated',pads=tuple(replace(p,kind=PadKind.THROUGH_HOLE,
        size=Size.mm(.65,.65),drill=Size.mm(.3,.3),has_solder_paste=False) for p in fp.pads))
    poses=tuple(replace(p,footprint='plated' if (i==0 if reverse else i==1) else 'pair',
        side=BoardSide.FRONT if (i==0 if reverse else i==1) else BoardSide.BACK)
        for i,p in enumerate(b.placements))
    return replace(b,footprints={**b.footprints,'plated':plated},placements=poses,copper_keepouts=(),
        net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_2,CopperLayer.BACK)
            if inner else (CopperLayer.BACK,),max_vias=1 if inner else 0) for r in b.net_routing_rules))


@pytest.mark.parametrize('reverse',(False,True))
def test_front_pth_and_back_smd_connect_without_a_via(reverse):
    b=hybrid(reverse);routes={r.net:r for r in route_global(b).routes}
    c=next(paired_candidates(b,*b.net_routing_rules,routes['A'],routes['B']))
    assert not c.via_pairs and not c.return_vias
    assert {t.layer for t in (*c.first,*c.second)}=={CopperLayer.BACK}
    result,tracks,vias=_route_pair(b,*b.net_routing_rules,routes,exact_tracks=(c.first,c.second))
    result,_,_=_validate_candidate(b,result,tracks,vias,[],[])
    assert result.connected,result.diagnostics
    assert b.tracks==b.vias==()


def test_inner_middle_needs_only_smd_transition_and_its_return():
    b=hybrid(inner=True);routes={r.net:r for r in route_global(b).routes}
    stats=PairSearchStats()
    c=next(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B'],layers=(CopperLayer.INTERNAL_2,),stats=stats))
    assert len(stats.transition_inventory)==2
    assert any(item.direct_contacts for item in stats.transition_inventory)
    assert all(item.transitions for item in stats.transition_inventory)
    assert len(c.via_pairs)==1 and len(c.return_vias)==1
    result,tracks,vias=_route_pair(b,*b.net_routing_rules,routes,
        exact_tracks=(c.first,c.second),exact_via_pairs=c.via_pairs,exact_return_vias=c.return_vias)
    result,_,_=_validate_candidate(b,result,tracks,vias,[],[])
    assert result.connected,result.diagnostics


@pytest.mark.parametrize('failure',('unplated','duplicate','missing','smd','forbidden_layer','no_vias'))
def test_terminal_access_never_invents_copper(failure):
    b=hybrid(inner=True);fp=b.footprints['plated']
    if failure=='unplated':fp=replace(fp,pads=tuple(replace(p,kind=PadKind.NON_PLATED_THROUGH_HOLE) for p in fp.pads))
    if failure=='duplicate':fp=replace(fp,pads=(*fp.pads,fp.pads[0]))
    if failure=='missing':fp=replace(fp,pads=fp.pads[1:])
    if failure=='smd':fp=replace(fp,pads=tuple(replace(p,kind=PadKind.SMD,drill=None) for p in fp.pads))
    if failure in ('unplated','missing'):
        b=replace(b,nets=tuple(replace(n,pads=tuple(p for p in n.pads if p.component!='J2')) for n in b.nets))
    b=replace(b,footprints={**b.footprints,'plated':fp})
    if failure=='forbidden_layer':b=replace(b,net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.BACK,)) for r in b.net_routing_rules))
    if failure=='no_vias':b=replace(b,net_routing_rules=tuple(replace(r,max_vias=0) for r in b.net_routing_rules))
    # Use original physical guide identities for missing/duplicate contacts;
    # they cannot grant access to copper absent from the changed board.
    routes={r.net:r for r in route_global(hybrid(inner=True)).routes}
    stats=PairSearchStats()
    assert not list(paired_via_candidates(b,*b.net_routing_rules,routes['A'],routes['B'],
        layers=(CopperLayer.INTERNAL_2,),maximum_total_states=20,stats=stats))
    assert stats.expanded_states<=20
    if failure in ('unplated','duplicate','missing'):
        assert not terminal_copper_layers(b,PadReference('J2','1'))

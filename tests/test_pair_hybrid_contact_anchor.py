from dataclasses import replace
import pytest
from test_pair_terminal_fallback import fixture
from pcbir import route_global,route_critical_nets,CopperLayer,Size,PadReference
from pcbir.physical import BoardSide,PadKind
from pcbir.pair_terminals import prepare_pair_terminals
from pcbir.drc import explicit_copper_connectivity,run_physical_drc,PhysicalDrcPolicy


def hybrid(renamed=False,front_only=False):
    b=fixture(True);fp=b.footprints['contacts']
    fp=replace(fp,pads=tuple(replace(p,kind=PadKind.THROUGH_HOLE,drill=Size.mm(.2,.2),
        size=Size.mm(.65,.65),has_solder_paste=False) if p.number in ('3','4') else p for p in fp.pads))
    b=replace(b,footprints={**b.footprints,'contacts':fp},
        placements=tuple(replace(p,side=BoardSide.BACK) if p.reference=='U' and not front_only else p for p in b.placements),
        net_routing_rules=tuple(replace(r,allowed_layers=(CopperLayer.FRONT,) if front_only else (CopperLayer.FRONT,CopperLayer.BACK),max_vias=0) for r in b.net_routing_rules))
    if renamed:
        names={'U':'INPUT_NODE','J':'DUAL_CONTACT'}
        b=replace(b,placements=tuple(replace(p,reference=names[p.reference]) for p in b.placements),
            nets=tuple(replace(n,pads=tuple(PadReference(names[p.component],p.pad) for p in n.pads)) for n in b.nets))
    return b


@pytest.mark.parametrize('renamed',(False,True))
def test_joined_plated_contact_avoids_redundant_main_transition(renamed):
    b=hybrid(renamed);coarse=route_global(b)
    coarse=replace(coarse,routes=tuple(replace(r,connected=False,accesses=(),segments=(),vias=()) for r in coarse.routes))
    prepared,a,z,vias,returns=prepare_pair_terminals(b,*b.net_routing_rules,{r.net:r for r in coarse.routes})
    connector='DUAL_CONTACT' if renamed else 'J'
    assert {x.pad.pad for g in prepared.values() for x in g.accesses if x.pad.component==connector}=={'3','4'}
    assert a and z and not vias and not returns
    result=route_critical_nets(b,coarse)
    assert all(r.connected for r in result.nets),result.nets
    assert not result.board.vias
    assert {t.layer for t in result.board.tracks}=={CopperLayer.FRONT,CopperLayer.BACK}
    graph=explicit_copper_connectivity(result.board)
    assert all(graph.net_connected(n) for n in b.nets)
    assert not [f for f in run_physical_drc(result.board,policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']
    assert b.tracks==b.vias==()


def test_restricted_single_layer_keeps_equivalent_anchor_order():
    b=hybrid(front_only=True);routes={r.net:r for r in route_global(b).routes}
    prepared,*_=prepare_pair_terminals(b,*b.net_routing_rules,routes)
    assert {x.pad.pad for g in prepared.values() for x in g.accesses if x.pad.component=='J'}=={'1','2'}


def test_failed_main_search_rolls_back_every_local_contact_branch():
    b=hybrid();result=route_critical_nets(b,route_global(b),pair_state_limit=1)
    assert not any(r.connected for r in result.nets)
    assert result.board.tracks==b.tracks and result.board.vias==b.vias

from dataclasses import replace
import pytest
from pcbir import decoupling_paths as paths
from pcbir.fanout import FanoutOptions
from pcbir.physical import (CopperKeepout,CopperLayer,PolygonWithHoles,PolygonRing,
    Point,FootprintPad,Size,PadShape,PhysicalFootprint,PadReference)
from pcbir.drc import run_physical_drc
from pcbir.routing_clearance import RoutingClearanceIndex
from test_decoupling_crosslayer_paths import fixture


def grid(renamed=False):
    b,l=fixture(renamed=renamed)
    pads=[]
    for x in (-1,0,1):
        for y in (-1,0,1):
            name='S' if (x,y)==(0,0) else 'G' if (x,y)==(-1,-1) else f'{x}:{y}'
            pads.append(FootprintPad(name,Point.mm(x,y),Size.mm(.25,.25),shape=PadShape.CIRCLE))
    fp=PhysicalFootprint('chip',tuple(pads),Size.mm(3,3))
    poses=(replace(b.placements[0],position=Point.mm(8,8)),b.placements[1])
    keepouts=[]
    for sx in (-1,1):
        for sy in (-1,1):
            pts=[Point.mm(8+sx*x,8+sy*y) for x,y in ((.15,.15),(.3,.15),(.3,.3),(.15,.3))]
            keepouts.append(CopperKeepout(f'bend-{sx}-{sy}',(CopperLayer.FRONT,),
                PolygonWithHoles(PolygonRing(tuple(pts))),block_tracks=True,block_vias=False))
    b=replace(b,footprints={**b.footprints,'chip':fp},placements=poses,copper_keepouts=tuple(keepouts),
        nets=tuple(replace(n,pads=(*n.pads,*(PadReference(l.target.component,p.number) for p in pads if p.number not in {'S','G'}))) if n.name==l.return_net else n for n in b.nets))
    return b,l


@pytest.mark.parametrize('renamed',(False,True))
def test_actual_array_cell_maze_recovers_blocked_straight_launch(monkeypatch,renamed):
    b,l=grid(renamed)
    options=FanoutOptions(two_leg_escapes=False,maze_escapes=True)
    with monkeypatch.context() as old:
        old.setattr(paths,'_cell_maze_launches',lambda *a:iter(()))
        failed=paths.propose_crosslayer_decoupling(b,l,fanout_options=options,maximum_site_candidates=4)
        assert failed.witness is None and failed.board is b
    r=paths.propose_crosslayer_decoupling(b,l,fanout_options=options,maximum_site_candidates=4)
    assert r.witness is not None,r.reason
    assert paths.validate_crosslayer_witness(r.board,r.witness)==''
    assert not [f for f in run_physical_drc(r.board).findings if f.severity.value=='error' and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]


def test_cell_maze_shares_remaining_transaction_budget(monkeypatch):
    b,l=grid();origin=paths._land(b,l.target);work=paths._ConnectionWork(local_paths=3);calls=[]
    def blocked(*args,**kwargs):calls.append(kwargs);return None
    monkeypatch.setattr(paths,'local_access_path',blocked)
    assert not tuple(paths._cell_maze_launches(b,origin,l.net,150000,
        FanoutOptions(maze_escapes=True),64,RoutingClearanceIndex(b),work))
    assert len(calls)==1 and work.local_paths==4
    assert calls[0]['maximum_states']==4000


@pytest.mark.parametrize('reason',('disabled','non_array','exhausted'))
def test_unsupported_or_exhausted_source_maze_adds_no_work(monkeypatch,reason):
    b,l=grid() if reason!='non_array' else fixture()
    work=paths._ConnectionWork(local_paths=4 if reason=='exhausted' else 0)
    def forbidden(*args,**kwargs):raise AssertionError('source maze must not run')
    monkeypatch.setattr(paths,'local_access_path',forbidden)
    assert not tuple(paths._cell_maze_launches(b,paths._land(b,l.target),l.net,150000,
        FanoutOptions(maze_escapes=reason!='disabled'),64,RoutingClearanceIndex(b),work))


@pytest.mark.parametrize('restriction',('supply_vias','supply_layers','return_vias'))
def test_source_limits_and_failed_return_keep_half_routes_disposable(restriction):
    b,l=grid()
    rules=[]
    for rule in b.net_routing_rules:
        if restriction=='supply_vias' and rule.net==l.net:rule=replace(rule,max_vias=0)
        if restriction=='supply_layers' and rule.net==l.net:rule=replace(rule,allowed_layers=(CopperLayer.FRONT,))
        if restriction=='return_vias' and rule.net==l.return_net:rule=replace(rule,max_vias=0)
        rules.append(rule)
    b=replace(b,net_routing_rules=tuple(rules))
    r=paths.propose_crosslayer_decoupling(b,l,
        fanout_options=FanoutOptions(two_leg_escapes=False,maze_escapes=True),maximum_site_candidates=4)
    assert r.witness is None and r.board is b
    assert not r.created_tracks and not r.created_vias


def test_incomplete_circular_cell_does_not_invent_a_via_site(monkeypatch):
    b,l=grid();chip=b.footprints['chip']
    chip=replace(chip,pads=tuple(p for p in chip.pads if p.number in {'S','G','-1:0'}))
    b=replace(b,footprints={**b.footprints,'chip':chip},
        nets=tuple(replace(n,pads=tuple(p for p in n.pads if p.component!=l.target.component or p.pad in {'S','G','-1:0'})) for n in b.nets))
    def forbidden(*args,**kwargs):raise AssertionError('incomplete cell cannot launch a maze')
    monkeypatch.setattr(paths,'local_access_path',forbidden)
    assert not tuple(paths._cell_maze_launches(b,paths._land(b,l.target),l.net,150000,
        FanoutOptions(maze_escapes=True),64,RoutingClearanceIndex(b),paths._ConnectionWork()))

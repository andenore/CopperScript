"""Synthetic QFN source geometry; no manufacturer or electrical qualification."""
from dataclasses import replace
from decimal import Decimal

from pcbir.physical import (BoardOutline,BoardSide,ComponentPlacementRule,CopperLayer,
    DecouplingLink,DesignRules,FootprintPad,NetRoutingRule,PadReference,PhysicalBoard,
    PhysicalFootprint,PhysicalNet,Placement,Point,Size,Stackup)


def build(*,rotated_renamed=False):
    lands=[]
    for side in range(4):
        for i in range(6):
            t=(i-2.5)*.5
            x,y,w,h=((-1.9,t,.7,.25),(t,1.9,.25,.7),(1.9,-t,.7,.25),(-t,-1.9,.25,.7))[side]
            lands.append(FootprintPad(str(len(lands)+1),Point.mm(x,y),Size.mm(w,h)))
    lands.append(FootprintPad('25',Point(0,0),Size.mm(2.4,2.4)))
    qfn=PhysicalFootprint('synthetic-qfn',tuple(lands),Size.mm(4,4))
    dual=PhysicalFootprint('dual-terminal',(
        FootprintPad('1',Point.mm(0,-.5),Size.mm(.5,.4)),
        FootprintPad('2',Point.mm(0,.5),Size.mm(.5,.4))),Size.mm(1.5,1.8))
    cap=PhysicalFootprint('synthetic-bypass',(
        FootprintPad('1',Point.mm(.5,0),Size.mm(.5,.5)),
        FootprintPad('2',Point.mm(-.5,0),Size.mm(.5,.5))),Size.mm(1.5,1))
    poses=(Placement('DEVICE',qfn.name,Point.mm(10,10)),
           Placement('FILTER',dual.name,Point.mm(3.5,10)),
           Placement('LOAD',dual.name,Point.mm(3.5,15)),
           Placement('BYPASS',cap.name,Point.mm(6,5)))
    link=DecouplingLink(PadReference('BYPASS','1'),PadReference('DEVICE','4'),
                       PadReference('BYPASS','2'),'SUPPLY','RETURN')
    nets=(PhysicalNet('SIGNAL_A',(PadReference('DEVICE','2'),PadReference('FILTER','1'),PadReference('LOAD','1'))),
          PhysicalNet('SIGNAL_B',(PadReference('DEVICE','3'),PadReference('FILTER','2'),PadReference('LOAD','2'))),
          PhysicalNet(link.net,(link.target,link.capacitor)),
          PhysicalNet(link.return_net,(PadReference('DEVICE','12'),PadReference('DEVICE','25'),link.return_pad)))
    rules=tuple(NetRoutingRule(n.name,width_nm=200000,allowed_layers=(CopperLayer.FRONT,),max_vias=0)
                for n in nets[:2])+(NetRoutingRule(link.net,width_nm=200000),)
    if rotated_renamed:
        refs={'DEVICE':'IC_K','FILTER':'TERMINALS_L','LOAD':'TERMINALS_M','BYPASS':'CAP_N'}
        names={n.name:f'NET_{i}' for i,n in enumerate(nets)}
        def ref(r):return PadReference(refs[r.component],r.pad)
        poses=tuple(replace(p,reference=refs[p.reference],position=Point(25_000_000-p.position.y_nm,p.position.x_nm),
                            rotation_degrees=Decimal(90)) for p in poses)
        nets=tuple(replace(n,name=names[n.name],pads=tuple(ref(p) for p in n.pads)) for n in nets)
        rules=tuple(replace(r,net=names[r.net]) for r in rules)
        link=replace(link,capacitor=ref(link.capacitor),target=ref(link.target),return_pad=ref(link.return_pad),
                     net=names[link.net],return_net=names[link.return_net])
    placement_rules=tuple(ComponentPlacementRule(p.reference,fixed_position=p.position,side=BoardSide.FRONT)
                          for p in poses[:-1])+(ComponentPlacementRule(poses[-1].reference,side=BoardSide.FRONT),)
    board=PhysicalBoard('synthetic constrained access',BoardOutline.rectangle(25,25),
        {f.name:f for f in (qfn,dual,cap)},poses,nets,
        rules=DesignRules(minimum_track_width_nm=100000,minimum_clearance_nm=100000,
            default_track_width_nm=200000,default_via_size_nm=450000,default_via_drill_nm=200000,
            minimum_hole_clearance_nm=200000),
        stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,CopperLayer.BACK)),
        net_routing_rules=rules,placement_rules=placement_rules,decoupling_links=(link,))
    return board,link,tuple(nets[:2])

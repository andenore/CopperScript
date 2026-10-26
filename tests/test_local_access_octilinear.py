"""Off-grid local routes must remain usable as exact physical package exits."""
from pcbir.physical import (BoardOutline,BoardSide,CopperLayer,DesignRules,FootprintPad,PadReference,
    PhysicalBoard,PhysicalFootprint,PhysicalNet,Placement,Point,Size,Via)
from pcbir.pin_access import local_access_path
from pcbir.routing_clearance import RoutingClearanceIndex

def scene(obstacle=False,back=False,renamed=False,rotation=0):
    net,ref=('OTHER_RAIL','RENAMED') if renamed else ('SIGNAL','LAND')
    def turn(p):
        for _ in range(rotation//90):p=Point(10000000-p.y_nm,p.x_nm)
        return p
    start=turn(Point(2337504,3537504));end=turn(Point(7000000,3450000))
    land=PhysicalFootprint('test/terminal',(FootprintPad('1',Point(0,0),Size.mm('.3','.3')),),Size.mm('.6','.6'))
    block=PhysicalFootprint('test/block',(FootprintPad('1',Point(0,0),Size.mm('.8','1.2')),),Size.mm('1','1.4'))
    side=BoardSide.BACK if back else BoardSide.FRONT;layer=CopperLayer.BACK if back else CopperLayer.FRONT
    placements=[Placement(ref,land.name,start,side=side)]
    nets=[PhysicalNet(net,(PadReference(ref,'1'),))]
    if obstacle:
        placements.append(Placement('FOREIGN',block.name,turn(Point.mm('4.5','3.5')),rotation_degrees=rotation,side=side))
        nets.append(PhysicalNet('FOREIGN_RAIL',(PadReference('FOREIGN','1'),)))
    via=Via(net,end,450000,200000,CopperLayer.FRONT,CopperLayer.BACK)
    board=PhysicalBoard('off-grid-local',BoardOutline.rectangle(10,10),{land.name:land,block.name:block},tuple(placements),tuple(nets),
        rules=DesignRules(minimum_clearance_nm=100000,minimum_track_width_nm=100000,default_track_width_nm=150000),vias=(via,))
    return board,PadReference(ref,'1'),net,start,end,layer


def test_existing_straight_shortcut_still_needs_one_clearance_query():
    board,pad,net,start,end,layer=scene()
    end=Point(start.x_nm+1000000,start.y_nm+1000000)
    base=RoutingClearanceIndex(board);calls=[]
    class Observed:
        def can_route(self,*a):calls.append(a);return base.can_route(*a)
        def route_pieces(self,*a):return base.route_pieces(*a)
    route=local_access_path(board,Observed(),net,start,end,layer,150000,step_nm=100000,detour_nm=1500000,maximum_states=1)
    assert route and len(route)==1 and len(calls)==1
    assert route[0].start==start and route[0].end==end

def test_tiny_budget_cannot_claim_unsearched_off_grid_join():
    board,pad,net,start,end,layer=scene(obstacle=True)
    assert local_access_path(board,RoutingClearanceIndex(board),net,start,end,layer,150000,
        step_nm=100000,detour_nm=1500000,maximum_states=1) is None


def test_local_diagnostics_distinguish_exhaustion_without_changing_copper():
    from pcbir.pin_access import LocalAccessStats
    board,pad,net,start,end,layer=scene(obstacle=True)
    for limit in (1,4000):
        stats=LocalAccessStats()
        def run(observed=None):
            return local_access_path(board,RoutingClearanceIndex(board),net,start,end,layer,150000,
                step_nm=100000,detour_nm=1500000,maximum_states=limit,stats=observed)
        assert run(stats)==run()
        assert stats.expanded_states<=limit
        assert stats.discovered_nodes>=stats.expanded_states
        assert stats.outcome==('state_limit' if limit==1 else 'connected')
    assert board.tracks==()

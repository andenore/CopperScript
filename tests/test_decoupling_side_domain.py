from dataclasses import replace
import pytest
from pcbir.physical import (BoardSide, BoardOutline, ComponentPlacementRule,
    CopperLayer, CopperKeepout, PlacementRegion, Point, PolygonRing, PolygonWithHoles)
from pcbir.decoupling import improve_decoupling_placement,route_decouplers
from pcbir.decoupling_paths import verify_crosslayer_decoupling
from pcbir.placement import PlacementPlannerOptions,placement_solution_is_legal
from test_decoupling_crosslayer_paths import fixture

def scene(renamed=False,side=None):
 b,link=fixture(renamed=renamed)
 b=replace(b,placements=tuple(replace(p,side=BoardSide.FRONT) for p in b.placements),
   placement_rules=(ComponentPlacementRule(link.target.component,fixed_position=b.placements[0].position,side=BoardSide.FRONT),
                    ComponentPlacementRule(link.capacitor.component,region='slot',side=side)),
   regions=(PlacementRegion('slot',BoardOutline((Point.mm(8,7),Point.mm(15,7),Point.mm(15,11),Point.mm(8,11)))),),
   copper_keepouts=(CopperKeepout('surface-wall',(CopperLayer.FRONT,),PolygonWithHoles(PolygonRing((Point.mm(10,0),Point.mm(10.4,0),Point.mm(10.4,20),Point.mm(10,20))))),))
 return b,link

@pytest.mark.parametrize('renamed',[False,True])
def test_unrestricted_front_incumbent_can_recover_on_back(renamed):
 b,link=scene(renamed);poses={p.reference:p for p in b.placements}
 assert placement_solution_is_legal(b,poses,PlacementPlannerOptions())
 assert route_decouplers(b).pending
 generated,moved=improve_decoupling_placement(b,poses,PlacementPlannerOptions())
 assert moved==1 and generated[link.capacitor.component].side is BoardSide.BACK
 assert generated[link.target.component]==poses[link.target.component]
 assert placement_solution_is_legal(b,generated,PlacementPlannerOptions())
 final=route_decouplers(replace(b,placements=tuple(generated.values())))
 assert not final.pending
 witness=verify_crosslayer_decoupling(final.board,link).witness
 assert witness and witness.supply_path.vias and witness.return_path.vias
 assert b.tracks==b.vias==() and b.placements==tuple(poses.values())

@pytest.mark.parametrize('restriction',['front','fixed','no_return_via'])
def test_side_and_copper_constraints_still_fail_closed(restriction):
 b,link=scene(side=BoardSide.FRONT if restriction=='front' else None)
 if restriction=='fixed':
  b=replace(b,placement_rules=tuple(replace(r,fixed_position=b.placements[1].position) if r.reference==link.capacitor.component else r for r in b.placement_rules))
 if restriction=='no_return_via':
  b=replace(b,net_routing_rules=tuple(replace(r,max_vias=0) if r.net==link.return_net else r for r in b.net_routing_rules))
 poses={p.reference:p for p in b.placements}
 generated,moved=improve_decoupling_placement(b,poses,PlacementPlannerOptions())
 if restriction=='fixed':
  assert (generated,moved)==(poses,0)
 else:
  # A legal front-side supply move remains allowed. The restriction must
  # prevent a back-side admission, not suppress unrelated valid alternatives.
  assert generated[link.capacitor.component].side is BoardSide.FRONT
 assert b.tracks==b.vias==()


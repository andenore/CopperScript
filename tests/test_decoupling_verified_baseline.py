"""Source-order-independent feed identity preservation during pose refinement."""
from dataclasses import replace

import pytest

from pcbir.decoupling import improve_decoupling_placement, route_decouplers
from pcbir.package_access import _verified_bypass_links
from pcbir.physical import (BoardSide, ComponentPlacementRule, DecouplingLink,
                           PadReference, PhysicalNet, Placement, Point, NetRoutingRule,
                           FootprintPad, Size)
from pcbir.placement import PlacementPlannerOptions, placement_solution_is_legal
from pcbir.routing_clearance import RoutingClearanceIndex
from test_routing_quality import bypass_board


def names(renamed):
    return ("IC_ALPHA", "BYPASS_BETA", "IC_GAMMA", "BYPASS_DELTA",
            "SUPPLY_ONE", "SUPPLY_TWO", "RETURN") if renamed else (
            "U1", "C1", "U2", "C2", "P1", "P2", "G")


def pending_before_valid(renamed=False):
    u1,c1,u2,c2,p1,p2,g = names(renamed)
    base=bypass_board()
    # The first capacitor starts on the other surface. Its original nearest
    # legal front pose puts the return land on the later vertical valid feed.
    x=10_833_332
    first=DecouplingLink(PadReference(c1,"1"),PadReference(u1,"1"),
                        PadReference(c1,"2"),p1,g)
    later=DecouplingLink(PadReference(c2,"1"),PadReference(u2,"1"),
                        PadReference(c2,"2"),p2,g)
    poses=(Placement(u1,"chip",Point.mm(8,8)),
           Placement(c1,"cap",Point.mm(20,20),side=BoardSide.BACK),
           Placement(u2,"chip",Point(x,3_000_000),rotation_degrees=90),
           Placement(c2,"cap",Point(x,13_000_000),rotation_degrees=90))
    return replace(base,placements=poses,
        nets=(PhysicalNet(p1,(first.target,first.capacitor)),
              PhysicalNet(p2,(later.target,later.capacitor)),
              PhysicalNet(g,(first.return_pad,later.return_pad))),
        decoupling_links=(first,later),
        placement_rules=(ComponentPlacementRule(u2,fixed_position=poses[2].position,
                                               fixed_rotation_degrees=90),
                         ComponentPlacementRule(c2,fixed_position=poses[3].position,
                                               fixed_rotation_degrees=90)))


@pytest.mark.parametrize("renamed",[False,True])
def test_pending_first_candidate_cannot_cut_later_verified_feed(renamed):
    board=pending_before_valid(renamed)
    options=PlacementPlannerOptions()
    initial={p.reference:p for p in board.placements};original=initial.copy()
    baseline=route_decouplers(board)
    witness={}
    verified=_verified_bypass_links(baseline.board,{n.name for n in board.nets},
                                   witness_paths=witness)
    first,later=board.decoupling_links
    assert verified==frozenset({later})
    assert {link for link,_ in baseline.pending}=={first}
    # This legal source-order-first proposal was unprotected by the old prefix.
    unsafe=replace(initial[first.capacitor.component],side=BoardSide.FRONT,
                   position=Point(10_333_332,8_000_000),rotation_degrees=0)
    trial={**initial,unsafe.reference:unsafe}
    assert placement_solution_is_legal(board,trial,options)
    obstructed=replace(baseline.board,placements=tuple(trial.values()))
    index=RoutingClearanceIndex(obstructed)
    assert any(not index.can_track(t.net,t.start,t.end,t.width_nm,t.layer)
               for t in witness[later])
    generated,moved=improve_decoupling_placement(board,initial,options)
    assert moved==0 and generated==initial
    assert generated[later.capacitor.component]==initial[later.capacitor.component]
    assert generated[later.target.component]==initial[later.target.component]
    assert initial==original and board.placements==tuple(original.values())
    assert placement_solution_is_legal(board,generated,options)
    current=route_decouplers(replace(board,placements=tuple(generated.values())))
    assert verified <= _verified_bypass_links(current.board,{n.name for n in board.nets})
    assert {link for link,_ in current.pending}=={first}
    assert board.placement_rules[0].reference==later.target.component


@pytest.mark.parametrize("renamed",[False,True])
def test_shared_original_witness_segments_are_not_released(renamed,monkeypatch):
    u1,c1,_,c2,p,_,g=names(renamed)
    base=bypass_board()
    links=tuple(DecouplingLink(PadReference(c,"1"),PadReference(u1,"1"),
                              PadReference(c,"2"),p,g) for c in (c1,c2))
    chip=replace(base.footprints["chip"],pads=(
        FootprintPad("1",Point.mm(1,0),Size.mm(.8,.225)),
        FootprintPad("2",Point.mm(1,.4),Size.mm(.8,.225))))
    board=replace(base,
        footprints={**base.footprints,"chip":chip},
        placements=(Placement(u1,"chip",Point.mm(8,8)),
                    Placement(c1,"cap",Point.mm(15,14)),
                    Placement(c2,"cap",Point.mm(15,20))),
        nets=(PhysicalNet(p,(links[0].target,links[0].capacitor,links[1].capacitor)),
              PhysicalNet(g,(PadReference(u1,"2"),links[0].return_pad,
                             links[1].return_pad))),decoupling_links=links,
        rules=replace(base.rules,minimum_track_width_nm=100000,minimum_clearance_nm=90000),
        net_routing_rules=(NetRoutingRule(p,width_nm=200000,
            breakout_length_nm=2000000,breakout_width_nm=100000),))
    initial={p.reference:p for p in board.placements}
    baseline=route_decouplers(board);paths={}
    verified=_verified_bypass_links(baseline.board,{p},witness_paths=paths)
    assert verified==frozenset(links)
    shared=set(paths[links[0]]) & set(paths[links[1]])
    assert shared, [[(t.start,t.end,t.width_nm) for t in path] for path in paths.values()]
    assert all(any(track is original for original in baseline.board.tracks)
               for path in paths.values() for track in path)
    import pcbir.decoupling as module
    original=module._surface_candidates;observed=[]
    def observe(scratch,index,link,*args):
        if link==links[0] and scratch.tracks:
            assert shared <= set(scratch.tracks)
            observed.append(True)
        yield from original(scratch,index,link,*args)
    monkeypatch.setattr(module,"_surface_candidates",observe)
    result,moved=improve_decoupling_placement(board,initial,PlacementPlannerOptions())
    assert observed and moved>=0
    assert board.placements==tuple(initial.values())
    current=route_decouplers(replace(board,placements=tuple(result.values())))
    assert verified <= _verified_bypass_links(current.board,{p})


def test_exclusive_generated_feed_can_shorten_without_losing_source_identity():
    board=bypass_board();initial={p.reference:p for p in board.placements}
    before=route_decouplers(board)
    verified=_verified_bypass_links(before.board,{"P"})
    assert verified==frozenset(board.decoupling_links)
    poses,moved=improve_decoupling_placement(board,initial,PlacementPlannerOptions())
    assert moved==1 and poses["C"]!=initial["C"] and poses["U"]==initial["U"]
    after=route_decouplers(replace(board,placements=tuple(poses.values())))
    assert _verified_bypass_links(after.board,{"P"})==verified
    assert not after.pending and not after.board.vias
    assert board.tracks==() and board.vias==()


def test_routed_source_rejection_preserves_input_frame():
    routed=route_decouplers(bypass_board()).board
    tracks,vias,placements=routed.tracks,routed.vias,routed.placements
    with pytest.raises(ValueError,match="unrouted"):
        improve_decoupling_placement(routed,{p.reference:p for p in placements},
                                    PlacementPlannerOptions())
    assert (routed.tracks,routed.vias,routed.placements)==(tracks,vias,placements)

"""Bounded generated pose refinement preserves actual supply/return owners."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir.decoupling import improve_decoupling_placement, route_decouplers
from pcbir.decoupling_paths import verify_crosslayer_decoupling, validate_crosslayer_witness
from pcbir.hard_macros import materialize_hard_macros
from pcbir.package_access import _verified_bypass_links
from pcbir.physical import (BoardSide, ComponentPlacementRule, Point,
                           PadReference, Placement)
from pcbir.placement import PlacementPlannerOptions, placement_solution_is_legal
from test_decoupling_crosslayer_paths import fixture


def back_only(board, *, movable):
    return replace(board,placement_rules=tuple(ComponentPlacementRule(
        p.reference,side=p.side,fixed_position=None if p.reference in movable else p.position,
        fixed_rotation_degrees=0) for p in board.placements))


def two_links(renamed=False):
    first,link=fixture(renamed=renamed)
    other,other_link=fixture(renamed=not renamed)
    board=replace(first,
        placements=(*first.placements,*(replace(p,position=Point(p.position.x_nm,p.position.y_nm+6_000_000))
                                       for p in other.placements)),
        nets=(*first.nets,*other.nets),decoupling_links=(link,other_link),
        net_routing_rules=(*first.net_routing_rules,*other.net_routing_rules))
    return back_only(board,movable={link.capacitor.component}),link,other_link


@pytest.mark.parametrize("renamed",[False,True])
def test_back_pose_refinement_regenerates_both_paths_preserving_prior_ids(renamed):
    board,link,later=two_links(renamed)
    original={p.reference:p for p in board.placements}
    baseline=route_decouplers(board)
    tracks,vias={},{}
    nets={n.name for n in board.nets}
    verified=_verified_bypass_links(baseline.board,nets,witness_paths=tracks,witness_vias=vias)
    assert verified==frozenset({link,later}) and all(vias.values())
    generated,moved=improve_decoupling_placement(board,original,PlacementPlannerOptions())
    assert moved==1 and generated[link.capacitor.component]!=original[link.capacitor.component]
    assert generated[link.capacitor.component].side is BoardSide.BACK
    assert all(generated[ref]==pose for ref,pose in original.items() if ref!=link.capacitor.component)
    assert placement_solution_is_legal(board,generated,PlacementPlannerOptions())
    assert board.placements==tuple(original.values()) and board.tracks==board.vias==()
    rerouted=route_decouplers(replace(board,placements=tuple(generated.values())))
    assert not rerouted.pending
    assert verified <= _verified_bypass_links(rerouted.board,nets)
    for association in (link,later):
        measured=verify_crosslayer_decoupling(rerouted.board,association)
        assert measured.witness and measured.supply_path.vias and measured.return_path.vias


def test_blocked_return_rejects_pose_changes_and_generated_copper():
    board,link=fixture()
    board=back_only(board,movable={link.capacitor.component})
    board=replace(board,net_routing_rules=tuple(replace(r,max_vias=0)
        if r.net==link.return_net else r for r in board.net_routing_rules))
    original={p.reference:p for p in board.placements}
    assert route_decouplers(board).pending
    generated,moved=improve_decoupling_placement(board,original,PlacementPlannerOptions())
    assert moved==0 and generated==original
    assert board.placements==tuple(original.values()) and board.tracks==board.vias==()


def test_explicit_fixed_cap_and_zero_via_supply_are_preserved():
    board,link=fixture()
    fixed=back_only(board,movable=set())
    poses={p.reference:p for p in fixed.placements}
    assert improve_decoupling_placement(fixed,poses,PlacementPlannerOptions())==(poses,0)
    constrained=back_only(board,movable={link.capacitor.component})
    constrained=replace(constrained,net_routing_rules=tuple(replace(r,max_vias=0)
        if r.net==link.net else r for r in constrained.net_routing_rules))
    assert improve_decoupling_placement(constrained,poses,PlacementPlannerOptions())==(poses,0)
    from pcbir.physical import CopperLayer
    front_only=replace(constrained,net_routing_rules=tuple(replace(r,max_vias=2,
        allowed_layers=(CopperLayer.FRONT,)) if r.net==link.net else r
        for r in constrained.net_routing_rules))
    assert improve_decoupling_placement(front_only,poses,PlacementPlannerOptions())==(poses,0)


def test_final_normal_regeneration_requires_fresh_return_edges(monkeypatch):
    import pcbir.decoupling as module
    board,link=fixture();board=back_only(board,movable={link.capacitor.component})
    poses={p.reference:p for p in board.placements}
    ordinary=module.route_decouplers;calls=[]
    def regenerate(current,**kwargs):
        result=ordinary(current,**kwargs)
        calls.append(result)
        if len(calls)==2:
            assert current.placements!=board.placements
            # Inject loss of a real final return edge. Successful labels and
            # stale result witnesses cannot authorize the returned poses.
            damaged=replace(result.board,vias=tuple(v for v in result.board.vias if v.net!=link.return_net))
            return replace(result,board=damaged)
        return result
    monkeypatch.setattr(module,"route_decouplers",regenerate)
    generated,moved=module.improve_decoupling_placement(board,poses,PlacementPlannerOptions())
    assert len(calls)==2 and generated==poses and moved==0
    assert board.placements==tuple(poses.values()) and board.tracks==board.vias==()


def test_shared_generated_tracks_and_vias_are_not_released(monkeypatch):
    import pcbir.decoupling_paths as helper
    board,link=fixture()
    second=replace(link,capacitor=PadReference("C2","1"),return_pad=PadReference("C2","2"))
    board=replace(board,
        placements=(*board.placements,Placement("C2","cap",Point.mm(14,14),side=BoardSide.BACK)),
        nets=tuple(replace(n,pads=(*n.pads,second.capacitor if n.name==link.net else second.return_pad))
                   for n in board.nets),decoupling_links=(link,second),
        net_routing_rules=tuple(replace(r,max_vias=4,max_length_nm=50_000_000)
                                for r in board.net_routing_rules))
    board=back_only(board,movable={link.capacitor.component})
    baseline=route_decouplers(board);tracks,vias={},{}
    verified=_verified_bypass_links(baseline.board,{link.net},witness_paths=tracks,witness_vias=vias)
    assert verified==frozenset({link,second})
    shared_tracks=set(tracks[link]) & set(tracks[second])
    shared_vias=set(vias[link]) & set(vias[second])
    assert shared_tracks and shared_vias
    original=helper.propose_crosslayer_decoupling;observed=[]
    def observe(scratch,association,**kwargs):
        if association==link and scratch.vias:
            assert shared_tracks <= set(scratch.tracks)
            assert shared_vias <= set(scratch.vias)
            observed.append(True)
        return original(scratch,association,**kwargs)
    monkeypatch.setattr(helper,"propose_crosslayer_decoupling",observe)
    poses={p.reference:p for p in board.placements}
    generated,moved=improve_decoupling_placement(board,poses,PlacementPlannerOptions())
    assert observed and moved==1
    rerouted=route_decouplers(replace(board,placements=tuple(generated.values())))
    assert verified <= _verified_bypass_links(rerouted.board,{link.net})


def test_source_routed_input_is_rejected_without_releasing_tracks_or_vias():
    board,link=fixture()
    source=route_decouplers(board).board
    poses={p.reference:p for p in source.placements}
    tracks,vias=source.tracks,source.vias
    with pytest.raises(ValueError,match="unrouted"):
        improve_decoupling_placement(source,poses,PlacementPlannerOptions())
    assert source.tracks==tracks and source.vias==vias and source.placements==tuple(poses.values())


def test_explicit_planner_clearance_is_not_replaced_by_drc_defaults():
    from test_routing_quality import bypass_board
    board=bypass_board();poses={p.reference:p for p in board.placements}
    chosen=PlacementPlannerOptions(component_clearance_nm=200000)
    generated,moved=improve_decoupling_placement(board,poses,chosen)
    assert moved==1 and placement_solution_is_legal(board,generated,chosen)
    assert not placement_solution_is_legal(board,generated,PlacementPlannerOptions())
    default,_=improve_decoupling_placement(board,poses,PlacementPlannerOptions())
    assert generated["C"].position.x_nm < default["C"].position.x_nm
    routed=route_decouplers(replace(board,placements=tuple(generated.values())))
    assert not routed.pending and _verified_bypass_links(routed.board,{"P"})==frozenset(board.decoupling_links)
    assert board.placements==tuple(poses.values()) and board.tracks==board.vias==()


def test_required_array_multiplicity_and_pad_pose_survive_refinement(monkeypatch):
    import pcbir.decoupling_paths as helper
    from pcbir.physical import CopperLayer,CopperZone,PolygonRing,PolygonWithHoles,PadViaInPadRule
    board,link=fixture();old_return=link.return_net
    link=replace(link,return_net="GND")
    board=replace(board,metadata={"fabrication_profile":"jlcpcb-six-layer"},
        nets=tuple(replace(n,name="GND") if n.name==old_return else n for n in board.nets),
        net_routing_rules=tuple(replace(r,net="GND") if r.net==old_return else r
                               for r in board.net_routing_rules),decoupling_links=(link,),
        zones=(CopperZone("declared-ground","GND",(CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing((Point.mm(1,1),Point.mm(24,1),Point.mm(24,19),Point.mm(1,19))))),),
        via_in_pad_rules=(PadViaInPadRule(PadReference(link.target.component,"G"),rows=1,columns=1),))
    board=back_only(board,movable={link.capacitor.component})
    frame=materialize_hard_macros(board);arrays=Counter(frame.vias)
    assert arrays
    original=helper.propose_crosslayer_decoupling;observed=[]
    def observe(scratch,association,**kwargs):
        assert not arrays-Counter(scratch.vias)
        assert scratch.via_in_pad_rules==board.via_in_pad_rules
        observed.append(True)
        return original(scratch,association,**kwargs)
    monkeypatch.setattr(helper,"propose_crosslayer_decoupling",observe)
    poses={p.reference:p for p in board.placements}
    generated,moved=improve_decoupling_placement(board,poses,PlacementPlannerOptions())
    assert observed and moved==1
    assert generated[link.target.component]==poses[link.target.component]
    rerouted=route_decouplers(materialize_hard_macros(replace(board,placements=tuple(generated.values()))))
    assert not arrays-Counter(rerouted.board.vias)
    assert verify_crosslayer_decoupling(rerouted.board,link).witness
    assert board.vias==() and board.via_in_pad_rules==frame.via_in_pad_rules


def test_macro_copper_and_rigid_member_poses_remain_source_owned(tmp_path,monkeypatch):
    import pcbir.decoupling_paths as helper
    from test_hard_macros import fixture as macro_fixture
    macro_source,_,bind=macro_fixture(tmp_path)
    bound=bind()
    board,link=fixture()
    board=replace(board,footprints={**board.footprints,**bound.footprints},
        placements=(*board.placements,*bound.placements),nets=(*board.nets,*bound.nets),
        hard_macros=bound.hard_macros,rigid_clusters=bound.rigid_clusters)
    board=back_only(board,movable={link.capacitor.component})
    frame=materialize_hard_macros(board)
    measured=route_decouplers(frame)
    witness=verify_crosslayer_decoupling(measured.board,link).witness
    assert witness and validate_crosslayer_witness(measured.board,witness)==""
    assert Counter(frame.tracks) <= Counter(measured.board.tracks)
    assert measured.board.materialized_macros==frame.materialized_macros
    original=helper.propose_crosslayer_decoupling;observed=[]
    def observe(scratch,association,**kwargs):
        assert not Counter(frame.tracks)-Counter(scratch.tracks)
        assert not Counter(frame.vias)-Counter(scratch.vias)
        assert scratch.materialized_macros==frame.materialized_macros
        observed.append(True)
        return original(scratch,association,**kwargs)
    monkeypatch.setattr(helper,"propose_crosslayer_decoupling",observe)
    poses={p.reference:p for p in board.placements}
    generated,moved=improve_decoupling_placement(board,poses,PlacementPlannerOptions())
    assert observed and moved==1
    assert all(generated[p.reference]==p for p in macro_source.placements)
    final_frame=materialize_hard_macros(replace(board,placements=tuple(generated.values())))
    assert Counter(frame.tracks)==Counter(final_frame.tracks) and Counter(frame.vias)==Counter(final_frame.vias)
    assert board.tracks==board.vias==() and not board.materialized_macros

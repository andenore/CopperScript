"""Real geometry tests for explicit opposite-surface supply AND return paths."""
from collections import Counter
from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir.decoupling_paths import (propose_crosslayer_decoupling,
                                   verify_crosslayer_decoupling,
                                   validate_crosslayer_witness)
from pcbir.drc import run_physical_drc
from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
from pcbir.physical import (BoardOutline,BoardSide,CopperLayer,DecouplingLink,
    DesignRules,FootprintPad,InternalPadGroup,NetRoutingRule,PadReference,
    PadViaInPadRule,PhysicalBoard,PhysicalFootprint,PhysicalNet,Placement,
    Point,RouteKind,Size,Stackup,TrackSegment,Via,ViaKind,ViaTechnology)


def fixture(*,renamed=False,six=True):
    u,c,p,g=("IC_A","BYPASS_B","RAIL_C","RETURN_D") if renamed else ("U","C","P","R")
    chip=PhysicalFootprint("chip",(
        FootprintPad("S",Point.mm(1,0),Size.mm(.4,.4)),
        FootprintPad("G",Point.mm(1,2),Size.mm(.4,.4))),Size.mm(2,3))
    cap=PhysicalFootprint("cap",(
        FootprintPad("1",Point.mm(-.5,0),Size.mm(.4,.4)),
        FootprintPad("2",Point.mm(.5,0),Size.mm(.4,.4))),Size.mm(1.5,1))
    link=DecouplingLink(PadReference(c,"1"),PadReference(u,"S"),PadReference(c,"2"),p,g)
    layers=(CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,
            CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK) if six else (CopperLayer.FRONT,CopperLayer.BACK)
    board=PhysicalBoard("crosslayer",BoardOutline.rectangle(25,20),
        {"chip":chip,"cap":cap},
        (Placement(u,"chip",Point.mm(8,8)),Placement(c,"cap",Point.mm(13,9),side=BoardSide.BACK)),
        (PhysicalNet(p,(link.target,link.capacitor)),
         PhysicalNet(g,(PadReference(u,"G"),link.return_pad))),
        rules=DesignRules(minimum_track_width_nm=100000,minimum_clearance_nm=100000,
                         default_track_width_nm=150000,default_via_size_nm=450000,
                         default_via_drill_nm=200000,minimum_hole_clearance_nm=200000),
        stackup=Stackup(layers),decoupling_links=(link,),
        net_routing_rules=tuple(NetRoutingRule(n,width_nm=150000,
            allowed_layers=(CopperLayer.FRONT,CopperLayer.BACK),max_vias=2,
            max_length_nm=20_000_000) for n in (p,g)))
    return board,link


@pytest.fixture(scope="module")
def completed():
    board,link=fixture()
    result=propose_crosslayer_decoupling(board,link)
    assert result.witness is not None,result.reason
    return board,link,result


@pytest.mark.parametrize("renamed",[False,True])
def test_both_paths_adopted_atomically_with_original_through_edges(renamed):
    board,link=fixture(renamed=renamed)
    result=propose_crosslayer_decoupling(board,link)
    assert result.witness is not None,result.reason
    assert result.supply_verified and result.return_verified
    assert result.created_tracks and len(result.created_vias)==2
    assert result.board.placements==board.placements
    assert result.board.net_routing_rules==board.net_routing_rules
    assert result.witness.return_target==PadReference(link.target.component,"G")
    for path in (result.witness.supply_path,result.witness.return_path):
        assert path.vias
        assert all(any(t is original for original in result.board.tracks) for t in path.tracks)
        assert all(any(v is original for original in result.board.vias) for v in path.vias)
        assert all(v.finish=="standard" and v.from_layer is CopperLayer.FRONT
                   and v.to_layer is CopperLayer.BACK for v in path.vias)
    assert validate_crosslayer_witness(result.board,result.witness)==""
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value=="error" and f.code not in {"DRC-OPEN-NET","DRC-ROUTE-INCOMPLETE"}]
    assert "internals" in result.witness.qualification


def test_existing_path_is_read_only_and_idempotent(completed):
    _,link,result=completed
    measured=verify_crosslayer_decoupling(result.board,link)
    assert measured.witness and measured.created_tracks==measured.created_vias==()
    again=propose_crosslayer_decoupling(result.board,link)
    assert again.board is result.board and not again.created_tracks and not again.created_vias


def test_fresh_shared_index_requires_exact_current_board_object(completed):
    from pcbir.routing_clearance import RoutingClearanceIndex
    _,link,result=completed
    index=RoutingClearanceIndex(result.board)
    measured=verify_crosslayer_decoupling(result.board,link,index=index)
    assert measured.witness and validate_crosslayer_witness(result.board,measured.witness,index=index)==""
    # An equal immutable clone still requires its own newly constructed index.
    clone=replace(result.board)
    assert clone==result.board and clone is not result.board
    rejected=verify_crosslayer_decoupling(clone,link,index=index)
    assert rejected.reason=="stale_clearance_index" and not rejected.supply_verified and not rejected.return_verified
    assert validate_crosslayer_witness(clone,measured.witness,index=index)=="stale_clearance_index"
    changed=replace(result.board,tracks=())
    assert verify_crosslayer_decoupling(changed,link,index=index).reason=="stale_clearance_index"
    assert validate_crosslayer_witness(changed,measured.witness,index=index)=="stale_clearance_index"
    assert not verify_crosslayer_decoupling(changed,link).witness


def test_proposal_other_association_batch_builds_one_fresh_index(completed,monkeypatch):
    import pcbir.decoupling_paths as paths
    _,link,result=completed
    other,other_link=fixture(renamed=True)
    board=replace(result.board,
        placements=(*result.board.placements,*(replace(p,position=Point(p.position.x_nm,p.position.y_nm+6_000_000))
                                               for p in other.placements)),
        nets=(*result.board.nets,*other.nets),decoupling_links=(link,other_link),
        net_routing_rules=(*result.board.net_routing_rules,*other.net_routing_rules))
    original=paths.RoutingClearanceIndex
    calls=[]
    def fresh_index(current):
        calls.append(current)
        return original(current)
    monkeypatch.setattr(paths,"RoutingClearanceIndex",fresh_index)
    measured=paths.propose_crosslayer_decoupling(board,link)
    assert measured.witness and measured.board is board
    assert calls==[board]


def test_supply_only_does_not_claim_return_or_adopt_partial_attempt(completed):
    _,link,result=completed
    supply=replace(result.board,tracks=tuple(t for t in result.board.tracks if t.net==link.net),
                   vias=tuple(v for v in result.board.vias if v.net==link.net))
    check=verify_crosslayer_decoupling(supply,link)
    assert check.supply_verified and not check.return_verified and check.witness is None
    blocked=replace(supply,net_routing_rules=tuple(replace(r,max_vias=0)
        if r.net==link.return_net else r for r in supply.net_routing_rules))
    proposed=propose_crosslayer_decoupling(blocked,link)
    assert proposed.board is blocked and proposed.supply_verified and not proposed.return_verified
    assert proposed.created_tracks==proposed.created_vias==()


def test_existing_return_is_reported_and_borrowed_without_extra_via(completed):
    _,link,result=completed
    returned=replace(result.board,tracks=tuple(t for t in result.board.tracks if t.net==link.return_net),
                     vias=tuple(v for v in result.board.vias if v.net==link.return_net),
                     net_routing_rules=tuple(replace(r,max_vias=1) if r.net==link.return_net else r
                                              for r in result.board.net_routing_rules))
    partial=verify_crosslayer_decoupling(returned,link)
    assert partial.return_verified and not partial.supply_verified
    repaired=propose_crosslayer_decoupling(returned,link,return_target=partial.return_path.end)
    assert repaired.witness and all(v.net==link.net for v in repaired.created_vias)
    assert repaired.witness.return_path.vias[0] is returned.vias[0]


def test_vias_labels_and_zone_intent_do_not_prove_paths(completed):
    _,link,result=completed
    via_only=replace(result.board,tracks=(),metadata={"connected":True})
    assert not verify_crosslayer_decoupling(via_only,link).supply_verified
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles
    zone=CopperZone("intent",link.return_net,(CopperLayer.FRONT,CopperLayer.BACK),
        PolygonWithHoles(PolygonRing((Point.mm(1,1),Point.mm(24,1),Point.mm(24,19),Point.mm(1,19)))))
    supply=replace(result.board,tracks=tuple(t for t in result.board.tracks if t.net==link.net),
                   vias=tuple(v for v in result.board.vias if v.net==link.net),zones=(zone,))
    assert not verify_crosslayer_decoupling(supply,link).return_verified


@pytest.mark.parametrize("restriction",["zero_vias","front_only","length","specialized"])
def test_source_budgets_and_specialized_owners_fail_closed(restriction):
    board,link=fixture()
    rule=board.net_routing_rules[0]
    changes={"zero_vias":{"max_vias":0},"front_only":{"allowed_layers":(CopperLayer.FRONT,)},
             "length":{"max_length_nm":500000},"specialized":{"kind":RouteKind.POWER}}[restriction]
    board=replace(board,net_routing_rules=(replace(rule,**changes),board.net_routing_rules[1]))
    result=propose_crosslayer_decoupling(board,link,maximum_proposals=4)
    assert result.witness is None and result.board is board and result.reason
    assert result.created_tracks==result.created_vias==()


def test_whole_net_via_budget_counts_unrelated_existing_branch(completed):
    _,link,result=completed
    board=replace(result.board,vias=(*result.board.vias,
        Via(link.net,Point.mm(21,4),450000,200000)),
        net_routing_rules=tuple(replace(r,max_vias=1) if r.net==link.net else r
                               for r in result.board.net_routing_rules))
    check=verify_crosslayer_decoupling(board,link)
    assert not check.supply_verified and check.reason=="whole_net_via_budget_exceeded"


def test_span_labels_cannot_masquerade_as_plated_through_process(completed):
    _,link,result=completed
    tech=ViaTechnology("micro",ViaKind.MICROVIA,CopperLayer.FRONT,CopperLayer.BACK,
                       100000,100000,Decimal(10))
    # On a two-layer table the span labels match, but the declared kind is micro.
    board=replace(result.board,stackup=Stackup((CopperLayer.FRONT,CopperLayer.BACK),
        via_technologies=(tech,)),vias=tuple(replace(v,technology="micro") for v in result.board.vias))
    assert not verify_crosslayer_decoupling(board,link).supply_verified
    assert propose_crosslayer_decoupling(board,link).witness is None
    finished=replace(result.board,vias=tuple(replace(v,finish="filled-capped") for v in result.board.vias))
    assert not verify_crosslayer_decoupling(finished,link).supply_verified


def test_declared_normal_through_catalog_is_supported():
    board,link=fixture()
    tech=ViaTechnology("through",ViaKind.THROUGH,CopperLayer.FRONT,CopperLayer.BACK,
                       200000,100000,Decimal(10))
    board=replace(board,stackup=replace(board.stackup,via_technologies=(tech,)))
    result=propose_crosslayer_decoupling(board,link)
    assert result.witness and all(v.technology=="through" for v in result.created_vias)


def test_no_external_return_pin_is_inferred(completed):
    _,link,result=completed
    assert verify_crosslayer_decoupling(result.board,link,return_target=link.return_pad).reason=="return_target_not_declared_on_target_component"
    no_return=replace(result.board,nets=tuple(replace(n,pads=(link.return_pad,))
        if n.name==link.return_net else n for n in result.board.nets))
    check=verify_crosslayer_decoupling(no_return,link)
    assert check.supply_verified and not check.return_verified
    assert check.reason=="no_declared_target_return_pad"


def test_internal_pad_group_is_not_an_unproved_return_shortcut(completed):
    _,link,result=completed
    chip=result.board.footprints["chip"]
    chip=replace(chip,pads=(*chip.pads,FootprintPad("G2",Point.mm(-1,2),Size.mm(.4,.4))),
                 internal_pad_groups=(InternalPadGroup(("G","G2")),))
    board=replace(result.board,footprints={**result.board.footprints,"chip":chip},
        nets=tuple(replace(n,pads=(*n.pads,PadReference(link.target.component,"G2")))
                   if n.name==link.return_net else n for n in result.board.nets))
    check=verify_crosslayer_decoupling(board,link,return_target=PadReference(link.target.component,"G2"))
    assert check.supply_verified and not check.return_verified


def test_required_array_must_preexist_and_cannot_be_displaced():
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles
    board,link=fixture()
    old_return=link.return_net
    link=replace(link,return_net="GND")
    board=replace(board,metadata={"fabrication_profile":"jlcpcb-six-layer"},
        nets=tuple(replace(n,name="GND") if n.name==old_return else n for n in board.nets),
        net_routing_rules=tuple(replace(r,net="GND") if r.net==old_return else r
                               for r in board.net_routing_rules),decoupling_links=(link,),
        zones=(CopperZone("declared-ground","GND",(CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing((Point.mm(1,1),Point.mm(24,1),Point.mm(24,19),Point.mm(1,19))))),),
        via_in_pad_rules=(PadViaInPadRule(PadReference(link.target.component,"G"),rows=1,columns=1),))
    assert propose_crosslayer_decoupling(board,link).reason=="missing_required_array"
    fixed=materialize_via_in_pad_arrays(board)
    before=Counter(fixed.vias)
    result=propose_crosslayer_decoupling(fixed,link)
    assert result.witness is not None,result.reason
    assert not before-Counter(result.board.vias)
    assert result.board.placements==fixed.placements
    assert result.board.via_in_pad_rules==fixed.via_in_pad_rules


def test_exact_protected_path_rejects_missing_or_forged_edges(completed):
    _,link,result=completed
    witness=result.witness
    missing=replace(result.board,vias=())
    assert validate_crosslayer_witness(missing,witness)
    other,other_link=fixture(renamed=True)
    proposal=propose_crosslayer_decoupling(other,other_link,protected_witnesses=(witness,))
    assert proposal.board is other and proposal.reason=="protected_witness_invalid"
    twins=replace(result.board,tracks=tuple(replace(t) for t in result.board.tracks))
    assert validate_crosslayer_witness(twins,witness)=="witness_track_invalid"


def test_next_association_preserves_previous_exact_supply_and_return_paths():
    first,first_link=fixture()
    second,second_link=fixture(renamed=True)
    board=replace(first,
        placements=(*first.placements,*(replace(p,position=Point(p.position.x_nm,p.position.y_nm+6_000_000))
                                       for p in second.placements)),
        nets=(*first.nets,*second.nets),
        decoupling_links=(first_link,second_link),
        net_routing_rules=(*first.net_routing_rules,*second.net_routing_rules))
    earlier=propose_crosslayer_decoupling(board,first_link)
    assert earlier.witness is not None,earlier.reason
    later=propose_crosslayer_decoupling(earlier.board,second_link,
                                      protected_witnesses=(earlier.witness,))
    assert later.witness is not None,later.reason
    assert validate_crosslayer_witness(later.board,earlier.witness)==""
    assert all(t is earlier.board.tracks[i] for i,t in enumerate(later.board.tracks[:len(earlier.board.tracks)]))
    assert all(v is earlier.board.vias[i] for i,v in enumerate(later.board.vias[:len(earlier.board.vias)]))


def test_original_track_identity_survives_exact_interior_via_split(completed):
    _,link,result=completed
    path=result.witness.supply_path
    first=path.tracks[0]
    # Extend the source segment beyond its existing via. BFS splits the original
    # track at the exact via centre rather than synthesizing replacement edges.
    via=path.vias[0]
    direction=Point(via.position.x_nm-first.start.x_nm,via.position.y_nm-first.start.y_nm)
    extended=replace(first,end=Point(via.position.x_nm+direction.x_nm,
                                    via.position.y_nm+direction.y_nm))
    board=replace(result.board,tracks=tuple(extended if t is first else t for t in result.board.tracks))
    measured=verify_crosslayer_decoupling(board,link)
    assert measured.witness
    assert any(t is extended for t in measured.witness.supply_path.tracks)


def test_finite_bound_arguments_are_not_electrical_limits():
    board,link=fixture()
    with pytest.raises(ValueError,match="positive"):
        propose_crosslayer_decoupling(board,link,maximum_launch_candidates=0)
    assert verify_crosslayer_decoupling(board,link,maximum_path_states=1).witness is None

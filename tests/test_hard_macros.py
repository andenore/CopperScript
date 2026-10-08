from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path
import subprocess

import pytest

from pcbir.clusters import cluster_placements, footprint_geometry_digest
from pcbir.hard_macros import bind_hard_macro, materialize_hard_macros, macro_routing_pads
from pcbir.physical import (
    BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, Via,
)


def fixture(tmp_path):
    fp = PhysicalFootprint("test", (FootprintPad("1", Point(0,0), Size.mm(.2,.2)),), Size.mm(.5,.5))
    board = PhysicalBoard("MacroTest", BoardOutline.rectangle(30,30), {"test": fp},
        (Placement("U1","test",Point.mm(10,10)), Placement("R1","test",Point.mm(12,10))),
        (PhysicalNet("N", (PadReference("U1","1"), PadReference("R1","1"))),))
    region = dict(id="private", layers=["F.Cu"], vertices=[[1000000,-500000],[3000000,-500000],
                    [3000000,500000],[1000000,500000]], block_tracks=True, block_vias=True, block_zones=False)
    asset = dict(schema="copperlib-physical-hard-macro/v0.1", source="synthetic test", production_publishable=False,
        anchor="chip", members=[dict(reference=ref, footprint="test", footprint_digest=footprint_geometry_digest(fp),
            center_nm=point, rotation_degrees="0", edge_clearance_nm=250000)
            for ref,point in (("chip",[0,0]),("passive",[2000000,0]))],
        pad_nets=[["chip","1","signal"],["passive","1","signal"]],
        isolated_pads=[],
        tracks=[dict(net="signal",width_nm=200000,layer="F.Cu",
                     points=[{"pad":["chip","1"]},{"pad":["passive","1"]}])], vias=[],
        ports=[dict(name="signal",net="signal",point={"pad":["chip","1"]},layer="F.Cu",
                    pads=[["chip","1"],["passive","1"]])], protected_regions=[region],
        keepouts=[{**region,"block_tracks":False,"block_zones":True}], required_layers=["F.Cu","B.Cu"],
        allowed_rotations=[0,45,90], internal_clearance_nm=0, unresolved=["synthetic, not fabrication"])
    path = tmp_path / "macro.json"
    def bind(data=asset, input_board=board, identity=None):
        raw = (json.dumps(data,sort_keys=True)+"\n").encode()
        path.write_bytes(raw)
        return bind_hard_macro(input_board,path,expected_sha256=identity or sha256(raw).hexdigest(),name="unit",
                              bindings={"chip":"U1","passive":"R1"}, net_bindings={"signal":"N"})
    return board,asset,bind


@pytest.mark.parametrize("rotation",[0,45,90])
def test_materialization_is_transactional_and_rotates_all_geometry(tmp_path,rotation):
    original,_,bind = fixture(tmp_path)
    board = bind()
    poses = {p.reference:p for p in board.placements}
    poses.update(cluster_placements(board,board.rigid_clusters[0],replace(poses["U1"],rotation_degrees=rotation)))
    board = replace(board,placements=tuple(poses.values()))
    result = materialize_hard_macros(board)
    assert result.tracks[0].start == poses["U1"].position
    assert result.tracks[0].end == poses["R1"].position
    assert result.materialized_macros == ("unit",)
    assert result.nets == original.nets
    assert not board.tracks and not original.hard_macros
    assert materialize_hard_macros(result) is result
    assert macro_routing_pads(result,result.nets[0]) == (PadReference("U1","1"),)


def test_owned_local_zone_is_immutable_and_does_not_defer_external_routing(tmp_path):
    from pcbir.hard_macros import macro_source
    from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles
    from pcbir.zone_geometry import distribution_zone_nets
    _, asset, bind = fixture(tmp_path)
    asset["schema"] = "copperlib-physical-hard-macro/v0.2"
    asset["keepouts"] = []
    asset["plane_returns"] = []
    asset["zones"] = [dict(id="local",net="signal",layers=["F.Cu"],
        vertices=[[1600000,-250000],[2400000,-250000],[2400000,250000],[1600000,250000]],
        priority=2,clearance_nm=200000,minimum_width_nm=200000,pad_connection="solid")]
    board = materialize_hard_macros(bind())
    assert len(board.zones) == 1 and board.zones[0].id == "unit/local"
    assert distribution_zone_nets(board) == set()
    assert not macro_source(board).zones
    rotated = bind()
    poses = {p.reference: p for p in rotated.placements}
    poses.update(cluster_placements(rotated, rotated.rigid_clusters[0],
                                    replace(poses["U1"], rotation_degrees=45)))
    turned = materialize_hard_macros(replace(rotated, placements=tuple(poses.values())))
    assert turned.zones[0].outline != board.zones[0].outline
    with pytest.raises(ValueError, match="immutable hard-macro zone"):
        replace(board, zones=())
    host = CopperZone("host", "N", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(11.5, 9.7), Point.mm(12.5, 9.7),
        Point.mm(12.5, 10.3), Point.mm(11.5, 10.3)))))
    with pytest.raises(ValueError, match="host zone overlaps"):
        replace(board, zones=(*board.zones, host))
    asset["zones"][0]["vertices"][1:3] = [[4000000,-250000],[4000000,250000]]
    with pytest.raises(ValueError, match="inside its protected region"):
        bind()


@pytest.mark.parametrize("rotation", [45, 90])
def test_fixed_polygon_is_netted_explicit_copper_and_immutable(tmp_path, rotation):
    from pcbir.drc import explicit_copper_connectivity
    from pcbir.hard_macros import macro_source
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    _, asset, bind = fixture(tmp_path)
    asset["schema"] = "copperlib-physical-hard-macro/v0.3"
    asset["zones"] = []
    asset["plane_returns"] = []
    asset["polygons"] = [dict(id="bridge", net="signal", layer="F.Cu", vertices=[
        [1400000,-200000],[2200000,-200000],[2200000,200000],
        [1900000,200000],[1900000,100000],[1400000,100000]])]
    asset["tracks"][0]["points"] = [{"pad":["chip","1"]},[1500000,0]]
    board = materialize_hard_macros(bind())
    assert explicit_copper_connectivity(board).net_connected(board.nets[0])
    assert board.polygons[0].id == "unit/bridge"
    assert not macro_source(board).polygons
    manifest = KiCadPcbBackend().generate(board)
    assert manifest.target_version == "10.0"
    copper = manifest.artifacts[0].content.split("(gr_poly", 1)[1].split("\n  )", 1)[0]
    assert "(fill yes)" in copper and "(locked yes)" in copper and "(net 1)" in copper
    with pytest.raises(ValueError, match="immutable hard-macro polygon"):
        replace(board, polygons=())
    rotated = bind()
    poses = {p.reference:p for p in rotated.placements}
    poses.update(cluster_placements(rotated, rotated.rigid_clusters[0],
                                    replace(poses["U1"], rotation_degrees=rotation)))
    turned = materialize_hard_macros(replace(rotated, placements=tuple(poses.values())))
    assert turned.polygons[0].outline != board.polygons[0].outline
    asset["polygons"][0]["vertices"][1] = [4000000,-200000]
    with pytest.raises(ValueError, match="inside its protected region"):
        bind()


def test_plane_return_requires_real_via_and_declared_plane(tmp_path):
    from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles
    _, asset, bind = fixture(tmp_path)
    asset["schema"] = "copperlib-physical-hard-macro/v0.2"
    asset["zones"] = []
    asset["keepouts"] = []
    asset["ports"] = []
    asset["plane_returns"] = [dict(net="signal",layers=["B.Cu"],
        pads=[["chip","1"],["passive","1"]],
        dedicated_contacts=[dict(pad=["chip","1"],via_position_nm=[600000,0]),
                            dict(pad=["passive","1"],via_position_nm=[1400000,0])])]
    asset["vias"] = [dict(net="signal",position_nm=xy,size_nm=600000,drill_nm=300000,
                          from_layer="F.Cu",to_layer="B.Cu",technology=None)
                     for xy in ([600000,0],[1400000,0])]
    with pytest.raises(ValueError, match="no declared plane"):
        materialize_hard_macros(bind())
    original, _, _ = fixture(tmp_path)
    plane = CopperZone("return", "N", (CopperLayer.BACK,), PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(29, 1), Point.mm(29, 29), Point.mm(1, 29)))))
    source = replace(original, zones=(plane,))
    assert materialize_hard_macros(bind(input_board=source)).vias
    asset["vias"] = []
    with pytest.raises(ValueError, match="lacks a local via"):
        materialize_hard_macros(bind(input_board=source))


@pytest.mark.parametrize("change,message",[
    ("hash","identity"),("footprint","footprint identity"),("layer","layer contract"),
    ("coordinate","integer"),("unknown","unsupported"),("pad","pad/net"),
    ("port","outside protected"),("qualification","qualification"),
])
def test_assets_fail_closed(tmp_path,change,message):
    board,asset,bind = fixture(tmp_path)
    identity=None
    if change == "hash": identity="0"*64
    elif change == "footprint": asset["members"][0]["footprint_digest"]="0"*64
    elif change == "layer": asset["required_layers"].append("In1.Cu")
    elif change == "coordinate": asset["members"][0]["center_nm"][0]=.5
    elif change == "unknown": asset["run_script"]="not executed"
    elif change == "pad": asset["pad_nets"][0][1]="2"
    elif change == "port": asset["ports"][0]["point"]=[2000000,0]
    elif change == "qualification": asset["production_publishable"]=True
    with pytest.raises(ValueError,match=message): bind(identity=identity)
    assert not board.hard_macros and not board.tracks


def test_owner_copper_and_same_net_private_access_are_immutable(tmp_path):
    _,_,bind=fixture(tmp_path)
    board=materialize_hard_macros(bind())
    from pcbir.routing_clearance import RoutingClearanceIndex
    index=RoutingClearanceIndex(board)
    track=TrackSegment("N",Point.mm(12,10),Point.mm(12,11),200000,CopperLayer.FRONT)
    assert not index.can_track(track.net,track.start,track.end,track.width_nm,track.layer)
    assert index.blocking_track_nets(track)[1]
    assert not index.can_via("N",Point.mm(12,10),600000,CopperLayer.FRONT,CopperLayer.BACK)
    with pytest.raises(ValueError,match="intrudes"): replace(board,tracks=(*board.tracks,track))
    with pytest.raises(ValueError,match="changed or removed"): replace(board,tracks=())
    poses={p.reference:p for p in board.placements}
    poses.update(cluster_placements(board,board.rigid_clusters[0],replace(poses["U1"],position=Point.mm(11,10))))
    with pytest.raises(ValueError,match="changed or removed"): replace(board,placements=tuple(poses.values()))


def test_omitted_private_pad_cannot_hide_a_broken_internal_net(tmp_path):
    original, asset, bind = fixture(tmp_path)
    bound = bind()
    # One remaining bound node would appear connected if coverage were only
    # checked against the asset's own incomplete pad list.
    asset.update(pad_nets=asset["pad_nets"][:1], ports=[])
    asset["tracks"][0]["points"][-1] = [500000, 0]
    with pytest.raises(ValueError, match="requires an external port"):
        materialize_hard_macros(bind(data=asset))
    macro = bound.hard_macros[0]
    with pytest.raises(ValueError, match="requires an external port"):
        materialize_hard_macros(replace(bound, hard_macros=(replace(macro,
            pad_bindings=macro.pad_bindings[:1], ports=(),
            tracks=(replace(macro.tracks[0], end=Point(500000, 0)),)),)))
    assert not original.tracks and not original.hard_macros


def test_member_ic_can_retain_unrelated_public_pads(tmp_path):
    original, asset, bind = fixture(tmp_path)
    fp = replace(original.footprints["test"], pads=(
        *original.footprints["test"].pads, FootprintPad("2", Point.mm(0, 1), Size.mm(.2, .2))))
    for member in asset["members"]:
        member["footprint_digest"] = footprint_geometry_digest(fp)
    public = PhysicalNet("GPIO", (PadReference("U1", "2"), PadReference("R1", "2")))
    board = replace(original, footprints={"test": fp}, nets=(*original.nets, public))
    result = materialize_hard_macros(bind(input_board=board))
    assert macro_routing_pads(result, public) == public.pads


def test_bad_copper_port_and_via_are_rejected_without_partial_commit(tmp_path):
    _,asset,bind=fixture(tmp_path)
    asset["vias"]=[dict(net="signal",position_nm=[2000000,0],size_nm=600000,drill_nm=300000,
                        from_layer="F.Cu",to_layer="B.Cu",technology=None)]
    source=bind()
    with pytest.raises(ValueError,match="VIA-PAD-OVERLAP"): materialize_hard_macros(source)
    assert not source.tracks and not source.vias
    asset["vias"]=[]
    asset["ports"][0]["point"]=[-500000,0]
    with pytest.raises(ValueError,match="no owner copper contact"): materialize_hard_macros(bind())


def test_internal_net_without_a_port_still_requires_complete_copper(tmp_path):
    _, asset, bind = fixture(tmp_path)
    asset["ports"] = []
    assert materialize_hard_macros(bind()).tracks
    asset["tracks"][0]["points"][-1] = [500000, 0]
    with pytest.raises(ValueError, match="internal net.*not connected"):
        materialize_hard_macros(bind())


def test_private_net_with_external_consumer_requires_a_boundary_port(tmp_path):
    original, asset, bind = fixture(tmp_path)
    asset["ports"] = []
    board = replace(original, placements=(*original.placements, Placement("J", "test", Point.mm(5, 5))),
        nets=(replace(original.nets[0], pads=(*original.nets[0].pads, PadReference("J", "1"))),))
    with pytest.raises(ValueError, match="requires an external port"):
        materialize_hard_macros(bind(input_board=board))


def test_ports_cannot_silently_omit_disconnected_bound_pads(tmp_path):
    _, asset, bind = fixture(tmp_path)
    asset["ports"][0]["pads"] = [["chip", "1"]]
    asset["tracks"][0]["points"][-1] = [500000, 0]
    with pytest.raises(ValueError, match="ports omit private pads"):
        materialize_hard_macros(bind())


def test_macro_filled_via_finish_requires_scoped_permission_and_containment(tmp_path):
    from pcbir.physical import CopperZone, PadViaInPadRule, PolygonRing, PolygonWithHoles, Stackup
    original, asset, _ = fixture(tmp_path)
    fp = replace(original.footprints["test"], pads=(FootprintPad("1", Point(0, 0), Size.mm(1, 1)),))
    for member in asset["members"]:
        member["footprint_digest"] = footprint_geometry_digest(fp)
    asset["vias"] = [dict(net="signal", position_nm=[0, 0], size_nm=300000,
        drill_nm=200000, from_layer="F.Cu", to_layer="B.Cu", technology=None, finish="filled-capped")]
    board = replace(original, footprints={fp.name: fp}, nets=(replace(original.nets[0], name="GND"),),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                        CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)),
        metadata={"fabrication_profile": "jlcpcb-six-layer"},
        zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(
            (Point.mm(1, 1), Point.mm(29, 1), Point.mm(29, 29), Point.mm(1, 29))))),))
    def bind_ground(source):
        path = tmp_path / "ground.json"
        path.write_text(json.dumps(asset))
        return bind_hard_macro(source, path, expected_sha256=sha256(path.read_bytes()).hexdigest(),
            name="ground", bindings={"chip": "U1", "passive": "R1"}, net_bindings={"signal": "GND"})
    with pytest.raises(ValueError, match="explicit permission"):
        materialize_hard_macros(bind_ground(board))
    qualified = replace(board, via_in_pad_rules=(PadViaInPadRule(PadReference("U1", "1")),))
    result = materialize_hard_macros(bind_ground(qualified))
    assert result.vias[0].finish == "filled-capped"
    from pcbir.hard_macros import macro_source
    assert materialize_hard_macros(macro_source(result)).vias == result.vias
    asset["vias"][0]["position_nm"] = [450000, 0]
    with pytest.raises(ValueError, match="contained"):
        materialize_hard_macros(bind_ground(qualified))
    asset["vias"][0]["finish"] = "unqualified"
    with pytest.raises(ValueError, match="unsupported via finish"):
        bind_ground(qualified)


def test_general_router_connects_only_external_port_and_preserves_owner(tmp_path):
    original,_,bind=fixture(tmp_path)
    board=bind(input_board=replace(original,
        placements=(*original.placements,Placement("J1","test",Point.mm(7,12))),
        nets=(replace(original.nets[0],pads=(*original.nets[0].pads,PadReference("J1","1"))),)))
    from pcbir.routing import route_global,GlobalRouterOptions
    from pcbir.detailed import route_detailed,DetailedRouterOptions
    from pcbir.drc import run_physical_drc,PhysicalDrcPolicy
    guides=route_global(board,GlobalRouterOptions(maximum_iterations=2,tile_size_nm=1000000))
    with pytest.raises(ValueError,match="materialize"): route_detailed(board,guides)
    board=materialize_hard_macros(board)
    routed=route_detailed(board,guides,DetailedRouterOptions(maximum_passes=1,maximum_search_states=20000))
    assert routed.nets[0].connected, routed.nets[0].diagnostics
    assert routed.board.tracks[:len(board.tracks)]==board.tracks
    assert routed.locked_track_count==len(board.tracks)
    assert not run_physical_drc(routed.board,policy=PhysicalDrcPolicy(False)).findings
    # A second pass doesn't build a parallel replacement network.
    again=route_detailed(routed.board,guides,DetailedRouterOptions(maximum_passes=1))
    assert again.board.tracks==routed.board.tracks
    assert again.metrics.track_count==0


def test_full_pipeline_preserves_macro_geometry(tmp_path):
    _,_,bind=fixture(tmp_path)
    from pcbir.flow import run_routing_pipeline
    result = run_routing_pipeline(bind())
    assert result.board.materialized_macros == ("unit",)
    assert len(result.board.tracks) == 1
    assert result.board.tracks[0].net == "N"


def test_macro_geometry_changes_invalidate_physical_digest(tmp_path):
    _,asset,bind=fixture(tmp_path)
    from pcbir.drc import physical_board_digest
    board=bind()
    asset["tracks"][0]["width_nm"]=210000
    changed=bind()
    assert physical_board_digest(board)!=physical_board_digest(changed)


def test_free_via_port_is_proved_and_exposed_as_routing_terminal(tmp_path):
    _,asset,bind=fixture(tmp_path)
    asset["tracks"].append(dict(net="signal",width_nm=200000,layer="F.Cu",
        points=[{"pad":["chip","1"]},[-1000000,0]]))
    asset["vias"]=[dict(net="signal",position_nm=[-1000000,0],size_nm=600000,drill_nm=300000,
                        from_layer="F.Cu",to_layer="B.Cu",technology=None)]
    asset["ports"][0]["point"]=[-1000000,0]
    asset["ports"][0]["layer"]="B.Cu"
    board=materialize_hard_macros(bind())
    from pcbir.hard_macros import macro_routing_ports, macro_port_layers
    pads = macro_routing_pads(board,board.nets[0])
    assert len(pads) == 1
    port = macro_routing_ports(board,"N")[pads[0]]
    assert port.position == Point.mm(9,10)
    assert macro_port_layers(board,port) == (CopperLayer.FRONT,CopperLayer.BACK)


@pytest.mark.parametrize("via_port", [False, True])
@pytest.mark.parametrize("rotation", [0,45])
def test_complete_pipeline_routes_external_free_ports_without_replacing_owner(tmp_path, via_port, rotation):
    original, asset, bind = fixture(tmp_path)
    asset["tracks"].append(dict(net="signal",width_nm=200000,layer="F.Cu",
        points=[{"pad":["chip","1"]},[-1000000,0]]))
    asset["ports"][0]["point"] = [-1000000,0]
    if via_port:
        asset["vias"] = [dict(net="signal",position_nm=[-1000000,0],size_nm=600000,
            drill_nm=300000,from_layer="F.Cu",to_layer="B.Cu",technology=None)]
        asset["ports"][0]["layer"] = "B.Cu"
    from pcbir.physical import ComponentPlacementRule
    input_board = replace(original,
        placements=(*original.placements, Placement("J1","test",Point.mm(7,12))),
        placement_rules=(ComponentPlacementRule("U1",allowed_orientations=(0,45),fixed_position=Point.mm(10,10),fixed_rotation_degrees=rotation),
                         ComponentPlacementRule("J1",fixed_position=Point.mm(7,12),fixed_rotation_degrees=0)),
        nets=(replace(original.nets[0],pads=(*original.nets[0].pads,PadReference("J1","1"))),))
    source = bind(input_board=input_board)
    poses = {p.reference:p for p in source.placements}
    poses.update(cluster_placements(source,source.rigid_clusters[0],replace(poses["U1"],rotation_degrees=rotation)))
    source = replace(source,placements=tuple(poses.values()))
    owner = materialize_hard_macros(source)
    from pcbir.flow import run_routing_pipeline
    from pcbir.fanout import FanoutOptions
    from pcbir.detailed import DetailedRouterOptions
    result = run_routing_pipeline(source, fanout_options=FanoutOptions(),
        detailed_options=DetailedRouterOptions(maximum_search_states=20000))
    assert result.status.value == "pass", result.detailed.nets
    assert result.board.tracks[:len(owner.tracks)] == owner.tracks
    assert result.board.vias[:len(owner.vias)] == owner.vias
    assert result.board.nets == source.nets


@pytest.mark.parametrize("limit,connected", [(3000000,True),(1000000,False)])
def test_single_ended_critical_owner_is_reused_and_budgeted(tmp_path,limit,connected):
    original,_,bind = fixture(tmp_path)
    from pcbir.physical import NetRoutingRule, RouteKind
    from pcbir.routing import route_global
    from pcbir.critical import route_critical_nets
    source = bind(input_board=replace(original,net_routing_rules=(NetRoutingRule(
        "N",RouteKind.CRITICAL,max_length_nm=limit),)))
    if not connected:
        with pytest.raises(ValueError,match="DRC-MAX-LENGTH"):
            route_global(source)
        assert not source.tracks
        return
    result = route_critical_nets(source,route_global(source))
    assert result.nets[0].connected == connected
    assert result.nets[0].strategy == "immutable_hard_macro"
    assert result.nets[0].lengths_nm == (2000000,)
    assert result.board.tracks == materialize_hard_macros(source).tracks


def test_plane_stitch_reuses_macro_return_without_shortcutting_private_pads(tmp_path):
    original, asset, bind = fixture(tmp_path)
    from pcbir.physical import CopperZone, PolygonRing, PolygonWithHoles, Stackup
    from pcbir.plane import stitch_zone_pads
    zone = CopperZone("plane","N",(CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(
        (Point.mm(1,1),Point.mm(29,1),Point.mm(29,29),Point.mm(1,29)))))
    source = replace(original,stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,
        CopperLayer.INTERNAL_2,CopperLayer.BACK)),zones=(zone,))
    without_contact = materialize_hard_macros(bind(input_board=source))
    pending = stitch_zone_pads(without_contact)
    assert pending.pending_pads == tuple(sorted(source.nets[0].pads))
    assert pending.board.tracks == without_contact.tracks and not pending.board.vias
    asset["tracks"].append(dict(net="signal",width_nm=200000,layer="F.Cu",
        points=[{"pad":["chip","1"]},[-1000000,0]]))
    asset["vias"] = [dict(net="signal",position_nm=[-1000000,0],size_nm=600000,
        drill_nm=300000,from_layer="F.Cu",to_layer="B.Cu",technology=None)]
    asset["ports"][0].update(point=[-1000000,0],layer="B.Cu")
    board = materialize_hard_macros(bind(input_board=source))
    result = stitch_zone_pads(board)
    assert result.complete and result.added_track_count == result.added_via_count == 0
    assert result.board.tracks == board.tracks and result.board.vias == board.vias
    assert not result.board.zone_fills  # A real contact is NOT filled-plane proof.


def test_planning_never_discards_arbitrary_existing_copper(tmp_path):
    _,_,bind = fixture(tmp_path)
    board = materialize_hard_macros(bind())
    extra = TrackSegment("N",Point.mm(10,10),Point.mm(9,10),200000,CopperLayer.FRONT)
    from pcbir.hard_macros import macro_source
    with pytest.raises(ValueError,match="no copper except immutable"):
        macro_source(replace(board,tracks=(*board.tracks,extra)))


def test_later_electrical_edits_cannot_reassign_private_pads(tmp_path):
    _,_,bind=fixture(tmp_path)
    board=materialize_hard_macros(bind())
    with pytest.raises(ValueError,match="pad/net bindings changed"):
        replace(board,nets=(PhysicalNet("N",(PadReference("U1","1"),)),
                            PhysicalNet("WRONG",(PadReference("R1","1"),))))


@pytest.mark.parametrize("outside_ref",["A_OUTSIDE","Z_OUTSIDE"])
def test_private_region_placement_is_independent_of_reference_order(tmp_path,outside_ref):
    original,_,bind=fixture(tmp_path)
    # Thin outsider between the chip and passive: it overlaps neither body,
    # but occupies the deliberately protected lead-in corridor.
    tiny=PhysicalFootprint("tiny",(),Size.mm(.2,.2))
    board=bind(input_board=replace(original,footprints={**original.footprints,"tiny":tiny},
        placements=(*original.placements,Placement(outside_ref,"tiny",Point.mm(11.5,10.4)))))
    from pcbir.placement import placement_solution_is_legal
    assert not placement_solution_is_legal(board,{p.reference:p for p in board.placements})


def test_existing_fixed_orientation_is_not_broadened_by_macro(tmp_path):
    original,_,bind=fixture(tmp_path)
    from pcbir.physical import ComponentPlacementRule
    from pcbir.placement import placement_solution_is_legal
    rule=ComponentPlacementRule("U1",allowed_orientations=(0,),fixed_position=Point.mm(10,10),fixed_rotation_degrees=0)
    board=bind(input_board=replace(original,placement_rules=(rule,)))
    poses={p.reference:p for p in board.placements}
    poses.update(cluster_placements(board,board.rigid_clusters[0],replace(poses["U1"],rotation_degrees=45)))
    assert not placement_solution_is_legal(board,poses)


@pytest.mark.parametrize("rotation",[0,45])
def test_actual_nordic_antenna_trial_has_no_opens_or_hard_findings(rotation,tmp_path):
    roots=(Path("C:/Program Files/KiCad/10.0/share/kicad/footprints"),Path(__file__).resolve().parents[2]/"CopperLib/footprints")
    if not all(p.is_dir() for p in roots): pytest.skip("optional installed-footprint/source-backed trial")
    from examples.nrf_antenna_macro.hard_macro_trial import make_trial,document
    from pcbir.drc import run_physical_drc,PhysicalDrcPolicy
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    board=make_trial(roots,rotation)
    assert {d["code"] for d in json.loads(board.metadata["erc_findings"])}=={"UNSOURCED_POWER_INPUT"}
    assert len(board.tracks)==20 and len(board.vias)==2
    assert not run_physical_drc(board,policy=PhysicalDrcPolicy(False)).findings
    assert json.loads(json.dumps(document(board)))["materialized_macros"]==["nordic-antenna-trial"]
    antenna=next(p for p in board.placements if p.reference=="ANT_BT")
    assert antenna.rotation_degrees==(90+rotation)%360
    assert all(PadReference("ANT_BT","2") not in n.pads for n in board.nets)
    cli=Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file(): return
    path=tmp_path/"probe.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board),path)
    assert path.read_text().count("(locked yes)")==29
    subprocess.run([str(cli),"pcb","drc","--format","json","-o",str(tmp_path/"drc.json"),str(path)],check=True,capture_output=True)
    report=json.loads((tmp_path/"drc.json").read_text())
    assert not report["violations"] and not report["unconnected_items"]


def test_vendor_extraction_is_deterministic_and_identity_pinned(monkeypatch):
    library=Path(__file__).resolve().parents[2]/"CopperLib"
    archive=library/"cache/rf-reference/nrf52832qfaxreflayoutv11.zip"
    if not archive.is_file(): pytest.skip("optional locally cached Nordic archive")
    monkeypatch.syspath_prepend(str(library/"packages/circuits/nordic/nrf52832-johanson-reference"))
    from generate_trial import generate,gerber_strokes
    raw=(json.dumps(generate(archive),indent=2,sort_keys=True)+"\n").encode()
    assert raw==(library/"packages/circuits/nordic/nrf52832-johanson-reference/assets/nrf52832-johanson-six-layer-trial.json").read_bytes()
    with pytest.raises(ValueError,match="coordinate format"): gerber_strokes("not Gerber")


def test_kicad_ground_refill_respects_matching_and_antenna_exclusions(tmp_path):
    native=Path("C:/Program Files/KiCad/10.0/bin/python.exe")
    cli=native.with_name("kicad-cli.exe")
    roots=(native.parents[1]/"share/kicad/footprints",Path(__file__).resolve().parents[2]/"CopperLib/footprints")
    if not native.is_file() or not cli.is_file() or not all(p.is_dir() for p in roots):
        pytest.skip("optional independent native KiCad refill check")
    from examples.nrf_antenna_macro.hard_macro_trial import make_trial
    from pcbir.physical import CopperZone,PolygonRing,PolygonWithHoles,ZoneConnection
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    board=make_trial(roots)
    outline=PolygonWithHoles(PolygonRing((Point.mm(.5,.5),Point.mm(49.5,.5),Point.mm(49.5,39.5),Point.mm(.5,39.5))))
    board=replace(board,zones=(CopperZone("refill-test","GND",board.stackup.copper_layers,outline,
                                         pad_connection=ZoneConnection.SOLID),))
    path=tmp_path/"filled.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board),path)
    subprocess.run([str(cli),"pcb","drc","--refill-zones","--save-board","--format","json",
                    "-o",str(tmp_path/"drc.json"),str(path)],check=True,capture_output=True)
    report=json.loads((tmp_path/"drc.json").read_text())
    assert not report["violations"] and not report["unconnected_items"]
    # Query actual filled polygons, not merely intent/keepout outlines. Source
    # probe is saved only in pytest's owned temp directory; never refill a live run.
    code="""import pcbnew,sys
b=pcbnew.LoadBoard(sys.argv[1])
zones=[z for z in b.Zones() if not z.GetIsRuleArea()]
def filled(layer,x,y):
    return any(z.HitTestFilledArea(layer,pcbnew.VECTOR2I(round(x*1e6),round(y*1e6))) for z in zones)
assert filled(pcbnew.F_Cu,10,10), 'positive control: top GND fill is absent'
for layer in (pcbnew.F_Cu,pcbnew.In1_Cu,pcbnew.B_Cu):
    assert not filled(layer,38.191,11.52), 'fill bypasses C3 return'
    assert not filled(layer,48.5,3.25), 'fill enters antenna corner'
assert sum(t.IsLocked() for t in b.GetTracks())==22
print('verified actual fill exclusions and immutable copper')
"""
    result=subprocess.run([str(native),"-c",code,str(path)],check=True,capture_output=True,text=True)
    assert "verified actual fill exclusions" in result.stdout


@pytest.mark.parametrize("invalid", [None, "geometry", "module"])
def test_pinned_legacy_module_asset_ids_bind_canonical_footprints(tmp_path, invalid):
    board, asset, bind = fixture(tmp_path)
    reference = "footprints/test.kicad_mod"
    old = replace(board.footprints["test"], name=reference, source_library_id=reference,
                  metadata={"resolved_reference": reference, "source_sha256": "pinned"})
    for row in asset["members"]:
        row["footprint"] = reference
        row["footprint_digest"] = footprint_geometry_digest(old)
    module = "github.com/vendor/library"
    identity = module + "/" + reference
    source_root = tmp_path / "other-module" if invalid == "module" else tmp_path
    current = replace(old, name=identity, source_library_id=identity,
                      metadata={**old.metadata, "resolved_reference": identity,
                                "resolution": "managed", "module_path": module,
                                "source_asset": identity,
                                "source_path": str(source_root / reference)})
    if invalid == "geometry":
        current = replace(current, body_size=Size.mm(1, 1))
    canonical = replace(board, footprints={identity: current},
                        placements=tuple(replace(p, footprint=identity) for p in board.placements))
    if invalid:
        with pytest.raises(ValueError, match="footprint identity mismatch"):
            bind(input_board=canonical)
    else:
        result = bind(input_board=canonical)
        assert all(m.footprint == identity for m in result.rigid_clusters[0].members)
        assert materialize_hard_macros(result).tracks

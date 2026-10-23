"""Proximity release must not sacrifice a still-legal complete package exit."""
from dataclasses import replace
from collections import Counter
from pcbir import CopperLayer, Point, TrackSegment, Via, nm_from_mm, route_global
from pcbir.physical import NetRoutingRule, RouteKind, CopperKeepout, PolygonWithHoles, PolygonRing
from pcbir.flow import _copper_keepouts, _revalidate_released_escapes
from pcbir.critical import route_critical_nets
from pcbir.routing_clearance import RoutingClearanceIndex
from test_unified_router import _board


def fixture(block_stem=False):
    board=_board({'S':((10,7),(10,17)),'C':((1,9),(19,9))})
    board=replace(board,net_routing_rules=(NetRoutingRule('C',RouteKind.CLOCK,
        width_nm=nm_from_mm('0.2'),allowed_layers=(CopperLayer.FRONT,),max_vias=0),))
    stem=TrackSegment('S',Point.mm(13,10),Point.mm(13,12),nm_from_mm('0.2'),CopperLayer.FRONT)
    via=Via('S',Point.mm(13,12),nm_from_mm('0.6'),nm_from_mm('0.3'))
    if block_stem:
        poly=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
            ((12,10.8),(14,10.8),(14,11.2),(12,11.2)))))
        board=replace(board,copper_keepouts=(CopperKeepout('real-blocker',(CopperLayer.FRONT,),poly),))
    keepouts=_copper_keepouts(board,(stem,),(via,),'escape-reserve',fixed=board)
    return board,((stem,),(via,),keepouts),((),(),(),keepouts,())


def test_collateral_release_restores_complete_exit_and_deflects_clock():
    board,reserved,held=fixture()
    kept,updated,report=_revalidate_released_escapes(board,reserved,held)
    assert kept[2] == reserved[2] and not updated[3]
    assert report['restored_nets'] == ['S']
    assert report['restored_keepouts'] == len(reserved[2])
    guides=route_global(board)
    for protect in (False,True):
        source=replace(board,copper_keepouts=kept[2] if protect else ())
        routed=route_critical_nets(source,guides)
        assert all(n.connected for n in routed.nets)
        # Temporary masks are planning-only; query actual routed clock copper.
        actual=replace(routed.board,copper_keepouts=())
        index=RoutingClearanceIndex(actual)
        stem,via=reserved[0][0],reserved[1][0]
        legal=index.can_track(stem.net,stem.start,stem.end,stem.width_nm,stem.layer) and index.can_via(
            via.net,via.position,via.size_nm,via.from_layer,via.to_layer,drill_nm=via.drill_nm)
        assert legal is protect
    assert board.tracks == board.vias == ()


def test_legal_via_cannot_restore_an_exit_with_a_blocked_stem():
    board,reserved,held=fixture(block_stem=True)
    kept,updated,report=_revalidate_released_escapes(board,reserved,held)
    assert not kept[2] and report['restored_keepouts'] == 0
    assert report['unrestored_nets'] == ['S']
    assert updated is held and board.tracks == board.vias == ()


def bypass_fixture():
    from pcbir import PhysicalNet, Placement, PadReference
    from pcbir.decoupling import route_decouplers
    from test_decoupling_crosslayer_paths import fixture as crosslayer
    from test_unified_router import _footprint
    board,_=crosslayer()
    fp=_footprint()
    board=replace(board,footprints={**board.footprints,fp.name:fp},
        placements=(*board.placements,Placement('S0',fp.name,Point.mm(3,3)),Placement('S1',fp.name,Point.mm(20,3))),
        nets=(*board.nets,PhysicalNet('S',(PadReference('S0','1'),PadReference('S1','1')))))
    bypass=route_decouplers(board)
    assert bypass.admitted_path_witnesses
    stem=TrackSegment('S',Point.mm(3,3),Point.mm(3,5),150000,CopperLayer.FRONT)
    via=Via('S',Point.mm(3,5),450000,200000)
    masks=_copper_keepouts(board,(stem,),(via,),'escape-reserve',fixed=board)
    held=(bypass.created_tracks,bypass.created_vias,(),masks,bypass.admitted_path_witnesses)
    return board,((stem,),(via,),masks),held


def test_restored_context_requires_a_new_fresh_bypass_ledger():
    from pcbir.decoupling_paths import validate_crosslayer_witness
    board,reserved,held=bypass_fixture()
    kept,updated,report=_revalidate_released_escapes(board,reserved,held)
    assert report['outcome'] == 'restored'
    candidate=replace(board,tracks=held[0],vias=held[1],copper_keepouts=kept[2])
    assert validate_crosslayer_witness(candidate,held[4][0])
    assert not validate_crosslayer_witness(candidate,updated[4][0])
    assert held[4][0].context_digest != updated[4][0].context_digest
    assert updated[:3] == held[:3]


def test_failed_bypass_revalidation_rolls_back_masks_and_original_ledger(monkeypatch):
    import pcbir.decoupling_paths as paths
    board,reserved,held=bypass_fixture()
    def failed(candidate,*args,**kwargs):
        return paths.DecouplingPathResult(candidate,reason='injected verification failure')
    monkeypatch.setattr(paths,'verify_crosslayer_decoupling',failed)
    kept,updated,report=_revalidate_released_escapes(board,reserved,held)
    assert not kept[2] and updated is held
    assert report['outcome'] == 'bypass_revalidation_failed'
    assert report['restored_keepouts'] == 0
    assert board.tracks == board.vias == ()

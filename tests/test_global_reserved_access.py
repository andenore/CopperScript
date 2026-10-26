"""Fresh global guides can enter through already verified package copper."""
from dataclasses import replace
import pytest
from test_fanout_and_plane_verify import _dense_board
from pcbir.fanout import route_fanout
from pcbir.routing import route_global, GlobalRouterOptions
from pcbir.physical import PadReference, PhysicalNet, Point, nm_from_mm


def test_global_guide_uses_actual_owned_launch_without_proposing_a_duplicate_via():
    original=_dense_board()
    fanout=route_fanout(original)
    pad=PadReference('U1','1')
    before=(fanout.board.tracks,fanout.board.vias)
    result=route_global(original,GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),fixed_prefix=fanout.board,
                        reserved_accesses=fanout.routing_accesses)
    route=next(r for r in result.routes if r.net=='SIGNAL')
    assert route.connected
    access=next(a for a in route.accesses if a.pad==pad)
    assert access.retained_via==fanout.board.vias[0] and access.via is None
    assert all(t in access.tracks for t in fanout.created_tracks)
    assert not any(v.position==access.retained_via.position for v in route.vias)
    assert (fanout.board.tracks,fanout.board.vias)==before
    assert result.fixed_prefix_digest
    assert result==route_global(original,GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),fixed_prefix=fanout.board,
                               reserved_accesses=fanout.routing_accesses)


@pytest.mark.parametrize('damage',('missing_track','wrong_position','foreign_via'))
def test_invalid_reserved_launch_fails_closed_without_substituting_a_new_exit(damage):
    fanout=route_fanout(_dense_board())
    anchors=dict(fanout.routing_accesses)
    board=fanout.board
    if damage=='missing_track':board=replace(board,tracks=())
    if damage=='wrong_position':anchors[PadReference('U1','1')]=Point.mm(6,8)
    if damage=='foreign_via':board=replace(board,nets=(*board.nets,PhysicalNet('OTHER',())),
        vias=tuple(replace(v,net='OTHER') for v in board.vias))
    source=replace(_dense_board(),nets=board.nets)
    if damage=='foreign_via':
        with pytest.raises(ValueError,match='fixed global prefix fails fresh physical DRC'):
            route_global(source,fixed_prefix=board,reserved_accesses=anchors)
        return
    route=route_global(source,GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),fixed_prefix=board,
                       reserved_accesses=anchors).routes[0]
    assert not route.connected and 'no legal bounded' in route.diagnostics[0]


def test_fixed_prefix_cannot_change_placement_or_source_copper():
    source=_dense_board()
    fanout=route_fanout(source)
    shifted=replace(fanout.board,placements=(replace(source.placements[0],position=Point.mm(9,10)),source.placements[1]))
    with pytest.raises(ValueError,match='changes source contracts'):
        route_global(source,fixed_prefix=shifted,reserved_accesses=fanout.routing_accesses)
    with pytest.raises(ValueError,match='require an explicitly fixed'):
        route_global(source,reserved_accesses=fanout.routing_accesses)


def test_fixed_global_prefix_preserves_immutable_macro_tracks(tmp_path):
    from test_hard_macros import fixture
    from pcbir.hard_macros import materialize_hard_macros
    source=fixture(tmp_path)[2]()
    held=materialize_hard_macros(source)
    before=held.tracks
    result=route_global(source,fixed_prefix=held)
    assert result.routes[0].connected and held.tracks==before
    # The immutable board type may reject tampering before planner admission.
    with pytest.raises(ValueError,match='immutable'):
        changed=replace(held,tracks=(replace(held.tracks[0],width_nm=100_000),))
        route_global(source,fixed_prefix=changed)


def test_fixed_global_prefix_cannot_drop_required_array_multiplicity():
    from test_pad_via_arrays import _array_board
    from pcbir.hard_macros import materialize_hard_macros
    source=_array_board(rows=3,columns=3,pitch_nm=750_000)
    held=materialize_hard_macros(source)
    assert len(held.vias)==9
    with pytest.raises(ValueError,match='immutable copper or required arrays'):
        route_global(source,fixed_prefix=replace(held,vias=held.vias[:-1]))
    assert len(held.vias)==9 and not source.vias


def test_reserved_port_tries_checked_corner_before_a_bounded_maze(monkeypatch):
    from test_detailed_routing import _board
    from pcbir import routing
    from pcbir.global_reserved_access import reserved_pin_access_candidates
    import pcbir.global_reserved_access as reserved
    from pcbir.physical import TrackSegment, Via, CopperLayer
    from pcbir.routing_clearance import RoutingClearanceIndex
    source = _board()
    launch = Point.mm(4, 6)
    track = TrackSegment("SIGNAL", Point.mm(3, 6), launch, 250_000, CopperLayer.FRONT)
    via = Via("SIGNAL", launch, 600_000, 300_000)
    held = replace(source, tracks=(track,), vias=(via,))
    a, b = routing.GridNode(0, 0, 0), routing.GridNode(0, 1, 0)
    graph = routing._Graph((CopperLayer.FRONT,), (6_000_000, 8_000_000), (7_000_000,),
        frozenset((a, b)), {routing._edge_key(a, b):
            routing._Resource("edge", a, b, 1, CopperLayer.FRONT)}, {}, {}, {})

    def no_maze(*args, **kwargs):
        raise AssertionError("an exact checked corner must avoid the bounded maze")

    monkeypatch.setattr(reserved, "local_access_path", no_maze)
    clear = RoutingClearanceIndex(held)
    choices = reserved_pin_access_candidates(held, graph, PadReference("J1", "1"), "SIGNAL",
        (CopperLayer.FRONT,), None, clear, GlobalRouterOptions(), launch)
    assert choices and all(not choice.region_only and choice.retained_via == via for choice in choices)
    assert all(choice.tracks[0] == track for choice in choices)
    assert all(clear.can_route(t.net, t.start, t.end, t.width_nm, t.layer)
               for choice in choices for t in choice.tracks)
    assert held.tracks == (track,) and held.vias == (via,)

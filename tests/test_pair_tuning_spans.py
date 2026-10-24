"""Bounded candidate-span tuning with real copper and ownership.

No global/detailed/pair search. All proposals use actual typed copper and the
existing tuner/clearance/profile checks; only call-order spies are patched.
"""
from dataclasses import replace
import pytest
from pcbir.physical import (BoardOutline, CopperLayer, DesignRules, FootprintPad,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, RouteKind, Size, TrackSegment, Via)
from pcbir.breakout import BreakoutRegions
from pcbir.critical import _route_pair, _track_length, _tune_pair
from pcbir.pair_tuning_spans import tune_virtual_pair_spans
from pcbir.drc import run_physical_drc, explicit_copper_connectivity
from pcbir.routing import GlobalNetRoute, GridNode, PinAccess

F = CopperLayer.FRONT
W, GAP, SKEW, AMP = 145000, 150000, 63500, 1000000


def fixture(*, length=10000000, neck=120000, spacing=W+GAP):
    fp1 = PhysicalFootprint("start",(
        FootprintPad("1",Point(0,0),Size(W,W)),
        FootprintPad("2",Point(0,spacing),Size(W,W))),Size.mm(1,1))
    fp2 = PhysicalFootprint("end",(
        FootprintPad("1",Point(0,0),Size(W,W)),
        FootprintPad("2",Point(451703,spacing),Size(W,W))),Size.mm(1,1))
    rules = tuple(NetRoutingRule(n,RouteKind.DIFFERENTIAL,width_nm=W,clearance_nm=290000,
        differential_partner=p,pair_gap_nm=GAP,max_skew_nm=SKEW,max_vias=2,
        max_length_nm=63600000,tuning_amplitude_limit_nm=AMP,
        breakout_length_nm=2000000,breakout_width_nm=neck,breakout_gap_nm=100000,
        breakout_clearance_nm=100000) for n,p in (("A","B"),("B","A")))
    board = PhysicalBoard("SpanFixture",BoardOutline.rectangle(30,15),
        {fp1.name:fp1,fp2.name:fp2},
        (Placement("J1",fp1.name,Point.mm(5,5)),
         Placement("J2",fp2.name,Point(5000000+length,5000000))),
        (PhysicalNet("A",(PadReference("J1","1"),PadReference("J2","1"))),
         PhysicalNet("B",(PadReference("J1","2"),PadReference("J2","2"))),
         PhysicalNet("OBSTACLE",())),
        rules=DesignRules(minimum_clearance_nm=100000,minimum_track_width_nm=100000,
                          default_track_width_nm=200000),net_routing_rules=rules)
    lanes = ((TrackSegment("A",Point.mm(5,5),Point(5000000+length,5000000),W,F),),
             (TrackSegment("B",Point(5000000,5000000+spacing),
                           Point(5451703+length,5000000+spacing),W,F),))
    regions = BreakoutRegions(board)
    return board, tuple(regions.split_tracks(lane,W) for lane in lanes)


def fragment(lane):
    out=[]
    for t in lane:
        marks=(*range(t.start.x_nm,t.end.x_nm,500000),t.end.x_nm)
        out.extend(replace(t,start=Point(a,t.start.y_nm),end=Point(b,t.end.y_nm))
                   for a,b in zip(marks,marks[1:]))
    return out


def attempt(board, lanes, *, held=None, protected_nodes=(), proposed_vias=(), amplitude=AMP):
    first,second = map(list,lanes)
    # Production fallback condition: accepted legacy geometry is never changed.
    if not _tune_pair(first,second,SKEW,amplitude,32):
        added=tune_virtual_pair_spans(first,second,SKEW,amplitude,32,
            source=board,held=held if held is not None else board,
            protected_nodes=protected_nodes,proposed_vias=proposed_vias,tuner=_tune_pair)
    else:
        added=_track_length(tuple(first))-_track_length(tuple(lanes[0]))
    return added,first,second


def assert_unchanged(before, after):
    assert len(before)==len(after)
    assert all(a is b for a,b in zip(before,after))


def guides(board):
    # Deliberately NOT successful global routing certificates. Exact-construction
    # fixtures use real source terminal identities without invoking a global solve.
    from pcbir.placement import transformed_pad_position
    poses={p.reference:p for p in board.placements}
    return {net.name:GlobalNetRoute(net.name,False,tuple(
        PinAccess(p,(q:=transformed_pad_position(board,poses[p.component],p.pad)),
                  GridNode(0,i,0),q,F) for i,p in enumerate(net.pads)),(),(),0,
        ("exact-construction fixture; no global route claimed",))
        for net in board.nets if net.name in {"A","B"}}


def test_fragmented_source_breakout_lanes_compensate_without_losing_anchors():
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    before=tuple(tuple(x) for x in parts)
    assert _tune_pair(list(parts[0]),list(parts[1]),SKEW,AMP,32)==0
    added,a,b=attempt(board,parts)
    assert added>0 and abs(_track_length(tuple(a))-_track_length(tuple(b)))<=SKEW
    assert a[0].start==parts[0][0].start and a[-1].end==parts[0][-1].end
    assert_unchanged(before[1],b)
    checked=replace(board,tracks=(*a,*b))
    assert all(explicit_copper_connectivity(checked,include_internal_connections=False).net_connected(n)
               for n in board.nets if n.name in {"A","B"})
    # Exact lane connectivity was checked above. The source scene has not
    # passed a complete routing pipeline, so its bookkeeping stays incomplete.
    assert [f.code for f in run_physical_drc(checked).findings
            if f.severity.value=="error"]==["DRC-ROUTE-INCOMPLETE"]
    # Emitted regions are still cut and carry their exact source width.
    regions=BreakoutRegions(board)
    assert regions.split_tracks(a,W)==tuple(a)
    assert len(board.tracks)==len(board.vias)==0


def test_exact_constructor_uses_fallback_only_with_full_held_context():
    board,lanes=fixture();parts=tuple(tuple(fragment(x)) for x in lanes)
    rs=board.net_routing_rules;g=guides(board)
    legacy=_route_pair(board,*rs,g,exact_tracks=parts)
    fixed=_route_pair(board,*rs,g,exact_tracks=parts,tuning_board=board)
    # Untuned, the pair connects over its skew budget (a warning); the full
    # held context lets the span fallback tune it within the budget.
    assert legacy[0].connected and legacy[0].skew_nm==451703 and legacy[0].warnings
    assert fixed[0].connected and fixed[0].skew_nm<=SKEW and not fixed[0].warnings
    assert fixed[0].tuned_length_nm>0


def test_legacy_success_never_enters_span_fallback(monkeypatch):
    board,lanes=fixture();g=guides(board)
    baseline=_route_pair(board,*board.net_routing_rules,g,exact_tracks=lanes)
    assert baseline[0].connected and baseline[0].tuned_length_nm>0
    import pcbir.pair_tuning_spans as spans
    def unexpected(*args,**kwargs):
        raise AssertionError("accepted legacy geometry must not reach fallback")
    monkeypatch.setattr(spans,"tune_virtual_pair_spans",unexpected)
    assert _route_pair(board,*board.net_routing_rules,g,exact_tracks=lanes,
                       tuning_board=board)==baseline


@pytest.mark.parametrize("where",["source","held","source_reverse"])
def test_source_equal_subdivisions_do_not_acquire_disposable_ownership(where):
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    # Exact geometry covers all shorter-lane pieces; clones do not authorize it.
    held_edge=TrackSegment("A",parts[0][0].start,parts[0][-1].end,W,F)
    if where=="source_reverse":held_edge=replace(held_edge,start=held_edge.end,end=held_edge.start)
    source=replace(board,tracks=(held_edge,)) if where!="held" else board
    held=source if where!="held" else replace(board,tracks=(held_edge,))
    added,a,b=attempt(source,parts,held=held)
    assert added==0;assert_unchanged(parts[0],a);assert_unchanged(parts[1],b)
    assert held.tracks==(held_edge,) and held.tracks[0] is held_edge


def test_branch_is_unsupported_and_atomic():
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    parts[0].append(TrackSegment("A",Point.mm(10,5),Point.mm(10,4.9),120000,F))
    before=tuple(parts[0]);added,a,b=attempt(board,parts)
    assert added==0;assert_unchanged(before,a);assert_unchanged(parts[1],b)


def test_explicit_contact_and_via_nodes_split_otherwise_virtual_runs():
    board,lanes=fixture(length=3000000,spacing=600000);parts=tuple(fragment(x) for x in lanes)
    # Two genuine transitions exhaust the source max_vias2; no new allowance.
    vias=tuple(Via("A",Point(x,5000000),450000,200000,F,CopperLayer.BACK)
               for x in (6000000,7000000))
    added,a,b=attempt(board,parts,proposed_vias=vias)
    assert added==0;assert_unchanged(parts[0],a);assert_unchanged(parts[1],b)
    nodes=tuple(("A",F,t.end) for t in parts[0][:-1])
    added,a,b=attempt(board,parts,protected_nodes=nodes)
    assert added==0;assert_unchanged(parts[0],a);assert_unchanged(parts[1],b)


def test_equal_width_does_not_authorize_crossing_source_region_boundaries():
    board,lanes=fixture(neck=W);parts=tuple(fragment(x) for x in lanes)
    captured=[]
    def observer(a,b,*args):
        captured.extend(a)
        return _tune_pair(a,b,*args)
    a,b=map(list,parts)
    tune_virtual_pair_spans(a,b,SKEW,AMP,32,source=board,held=board,tuner=observer)
    regions=BreakoutRegions(board)
    assert captured
    # Every virtual host remains one original source region, even if both
    # neighboring regions use exactly the same width.
    for host in captured:
        owned=[t for t in parts[0] if t.start.y_nm==host.start.y_nm
               and host.start.x_nm<=t.start.x_nm<t.end.x_nm<=host.end.x_nm]
        if owned:
            assert len({regions.region(t.net,(t.start,t.end)) for t in owned})==1


@pytest.mark.parametrize("reason",["amplitude","stale_intent","lost_prefix","blocked"])
def test_failed_or_blocked_compensation_preserves_exact_occurrences(reason):
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes);source=board;held=board
    amplitude=0 if reason=="amplitude" else AMP
    if reason=="stale_intent":held=replace(board,metadata={"stale":"context"})
    if reason=="lost_prefix":
        # Required occurrence multiplicity cannot be projected out of context.
        edge=TrackSegment("OBSTACLE",Point.mm(20,12),Point.mm(21,12),100000,F)
        source=replace(board,tracks=(edge,edge));held=replace(source,tracks=(edge,))
    if reason=="blocked":
        wall=TrackSegment("OBSTACLE",Point.mm(13,4.75),Point.mm(14.8,4.75),100000,F)
        held=replace(board,tracks=(wall,))
    added,a,b=attempt(source,parts,held=held,amplitude=amplitude)
    assert added==0;assert_unchanged(parts[0],a);assert_unchanged(parts[1],b)


def test_renaming_and_reversal_preserve_success_and_determinism():
    board,lanes=fixture();parts=tuple(fragment(x) for x in lanes)
    first=attempt(board,parts);again=attempt(board,parts)
    assert first==again and first[0]>0
    reversed_parts=tuple([replace(t,start=t.end,end=t.start) for t in reversed(x)] for x in parts)
    reverse=attempt(board,reversed_parts)
    assert reverse[0]>0 and abs(_track_length(tuple(reverse[1]))-_track_length(tuple(reverse[2])))<=SKEW
    names={"A":"RENAMED_SIGNAL","B":"RENAMED_PARTNER","OBSTACLE":"OTHER"}
    renamed=replace(board,nets=tuple(replace(n,name=names[n.name]) for n in board.nets),
        net_routing_rules=tuple(replace(r,net=names[r.net],differential_partner=names[r.differential_partner])
                                for r in board.net_routing_rules))
    renamed_parts=tuple([replace(t,net=names[t.net]) for t in x] for x in parts)
    other=attempt(renamed,renamed_parts)
    assert other[0]==first[0]


@pytest.mark.parametrize("layer", [CopperLayer.FRONT, CopperLayer.BACK])
def test_unordered_edges_ending_at_previous_start_are_not_merged(layer):
    board,_ = fixture()
    board=replace(board,placements=(),nets=(),net_routing_rules=())
    # A legal corner supplied in reverse list order. The second edge ends at
    # the first edge's start, but is not a forward continuation of that edge.
    first=[TrackSegment("A",Point.mm(10,5),Point.mm(11,5),W,layer),
           TrackSegment("A",Point.mm(10,4),Point.mm(10,5),W,layer)]
    second=[TrackSegment("B",Point.mm(5,10),Point.mm(20,10),W,layer)]
    before=tuple(first),tuple(second)
    def unexpected(*args):
        raise AssertionError("no collinear span exists to tune")
    assert tune_virtual_pair_spans(first,second,SKEW,AMP,32,
        source=board,held=board,tuner=unexpected)==0
    assert_unchanged(before[0],first)
    assert_unchanged(before[1],second)

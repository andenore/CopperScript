"""Support trees remain obligations while the pair/return proposal is mutable."""
from dataclasses import replace
import pytest

from pcbir import route_global, route_critical_nets, CopperLayer
from pcbir.physical import Point, TrackSegment, PadReference, PhysicalFootprint, FootprintPad, Size, Placement
from pcbir.pair_terminals import plan_pair_supports, PairSupportCompletion, PairTerminalPreparationStats
from pcbir.drc import explicit_copper_connectivity
from test_local_pair_branches import support_fixture
from test_pair_return_boundaries import fixture, reserved
from pcbir.pair_vias import _return_via


def test_pending_guides_are_not_connectivity_and_failure_keeps_every_input_occurrence():
    board=support_fixture()
    prefix=TrackSegment(board.nets[0].name,Point.mm(3,3),Point.mm(4,3),100000,CopperLayer.FRONT)
    # Only call the local planner with this immutable context; critical's public
    # selected replay deliberately rejects arbitrary input area copper.
    held=replace(board,tracks=(prefix,prefix))
    plan=plan_pair_supports(held,*board.net_routing_rules,{})
    assert plan is not None and all(not g.connected for g in plan.guides)
    assert all(len(g.accesses)==2 for g in plan.guides)
    # No inferred copper may connect the two pending supports by a label.
    graph=explicit_copper_connectivity(held)
    assert not all(graph.net_connected(n) for n in board.nets)
    assert held.tracks==(prefix,prefix) and board.tracks==()


@pytest.mark.parametrize('failure',('remote','ambiguous','missing_contract'))
def test_joint_support_plan_rejects_unsupported_contact_obligations(failure):
    board=support_fixture()
    if failure=='remote':
        board=replace(board,placements=tuple(replace(p,position=Point.mm(20,4))
            if p.reference=='SUP_P' else p for p in board.placements))
    else:
        board=replace(board,net_routing_rules=tuple(replace(r,
            breakout_length_nm=40000000 if failure=='ambiguous' else None,
            breakout_width_nm=r.breakout_width_nm if failure=='ambiguous' else None) for r in board.net_routing_rules))
    assert plan_pair_supports(board,*board.net_routing_rules,{}) is None


def test_return_proposal_can_change_when_support_completion_rejects_first_legal_site():
    board,rules,pair,port=fixture()
    seen=[]
    def finish(returns):
        seen.append(returns)
        return len(seen)>1
    index=reserved(board,pair)
    result=_return_via(board,index,port,pair,*rules,CopperLayer.FRONT,CopperLayer.BACK,completion=finish)
    assert result is not None and len(seen)==2 and seen[0]!=result
    assert index.candidate_vias_clear((*pair,*result))
    assert board.tracks==board.vias==()


def test_support_rejection_never_relaxes_return_or_keeps_tentative_vias():
    board,rules,pair,port=fixture()
    assert _return_via(board,reserved(board,pair),port,pair,*rules,
        CopperLayer.FRONT,CopperLayer.BACK,completion=lambda _:False) is None
    assert board.tracks==board.vias==()


def test_failed_pair_keeps_pending_supports_and_rolls_back_whole_proposal():
    board=support_fixture()
    result=route_critical_nets(board,route_global(board),pair_state_limit=1)
    assert not result.nets[0].connected
    assert result.board.tracks==board.tracks and result.board.vias==board.vias
    assert set(result.nets[0].local_preparation.pending_contacts)=={
        PadReference('SUP_P','1'),PadReference('SUP_N','1')}


@pytest.mark.parametrize('return_distance', (1500000, 100000))
def test_joint_supports_cross_layers_with_real_returns_or_roll_back(return_distance):
    from test_pair_vias import board_fixture
    from pcbir.drc import run_physical_drc, PhysicalDrcPolicy
    board=board_fixture(True)
    support=PhysicalFootprint('support',(FootprintPad('1',Point(0,0),Size.mm(.4,.4)),),Size.mm(1,1))
    board=replace(board,footprints={**board.footprints,'support':support},
        placements=(*board.placements,Placement('SUP_A','support',Point.mm(6.4,10)),
                    Placement('SUP_B','support',Point.mm(6.4,14))),
        nets=tuple(replace(n,pads=(*n.pads,PadReference('SUP_'+n.name,'1')))
                   if n.name in {'A','B'} else n for n in board.nets),
        net_routing_rules=tuple(replace(r,breakout_length_nm=4000000,breakout_width_nm=250000,
            maximum_return_via_distance_nm=return_distance) for r in board.net_routing_rules))
    result=route_critical_nets(board,route_global(board),pair_state_limit=6000)
    group=result.nets[0]
    assert board.tracks==board.vias==()
    assert group.local_preparation.support_trials<=16
    assert group.local_preparation.support_expanded_states<=1200
    if return_distance==100000:
        assert not group.connected and result.board.tracks==result.board.vias==()
        assert len(group.local_preparation.pending_contacts)==2
    else:
        assert group.connected and group.paired_via_transitions==2 and group.return_via_count==2
        assert all(explicit_copper_connectivity(result.board).net_connected(n) for n in board.nets)
        assert not group.local_preparation.pending_contacts
        assert not [f for f in run_physical_drc(result.board,
            policy=PhysicalDrcPolicy(require_completed_detailed_route=False)).findings if f.severity.value=='error']


def test_repeated_local_work_requires_exact_source_copper_and_occurrence_context():
    board=support_fixture()
    plan=plan_pair_supports(board,*board.net_routing_rules,{})
    stats=PairTerminalPreparationStats()
    complete=PairSupportCompletion(plan,*board.net_routing_rules,stats,source=board)
    prefixes=tuple((TrackSegment(rule.net,anchor.pad_position,
        Point(anchor.pad_position.x_nm-1000000,anchor.pad_position.y_nm),
        rule.width_nm,CopperLayer.FRONT),)
        for rule,(anchor,_) in zip(board.net_routing_rules,plan.contacts))
    held=replace(board,tracks=(*prefixes[0],*prefixes[1]))
    component=plan.contacts[0][0].pad.component
    first=complete(component,held,*prefixes,'surface')
    assert first is not None
    count=stats.support_trials
    assert complete(component,held,*prefixes,'surface')==first
    assert stats.support_trials==count
    changed=replace(held,rules=replace(held.rules,minimum_clearance_nm=held.rules.minimum_clearance_nm+1000))
    assert complete(component,changed,*prefixes,'surface') is None
    # An added occurrence is a distinct proposal, not a reusable set key.
    repeated=replace(held,tracks=(*held.tracks,held.tracks[0]))
    complete(component,repeated,*prefixes,'surface')
    assert stats.support_trials==count+1
    assert stats.support_expanded_states<=1200 and len(complete.completed)<=16

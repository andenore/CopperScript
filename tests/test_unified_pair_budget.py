from dataclasses import replace
import pytest
import pcbir.flow as flow
from pcbir import (BoardOutline, PhysicalBoard, PhysicalFootprint, FootprintPad,
    Placement, PhysicalNet, PadReference, Point, Size, PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions)
from pcbir.unified_router import UnifiedRouterOptions

def _board():
    fp=PhysicalFootprint("one", (FootprintPad("1",Point(0,0),Size.mm(.6,.6)),),Size.mm(1,1))
    return PhysicalBoard("budget",BoardOutline.rectangle(20,12),{"one":fp},
        (Placement("A","one",Point.mm(3,6)),Placement("B","one",Point.mm(17,6))),
        (PhysicalNet("S",(PadReference("A","1"),PadReference("B","1"))),))

# (--unified-set critical_pair_state_limit=N, the stage bound route-board passes from
# --package-initial-pair-states, the bound route_critical_nets receives).
@pytest.mark.parametrize("limit,stage,expected", (
    (None, 6000, 6000), (16000, 6000, 16000), (None, 1, 1), (None, 0, None)))
def test_pipeline_forwards_the_pair_cap(monkeypatch, limit, stage, expected):
    board=_board()
    original=flow.route_critical_nets
    seen=[]
    def observe(*args,**kwargs):
        seen.append(kwargs.get("pair_state_limit"))
        return original(*args,**kwargs)
    monkeypatch.setattr(flow,"route_critical_nets",observe)
    result=flow.run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(critical_pair_state_limit=limit),
        critical_pair_state_limit=stage)
    assert seen == [expected]
    assert all(n.connected for n in result.detailed.nets)


def test_negative_stage_cap_is_rejected():
    with pytest.raises(ValueError, match="critical pair state limit"):
        flow.run_routing_pipeline(_board(), critical_pair_state_limit=-1)



@pytest.mark.parametrize("limit", (0,-1,True,2.5))
def test_invalid_pair_caps_are_rejected(limit):
    with pytest.raises(ValueError,match="critical pair state limit"):
        UnifiedRouterOptions(critical_pair_state_limit=limit)

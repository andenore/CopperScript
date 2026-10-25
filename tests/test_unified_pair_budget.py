from dataclasses import replace
import pytest
import pcbir.flow as flow
from pcbir import (BoardOutline, PhysicalBoard, PhysicalFootprint, FootprintPad,
    Placement, PhysicalNet, PadReference, Point, Size, PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions)
from pcbir.unified_router import UnifiedRouterOptions
from pcbir.package_access import PackageAccessOptions

@pytest.mark.parametrize("limit", (None, 16000))
def test_pipeline_forwards_explicit_pair_cap_without_changing_default(monkeypatch, limit):
    fp=PhysicalFootprint("one", (FootprintPad("1",Point(0,0),Size.mm(.6,.6)),),Size.mm(1,1))
    board=PhysicalBoard("budget",BoardOutline.rectangle(20,12),{"one":fp},
        (Placement("A","one",Point.mm(3,6)),Placement("B","one",Point.mm(17,6))),
        (PhysicalNet("S",(PadReference("A","1"),PadReference("B","1"))),))
    original=flow.route_critical_nets
    seen=[]
    def observe(*args,**kwargs):
        seen.append(kwargs.get("pair_state_limit"))
        return original(*args,**kwargs)
    monkeypatch.setattr(flow,"route_critical_nets",observe)
    result=flow.run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,refinement_passes=0),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        unified_options=UnifiedRouterOptions(critical_pair_state_limit=limit))
    assert seen == [limit if limit is not None else PackageAccessOptions().initial_pair_state_limit]
    assert all(n.connected for n in result.detailed.nets)

@pytest.mark.parametrize("limit", (0,-1,True,2.5))
def test_invalid_pair_caps_are_rejected(limit):
    with pytest.raises(ValueError,match="critical pair state limit"):
        UnifiedRouterOptions(critical_pair_state_limit=limit)

"""Transition telemetry remains finite and cannot admit or alter copper."""
from pcbir.pair_vias import _TransitionTrace, _transitions
from pcbir import CopperLayer
from pcbir.physical import Point
from test_pair_vias import board_fixture


def test_transition_trace_preserves_candidate_inventory_and_source():
    board=board_fixture(True)
    args=(board,*board.net_routing_rules,Point.mm(5,11.5),Point.mm(5,12.5),'J1',
          CopperLayer.FRONT,CopperLayer.INTERNAL_2)
    trace=_TransitionTrace()
    actual=_transitions(*args,trace=trace)
    assert actual==_transitions(*args)
    assert trace.counts['short_surface_collars']>0
    assert trace.counts['middle_layer_collars']>0
    assert board.tracks==board.vias==()


def test_transition_rejection_samples_are_bounded_per_stage_and_in_total():
    trace=_TransitionTrace()
    for n in range(100):
        for stage in range(20):
            trace.record(str(stage),positions=(Point(n,stage),))
    assert len(trace.examples)==16
    assert all(sum(x.stage==s for x in trace.examples)<=2 for s in trace.counts)
    assert all(count==100 for count in trace.counts.values())

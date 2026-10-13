"""Source routing clearance must survive native zone refill, not only routing."""
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from pcbir.backends.routing_clearance import routing_clearance_rules


def board(*rules, minimum=90_000):
    return SimpleNamespace(rules=SimpleNamespace(minimum_clearance_nm=minimum),
                           net_routing_rules=rules)


def rule(net, clearance, breakout=None):
    return SimpleNamespace(net=net, clearance_nm=clearance,
                           breakout_clearance_nm=breakout)


def test_exact_rf_clearance_is_exported_in_fixed_point_units():
    lines = routing_clearance_rules(board(rule('CELL_RF', 201_000)))
    assert '(constraint clearance (min 0.201mm))' in lines[1]
    assert "A.NetName == 'CELL_RF' || B.NetName == 'CELL_RF'" in lines[2]


def test_stricter_matching_rule_is_last_independent_of_source_order():
    first, second = rule('Z', 250_000), rule('A', 201_000)
    lines = routing_clearance_rules(board(first, second))
    assert lines == routing_clearance_rules(board(second, first))
    assert '0.201mm' in lines[1] and '0.25mm' in lines[4]


def test_default_and_board_minimum_need_no_override():
    assert routing_clearance_rules(board(rule('A', None), rule('B', 75_000))) == ()


def test_breakout_export_does_not_claim_full_region_specific_rule():
    lines = routing_clearance_rules(board(rule('A', 250_000, 100_000)))
    assert '0.1mm' in lines[1]
    assert routing_clearance_rules(board(rule('A', 250_000, 75_000))) == ()


def test_backend_emits_dru_without_component_hole_exceptions():
    from pcbir import (BoardOutline, DesignRules, KiCadPcbBackend, PhysicalBoard,
                       PhysicalNet, NetRoutingRule)
    candidate = PhysicalBoard(name='RF', outline=BoardOutline.rectangle(10, 10),
        rules=DesignRules(minimum_clearance_nm=90_000, minimum_track_width_nm=90_000),
        footprints={}, placements=(), nets=(PhysicalNet('CELL_RF', ()),),
        net_routing_rules=(NetRoutingRule('CELL_RF', clearance_nm=201_000),))
    artifacts = {a.name:a.content for a in KiCadPcbBackend().generate(candidate).artifacts}
    assert '0.201mm' in artifacts['RF.kicad_dru']
    project = json.loads(artifacts['RF.kicad_pro'])
    assert project['board']['design_settings']['rules']['min_clearance'] == 0.09


@pytest.mark.skipif(not (os.environ.get('COPPERSCRIPT_KICAD_CLI') and
                        os.environ.get('COPPERSCRIPT_KICAD_PYTHON')),
                    reason='explicit KiCad CLI/Python paths required')
def test_native_refill_obeys_net_clearance_with_default_control(tmp_path):
    from pcbir import (BoardOutline, DesignRules, FootprintPad, KiCadPcbBackend,
                       PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                       Point, Size, PadReference, TrackSegment, CopperLayer,
                       CopperZone, PolygonRing, PolygonWithHoles, NetRoutingRule)
    from pcbir.physical import ZoneConnection
    from pcbir.plane_verify import verify_filled_planes
    from pcbir.rf_cross_section import sample_coplanar_cross_sections
    fp = PhysicalFootprint('land', (FootprintPad('1', Point(0,0), Size(142_000,142_000)),), Size.mm(1,1))
    outline = PolygonWithHoles(PolygonRing((Point.mm(0.5,0.5),Point.mm(9.5,0.5),
                                          Point.mm(9.5,9.5),Point.mm(0.5,9.5))))
    for name, clearance, ground_clearance, expected in (
            ('default',None,None,90_000), ('explicit',201_000,None,201_000),
            ('stricter_ground',201_000,250_000,250_000)):
        candidate = PhysicalBoard(name='RF', outline=BoardOutline.rectangle(10,10),
            rules=DesignRules(minimum_clearance_nm=90_000,minimum_track_width_nm=90_000),
            footprints={'land':fp}, placements=(Placement('G1','land',Point.mm(2,2)),
                Placement('R1','land',Point.mm(3,5)), Placement('R2','land',Point.mm(7,5))),
            nets=(PhysicalNet('RF',(PadReference('R1','1'),PadReference('R2','1'))),
                  PhysicalNet('GND',(PadReference('G1','1'),))),
            tracks=(TrackSegment('RF',Point.mm(3,5),Point.mm(7,5),142_000,CopperLayer.FRONT),),
            zones=(CopperZone('ground','GND',(CopperLayer.FRONT,),outline,
                             pad_connection=ZoneConnection.SOLID),),
            net_routing_rules=(NetRoutingRule('GND',clearance_nm=ground_clearance),
                               NetRoutingRule('RF',clearance_nm=clearance)))
        stage = tmp_path/name
        verify_filled_planes(candidate,kicad_cli=Path(os.environ['COPPERSCRIPT_KICAD_CLI']),
                             artifact_directory=stage)
        probe = Path(__file__).parents[1]/'pcbir/editor/native_overlay.py'
        result = subprocess.run([os.environ['COPPERSCRIPT_KICAD_PYTHON'],str(probe),str(stage/'RF.kicad_pcb')],
                                capture_output=True,text=True,check=True)
        native = json.loads(result.stdout)
        measured = sample_coplanar_cross_sections(native,identifier='RF',net='RF',ground_net='GND',
            signal_layer='F.Cu',expected_width_nm=142_000,expected_gap_nm=expected,
            gap_tolerance_nm=1000,sample_pitch_nm=100_000,probe_length_nm=1_000_000)
        assert measured['metrics']['geometry_matches_model_at_samples'], measured
        assert measured['status'] == 'incomplete'  # geometry is not RF signoff

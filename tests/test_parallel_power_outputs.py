# Explicit power-output ties do not merge package contacts or regulators.
from dataclasses import replace
import pytest
from pcbir import Board, ComponentInstance, Net, check, ep
from pcbir.compiler import _compile_part
from pcbir.parser import parse
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError
from pcbir.model import Direction, ElectricalProfile, PackagePinDefinition, SignalDomain


def part(group=True, directions='output', domains='power'):
    policy='parallel_power_group = "CORE";' if group else ''
    source='''part CoreSupply {
      pin OUT_A { number = "1"; domains = "DOMAINS"; directions = "DIRS"; POLICY }
      pin OUT_B { number = "2"; domains = "DOMAINS"; directions = "DIRS"; POLICY }
    }'''.replace('DOMAINS',domains).replace('DIRS',directions).replace('POLICY',policy)
    return _compile_part(parse(source), 'CoreSupply')


def fixture(p, endpoints, references=('U1',)):
    return Board('TiedCore', {'CoreSupply':p},
        tuple(ComponentInstance(ref,'CoreSupply') for ref in references),
        (Net('CORE',tuple(ep(e) for e in endpoints)),))


def conflicts(board):
    return [d for d in check(board) if d.code=='OUTPUT_CONFLICT']


def test_explicit_same_component_power_group_accepts_external_tie_and_serializes():
    p=part()
    board=fixture(p,('U1.OUT_A','U1.OUT_B'))
    assert not conflicts(board)
    assert p.internal_pad_groups==()
    data=board_to_dict(board)
    assert all(pin['parallel_power_group']=='CORE' for pin in data['parts'][0]['pins'])


def test_unmarked_outputs_still_conflict():
    assert conflicts(fixture(part(False),('U1.OUT_A','U1.OUT_B')))


def test_group_is_scoped_to_component_and_cannot_parallel_two_regulators():
    assert conflicts(fixture(part(),('U1.OUT_A','U1.OUT_B','U2.OUT_A','U2.OUT_B'),('U1','U2')))


def test_distinct_output_groups_on_same_component_remain_conflicting():
    p=part()
    p=replace(p,pins={**p.pins,'OUT_B':replace(p.pins['OUT_B'],parallel_power_group='OTHER')})
    assert conflicts(fixture(p,('U1.OUT_A','U1.OUT_B')))


@pytest.mark.parametrize('domains,directions',(('digital','output'),('power','input'),('power','bidirectional'),('power,digital','output')))
def test_permission_rejects_digital_input_and_mixed_profiles(domains,directions):
    with pytest.raises(CopperScriptError,match='power-only output'):
        part(domains=domains,directions=directions)


def test_direct_ir_rejects_profileless_or_unnamed_permission():
    with pytest.raises(ValueError,match='power-only output'):
        PackagePinDefinition('OUT','1',parallel_power_group='CORE')
    profile=ElectricalProfile(frozenset({SignalDomain.POWER}),frozenset({Direction.OUTPUT}))
    with pytest.raises(ValueError,match='power-only output'):
        PackagePinDefinition('OUT','1',profile,parallel_power_group='')


def test_external_tie_permission_does_not_join_unrouted_physical_contacts():
    from pcbir import (BoardOutline, FootprintPad, PadKind, PadReference,
        PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, run_physical_drc)
    fp=PhysicalFootprint('Core',(FootprintPad('1',Point.mm(-1,0),Size.mm(.5,.5),kind=PadKind.SMD),
        FootprintPad('2',Point.mm(1,0),Size.mm(.5,.5),kind=PadKind.SMD)),Size.mm(3,2))
    board=PhysicalBoard('Core',BoardOutline.rectangle(10,10),{'Core':fp},
        (Placement('U1','Core',Point.mm(5,5)),),
        (PhysicalNet('CORE',(PadReference('U1','1'),PadReference('U1','2'))),))
    assert not fp.internal_pad_groups
    assert any(f.code=='DRC-OPEN-NET' for f in run_physical_drc(board).findings)

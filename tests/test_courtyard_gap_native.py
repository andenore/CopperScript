"""Source member floors, rigid contracts and native pair-policy parity."""
from dataclasses import replace
from itertools import combinations

import pytest

from pcbir import compile_source,prototype_physicalize
from pcbir.backends.kicad_pcb import (KiCadPcbBackend,_courtyard_pair_groups,
    _render_rules,kicad_reference)
from pcbir.clusters import footprint_geometry_digest
from pcbir.drc import physical_board_digest,run_physical_drc
from pcbir.physical import (BoardOutline,ComponentPlacementRule,FootprintLayer,
    FootprintPad,FootprintRectangle,PhysicalBoard,PhysicalFootprint,Placement,
    PlacementTarget,Point,RigidPlacementCluster,RigidPlacementMember,Size,
    component_pair_courtyard_gap_nm)
from pcbir.requirement_verification import verify_requirements


def fixture(gaps=(None,None), *, gap_nm=200000,rigid=None,refs=('ALPHA','BETA')):
    vertices=tuple(Point.mm(x,y) for x,y in ((-.5,-.5),(.5,-.5),(.5,.5),(-.5,.5)))
    fp=PhysicalFootprint('courtyard-unit',
        (FootprintPad('1',Point(0,0),Size.mm('.2','.2')),),Size.mm(1,1),courtyard=vertices,
        graphics=(FootprintRectangle(Point.mm(-.5,-.5),Point.mm(.5,.5),10000,FootprintLayer.COURTYARD),))
    poses=(Placement(refs[0],fp.name,Point.mm(5,5)),
           Placement(refs[1],fp.name,Point(6_000_000+gap_nm,5_000_000)))
    board=PhysicalBoard('SourceCourtyard',BoardOutline.rectangle(20,15),{fp.name:fp},poses,(),
        placement_rules=tuple(ComponentPlacementRule(ref,additional_courtyard_gap_nm=value)
                              for ref,value in zip(refs,gaps)))
    if rigid is not None:
        members=tuple(RigidPlacementMember(p.reference,fp.name,footprint_geometry_digest(fp),
            Point(p.position.x_nm-poses[0].position.x_nm,0)) for p in poses)
        board=replace(board,rigid_clusters=(RigidPlacementCluster('audited',PlacementTarget(refs[0]),
            members,'synthetic audited fixture',internal_clearance_nm=rigid),))
    return board


def exported_policy(board,first,second):
    matches=[gap for gap,left,right in _courtyard_pair_groups(board)
             if first in left and second in right or second in left and first in right]
    assert len(set(matches))<=1
    return matches[0] if matches else 500000


@pytest.mark.parametrize('gaps,expected',(((None,None),500000),((100000,None),500000),
    ((100000,100000),100000),((100000,800000),800000),((0,0),0),((0,None),500000)))
def test_generated_pair_policy_preserves_max_floor_and_exact_zero(gaps,expected):
    board=fixture(gaps)
    assert exported_policy(board,'ALPHA','BETA')==expected
    assert exported_policy(board,'BETA','ALPHA')==expected
    assert component_pair_courtyard_gap_nm(board,'ALPHA','BETA')==expected
    text=_render_rules(board)
    assert '(min 0.5mm)' in text and '(min -' not in text
    if expected==0:assert '(min 0mm)' in text


@pytest.mark.parametrize('gaps,internal,expected',(((None,None),100000,100000),
    ((100000,100000),200000,200000),((800000,100000),200000,800000),((0,0),0,0)))
def test_native_rigid_contract_and_stricter_source_floor_share_one_resolver(gaps,internal,expected):
    board=fixture(gaps,rigid=internal)
    assert exported_policy(board,'ALPHA','BETA')==expected


def test_many_annotations_compact_to_one_exact_clique_and_default_cross_pairs():
    board=fixture((100000,100000))
    fp=next(iter(board.footprints))
    names=tuple(f'CAP_{index:02}' for index in range(55))
    board=replace(board,outline=BoardOutline.rectangle(100,100),
        placements=tuple(Placement(ref,fp,Point.mm(5+3*(i%10),5+3*(i//10))) for i,ref in enumerate(names))
                   +(Placement('UNANNOTATED',fp,Point.mm(70,70)),),
        placement_rules=tuple(ComponentPlacementRule(ref,additional_courtyard_gap_nm=100000) for ref in names))
    assert len(_courtyard_pair_groups(board))==1
    text=_render_rules(board)
    assert len(text)<6000
    for first,second in combinations(names,2):assert exported_policy(board,first,second)==100000
    assert exported_policy(board,names[0],'UNANNOTATED')==500000
    assert text==_render_rules(replace(board,placements=tuple(reversed(board.placements)),
                                     placement_rules=tuple(reversed(board.placement_rules))))


def test_safe_native_reference_mapping_and_alias_collision_fail_closed():
    board=fixture((100000,100000),refs=("A'quoted",r'B\slash'))
    text=_render_rules(board)
    assert "A.Reference == 'A_quoted'" in text and "B.Reference == 'B_slash'" in text
    assert kicad_reference("A'quoted")=='A_quoted'
    collision=fixture(refs=('A/B','A_B'))
    with pytest.raises(ValueError,match='colliding native references'):
        KiCadPcbBackend().generate(collision)


def test_default_pose_option_does_not_authorize_a_weaker_source_board_or_digest():
    from pcbir.placement import PlacementPlannerOptions,_legal
    board=fixture()
    first,second=board.placements
    assert _legal(second,{first.reference:first},board,PlacementPlannerOptions(component_clearance_nm=200000))
    assert 'DRC-PLACEMENT' in {f.code for f in run_physical_drc(board).findings}
    annotated=fixture((100000,100000))
    assert 'DRC-PLACEMENT' not in {f.code for f in run_physical_drc(annotated).findings}
    zero=fixture((0,0))
    assert len({physical_board_digest(b) for b in (board,annotated,zero)})==3
    coverage={c.check:c.status.value for c in run_physical_drc(annotated).coverage}
    assert coverage['courtyard_gap']=='executed'
    assert next(c for c in run_physical_drc(board).coverage if c.check=='courtyard_gap').status.value=='not_applicable'


@pytest.mark.parametrize('declaration',(
    'constraint courtyard_gap(A,B) { additional = 0.1mm; }',
    'constraint placement_region(A,B) { x=1mm; y=1mm; width=30mm; height=20mm; courtyard_gap=0.1mm; }'))
def test_requirement_verifies_current_source_floor_and_cannot_hide_missing_or_weaker_policy(declaration):
    source=compile_source('board NativeFloor { use library "tiny"; component A:RESISTOR; component B:RESISTOR; '
        'net N { A.1; B.1; } '+declaration+' }')
    board=prototype_physicalize(source)
    board=replace(board,placements=tuple(replace(p,position=Point.mm(10 if p.reference=='A' else 20,10))
                                        for p in board.placements))
    report=verify_requirements(source,board,stage='placement')
    assert report['requirements'][0]['status']=='pass'
    missing=replace(board,placement_rules=tuple(replace(r,additional_courtyard_gap_nm=None) for r in board.placement_rules))
    assert verify_requirements(source,missing,stage='placement')['requirements'][0]['status']=='blocked'
    weaker=replace(board,placement_rules=tuple(replace(r,additional_courtyard_gap_nm=50000) for r in board.placement_rules))
    assert verify_requirements(source,weaker,stage='placement')['requirements'][0]['status']=='fail'


def test_requirements_max_merged_stricter_duplicates_pass_and_native_consumers_are_declared():
    from pcbir.constraint_coverage import normalize_constraints
    source=compile_source('''board Floors { use library "tiny";
        component A:RESISTOR; component B:RESISTOR; net N { A.1; B.1; }
        constraint courtyard_gap(A,B) { additional=0.1mm; }
        constraint courtyard_gap(B,A) { additional=0.8mm; }
    }''')
    board=prototype_physicalize(source)
    board=replace(board,placements=tuple(replace(p,position=Point.mm(10 if p.reference=='A' else 20,10)) for p in board.placements))
    assert all(r['status']=='pass' for r in verify_requirements(source,board,stage='placement')['requirements'])
    assert all('kicad_rules' in c.consumers for c in normalize_constraints(source.constraints))


def test_zero_courtyard_boundary_does_not_relax_protected_nonrectangular_keepout():
    from pcbir.physical import PlacementKeepout
    from pcbir.placement import (PlacementPlannerOptions, _courtyards_too_close,
        _legal, _placement_polygon, _polygons_too_close)
    board = fixture((0, 0), gap_nm=0)
    first, second = board.placements
    a, b = (_placement_polygon(board, pose) for pose in board.placements)
    assert not _courtyards_too_close(a, b, 0)
    assert _legal(second, {first.reference: first}, board, PlacementPlannerOptions())
    triangle = BoardOutline(tuple(Point.mm(x, y) for x, y in ((5, 5), (7, 5), (5, 7))))
    protected = replace(board, keepouts=(PlacementKeepout('protected', triangle),))
    touching = replace(second, position=Point.mm(6.5, 6.5))
    assert _polygons_too_close(_placement_polygon(protected, touching), triangle.vertices, 0)
    assert not _legal(touching, {}, protected, PlacementPlannerOptions())

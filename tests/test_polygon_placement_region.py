"""Concave placement domains preserve isolated floorplan areas."""
import pytest
from pcbir import compile_source, prototype_physicalize, plan_placement, PlacementPlannerOptions, Point
from pcbir.geometry import segment_in_polygon


POLYGON='2,2;10,2;10,12;28,12;28,28;2,28'

def source(extra=''):
    return compile_source('''board PolygonRegion {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component R2: RESISTOR { footprint = "0402"; }
        net P { R1.1; R2.1; }
        constraint placement_region(R1,R2) { name = "secondary";
            polygon_mm = "'''+POLYGON+'''"; side = both; }
        '''+extra+''' mechanical { outline rectangle { width = 30 mm; height = 30 mm; } }
    }''')


def test_concave_region_lowers_and_legalizes_without_entering_the_excluded_corner():
    board=prototype_physicalize(source())
    region=board.regions[0]
    assert region.outline.vertices==tuple(Point.mm(*p.split(',')) for p in POLYGON.split(';'))
    planned=plan_placement(board,PlacementPlannerOptions(candidate_count=1,seed_count=1,
        refinement_passes=0,escape_spacing_passes=0,decoupling_placement_trials=0))
    assert planned.report.metrics.constraint_penalty_nm==0
    from pcbir.placement import _placement_polygon
    for pose in planned.board.placements:
        corners=_placement_polygon(planned.board,pose)
        assert all(segment_in_polygon(a,b,region.outline.vertices)
                   for a,b in zip(corners,(*corners[1:],corners[0])))
    assert all(not (p.position.x_nm>10_000_000 and p.position.y_nm<12_000_000) for p in planned.board.placements)


def test_zone_can_reuse_the_same_polygon_region():
    board=prototype_physicalize(source('constraint copper_zone(P) { layers = "F.Cu"; region = "secondary"; }'))
    assert board.zones[0].outline.outer.vertices==board.regions[0].outline.vertices


def test_polygon_and_rectangle_region_modes_are_exclusive():
    # Keep the invalid source visible to the ordinary lowering interface.
    text='''board Bad { use library "tiny"; component R1: RESISTOR;
        constraint placement_region(R1) { polygon_mm = "2,2;10,2;10,10;2,10"; x = 2 mm; }
    }'''
    with pytest.raises(ValueError,match='mutually exclusive'):
        prototype_physicalize(compile_source(text))


@pytest.mark.parametrize('x',('20','9.8'))
def test_fixed_courtyard_cannot_enter_or_cross_the_excluded_corner(x):
    from pcbir.layout import PlacementPlanningError
    board=prototype_physicalize(source('constraint fixed_placement(R1) { x = '+x+' mm; y = 5 mm; }'))
    with pytest.raises(PlacementPlanningError,match='fixed component'):
        plan_placement(board,PlacementPlannerOptions(candidate_count=1,seed_count=1,refinement_passes=0))

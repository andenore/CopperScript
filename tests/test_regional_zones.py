"""Source-to-physical regional pours, with no fabricated filled connectivity."""
from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperLayer, KiCadPcbBackend, Point,
                   compile_source, prototype_physicalize)
from pcbir.quantities import millimeters
from pcbir.zone_geometry import lower_zone_outline


def board(parameters, declarations=""):
    return prototype_physicalize(compile_source('''board Regional {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component C1: CAPACITOR { footprint = "0402"; }
        net V3V3 { R1.1; C1.1; }
        net GND { R1.2; C1.2; }
        constraint copper_zone(V3V3) { layers = "F.Cu"; ''' + parameters + " }\n" + declarations + "}"))


@pytest.mark.parametrize("parameters", [
    'x = 4mm; y = 4mm; width = 31mm; height = 24mm;',
    'polygon_mm = "4,4;35,4;35,28;4,28";',
])
def test_regional_zone_source_exports_requested_geometry(parameters):
    physical = board(parameters + ' priority = 3; pad_connection = solid;')
    zone = physical.zones[0]
    assert set(zone.outline.outer.vertices) == {Point.mm(4, 4), Point.mm(35, 4), Point.mm(35, 28), Point.mm(4, 28)}
    assert zone.layers == (CopperLayer.FRONT,) and zone.priority == 3
    assert not physical.zone_fills and physical.metadata["fabrication_ready"] == "false"
    exported = KiCadPcbBackend().generate(physical).artifacts[0].content
    assert "(priority 3)" in exported and '(net_name "V3V3")' in exported


def test_named_region_resolves_even_when_declared_after_zone():
    physical = board('region = "logic"; inset = 1mm;', '''
        constraint placement_region(R1, C1) {
            name = "logic"; x = 4mm; y = 4mm; width = 31mm; height = 24mm;
        }''')
    assert set(physical.zones[0].outline.outer.vertices) == {Point.mm(5, 5), Point.mm(34, 5), Point.mm(34, 27), Point.mm(5, 27)}


@pytest.mark.parametrize("parameters, message", [
    ('region = "missing";', "unknown or ambiguous"),
    ('polygon_mm = "4,4;35,4;35,28;4,28"; x = 4mm;', "mutually exclusive"),
    ('x = 4mm; y = 4mm; width = 31mm;', "requires x"),
    ('polygon_mm = "4,4;35,4";', "invalid copper_zone"),
    ('polygon_mm = "4,4;35,4;4,28;35,28";', "invalid copper_zone"),
    ('polygon_mm = "-4,4;35,4;35,28;4,28";', "inside the board"),
    ('polygon_mm = "4,4;35,4;30,20;4,28"; inset = 1mm;', "nonzero copper_zone inset"),
    ('priority = 1.5;', "nonnegative integer"),
    ('priority = -1;', "nonnegative integer"),
])
def test_ambiguous_or_invalid_regional_zones_fail(parameters, message):
    with pytest.raises(ValueError, match=message):
        board(parameters)


def test_default_zone_remains_whole_outline_and_inset_is_exact():
    physical = board('inset = 0.5mm;')
    assert set(physical.zones[0].outline.outer.vertices) == {
        Point.mm(.5, .5), Point.mm(99.5, .5), Point.mm(99.5, 79.5), Point.mm(.5, 79.5)}


def test_zone_cannot_cross_concave_board_void_between_vertices():
    outline = BoardOutline(tuple(Point.mm(x, y) for x, y in (
        (0, 0), (20, 0), (20, 20), (12, 20), (12, 8), (8, 8), (8, 20), (0, 20))))
    with pytest.raises(ValueError, match="inside the board"):
        lower_zone_outline({"polygon_mm": "4,4;16,4;16,16;4,16"}, outline, ())

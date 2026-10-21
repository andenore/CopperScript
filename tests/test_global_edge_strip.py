"""Global tile centres cover a partial edge tile (D3 plan 3.5)."""
from pcbir.physical import CopperLayer, Point, nm_from_mm
from pcbir.routing import _axis_layout, _planar_capacity

TILE = nm_from_mm(5)


def test_a_wide_leftover_strip_gets_a_centre_in_its_middle():
    # 11.9 mm: centres at 2.5 and 7.5 cover up to 10 mm; the 1.9 mm strip gets its own centre.
    centers, band = _axis_layout(nm_from_mm("34.1"), nm_from_mm("46"), TILE)
    assert centers == (nm_from_mm("36.6"), nm_from_mm("41.6"), nm_from_mm("45.05"))
    assert band == (nm_from_mm("44.1"), nm_from_mm(46))


def test_whole_tiles_and_narrow_strips_are_unchanged():
    assert _axis_layout(0, nm_from_mm(10), TILE) == ((nm_from_mm("2.5"), nm_from_mm("7.5")), None)
    # 11 mm: the last centre is 3.5 mm from the edge, within three quarters of a tile.
    assert _axis_layout(0, nm_from_mm(11), TILE) == ((nm_from_mm("2.5"), nm_from_mm("7.5")), None)
    assert _axis_layout(0, nm_from_mm(3), TILE) == ((nm_from_mm("1.5"),), None)


def test_an_edge_along_the_strip_counts_only_the_strip_width():
    first, second = Point(0, nm_from_mm("45.05")), Point(TILE, nm_from_mm("45.05"))
    pitch = nm_from_mm("0.5")
    full = _planar_capacity(first, second, TILE, pitch, 0, CopperLayer.FRONT, (), ())
    strip = _planar_capacity(first, second, TILE, pitch, 0, CopperLayer.FRONT, (), (),
                             band=(nm_from_mm("44.1"), nm_from_mm(46)))
    assert (full, strip) == (10, 3)


def test_the_strip_row_is_added_only_on_request():
    assert _axis_layout(nm_from_mm("34.1"), nm_from_mm("46"), TILE, edge_strip=False) == (
        (nm_from_mm("36.6"), nm_from_mm("41.6")), None)

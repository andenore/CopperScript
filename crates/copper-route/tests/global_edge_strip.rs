//! Global tile centres cover a partial edge tile (ported from the former
//! `tests/test_global_edge_strip.py`).

use copper_physical::{CopperLayer, Point};
use copper_route::routing::{axis_layout, planar_capacity};

const TILE: i64 = 5_000_000;

fn um(value: i64) -> i64 {
    value * 1_000
}

#[test]
fn a_wide_leftover_strip_gets_a_centre_in_its_middle() {
    // 11.9 mm: centres at 2.5 and 7.5 cover up to 10 mm; the 1.9 mm strip gets its own centre.
    let (centers, band) = axis_layout(um(34_100), um(46_000), TILE, true);
    assert_eq!(centers, [um(36_600), um(41_600), um(45_050)]);
    assert_eq!(band, Some((um(44_100), um(46_000))));
}

#[test]
fn whole_tiles_and_narrow_strips_are_unchanged() {
    assert_eq!(axis_layout(0, um(10_000), TILE, true), (vec![um(2_500), um(7_500)], None));
    // 11 mm: the last centre is 3.5 mm from the edge, within three quarters of a tile.
    assert_eq!(axis_layout(0, um(11_000), TILE, true), (vec![um(2_500), um(7_500)], None));
    assert_eq!(axis_layout(0, um(3_000), TILE, true), (vec![um(1_500)], None));
}

#[test]
fn an_edge_along_the_strip_counts_only_the_strip_width() {
    let (first, second) = (Point::new(0, um(45_050)), Point::new(TILE, um(45_050)));
    let pitch = um(500);
    let full = planar_capacity(first, second, TILE, pitch, 0, CopperLayer::Front, &[], &[], None);
    let strip =
        planar_capacity(first, second, TILE, pitch, 0, CopperLayer::Front, &[], &[], Some((um(44_100), um(46_000))));
    assert_eq!((full, strip), (10, 3));
}

#[test]
fn the_strip_row_is_added_only_on_request() {
    assert_eq!(axis_layout(um(34_100), um(46_000), TILE, false), (vec![um(36_600), um(41_600)], None));
}

#[test]
fn a_last_centre_inside_the_edge_clearance_moves_into_the_board() {
    // 8 mm: the second centre sits 0.5 mm from the edge; with strips it moves
    // to the middle of its tile's part inside the board (5..8 mm).
    assert_eq!(axis_layout(0, um(8_000), TILE, false), (vec![um(2_500), um(7_500)], None));
    assert_eq!(axis_layout(0, um(8_000), TILE, true), (vec![um(2_500), um(6_500)], Some((um(5_000), um(8_000)))));
    // 9 mm: the last centre is 1.5 mm from the edge, more than a quarter tile: unchanged.
    assert_eq!(axis_layout(0, um(9_000), TILE, true), (vec![um(2_500), um(7_500)], None));
}

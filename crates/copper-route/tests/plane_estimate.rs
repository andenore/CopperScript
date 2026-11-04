//! The raster plane estimate names the pads it expects to stay cut off;
//! ported from `tests/test_plane_estimate.py`. Rust's `PlaneEstimate` has no
//! `report()`: the open nets are the nets with more than one group.

mod support_planes;

use copper_pcb::board::PhysicalBoard;
use copper_physical::{CopperLayer, Point};
use copper_route::plane::{PlaneStitchOptions, PlaneStitchResult, repair_cut_off_plane_pads, stitch_zone_pads};
use copper_route::plane_estimate::{PlaneEstimate, estimate_plane_connectivity};
use indexmap::IndexMap;
use support_planes::*;

/// GND pads J1..J3 at x = 3, 7 and 17 mm under a front plane ending at `right_mm`.
fn plane_board(right_mm: f64) -> PhysicalBoard {
    let mut board = board("estimate", 20.0, 12.0);
    add_footprint(&mut board, footprint("one-pad", vec![pad("1", Point::new(0, 0), size(0.6, 0.6))], size(1.0, 1.0)));
    board.placements = [3.0, 7.0, 17.0]
        .iter()
        .enumerate()
        .map(|(i, x)| place(&format!("J{}", i + 1), "one-pad", mm(*x, 6.0)))
        .collect();
    board.nets = vec![net("GND", &[("J1", "1"), ("J2", "1"), ("J3", "1")])];
    board.zones = vec![zone("plane", "GND", &[CopperLayer::Front], rect(1.0, 1.0, right_mm, 11.0))];
    board
}

fn estimate(board: &PhysicalBoard) -> PlaneEstimate {
    estimate_plane_connectivity(board).unwrap()
}

fn open_nets(estimate: &PlaneEstimate) -> Vec<&str> {
    let mut nets: Vec<&str> =
        estimate.net_groups.iter().filter(|(_, groups)| **groups > 1).map(|(net, _)| net.as_str()).collect();
    nets.sort();
    nets
}

fn j3_open() -> IndexMap<String, Vec<String>> {
    IndexMap::from([("GND".to_owned(), vec!["J3.1".to_owned()])])
}

fn options() -> PlaneStitchOptions {
    PlaneStitchOptions { maximum_radius_nm: nm(2.0), include_surface_zones: true, ..PlaneStitchOptions::default() }
}

#[test]
fn pads_outside_the_largest_group_are_named() {
    let estimate = estimate(&plane_board(10.0));
    assert_eq!(estimate.opens, 1);
    assert_eq!(open_nets(&estimate), ["GND"]);
    assert_eq!(estimate.open_pads, j3_open());
}

#[test]
fn a_connected_plane_names_no_pads() {
    let estimate = estimate(&plane_board(19.0));
    assert_eq!(estimate.opens, 0);
    assert!(estimate.open_pads.is_empty());
}

/// Both planes stitched, then J3's own contact dropped (as if it relied on
/// the F.Cu pour) and J3 walled in by a foreign front trace ring.
fn walled_board() -> PlaneStitchResult {
    let mut board = plane_board(19.0);
    board.zones.push(zone("plane-b", "GND", &[CopperLayer::Back], rect(1.0, 1.0, 19.0, 11.0)));
    board.nets.push(net("X", &[]));
    let mut stitched = stitch_zone_pads(&board, Some(&options())).unwrap();
    let j3 = mm(17.0, 6.0);
    let own = (0..stitched.board.vias.len())
        .min_by_key(|i| {
            let p = stitched.board.vias[*i].position;
            (p.x_nm - j3.x_nm).abs() + (p.y_nm - j3.y_nm).abs()
        })
        .expect("a stitched via");
    let own_via = stitched.board.vias.remove(own);
    stitched.board.tracks.retain(|t| t.start != own_via.position && t.end != own_via.position);
    let corners = [mm(15.3, 4.3), mm(18.7, 4.3), mm(18.7, 7.7), mm(15.3, 7.7)];
    stitched
        .board
        .tracks
        .extend((0..4).map(|i| track("X", corners[i], corners[(i + 1) % 4], nm(0.2), CopperLayer::Front)));
    stitched
}

#[test]
fn walled_off_pad_is_restitched_with_explicit_copper() {
    let stitch = walled_board();
    assert_eq!(estimate(&stitch.board).open_pads, j3_open());
    let vias = stitch.board.vias.len();
    let (repaired, still) = repair_cut_off_plane_pads(stitch, &options(), 2).unwrap();
    assert!(still.is_empty(), "{still:?}");
    assert_eq!(repaired.board.vias.len(), vias + 1);
    assert_eq!(estimate(&repaired.board).opens, 0);
}

//! Off-ray physical escape sites, exact anchor verification and owned
//! cleanup; ported from `tests/test_two_leg_fanout.py`. The site-budget and
//! bounded-maze cases are crate-private (`src/test_support_planes.rs`).

mod support_planes;

use copper_pcb::board::{PadReference, PhysicalBoard, PhysicalNet, TrackSegment};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::fanout::{FanoutOptions, FanoutResult, route_fanout};
use copper_route::pin_escape::verified_fanout_path;
use support_planes::fanout::*;
use support_planes::*;

fn opts() -> FanoutOptions {
    FanoutOptions { minimum_component_pads: 2, maximum_neighbor_distance_nm: nm(3.0), ..FanoutOptions::default() }
}

fn fanout(board: &PhysicalBoard, options: &FanoutOptions) -> FanoutResult {
    route_fanout(board, Some(options), None).unwrap()
}

fn u1() -> PadReference {
    pad_ref("U", "1")
}

fn verified(board: &PhysicalBoard, anchor: Point) -> Option<Vec<TrackSegment>> {
    verified_fanout_path(board, &u1(), "A", anchor, &RoutingClearanceIndex::new(board).unwrap()).unwrap()
}

#[test]
fn two_leg_escape_reaches_real_off_ray_site_and_tries_other_order() {
    let board = two_leg_board(true);
    let old = fanout(&board, &FanoutOptions { two_leg_escapes: false, surface_fallback: false, ..opts() });
    let new = fanout(&board, &opts());
    assert_eq!(old.pending_pads, [u1()]);
    assert!(new.pending_pads.is_empty(), "{:?}", new.pending_pads);
    assert_eq!(new.accesses[&u1()], mm(4.0, 4.0));
    let width = board.rules.default_track_width_nm;
    // The diagonal-first order is blocked, so the straight-first one is used.
    assert_eq!(
        new.created_tracks,
        [
            track("A", mm(5.0, 6.0), mm(5.0, 5.0), width, CopperLayer::Front),
            track("A", mm(5.0, 5.0), mm(4.0, 4.0), width, CopperLayer::Front),
        ]
    );
    assert_eq!((new.added_track_count, new.added_via_count), (2, 1));
    let again = fanout(&board, &opts());
    assert_eq!(
        (&again.board, &again.accesses, &again.created_tracks),
        (&new.board, &new.accesses, &new.created_tracks)
    );
    assert_drc_clear(&new.board, &["DRC-COPPER-KEEPOUT", "DRC-CLEARANCE", "DRC-BOARD-EDGE"]);
}

#[test]
fn two_leg_site_budget_must_be_positive() {
    // The bound itself is checked crate-privately (`_two_leg_candidates`).
    assert!(FanoutOptions { maximum_two_leg_candidates: 0, ..opts() }.validated().is_err());
}

#[test]
fn multi_bend_anchor_is_verified_only_through_actual_connected_copper() {
    for defect in [None, Some("gap"), Some("layer"), Some("net")] {
        let mut board = fanout(&two_leg_board(false), &opts()).board;
        let width = board.rules.default_track_width_nm;
        let points = [mm(5.0, 6.0), mm(5.0, 5.5), mm(4.5, 5.0), mm(4.5, 4.5), mm(4.0, 4.0)];
        let mut tracks: Vec<TrackSegment> =
            points.windows(2).map(|pair| track("A", pair[0], pair[1], width, CopperLayer::Front)).collect();
        match defect {
            Some("gap") => {
                tracks.remove(1);
            }
            Some("layer") => tracks.iter_mut().for_each(|t| t.layer = CopperLayer::Back),
            Some("net") => {
                board.nets.push(PhysicalNet { name: "other".to_owned(), pads: Vec::new() });
                tracks.iter_mut().for_each(|t| t.net = "other".to_owned());
            }
            _ => {}
        }
        board.tracks = tracks.clone();
        let path = verified(&board, mm(4.0, 4.0));
        if defect.is_none() {
            assert_eq!(path, Some(tracks));
        } else {
            assert_eq!(path, None, "{defect:?}");
        }
    }
}

#[test]
fn claimed_anchor_cannot_skip_missing_wrong_layer_or_net_legs() {
    let routed = fanout(&two_leg_board(true), &opts()).board;
    assert!(verified(&routed, mm(4.0, 4.0)).is_some());
    for mutation in ["missing", "gap", "layer", "net", "via-net", "span", "oblique"] {
        let mut board = routed.clone();
        if matches!(mutation, "net" | "via-net") {
            board.nets.push(PhysicalNet { name: "other".to_owned(), pads: Vec::new() });
        }
        match mutation {
            "missing" => board.tracks.truncate(1),
            "gap" => board.tracks.last_mut().unwrap().start.x_nm += 1,
            "layer" => board.tracks.iter_mut().for_each(|t| t.layer = CopperLayer::Back),
            "net" => board.tracks.iter_mut().for_each(|t| t.net = "other".to_owned()),
            "via-net" => board.vias[0].net = "other".to_owned(),
            "span" => {
                board.stackup = four_layers();
                board.vias[0].from_layer = CopperLayer::In1;
            }
            "oblique" => {
                let width = board.rules.default_track_width_nm;
                board.tracks = vec![track("A", mm(5.0, 6.0), mm(4.0, 4.0), width, CopperLayer::Front)];
            }
            _ => unreachable!(),
        }
        assert_eq!(verified(&board, mm(4.0, 4.0)), None, "{mutation}");
    }
}

#[test]
fn reversed_input_segments_verified_without_rewriting_identity() {
    let mut board = fanout(&two_leg_board(true), &opts()).board;
    for t in &mut board.tracks {
        std::mem::swap(&mut t.start, &mut t.end);
    }
    assert_eq!(verified(&board, mm(4.0, 4.0)), Some(board.tracks.clone()));
}

#[test]
fn existing_two_leg_escape_reused_without_new_ownership_or_duplicates() {
    let initial = fanout(&two_leg_board(true), &opts());
    let repeated = fanout(&initial.board, &opts());
    assert_eq!(repeated.accesses, initial.accesses);
    assert_eq!(repeated.board, initial.board);
    assert!(repeated.created_tracks.is_empty() && repeated.created_vias.is_empty());
    assert_eq!((repeated.added_track_count, repeated.added_via_count), (0, 0));
}

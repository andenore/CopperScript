//! Local plane access must honor declared, bounded power-pin neckdowns;
//! ported from `tests/test_surface_power_breakout.py`.

mod support_planes;

use std::collections::BTreeSet;

use copper_drc::breakout::BreakoutRegions;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_physical::{CopperKeepout, CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::surface_path::{ViaSearch, surface_path, surface_path_to_via, track_inside_board};
use support_planes::*;

const FRONT: CopperLayer = CopperLayer::Front;

/// `board`: a three-land 0.5 mm-pitch row; only the middle land U.2 is on
/// the 0.8 mm power net P, which may neck to 0.2 mm within 1 mm of it.
fn power_board(neck: bool) -> PhysicalBoard {
    let mut board = board("power escape", 20.0, 10.0);
    let lands =
        [-0.5, 0.0, 0.5].iter().enumerate().map(|(i, y)| pad(&(i + 1).to_string(), mm(0.0, *y), size(0.6, 0.25)));
    add_footprint(&mut board, footprint("fine-row", lands.collect(), size(1.0, 2.0)));
    board.placements = vec![place("U", "fine-row", mm(5.0, 5.0))];
    board.nets = vec![net("P", &[("U", "2")])];
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(nm(0.8)),
        breakout_length_nm: neck.then(|| nm(1.0)),
        breakout_width_nm: neck.then(|| nm(0.2)),
        ..rule("P")
    }];
    board
}

/// The board with only the assigned land: the 0.8 mm profile fits everywhere.
fn lone_land() -> PhysicalBoard {
    let mut board = power_board(true);
    let row = board.footprints.get_mut("fine-row").unwrap();
    row.pads = vec![row.pads[1].clone()];
    board
}

fn path(source: &PhysicalBoard, committed: &[TrackSegment]) -> Option<Vec<TrackSegment>> {
    let index = RoutingClearanceIndex::new(source).unwrap();
    surface_path(source, &index, "P", mm(5.0, 5.0), mm(8.0, 5.0), nm(0.8), FRONT, committed).unwrap()
}

fn widths(tracks: &[TrackSegment]) -> BTreeSet<i64> {
    tracks.iter().map(|track| track.width_nm).collect()
}

/// Every piece is at least as wide as the region requires and clears.
fn checked(source: &PhysicalBoard, tracks: &[TrackSegment]) -> bool {
    let index = RoutingClearanceIndex::new(source).unwrap();
    tracks.iter().all(|t| {
        t.width_nm >= index.breakout.required_width_nm(t, nm(0.8))
            && index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).unwrap()
    })
}

#[test]
fn local_power_escape_necks_only_inside_declared_region() {
    let source = power_board(true);
    let tracks = path(&source, &[]).expect("a path");
    assert_eq!(widths(&tracks), [nm(0.2), nm(0.8)].into());
    assert!(checked(&source, &tracks));
    assert_eq!(path(&power_board(false), &[]), None);
}

#[test]
fn open_pad_contact_prefers_full_width_despite_breakout_permission() {
    let tracks = path(&lone_land(), &[]).expect("a path");
    assert!(!tracks.is_empty());
    assert_eq!(widths(&tracks), [nm(0.8)].into());
}

#[test]
fn cleanup_does_not_narrow_an_already_checked_full_width_contact() {
    let source = lone_land();
    let contact = track("P", mm(5.0, 5.0), mm(6.0, 5.0), nm(0.8), FRONT);
    let index = RoutingClearanceIndex::new(&source).unwrap();
    assert!(index.can_track("P", contact.start, contact.end, contact.width_nm, contact.layer).unwrap());
    assert_eq!(BreakoutRegions::new(&source).neck_down(std::slice::from_ref(&contact)), [contact]);
}

#[test]
fn back_layer_contact_prefers_full_width_without_surface_pad_obstacles() {
    let source = power_board(true);
    let index = RoutingClearanceIndex::new(&source).unwrap();
    let tracks = index.route_pieces("P", mm(5.0, 5.0), mm(8.0, 5.0), nm(0.8), CopperLayer::Back).unwrap();
    assert!(!tracks.is_empty());
    assert_eq!(widths(&tracks), [nm(0.8)].into());
}

#[test]
fn widening_must_clear_board_material_and_keepouts() {
    for obstacle in ["edge", "keepout"] {
        let mut source = lone_land();
        let (mut start, mut end) = (mm(5.0, 5.0), mm(6.0, 5.0));
        if obstacle == "edge" {
            (start, end) = (mm(5.0, 0.35), mm(6.0, 0.35));
            source.placements[0].position = start;
        } else {
            source.copper_keepouts = vec![CopperKeepout {
                block_vias: false,
                block_zones: false,
                ..keepout("routing obstacle", &[FRONT], rect(5.0, 5.35, 6.0, 5.45))
            }];
        }
        let index = RoutingClearanceIndex::new(&source).unwrap();
        let tracks = index.route_pieces("P", start, end, nm(0.8), FRONT).unwrap();
        assert!(!tracks.is_empty(), "{obstacle}");
        assert_eq!(widths(&tracks), [nm(0.2)].into(), "{obstacle}");
        for t in &tracks {
            assert!(index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).unwrap(), "{obstacle}");
            assert!(track_inside_board(&source, t.start, t.end, t.width_nm).unwrap(), "{obstacle}");
        }
    }
}

#[test]
fn valid_existing_neckdown_chain_is_reused_not_recreated() {
    let mut source = power_board(true);
    let tracks = path(&source, &[]).expect("a path");
    assert!(!tracks.is_empty());
    source.tracks = tracks.clone();
    assert_eq!(path(&source, &tracks), Some(Vec::new()));
}

#[test]
fn unapproved_narrow_trunk_cannot_be_reused() {
    let mut source = power_board(true);
    let tracks: Vec<TrackSegment> =
        path(&source, &[]).expect("a path").into_iter().map(|t| TrackSegment { width_nm: nm(0.2), ..t }).collect();
    source.tracks = tracks.clone();
    let result = path(&source, &tracks).expect("a path");
    assert!(result.iter().any(|t| t.width_nm == nm(0.8)), "{result:?}");
}

#[test]
fn via_maze_emits_checked_neckdown_and_full_width_pieces() {
    let end = mm(8.0, 5.0);
    let accept = |p: Point| p == end;
    let targets = [end];
    let search = |source: &PhysicalBoard| {
        let index = RoutingClearanceIndex::new(source).unwrap();
        let bounds = ViaSearch {
            step_nm: nm(0.1),
            radius_nm: nm(3.0),
            via_size_nm: nm(0.6),
            via_drill_nm: nm(0.3),
            via_layers: (FRONT, CopperLayer::Back),
            accept_via: &accept,
            via_targets: &targets,
            state_budget: 5000,
        };
        surface_path_to_via(source, &index, "P", mm(5.0, 5.0), nm(0.8), FRONT, &bounds).unwrap()
    };
    let source = power_board(true);
    let (tracks, via) = search(&source).expect("a via escape");
    assert_eq!(via, end);
    assert_eq!(widths(&tracks), [nm(0.2), nm(0.8)].into());
    assert!(checked(&source, &tracks));
    assert_eq!(search(&power_board(false)), None);
}

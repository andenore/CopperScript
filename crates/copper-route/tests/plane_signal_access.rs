//! Prefer plane contacts that preserve sampled exits and retain legal fallbacks.
mod support_planes;

use copper_physical::CopperLayer;
use copper_route::RoutingClearanceIndex;
use copper_route::boundary_access::{BoundaryAccessOptions, package_collar, surface_escapes};
use copper_route::plane::{PlaneStitchOptions, stitch_zone_pads};
use support_planes::*;

fn signal_exits(board: &copper_pcb::board::PhysicalBoard) -> Vec<Vec<copper_pcb::board::TrackSegment>> {
    let settings = BoundaryAccessOptions::default();
    surface_escapes(
        board,
        &RoutingClearanceIndex::new(board).unwrap(),
        "SIGNAL",
        mm(8.5, 8.0),
        CopperLayer::Front,
        package_collar(board, "U", settings.collar_margin_nm).unwrap(),
        &settings,
        false,
    )
    .unwrap()
}

fn contact_board() -> copper_pcb::board::PhysicalBoard {
    let mut board = board("ground-contact-signal-exit", 16.0, 16.0);
    board.stackup = four_layers();
    board.rules.minimum_clearance_nm = nm(0.2);
    board.rules.default_track_width_nm = nm(0.2);
    board.rules.default_via_size_nm = nm(0.6);
    board.rules.default_via_drill_nm = nm(0.3);
    add_footprint(
        &mut board,
        footprint(
            "package",
            vec![pad("1", mm(0.0, -0.75), size(0.2, 0.3)), pad("2", mm(0.5, -0.75), size(0.2, 0.3))],
            size(2.0, 2.0),
        ),
    );
    add_footprint(&mut board, footprint("terminal", vec![pad("1", mm(0.0, 0.0), size(0.2, 0.3))], size(0.5, 0.5)));
    board.placements = vec![place("U", "package", mm(8.0, 8.75)), place("REMOTE", "terminal", mm(8.5, 5.0))];
    board.nets = vec![net("GND", &[("U", "1")]), net("SIGNAL", &[("U", "2"), ("REMOTE", "1")])];
    let mut ground = zone("contact-column", "GND", &[CopperLayer::In1], rect(7.95, 5.0, 8.05, 12.0));
    ground.minimum_width_nm = nm(0.09);
    board.zones = vec![ground];
    board.copper_keepouts = vec![
        keepout("right", &[CopperLayer::Front], rect(8.65, 6.0, 10.0, 10.0)),
        keepout("bottom", &[CopperLayer::Front], rect(8.3, 8.3, 10.0, 10.0)),
        keepout("upper-left", &[CopperLayer::Front], rect(7.8, 6.0, 8.2, 7.1)),
    ];
    board
}

#[test]
fn ground_contact_keeps_the_existing_signal_exit_and_still_reaches_its_plane() {
    let board = contact_board();
    let original = board.clone();
    assert!(!signal_exits(&board).is_empty(), "the untouched package has a checked surface exit");
    let stitched = stitch_zone_pads(&board, Some(&PlaneStitchOptions::default())).unwrap();
    assert!(stitched.pending_pads.is_empty());
    assert!(stitched.stitched_pads.contains(&pad_ref("U", "1")));
    assert_eq!(stitched.added_via_count, 1);
    assert!(
        !signal_exits(&stitched.board).is_empty(),
        "contact must preserve at least one exact package-boundary path"
    );
    assert!(hard_findings(&stitched.board).is_empty(), "{:?}", hard_findings(&stitched.board));
    assert_eq!(board, original, "authored copper and constraints remain unchanged");
}

#[test]
fn a_safe_nearest_contact_keeps_its_existing_choice() {
    let mut board = contact_board();
    board.copper_keepouts.clear();
    let stitched = stitch_zone_pads(&board, None).unwrap();
    assert_eq!(stitched.added_via_count, 1);
    assert_eq!(stitched.board.vias[0].position, mm(8.0, 7.5));
    assert!(!signal_exits(&stitched.board).is_empty());
}

#[test]
fn source_owned_return_is_not_removed_to_restore_a_signal_exit() {
    let mut board = contact_board();
    board.tracks.push(track("GND", mm(8.0, 8.0), mm(8.0, 7.5), nm(0.2), CopperLayer::Front));
    board.vias.push(via("GND", mm(8.0, 7.5), nm(0.6), nm(0.3)));
    assert!(signal_exits(&board).is_empty());
    let stitched = stitch_zone_pads(&board, None).unwrap();
    assert!(stitched.board.tracks.starts_with(&board.tracks));
    assert!(stitched.board.vias.starts_with(&board.vias));
    assert!(stitched.pending_pads.is_empty());
    assert!(signal_exits(&stitched.board).is_empty(), "no false claim that source geometry was repaired");
}

#[test]
fn nearby_package_contact_also_preserves_the_signal_exit() {
    let mut board = contact_board();
    let package = board.footprints.get_mut("package").unwrap();
    package.pads.retain(|pad| pad.number != "1");
    package.body_size = size(0.1, 0.1);
    board.footprints.get_mut("terminal").unwrap().body_size = size(0.1, 0.1);
    board.placements.push(place("GROUND", "terminal", mm(8.0, 8.0)));
    board.nets[0] = net("GND", &[("GROUND", "1")]);
    let stitched = stitch_zone_pads(&board, None).unwrap();
    assert!(stitched.pending_pads.is_empty());
    assert_eq!(stitched.added_via_count, 1);
    assert!(!signal_exits(&stitched.board).is_empty());
    assert!(hard_findings(&stitched.board).is_empty());
}

#[test]
fn the_only_legal_explicit_contact_is_kept_when_it_closes_a_sampled_exit() {
    let mut board = contact_board();
    board.zones[0].outer = rect(7.95, 7.45, 8.05, 7.55);
    let options = PlaneStitchOptions { require_explicit: true, maximum_radius_nm: nm(0.5), ..Default::default() };
    assert!(!signal_exits(&board).is_empty());
    let stitched = stitch_zone_pads(&board, Some(&options)).unwrap();
    assert!(stitched.pending_pads.is_empty());
    assert_eq!(stitched.stitched_pads, vec![pad_ref("U", "1")]);
    assert_eq!(stitched.added_via_count, 1);
    assert!(signal_exits(&stitched.board).is_empty());
    assert!(hard_findings(&stitched.board).is_empty());
}

#[test]
fn anchor_retry_keeps_the_only_legal_connection_even_when_samples_are_closed() {
    let mut board = contact_board();
    board.footprints.get_mut("package").unwrap().pads[0].position = mm(-0.2, -0.75);
    board.copper_keepouts[0] = keepout("right", &[CopperLayer::Front], rect(8.65, 7.55, 10.0, 10.0));
    board.zones[0].outer = rect(9.25, 5.95, 9.35, 6.05);
    board.placements.push(place("ANCHOR", "terminal", mm(9.3, 6.5)));
    board.nets[0] = net("GND", &[("U", "1"), ("ANCHOR", "1")]);
    let options = PlaneStitchOptions {
        require_explicit: true,
        maximum_radius_nm: nm(0.5),
        maximum_contact_radius_nm: nm(3.0),
        ..Default::default()
    };
    assert!(!signal_exits(&board).is_empty());
    // Reproduce the old unfiltered anchor candidate using the same exact
    // surface-path generator. It is legal copper but removes the last exit.
    let anchor_options = PlaneStitchOptions {
        only_pads: Some(std::collections::BTreeSet::from([pad_ref("ANCHOR", "1")])),
        ..options.clone()
    };
    let mut unguarded = stitch_zone_pads(&board, Some(&anchor_options)).unwrap().board;
    let index = RoutingClearanceIndex::new(&unguarded).unwrap();
    let path = copper_route::surface_path::surface_path_detour(
        &unguarded,
        &index,
        "GND",
        mm(7.8, 8.0),
        mm(9.3, 6.5),
        nm(0.2),
        CopperLayer::Front,
        &unguarded.tracks,
        0,
        nm(0.5),
    )
    .unwrap()
    .expect("old pending-anchor path is physically legal");
    drop(index);
    unguarded.tracks.extend(path);
    assert!(signal_exits(&unguarded).is_empty(), "old anchor retry closes the sampled exit");
    let stitched = stitch_zone_pads(&board, Some(&options)).unwrap();
    assert!(stitched.stitched_pads.contains(&pad_ref("ANCHOR", "1")));
    assert!(stitched.pending_pads.is_empty());
    assert!(stitched.stitched_pads.contains(&pad_ref("U", "1")));
    assert!(signal_exits(&stitched.board).is_empty());
    assert!(hard_findings(&stitched.board).is_empty(), "{:?}", hard_findings(&stitched.board));
}

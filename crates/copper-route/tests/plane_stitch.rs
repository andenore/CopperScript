//! `stitch_zone_pads` behaviour, ported from `tests/test_plane_stitch.py`.

mod support_planes;

use copper_physical::{CopperLayer, DesignRules, Point};
use copper_route::plane::{PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads};
use support_planes::*;

const STITCH_DRC: [&str; 4] = ["DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-HOLE-CLEARANCE"];
const ESCAPE_DRC: [&str; 4] = ["DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE"];

/// `_plane_board`: two one-pad GND connectors over an In1.Cu ground plane.
fn plane_board() -> copper_pcb::board::PhysicalBoard {
    let mut board = board("PlaneStitch", 20.0, 12.0);
    add_footprint(
        &mut board,
        footprint("test/one-pad", vec![pad("1", Point::new(0, 0), size(0.6, 0.6))], size(1.0, 1.0)),
    );
    board.placements = vec![place("J1", "test/one-pad", mm(3.0, 6.0)), place("J2", "test/one-pad", mm(17.0, 6.0))];
    board.nets = vec![net("GND", &[("J1", "1"), ("J2", "1")])];
    board.stackup = four_layers();
    board.zones = vec![zone("ground-plane", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 19.0, 11.0))];
    board
}

fn stitch(board: &copper_pcb::board::PhysicalBoard, options: PlaneStitchOptions) -> PlaneStitchResult {
    stitch_zone_pads(board, Some(&options)).expect("stitch")
}

fn reuse_first() -> PlaneStitchOptions {
    PlaneStitchOptions { preferred_ground_pads: Some(Default::default()), ..Default::default() }
}

fn gnd_via(x: f64, y: f64) -> copper_pcb::board::Via {
    via("GND", mm(x, y), nm(0.6), nm(0.3))
}

fn tiny_zone_around_j1() -> Vec<Point> {
    rect(2.7, 5.7, 3.3, 6.3)
}

#[test]
fn stitches_surface_pads_without_claiming_plane_fill() {
    let result = stitch_zone_pads(&plane_board(), None).unwrap();
    assert!(result.complete());
    assert_eq!(result.stitched_pads.len(), 2);
    assert_eq!(result.added_track_count, 2);
    assert_eq!(result.added_via_count, 2);
    assert_eq!(result.board.metadata["fabrication_ready"], "false");
    let findings = drc_codes(&result.board);
    assert!(!findings.contains("DRC-SHORT") && !findings.contains("DRC-CLEARANCE"));
    assert!(findings.contains("DRC-OPEN-NET"), "the plane fill is not claimed: {findings:?}");
}

#[test]
fn multiple_zones_on_same_net_do_not_duplicate_stitches() {
    let mut board = plane_board();
    let mut second = board.zones[0].clone();
    second.id = "second-plane".to_owned();
    board.zones.push(second);
    let result = stitch_zone_pads(&board, None).unwrap();
    assert_eq!(result.added_via_count, 2);
    assert_eq!(result.stitched_pads.len(), 2);
}

#[test]
fn existing_same_net_via_is_reused_not_drilled_twice() {
    let board = plane_board();
    let first = stitch_zone_pads(&board, None).unwrap();
    let mut seeded = board.clone();
    seeded.vias = vec![first.board.vias[0].clone()];
    let result = stitch(&seeded, reuse_first());
    assert!(result.complete());
    assert_eq!(result.added_track_count, 2);
    assert_eq!(result.added_via_count, 1);
    assert_eq!(result.board.vias.len(), 2);
}

#[test]
fn preferred_ground_pad_chooses_short_local_via_over_long_reuse() {
    let mut board = plane_board();
    board.vias = vec![gnd_via(5.0, 6.0)];
    let only = Some(pads(&[("J1", "1")]));
    let baseline = stitch(
        &board,
        PlaneStitchOptions {
            only_pads: only.clone(),
            preferred_ground_pads: Some(Default::default()),
            ..Default::default()
        },
    );
    let preferred_options =
        PlaneStitchOptions { only_pads: only.clone(), preferred_ground_pads: only.clone(), ..Default::default() };
    let preferred = stitch(&board, preferred_options.clone());

    assert_eq!(baseline.added_via_count, 0);
    assert!(preferred.complete() && preferred.added_via_count == 1);
    // GND pads are preferred by default.
    let default = stitch(&board, PlaneStitchOptions { only_pads: only, ..Default::default() });
    assert_eq!(default.board, preferred.board);
    let last = preferred.board.vias.last().unwrap().position;
    assert!(last == mm(2.0, 6.0) || last == mm(4.0, 6.0), "{last:?}");
    assert_eq!(stitch(&board, preferred_options).board, preferred.board, "deterministic");
    assert_drc_clear(&preferred.board, &STITCH_DRC);
}

#[test]
fn preferred_ground_pad_reuses_short_existing_via() {
    let mut board = plane_board();
    let existing = gnd_via(4.0, 6.0);
    board.vias = vec![existing.clone()];
    let only = Some(pads(&[("J1", "1")]));
    let result = stitch(
        &board,
        PlaneStitchOptions { only_pads: only.clone(), preferred_ground_pads: only, ..Default::default() },
    );
    assert!(result.complete() && result.added_via_count == 0);
    assert!(touches(&result.board.tracks, existing.position));
}

/// The Python test also counted `_candidate_points` through a monkeypatch;
/// that internal count is not observable here, only the chosen site.
#[test]
fn preferred_ground_search_picks_a_first_ring_site() {
    let only = Some(pads(&[("J1", "1")]));
    let result = stitch(
        &plane_board(),
        PlaneStitchOptions { only_pads: only.clone(), preferred_ground_pads: only, ..Default::default() },
    );
    assert!(result.complete() && result.added_via_count == 1);
    let last = result.board.vias.last().unwrap().position;
    assert!(last == mm(2.0, 6.0) || last == mm(4.0, 6.0), "{last:?}");
}

fn distribution_board(existing_x: f64) -> copper_pcb::board::PhysicalBoard {
    let mut board = plane_board();
    board.nets[0].name = "V3V3".to_owned();
    board.zones[0].net = "V3V3".to_owned();
    board.vias = vec![via("V3V3", mm(existing_x, 6.0), nm(0.6), nm(0.3))];
    board
}

#[test]
fn distribution_zone_prefers_short_pad_to_via() {
    for (existing_x, new_vias) in [(4.0, 0), (5.0, 1)] {
        let board = distribution_board(existing_x);
        let only = PlaneStitchOptions { only_pads: Some(pads(&[("J1", "1")])), ..Default::default() };
        let result = stitch(&board, only.clone());
        assert!(result.complete() && result.added_via_count == new_vias, "existing at {existing_x}");
        let last = result.board.vias.last().unwrap().position;
        assert!(last == mm(2.0, 6.0) || last == mm(4.0, 6.0), "{last:?}");
        let repeated = stitch(&result.board, only);
        assert_eq!((repeated.added_track_count, repeated.added_via_count), (0, 0));
        assert_drc_clear(&result.board, &STITCH_DRC);
    }
}

#[test]
fn distribution_zone_keeps_each_existing_local_contact() {
    let mut board = plane_board();
    board.placements[1].position = mm(5.0, 6.0);
    board.nets[0].name = "V3V3".to_owned();
    board.zones[0].net = "V3V3".to_owned();
    // Existing 200 um contacts must meet the declared profile; they are not
    // an implicit exemption from a wider one.
    board.rules.default_track_width_nm = nm(0.2);
    board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { width_nm: Some(nm(0.2)), ..rule("V3V3") }];
    board.tracks = vec![
        track("V3V3", mm(3.0, 6.0), mm(4.0, 6.0), nm(0.2), CopperLayer::Front),
        track("V3V3", mm(5.0, 6.0), mm(6.0, 6.0), nm(0.2), CopperLayer::Front),
    ];
    board.vias = vec![via("V3V3", mm(4.0, 6.0), nm(0.6), nm(0.3)), via("V3V3", mm(6.0, 6.0), nm(0.6), nm(0.3))];
    let result = stitch_zone_pads(&board, None).unwrap();
    assert!(result.complete());
    assert_eq!((result.added_track_count, result.added_via_count), (0, 0));
}

#[test]
fn preferred_ground_pads_do_not_share_a_via_when_local_site_is_free() {
    let mut board = plane_board();
    board.placements[1].position = mm(5.0, 6.0);
    board.vias = vec![gnd_via(4.0, 6.0)];
    let result = stitch(
        &board,
        PlaneStitchOptions { preferred_ground_pads: Some(pads(&[("J1", "1"), ("J2", "1")])), ..Default::default() },
    );
    assert!(result.complete() && result.added_via_count == 1);
    let last = result.board.vias.last().unwrap().position;
    assert!([mm(5.0, 5.0), mm(5.0, 7.0), mm(6.0, 6.0)].contains(&last), "{last:?}");
    assert_drc_clear(&result.board, &STITCH_DRC);
}

#[test]
fn preferred_ground_pad_keeps_its_existing_local_plane_contact() {
    let mut board = plane_board();
    board.zones[0].layers = vec![CopperLayer::In1, CopperLayer::Back];
    board.placements[1].position = mm(4.75, 5.25);
    board.tracks = vec![
        track("GND", mm(3.0, 6.0), mm(1.5, 6.0), nm(0.2), CopperLayer::Front),
        track("GND", mm(4.75, 5.25), mm(3.75, 5.25), nm(0.2), CopperLayer::Front),
    ];
    board.vias = vec![gnd_via(1.5, 6.0), gnd_via(3.75, 5.25)];
    let mut fill_only = keepout("back-fill-only", &[CopperLayer::Back], board.zones[0].outer.clone());
    fill_only.block_tracks = false;
    fill_only.block_vias = false;
    board.copper_keepouts = vec![fill_only];
    let result = stitch(
        &board,
        PlaneStitchOptions { preferred_ground_pads: Some(pads(&[("J1", "1"), ("J2", "1")])), ..Default::default() },
    );
    assert!(result.complete());
    assert_eq!((result.added_track_count, result.added_via_count), (0, 0));
    assert_eq!(result.board.tracks, board.tracks);
    assert_eq!(result.board.vias, board.vias);
}

#[test]
fn reuses_off_grid_same_net_via_before_adding_drill() {
    let mut board = plane_board();
    let existing = via("GND", mm(4.1, 6.3), nm(0.8), nm(0.4));
    board.vias = vec![existing.clone()];
    let result = stitch(&board, reuse_first());
    assert!(result.complete());
    assert!(result.board.tracks.iter().any(|track| track.start == mm(3.0, 6.0)));
    assert!(touches(&result.board.tracks, existing.position));
    assert!(result.board.tracks.iter().all(is_octilinear));
    assert_eq!(result.board.vias.len(), 2, "only J2 needs a new drill");
    assert_drc_clear(&result.board, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING"]);
}

#[test]
fn existing_via_can_be_reached_with_legal_two_segment_escape() {
    let mut board = plane_board();
    add_footprint(
        &mut board,
        footprint("test/blocker", vec![pad("1", Point::new(0, 0), size(0.6, 0.6))], size(1.0, 1.0)),
    );
    let existing = via("GND", mm(5.0, 8.0), nm(0.8), nm(0.4));
    board.placements.push(place("U1", "test/blocker", mm(4.0, 7.0)));
    board.nets.push(net("SIGNAL", &[("U1", "1")]));
    board.vias = vec![existing.clone()];

    let result = stitch(&board, reuse_first());
    assert!(result.complete());
    let reaching = result
        .board
        .tracks
        .iter()
        .filter(|track| track.start == existing.position || track.end == existing.position)
        .count();
    assert_eq!(reaching, 1);
    assert!(touches(&result.board.tracks, mm(3.0, 6.0)));
    assert_eq!(result.board.tracks.len(), 4, "chamfered elbow for J1, one leg for J2");
    assert!(result.board.tracks.iter().any(is_diagonal));
    assert_eq!(result.board.vias.len(), 2);
    let repeated = stitch_zone_pads(&result.board, None).unwrap();
    assert_eq!((repeated.added_track_count, repeated.added_via_count), (0, 0));
    assert_drc_clear(&result.board, &ESCAPE_DRC);
}

#[test]
fn blocked_elbows_can_use_bounded_three_segment_escape() {
    let mut board = plane_board();
    add_footprint(
        &mut board,
        footprint(
            "test/three-blockers",
            vec![
                pad("1", Point::new(0, 0), size(0.6, 0.6)),
                pad("2", mm(-1.0, 0.0), size(0.6, 0.6)),
                pad("3", mm(0.0, -1.0), size(0.6, 0.6)),
            ],
            size(2.0, 2.0),
        ),
    );
    let existing = via("GND", mm(5.0, 8.0), nm(0.8), nm(0.4));
    board.placements.push(place("U1", "test/three-blockers", mm(4.0, 7.0)));
    board.nets.push(net("SIGNAL", &[("U1", "1"), ("U1", "2"), ("U1", "3")]));
    board.vias = vec![existing.clone()];

    let result = stitch(&board, PlaneStitchOptions { maximum_detour_nm: nm(1.5), ..reuse_first() });
    let first_escape: Vec<_> = result
        .board
        .tracks
        .iter()
        .filter(|track| track.net == "GND" && track.start.x_nm.max(track.end.x_nm) < nm(10.0))
        .cloned()
        .collect();
    assert!(result.complete());
    assert_eq!(first_escape.len(), 5, "two clear detour corners become 45-degree chamfers");
    assert_eq!(first_escape.iter().filter(|track| is_diagonal(track)).count(), 2);
    assert!(touches(&first_escape, mm(3.0, 6.0)));
    assert!(touches(&first_escape, existing.position));
    assert_drc_clear(&result.board, &ESCAPE_DRC);
}

#[test]
fn dense_package_escape_can_route_around_pad_to_one_legal_via() {
    let mut board = plane_board();
    add_footprint(
        &mut board,
        footprint("test/escape-obstacle", vec![pad("1", Point::new(0, 0), size(1.0, 2.0))], size(1.0, 2.0)),
    );
    board.placements.push(place("U_BLOCK", "test/escape-obstacle", mm(5.0, 6.0)));
    board.nets.push(net("BLOCKED", &[("U_BLOCK", "1")]));
    board.zones[0].outer = rect(6.8, 5.8, 7.2, 6.2);
    let result = stitch(
        &board,
        PlaneStitchOptions {
            only_pads: Some(pads(&[("J1", "1")])),
            maximum_radius_nm: nm(5.0),
            maze_step_nm: nm(0.25),
            ..Default::default()
        },
    );
    assert!(result.complete());
    assert_eq!(result.board.vias.last().unwrap().position, mm(7.0, 6.0));
    assert!(result.added_track_count >= 3);
    assert_drc_clear(&result.board, &ESCAPE_DRC);
}

#[test]
fn every_separate_land_of_one_logical_ground_pad_is_escaped() {
    let mut board = plane_board();
    board.footprints.clear();
    add_footprint(
        &mut board,
        footprint(
            "test/two-ground-lands",
            vec![
                pad("G", mm(-2.0, 0.0), size(0.6, 0.6)),
                pad("S", Point::new(0, 0), size(0.6, 0.6)),
                pad("G", mm(2.0, 0.0), size(0.6, 0.6)),
            ],
            size(6.0, 2.0),
        ),
    );
    board.placements = vec![place("J1", "test/two-ground-lands", mm(10.0, 6.0))];
    board.nets = vec![net("GND", &[("J1", "G")]), net("SIGNAL", &[("J1", "S")])];

    let result = stitch_zone_pads(&board, None).unwrap();
    assert_eq!(result.stitched_pads, vec![pad_ref("J1", "G")]);
    assert!(result.pending_pads.is_empty());
    assert!(touches(&result.board.tracks, mm(8.0, 6.0)));
    assert!(touches(&result.board.tracks, mm(12.0, 6.0)));
    assert_eq!(result.board.vias.len(), 2);
    assert_drc_clear(&result.board, &ESCAPE_DRC);
}

#[test]
fn explicit_local_escape_width_preserves_clearance_checks() {
    let mut board = plane_board();
    board.rules = DesignRules {
        minimum_clearance_nm: nm(0.09),
        minimum_track_width_nm: nm(0.09),
        default_track_width_nm: nm(0.2),
        ..DesignRules::default()
    };
    let result = stitch(&board, PlaneStitchOptions { escape_width_nm: Some(nm(0.12)), ..Default::default() });
    assert!(result.complete());
    assert!(result.board.tracks.iter().all(|track| track.width_nm == nm(0.12)));
    assert_drc_clear(&result.board, &ESCAPE_DRC);
}

#[test]
fn local_escape_width_cannot_undercut_board_rule_floor() {
    let options = PlaneStitchOptions { escape_width_nm: Some(nm(0.12)), ..Default::default() };
    let error = stitch_zone_pads(&plane_board(), Some(&options)).unwrap_err();
    assert!(error.to_string().contains("below the board rule floor"), "{error}");
}

#[test]
fn blocked_land_can_reach_plane_through_escaped_neighbor() {
    let mut board = plane_board();
    let mut wall = keepout("local-via-exclusion", &board.stackup.copper_layers.clone(), rect(4.0, 1.0, 10.0, 11.0));
    wall.block_tracks = false;
    wall.block_zones = false;
    board.placements[1].position = mm(7.0, 6.0);
    board.copper_keepouts = vec![wall];

    let limited = stitch(&board, PlaneStitchOptions { maximum_contact_radius_nm: nm(3.0), ..Default::default() });
    let rescued = stitch(&board, PlaneStitchOptions { maximum_contact_radius_nm: nm(5.0), ..Default::default() });

    assert_eq!(limited.pending_pads, vec![pad_ref("J2", "1")]);
    assert!(rescued.complete());
    assert_eq!(rescued.added_via_count, 1);
    assert!(rescued.board.tracks.iter().any(|track| {
        (track.start == mm(3.0, 6.0) && track.end == mm(7.0, 6.0))
            || (track.start == mm(7.0, 6.0) && track.end == mm(3.0, 6.0))
    }));
    assert_drc_clear(
        &rescued.board,
        &["DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING", "DRC-BOARD-EDGE", "DRC-COPPER-KEEPOUT"],
    );
}

#[test]
fn selected_early_escape_is_not_duplicated_by_late_pass() {
    let board = plane_board();
    let early = stitch(&board, PlaneStitchOptions { only_pads: Some(pads(&[("J1", "1")])), ..Default::default() });
    let late = stitch_zone_pads(&early.board, None).unwrap();
    assert_eq!(early.stitched_pads, vec![pad_ref("J1", "1")]);
    assert_eq!(early.added_via_count, 1);
    assert!(late.complete());
    assert_eq!(late.added_track_count, 1);
    assert_eq!(late.added_via_count, 1);
}

#[test]
fn blocked_stitches_remain_explicitly_pending() {
    let mut board = plane_board();
    board.copper_keepouts = vec![keepout("all-vias", &board.stackup.copper_layers.clone(), rect(1.0, 1.0, 19.0, 11.0))];
    let result = stitch_zone_pads(&board, None).unwrap();
    assert!(!result.complete());
    assert_eq!(result.pending_pads.len(), 2);
    assert!(result.board.tracks.is_empty());
    assert!(result.board.vias.is_empty());
}

#[test]
fn large_land_uses_same_side_pour_without_inventing_via_or_fill() {
    for (same_side, block_fill) in [(true, false), (false, false), (true, true)] {
        let mut board = plane_board();
        board.footprints.clear();
        add_footprint(
            &mut board,
            footprint("test/large-land", vec![pad("1", Point::new(0, 0), size(12.0, 8.0))], size(12.0, 8.0)),
        );
        let mut back = board.zones[0].clone();
        back.layers = vec![CopperLayer::Back];
        board.placements = vec![place("J1", "test/large-land", mm(10.0, 6.0))];
        board.nets = vec![net("GND", &[("J1", "1")])];
        board.stackup = copper_pcb::board::Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]);
        board.zones = if same_side {
            let mut front = back.clone();
            front.id = "front-pour".to_owned();
            front.layers = vec![CopperLayer::Front];
            vec![back.clone(), front]
        } else {
            vec![back.clone()]
        };
        if block_fill {
            board.copper_keepouts = vec![keepout("no-front-fill", &[CopperLayer::Front], back.outer.clone())];
        }
        let result = stitch(&board, PlaneStitchOptions { include_surface_zones: true, ..Default::default() });
        assert_eq!(result.complete(), same_side && !block_fill, "same_side={same_side} block_fill={block_fill}");
        assert!(result.board.tracks.is_empty() && result.board.vias.is_empty());
        assert!(result.board.polygons.is_empty(), "no fill is invented");
        assert_eq!(result.board.metadata["fabrication_ready"], "false");
    }
}

#[test]
fn filled_capped_ground_via_in_pad_rescues_only_qualified_board() {
    let base = plane_board();
    let mut board = base.clone();
    board.placements.truncate(1);
    board.nets = vec![net("GND", &[("J1", "1")])];
    board.zones[0].outer = tiny_zone_around_j1();
    board.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    assert_eq!(stitch_zone_pads(&board, None).unwrap().pending_pads, vec![pad_ref("J1", "1")]);
    let in_pad = PlaneStitchOptions { ground_via_in_pad: true, ..Default::default() };
    let result = stitch(&board, in_pad.clone());
    assert!(result.complete());
    assert_eq!(result.added_track_count, 0);
    assert_eq!(result.added_via_count, 1);
    let via = &result.board.vias[0];
    assert_eq!(via.position, mm(3.0, 6.0));
    assert_eq!((via.size_nm, via.drill_nm), (nm(0.30), nm(0.20)));
    assert_eq!(via.finish, "filled-capped");
    assert_eq!(result.board.metadata["via_in_pad_count"], "1");
    assert_drc_clear(&result.board, &ESCAPE_DRC);
    let error = stitch_zone_pads(&base, Some(&in_pad)).unwrap_err();
    assert!(error.to_string().contains("six-layer"), "{error}");
}

fn foreign_signal_pad(board: &mut copper_pcb::board::PhysicalBoard, name: &str) {
    add_footprint(board, footprint(name, vec![pad("1", Point::new(0, 0), size(0.2, 0.2))], size(0.2, 0.2)));
}

#[test]
fn ground_via_in_pad_still_rejects_foreign_copper() {
    let mut board = plane_board();
    foreign_signal_pad(&mut board, "test/foreign-pad");
    board.placements.push(place("U1", "test/foreign-pad", mm(3.4, 6.0)));
    board.nets.push(net("SIGNAL", &[("U1", "1")]));
    board.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    // With a tiny zone around J1, only the centered via is a candidate.
    board.zones[0].outer = tiny_zone_around_j1();
    let result = stitch(&board, PlaneStitchOptions { ground_via_in_pad: true, ..Default::default() });
    assert!(result.pending_pads.contains(&pad_ref("J1", "1")));
}

#[test]
fn ground_via_in_pad_rejects_hole_clearance_even_when_annulus_clears() {
    let mut board = plane_board();
    foreign_signal_pad(&mut board, "test/nearby-signal");
    board.placements = vec![board.placements[0].clone(), place("U1", "test/nearby-signal", mm(3.5, 6.0))];
    board.nets = vec![net("GND", &[("J1", "1")]), net("SIGNAL", &[("U1", "1")])];
    board.zones[0].outer = tiny_zone_around_j1();
    board.tracks = vec![track("SIGNAL", mm(2.0, 6.4), mm(4.0, 6.4), nm(0.2), CopperLayer::In2)];
    board.rules =
        DesignRules { minimum_clearance_nm: nm(0.09), minimum_track_width_nm: nm(0.09), ..DesignRules::default() };
    board.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    let result = stitch(&board, PlaneStitchOptions { ground_via_in_pad: true, ..Default::default() });
    assert!(result.pending_pads.contains(&pad_ref("J1", "1")));
    let mut unsafe_board = board.clone();
    let mut capped = via("GND", mm(3.0, 6.0), nm(0.30), nm(0.20));
    capped.finish = "filled-capped".to_owned();
    unsafe_board.vias = vec![capped];
    assert!(drc_codes(&unsafe_board).contains("DRC-HOLE-CLEARANCE"));
}

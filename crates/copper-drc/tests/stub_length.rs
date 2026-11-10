//! `DRC-STUB-LENGTH`: `maximum_stub_length` against routed copper.

use copper_drc::{Finding, Policy, Severity, run_physical_drc};
use copper_pcb::board::{
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalHardMacro, PhysicalNet, Placement, RigidPlacementCluster,
    RigidPlacementMember, RouteKind, TrackArc, TrackSegment, Via,
};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size, footprint_geometry_digest};
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

const MM: i64 = 1_000_000;
const Y: i64 = 5 * MM;

fn footprint() -> PhysicalFootprint {
    let pad = FootprintPad {
        number: "1".to_owned(),
        position: Point::new(0, 0),
        size: Size { width_nm: 400_000, height_nm: 400_000 },
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::zero(),
        drill: None,
        roundrect_ratio_ppm: 250_000,
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    };
    PhysicalFootprint {
        name: "test/pad".to_owned(),
        pads: vec![pad],
        body_size: Size { width_nm: MM, height_nm: MM },
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    }
}

fn rule(net: &str, stub: Option<i64>) -> NetRoutingRule {
    NetRoutingRule {
        net: net.to_owned(),
        kind: RouteKind::General,
        priority: 0,
        width_nm: None,
        clearance_nm: None,
        allowed_layers: Vec::new(),
        max_vias: None,
        max_length_nm: None,
        differential_partner: None,
        pair_gap_nm: None,
        max_skew_nm: None,
        topology: "point_to_point".to_owned(),
        target_impedance_ohms: None,
        maximum_uncoupled_length_nm: None,
        maximum_stub_length_nm: stub,
        tuning_amplitude_limit_nm: None,
        require_return_vias: false,
        reserve_corridor: false,
        return_via_net: None,
        maximum_return_via_distance_nm: None,
        impedance_evidence_digest: None,
        return_via_policy: copper_pcb::board::ReturnViaPolicy::Always,
        shared_reference_layer: None,
        target_single_ended_ohms: None,
        impedance_tolerance_percent: None,
        layer_group: None,
        breakout_length_nm: None,
        breakout_width_nm: None,
        breakout_gap_nm: None,
        breakout_clearance_nm: None,
        tuning_style: copper_pcb::board::TuningStyle::Bumps,
        tuning_spacing_nm: None,
        tuning_group: None,
        corner_style: copper_pcb::board::CornerStyle::Octilinear,
        corner_radius_nm: None,
    }
}

fn track(start: (i64, i64), end: (i64, i64)) -> TrackSegment {
    TrackSegment {
        net: "N".to_owned(),
        start: Point::new(start.0, start.1),
        end: Point::new(end.0, end.1),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    }
}

fn via(x: i64, y: i64) -> Via {
    Via {
        net: "N".to_owned(),
        position: Point::new(x, y),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "tented".to_owned(),
    }
}

/// Net `N` with pads at x = 2 mm and 12 mm (and 8 mm further up at x = 7 mm
/// for `third`), joined by one straight track along y = 5 mm, with the given
/// limit.
fn board(limit: Option<i64>, third: bool) -> PhysicalBoard {
    let zero = Decimal::zero();
    let at = |reference: &str, x: i64, y: i64| {
        Placement::new(reference, "test/pad", Point::new(x, y), &zero).expect("placement")
    };
    let mut placements = vec![at("A", 2 * MM, Y), at("B", 12 * MM, Y)];
    let mut pads = vec![PadReference::new("A", "1"), PadReference::new("B", "1")];
    if third {
        placements.push(at("C", 7 * MM, Y + 8 * MM));
        pads.push(PadReference::new("C", "1"));
    }
    let footprint = footprint();
    PhysicalBoard {
        name: "Stubs".to_owned(),
        outline: BoardOutline::rectangle(30 * MM, 20 * MM, None).expect("outline"),
        footprints: IndexMap::from([(footprint.name.clone(), footprint)]),
        placements,
        nets: vec![PhysicalNet { name: "N".to_owned(), pads }],
        stackup: copper_pcb::board::Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
        rules: copper_physical::DesignRules::default(),
        tracks: vec![track((2 * MM, Y), (12 * MM, Y))],
        vias: Vec::new(),
        arcs: Vec::new(),
        metadata: IndexMap::new(),
        regions: Vec::new(),
        keepouts: Vec::new(),
        placement_rules: Vec::new(),
        relative_rules: Vec::new(),
        placement_groups: Vec::new(),
        power_domains: Vec::new(),
        net_routing_rules: limit.map(|limit| rule("N", Some(limit))).into_iter().collect(),
        zones: Vec::new(),
        polygons: Vec::new(),
        copper_keepouts: Vec::new(),
        rigid_clusters: Vec::new(),
        hard_macros: Vec::new(),
        materialized_macros: Vec::new(),
        via_in_pad_rules: Vec::new(),
        mechanical_holes: Vec::new(),
        mechanical_slots: Vec::new(),
        datums: Vec::new(),
        boundary_edges: Vec::new(),
        attachments: Vec::new(),
        component_heights: Vec::new(),
        body_overhangs: Vec::new(),
        assembly_envelopes: Vec::new(),
        assembly_access: Vec::new(),
        match_groups: Vec::new(),
        component_hole_clearances: Vec::new(),
        decoupling_links: Vec::new(),
        silkscreen: Vec::new(),
        power_feeds: Vec::new(),
        mechanical_references: Vec::new(),
    }
}

fn stub_findings(board: &PhysicalBoard) -> Vec<Finding> {
    let report = run_physical_drc(board, Policy { require_completed_detailed_route: false, fail_on_warnings: false })
        .expect("DRC");
    report.findings.into_iter().filter(|f| f.code == "DRC-STUB-LENGTH").collect()
}

/// A dangling track of `length` leaving the main track at x = 7 mm.
fn with_stub(length: i64, limit: Option<i64>) -> PhysicalBoard {
    let mut board = board(limit, false);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + length)));
    board
}

#[test]
fn a_stub_under_or_at_the_limit_passes() {
    assert!(stub_findings(&with_stub(MM - 1, Some(MM))).is_empty());
    assert!(stub_findings(&with_stub(MM, Some(MM))).is_empty());
}

#[test]
fn a_stub_over_the_limit_is_one_finding_naming_net_length_and_limit() {
    let found = stub_findings(&with_stub(MM + 1_000, Some(MM)));
    assert_eq!(found.len(), 1, "{found:?}");
    let finding = &found[0];
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.nets, vec!["N".to_owned()]);
    assert_eq!((finding.measured_nm, finding.required_nm), (Some(MM + 1_000), Some(MM)));
    assert_eq!(finding.objects, vec!["track:1".to_owned()]);
    assert!(finding.message.contains("'N'"), "{}", finding.message);
}

#[test]
fn a_net_without_a_rule_or_limit_produces_nothing() {
    assert!(stub_findings(&with_stub(5 * MM, None)).is_empty());
    let mut no_limit = with_stub(5 * MM, None);
    no_limit.net_routing_rules = vec![rule("N", None)];
    assert!(stub_findings(&no_limit).is_empty());
    let mut other_net = with_stub(5 * MM, Some(MM));
    other_net.net_routing_rules = vec![rule("OTHER", Some(MM))];
    assert!(stub_findings(&other_net).is_empty());
}

#[test]
fn a_clean_route_has_no_stub() {
    assert!(stub_findings(&board(Some(1), false)).is_empty());
}

#[test]
fn a_stub_of_several_segments_is_measured_along_the_copper() {
    let mut board = board(Some(MM), false);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 600_000)));
    board.tracks.push(track((7 * MM, Y + 600_000), (7 * MM + 600_000, Y + 600_000)));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(1_200_000));
    assert_eq!(found[0].objects, vec!["track:2".to_owned()]);
}

#[test]
fn a_fork_in_a_stub_counts_its_longest_branch() {
    let mut board = board(Some(MM), false);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 400_000)));
    board.tracks.push(track((7 * MM, Y + 400_000), (7 * MM + 300_000, Y + 400_000)));
    board.tracks.push(track((7 * MM, Y + 400_000), (7 * MM, Y + 1_100_000)));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(1_100_000));
    assert_eq!(found[0].objects, vec!["track:3".to_owned()]);
}

#[test]
fn a_stub_with_an_arc_is_measured_at_the_arc_length() {
    // A quarter circle of radius 1 mm leaving the main track at a right angle,
    // then 0.5 mm of track.
    let mut board = board(Some(2 * MM), false);
    let half = 707_107;
    board.arcs.push(TrackArc {
        net: "N".to_owned(),
        start: Point::new(7 * MM, Y),
        mid: Point::new(8 * MM - half, Y + half),
        end: Point::new(8 * MM, Y + MM),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    });
    board.tracks.push(track((8 * MM, Y + MM), (8 * MM + 500_000, Y + MM)));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    let measured = found[0].measured_nm.expect("measured");
    let expected = 1_570_796 + 500_000;
    assert!((measured - expected).abs() < 1_000, "{measured} against {expected}");
    // The same copper under a limit just above it passes.
    board.net_routing_rules = vec![rule("N", Some(expected + 1_000))];
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn an_unused_via_at_the_end_of_a_stub_ends_it_without_barrel_length() {
    let mut board = board(Some(500_000), false);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 800_000)));
    board.vias.push(via(7 * MM, Y + 800_000));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(800_000));
    assert_eq!(found[0].objects, vec!["via:0".to_owned()]);
}

#[test]
fn a_via_that_carries_the_route_to_another_layer_is_not_a_stub() {
    let mut board = board(Some(1), false);
    board.tracks.clear();
    board.tracks.push(track((2 * MM, Y), (5 * MM, Y)));
    let mut back = track((5 * MM, Y), (12 * MM, Y));
    back.layer = CopperLayer::Back;
    board.tracks.push(back);
    board.vias.push(via(5 * MM, Y));
    // Pad B is on the front, so the back track returns through a via in the pad.
    board.vias.push(via(12 * MM, Y));
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn a_branch_to_another_pad_is_not_a_stub() {
    let mut board = board(Some(1), true);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 8 * MM)));
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn an_unconnected_net_reports_no_stub() {
    let mut board = with_stub(5 * MM, Some(MM));
    board.tracks.remove(0);
    board.tracks.push(track((2 * MM, Y), (4 * MM, Y)));
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn a_stub_leaving_a_corner_of_the_main_route_is_found() {
    // The main route is two tracks meeting at x = 7 mm, where the stub starts.
    let mut board = board(Some(MM), false);
    board.tracks = vec![track((2 * MM, Y), (7 * MM, Y)), track((7 * MM, Y), (12 * MM, Y))];
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 1_500_000)));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(1_500_000));
}

#[test]
fn a_via_mid_track_splits_it_and_the_overhang_is_measured_from_the_via() {
    // Pad A, front track past a via at x = 7 mm (2 mm of overhang to x = 9 mm),
    // back track from the via to pad B.
    let mut board = board(Some(MM), false);
    board.tracks = vec![track((2 * MM, Y), (9 * MM, Y))];
    let mut back = track((7 * MM, Y), (12 * MM, Y));
    back.layer = CopperLayer::Back;
    board.tracks.push(back);
    board.vias.push(via(7 * MM, Y));
    board.vias.push(via(12 * MM, Y));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(2 * MM));
    board.net_routing_rules = vec![rule("N", Some(2 * MM))];
    assert!(stub_findings(&board).is_empty());
    // Without the overhang the route has no stub at all.
    board.tracks[0].end = Point::new(7 * MM, Y);
    board.net_routing_rules = vec![rule("N", Some(1))];
    assert!(stub_findings(&board).is_empty());
}

/// Net `N` with only pad A, a feed track from it to x = 7 mm, and a synthesized
/// antenna (hard macro `ant`, recorded in the metadata) from there to the right.
fn antenna_board() -> PhysicalBoard {
    let mut board = board(Some(MM), false);
    board.nets[0].pads = vec![PadReference::new("A", "1")];
    let antenna = vec![track((7 * MM, Y), (11 * MM, Y)), track((11 * MM, Y), (11 * MM, Y + 2 * MM))];
    board.tracks = vec![track((2 * MM, Y), (7 * MM, Y))];
    board.tracks.extend(antenna.clone());
    let zero = Decimal::zero();
    let member = RigidPlacementMember::new(
        "A",
        "test/pad",
        &footprint_geometry_digest(&board.footprints["test/pad"]),
        Point::new(0, 0),
        &zero,
    )
    .expect("member");
    board.rigid_clusters.push(
        RigidPlacementCluster {
            name: "ant".to_owned(),
            anchor_reference: "A".to_owned(),
            anchor_pad: None,
            members: vec![member],
            source: "Synthetic antenna".to_owned(),
            allowed_rotations: vec![zero],
            keepouts: Vec::new(),
            internal_clearance_nm: None,
            edge_openings: Vec::new(),
        }
        .normalized()
        .expect("cluster"),
    );
    let local = |point: Point| Point::new(point.x_nm - 2 * MM, point.y_nm - Y);
    board.hard_macros.push(PhysicalHardMacro {
        cluster: "ant".to_owned(),
        asset_sha256: "0".repeat(64),
        tracks: antenna
            .iter()
            .map(|t| TrackSegment { start: local(t.start), end: local(t.end), ..t.clone() })
            .collect(),
        vias: Vec::new(),
        ports: Vec::new(),
        protected_regions: Vec::new(),
        required_layers: vec![CopperLayer::Front],
        pad_bindings: Vec::new(),
        isolated_pads: Vec::new(),
        zones: Vec::new(),
        plane_returns: Vec::new(),
        polygons: Vec::new(),
        width_contracts: Vec::new(),
    });
    board.metadata.insert("antenna_synthesis".to_owned(), "[{\"name\": \"ant\"}]".to_owned());
    let owned = copper_pcb::antenna::antenna_tracks(&board).expect("antenna tracks");
    assert_eq!(owned.len(), 2, "the macro's tracks land on the board's antenna copper");
    board
}

#[test]
fn a_feed_ending_on_antenna_copper_is_not_a_stub() {
    assert!(stub_findings(&antenna_board()).is_empty());
}

#[test]
fn a_dangling_branch_off_an_antenna_feed_is_still_a_stub() {
    let mut board = antenna_board();
    board.tracks.push(track((4 * MM, Y), (4 * MM, Y + 1_500_000)));
    let found = stub_findings(&board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].measured_nm, Some(1_500_000));
}

/// The one finding on `board`, which must be the only one.
fn only_finding(board: &PhysicalBoard) -> Finding {
    let mut found = stub_findings(board);
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

fn near(measured: Option<i64>, expected: i64, tolerance: i64) {
    let measured = measured.expect("measured");
    assert!((measured - expected).abs() <= tolerance, "{measured} against {expected}");
}

// --- crossings ---------------------------------------------------------------

#[test]
fn a_track_crossing_the_route_in_an_x_leaves_a_stub_on_each_side() {
    // 2 mm hang below the main route and 3 mm above it; no end is near it.
    let mut board = board(Some(MM), false);
    board.tracks.push(track((7 * MM, Y - 2 * MM), (7 * MM, Y + 3 * MM)));
    assert_eq!(only_finding(&board).measured_nm, Some(3 * MM));
    board.net_routing_rules = vec![rule("N", Some(3 * MM))];
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn a_crossing_with_both_sides_within_the_limit_is_quiet() {
    let mut board = board(Some(MM), false);
    board.tracks.push(track((7 * MM, Y - 500_000), (7 * MM, Y + 600_000)));
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn a_branch_stopping_on_the_far_edge_of_the_route_is_a_tee_not_a_stub() {
    // Pad C feeds a track that stops on the far edge of the main route.
    let mut board = board(Some(1), true);
    board.tracks.push(track((7 * MM, Y + 8 * MM), (7 * MM, Y - 100_000)));
    assert!(stub_findings(&board).is_empty());
}

#[test]
fn an_arc_crossing_the_route_leaves_a_stub_on_each_side() {
    // 120 degrees of a 2 mm circle centred on the main route, crossing it once.
    let mut board = board(Some(MM), false);
    board.arcs.push(TrackArc {
        net: "N".to_owned(),
        start: Point::new(7 * MM, Y - 1_732_051),
        mid: Point::new(8 * MM, Y),
        end: Point::new(7 * MM, Y + 1_732_051),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    });
    near(only_finding(&board).measured_nm, 2_094_395, 10_000);
    board.net_routing_rules = vec![rule("N", Some(2_100_000))];
    assert!(stub_findings(&board).is_empty());
}

// --- overlapping copper and loops ---------------------------------------------

#[test]
fn a_redundant_segment_over_the_middle_of_a_stub_does_not_hide_it() {
    let mut board = with_stub(2 * MM, Some(MM));
    board.tracks.push(track((7 * MM, Y + 500_000), (7 * MM, Y + 1_500_000)));
    assert_eq!(only_finding(&board).measured_nm, Some(2 * MM));
}

#[test]
fn a_redundant_segment_running_past_the_end_of_a_stub_lengthens_it() {
    let mut board = with_stub(2 * MM, Some(MM));
    board.tracks.push(track((7 * MM, Y + MM), (7 * MM, Y + 2_500_000)));
    assert_eq!(only_finding(&board).measured_nm, Some(2_500_000));
}

#[test]
fn an_exact_duplicate_of_a_stub_is_one_stub() {
    let mut board = with_stub(2 * MM, Some(MM));
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + 2 * MM)));
    assert_eq!(only_finding(&board).measured_nm, Some(2 * MM));
}

#[test]
fn a_parallel_segment_touching_a_stub_is_one_stub() {
    let mut board = with_stub(2 * MM, Some(MM));
    board.tracks.push(track((7 * MM + 100_000, Y), (7 * MM + 100_000, Y + 2 * MM)));
    near(only_finding(&board).measured_nm, 2 * MM, 50_000);
}

#[test]
fn a_lasso_is_measured_to_the_farthest_junction_of_its_loop() {
    // A 1 mm neck, then a triangle: 1 mm across, 1 mm up, and 1.414 mm back.
    let mut board = board(Some(MM), false);
    board.tracks.push(track((7 * MM, Y), (7 * MM, Y + MM)));
    board.tracks.push(track((7 * MM, Y + MM), (8 * MM, Y + MM)));
    board.tracks.push(track((8 * MM, Y + MM), (8 * MM, Y + 2 * MM)));
    board.tracks.push(track((8 * MM, Y + 2 * MM), (7 * MM, Y + MM)));
    near(only_finding(&board).measured_nm, MM + 1_414_214, 2);
}

#[test]
fn a_loop_between_two_pads_is_route_copper() {
    let mut board = board(Some(1), false);
    board.tracks.push(track((2 * MM, Y), (2 * MM, Y + 1_500_000)));
    board.tracks.push(track((2 * MM, Y + 1_500_000), (12 * MM, Y + 1_500_000)));
    board.tracks.push(track((12 * MM, Y + 1_500_000), (12 * MM, Y)));
    assert!(stub_findings(&board).is_empty());
}

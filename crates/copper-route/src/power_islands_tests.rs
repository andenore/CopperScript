use super::*;
use crate::test_support_planes::support_planes::*;
use crate::via_lattice::LatticeSite;
use copper_pcb::board::DecouplingLink;
use copper_physical::footprint::PadShape;

fn source() -> PhysicalBoard {
    let mut result = board("accepted-power-sites", 12.0, 9.0);
    let mut balls = Vec::new();
    for y in 0..3 {
        for x in 0..3 {
            let mut ball =
                pad(&(3 * y + x + 1).to_string(), Point::new((x - 1) * 750_000, (y - 1) * 750_000), size(0.25, 0.25));
            ball.shape = PadShape::Circle;
            balls.push(ball);
        }
    }
    add_footprint(&mut result, footprint("bga", balls, size(2.5, 2.5)));
    add_footprint(
        &mut result,
        footprint(
            "cap",
            vec![pad("1", mm(-0.5, 0.0), size(0.5, 0.5)), pad("2", mm(0.5, 0.0), size(0.5, 0.5))],
            size(1.5, 0.8),
        ),
    );
    result.placements = vec![place("U", "bga", mm(4.0, 4.0)), place("C", "cap", mm(9.0, 6.0))];
    result.nets = vec![net("P", &[("U", "1"), ("U", "9"), ("C", "1")]), net("GND", &[("C", "2")])];
    result.decoupling_links = vec![DecouplingLink {
        capacitor: pad_ref("C", "1"),
        target: pad_ref("U", "1"),
        return_pad: pad_ref("C", "2"),
        net: "P".to_owned(),
        return_net: "GND".to_owned(),
    }];
    result.rules.minimum_clearance_nm = 100_000;
    result.rules.minimum_hole_clearance_nm = 100_000;
    result.rules.minimum_track_width_nm = 200_000;
    result.rules.default_track_width_nm = 200_000;
    result.rules.default_via_size_nm = 450_000;
    result.rules.default_via_drill_nm = 200_000;
    let mut host = zone("reference", "GND", &[CopperLayer::Back], rect(0.0, 0.0, 12.0, 9.0));
    host.holes = vec![rect(6.0, 3.0, 7.0, 4.0)];
    result.zones = vec![host];
    result
}

fn contribution() -> (Vec<PackageLattice>, Vec<TrackSegment>, Vec<Via>) {
    let position = mm(2.875, 2.875);
    let package = PackageLattice {
        component: "U".to_owned(),
        pitch_nm: 750_000,
        sites: vec![LatticeSite {
            pad: pad_ref("U", "1"),
            net: "P".to_owned(),
            position,
            cell: (-1, -1),
            preferred: true,
            track_range: 0..1,
            via_occurrence: 0,
        }],
        unsited: vec![(pad_ref("U", "9"), "no legal site".to_owned())],
        coverage: Vec::new(),
    };
    let stub = track("P", mm(3.25, 3.25), position, 200_000, CopperLayer::Front);
    let via = Via {
        net: "P".to_owned(),
        position,
        size_nm: 450_000,
        drill_nm: 200_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    };
    (vec![package], vec![stub], vec![via])
}

#[test]
fn accepted_power_sites_keep_unsited_contacts_and_source_cuts_explicit() {
    let source = source();
    let before = source.clone();
    let (packages, tracks, vias) = contribution();
    let result = plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &tracks, &vias).unwrap();
    assert_eq!(source, before);
    assert_eq!(result.accepted_power_sites, 1);
    assert_eq!(
        result.missing_contacts,
        vec![MissingPowerContact { pad: pad_ref("U", "9"), net: "P".to_owned(), reason: "no legal site".to_owned() }]
    );
    assert!(!result.partition.zones.is_empty());
    assert!(result.partition.zones.iter().all(|zone| zone.net == "P" && zone.layers == vec![CopperLayer::Back]));
    // The host's authored cut is excluded even when the partition grows corridors.
    let in_cut = mm(6.5, 3.5);
    assert!(result.partition.zones.iter().all(|zone| !polygon_inside(
        &zone.outer,
        in_cut.x_nm as f64,
        in_cut.y_nm as f64
    ) || zone.holes.iter().any(|hole| polygon_inside(
        hole,
        in_cut.x_nm as f64,
        in_cut.y_nm as f64
    ))));
}

#[test]
fn accepted_power_sites_never_fabricate_unsited_ball_cells() {
    let source = source();
    let result = plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &[], &[], &[]).unwrap();
    assert_eq!(result.accepted_power_sites, 0);
    assert_eq!(result.missing_contacts.len(), 2);
    assert!(result.partition.zones.is_empty());
    // This distinguishes the new source/catalogue contract from legacy fallback.
    assert!(!plan_ball_grid_islands(&source, CopperLayer::Back).unwrap().zones.is_empty());
}

#[test]
fn accepted_power_sites_reject_missing_ambiguous_or_unowned_contributions() {
    let source = source();
    let (packages, tracks, vias) = contribution();
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &[], &vias).is_err());
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &tracks, &[]).is_err());
    let mut duplicate = tracks.clone();
    duplicate.extend_from_slice(&tracks);
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &duplicate, &vias).is_err());
    let mut extra = tracks.clone();
    extra.push(track("P", mm(9.0, 7.0), mm(10.0, 7.0), 200_000, CopperLayer::Front));
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &extra, &vias).is_err());
    let mut wrong = packages.clone();
    wrong[0].sites[0].net = "GND".to_owned();
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &wrong, &tracks, &vias).is_err());
    let mut wrong = packages.clone();
    wrong[0].sites[0].cell = (1, 1);
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &wrong, &tracks, &vias).is_err());
}

#[test]
fn accepted_power_sites_freshly_reject_illegal_copper_despite_matching_catalogue() {
    let mut source = source();
    let (packages, tracks, vias) = contribution();
    // A new foreign pad occupies the declared via site; ownership alone is no
    // legality proof. Re-admission must see the changed source geometry.
    add_footprint(&mut source, footprint("blocker", vec![pad("1", mm(0.0, 0.0), size(0.3, 0.3))], size(0.3, 0.3)));
    source.placements.push(place("BLOCK", "blocker", vias[0].position));
    source.nets.iter_mut().find(|net| net.name == "GND").unwrap().pads.push(pad_ref("BLOCK", "1"));
    let error = plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &tracks, &vias).unwrap_err();
    assert!(error.to_string().contains("fresh physical DRC"), "{error}");
}

#[test]
fn accepted_power_sites_consume_actual_rule_qualified_piece_ranges_from_the_allocator() {
    let mut source = source();
    source.rules.minimum_track_width_nm = 100_000;
    source.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        kind: copper_pcb::board::RouteKind::Power,
        width_nm: Some(240_000),
        breakout_length_nm: Some(200_000),
        breakout_width_nm: Some(100_000),
        ..rule("P")
    });
    let lattice = crate::via_lattice::plan_via_lattices(&source, None, 2, 1).unwrap();
    assert_eq!(lattice.lattices[0].sites.len(), 2);
    assert!(lattice.tracks.iter().any(|piece| piece.width_nm == 100_000));
    assert!(lattice.tracks.iter().any(|piece| piece.width_nm == 240_000));
    let result = plan_ball_grid_islands_from_lattice(
        &source,
        CopperLayer::Back,
        &lattice.lattices,
        &lattice.tracks,
        &lattice.vias,
    )
    .expect("the power consumer uses the allocator's qualified pieces");
    assert_eq!(result.accepted_power_sites, 2);
    assert!(result.missing_contacts.is_empty());
    assert!(!result.partition.zones.is_empty());
}

#[test]
fn accepted_power_sites_do_not_trust_range_or_barrel_locators_as_proof() {
    let source = source();
    let (mut packages, tracks, vias) = contribution();
    packages[0].sites[0].track_range = 0..2;
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &tracks, &vias).is_err());
    packages[0].sites[0].track_range = 0..1;
    packages[0].sites[0].via_occurrence = 1;
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &tracks, &vias).is_err());
    packages[0].sites[0].via_occurrence = 0;
    let mut wrong_width = tracks.clone();
    wrong_width[0].width_nm += 10_000;
    assert!(plan_ball_grid_islands_from_lattice(&source, CopperLayer::Back, &packages, &wrong_width, &vias).is_err());
}

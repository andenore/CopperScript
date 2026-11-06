use super::*;
use crate::test_support_planes::support_planes as scene;
use copper_pcb::board::{MacroPort, PhysicalHardMacro, RigidPlacementCluster, RigidPlacementMember};
use copper_physical::footprint::footprint_geometry_digest;
use copper_units::Decimal;

fn inner_board(numbers: &[i64]) -> PhysicalBoard {
    let mut board = scene::ball_grid(6, 0.45, &[]);
    board.nets.retain(|net| numbers.iter().any(|number| net.name == format!("N{number}")));
    board
}

fn plan(board: &PhysicalBoard, surface: Option<&IndexSet<PadReference>>) -> LatticeResult {
    plan_via_lattices(board, surface, 2, 1).expect("bounded local allocation")
}

fn coverage<'a>(result: &'a LatticeResult, pad: &str) -> &'a BallCoverage {
    result
        .lattices
        .iter()
        .find(|package| package.component == "U")
        .expect("recognized U")
        .coverage
        .iter()
        .find(|item| item.pad.pad == pad)
        .expect("connected ball census entry")
}

#[test]
fn retained_signal_handoff_requires_exact_lattice_ownership_and_fresh_copper() {
    use crate::fixed_prefix::{AdmittedFixedPrefix, FixedContribution, FixedPrefixRole};
    let mut source = inner_board(&[15]);
    source.placements.iter_mut().find(|pose| pose.reference == "J").unwrap().position.x_nm = 25_000_000;
    let plan = plan(&source, None);
    let contribution = FixedContribution {
        id: "sites".into(),
        role: FixedPrefixRole::ViaLattice,
        tracks: plan.tracks.clone(),
        vias: plan.vias.clone(),
    };
    let prefix = AdmittedFixedPrefix::admit(&source, std::slice::from_ref(&contribution)).unwrap();
    let handoff = retained_signal_accesses(prefix.board(), &plan.lattices, &plan.tracks, &plan.vias, &prefix).unwrap();
    assert!(handoff.pending.is_empty(), "{:?}", handoff.pending);
    assert_eq!(handoff.accesses.len(), 1);
    assert!(!handoff.accesses.values().next().unwrap().via_layers.is_empty());
    let mut duplicate = prefix.board().clone();
    duplicate.vias.push(plan.vias[0].clone());
    let stale = retained_signal_accesses(&duplicate, &plan.lattices, &plan.tracks, &plan.vias, &prefix).unwrap();
    assert!(stale.accesses.is_empty() && !stale.pending.is_empty());
    let mut ordinary = contribution;
    ordinary.role = FixedPrefixRole::OrdinaryEscape;
    let prefix = AdmittedFixedPrefix::admit(&source, &[ordinary]).unwrap();
    let wrong_owner =
        retained_signal_accesses(prefix.board(), &plan.lattices, &plan.tracks, &plan.vias, &prefix).unwrap();
    assert!(
        wrong_owner.accesses.is_empty() && !wrong_owner.pending.is_empty(),
        "matching geometry is not lattice ownership"
    );
}

fn assert_pending(result: &LatticeResult, pad: &str, reason: &str) {
    assert_eq!(coverage(result, pad).status, BallSiteStatus::Pending(reason.to_owned()));
    assert!(result.lattices[0].unsited.iter().any(|(reference, why)| reference.pad == pad && why == reason));
    assert!(!result.lattices[0].sites.iter().any(|site| site.pad.pad == pad));
}

pub(crate) fn bind_materialized_launch(board: &mut PhysicalBoard) {
    let pose = board.placements.iter().find(|pose| pose.reference == "U").unwrap().clone();
    let local = |point: Point| Point::new(point.x_nm - pose.position.x_nm, point.y_nm - pose.position.y_nm);
    let zero = Decimal::from_i64(0);
    board.rigid_clusters.push(
        RigidPlacementCluster {
            name: "source-launch".to_owned(),
            anchor_reference: "U".to_owned(),
            anchor_pad: None,
            members: vec![
                RigidPlacementMember::new(
                    "U",
                    &pose.footprint,
                    &footprint_geometry_digest(&board.footprints[&pose.footprint]),
                    Point::new(0, 0),
                    &zero,
                )
                .unwrap(),
            ],
            source: "Synthetic source-launch contract".to_owned(),
            allowed_rotations: vec![zero],
            keepouts: Vec::new(),
            internal_clearance_nm: None,
            edge_openings: Vec::new(),
        }
        .normalized()
        .unwrap(),
    );
    board.hard_macros.push(PhysicalHardMacro {
        cluster: "source-launch".to_owned(),
        asset_sha256: "0".repeat(64),
        tracks: board
            .tracks
            .iter()
            .map(|track| TrackSegment { start: local(track.start), end: local(track.end), ..track.clone() })
            .collect(),
        vias: board.vias.iter().map(|via| Via { position: local(via.position), ..via.clone() }).collect(),
        ports: vec![MacroPort {
            name: "launch".to_owned(),
            net: "N15".to_owned(),
            position: local(board.vias[0].position),
            layer: CopperLayer::Back,
            pads: vec![scene::pad_ref("U", "15")],
        }],
        protected_regions: Vec::new(),
        required_layers: vec![CopperLayer::Front, CopperLayer::Back],
        pad_bindings: vec![(scene::pad_ref("U", "15"), "N15".to_owned())],
        isolated_pads: Vec::new(),
        zones: Vec::new(),
        plane_returns: Vec::new(),
        polygons: Vec::new(),
        width_contracts: Vec::new(),
    });
    board.materialized_macros.push("source-launch".to_owned());
    copper_pcb::hard_macros::validate_hard_macros(board).expect("coherent macro source poses and occurrences");
}

#[test]
fn inner_critical_clock_power_and_ground_are_in_the_joint_site_census() {
    let mut board = scene::ball_grid(6, 0.45, &[22]);
    board.nets.retain(|net| ["N15", "N16", "N21", "GND"].contains(&net.name.as_str()));
    board.net_routing_rules = vec![
        copper_pcb::board::NetRoutingRule { kind: RouteKind::Critical, width_nm: Some(150_000), ..scene::rule("N15") },
        copper_pcb::board::NetRoutingRule { kind: RouteKind::Clock, width_nm: Some(180_000), ..scene::rule("N16") },
        copper_pcb::board::NetRoutingRule { kind: RouteKind::Power, ..scene::rule("N21") },
    ];
    let original = board.clone();
    let result = plan(&board, None);
    assert_eq!(board, original);
    assert_eq!(result.lattices[0].coverage.len(), 4);
    assert_eq!(result.lattices[0].sites.len(), 4);
    for pad in ["15", "16", "21", "22"] {
        assert!(coverage(&result, pad).mandatory_inner);
        assert_eq!(coverage(&result, pad).status, BallSiteStatus::Allocated);
    }
    assert!(result.tracks.iter().filter(|track| track.net == "N15").all(|track| track.width_nm == 150_000));
    assert!(result.tracks.iter().filter(|track| track.net == "N16").all(|track| track.width_nm == 180_000));
    let before = scene::drc_codes(&board);
    let after = scene::drc_codes(&result.board);
    assert!(after.is_subset(&before), "new geometry findings: {:?}", after.difference(&before).collect::<Vec<_>>());
}

#[test]
fn abstract_surface_hints_cannot_waive_inner_dogbones() {
    let board = inner_board(&[1, 15]);
    let hints = [scene::pad_ref("U", "1"), scene::pad_ref("U", "15")].into_iter().collect();
    let result = plan(&board, Some(&hints));
    assert_eq!(result.lattices[0].coverage.len(), 2);
    assert_eq!(coverage(&result, "15").status, BallSiteStatus::Allocated);
    assert!(coverage(&result, "15").mandatory_inner);
    assert_eq!(coverage(&result, "1").status, BallSiteStatus::SurfacePolicy);
    assert!(!coverage(&result, "1").mandatory_inner);
    assert!(result.lattices[0].unsited.is_empty());
}

#[test]
fn an_outer_power_kind_ball_needs_a_site_without_a_zone_or_decoupling_link() {
    let mut board = inner_board(&[1]);
    board.net_routing_rules.push(copper_pcb::board::NetRoutingRule { kind: RouteKind::Power, ..scene::rule("N1") });
    let hints = [scene::pad_ref("U", "1")].into_iter().collect();
    let result = plan(&board, Some(&hints));
    assert_eq!(coverage(&result, "1").status, BallSiteStatus::Allocated);
    assert_eq!(result.lattices[0].sites[0].cell, (-1, -1));
}

#[test]
fn rule_and_coupled_launch_exclusions_are_explicit_pending_requirements() {
    let mut cases = Vec::new();
    let mut budget = scene::rule("N15");
    budget.max_vias = Some(0);
    cases.push((budget, "via budget exhausted"));
    let mut layer = scene::rule("N15");
    layer.allowed_layers = vec![CopperLayer::Front];
    cases.push((layer, "no declared off-surface destination"));
    let mut pair = scene::rule("N15");
    pair.kind = RouteKind::Differential;
    pair.differential_partner = Some("N16".to_owned());
    cases.push((pair, "coupled launch qualification unsupported"));
    let mut returning = scene::rule("N15");
    returning.kind = RouteKind::Clock;
    returning.require_return_vias = true;
    returning.return_via_net = Some("GND".to_owned());
    returning.maximum_return_via_distance_nm = Some(1_000_000);
    cases.push((returning, "required existing return via unavailable"));
    let mut stub = scene::rule("N15");
    stub.maximum_stub_length_nm = Some(1_000_000);
    cases.push((stub, "local stub or uncoupled topology qualification unsupported"));
    for (rule, reason) in cases {
        let mut board = inner_board(&[15]);
        board.net_routing_rules.push(rule);
        let original = board.clone();
        let result = plan(&board, None);
        assert_eq!(board, original);
        assert_eq!(result.lattices[0].coverage.len(), 1);
        assert_pending(&result, "15", reason);
        assert!(result.tracks.is_empty() && result.vias.is_empty());
    }
}

#[test]
fn source_macro_barrels_and_track_multiplicity_are_preserved_and_freshly_verified() {
    let mut board = inner_board(&[15]);
    // The shared far connector has a 40 mm courtyard. Keep that entire
    // source footprint inside the board for strict physical materialization.
    board.placements.iter_mut().find(|pose| pose.reference == "J").unwrap().position = scene::mm(30.0, 30.0);
    let first = plan(&board, None);
    let mut source = first.board;
    // An authored duplicate segment is retained as a distinct occurrence.
    source.tracks.push(source.tracks[0].clone());
    let second = Via { position: scene::mm(12.0, 11.2), ..source.vias[0].clone() };
    source.tracks.push(scene::track("N15", scene::mm(11.6, 11.6), second.position, 100_000, CopperLayer::Front));
    source.vias.push(second);
    bind_materialized_launch(&mut source);
    let clean = copper_pcb::hard_macros::macro_source(source.clone()).expect("exact source occurrences");
    source = copper_drc::macros::materialize_hard_macros(clean).expect("physical macro and port proof");
    let original = source.clone();
    let result = plan(&source, None);
    assert_eq!(source, original);
    assert_eq!(result.board, original, "no re-materialization, deduplication or replacement");
    assert!(result.tracks.is_empty() && result.vias.is_empty());
    match &coverage(&result, "15").status {
        BallSiteStatus::ExistingSourceLaunch { via_occurrence, track_occurrences } => {
            assert_eq!(*via_occurrence, 0);
            assert_eq!(track_occurrences, &[0]);
            assert_eq!(source.vias.len(), 2);
            assert_eq!(source.tracks.len(), 3);
        }
        status => panic!("source path proof expected, got {status:?}"),
    }
}

#[test]
fn macro_association_or_same_net_coordinate_cannot_invent_source_launch_proof() {
    let first = plan(&inner_board(&[15]), None);
    let mut source = first.board;
    source.tracks.clear(); // The barrel exists; its promised pad path does not.
    bind_materialized_launch(&mut source);
    let original = source.clone();
    let result = plan(&source, None);
    assert_pending(&result, "15", "macro-owned pad has no verified rule-qualified source dogbone");
    assert_eq!(result.board, original);
    assert!(result.tracks.is_empty() && result.vias.is_empty());
}

#[test]
fn shared_net_via_budget_counts_each_new_barrel_occurrence() {
    let mut board = inner_board(&[15, 16]);
    board.nets = vec![scene::net("P", &[("U", "15"), ("U", "16")])];
    board.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        kind: RouteKind::Power,
        max_vias: Some(1),
        ..scene::rule("P")
    });
    let result = plan(&board, None);
    assert_eq!(result.vias.len(), 1);
    assert_eq!(result.lattices[0].sites.len(), 1);
    assert_eq!(result.lattices[0].coverage.len(), 2);
    assert_eq!(result.lattices[0].unsited.len(), 1);
    assert_eq!(result.lattices[0].unsited[0].1, "via budget exhausted");
}

#[test]
fn a_blocked_inner_ball_stays_in_coverage_with_no_claimed_launch() {
    let mut board = inner_board(&[15]);
    let mut region =
        scene::keepout("blocked field", &[CopperLayer::Front, CopperLayer::Back], scene::rect(10.7, 10.7, 12.5, 12.5));
    region.block_tracks = false;
    board.copper_keepouts.push(region);
    let result = plan(&board, None);
    assert_pending(&result, "15", "no legal site");
    assert!(coverage(&result, "15").mandatory_inner);
    assert!(result.tracks.is_empty() && result.vias.is_empty());
}

#[test]
fn plane_contact_span_does_not_require_a_future_island_but_pad_face_permission_is_explicit() {
    use copper_pcb::board::Stackup;
    let mut board = scene::ball_grid(6, 0.45, &[1]);
    board.nets.retain(|net| net.name == "N15" || net.name == "GND");
    board.stackup = Stackup::new(vec![
        CopperLayer::Front,
        CopperLayer::In1,
        CopperLayer::In2,
        CopperLayer::In3,
        CopperLayer::In4,
        CopperLayer::Back,
    ]);
    board.zones[0].layers = vec![CopperLayer::In4];
    let mut rail = scene::rule("N15");
    rail.kind = RouteKind::Power;
    rail.allowed_layers = vec![CopperLayer::In4];
    rail.breakout_length_nm = Some(1_000_000);
    rail.breakout_width_nm = Some(100_000);
    board.net_routing_rules.push(rail);
    let original = board.clone();
    let blocked = plan(&board, None);
    assert_pending(&blocked, "15", "pad-face layer excluded by source rule");
    assert_eq!(coverage(&blocked, "15").pending_plane_layers, [CopperLayer::In4]);
    assert_eq!(board, original);
    board.net_routing_rules[0].allowed_layers.insert(0, CopperLayer::Front);
    assert_eq!(routing_layers(&board, "N15", Some(&board.net_routing_rules[0])), [CopperLayer::Front]);
    let result = plan(&board, None);
    assert_eq!(coverage(&result, "15").status, BallSiteStatus::Allocated);
    assert_eq!(coverage(&result, "15").pending_plane_layers, [CopperLayer::In4]);
    assert_eq!(result.board.zones, board.zones, "no fabricated island or fill proof");
    assert!(result.tracks.iter().filter(|track| track.net == "N15").all(|track| track.layer == CopperLayer::Front));
    let before = scene::drc_codes(&board);
    let after = scene::drc_codes(&result.board);
    assert!(after.is_subset(&before), "new geometry findings: {:?}", after.difference(&before).collect::<Vec<_>>());
}

#[test]
fn allocated_site_ranges_locate_exact_rule_qualified_breakout_pieces_and_barrel() {
    let mut board = inner_board(&[15]);
    board.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        kind: RouteKind::Clock,
        width_nm: Some(200_000),
        breakout_length_nm: Some(200_000),
        breakout_width_nm: Some(100_000),
        ..scene::rule("N15")
    });
    let result = plan(&board, None);
    assert_eq!(coverage(&result, "15").status, BallSiteStatus::Allocated);
    let site = &result.lattices[0].sites[0];
    let pieces = &result.tracks[site.track_range.clone()];
    assert!(pieces.len() >= 2, "a boundary-crossing neckdown retains every emitted piece");
    assert_eq!(pieces.first().unwrap().start, scene::mm(11.6, 11.6));
    assert_eq!(pieces.last().unwrap().end, site.position);
    assert!(pieces.iter().any(|track| track.width_nm == 100_000));
    assert!(pieces.iter().any(|track| track.width_nm == 200_000));
    let barrel = &result.vias[site.via_occurrence];
    assert_eq!(barrel.net, "N15");
    assert_eq!(barrel.position, site.position);
    assert_eq!(barrel.from_layer, CopperLayer::Front);
    assert_eq!(barrel.to_layer, CopperLayer::Back);
    let before = scene::drc_codes(&board);
    let after = scene::drc_codes(&result.board);
    assert!(after.is_subset(&before), "new geometry findings: {:?}", after.difference(&before).collect::<Vec<_>>());
}

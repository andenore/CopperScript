//! Macro copper/copper acceptance rejects a seed, not an earlier valid candidate.
#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;
use boards::*;
use copper_drc::macros::{MacroMaterialization, materialize_hard_macros, try_materialize_hard_macros};
use copper_pcb::board::*;
use copper_physical::CopperLayer::{Back, Front};
use copper_physical::footprint::footprint_geometry_digest;
use copper_place::PlannerOptions;
use copper_place::planner::generate_placement_candidates_with_diagnostics;
use copper_units::Decimal;

fn fixture() -> (PhysicalBoard, PlannerOptions) {
    let fp = one_pad();
    let cap = footprint("cap", vec![pad("1", -0.6, 0.0, 0.5, 0.5), pad("2", 0.6, 0.0, 0.5, 0.5)], 1.8, 1.0);
    let mut board = board(
        "macro-seeds",
        40.0,
        30.0,
        vec![fp.clone(), cap.clone()],
        vec![
            placement("A", &fp.name, 10.0, 10.0),
            placement("B", &fp.name, 35.0, 20.0),
            placement("C", &cap.name, 5.0, 25.0),
            placement("U", &fp.name, 10.0, 25.0),
        ],
        vec![
            net("A_NET", &[("A", "1")]),
            net("B_NET", &[("B", "1")]),
            net("V", &[("C", "1"), ("U", "1")]),
            net("GND", &[("C", "2")]),
        ],
    );
    for reference in ["B", "C", "U"] {
        let pose = board.placements.iter().find(|p| p.reference == reference).unwrap();
        board.placement_rules.push(ComponentPlacementRule {
            fixed_position: Some(pose.position),
            fixed_rotation_degrees: Some(Decimal::from_i64(0)),
            ..ComponentPlacementRule::new(reference)
        });
    }
    for (reference, net, tracks) in [
        ("A", "A_NET", vec![track("A_NET", (0.0, 0.0), (0.0, 10.0), 0.2, Front)]),
        ("B", "B_NET", vec![track("B_NET", (-10.0, 0.0), (0.0, 0.0), 0.2, Front)]),
    ] {
        board.rigid_clusters.push(RigidPlacementCluster {
            name: reference.to_owned(),
            anchor_reference: reference.to_owned(),
            anchor_pad: None,
            members: vec![
                RigidPlacementMember::new(
                    reference,
                    &fp.name,
                    &footprint_geometry_digest(&fp),
                    pt(0.0, 0.0),
                    &Decimal::from_i64(0),
                )
                .unwrap(),
            ],
            source: "generic crossing macro regression".to_owned(),
            allowed_rotations: vec![Decimal::from_i64(0)],
            keepouts: Vec::new(),
            internal_clearance_nm: None,
            edge_openings: Vec::new(),
        });
        board.hard_macros.push(PhysicalHardMacro {
            cluster: reference.to_owned(),
            asset_sha256: "a".repeat(64),
            tracks,
            vias: Vec::new(),
            ports: Vec::new(),
            protected_regions: Vec::new(),
            required_layers: vec![Front],
            pad_bindings: vec![(PadReference::new(reference, "1"), net.to_owned())],
            isolated_pads: Vec::new(),
            zones: Vec::new(),
            plane_returns: Vec::new(),
            polygons: Vec::new(),
            width_contracts: Vec::new(),
        });
    }
    board.decoupling_links.push(DecouplingLink {
        capacitor: PadReference::new("C", "1"),
        target: PadReference::new("U", "1"),
        return_pad: PadReference::new("C", "2"),
        net: "V".to_owned(),
        return_net: "GND".to_owned(),
    });
    let options = PlannerOptions {
        seed_count: Some(2),
        candidate_count: 2,
        analytical_iterations: 0,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        decoupling_placement_trials: 0,
        satellite_seeding: false,
        ..PlannerOptions::default()
    };
    (board, options)
}

#[test]
fn later_macro_copper_rejection_preserves_the_valid_earlier_seed() {
    let (board, options) = fixture();
    let original = board.clone();
    let mut diagnostics = Vec::new();
    let mut observe = |d: &copper_place::planner::PlacementFailureDiagnostic| diagnostics.push(d.clone());
    let candidates =
        generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe)).unwrap();
    assert_eq!(board, original);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].seed, 0);
    assert_eq!(candidates[0].statistics.failed_seed_attempts.len(), 1);
    assert_eq!(candidates[0].statistics.failed_seed_attempts[0].seed, 1);
    assert_eq!(diagnostics.len(), 1);
    let rejected = diagnostics[0].physical_rejection.as_ref().unwrap();
    assert!(
        rejected.findings.iter().any(|f| f.code == "DRC-CLEARANCE" || f.code == "DRC-SHORT"),
        "{:?}",
        rejected.findings
    );
    assert_eq!(rejected.board.tracks.len(), 2);
    assert!(diagnostics[0].proposal.is_none());
    assert!(diagnostics[0].search_succeeded);
    let mut accepted = board.clone();
    accepted.placements = candidates[0].placements.clone();
    materialize_hard_macros(accepted).expect("accepted seed passes the unchanged exact macro gate");
}

#[test]
fn all_macro_copper_rejections_fail_with_inspectable_tracks_and_drills() {
    let (mut board, options) = fixture();
    // This immutable intra-owner error occurs in both seed poses.
    board.hard_macros[0].vias.push(via("B_NET", (0.0, 5.0), 0.6, 0.3, Front, Back));
    let mut diagnostics = Vec::new();
    let mut observe = |d: &copper_place::planner::PlacementFailureDiagnostic| diagnostics.push(d.clone());
    let error = generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe))
        .unwrap_err()
        .to_string();
    assert!(error.contains("seed 0") && error.contains("seed 1"), "{error}");
    assert_eq!(diagnostics.len(), 1);
    let rejected = diagnostics[0].physical_rejection.as_ref().unwrap();
    assert!(rejected.findings.iter().any(|f| f.code == "DRC-HOLE-CLEARANCE"));
    assert_eq!(rejected.board.vias.len(), 1);
    assert!(materialize_hard_macros(board.clone()).is_err(), "strict public wrapper still rejects");
    assert!(matches!(try_materialize_hard_macros(board).unwrap(), MacroMaterialization::Rejected(_)));
}

#[test]
fn malformed_macro_source_remains_fatal_not_a_seed_rejection() {
    let (mut board, _) = fixture();
    board.tracks.push(track("A_NET", (10.0, 10.0), (11.0, 10.0), 0.2, Front));
    assert!(try_materialize_hard_macros(board).is_err());
}

#[test]
fn an_earlier_macro_rejection_does_not_prevent_a_later_valid_seed() {
    let (mut board, options) = fixture();
    board.placements[0].position.x_nm = mm(30.0);
    let candidates = generate_placement_candidates_with_diagnostics(&board, &options, None, None).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].seed, 1);
    assert_eq!(candidates[0].statistics.failed_seed_attempts[0].seed, 0);
    let mut accepted = board;
    accepted.placements = candidates[0].placements.clone();
    materialize_hard_macros(accepted).unwrap();
}

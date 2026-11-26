//! A bounded rigid-group search failure rejects one seed, not the whole planner.

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use std::cell::RefCell;

use boards::*;
use copper_pcb::board::{
    ComponentPlacementRule, PhysicalBoard, PlacementRegion, PlacementTarget, RelativePlacementKind,
    RelativePlacementRule, RigidPlacementCluster, RigidPlacementMember,
};
use copper_pcb::hard_macros::{cluster_placement_matches, cluster_placements};
use copper_pcb::keepouts::resolved_cluster_keepouts;
use copper_physical::footprint::footprint_geometry_digest;
use copper_physical::{BoardOutline, CopperLayer, PlacementKeepout};
use copper_place::PlannerOptions;
use copper_place::clusters::place_rigid_clusters;
use copper_place::escape::EscapeSpacingModel;
use copper_place::legalize::{Legalized, legalize};
use copper_place::planner::{
    generate_placement_candidates_with_diagnostics, generate_placement_candidates_with_progress,
};
use copper_place::scene::{Poses, Scene};
use copper_units::Decimal;

fn rectangle(left: f64, top: f64, right: f64, bottom: f64) -> BoardOutline {
    BoardOutline::new(vec![pt(left, top), pt(right, top), pt(right, bottom), pt(left, bottom)], Vec::new(), None, None)
        .expect("rectangle")
}

fn fixture() -> (PhysicalBoard, PlannerOptions) {
    let fp = one_pad();
    let digest = footprint_geometry_digest(&fp);
    let placements = vec![placement("A", &fp.name, 10.0, 10.0), placement("B", &fp.name, 13.0, 10.0)];
    let cluster = RigidPlacementCluster {
        name: "pair".to_owned(),
        anchor_reference: "A".to_owned(),
        anchor_pad: None,
        members: [("A", 0.0), ("B", 3.0)]
            .iter()
            .map(|(reference, x)| {
                RigidPlacementMember::new(reference, &fp.name, &digest, pt(*x, 0.0), &Decimal::from_i64(0))
                    .expect("member")
            })
            .collect(),
        source: "bounded seed regression".to_owned(),
        allowed_rotations: vec![Decimal::from_i64(0)],
        keepouts: Vec::new(),
        internal_clearance_nm: None,
        edge_openings: Vec::new(),
    }
    .normalized()
    .expect("cluster");
    let mut board = board("cluster-seeds", 40.0, 30.0, vec![fp], placements, Vec::new());
    board.rigid_clusters.push(cluster);
    let options = PlannerOptions {
        candidate_count: 2,
        seed_count: Some(2),
        legalization_candidates: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        decoupling_placement_trials: 0,
        satellite_seeding: false,
        ..PlannerOptions::default()
    };
    (board, options.validated().expect("options"))
}

fn pocket_fixture(block_second_seed: bool) -> (PhysicalBoard, PlannerOptions) {
    let (mut board, options) = fixture();
    // A complete pair fits only when A is within 0.05 mm of (29.5, y).
    // No integer lattice proposal fits. Seed 1 supplies its exact off-grid
    // target (29.5,10.5); seed 0's target is (9,11). Moving the pocket to
    // Y=10.25 leaves continuous legal geometry that neither seed samples.
    let y = if block_second_seed { 10.25 } else { 10.5 };
    for (name, outline) in [
        ("above", rectangle(0.0, 0.0, 40.0, y - 0.55)),
        ("below", rectangle(0.0, y + 0.55, 40.0, 30.0)),
        ("left", rectangle(0.0, y - 0.55, 28.95, y + 0.55)),
        ("right", rectangle(33.05, y - 0.55, 40.0, y + 0.55)),
    ] {
        board.keepouts.push(PlacementKeepout::new(name, outline, None, None).expect("keepout"));
    }
    (board, options)
}

#[test]
fn a_later_seed_can_place_a_group_after_the_first_exhausts_its_budget() {
    let (board, options) = pocket_fixture(false);
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, seed, _| events.push((stage.to_owned(), seed)));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&observer))
        .expect("the second seed has a legal group pose");
    drop(observer);

    assert!(events.contains(&("legalization_failed".to_owned(), 0)));
    assert!(events.contains(&("analytical".to_owned(), 1)));
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.seed, 1);
    assert_eq!(candidate.statistics.failed_seed_attempts.len(), 1);
    let failure = &candidate.statistics.failed_seed_attempts[0];
    assert_eq!(failure.seed, 0);
    assert_eq!(failure.failed_reference, "A");
    assert_eq!(failure.accepted_count, 0);
    assert!(failure.reason.contains("rigid cluster 'pair'"), "{}", failure.reason);
    let poses: Poses = candidate.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    assert_eq!(poses["A"].position, pt(29.5, 10.5));
    assert!(cluster_placement_matches(&board, &poses).expect("rigid relation"));
    assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&poses).expect("legality"));
}

#[test]
fn exhausting_every_rigid_seed_reports_all_attempts_without_accepting_a_partial_board() {
    let (board, options) = pocket_fixture(true);
    let mut visited = Vec::new();
    let observer = RefCell::new(|stage: &str, seed, _| {
        if stage == "analytical" {
            visited.push(seed);
        }
    });
    let error = generate_placement_candidates_with_progress(&board, &options, Some(&observer))
        .expect_err("neither bounded group search samples the off-grid pocket")
        .to_string();
    drop(observer);

    assert_eq!(visited, vec![0, 1]);
    assert!(error.contains("cannot legalize rigid cluster 'pair' within its candidate budget"), "{error}");
    assert!(error.contains("seed 0: A after 0 accepted placements"), "{error}");
    assert!(error.contains("seed 1: A after 0 accepted placements"), "{error}");
}

#[test]
fn a_fixed_group_collision_remains_fatal_instead_of_consuming_more_seeds() {
    let (mut board, mut options) = fixture();
    // A is locked, so B must occupy (13,10), exactly where the locked obstacle sits.
    board.placements.push(placement("F0", &board.placements[0].footprint, 13.0, 10.0));
    options.fixed_references.insert("A".to_owned());
    options.fixed_references.insert("F0".to_owned());
    let original = board.clone();
    let mut visited = Vec::new();
    let observer = RefCell::new(|stage: &str, seed, _| {
        if stage == "analytical" {
            visited.push(seed);
        }
    });
    let error = generate_placement_candidates_with_progress(&board, &options, Some(&observer))
        .expect_err("no seed can move the fixed group")
        .to_string();
    drop(observer);

    assert_eq!(visited, vec![0]);
    assert!(error.contains("fixed rigid cluster 'pair'"), "{error}");
    assert!(!error.contains("bounded seed attempts"), "{error}");
    assert_eq!(board, original);
}

#[test]
fn a_distant_legal_pocket_is_found_after_nearest_candidates_exhaust() {
    let (mut board, mut options) = fixture();
    options.legalization_candidates = 96;
    options.seed_count = Some(1);
    board
        .keepouts
        .push(PlacementKeepout::new("blocked-left", rectangle(0.0, 0.0, 30.0, 30.0), None, None).expect("keepout"));
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, seed, _| events.push((stage.to_owned(), seed)));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&observer))
        .expect("the distant right-hand pocket fits the rigid pair");
    drop(observer);

    assert!(events.contains(&("cluster_wide:pair".to_owned(), 0)));
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].seed, 0);
    let poses: Poses = candidates[0].placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    assert!(poses["A"].position.x_nm >= mm(31.0));
    assert!(cluster_placement_matches(&board, &poses).expect("rigid relation"));
    assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&poses).expect("legality"));
}

#[test]
fn a_legal_local_choice_is_preserved_even_when_distant_sites_have_lower_wirelength() {
    let (mut board, mut options) = fixture();
    board.placements.push(placement("S", &board.placements[0].footprint, 35.0, 10.0));
    board.nets.push(net("signal", &[("A", "1"), ("S", "1")]));
    options.fixed_references.insert("S".to_owned());
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let scene = Scene::new(&board, options.policy()).with_progress(Some(&observer));
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    let (placed, _, _) = place_rigid_clusters(&scene, &source, &source, &options, &spacing).expect("local choice");
    assert_eq!(placed, source);
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("legality"));
    drop(spacing);
    drop(scene);
    drop(observer);
    assert!(!events.iter().any(|stage| stage.starts_with("cluster_wide:")));
}

#[test]
fn wider_search_uses_cluster_rotations_when_component_domains_include_other_orientations() {
    let (mut board, mut options) = fixture();
    let long = footprint("long", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 8.0, 2.0);
    board.rigid_clusters[0].members[0].footprint = long.name.clone();
    board.rigid_clusters[0].members[0].footprint_digest = footprint_geometry_digest(&long);
    board.placements[0] = placement("A", &long.name, 4.0, 10.0);
    board.placements[1].position = pt(4.0, 14.0);
    board.rigid_clusters[0].members[1].position = pt(0.0, 4.0);
    board.footprints.insert(long.name.clone(), long);
    options.legalization_candidates = 2;

    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let scene = Scene::new(&board, options.policy()).with_progress(Some(&observer));
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    // The nearest origins fit a vertical component, but the rigid frame allows
    // only zero degrees. Its wider search must find a horizontal origin instead.
    let (placed, _, _) = place_rigid_clusters(&scene, &source, &source, &options, &spacing).expect("wider choice");
    assert_eq!(placed["A"].rotation_degrees, Decimal::from_i64(0));
    assert!(placed["A"].position.x_nm >= mm(6.0));
    assert!(cluster_placement_matches(&board, &placed).expect("rigid relation"));
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("legality"));
    drop(spacing);
    drop(scene);
    drop(observer);
    assert!(events.contains(&"cluster_wide:pair".to_owned()));
}

#[test]
fn a_translated_cluster_keepout_cannot_cover_a_held_outsider() {
    let (mut board, mut options) = fixture();
    board.placements.push(placement("F", &board.placements[0].footprint, 20.0, 10.0));
    options.fixed_references.insert("F".to_owned());
    let mut keepout = copper_keepout(
        "clear-area",
        &[CopperLayer::Front],
        vec![pt(4.0, -1.0), pt(6.0, -1.0), pt(6.0, 1.0), pt(4.0, 1.0)],
    );
    keepout.block_footprints = true;
    board.rigid_clusters[0].keepouts.push(keepout);
    let scene = Scene::new(&board, options.policy());
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    assert!(scene.checker.placement_solution_is_legal(&source).expect("source legality"));

    let mut target_anchor = source["A"].clone();
    target_anchor.position = pt(15.0, 10.0);
    let members = cluster_placements(&board.rigid_clusters[0], &target_anchor, &source).expect("translated pair");
    let mut targets = source.clone();
    targets.extend(members.clone());
    let keepouts = resolved_cluster_keepouts(&board, &targets);
    for member in members.values() {
        let others = copper_place::scene::without(&targets, &member.reference);
        assert!(scene.checker.legal(member, &others, Some(&keepouts)).expect("new members are legal"));
    }
    let others = copper_place::scene::without(&targets, "F");
    assert!(!scene.checker.legal(&source["F"], &others, Some(&keepouts)).expect("held outsider is blocked"));

    let (placed, _, _) = place_rigid_clusters(&scene, &targets, &source, &options, &spacing)
        .expect("reject the local pose and find a wider legal pose");
    assert_ne!(placed["A"].position, target_anchor.position);
    assert_eq!(placed["F"], source["F"]);
    assert!(cluster_placement_matches(&board, &placed).expect("rigid relation"));
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
}

fn competing_groups() -> (PhysicalBoard, PlannerOptions) {
    let (mut board, options) = fixture();
    board.rigid_clusters[0].name = "A_free".to_owned();
    let mut constrained = board.rigid_clusters[0].clone();
    constrained.name = "Z_region".to_owned();
    constrained.anchor_reference = "C".to_owned();
    for (member, reference) in constrained.members.iter_mut().zip(["C", "D"]) {
        member.reference = reference.to_owned();
    }
    board.rigid_clusters.push(constrained);
    let fp = board.placements[0].footprint.clone();
    board.placements.extend([placement("C", &fp, 10.0, 10.0), placement("D", &fp, 13.0, 10.0)]);
    board.regions.push(PlacementRegion {
        name: "connector-pocket".to_owned(),
        outline: rectangle(12.45, 9.45, 13.55, 10.55),
        side: None,
    });
    // A region on a non-anchor member constrains the entire rigid group.
    board.placement_rules.push(ComponentPlacementRule {
        region: Some("connector-pocket".to_owned()),
        ..ComponentPlacementRule::new("D")
    });
    (board, options)
}

#[test]
fn a_region_bound_group_gets_its_only_pocket_before_an_unrestricted_group() {
    let (board, options) = competing_groups();
    let scene = Scene::new(&board, options.policy());
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    let (placed, _, _) = place_rigid_clusters(&scene, &source, &source, &options, &spacing)
        .expect("the constrained group gets its pocket and the free group relocates");
    assert_eq!(placed["C"].position, pt(10.0, 10.0));
    assert_eq!(placed["D"].position, pt(13.0, 10.0));
    assert_ne!(placed["A"].position, pt(10.0, 10.0));
    assert!(cluster_placement_matches(&board, &placed).expect("rigid relations"));
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
}

#[test]
fn fixed_groups_precede_movable_regions_and_retain_name_order_among_themselves() {
    for fix_region in [false, true] {
        let (board, mut options) = competing_groups();
        options.fixed_references.insert("A".to_owned());
        if fix_region {
            options.fixed_references.insert("C".to_owned());
        }
        let scene = Scene::new(&board, options.policy());
        let spacing =
            EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
        let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
        let error = place_rigid_clusters(&scene, &source, &source, &options, &spacing)
            .expect_err("the fixed alphabetical-first group owns the overlapping pocket")
            .to_string();
        let expected =
            if fix_region { "fixed rigid cluster 'Z_region'" } else { "cannot legalize rigid cluster 'Z_region'" };
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn unrestricted_groups_retain_their_name_order() {
    let (mut board, options) = competing_groups();
    board.placement_rules.clear();
    let scene = Scene::new(&board, options.policy());
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("spacing");
    let source: Poses = board.placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    let (placed, _, _) = place_rigid_clusters(&scene, &source, &source, &options, &spacing).expect("free groups");
    assert_eq!(placed["A"].position, pt(10.0, 10.0));
    assert_ne!(placed["C"].position, pt(10.0, 10.0));
    assert!(cluster_placement_matches(&board, &placed).expect("rigid relations"));
    assert!(scene.checker.placement_solution_is_legal(&placed).expect("complete legality"));
}

#[test]
fn diagnostics_choose_the_deepest_failed_seed_and_keep_the_target_unaccepted() {
    let (mut board, options) = pocket_fixture(false);
    let mut last = board.rigid_clusters[0].clone();
    last.name = "z-last".to_owned();
    last.anchor_reference = "Z".to_owned();
    last.members.truncate(1);
    last.members[0].reference = "Z".to_owned();
    board.rigid_clusters.push(last);
    board.placements.push(placement("Z", &board.placements[0].footprint, 20.0, 20.0));
    let mut diagnostics = Vec::new();
    let mut observe = |failure: &copper_place::planner::PlacementFailureDiagnostic| diagnostics.push(failure.clone());
    assert!(generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe)).is_err());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].seed, 1);
    assert_eq!(diagnostics[0].accepted.len(), 2);
    assert_eq!(diagnostics[0].failed_reference, "Z");
    assert!(!diagnostics[0].accepted.contains_key("Z"));
    assert_eq!(diagnostics[0].proposal.as_ref().unwrap().reference, "Z");
}

#[test]
fn diagnostics_keep_the_earliest_tied_seed_and_are_silent_when_a_later_seed_succeeds() {
    for all_fail in [true, false] {
        let (board, options) = pocket_fixture(all_fail);
        let mut diagnostics = Vec::new();
        let mut observe =
            |failure: &copper_place::planner::PlacementFailureDiagnostic| diagnostics.push(failure.clone());
        let result = generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe));
        if all_fail {
            assert!(result.is_err());
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].seed, 0);
        } else {
            assert!(result.is_ok());
            assert!(diagnostics.is_empty());
        }
    }
}

#[test]
fn diagnostics_preserve_complete_constraint_rejections_over_partial_failures() {
    for mixed in [false, true] {
        let (mut board, options) = if mixed { pocket_fixture(false) } else { fixture() };
        // The rigid pair is individually hard-legal, but its 3 mm separation
        // contradicts this relative rule. Both stages must retain their gates.
        board.relative_rules.push(RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: ["B", "A"]
                .iter()
                .map(|r| PlacementTarget { reference: (*r).to_owned(), pad: Some("1".to_owned()) })
                .collect(),
            distance_nm: Some(mm(2.0)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        });
        let mut events = Vec::new();
        let progress = RefCell::new(|stage: &str, seed, _| events.push((stage.to_owned(), seed)));
        let mut diagnostics = Vec::new();
        let mut observe =
            |failure: &copper_place::planner::PlacementFailureDiagnostic| diagnostics.push(failure.clone());
        let error =
            generate_placement_candidates_with_diagnostics(&board, &options, Some(&progress), Some(&mut observe))
                .expect_err("a complete but contradictory layout must never become a candidate")
                .to_string();
        drop(progress);
        assert!(error.contains("maximum distance 2 mm, actual 3 mm"), "{error}");
        assert!(events.contains(&("constraints_rejected".to_owned(), 1)));
        assert!(events.contains(&(if mixed { "legalization_failed" } else { "constraints_rejected" }.to_owned(), 0)));
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.seed, if mixed { 1 } else { 0 });
        assert_eq!(diagnostic.accepted.len(), board.placements.len());
        assert_eq!(diagnostic.failed_reference, "B", "locate the actual first violated rule target");
        assert!(diagnostic.proposal.is_none());
        assert!(diagnostic.reason.contains("complete layout rejected"));
        let scene = Scene::new(&board, options.policy());
        assert!(scene.checker.placement_solution_is_hard_legal(&diagnostic.accepted).unwrap());
        assert!(!scene.checker.placement_solution_is_legal(&diagnostic.accepted).unwrap());
    }
}

fn scarce_singleton(with_rigid: bool) -> (PhysicalBoard, PlannerOptions) {
    let (mut board, mut options) = fixture();
    board.placements[0].position = pt(11.0, 9.0);
    board.placements.push(placement("J", &board.placements[0].footprint, 10.0, 10.0));
    board.regions.push(PlacementRegion {
        name: "landing".to_owned(),
        outline: rectangle(9.45, 9.45, 10.55, 10.55),
        side: None,
    });
    board.placement_rules.push(ComponentPlacementRule {
        region: Some("landing".to_owned()),
        allowed_orientations: vec![Decimal::from_i64(0)],
        ..ComponentPlacementRule::new("J")
    });
    if !with_rigid {
        board.rigid_clusters.clear();
        board.relative_rules.push(RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: ["A", "B"]
                .iter()
                .map(|r| PlacementTarget { reference: (*r).to_owned(), pad: Some("1".to_owned()) })
                .collect(),
            distance_nm: Some(mm(4.0)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        });
    }
    options.seed_count = Some(1);
    options.exact_repair_limit = 1;
    (board, options)
}

#[test]
fn scarce_singleton_precedes_unrestricted_rigid_and_relative_groups() {
    for rigid in [true, false] {
        let (board, options) = scarce_singleton(rigid);
        let mut old_targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        old_targets.get_mut("A").unwrap().position = pt(10.0, 10.0);
        let mut old_options = options.clone();
        if rigid {
            // The old rigid phase accepts its exact local target first.
            old_options.fixed_references.extend(["A".to_owned(), "B".to_owned()]);
        }
        let old_scene = Scene::new(&board, old_options.policy());
        let spacing =
            EscapeSpacingModel::new(&old_scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let old = legalize(&old_scene, &old_targets, &old_targets, &old_options, &spacing, 0).unwrap();
        assert!(
            matches!(old, Legalized::Failed(ref failure) if failure.failed_reference == "J"),
            "old order must lose the only landing pocket"
        );

        let mut events = Vec::new();
        let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
        let candidates =
            generate_placement_candidates_with_progress(&board, &options, Some(&observer)).expect("region first");
        drop(observer);
        assert!(events.contains(&"region_reserve:J".to_owned()));
        assert!(!events.contains(&"legalize:J".to_owned()), "reservation is not searched a second time");
        let placed: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        assert_eq!(placed["J"].position, pt(10.0, 10.0));
        let scene = Scene::new(&board, options.policy());
        assert!(scene.checker.placement_solution_is_legal(&placed).unwrap());
        assert_eq!(scene.penalty(&placed).unwrap(), 0);
    }
}

#[test]
fn fixed_group_precedes_scarce_singleton_and_broad_regions_are_jointly_reserved() {
    let (mut board, mut options) = scarce_singleton(true);
    board.placements[0].position = pt(10.0, 10.0);
    options.fixed_references.insert("A".to_owned());
    let original = board.clone();
    let error = generate_placement_candidates_with_progress(&board, &options, None)
        .expect_err("fixed group owns the pocket")
        .to_string();
    assert!(error.contains("narrow-region component 'J'"), "{error}");
    assert_eq!(board, original);

    let (mut board, options) = scarce_singleton(true);
    board.regions[0].outline = rectangle(0.0, 0.0, 40.0, 30.0);
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    generate_placement_candidates_with_progress(&board, &options, Some(&observer)).expect("broad region");
    drop(observer);
    assert!(events.contains(&"region_pack:J".to_owned()));
    assert!(!events.contains(&"legalize:J".to_owned()));
    assert!(!events.contains(&"region_reserve:J".to_owned()));
}

#[test]
fn overlapping_regional_connectors_pack_before_an_unrestricted_group_consumes_their_landing() {
    let (mut board, mut options) = fixture();
    options.legalization_candidates = 96;
    options.seed_count = Some(1);
    options.exact_repair_limit = 2;
    board.placements[0].position = pt(14.0, 11.5);
    board.placements[1].position = pt(17.0, 11.5);
    let connector = footprint("wide-connector", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 8.0, 3.0);
    board.footprints.insert(connector.name.clone(), connector);
    for (reference, left) in [("J1", 4.0), ("J2", 9.0)] {
        board.placements.push(placement(reference, "wide-connector", 16.0, 11.5));
        board.regions.push(PlacementRegion {
            name: reference.to_owned(),
            outline: rectangle(left, 4.0, 27.0, 19.0),
            side: None,
        });
        board.placement_rules.push(ComponentPlacementRule {
            region: Some(reference.to_owned()),
            allowed_orientations: vec![Decimal::from_i64(0)],
            ..ComponentPlacementRule::new(reference)
        });
    }
    // The authored regions are broad, but mechanical obstacles leave a shared
    // landing. Both connectors fit together; a small unrestricted group in its
    // middle fragments it. No connector is fixed to the constructive witness.
    for (name, outline) in [
        ("above", rectangle(4.0, 4.0, 27.0, 9.4)),
        ("below", rectangle(4.0, 13.6, 27.0, 19.0)),
        ("left", rectangle(4.0, 9.4, 5.5, 13.6)),
        ("right", rectangle(25.5, 9.4, 27.0, 13.6)),
    ] {
        board.keepouts.push(PlacementKeepout::new(name, outline, None, None).unwrap());
    }
    let scene = Scene::new(&board, options.policy());
    let mut witness: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    witness.get_mut("J1").unwrap().position = pt(10.0, 11.5);
    witness.get_mut("J2").unwrap().position = pt(20.0, 11.5);
    witness.get_mut("A").unwrap().position = pt(10.0, 23.0);
    witness.get_mut("B").unwrap().position = pt(13.0, 23.0);
    assert!(scene.checker.placement_solution_is_legal(&witness).unwrap(), "joint layout is exactly legal");
    assert!(cluster_placement_matches(&board, &witness).unwrap());
    for reference in ["J1", "J2"] {
        assert!(
            copper_place::legalize::candidate_positions(&scene, &witness[reference], &options).len()
                > options.legalization_candidates as usize,
            "raw region size must exceed the old early-reservation cutoff"
        );
    }
    let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let packed = copper_place::legalize::pack_local_components(
        &scene,
        &["J1".to_owned(), "J2".to_owned()],
        &Poses::new(),
        &targets,
        &options,
        true,
        false,
        &[],
    )
    .unwrap()
    .expect("the existing joint packer and unchanged budget admit both connectors");
    assert_eq!(packed.len(), 2);
    witness.extend(packed);
    assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
    let candidates = generate_placement_candidates_with_progress(&board, &options, None)
        .expect("joint regional placement must precede the unrestricted group");
    let poses: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    assert!(scene.checker.placement_solution_is_legal(&poses).unwrap());
    assert!(cluster_placement_matches(&board, &poses).unwrap());
}

#[test]
fn regional_rigid_group_keeps_its_pocket_before_broad_singleton_packing() {
    for fixed in [false, true] {
        let (mut board, mut options) = scarce_singleton(true);
        options.legalization_candidates = 96;
        board.regions[0].outline = rectangle(0.0, 0.0, 40.0, 30.0);
        board.regions.push(PlacementRegion {
            name: "rigid-pocket".to_owned(),
            outline: rectangle(9.45, 9.45, 10.55, 10.55),
            side: None,
        });
        board.placement_rules.push(ComponentPlacementRule {
            region: Some("rigid-pocket".to_owned()),
            allowed_orientations: vec![Decimal::from_i64(0)],
            ..ComponentPlacementRule::new("A")
        });
        board.placements[0].position = pt(10.0, 10.0);
        if fixed {
            options.fixed_references.insert("A".to_owned());
        }
        let original = board.clone();
        let candidates = generate_placement_candidates_with_progress(&board, &options, None).unwrap();
        let poses: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        if fixed {
            assert_eq!(poses["A"].position, pt(10.0, 10.0));
        } else {
            assert!((poses["A"].position.x_nm - mm(10.0)).abs() <= mm(0.05));
            assert!((poses["A"].position.y_nm - mm(10.0)).abs() <= mm(0.05));
        }
        assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&poses).unwrap());
        assert!(cluster_placement_matches(&board, &poses).unwrap());
        assert_eq!(board, original, "reservation must not add source locks or alter constraints");
    }
}

#[test]
fn broad_regional_singleton_is_reserved_without_any_rigid_groups() {
    let (mut board, mut options) = scarce_singleton(false);
    options.legalization_candidates = 96;
    board.regions[0].outline = rectangle(0.0, 0.0, 40.0, 30.0);
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&observer)).unwrap();
    drop(observer);
    assert!(events.contains(&"region_pack:J".to_owned()));
    assert!(!events.contains(&"legalize:J".to_owned()));
    let poses: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&poses).unwrap());
}

fn mixed_regional_fixture() -> (PhysicalBoard, PlannerOptions) {
    let (mut board, mut options) = fixture();
    options.legalization_candidates = 96;
    options.exact_repair_limit = 2;
    options.seed_count = Some(1);
    board.placements[0].position = pt(10.0, 23.0);
    board.placements[1].position = pt(13.0, 23.0);
    let connector = footprint("wide-connector", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 12.0, 3.0);
    board.footprints.insert(connector.name.clone(), connector);
    board.placements.push(placement("NARROW", &board.placements[0].footprint, 16.0, 11.5));
    board.placements.push(placement("BROAD", "wide-connector", 16.0, 11.5));
    for (reference, outline) in
        [("NARROW", rectangle(6.0, 10.4, 17.0, 12.6)), ("BROAD", rectangle(4.0, 4.0, 27.0, 19.0))]
    {
        board.regions.push(PlacementRegion { name: reference.to_owned(), outline, side: None });
        board.placement_rules.push(ComponentPlacementRule {
            region: Some(reference.to_owned()),
            allowed_orientations: vec![Decimal::from_i64(0)],
            ..ComponentPlacementRule::new(reference)
        });
    }
    // Both parts fit, but a legal central landing splits the wide pocket.
    for (name, outline) in [
        ("above", rectangle(4.0, 4.0, 27.0, 9.4)),
        ("below", rectangle(4.0, 13.6, 27.0, 19.0)),
        ("left", rectangle(4.0, 9.4, 5.5, 13.6)),
        ("right", rectangle(25.5, 9.4, 27.0, 13.6)),
    ] {
        board.keepouts.push(PlacementKeepout::new(name, outline, None, None).unwrap());
    }
    (board, options)
}

#[test]
fn narrow_and_broad_overlapping_domains_are_one_joint_reservation() {
    let (board, options) = mixed_regional_fixture();
    let original = board.clone();
    let scene = Scene::new(&board, options.policy());
    let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let mut witness = targets.clone();
    witness.get_mut("NARROW").unwrap().position = pt(8.0, 11.5);
    witness.get_mut("BROAD").unwrap().position = pt(18.0, 11.5);
    assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
    for (reference, narrow) in [("NARROW", true), ("BROAD", false)] {
        let count = copper_place::legalize::candidate_positions(&scene, &targets[reference], &options).len();
        assert_eq!(count <= options.legalization_candidates as usize, narrow, "{reference}: {count}");
    }
    let held: Poses = [("NARROW".to_owned(), targets["NARROW"].clone())].into_iter().collect();
    assert!(scene.legal(&targets["NARROW"], &Poses::new()).unwrap());
    assert!(
        copper_place::legalize::pack_local_components(
            &scene,
            &["BROAD".to_owned()],
            &held,
            &targets,
            &options,
            true,
            false,
            &[],
        )
        .unwrap()
        .is_none(),
        "a greedy legal narrow reservation destroys the broad sampled domain"
    );
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&observer))
        .expect("overlapping narrow and broad domains must negotiate jointly");
    drop(observer);
    assert!(events.contains(&"region_pack:NARROW,BROAD".to_owned()));
    assert!(!events.contains(&"region_reserve:NARROW".to_owned()));
    let poses: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    assert!(scene.checker.placement_solution_is_legal(&poses).unwrap());
    assert!(cluster_placement_matches(&board, &poses).unwrap());
    assert_eq!(board, original);
}

#[test]
fn mixed_regional_packing_does_not_move_fixed_owners_or_accept_impossible_geometry() {
    let (board, mut options) = mixed_regional_fixture();
    options.fixed_references.insert("NARROW".to_owned());
    let original = board.clone();
    let mut failures = Vec::new();
    let mut observe = |failure: &copper_place::planner::PlacementFailureDiagnostic| failures.push(failure.clone());
    let result = generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe));
    assert!(result.is_err(), "the fixed central landing cannot be relocated");
    assert_eq!(board, original);
    assert!(!failures.is_empty());
    assert!(failures.iter().all(|failure| failure.accepted["NARROW"] == original.placements[2]));
}

fn movable_driver_with_large_switch() -> (PhysicalBoard, PlannerOptions) {
    let (mut board, mut options) = fixture();
    options.legalization_candidates = 24;
    options.exact_repair_candidates = 4;
    options.seed_count = Some(1);
    let fet = footprint("large-switch", vec![pad("G", -2.5, 0.0, 0.6, 0.6)], 6.0, 5.0);
    board.footprints.insert(fet.name.clone(), fet);
    board.placements.push(placement("FET", "large-switch", 10.0, 10.0));
    board.placements.push(placement("R_GATE", &board.placements[0].footprint, 7.0, 10.0));
    for (left, right, distance) in [(("A", "1"), ("FET", "G"), 8.0), (("FET", "G"), ("R_GATE", "1"), 2.0)] {
        board.relative_rules.push(RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: [left, right]
                .iter()
                .map(|(reference, pad)| PlacementTarget {
                    reference: (*reference).to_owned(),
                    pad: Some((*pad).to_owned()),
                })
                .collect(),
            distance_nm: Some(mm(distance)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        });
    }
    // A small driver/bypass macro fits the nearest 10x2 mm pocket. The larger
    // external switch does not, and its 8 mm gate limit cannot reach the other
    // room. A legal joint placement exists in the distant 14x22 mm room.
    for (name, outline) in [
        ("left-above", rectangle(0.0, 0.0, 24.0, 9.0)),
        ("left-below", rectangle(0.0, 11.0, 24.0, 30.0)),
        ("left-edge", rectangle(0.0, 9.0, 5.0, 11.0)),
        ("separator", rectangle(15.0, 9.0, 24.0, 11.0)),
        ("right-above", rectangle(24.0, 0.0, 40.0, 4.0)),
        ("right-below", rectangle(24.0, 26.0, 40.0, 30.0)),
        ("right-edge", rectangle(38.0, 4.0, 40.0, 26.0)),
    ] {
        board.keepouts.push(PlacementKeepout::new(name, outline, None, None).unwrap());
    }
    (board, options)
}

#[test]
fn movable_driver_macro_preserves_a_large_switch_and_gate_resistor_witness() {
    let (board, options) = movable_driver_with_large_switch();
    let original_board = board.clone();
    let scene = Scene::new(&board, options.policy());
    let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let mut witness = targets.clone();
    for (reference, position) in
        [("A", pt(28.0, 15.0)), ("B", pt(31.0, 15.0)), ("FET", pt(30.0, 8.0)), ("R_GATE", pt(25.5, 8.0))]
    {
        witness.get_mut(reference).unwrap().position = position;
    }
    assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
    assert!(cluster_placement_matches(&board, &witness).unwrap());
    assert_eq!(scene.penalty(&witness).unwrap(), 0);
    let held: Poses = targets
        .iter()
        .filter(|(reference, _)| *reference == "A" || *reference == "B")
        .map(|(reference, pose)| (reference.clone(), pose.clone()))
        .collect();
    assert!(
        copper_place::legalize::pack_local_components(
            &scene,
            &["FET".to_owned(), "R_GATE".to_owned()],
            &held,
            &targets,
            &options,
            true,
            false,
            &[]
        )
        .unwrap()
        .is_none(),
        "freezing the nearest driver pose strands its larger switch"
    );
    let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
    let (chosen, _, phase) = place_rigid_clusters(&scene, &targets, &targets, &options, &spacing).unwrap();
    assert!(chosen["A"].position.x_nm >= mm(24.0));
    assert!(!phase.fixed_references.contains("FET"));
    assert!(!phase.fixed_references.contains("R_GATE"));
    assert_eq!(scene.penalty(&chosen).unwrap(), 0);
    let mut events = Vec::new();
    let observer = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&observer)).unwrap();
    drop(observer);
    assert!(events.iter().any(|event| event.starts_with("cluster_witness:pair:")));
    assert!(!events.contains(&"legalize:FET".to_owned()), "ordinary legalization retains the mutable witness");
    let poses: Poses = candidates[0].placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    assert!(scene.checker.placement_solution_is_legal(&poses).unwrap());
    assert!(cluster_placement_matches(&board, &poses).unwrap());
    assert_eq!(scene.penalty(&poses).unwrap(), 0);
    assert_eq!(board, original_board);
}

#[test]
fn failed_macro_companion_trials_are_bounded_and_leave_no_accepted_partial_pose() {
    let (mut board, options) = movable_driver_with_large_switch();
    let fet = board.footprints.get_mut("large-switch").unwrap();
    fet.body_size.width_nm = mm(20.0);
    fet.body_size.height_nm = mm(20.0);
    let original = board.clone();
    let mut events = Vec::new();
    let progress = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let mut failures = Vec::new();
    let mut observe = |failure: &copper_place::planner::PlacementFailureDiagnostic| failures.push(failure.clone());
    let result = generate_placement_candidates_with_diagnostics(&board, &options, Some(&progress), Some(&mut observe));
    drop(progress);
    assert!(result.is_err());
    let trials = events.iter().filter(|event| event.starts_with("cluster_witness:pair:")).count();
    assert!(trials > 0 && trials <= 2 * options.exact_repair_candidates as usize);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].reason.contains("no sampled joint placement witness"));
    assert!(failures[0].accepted.is_empty(), "failed witnesses cannot leak macro or companion ownership");
    assert_eq!(board, original);
}

#[test]
fn a_source_fixed_driver_cannot_move_to_fit_its_external_switch() {
    let (board, mut options) = movable_driver_with_large_switch();
    options.fixed_references.insert("A".to_owned());
    let original = board.clone();
    let mut failures = Vec::new();
    let mut observe = |failure: &copper_place::planner::PlacementFailureDiagnostic| failures.push(failure.clone());
    assert!(generate_placement_candidates_with_diagnostics(&board, &options, None, Some(&mut observe)).is_err());
    assert!(failures.iter().all(|failure| failure.accepted["A"].position == pt(10.0, 10.0)));
    assert_eq!(board, original);
}

#[test]
fn large_companion_closures_keep_a_bounded_witness_and_place_the_remaining_members_normally() {
    let (mut board, mut options) = fixture();
    options.legalization_candidates = 24;
    options.exact_repair_limit = 2;
    options.exact_repair_candidates = 4;
    options.seed_count = Some(1);
    for index in 0..7 {
        let reference = format!("S{index}");
        board.placements.push(placement(&reference, &board.placements[0].footprint, 10.0, 10.0));
        board.relative_rules.push(RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: vec![
                PlacementTarget { reference: "A".to_owned(), pad: Some("1".to_owned()) },
                PlacementTarget { reference, pad: Some("1".to_owned()) },
            ],
            distance_nm: Some(mm(12.0)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        });
    }
    let original = board.clone();
    let mut events = Vec::new();
    let progress = RefCell::new(|stage: &str, _, _| events.push(stage.to_owned()));
    let candidates = generate_placement_candidates_with_progress(&board, &options, Some(&progress))
        .expect("a closure larger than the witness width still has its ordinary placement path");
    drop(progress);
    assert!(events.contains(&"cluster_companions_deferred:pair:5".to_owned()));
    let poses: Poses = candidates[0].placements.iter().map(|pose| (pose.reference.clone(), pose.clone())).collect();
    assert_eq!(poses.len(), 9);
    let scene = Scene::new(&board, options.policy());
    assert!(scene.checker.placement_solution_is_legal(&poses).unwrap());
    assert_eq!(scene.penalty(&poses).unwrap(), 0, "every deferred member retains its final distance constraint");
    assert!(cluster_placement_matches(&board, &poses).unwrap());
    assert_eq!(board, original);
}

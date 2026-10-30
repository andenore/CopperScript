//! Placement escape spacing and local companion packing (ported from the
//! former `tests/test_escape_placement.py`). Tests that replaced engine
//! internals with monkeypatched fakes are listed as skipped in
//! `crates/copper-route/tests/PORTED-routers.md`.

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use std::rc::Rc;

use boards::*;
use copper_drc::legality::polygons_too_close;
use copper_pcb::board::{
    AlignmentAxis, ComponentPlacementRule, NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement,
    PlacementRegion, PlacementTarget, RelativePlacementKind, RelativePlacementRule, RigidPlacementCluster,
    RigidPlacementMember, Stackup,
};
use copper_pcb::hard_macros::{cluster_placement_matches, cluster_placements};
use copper_physical::footprint::{PadKind, Size, footprint_geometry_digest};
use copper_physical::{BoardOutline, BoardSide, CopperLayer, DesignRules, Point};
use copper_place::escape::{EscapeChannel, EscapeSide, EscapeSpacingModel, placement_units};
use copper_place::legalize::{
    adjacency, anchored_candidate_rules, best_legal_choice, candidate_positions, pack_local_components,
    relative_candidates, relative_cluster_order,
};
use copper_place::metrics::{fast_score, hpwl, placement_metrics};
use copper_place::refine::spread_escape_components;
use copper_place::scene::{Poses, Scene};
use copper_place::seed::choose_orientations;
use copper_place::{PlannerOptions, plan_placement};
use copper_units::Decimal;
use indexmap::{IndexMap, IndexSet};

fn d(value: i64) -> Decimal {
    Decimal::from_i64(value)
}

fn fixture(companion: bool) -> PhysicalBoard {
    let mut pads = Vec::new();
    for side in 0..4 {
        for index in 0..16 {
            let tangent = (index as f64 - 7.5) * 0.5;
            let (x, y) = [(tangent, -5.75), (5.75, tangent), (tangent, 5.75), (-5.75, tangent)][side];
            let (w, h) = if side % 2 == 0 { (0.25, 1.5) } else { (1.5, 0.25) };
            pads.push(pad(&(side * 16 + index + 1).to_string(), x, y, w, h));
        }
    }
    let large = footprint("large", pads, 13.4, 13.4);
    let mut pads = Vec::new();
    for side in 0..4 {
        for index in 0..3 {
            let tangent = (index as f64 - 1.0) * 0.5;
            let (x, y) = [(tangent, -0.9), (0.9, tangent), (tangent, 0.9), (-0.9, tangent)][side];
            pads.push(pad(&(side * 3 + index + 1).to_string(), x, y, 0.28, 0.28));
        }
    }
    let small = footprint("small", pads, 2.5, 2.5);
    let cap = footprint("cap", vec![pad("1", -0.35, 0.0, 0.3, 0.3), pad("2", 0.35, 0.0, 0.3, 0.3)], 1.2, 0.8);
    let mut placements = vec![placement("U_MCU", "large", 20.0, 15.0), placement("U_SENSOR", "small", 20.0, 24.0)];
    let mut relatives = Vec::new();
    if companion {
        placements.push(placement("C1", "cap", 17.5, 24.0));
        relatives.push(RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: vec![target("U_SENSOR", None), target("C1", None)],
            distance_nm: Some(mm(3.0)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        });
    }
    let footprints = vec![large, small, cap];
    let by_name: IndexMap<String, Vec<String>> =
        footprints.iter().map(|f| (f.name.clone(), f.pads.iter().map(|p| p.number.clone()).collect())).collect();
    let gnd = PhysicalNet {
        name: "GND".to_owned(),
        pads: placements
            .iter()
            .flat_map(|p| by_name[&p.footprint].iter().map(|n| PadReference::new(&p.reference, n)))
            .collect(),
    };
    let mut board = board("escape", 60.0, 45.0, footprints, placements, vec![gnd]);
    board.relative_rules = relatives;
    board.rules = DesignRules {
        minimum_clearance_nm: mm(0.09),
        minimum_track_width_nm: mm(0.09),
        default_track_width_nm: mm(0.2),
        ..DesignRules::default()
    };
    board
}

fn target(reference: &str, pad: Option<&str>) -> PlacementTarget {
    PlacementTarget { reference: reference.to_owned(), pad: pad.map(str::to_owned) }
}

fn max_distance(a: (&str, Option<&str>), b: (&str, Option<&str>), distance: f64) -> RelativePlacementRule {
    RelativePlacementRule {
        kind: RelativePlacementKind::MaxDistance,
        targets: vec![target(a.0, a.1), target(b.0, b.1)],
        distance_nm: Some(mm(distance)),
        axis: None,
        tolerance_nm: 0,
        weight: 1,
    }
}

fn poses(board: &PhysicalBoard) -> Poses {
    board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect()
}

fn options() -> PlannerOptions {
    PlannerOptions { candidate_count: 1, analytical_iterations: 0, refinement_passes: 0, ..PlannerOptions::default() }
        .validated()
        .expect("options")
}

fn with_fixed(references: &[&str]) -> PlannerOptions {
    PlannerOptions { fixed_references: references.iter().map(|r| (*r).to_owned()).collect(), ..options() }
}

fn scene<'a>(board: &'a PhysicalBoard, options: &PlannerOptions) -> Scene<'a> {
    Scene::new(board, options.policy())
}

/// `EscapeSpacingModel(board)` at the default margin and transit lanes.
fn model<'a>(scene: &'a Scene<'a>) -> EscapeSpacingModel<'a> {
    let defaults = PlannerOptions::default();
    EscapeSpacingModel::new(scene, defaults.escape_margin_nm, defaults.escape_transit_lanes).expect("model")
}

fn legal(board: &PhysicalBoard, placements: &Poses, options: &PlannerOptions) -> bool {
    scene(board, options).checker.placement_solution_is_legal(placements).expect("legal")
}

fn channels(board: &PhysicalBoard, placements: &Poses) -> Vec<EscapeChannel> {
    let scene = scene(board, &options());
    model(&scene).channels(placements, false)
}

fn spread(board: &PhysicalBoard, source: &Poses, options: &PlannerOptions) -> (Poses, i64) {
    let scene = scene(board, options);
    let spacing =
        EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).expect("model");
    spread_escape_components(&scene, source, options, &spacing).expect("spread")
}

fn moved(pose: &Placement, x: f64, y: f64) -> Placement {
    Placement { position: pt(x, y), ..pose.clone() }
}

#[test]
fn dense_small_pair_has_a_directional_channel_and_both_via_banks() {
    let board = fixture(false);
    let found = channels(&board, &poses(&board));
    assert_eq!(found.len(), 1);
    let channel = &found[0];
    assert_eq!(channel.axis, "y");
    assert!(channel.left_side == EscapeSide::South && channel.right_side == EscapeSide::North);
    assert!(channel.left_depth_nm > 0 && channel.right_depth_nm > 0);
    assert!(channel.left_rows >= 1 && channel.right_rows >= 1);
    assert!(channel.required_gap_nm > mm(2.16));
    assert!(channel.deficit_nm() > 0);
}

#[test]
fn spreading_preserves_fixed_mcu_and_moves_sensor_with_its_decoupler() {
    let board = fixture(true);
    let source = poses(&board);
    let opts = with_fixed(&["U_MCU"]);
    assert!(legal(&board, &source, &opts));
    let (result, count) = spread(&board, &source, &opts);
    assert!(count > 0 && result["U_MCU"] == source["U_MCU"]);
    assert!(legal(&board, &result, &opts));
    assert!(channels(&board, &result).is_empty());
    let delta = |r: &str| {
        (result[r].position.x_nm - source[r].position.x_nm, result[r].position.y_nm - source[r].position.y_nm)
    };
    assert_eq!(delta("U_SENSOR"), delta("C1"));
}

#[test]
fn locked_short_channel_warns_instead_of_rejecting_or_unlocking() {
    let mut board = fixture(false);
    board.placement_rules = board
        .placements
        .iter()
        .map(|p| ComponentPlacementRule {
            fixed_position: Some(p.position),
            fixed_rotation_degrees: Some(p.rotation_degrees.clone()),
            ..ComponentPlacementRule::new(&p.reference)
        })
        .collect();
    let plan = plan_placement(&board, &options()).expect("plan");
    assert_eq!(plan.board.placements, board.placements);
    assert!(plan.report.metrics.escape_channel_deficit_nm > 0);
    assert!(plan.report.findings.iter().any(|f| f.code == "ESCAPE_CHANNEL_DEFICIT"));
    let document = plan.report.document();
    let channel = &document.get("escape_channels").and_then(|c| c.as_array()).expect("channels")[0];
    assert_eq!(channel.get("left").and_then(|v| v.as_str()), Some("U_MCU"));
    assert_eq!(channel.get("right").and_then(|v| v.as_str()), Some("U_SENSOR"));
    let int = |key: &str| match channel.get(key) {
        Some(copper_json::Value::Int(v)) => i64::try_from(v.clone()).expect("i64"),
        other => panic!("{key}: {other:?}"),
    };
    assert_eq!(int("deficit_nm"), int("required_gap_nm") - int("gap_nm"));
    assert_eq!(plan.board.metadata["fabrication_ready"], "false");
}

fn fast(board: &PhysicalBoard, placements: &Poses) -> [i64; 4] {
    let scene = scene(board, &PlannerOptions::default());
    let spacing = model(&scene);
    fast_score(&scene, placements, &spacing, PlannerOptions::default().power_domain_weight).expect("score")
}

fn quality(board: &PhysicalBoard, placements: &Poses) -> [i64; 10] {
    let opts = options();
    let scene = scene(board, &opts);
    let spacing = EscapeSpacingModel::new(&scene, opts.escape_margin_nm, opts.escape_transit_lanes).expect("model");
    placement_metrics(&scene, placements, &opts, &spacing).expect("metrics").quality_vector()
}

#[test]
fn escape_deficit_precedes_short_wire_length_in_local_and_final_scores() {
    let board = fixture(false);
    let original = poses(&board);
    let mut spread = original.clone();
    spread.insert("U_SENSOR".to_owned(), moved(&original["U_SENSOR"], 20.0, 30.0));
    assert!(fast(&board, &spread) < fast(&board, &original));
    assert!(quality(&board, &spread) < quality(&board, &original));
}

#[test]
fn no_net_no_demand_and_more_layers_do_not_erase_landing_space() {
    let board = fixture(false);
    let mut bare = board.clone();
    bare.nets.clear();
    assert!(channels(&bare, &poses(&board)).is_empty());
    let mut more = board.clone();
    more.stackup = Stackup::new(layers(6));
    assert_eq!(channels(&board, &poses(&board)), channels(&more, &poses(&more)));
}

#[test]
fn via_size_drill_spacing_margin_and_transit_lanes_change_target() {
    let board = fixture(false);
    let base = channels(&board, &poses(&board))[0].required_gap_nm;
    let mut larger = board.clone();
    larger.rules.default_via_size_nm = mm(1.2);
    assert!(channels(&larger, &poses(&larger))[0].required_gap_nm > base);
    let mut drill = board.clone();
    drill.rules.minimum_hole_clearance_nm = mm(1.0);
    assert!(channels(&drill, &poses(&drill))[0].required_gap_nm > base);
    let scene = scene(&board, &options());
    let defaults = PlannerOptions::default();
    let margin = EscapeSpacingModel::new(&scene, mm(1.0), defaults.escape_transit_lanes).expect("model");
    assert!(margin.channels(&poses(&board), false)[0].required_gap_nm > base);
    let lanes = EscapeSpacingModel::new(&scene, defaults.escape_margin_nm, 2).expect("model");
    assert!(lanes.channels(&poses(&board), false)[0].required_gap_nm > base);
}

#[test]
fn surface_only_restriction_does_not_invent_via_banks() {
    let mut board = fixture(false);
    board.net_routing_rules =
        vec![NetRoutingRule { allowed_layers: vec![CopperLayer::Front], max_vias: Some(0), ..rule("GND") }];
    let scene = scene(&board, &options());
    let channel = model(&scene).channels(&poses(&board), true).remove(0);
    assert!(channel.left_rows == 0 && channel.right_rows == 0);
    assert!(channel.required_gap_nm > 0);
}

#[test]
fn opposite_board_sides_are_not_a_surface_escape_channel() {
    let board = fixture(false);
    let mut positions = poses(&board);
    positions["U_SENSOR"].side = BoardSide::Back;
    assert!(channels(&board, &positions).is_empty());
}

#[test]
fn profile_rotations_and_mirrors_are_cached_without_xy_or_stale_rules() {
    // Python also checked that the cached profile is immutable; the Rust
    // profile is shared behind an `Rc` and cannot be mutated.
    let board = fixture(false);
    let scene = scene(&board, &options());
    let model = model(&scene);
    let pose = &board.placements[1];
    let local = model.profile(pose);
    assert!(Rc::ptr_eq(&model.profile(&moved(pose, 35.0, 35.0)), &local));
    let turned = model.profile(&Placement { rotation_degrees: d(45), side: BoardSide::Back, ..pose.clone() });
    assert!(!Rc::ptr_eq(&turned, &local) && turned.bounds.max_x > local.bounds.max_x);
    let mut changed = board.clone();
    changed.rules.default_via_size_nm = mm(1.2);
    let changed_scene = self::scene(&changed, &options());
    assert_ne!(self::model(&changed_scene).profile(pose).sides, local.sides);
}

#[test]
fn passive_is_a_blocker_without_its_own_bank_but_companion_is_exempt() {
    let board = fixture(true);
    let mut source = poses(&board);
    source["C1"].position = pt(20.0, 22.5);
    let mut independent = board.clone();
    independent.relative_rules.clear();
    assert!(channels(&independent, &source).iter().any(|c| c.right == "U_MCU" || c.left == "U_MCU"));
    let scene = scene(&board, &options());
    let model = model(&scene);
    assert!(model.profile(&source["C1"]).sides.iter().all(|demand| demand.pad_count == 0));
    let pair: IndexSet<&str> = IndexSet::from(["C1", "U_SENSOR"]);
    assert!(
        !model.channels(&source, false).iter().any(|c| IndexSet::from([c.left.as_str(), c.right.as_str()]) == pair)
    );
}

#[test]
fn spreading_is_bounded_disabled_and_deterministic() {
    let board = fixture(false);
    let source = poses(&board);
    assert_eq!(spread(&board, &source, &PlannerOptions { escape_spacing_passes: 0, ..options() }), (source.clone(), 0));
    assert_eq!(spread(&board, &source, &options()), spread(&board, &source, &options()));
    let tight = PlannerOptions { escape_max_movement_nm: mm(0.2), ..with_fixed(&["U_MCU"]) };
    let (result, count) = spread(&board, &source, &tight);
    for reference in source.keys() {
        assert!((result[reference].position.x_nm - source[reference].position.x_nm).abs() <= count * mm(0.2));
        assert!((result[reference].position.y_nm - source[reference].position.y_nm).abs() <= count * mm(0.2));
    }
}

#[test]
fn pair_cache_matches_fresh_models_after_pose_changes_and_reverts() {
    // Python also bounded the private cache sizes; the kept property is that
    // a reused model answers as a fresh one.
    let board = fixture(true);
    let scene = scene(&board, &options());
    let model = model(&scene);
    let initial = poses(&board);
    let variant = |change: &dyn Fn(&mut Placement)| {
        let mut trial = initial.clone();
        change(&mut trial["U_SENSOR"]);
        trial
    };
    let trials = [
        initial.clone(),
        variant(&|p| p.position = pt(35.0, 30.0)),
        variant(&|p| p.rotation_degrees = d(45)),
        variant(&|p| p.side = BoardSide::Back),
        initial.clone(),
    ];
    for _ in 0..2 {
        for trial in &trials {
            for include_clear in [false, true] {
                let fresh_scene = self::scene(&board, &options());
                assert_eq!(
                    model.channels(trial, include_clear),
                    self::model(&fresh_scene).channels(trial, include_clear)
                );
            }
        }
    }
}

/// A small deterministic generator for the trial poses (Python used `random.Random(41)`).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

fn deficit_of(board: &PhysicalBoard, placements: &Poses) -> i64 {
    channels(board, placements).iter().map(EscapeChannel::deficit_nm).sum()
}

#[test]
fn incremental_deficit_matches_full_geometry_through_trials_and_inventory_changes() {
    let mut rng = Lcg(41);
    let board = fixture(true);
    let scene = scene(&board, &options());
    let model = model(&scene);
    let initial = poses(&board);
    let mut current = initial.clone();
    let mut trials = vec![current.clone()];
    let mut references: Vec<String> = current.keys().cloned().collect();
    references.sort();
    for _ in 0..40 {
        let first = rng.next(references.len() as u64) as usize;
        let second = (first + 1 + rng.next(references.len() as u64 - 1) as usize) % references.len();
        for reference in [&references[first], &references[second]] {
            let pose = &mut current[reference.as_str()];
            pose.position = pt(20.0 + rng.next(20) as f64, 20.0 + rng.next(20) as f64);
            pose.rotation_degrees = d([0, 45, 90, 180][rng.next(4) as usize]);
            pose.side = if rng.next(2) == 0 { BoardSide::Front } else { BoardSide::Back };
        }
        trials.push(current.clone());
    }
    let mut without_c1 = initial.clone();
    without_c1.shift_remove("C1");
    trials.extend([initial.clone(), without_c1, Poses::new(), initial.clone()]);
    for trial in &trials {
        let expected = deficit_of(&board, trial);
        assert_eq!(model.deficit_nm(trial), expected);
        // Full diagnostics may populate pair caches between scoring calls.
        model.channels(&initial, true);
        assert_eq!(model.deficit_nm(trial), expected);
    }
}

#[test]
fn deficit_after_a_single_move_matches_full_geometry() {
    // Python also counted the pairs the incremental update revisits (by
    // patching `channels`); the kept property is the exact incremental result.
    let base = fixture(false);
    let template = base.placements[1].clone();
    let placements: Vec<Placement> = (0..30)
        .map(|i| Placement { reference: format!("U{i}"), position: pt(i as f64 * 3.0, 20.0), ..template.clone() })
        .collect();
    let pads = placements
        .iter()
        .flat_map(|p| base.footprints[&p.footprint].pads.iter().map(|pad| PadReference::new(&p.reference, &pad.number)))
        .collect();
    let mut board = base.clone();
    board.placements = placements;
    board.nets = vec![PhysicalNet { name: "GND".to_owned(), pads }];
    let scene = scene(&board, &options());
    let model = model(&scene);
    let initial = poses(&board);
    model.deficit_nm(&initial);
    let mut trial = initial.clone();
    trial["U0"].position = pt(4.0, 20.0);
    assert_eq!(model.deficit_nm(&trial), deficit_of(&board, &trial));
    assert_eq!(model.deficit_nm(&trial), deficit_of(&board, &trial));
}

#[test]
fn spacing_moves_whole_rigid_unit_without_changing_member_poses() {
    for angle in [0, 45] {
        let mut board = fixture(true);
        let mut initial = poses(&board);
        let anchor = initial["U_SENSOR"].position;
        let members = ["U_SENSOR", "C1"]
            .iter()
            .map(|r| {
                let pose = &initial[*r];
                let digest = footprint_geometry_digest(&board.footprints[&pose.footprint]);
                let offset = Point::new(pose.position.x_nm - anchor.x_nm, pose.position.y_nm - anchor.y_nm);
                RigidPlacementMember::new(r, &pose.footprint, &digest, offset, &d(0)).expect("member")
            })
            .collect();
        let cluster = RigidPlacementCluster {
            name: "sensor-macro".to_owned(),
            anchor_reference: "U_SENSOR".to_owned(),
            anchor_pad: None,
            members,
            source: "synthetic placement regression".to_owned(),
            allowed_rotations: vec![d(0), d(45)],
            keepouts: Vec::new(),
            internal_clearance_nm: None,
            edge_openings: Vec::new(),
        }
        .normalized()
        .expect("cluster");
        board.rigid_clusters = vec![cluster.clone()];
        board.placement_rules = ["U_SENSOR", "C1"]
            .iter()
            .map(|r| ComponentPlacementRule {
                allowed_orientations: vec![d(0), d(45), d(90)],
                ..ComponentPlacementRule::new(r)
            })
            .collect();
        let turned = Placement { rotation_degrees: d(angle), ..initial["U_SENSOR"].clone() };
        let members = cluster_placements(&cluster, &turned, &initial).expect("members");
        initial.extend(members);
        let opts = with_fixed(&["U_MCU"]);
        assert!(legal(&board, &initial, &opts));
        let scene = scene(&board, &opts);
        let spacing = model(&scene);
        let pair: IndexSet<&str> = IndexSet::from(["U_SENSOR", "C1"]);
        assert!(
            !spacing
                .channels(&initial, true)
                .iter()
                .any(|c| IndexSet::from([c.left.as_str(), c.right.as_str()]) == pair)
        );
        let (result, count) = spread_escape_components(&scene, &initial, &opts, &spacing).expect("spread");
        assert!(count > 0 && result["U_MCU"] == initial["U_MCU"]);
        assert!(cluster_placement_matches(&board, &result).expect("matches"));
        assert!(legal(&board, &result, &opts));
        assert!(spacing.channels(&result, false).is_empty());
        assert!(result.iter().all(|(r, p)| p.rotation_degrees == initial[r].rotation_degrees));
    }
}

#[test]
fn through_hole_pads_do_not_require_surface_via_banks() {
    let mut board = fixture(false);
    for footprint in board.footprints.values_mut() {
        for pad in &mut footprint.pads {
            pad.kind = PadKind::ThroughHole;
            pad.drill = Some(Size { width_nm: mm(0.1), height_nm: mm(0.1) });
        }
    }
    assert!(channels(&board, &poses(&board)).is_empty());
}

#[test]
fn tight_region_retains_unmet_spacing_without_breaking_hard_legality() {
    let mut board = fixture(false);
    board.regions = vec![PlacementRegion {
        name: "sensor-window".to_owned(),
        outline: BoardOutline::rectangle(mm(2.5), mm(2.5), Some(pt(18.75, 22.75))).expect("outline"),
        side: None,
    }];
    board.placement_rules = vec![ComponentPlacementRule {
        region: Some("sensor-window".to_owned()),
        allowed_orientations: vec![d(0)],
        ..ComponentPlacementRule::new("U_SENSOR")
    }];
    let source = poses(&board);
    let opts = with_fixed(&["U_MCU"]);
    let (result, count) = spread(&board, &source, &opts);
    assert!(result == source && count == 0);
    assert!(legal(&board, &result, &opts));
    assert!(!channels(&board, &result).is_empty());
}

#[test]
fn invalid_escape_options_fail() {
    for change in [
        PlannerOptions { escape_margin_nm: -1, ..PlannerOptions::default() },
        PlannerOptions { escape_transit_lanes: -1, ..PlannerOptions::default() },
        PlannerOptions { escape_spacing_passes: -1, ..PlannerOptions::default() },
        PlannerOptions { escape_max_movement_nm: 0, ..PlannerOptions::default() },
    ] {
        let error = change.validated().unwrap_err();
        assert!(error.to_string().contains("escape"), "{error}");
    }
}

fn local_companion_board() -> PhysicalBoard {
    let chip = footprint("chip", vec![pad("1", 1.0, -0.6, 0.3, 0.3), pad("2", 1.0, 0.6, 0.3, 0.3)], 2.0, 2.0);
    let cap = footprint("capacitor", vec![pad("1", -0.3, 0.0, 0.3, 0.3), pad("2", 0.3, 0.0, 0.3, 0.3)], 1.0, 0.5);
    let mut board = board(
        "local",
        30.0,
        30.0,
        vec![chip, cap],
        vec![
            placement("U", "chip", 10.0, 10.0),
            placement("C1", "capacitor", 20.0, 20.0),
            placement("C2", "capacitor", 23.0, 20.0),
        ],
        Vec::new(),
    );
    board.placement_rules = vec![fixed_rule("U", 10.0, 10.0)];
    board.relative_rules =
        (1..=2).map(|i| max_distance((&format!("C{i}"), Some("1")), ("U", Some(&i.to_string())), 1.5)).collect();
    board
}

fn fixed_rule(reference: &str, x: f64, y: f64) -> ComponentPlacementRule {
    ComponentPlacementRule {
        fixed_position: Some(pt(x, y)),
        fixed_rotation_degrees: Some(d(0)),
        ..ComponentPlacementRule::new(reference)
    }
}

fn region(name: &str, corners: [(f64, f64); 4]) -> PlacementRegion {
    PlacementRegion {
        name: name.to_owned(),
        outline: BoardOutline::new(corners.iter().map(|(x, y)| pt(*x, *y)).collect(), Vec::new(), None, None)
            .expect("outline"),
        side: None,
    }
}

fn plan(board: &PhysicalBoard) -> copper_place::PlacementPlan {
    plan_placement(board, &options()).expect("plan")
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn fixed_anchor_companions_share_order_but_stay_flexible_and_legal() {
    let mut board = local_companion_board();
    board.placements.push(placement("X", "chip", 12.0, 10.0));
    let scene = scene(&board, &options());
    assert_eq!(relative_cluster_order(&scene, &names(&["X", "C2", "C1"]), &poses(&board)), names(&["C1", "C2", "X"]));
    let first = plan(&board);
    let second = plan(&board);
    assert_eq!(first.board, second.board);
    assert_eq!(first.report.document(), second.report.document());
    let placed = poses(&first.board);
    assert_eq!(placed["U"], poses(&board)["U"]);
    assert!(legal(&first.board, &placed, &options()));
    // Flexible proximity does not preserve the arbitrary original separation.
    let original = poses(&board);
    assert_ne!(
        placed["C2"].position.x_nm - placed["C1"].position.x_nm,
        original["C2"].position.x_nm - original["C1"].position.x_nm
    );
}

#[test]
fn minimum_distance_and_alignment_do_not_merge_local_units() {
    let mut board = local_companion_board();
    board.relative_rules = vec![
        RelativePlacementRule {
            kind: RelativePlacementKind::MinDistance,
            targets: vec![target("C1", None), target("U", None)],
            distance_nm: Some(mm(2.0)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        },
        RelativePlacementRule {
            kind: RelativePlacementKind::Align,
            targets: vec![target("C2", None), target("U", None)],
            distance_nm: None,
            axis: Some(AlignmentAxis::X),
            tolerance_nm: 0,
            weight: 1,
        },
    ];
    let rules: Vec<&RelativePlacementRule> = board.relative_rules.iter().collect();
    let units = placement_units(&board, &rules);
    assert_eq!(units["C1"].iter().collect::<Vec<_>>(), ["C1"]);
    assert_eq!(units["C2"].iter().collect::<Vec<_>>(), ["C2"]);
    let scene = scene(&board, &options());
    assert_eq!(relative_cluster_order(&scene, &names(&["C2", "C1"]), &poses(&board)), names(&["C2", "C1"]));
}

#[test]
fn local_xy_candidates_find_off_grid_slot_missed_by_eight_rays() {
    for (slot_left, slot_right) in [(11.66, 12.70), (11.72, 12.74)] {
        let mut board = local_companion_board();
        let chip = board.footprints.get_mut("chip").expect("chip");
        chip.pads.truncate(1);
        chip.pads[0].position = pt(1.0, 0.0);
        board.placements = vec![moved(&board.placements[0], 10.13, 10.27), board.placements[1].clone()];
        board.placement_rules = vec![
            fixed_rule("U", 10.13, 10.27),
            ComponentPlacementRule {
                region: Some("slot".to_owned()),
                fixed_rotation_degrees: Some(d(0)),
                ..ComponentPlacementRule::new("C1")
            },
        ];
        board.regions =
            vec![region("slot", [(slot_left, 10.64), (slot_right, 10.64), (slot_right, 11.29), (slot_left, 11.29)])];
        board.relative_rules =
            vec![RelativePlacementRule { distance_nm: Some(mm(1.1)), ..board.relative_rules[0].clone() }];
        let planned = plan(&board);
        assert!(legal(&planned.board, &poses(&planned.board), &options()));
        let cap = &poses(&planned.board)["C1"];
        assert_ne!(cap.position.x_nm % mm(1.0), 0);
        assert_ne!(cap.position.y_nm % mm(1.0), 0);
    }
}

#[test]
fn pad_distance_prefers_facing_pin_even_with_identical_global_wirelength() {
    let mut board = local_companion_board();
    board.placements = vec![
        board.placements[0].clone(),
        Placement { position: pt(12.1, 9.4), rotation_degrees: d(180), ..board.placements[1].clone() },
        placement("P", "capacitor", 5.0, 5.0),
        placement("Q", "capacitor", 25.0, 25.0),
    ];
    board.relative_rules =
        vec![RelativePlacementRule { distance_nm: Some(mm(2.0)), ..board.relative_rules[0].clone() }];
    board.nets = vec![net("POWER", &[("U", "1"), ("C1", "1"), ("P", "1"), ("Q", "1")])];
    let original = poses(&board);
    let mut facing = original.clone();
    facing["C1"].rotation_degrees = d(0);
    let scene = scene(&board, &PlannerOptions::default());
    assert_eq!(hpwl(&scene, &facing).expect("hpwl"), hpwl(&scene, &original).expect("hpwl"));
    let local = |p: &Poses| scene.local_connection_length(p, &scene.relative).expect("length");
    assert!(local(&facing) < local(&original));
    assert!(fast(&board, &facing) < fast(&board, &original));
    assert!(quality(&board, &facing) < quality(&board, &original));
    let spacing = model(&scene);
    let fixed: IndexSet<String> = ["U", "P", "Q"].iter().map(|s| (*s).to_owned()).collect();
    let chosen =
        choose_orientations(&scene, &original, &fixed, &spacing, PlannerOptions::default().power_domain_weight)
            .expect("orientations");
    assert_eq!(chosen["C1"].rotation_degrees, d(0));
}

#[test]
fn shared_companion_never_translates_either_fixed_anchor() {
    let mut board = local_companion_board();
    board
        .footprints
        .insert("small-chip".to_owned(), footprint("small-chip", vec![pad("1", 0.0, 0.0, 0.2, 0.2)], 1.0, 1.0));
    board.placements = vec![
        placement("U", "small-chip", 8.0, 10.0),
        board.placements[1].clone(),
        placement("V", "small-chip", 12.0, 10.0),
    ];
    board.placement_rules = vec![fixed_rule("U", 8.0, 10.0), fixed_rule("V", 12.0, 10.0)];
    board.relative_rules = vec![
        max_distance(("C1", Some("1")), ("U", Some("1")), 2.2),
        max_distance(("C1", Some("2")), ("V", Some("1")), 2.2),
    ];
    let result = plan(&board).board;
    assert!(legal(&result, &poses(&result), &options()));
    assert_eq!(poses(&result)["U"], poses(&board)["U"]);
    assert_eq!(poses(&result)["V"], poses(&board)["V"]);
}

fn capacitor_macro(board: &PhysicalBoard, at: [(&str, f64); 2]) -> RigidPlacementCluster {
    let fp = &board.footprints["capacitor"];
    let digest = footprint_geometry_digest(fp);
    RigidPlacementCluster {
        name: "unrelated".to_owned(),
        anchor_reference: "M1".to_owned(),
        anchor_pad: None,
        members: at
            .iter()
            .map(|(r, x)| RigidPlacementMember::new(r, &fp.name, &digest, pt(*x, 0.0), &d(0)).expect("member"))
            .collect(),
        source: "synthetic regression".to_owned(),
        allowed_rotations: vec![d(0)],
        keepouts: Vec::new(),
        internal_clearance_nm: None,
        edge_openings: Vec::new(),
    }
    .normalized()
    .expect("cluster")
}

#[test]
fn fixed_anchor_companion_is_reserved_before_unrelated_movable_macro() {
    use copper_place::clusters::place_rigid_clusters;
    let mut board = local_companion_board();
    board.placements = vec![
        board.placements[0].clone(),
        board.placements[1].clone(),
        placement("M1", "capacitor", 12.0, 9.4),
        placement("M2", "capacitor", 14.0, 9.4),
    ];
    board.relative_rules.truncate(1);
    board.rigid_clusters = vec![capacitor_macro(&board, [("M1", 0.0), ("M2", 2.0)])];
    let opts = options();
    let scene = scene(&board, &opts);
    let spacing = EscapeSpacingModel::new(&scene, opts.escape_margin_nm, opts.escape_transit_lanes).expect("model");
    let (targets, _, phase_options) =
        place_rigid_clusters(&scene, &poses(&board), &poses(&board), &opts, &spacing).expect("clusters");
    assert_eq!(targets["U"], poses(&board)["U"]);
    let others: Poses = ["U", "M1", "M2"].iter().map(|r| ((*r).to_owned(), targets[*r].clone())).collect();
    assert!(scene.legal(&targets["C1"], &others).expect("legal"));
    assert_ne!(targets["M1"], poses(&board)["M1"]);
    assert!(!phase_options.fixed_references.contains("C1"), "proximity remains flexible");
    assert!(phase_options.fixed_references.contains("M1") && phase_options.fixed_references.contains("M2"));
    assert!(cluster_placement_matches(&board, &targets).expect("matches"));
    assert!(legal(&board, &targets, &opts));
}

#[test]
fn failed_local_relation_reports_measured_limit_and_blockers_without_moving_lock() {
    let mut board = local_companion_board();
    board.placements.truncate(2);
    board.relative_rules =
        vec![RelativePlacementRule { distance_nm: Some(mm(0.1)), ..board.relative_rules[0].clone() }];
    let error = plan_placement(&board, &options()).err().expect("planning error").to_string();
    assert!(error.contains("maximum distance 0.1 mm, actual"), "{error}");
    assert!(error.contains("local candidate blockers: U"), "{error}");
    assert_eq!(board.placements[0].position, pt(10.0, 10.0));
}

fn only_slot_board(movable_macro: bool) -> PhysicalBoard {
    let mut board = local_companion_board();
    board.relative_rules = ["C1", "C2"].iter().map(|r| max_distance((r, Some("1")), ("U", Some("1")), 2.0)).collect();
    board.placement_rules.push(ComponentPlacementRule {
        region: Some("only-slot".to_owned()),
        fixed_rotation_degrees: Some(d(0)),
        ..ComponentPlacementRule::new("C2")
    });
    board.regions = vec![region("only-slot", [(11.5, 9.15), (12.5, 9.15), (12.5, 9.65), (11.5, 9.65)])];
    if movable_macro {
        board.placements.push(placement("M1", "capacitor", 22.0, 22.0));
        board.placements.push(placement("M2", "capacitor", 24.0, 22.0));
        board.rigid_clusters = vec![capacitor_macro(&board, [("M1", 0.0), ("M2", 2.0)])];
    }
    board
}

#[test]
fn joint_packing_reserves_the_only_slot_for_the_smallest_feasible_domain() {
    for movable_macro in [false, true] {
        let board = only_slot_board(movable_macro);
        let initial = poses(&board);
        let opts = options();
        let scene = scene(&board, &opts);
        let spacing = EscapeSpacingModel::new(&scene, opts.escape_margin_nm, opts.escape_transit_lanes).expect("model");
        let anchor: Poses = [("U".to_owned(), initial["U"].clone())].into_iter().collect();
        let greedy = best_legal_choice(
            &scene,
            "C1",
            &initial["C1"],
            &anchor,
            &adjacency(&scene),
            &opts,
            &spacing,
            0,
            Some(opts.legalization_candidates as usize),
        )
        .expect("choice")
        .expect("a greedy pose");
        let only_pose = Placement { position: pt(12.0, 9.4), rotation_degrees: d(0), ..initial["C2"].clone() };
        let mut around = anchor.clone();
        around.insert("C1".to_owned(), greedy);
        assert!(!scene.legal(&only_pose, &around).expect("legal"));
        let planned = plan_placement(&board, &opts).expect("plan");
        let placed = poses(&planned.board);
        assert_eq!(placed["C2"], only_pose);
        assert_eq!(placed["U"], initial["U"]);
        assert!(legal(&planned.board, &placed, &opts));
        assert!(cluster_placement_matches(&planned.board, &placed).expect("matches"));
        assert!(planned.report.candidates[0].statistics.exact_repair_count > 0);
        assert_eq!(planned.board, plan_placement(&board, &opts).expect("plan").board);
    }
}

#[test]
fn joint_packing_refines_a_narrow_row_when_individual_coarse_poses_conflict() {
    let mut board = local_companion_board();
    board.regions = [("C1", 9.55, 9.70), ("C2", 10.50, 10.65)]
        .iter()
        .map(|(r, low, high)| {
            region(r, [(11.55, low - 0.25), (12.55, low - 0.25), (12.55, high + 0.25), (11.55, high + 0.25)])
        })
        .collect();
    for r in ["C1", "C2"] {
        board.placement_rules.push(ComponentPlacementRule {
            region: Some(r.to_owned()),
            fixed_rotation_degrees: Some(d(0)),
            ..ComponentPlacementRule::new(r)
        });
    }
    let initial = poses(&board);
    let opts = options();
    let scene = scene(&board, &opts);
    let fixed: Poses = [("U".to_owned(), initial["U"].clone())].into_iter().collect();
    let coarse = |r: &str| -> Vec<Placement> {
        relative_candidates(&scene, &scene.relative, r, &initial[r], &fixed, &opts, 250_000)
            .expect("candidates")
            .into_iter()
            .filter(|p| scene.legal(p, &fixed).expect("legal"))
            .collect()
    };
    let (left, right) = (coarse("C1"), coarse("C2"));
    assert!(!left.is_empty() && !right.is_empty());
    for l in &left {
        let mut with_left = fixed.clone();
        with_left.insert("C1".to_owned(), l.clone());
        assert!(!right.iter().any(|r| scene.legal(r, &with_left).expect("legal")));
    }
    let result = pack_local_components(&scene, &names(&["C1", "C2"]), &fixed, &initial, &opts, false, false, &[])
        .expect("pack")
        .expect("a packing");
    assert_eq!(result["U"], initial["U"]);
    assert!(legal(&board, &result, &opts));
}

#[test]
fn anchored_neighborhood_only_composes_identical_pad_targets() {
    for intermediate_pad in ["1", "2"] {
        let mut board = local_companion_board();
        board.relative_rules =
            vec![board.relative_rules[0].clone(), max_distance(("C2", Some("1")), ("C1", Some(intermediate_pad)), 1.5)];
        let anchors: Poses = [("U".to_owned(), board.placements[0].clone())].into_iter().collect();
        let scene = scene(&board, &options());
        let derived = anchored_candidate_rules(&scene, &anchors);
        if intermediate_pad == "1" {
            assert_eq!(derived, [max_distance(("C2", Some("1")), ("U", Some("1")), 3.0)]);
        } else {
            assert!(derived.is_empty(), "distinct pins cannot be treated as the same point");
        }
        let mut reversed = board.clone();
        reversed.relative_rules.reverse();
        let reversed_scene = self::scene(&reversed, &options());
        assert_eq!(derived, anchored_candidate_rules(&reversed_scene, &anchors));
    }
}

#[test]
fn indirect_companion_joins_anchor_pack_and_keeps_exact_pair_distance() {
    let mut board = local_companion_board();
    board.relative_rules =
        vec![board.relative_rules[0].clone(), max_distance(("C2", Some("1")), ("C1", Some("1")), 1.5)];
    let planned = plan(&board);
    assert!(planned.report.candidates[0].statistics.exact_repair_count > 0);
    assert!(legal(&planned.board, &poses(&planned.board), &options()));
    assert_eq!(planned.board.relative_rules, board.relative_rules, "candidate reach never becomes a source constraint");
    assert_eq!(poses(&planned.board)["U"], poses(&board)["U"]);
}

#[test]
fn rectangular_courtyard_clearance_matches_edges_and_diagonals() {
    let square = |dx: f64, dy: f64| -> Vec<Point> {
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|(x, y)| pt(x + dx, y + dy)).collect()
    };
    for ((dx, dy), clearance, expected) in
        [((1.3, 1.4), 0.5, false), ((1.3, 1.4), 0.51, true), ((0.5, 0.5), 0.0, true), ((1.0, 0.0), 0.0, false)]
    {
        assert_eq!(
            polygons_too_close(&square(0.0, 0.0), &square(dx, dy), mm(clearance)),
            expected,
            "{dx},{dy} at {clearance}"
        );
    }
}

#[test]
fn distant_obstacles_do_not_change_local_candidate_sampling() {
    let mut board = local_companion_board();
    let far = placement("X", "capacitor", 12.0, 24.0);
    board.placements.push(far.clone());
    let initial = poses(&board);
    let opts = options();
    let scene = scene(&board, &opts);
    let anchor: Poses = [("U".to_owned(), initial["U"].clone())].into_iter().collect();
    let mut with_far = anchor.clone();
    with_far.insert("X".to_owned(), far);
    let sample = |placed: &Poses| {
        relative_candidates(&scene, &scene.relative, "C1", &initial["C1"], placed, &opts, 250_000).expect("candidates")
    };
    assert_eq!(sample(&anchor), sample(&with_far));
}

#[test]
fn region_sampling_does_not_add_a_second_board_edge_clearance() {
    // Python also called the private `_project_coordinate`; the sampled
    // candidates carry the same property.
    let mut board = local_companion_board();
    board.placement_rules.push(ComponentPlacementRule {
        region: Some("local-bank".to_owned()),
        fixed_rotation_degrees: Some(d(0)),
        ..ComponentPlacementRule::new("C1")
    });
    board.regions = vec![region("local-bank", [(18.0, 18.0), (20.0, 18.0), (20.0, 20.0), (18.0, 20.0)])];
    let target = poses(&board)["C1"].clone();
    let scene = scene(&board, &options());
    let candidates = candidate_positions(&scene, &target, &options());
    assert!(candidates.contains(&pt(19.0, 19.0)));
    assert!(candidates.iter().all(|p| {
        scene
            .legal(&Placement { position: *p, rotation_degrees: d(0), ..target.clone() }, &Poses::new())
            .expect("legal")
    }));
}

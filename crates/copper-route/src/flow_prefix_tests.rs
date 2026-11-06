//! Real decoupler-swap coverage for the complete lattice planning frame.

use super::*;
use crate::test_support_routers as routers;
use copper_pcb::board::{PadReference, PhysicalNet};

fn recognized_inner_scene() -> PhysicalBoard {
    let mut source = crate::test_support_planes::support_planes::ball_grid(6, 0.45, &[]);
    source.nets.retain(|net| net.name == "N15");
    source.placements.iter_mut().find(|pose| pose.reference == "J").unwrap().position =
        Point::new(30_000_000, 30_000_000);
    source
}

fn add_pending_inner_grid(board: &mut PhysicalBoard) -> Lattice {
    let template = crate::test_support_planes::support_planes::ball_grid(6, 0.45, &[]);
    board.outline = template.outline.clone();
    let mut footprint = template.footprints["bga"].clone();
    footprint.name = "test/mandatory-inner-grid".to_owned();
    let mut pose = template.placements.iter().find(|pose| pose.reference == "U").unwrap().clone();
    pose.reference = "U_INNER".to_owned();
    pose.footprint = footprint.name.clone();
    pose.position = Point::new(40_000_000, 20_000_000);
    board.footprints.insert(footprint.name.clone(), footprint);
    board.placements.push(pose);
    board.nets.push(PhysicalNet { name: "INNER".to_owned(), pads: vec![PadReference::new("U_INNER", "15")] });
    board.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        net: "INNER".to_owned(),
        kind: copper_pcb::board::RouteKind::Power,
        max_vias: Some(0),
        ..crate::test_support_planes::support_planes::rule("INNER")
    });
    let plan =
        plan_lattice(board, Some(&EscapePlanOptions { via_lattice: true, ..Default::default() }), None, &mut None)
            .expect("actual source BGA census")
            .expect("connected mandatory inner ball");
    let coverage = plan.packages.iter().find(|package| package.component == "U_INNER").unwrap();
    assert_eq!(coverage.coverage.len(), 1);
    assert!(coverage.coverage[0].mandatory_inner);
    assert!(matches!(coverage.coverage[0].status, crate::via_lattice::BallSiteStatus::Pending(_)));
    assert!(plan.tracks.is_empty() && plan.vias.is_empty());
    plan
}

#[test]
fn retained_flow_frame_is_the_critical_input_and_is_not_joined_twice() {
    let source = recognized_inner_scene();
    let lattice =
        plan_lattice(&source, Some(&EscapePlanOptions { via_lattice: true, ..Default::default() }), None, &mut None)
            .expect("real ball-grid planner")
            .expect("recognized connected inner ball");
    assert!(lattice.packages[0].coverage[0].mandatory_inner);
    assert_eq!(lattice.packages[0].coverage[0].status, crate::via_lattice::BallSiteStatus::Allocated);
    assert_eq!(lattice.tracks.len(), 1);
    assert_eq!(lattice.vias.len(), 1);
    let track = lattice.tracks[0].clone();
    let site = lattice.vias[0].clone();
    let global_route = crate::route_global(&source, &GlobalRouterOptions::default(), None, None, None).expect("guides");
    let placement = PlacementRoutingResult {
        status: crate::routeflow::FeedbackStatus::Pass,
        board: source.clone(),
        global_route,
        placement_candidate: "test".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: false,
    };
    let options = UnifiedRouterOptions { retain_escape_copper: true, bga_fanout: false, ..Default::default() };
    let mut seen = false;
    let mut callback = |stage: Stage<'_>| {
        if let Stage::CriticalInputs { board, fixed_prefix, .. } = stage {
            let prefix = fixed_prefix.expect("actual physical frame reaches replay callback");
            assert_eq!(board, &source);
            assert_eq!(prefix.board().vias, vec![site.clone()]);
            assert_eq!(prefix.board().tracks, vec![track.clone()]);
            assert_eq!(prefix.source().copper_keepouts, source.copper_keepouts);
            seen = true;
        }
    };
    let (critical, reserved, held, held_as_copper, sites_as_copper, retained_prefix) = route_critical_stage(
        &placement,
        &options,
        None,
        None,
        None,
        Some(&lattice),
        &mut None,
        &mut Some(&mut callback),
    )
    .expect("actual flow stage");
    assert!(seen && sites_as_copper);
    assert!(retained_prefix.is_some());
    assert!(critical.board.tracks.starts_with(std::slice::from_ref(&track)));
    assert!(critical.board.vias.starts_with(std::slice::from_ref(&site)));
    let (completed, _) = complete_critical_stage(
        &source,
        critical,
        reserved.as_ref(),
        held.as_ref(),
        held_as_copper,
        sites_as_copper,
        &options,
        None,
        &mut None,
        None,
        Some(&lattice),
        None,
        None,
    )
    .expect("completion");
    assert_eq!(completed.board.tracks.iter().filter(|item| **item == track).count(), 1);
    assert_eq!(completed.board.vias.iter().filter(|item| **item == site).count(), 1);
}

#[test]
fn a_critical_inner_ball_routes_from_its_own_lattice_dogbone() {
    // Without retained escapes the lattice is reserved as keepouts while
    // critical nets route. The clock net's own dogbone stub covers its ball,
    // so it is not reserved (the net could not reach its pad) and not joined
    // afterwards (it would be a via stub on the routed net).
    let mut source = recognized_inner_scene();
    source.net_routing_rules = vec![copper_pcb::board::NetRoutingRule {
        kind: copper_pcb::board::RouteKind::Clock,
        ..crate::test_support_planes::support_planes::rule("N15")
    }];
    let lattice =
        plan_lattice(&source, Some(&EscapePlanOptions { via_lattice: true, ..Default::default() }), None, &mut None)
            .expect("real ball-grid planner")
            .expect("recognized connected inner ball");
    assert_eq!(lattice.packages[0].coverage[0].status, crate::via_lattice::BallSiteStatus::Allocated);
    let (track, site) = (lattice.tracks[0].clone(), lattice.vias[0].clone());
    let global_route = crate::route_global(&source, &GlobalRouterOptions::default(), None, None, None).expect("guides");
    let placement = PlacementRoutingResult {
        status: crate::routeflow::FeedbackStatus::Pass,
        board: source.clone(),
        global_route,
        placement_candidate: "test".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: false,
    };
    let options = UnifiedRouterOptions { bga_fanout: false, ..Default::default() };
    let mut seen = false;
    let mut callback = |stage: Stage<'_>| {
        if let Stage::CriticalInputs { board, fixed_prefix, .. } = stage {
            assert!(fixed_prefix.is_none());
            assert!(
                !board.copper_keepouts.iter().any(|keepout| keepout.id.starts_with("lattice-reserve")),
                "{:?}",
                board.copper_keepouts
            );
            seen = true;
        }
    };
    let (critical, reserved, held, held_as_copper, sites_as_copper, _) = route_critical_stage(
        &placement,
        &options,
        None,
        None,
        None,
        Some(&lattice),
        &mut None,
        &mut Some(&mut callback),
    )
    .expect("actual flow stage");
    assert!(seen && !sites_as_copper);
    let routed = critical.nets.iter().find(|item| item.nets.iter().any(|net| net == "N15")).expect("N15 routed");
    assert!(routed.connected, "{:?}", routed.diagnostics);
    let (completed, _) = complete_critical_stage(
        &source,
        critical,
        reserved.as_ref(),
        held.as_ref(),
        held_as_copper,
        sites_as_copper,
        &options,
        None,
        &mut None,
        None,
        Some(&lattice),
        None,
        None,
    )
    .expect("completion");
    assert!(!completed.board.tracks.contains(&track) && !completed.board.vias.contains(&site));
}

/// Two inner balls of one package: N15 a clock net, N16 a general net.
fn clock_and_general_inner_scene() -> (PhysicalBoard, Lattice) {
    let mut source = crate::test_support_planes::support_planes::ball_grid(6, 0.45, &[]);
    source.nets.retain(|net| net.name == "N15" || net.name == "N16");
    source.placements.iter_mut().find(|pose| pose.reference == "J").unwrap().position =
        Point::new(30_000_000, 30_000_000);
    source.net_routing_rules = vec![copper_pcb::board::NetRoutingRule {
        kind: copper_pcb::board::RouteKind::Clock,
        ..crate::test_support_planes::support_planes::rule("N15")
    }];
    let lattice =
        plan_lattice(&source, Some(&EscapePlanOptions { via_lattice: true, ..Default::default() }), None, &mut None)
            .expect("real ball-grid planner")
            .expect("recognized connected inner balls");
    (source, lattice)
}

#[test]
fn the_staged_lattice_drops_critical_dogbones_and_renumbers_the_rest() {
    let (source, lattice) = clock_and_general_inner_scene();
    let net_of =
        |plan: &Lattice| -> Vec<String> { plan.packages[0].sites.iter().map(|site| site.net.clone()).collect() };
    let mut nets = net_of(&lattice);
    nets.sort();
    assert_eq!(nets, ["N15", "N16"], "both inner balls get a site");
    let staged = staged_lattice(&source, &lattice).unwrap();
    assert!(staged.tracks.iter().chain(&lattice.tracks).any(|track| track.net == "N16"));
    assert!(staged.tracks.iter().all(|track| track.net == "N16") && staged.vias.iter().all(|via| via.net == "N16"));
    assert!(staged.keepouts.len() < lattice.keepouts.len());
    assert!(staged.keepouts.iter().all(|keepout| lattice.keepouts.contains(keepout)));
    // The general site still names its own copper in the shorter lists.
    let site = &staged.packages[0].sites[0];
    assert_eq!(site.net, "N16");
    assert!(staged.tracks[site.track_range.clone()].iter().all(|track| track.net == "N16"));
    assert_eq!(staged.vias[site.via_occurrence].net, "N16");
    // The clock ball is reported pending, not allocated.
    let clock = staged.packages[0].coverage.iter().find(|ball| ball.net == "N15").unwrap();
    assert!(
        matches!(&clock.status, crate::via_lattice::BallSiteStatus::Pending(reason) if reason.contains("critical router"))
    );
    assert!(staged.packages[0].unsited.iter().any(|(pad, _)| pad.pad == "15"));
}

#[test]
fn the_direct_pipeline_keeps_every_lattice_dogbone() {
    // No critical stage runs, so the unified router gets the clock net's
    // dogbone too.
    let (source, lattice) = clock_and_general_inner_scene();
    let global_route = crate::route_global(&source, &GlobalRouterOptions::default(), None, None, None).expect("guides");
    let placement = PlacementRoutingResult {
        status: crate::routeflow::FeedbackStatus::Pass,
        board: source.clone(),
        global_route,
        placement_candidate: "test".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: false,
    };
    let options = UnifiedRouterOptions { direct: true, bga_fanout: false, ..Default::default() };
    let (completed, _) = complete_critical_stage(
        &source,
        direct_critical(&placement).unwrap(),
        None,
        None,
        false,
        false,
        &options,
        None,
        &mut None,
        None,
        Some(&lattice),
        None,
        None,
    )
    .expect("completion");
    for track in &lattice.tracks {
        assert!(completed.board.tracks.contains(track), "{track:?}");
    }
    for via in &lattice.vias {
        assert!(completed.board.vias.contains(via), "{via:?}");
    }
}

#[test]
fn retention_scope_non_bga_keeps_legacy_pipeline_geometry_and_reports_not_applicable() {
    let (source, _) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    let global_route = crate::route_global(&source, &GlobalRouterOptions::default(), None, None, None).unwrap();
    let placement = PlacementRoutingResult {
        status: crate::routeflow::FeedbackStatus::Pass,
        board: source.clone(),
        global_route,
        placement_candidate: "no-inner-grid".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: false,
    };
    let run = |requested, direct| {
        run_routing_pipeline(
            source.clone(),
            RoutingPipelineOptions {
                initial_placement: Some(placement.clone()),
                fanout_options: Some(FanoutOptions::default()),
                unified_options: Some(UnifiedRouterOptions {
                    retain_escape_copper: requested,
                    direct,
                    ..Default::default()
                }),
                ..Default::default()
            },
            None,
        )
        .expect("standard no-BGA pipeline")
    };
    for direct in [false, true] {
        let off = run(false, direct);
        let on = run(true, direct);
        assert!(off.retained_prefix.is_none() && on.retained_prefix.is_none());
        assert_eq!(on.board(), off.board(), "the complete physical result must remain byte-equivalent legacy input");
        assert_eq!(on.critical.status, off.critical.status);
        assert_eq!(on.detailed.status, off.detailed.status);
        let policy = on.retained_escape_policy.expect("durable policy report");
        assert_eq!(policy.applicability, "not_applicable");
        assert!(!policy.active());
        let Value::Object(report) = policy.report(false, false) else { panic!("policy object") };
        assert_eq!(report["requested"], Value::Bool(true));
        assert_eq!(report["effective"], Value::Bool(false));
        assert_eq!(report["effective_policy"], text("legacy_mutable_fanout"));
        assert_eq!(report["requested_policy_satisfied"], Value::Bool(true));
    }
}

#[test]
fn retention_scope_counts_real_pending_and_source_launch_obligations_without_claiming_missing_contacts() {
    let mut source = recognized_inner_scene();
    let settings = EscapePlanOptions { via_lattice: true, ..Default::default() };
    let allocated = plan_lattice(&source, Some(&settings), None, &mut None).unwrap().unwrap();
    let policy = retained_escape_policy(&source, true, Some(&allocated)).unwrap();
    assert!(policy.active());
    assert_eq!((policy.mandatory_inner_obligations, policy.allocated_inner_sites), (1, 1));
    source.tracks.extend(allocated.tracks);
    source.vias.extend(allocated.vias);
    crate::via_lattice::coverage_tests::bind_materialized_launch(&mut source);
    let existing = plan_lattice(&source, Some(&settings), None, &mut None).unwrap().unwrap();
    let policy = retained_escape_policy(&source, true, Some(&existing)).unwrap();
    assert!(policy.active());
    assert_eq!((policy.existing_inner_launches, policy.allocated_inner_sites), (1, 0));
    assert!(policy.pending_inner.is_empty());
    assert!(policy.requested_policy_satisfied(true));
    assert!(!policy.requested_policy_satisfied(false), "allocated contacts alone do not prove retention");
    let Value::Object(lost) = policy.report(true, false) else { panic!("policy object") };
    assert_eq!(lost["retained_prefix_admitted"], Value::Bool(true));
    assert_eq!(lost["retained_prefix_occurrences_preserved"], Value::Bool(false));
    assert_eq!(lost["requested_policy_satisfied"], Value::Bool(false));
    let mut pending_board = routers::pad_board(&[("S", &[(2.0, 2.0), (18.0, 2.0)])], 2, Vec::new());
    let pending = add_pending_inner_grid(&mut pending_board);
    let policy = retained_escape_policy(&pending_board, true, Some(&pending)).unwrap();
    assert!(policy.active(), "failed allocation is still an applicable mandatory obligation");
    assert_eq!((policy.mandatory_inner_obligations, policy.censused_obligations), (1, 1));
    assert_eq!(policy.pending_inner[0].0, PadReference::new("U_INNER", "15"));
    let Value::Object(report) = policy.report(true, true) else { panic!("policy object") };
    assert_eq!(report["retained_prefix_admitted"], Value::Bool(true));
    assert_eq!(report["mandatory_inner_local_coverage_complete"], Value::Bool(false));
    assert_eq!(report["requested_policy_satisfied"], Value::Bool(false));
    assert_eq!(report["pending_inner_obligations"], int(1));
    let missing = retained_escape_policy(&pending_board, true, None).unwrap();
    assert_eq!(missing.applicability, "unsupported_or_uncensused");
    assert_eq!(missing.censused_obligations, 0);
}

#[test]
fn retention_scope_distinguishes_outer_only_unsupported_and_mixed_fields() {
    use copper_physical::footprint::PadShape;
    let settings = EscapePlanOptions { via_lattice: true, ..Default::default() };
    let mut outer = recognized_inner_scene();
    outer.nets[0].pads[0] = PadReference::new("U", "1");
    let lattice = plan_lattice(&outer, Some(&settings), None, &mut None).unwrap().unwrap();
    let policy = retained_escape_policy(&outer, true, Some(&lattice)).unwrap();
    assert_eq!(policy.applicability, "not_applicable");
    assert_eq!(policy.mandatory_inner_obligations, 0);
    for shape in [PadShape::Rectangle, PadShape::Roundrect] {
        let mut unsupported = recognized_inner_scene();
        for pad in &mut unsupported.footprints.get_mut("bga").unwrap().pads {
            pad.shape = shape;
        }
        let policy = retained_escape_policy(&unsupported, true, None).unwrap();
        assert_eq!(policy.applicability, "unsupported_or_uncensused");
        assert_eq!(policy.unsupported_pattern_candidates, 1);
    }
    let mut jittered = recognized_inner_scene();
    for pad in &mut jittered.footprints.get_mut("bga").unwrap().pads {
        pad.position.x_nm += (pad.position.y_nm / 800_000) * 5_000;
    }
    let policy = retained_escape_policy(&jittered, true, None).unwrap();
    assert_eq!(policy.applicability, "unsupported_or_uncensused", "row offsets cannot mean no inner field");
    assert_eq!(policy.unsupported_pattern_candidates, 1);
    let mut noncommon = recognized_inner_scene();
    let land = noncommon.footprints.get_mut("bga").unwrap().pads.iter_mut().find(|pad| pad.number == "15").unwrap();
    land.size.width_nm = 460_000;
    land.size.height_nm = 460_000;
    let policy = retained_escape_policy(&noncommon, true, None).unwrap();
    assert_eq!(policy.applicability, "unsupported_or_uncensused", "an excluded inner land cannot disappear");
    assert_eq!(policy.mandatory_inner_obligations, 1);
    assert_eq!(
        policy.pending_inner,
        vec![(PadReference::new("U", "15"), "unsupported_inner_land_geometry".to_owned())]
    );
    let mut subgroup = crate::test_support_planes::support_planes::ball_grid(5, 0.45, &[]);
    subgroup.nets.retain(|net| net.name == "N13");
    let mut other = 0;
    for pad in &mut subgroup.footprints.get_mut("bga").unwrap().pads {
        let x = pad.position.x_nm / 800_000;
        let y = pad.position.y_nm / 800_000;
        let size = if x % 2 == 0 && y % 2 == 0 {
            450_000
        } else {
            other += 1;
            if other <= 8 { 460_000 } else { 470_000 }
        };
        pad.size.width_nm = size;
        pad.size.height_nm = size;
    }
    let detected = crate::package_patterns::ball_grid(&subgroup, "U").unwrap().unwrap();
    assert_eq!(detected.xs_nm.len(), 3, "regression needs the misleading dominant-size subgrid");
    let policy = retained_escape_policy(&subgroup, true, None).unwrap();
    assert_eq!(policy.applicability, "unsupported_or_uncensused");
    assert_eq!(policy.pending_inner, vec![(PadReference::new("U", "13"), "uncensused_inner_surface_field".to_owned())]);
    assert!(!policy.requested_policy_satisfied(true));
    let mut mixed = recognized_inner_scene();
    let allocated = plan_lattice(&mixed, Some(&settings), None, &mut None).unwrap().unwrap();
    mixed.tracks.extend(allocated.tracks);
    mixed.vias.extend(allocated.vias);
    crate::via_lattice::coverage_tests::bind_materialized_launch(&mut mixed);
    add_pending_inner_grid(&mut mixed);
    for pad in &mut mixed.footprints.get_mut("test/mandatory-inner-grid").unwrap().pads {
        pad.shape = PadShape::Rectangle;
    }
    let lattice = plan_lattice(&mixed, Some(&settings), None, &mut None).unwrap().unwrap();
    let policy = retained_escape_policy(&mixed, true, Some(&lattice)).unwrap();
    assert!(policy.active(), "the recognized field still retains its mandatory sites");
    assert_eq!(policy.unsupported_pattern_candidates, 1);
    assert!(policy.pending_inner.is_empty());
    let Value::Object(report) = policy.report(true, true) else { panic!("policy object") };
    assert_eq!(report["mandatory_inner_local_coverage_complete"], Value::Bool(false));
    assert_eq!(report["requested_policy_satisfied"], Value::Bool(false));
}

#[test]
fn retained_frame_checks_occurrences_and_rejects_a_real_cross_net_collision() {
    let source = routers::pad_board(&[("S", &[]), ("T", &[])], 2, Vec::new());
    let track = routers::track("S", (5.0, 5.0), (10.0, 5.0), 0.2, CopperLayer::Front);
    let reserved = (vec![track.clone(), track.clone()], Vec::new(), Vec::new());
    let (prefix, _) = retained_critical_inputs(&source, Some(&reserved), None, None).expect("two owned occurrences");
    let mut damaged = prefix.board().clone();
    damaged.tracks.pop();
    assert!(
        require_retained_prefix(&damaged, &prefix).is_err(),
        "one contained copy cannot replace two owned occurrences"
    );
    let lattice = Lattice {
        tracks: vec![routers::track("T", (7.0, 4.0), (7.0, 6.0), 0.2, CopperLayer::Front)],
        vias: Vec::new(),
        keepouts: Vec::new(),
        packages: Vec::new(),
    };
    assert!(retained_critical_inputs(&source, Some(&reserved), Some(&lattice), None).is_err());
}

#[test]
fn raw_held_provenance_keeps_lattice_and_producer_halo_when_legacy_refresh_loses_the_path() {
    let (mut source, _) = crate::test_support_planes::support_planes::decoupling::existing_launches(false);
    let lattice = Lattice {
        tracks: std::mem::take(&mut source.tracks),
        vias: std::mem::take(&mut source.vias),
        keepouts: Vec::new(),
        packages: Vec::new(),
    };
    source.nets.push(PhysicalNet { name: "ORDINARY".to_owned(), pads: Vec::new() });
    let ordinary_track = routers::track("ORDINARY", (2.0, 2.0), (3.0, 2.0), 0.15, CopperLayer::Front);
    let ordinary_via = routers::via("ORDINARY", (3.0, 2.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back);
    let reserved = (
        vec![ordinary_track.clone()],
        vec![ordinary_via.clone()],
        copper_keepouts(&source, &[ordinary_track], &[ordinary_via], "escape-reserve", Some(&source)),
    );
    let halo =
        routers::copper_keepout("declared-producer-halo", &[CopperLayer::Front], routers::rect(1.0, 15.0, 2.0, 16.0));
    let (held, reserved) = plan_decouplers_with_provenance(
        &source,
        Some(reserved),
        &mut None,
        (&lattice.tracks, &lattice.vias),
        std::slice::from_ref(&halo),
        None,
        true,
    )
    .expect("real initial producer on retained lattice");
    let reserved = reserved.expect("ordinary reservations");
    assert!(!held.witnesses.is_empty() && !held.tracks.is_empty());
    let captured = held.provenance.as_ref().expect("raw producer provenance");
    assert!(captured.rejected.is_none(), "{:?}", captured.rejected);
    assert_eq!(captured.batches.len(), 1);
    assert!(captured.batches[0].temporary_keepouts.contains(&halo));
    assert!(captured.batches[0].raw.tracks.starts_with(&lattice.tracks));
    assert!(
        held.witnesses.iter().flat_map(|w| &w.supply_path.steps).any(|step| {
            matches!(&step.edge, crate::decoupling_paths::PathEdge::Track(track) if lattice.tracks.contains(track))
        }),
        "the actual original supply proof must depend on retained lattice copper"
    );
    assert!(held.tracks.iter().all(|track| !lattice.tracks.contains(track)));
    assert!(held.vias.iter().all(|via| !lattice.vias.contains(via)));
    let original_provenance = format!("{:?}", held.provenance);
    let (held, reason) = refresh_bypass_witnesses(&source, Some(&reserved), held).expect("legacy refresh");
    assert_eq!(reason.as_deref(), Some("supply_path_absent"), "legacy context intentionally lacks the lattice");
    assert_eq!(format!("{:?}", held.provenance), original_provenance);
    let (prefix, bound) = retained_critical_inputs(&source, Some(&reserved), Some(&lattice), Some(&held))
        .expect("original RAW proof permits fresh final binding despite failed legacy refresh");
    let bound = bound.expect("exact Held contribution");
    assert_eq!(bound.created_tracks, held.tracks);
    assert_eq!(bound.created_vias, held.vias);
    for (original, current) in captured_witnesses(&held).iter().zip(&bound.decoupling_witnesses) {
        assert_eq!(current.link, original.link);
        assert_eq!(current.return_target, original.return_target);
        assert_ne!(current.context_digest, original.context_digest);
    }
    assert_eq!(format!("{:?}", held.provenance), original_provenance, "fresh binding never rewrites provenance");
    assert!(prefix.board().tracks.starts_with(&lattice.tracks));
    assert!(prefix.board().vias.starts_with(&lattice.vias));

    let mut changed_source = source.clone();
    changed_source.metadata.insert("changed-source".to_owned(), "true".to_owned());
    assert!(
        retained_critical_inputs(&changed_source, Some(&reserved), Some(&lattice), Some(&held))
            .expect_err("source changes invalidate provenance")
            .to_string()
            .contains("exact clean source changed")
    );
    let mut missing_mask = held.clone();
    let batch = &mut missing_mask.provenance.as_mut().expect("provenance").batches[0];
    let mask_index = batch.raw.copper_keepouts.iter().rposition(|mask| *mask == halo).expect("raw halo");
    batch.raw.copper_keepouts.remove(mask_index);
    assert!(
        retained_critical_inputs(&source, Some(&reserved), Some(&lattice), Some(&missing_mask))
            .expect_err("missing declared mask occurrence cannot be reconstructed")
            .to_string()
            .contains("missing authored or declared temporary")
    );
    let mut stale = held.clone();
    stale.provenance.as_mut().expect("provenance").batches[0].witnesses[0].context_digest = "stale".to_owned();
    assert!(
        retained_critical_inputs(&source, Some(&reserved), Some(&lattice), Some(&stale))
            .expect_err("RAW witness cannot be replaced by a fresh invented association")
            .to_string()
            .contains("stale_witness_context")
    );
    let mut extra_owned = held.clone();
    extra_owned.vias.push(lattice.vias[0].clone());
    assert!(
        retained_critical_inputs(&source, Some(&reserved), Some(&lattice), Some(&extra_owned))
            .expect_err("a lattice occurrence is not generated Held copper")
            .to_string()
            .contains("exact producer-generated occurrences")
    );
}

fn captured_witnesses(held: &Held) -> Vec<crate::decoupling_paths::DecouplingCrossLayerWitness> {
    held.provenance.as_ref().expect("provenance").batches.iter().flat_map(|batch| batch.witnesses.clone()).collect()
}

#[test]
fn optional_retirement_rederives_paths_and_rejects_lost_or_unresolved_dependencies() {
    use crate::decoupling_paths::{
        renew_retirement_dependencies, retirement_dependencies, validate_crosslayer_witness,
    };
    let (source, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    let routed = crate::fanout::route_fanout(&source, None, None).unwrap();
    let mut before = routed.board;
    before.tracks.push(routers::track(&link.net, (20.0, 15.0), (21.0, 15.0), 0.15, CopperLayer::Front));
    let snapshot = retirement_dependencies(&before).unwrap();
    assert_eq!(snapshot.witnesses.len(), 1);
    let mut after = before.clone();
    after.tracks.pop();
    let changed = [link.net.clone()].into_iter().collect();
    let (fresh, rejected) = renew_retirement_dependencies(&before, &after, &snapshot, &changed).unwrap();
    assert!(rejected.is_empty());
    let fresh = fresh.unwrap();
    assert_ne!(snapshot.board_digest, fresh.board_digest);
    assert_eq!(fresh.witnesses[0].return_target, snapshot.witnesses[0].return_target);
    assert_eq!(validate_crosslayer_witness(&after, &fresh.witnesses[0], None).unwrap(), "");
    let edge = snapshot.witnesses[0].supply_path.tracks()[0].clone();
    after.tracks.retain(|t| *t != edge);
    let (failed, rejected) = renew_retirement_dependencies(&before, &after, &snapshot, &changed).unwrap();
    assert!(failed.is_none() && rejected.contains(&link.net));
    assert!(before.tracks.contains(&edge), "failed trials do not mutate their incumbent");
    let unknown = retirement_dependencies(&source).unwrap();
    assert_eq!(unknown.unresolved.len(), 1);
    assert!(renew_retirement_dependencies(&source, &source, &unknown, &changed).unwrap().0.is_none());
    let mut changed_source = before.clone();
    changed_source.metadata.insert("stale".into(), "yes".into());
    assert!(renew_retirement_dependencies(&changed_source, &after, &snapshot, &changed).is_err());
}

#[test]
fn optional_retirement_renews_surface_feeds_without_promoting_return_qualification() {
    use crate::decoupling_paths::{renew_retirement_dependencies, retirement_dependencies};
    let (mut before, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    before.placements.iter_mut().find(|p| p.reference == link.capacitor.component).unwrap().side =
        copper_physical::BoardSide::Front;
    before.tracks.extend([
        routers::track(&link.net, (9.0, 8.0), (12.5, 8.0), 0.15, CopperLayer::Front),
        routers::track(&link.net, (12.5, 8.0), (12.5, 9.0), 0.15, CopperLayer::Front),
        routers::track(&link.return_net, (13.5, 9.0), (13.5, 10.0), 0.15, CopperLayer::Front),
        routers::track(&link.return_net, (13.5, 10.0), (9.0, 10.0), 0.15, CopperLayer::Front),
        routers::track(&link.net, (20.0, 15.0), (21.0, 15.0), 0.15, CopperLayer::Front),
    ]);
    let snapshot = retirement_dependencies(&before).unwrap();
    assert!(snapshot.witnesses.is_empty());
    assert!(snapshot.unresolved.is_empty());
    assert_eq!(snapshot.surface_feeds.len(), 1);
    let changed = [link.net.clone(), link.return_net.clone()].into_iter().collect();
    let mut after = before.clone();
    after.tracks.pop();
    let renewed = renew_retirement_dependencies(&before, &after, &snapshot, &changed).unwrap().0.unwrap();
    assert!(renewed.witnesses.is_empty(), "feed verification is not supply/return qualification");
    assert_eq!(renewed.surface_feeds.len(), 1);
    let mut changed_return = after.clone();
    changed_return.tracks.pop();
    assert!(renew_retirement_dependencies(&before, &changed_return, &snapshot, &changed).unwrap().0.is_none());
    after.tracks.drain(..2);
    after.tracks.extend([
        routers::track(&link.net, (9.0, 8.0), (9.0, 7.0), 0.15, CopperLayer::Front),
        routers::track(&link.net, (9.0, 7.0), (12.5, 7.0), 0.15, CopperLayer::Front),
        routers::track(&link.net, (12.5, 7.0), (12.5, 9.0), 0.15, CopperLayer::Front),
    ]);
    assert!(
        renew_retirement_dependencies(&before, &after, &snapshot, &changed).unwrap().0.is_none(),
        "a longer feed must reject"
    );
}

#[test]
fn removing_producer_masks_keeps_original_owned_path_when_fresh_search_selects_a_shortcut() {
    let (source, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    let prefix = crate::fixed_prefix::AdmittedFixedPrefix::admit(&source, &[]).expect("clean source");
    let mut producer = prefix.board().clone();
    let halo = routers::copper_keepout("producer-only", &[CopperLayer::Front], routers::rect(1.0, 15.0, 2.0, 16.0));
    producer.copper_keepouts.push(halo.clone());
    producer.tracks.extend([
        routers::track("P", (9.0, 8.0), (11.0, 8.0), 0.15, CopperLayer::Front),
        routers::track("P", (11.0, 8.0), (11.0, 7.0), 0.15, CopperLayer::Back),
        routers::track("P", (11.0, 7.0), (13.5, 7.0), 0.15, CopperLayer::Back),
        routers::track("P", (13.5, 7.0), (13.5, 9.0), 0.15, CopperLayer::Back),
        routers::track("R", (9.0, 10.0), (11.0, 10.0), 0.15, CopperLayer::Front),
        routers::track("R", (11.0, 10.0), (12.5, 10.0), 0.15, CopperLayer::Back),
        routers::track("R", (12.5, 10.0), (12.5, 9.0), 0.15, CopperLayer::Back),
    ]);
    producer.vias.extend([
        routers::via("P", (11.0, 8.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back),
        routers::via("R", (11.0, 10.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back),
    ]);
    let original = crate::decoupling_paths::verify_crosslayer_decoupling(&producer, &link, None, 10_000, None)
        .expect("original producer measurement")
        .witness
        .expect("complete original supply and return");
    let mut current = producer.clone();
    current.copper_keepouts.pop();
    current.tracks.push(routers::track("P", (11.0, 8.0), (13.5, 8.0), 0.15, CopperLayer::Back));
    let selected = crate::decoupling_paths::verify_crosslayer_decoupling(&current, &link, None, 10_000, None)
        .expect("fresh search")
        .witness
        .expect("shortcut supply and original return");
    assert_ne!(selected.supply_path, original.supply_path, "the regression needs a different fresh path");
    let (_, reason) = crate::bypass_reservations::bypass_copper_reservations_in_frame(
        &source,
        &prefix,
        &current.tracks,
        &current.vias,
        std::slice::from_ref(&selected),
    )
    .expect("strict original binder");
    let reason = reason.expect("a replacement path must not silently orphan earlier owned copper");
    assert!(reason.starts_with("declared generated bypass copper is absent from a fresh source witness:"));
    assert!(reason.contains("unwitnessed_tracks=2 unwitnessed_vias=0"), "{reason}");
    assert!(reason.contains("net=P layer=B.Cu start_nm=(11000000,8000000) end_nm=(11000000,7000000)"), "{reason}");
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&current).expect("target clearance");
    let rebound = crate::decoupling_paths::revalidate_after_temporary_keepout_removal(
        prefix.baseline(),
        &producer,
        &current,
        &original,
        std::slice::from_ref(&halo),
        &index,
    )
    .expect("fresh exact original path validation");
    assert_eq!(rebound.supply_path, original.supply_path);
    assert_eq!(rebound.return_path, original.return_path);
    assert_ne!(rebound.context_digest, original.context_digest);
    let (bound, reason) = crate::bypass_reservations::bypass_copper_reservations_in_frame(
        &source,
        &prefix,
        &current.tracks,
        &current.vias,
        &[rebound],
    )
    .expect("strict binder with preserved original ownership");
    assert!(bound.is_some() && reason.is_none(), "{reason:?}");

    // Two exact producer batches retain their original paths even when the
    // second batch makes the first path unnecessary for a fresh search.
    let mut later_producer = current.clone();
    later_producer.copper_keepouts.push(halo.clone());
    let later_witness =
        crate::decoupling_paths::verify_crosslayer_decoupling(&later_producer, &link, None, 10_000, None)
            .expect("later original producer proof")
            .witness
            .expect("later complete path");
    let held = Held {
        tracks: current.tracks.clone(),
        vias: current.vias.clone(),
        keepouts: Vec::new(),
        released: Vec::new(),
        witnesses: vec![original.clone(), later_witness.clone()],
        provenance: Some(HeldProvenance {
            source: source.clone(),
            complete_prefix: Some(prefix.clone()),
            prefix_tracks: Vec::new(),
            prefix_vias: Vec::new(),
            rejected: None,
            batches: vec![
                HeldProducerBatch {
                    raw: producer.clone(),
                    temporary_keepouts: vec![halo.clone()],
                    tracks: 0..producer.tracks.len(),
                    vias: 0..producer.vias.len(),
                    witnesses: vec![original.clone()],
                },
                HeldProducerBatch {
                    raw: later_producer,
                    temporary_keepouts: vec![halo.clone()],
                    tracks: producer.tracks.len()..current.tracks.len(),
                    vias: producer.vias.len()..current.vias.len(),
                    witnesses: vec![later_witness],
                },
            ],
        }),
    };
    let (_, bound) = retained_critical_inputs(&source, None, None, Some(&held))
        .expect("actual flow must retain both original producer paths");
    let bound = bound.expect("the exact complete Held contribution is admitted");
    assert_eq!(bound.created_tracks, held.tracks);
    assert_eq!(bound.decoupling_witnesses[0].supply_path, original.supply_path);

    let mut missing_edge = current.clone();
    missing_edge.tracks.remove(1);
    assert!(
        crate::decoupling_paths::verify_crosslayer_decoupling(&missing_edge, &link, None, 10_000, None)
            .expect("shortcut still closes this association")
            .witness
            .is_some()
    );
    let missing_index =
        crate::routing_clearance::RoutingClearanceIndex::new(&missing_edge).expect("missing-edge index");
    assert!(
        crate::decoupling_paths::revalidate_after_temporary_keepout_removal(
            prefix.baseline(),
            &producer,
            &missing_edge,
            &original,
            std::slice::from_ref(&halo),
            &missing_index,
        )
        .expect_err("a replacement path cannot excuse a missing original owned occurrence")
        .to_string()
        .contains("witness_track_invalid")
    );
    let mut stale = original.clone();
    stale.context_digest = "forged".to_owned();
    assert!(
        crate::decoupling_paths::revalidate_after_temporary_keepout_removal(
            prefix.baseline(),
            &producer,
            &current,
            &stale,
            std::slice::from_ref(&halo),
            &index,
        )
        .expect_err("old proof must validate before context adaptation")
        .to_string()
        .contains("stale_witness_context")
    );
    let mut changed = current.clone();
    changed.rules.minimum_clearance_nm += 1;
    let changed_index = crate::routing_clearance::RoutingClearanceIndex::new(&changed).expect("changed index");
    assert!(
        crate::decoupling_paths::revalidate_after_temporary_keepout_removal(
            prefix.baseline(),
            &producer,
            &changed,
            &original,
            &[halo],
            &changed_index,
        )
        .expect_err("mask removal cannot admit unrelated source changes")
        .to_string()
        .contains("non-copper source intent")
    );
}

#[derive(Default)]
struct BypassEvents(Vec<(&'static str, IndexMap<String, Value>)>);

impl crate::progress::Progress for BypassEvents {
    fn observe(&mut self, phase: &str, event: &str, fields: Vec<(&'static str, Value)>) {
        if phase == "bypass_reservation" {
            let event = if event == "finished" { "finished" } else { "other" };
            self.0.push((event, fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()));
        }
    }
}

#[test]
fn physical_prefix_before_decouplers_preserves_an_unmasked_ordinary_stem_and_exact_ownership() {
    let (mut source, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    source.nets.push(PhysicalNet { name: "S".to_owned(), pads: Vec::new() });
    source.nets.push(PhysicalNet { name: "LATTICE".to_owned(), pads: Vec::new() });
    let lattice_via = routers::via("LATTICE", (2.0, 17.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back);
    let lattice =
        Lattice { tracks: Vec::new(), vias: vec![lattice_via.clone()], keepouts: Vec::new(), packages: Vec::new() };
    let mut baseline = source.clone();
    baseline.vias.push(lattice_via.clone());
    let unreserved = crate::decoupling::route_decouplers(&baseline, None, None, None, None).expect("unreserved bypass");
    assert!(unreserved.pending.is_empty() && !unreserved.admitted_path_witnesses.is_empty());
    let taken_site = unreserved.created_vias.first().expect("actual unreserved bypass via");
    // A short real copper stem fits inside this otherwise legal via's land.
    // An absent proxy mask must not make its physical copper disappear.
    let stem = TrackSegment {
        net: "S".to_owned(),
        layer: CopperLayer::Front,
        width_nm: 150_000,
        start: Point::new(taken_site.position.x_nm, taken_site.position.y_nm - 100_000),
        end: Point::new(taken_site.position.x_nm, taken_site.position.y_nm + 100_000),
    };
    let clear = crate::routing_clearance::RoutingClearanceIndex::new(&baseline).expect("baseline index");
    assert!(clear.can_track(&stem.net, stem.start, stem.end, stem.width_nm, stem.layer).expect("legal ordinary stem"));
    let reserved = (vec![stem.clone(), stem.clone()], Vec::new(), Vec::new());
    let original_reserved = format!("{reserved:?}");
    let (legacy, _) = plan_decouplers(&source, Some(reserved.clone()), &mut None, (&[], &lattice.vias), &[], None)
        .expect("legacy proxy planner");
    assert!(
        legacy.vias.iter().any(|via| via.position == taken_site.position),
        "legacy must reproduce the measured site choice"
    );
    let mut legacy_frame = baseline.clone();
    legacy_frame.tracks.extend(legacy.tracks);
    legacy_frame.vias.extend(legacy.vias);
    let legacy_index = crate::routing_clearance::RoutingClearanceIndex::new(&legacy_frame).expect("legacy index");
    assert!(
        !legacy_index
            .can_track(&stem.net, stem.start, stem.end, stem.width_nm, stem.layer)
            .expect("actual via/ordinary conflict")
    );

    let mut events = BypassEvents::default();
    let (held, kept) =
        plan_retained_decouplers(&source, Some(reserved.clone()), Some(&lattice), &[], None, &mut Some(&mut events))
            .expect("physical-prefix bypass planner");
    let kept = kept.expect("the complete ordinary contribution stays retained");
    assert_eq!(format!("{kept:?}"), original_reserved);
    assert!(held.released.is_empty());
    let provenance = held.provenance.as_ref().expect("complete raw provenance");
    assert!(provenance.rejected.is_none());
    let original_prefix = provenance.complete_prefix.as_ref().expect("explicit complete physical prefix");
    assert_eq!(original_prefix.contributions().len(), 2);
    assert!(provenance.batches[0].raw.tracks.starts_with(&original_prefix.board().tracks));
    assert!(provenance.batches[0].raw.vias.starts_with(&original_prefix.board().vias));
    assert!(held.tracks.iter().all(|track| *track != stem), "ordinary copper cannot be relabeled Held");
    assert!(held.vias.iter().all(|via| *via != lattice_via), "lattice copper cannot be relabeled Held");
    let raw = &provenance.batches[0].raw;
    let raw_index = crate::routing_clearance::RoutingClearanceIndex::new(raw).expect("complete result index");
    assert!(
        raw_index
            .can_track(&stem.net, stem.start, stem.end, stem.width_nm, stem.layer)
            .expect("stem stays legal beside every bypass candidate")
    );
    let finished = events.0.iter().find(|(event, _)| *event == "finished").expect("normal pending report");
    assert_eq!(finished.1.get("flow_displacement_retries"), Some(&int(0)));
    assert_eq!(finished.1.get("released"), Some(&int(0)));
    let nets = IndexSet::from([link.net.clone()]);
    let verified =
        crate::bypass_links::verified_bypass_links(raw, &nets, None, None, &[]).expect("fresh association status");
    if !verified.contains(&link) {
        let pending = finished.1.get("pending_links").and_then(Value::as_array).expect("reported pending links");
        assert!(!pending.is_empty(), "a blocked association stays explicitly pending");
    }

    let (prefix, bound) = retained_critical_inputs(&source, Some(&kept), Some(&lattice), Some(&held))
        .expect("exact same physical prefix reaches critical binding");
    let mut rejected = Some(held.clone());
    rejected.as_mut().expect("Held proposal").tracks.push(stem.clone());
    let preserved = retained_critical_binding(&source, Some(&kept), Some(&lattice), &mut rejected, &mut None)
        .expect("failed Held ownership is a local rejected proposal")
        .expect("physical prefix remains admitted");
    assert_eq!(preserved.0.board(), prefix.board());
    assert!(preserved.1.is_none() && rejected.is_none(), "no Held suffix remains for a late join to resurrect");
    let mut changed_ordinary = kept.clone();
    changed_ordinary.0.pop();
    if !held.tracks.is_empty() || !held.vias.is_empty() {
        assert!(
            retained_critical_inputs(&source, Some(&changed_ordinary), Some(&lattice), Some(&held))
                .expect_err("the successful producer's complete prefix is exact")
                .to_string()
                .contains("complete physical prefix changed")
        );
    }
    let global = crate::route_global(&source, &GlobalRouterOptions::default(), None, None, None).expect("guides");
    let critical = crate::critical::route_critical_nets(
        source.clone(),
        &global,
        &crate::critical::CriticalOptions {
            fixed_prefix: Some(&prefix),
            reserved_accesses: bound.as_ref(),
            ..Default::default()
        },
        None,
    )
    .expect("critical on the unchanged prefix");
    let options = UnifiedRouterOptions { retain_escape_copper: true, bga_fanout: false, ..Default::default() };
    let (mut completed, mut stitched) = complete_critical_stage(
        &source,
        critical,
        Some(&kept),
        Some(&held),
        bound.is_some(),
        true,
        &options,
        None,
        &mut None,
        None,
        Some(&lattice),
        None,
        None,
    )
    .expect("completion leaves physical prefix immutable");
    require_retained_prefix(&completed.board, &prefix).expect("all ordered original occurrences remain");
    let before_release = completed.board.clone();
    let final_board =
        release_escape_masks(&source, Some(&kept), &mut completed, &mut stitched, before_release, &options, &mut None)
            .expect("final release does not rejoin the same prefix");
    assert_eq!(final_board.tracks.iter().filter(|track| **track == stem).count(), 2);
    assert_eq!(final_board.vias.iter().filter(|via| **via == lattice_via).count(), 1);
    let final_index = crate::routing_clearance::RoutingClearanceIndex::new(&final_board).expect("final index");
    assert!(
        final_index
            .can_track(&stem.net, stem.start, stem.end, stem.width_nm, stem.layer)
            .expect("final retained stem legal")
    );
    assert!(source.tracks.is_empty() && source.vias.is_empty());

    let mut bad_lattice = Lattice {
        tracks: lattice.tracks.clone(),
        vias: lattice.vias.clone(),
        keepouts: lattice.keepouts.clone(),
        packages: Vec::new(),
    };
    bad_lattice.vias[0].position = taken_site.position;
    let mut failed_events = BypassEvents::default();
    let (unsupported, unchanged) = plan_retained_decouplers(
        &source,
        Some(kept.clone()),
        Some(&bad_lattice),
        &[],
        None,
        &mut Some(&mut failed_events),
    )
    .expect("unsupported initial prefix stays a reported partial result");
    assert!(unsupported.tracks.is_empty() && unsupported.vias.is_empty() && unsupported.provenance.is_none());
    assert_eq!(format!("{:?}", unchanged.as_ref().expect("unchanged reservations")), original_reserved);
    let failed = failed_events.0.iter().find(|(event, _)| *event == "finished").expect("prefix failure report");
    assert_eq!(failed.1.get("mode"), Some(&text("complete_physical_prefix_not_admitted")));
    assert!(!failed.1.get("pending_links").and_then(Value::as_array).expect("pending links").is_empty());
    let mut unsupported = Some(unsupported);
    assert!(
        retained_critical_binding(&source, unchanged.as_ref(), Some(&bad_lattice), &mut unsupported, &mut None)
            .expect("no global routing gate")
            .is_none(),
        "an unsupported initial frame has no admitted-prefix claim"
    );
}

#[test]
fn accepted_decoupler_swap_replans_escapes_beside_the_original_lattice() {
    let (mut board, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    let footprint = routers::footprint(
        "test/swap-three-pad",
        vec![
            routers::pad("1", 0.0, 0.0, 0.25, 0.25),
            routers::pad("2", 0.75, 0.0, 0.25, 0.25),
            routers::pad("3", 0.0, 0.75, 0.25, 0.25),
        ],
        1.5,
        1.5,
    );
    for (reference, x, y) in [("S0", 3.0, 3.0), ("S1", 20.0, 3.0)] {
        board.placements.push(routers::placement(reference, &footprint.name, x, y));
    }
    board.footprints.insert(footprint.name.clone(), footprint);
    board.nets.push(PhysicalNet {
        name: "S".to_owned(),
        pads: vec![PadReference::new("S0", "1"), PadReference::new("S1", "1")],
    });
    board.nets.push(PhysicalNet { name: "LATTICE".to_owned(), pads: Vec::new() });
    let options = FanoutOptions {
        minimum_component_pads: 3,
        joint_escapes: false,
        two_leg_escapes: false,
        surface_fallback: false,
        ..FanoutOptions::default()
    };
    // This is the same real bypass-blocking ring used by the existing release
    // regression. A successful bypass must displace S's entire reserved group
    // and, with fanout enabled, find replacement escapes before adoption.
    let ring = [(9.0, 8.7), (9.0, 7.3), (9.7, 8.0), (8.3, 8.0), (9.5, 8.5), (8.5, 8.5), (9.5, 7.5), (8.5, 7.5)];
    let vias: Vec<Via> =
        ring.iter().map(|at| routers::via("S", *at, 0.45, 0.2, CopperLayer::Front, CopperLayer::Back)).collect();
    let masks = copper_keepouts(&board, &[], &vias, "escape-reserve", Some(&board));
    assert_eq!(masks.len(), vias.len());
    let original = (Vec::new(), vias, masks);
    let mut blocked = board.clone();
    blocked.copper_keepouts.extend(original.2.iter().cloned());
    let first = crate::decoupling::route_decouplers(&blocked, None, Some(&options), None, None).expect("first pass");
    assert!(first.pending.iter().any(|(item, _)| *item == link), "the initial reservations must block the link");

    let (first_held, first_reserved) =
        plan_decouplers(&board, Some(original.clone()), &mut None, (&[], &[]), &[], Some(&options))
            .expect("unobstructed swap");
    let first_reserved = first_reserved.expect("kept escape ledger");
    assert!(!first_held.witnesses.is_empty(), "a real bypass must be accepted");
    assert!(first_held.released.is_empty(), "with fanout the taken group must be replaced, not released");
    assert!(first_reserved.1.iter().all(|via| !original.1.contains(via)), "the taken ring must actually be replaced");
    let replacement = first_reserved.1.iter().find(|via| via.net == "S").expect("real replacement escape via");

    // Use the measured first replacement site as a fixed foreign lattice site.
    // This avoids assuming a particular ray ordering. The old incomplete swap
    // frame repeats this site because added_elements(retry, base) omits lattice.
    let mut lattice = replacement.clone();
    lattice.net = "LATTICE".to_owned();
    let source_index = crate::routing_clearance::RoutingClearanceIndex::new(&board).expect("source index");
    assert!(
        source_index
            .can_via(
                &lattice.net,
                lattice.position,
                lattice.size_nm,
                lattice.from_layer,
                lattice.to_layer,
                Some(lattice.drill_nm),
                false,
                None,
            )
            .expect("legal lattice"),
        "the fixed lattice site must be independently legal before planning"
    );
    let (held, reserved) = plan_decouplers_with_provenance(
        &board,
        Some(original.clone()),
        &mut None,
        (&[], std::slice::from_ref(&lattice)),
        &[],
        Some(&options),
        true,
    )
    .expect("lattice-aware swap");
    let reserved = reserved.expect("replacement ledger");
    assert!(!held.witnesses.is_empty() && held.released.is_empty(), "the bypass and replacement escapes must survive");
    assert!(!reserved.1.is_empty(), "dropping all escapes is not a successful swap");
    assert!(reserved.1.iter().all(|via| !original.1.contains(via)), "the actual swap must replace the taken ring");
    assert!(!held.vias.contains(&lattice), "the fixed lattice is not generated bypass copper");
    assert!(!reserved.1.contains(&lattice), "the fixed lattice is not generated escape copper");

    let mut complete = board.clone();
    complete.vias.push(lattice.clone());
    complete.tracks.extend(held.tracks.iter().cloned());
    complete.vias.extend(held.vias.iter().cloned());
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&complete).expect("complete frame");
    assert!(
        !index
            .can_via(
                &replacement.net,
                replacement.position,
                replacement.size_nm,
                replacement.from_layer,
                replacement.to_layer,
                Some(replacement.drill_nm),
                false,
                None,
            )
            .expect("old site collision"),
        "the fixture must make the old replacement site illegal"
    );
    let legal = legal_reservation_nets(
        &board,
        &complete,
        (&[], &[]),
        &reserved.0,
        &reserved.1,
        &["S".to_owned()].into_iter().collect(),
    )
    .expect("replacement geometry validation");
    assert!(legal.contains("S"), "the complete replacement group must remain legal beside lattice and bypass");
    assert!(board.tracks.is_empty() && board.vias.is_empty(), "the original source stays unchanged");

    // Match the actual stage's old witness context, then adapt it only after
    // proving the original paths. This combination used to fall back because
    // strict binding saw the reservation-mask context digest on mask-free input.
    let (held, reason) = refresh_bypass_witnesses(&board, Some(&reserved), held).expect("old-context refresh");
    assert!(reason.is_none(), "fixture requires a valid original witness: {reason:?}");
    assert!(!reserved.2.is_empty() && !held.witnesses.is_empty());
    let original_held = format!("{held:?}");
    let lattice = Lattice { tracks: Vec::new(), vias: vec![lattice], keepouts: Vec::new(), packages: Vec::new() };
    let mut repeated = reserved.clone();
    let duplicate = repeated.0.first().expect("real ordinary launch track").clone();
    repeated.0.push(duplicate.clone());
    let (prefix, bound) = retained_critical_inputs(&board, Some(&repeated), Some(&lattice), Some(&held))
        .expect("real combined Held/lattice/ordinary frame must be admitted");
    let bound = bound.expect("Held bound as exact generated bypass copper");
    assert_eq!(format!("{held:?}"), original_held, "original witness provenance must stay unchanged");
    assert_eq!(bound.created_tracks, held.tracks);
    assert_eq!(bound.created_vias, held.vias);
    assert_eq!(bound.decoupling_witnesses.len(), held.witnesses.len());
    for (original, current) in held.witnesses.iter().zip(&bound.decoupling_witnesses) {
        assert_eq!(current.link, original.link);
        assert_eq!(current.return_target, original.return_target);
        assert_ne!(current.context_digest, original.context_digest, "known mask removal needs a fresh proof");
    }
    let mut damaged = held.clone();
    damaged
        .provenance
        .as_mut()
        .expect("captured provenance")
        .batches
        .iter_mut()
        .find(|batch| !batch.witnesses.is_empty())
        .expect("original witness batch")
        .witnesses[0]
        .context_digest = "unsupported-original-context".to_owned();
    let error = retained_critical_inputs(&board, Some(&repeated), Some(&lattice), Some(&damaged))
        .expect_err("never invent a fresh association when original provenance is stale");
    assert!(error.to_string().contains("invalid original Held producer witness: stale_witness_context"));
    let mut changed_mask_context = repeated.clone();
    changed_mask_context.2.pop();
    assert!(
        retained_critical_inputs(&board, Some(&changed_mask_context), Some(&lattice), Some(&held)).is_ok(),
        "raw producer contexts must survive later temporary-mask changes"
    );

    let global = crate::route_global(&board, &GlobalRouterOptions::default(), None, None, None).expect("guides");
    let critical = crate::critical::route_critical_nets(
        board.clone(),
        &global,
        &crate::critical::CriticalOptions {
            fixed_prefix: Some(&prefix),
            reserved_accesses: Some(&bound),
            ..Default::default()
        },
        None,
    )
    .expect("critical routing on the real combined frame");
    require_retained_prefix(&critical.board, &prefix).expect("ordered duplicate prefix retained");
    let expected_tracks = critical.board.tracks.clone();
    let expected_vias = critical.board.vias.clone();
    let unified = UnifiedRouterOptions {
        retain_escape_copper: true,
        planned_escapes_as_copper: true,
        pre_route_decouplers: true,
        bga_fanout: false,
        ..Default::default()
    };
    let (mut completed, mut stitched) = complete_critical_stage(
        &board,
        critical,
        Some(&repeated),
        Some(&held),
        true,
        true,
        &unified,
        None,
        &mut None,
        None,
        Some(&lattice),
        Some(&options),
        None,
    )
    .expect("combined-frame completion");
    let before_release = completed.board.clone();
    let final_board = release_escape_masks(
        &board,
        Some(&repeated),
        &mut completed,
        &mut stitched,
        before_release,
        &unified,
        &mut None,
    )
    .expect("combined-frame final join/release");
    assert_eq!(final_board.tracks, expected_tracks, "completion must not duplicate or collapse owned tracks");
    assert_eq!(final_board.vias, expected_vias, "completion must not join sites or Held twice");
    assert_eq!(final_board.tracks.iter().filter(|track| **track == duplicate).count(), 2);

    // Also exercise the actual stage producer and callback, rather than only
    // its helper: ordinary escapes, lattice and nonempty cross-layer Held must
    // reach critical routing together without a conservative fallback.
    let mut stage_board = board.clone();
    let mut stage_lattice = add_pending_inner_grid(&mut stage_board);
    stage_lattice.tracks.extend(lattice.tracks.iter().cloned());
    stage_lattice.vias.extend(lattice.vias.iter().cloned());
    stage_lattice.keepouts.extend(lattice.keepouts.iter().cloned());
    let stage_global =
        crate::route_global(&stage_board, &GlobalRouterOptions::default(), None, None, None).expect("BGA stage guides");
    let placement = PlacementRoutingResult {
        status: crate::routeflow::FeedbackStatus::Pass,
        board: stage_board.clone(),
        global_route: stage_global,
        placement_candidate: "combined-held-prefix".to_owned(),
        iterations: Vec::new(),
        accepted_moves: 0,
        full_route_certified: false,
    };
    let mut captured = None;
    let mut callback = |stage: Stage<'_>| {
        if let Stage::CriticalInputs { board: current, fixed_prefix, reserved_accesses, .. } = stage {
            assert_eq!(current, &stage_board);
            let prefix = fixed_prefix.expect("combined stage must not fall back to proxy masks");
            let bypass = reserved_accesses.expect("nonempty Held must bind on the physical frame");
            assert!(!bypass.decoupling_witnesses.is_empty());
            assert!(prefix.ranges().iter().any(|range| {
                range.role == crate::fixed_prefix::FixedPrefixRole::OrdinaryEscape
                    && (!range.tracks.is_empty() || !range.vias.is_empty())
            }));
            captured = Some((bypass.board.tracks.clone(), bypass.board.vias.clone()));
        }
    };
    let (critical, reserved, held, held_as_copper, sites_as_copper, retained_prefix) = route_critical_stage(
        &placement,
        &unified,
        Some(&options),
        None,
        None,
        Some(&stage_lattice),
        &mut None,
        &mut Some(&mut callback),
    )
    .expect("full combined stage");
    assert!(held_as_copper && sites_as_copper);
    assert!(retained_prefix.is_some());
    let expected = captured.expect("actual critical-input callback");
    assert_eq!((critical.board.tracks.clone(), critical.board.vias.clone()), expected);
    let (mut completed, mut stitched) = complete_critical_stage(
        &stage_board,
        critical,
        reserved.as_ref(),
        held.as_ref(),
        held_as_copper,
        sites_as_copper,
        &unified,
        None,
        &mut None,
        None,
        Some(&stage_lattice),
        Some(&options),
        None,
    )
    .expect("full combined stage completion");
    let before_release = completed.board.clone();
    let final_board = release_escape_masks(
        &stage_board,
        reserved.as_ref(),
        &mut completed,
        &mut stitched,
        before_release,
        &unified,
        &mut None,
    )
    .expect("full combined stage final join");
    assert_eq!((final_board.tracks, final_board.vias), expected);
}

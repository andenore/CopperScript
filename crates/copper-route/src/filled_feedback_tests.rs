//! Regression tests for native acceptance across index-based export UUIDs.
use super::*;
use crate::plane_verify::{NativeFinding, kicad_export_digest};
use crate::test_support_routers::{mm, pad_board, pt};

fn layer_census_fixture(connect_right_via: bool) -> (PhysicalBoard, PlaneVerification) {
    use crate::test_support_routers::{rect, via, zone};
    let mut tracks = Vec::new();
    for y in 3..8 {
        tracks.push(track("GND", (3.0, f64::from(y)), (3.0, f64::from(y + 1))));
        tracks.push(track("GND", (11.0, f64::from(y)), (11.0, f64::from(y + 1))));
    }
    tracks.push(track("GND", (3.0, 8.0), (2.0, 9.0)));
    if connect_right_via {
        tracks.push(track("GND", (11.0, 8.0), (12.0, 9.0)));
    }
    tracks.push(track("OTHER", (7.0, 0.5), (7.0, 12.0)));
    let mut board = pad_board(&[("GND", &[(0.0, 0.0), (8.0, 0.0)]), ("OTHER", &[])], 2, tracks);
    board.rules.default_via_size_nm = mm(0.45);
    board.rules.default_via_drill_nm = mm(0.20);
    board.rules.minimum_via_size_nm = Some(mm(0.45));
    board.rules.minimum_via_drill_nm = Some(mm(0.20));
    board.vias.extend([
        via("GND", (2.0, 9.0), 0.45, 0.20, CopperLayer::Front, CopperLayer::Back),
        via("GND", (12.0, 9.0), 0.45, 0.20, CopperLayer::Front, CopperLayer::Back),
    ]);
    // Declaring a zone makes GND eligible. Its unfilled source outline must
    // not join either plated component or the deliberately orphaned via.
    board.zones.push(zone("ground", "GND", &[CopperLayer::Back], rect(0.0, 0.0, 26.0, 26.0)));
    let objects = native_object_map(&board).unwrap();
    let pad =
        |reference: &str| objects.values().find(|item| item.component.as_deref() == Some(reference)).unwrap().clone();
    let native = evidence(
        &board,
        1,
        vec![NativeFinding {
            category: "unconnected".to_owned(),
            kind: "unconnected_items".to_owned(),
            description: String::new(),
            items: vec![pad("GND_0"), pad("GND_1")],
        }],
    );
    (board, native)
}

fn census_options() -> FilledCopperFeedbackOptions {
    FilledCopperFeedbackOptions {
        maximum_trials: 8,
        maximum_search_states: 4000,
        maximum_blockers: 2,
        maximum_dependency_expansions: 1,
    }
}

#[test]
fn native_open_overrides_fill_grouping_for_owned_bridge_access() {
    let (board, mut native) = layer_census_fixture(true);
    // An extracted fill can overjoin contacts that KiCad explicitly reports
    // disconnected. Native negative evidence must still get a bridge search;
    // the inferred fill grouping is not a connectivity certificate.
    native.filled_regions.push(crate::plane_verify::NativeFilledRegion {
        zone_uuid: "extracted-zone".into(),
        fragment_digest: "optimistic-fill".into(),
        net: "GND".into(),
        layer: "B.Cu".into(),
        vertices_nm: vec![(mm(1.0), mm(8.5)), (mm(13.0), mm(8.5)), (mm(13.0), mm(9.5)), (mm(1.0), mm(9.5))],
    });
    let eligible = copper_pcb::zones::distribution_zone_nets(&board);
    let inferred = filled_component_roots(&board, &native, &eligible).unwrap();
    let first = native_item_identity(&native.native_findings[0].items[0]).unwrap();
    let second = native_item_identity(&native.native_findings[0].items[1]).unwrap();
    assert_eq!(inferred.get(&first).unwrap(), inferred.get(&second).unwrap());
    let queue = proposals(&board, &native, MAXIMUM_FRAGMENT_ANCHORS).unwrap();
    let back = queue
        .iter()
        .find(|proposal| proposal.2 == CopperLayer::Back)
        .expect("the native gap must retain a proposal through each branch's actual plated contact");
    let mut ends = [(back.3.x_nm, back.3.y_nm), (back.4.x_nm, back.4.y_nm)];
    ends.sort();
    assert_eq!(ends, [(mm(2.0), mm(9.0)), (mm(12.0), mm(9.0))]);
    let bridge = crate::surface_path::surface_path_between_observed(
        &board,
        &RoutingClearanceIndex::new(&board).unwrap(),
        &back.0,
        back.3,
        back.4,
        mm(0.2),
        back.2,
        STEP_NM,
        MAXIMUM_DETOUR_NM,
        4000,
        true,
    )
    .unwrap()
    .path
    .expect("bounded bridge between the two real copper branches");
    let mut joined = board.clone();
    joined.tracks.extend(bridge);
    assert!(!hard_findings(&joined).unwrap());
    let explicit = filled_component_roots(&joined, &evidence(&joined, 0, vec![]), &eligible).unwrap();
    assert_eq!(explicit.get(&first).unwrap(), explicit.get(&second).unwrap());
}

#[test]
fn layer_fair_proposal_prefix_reaches_owned_back_access_without_promoting_orphan_geometry() {
    let options = census_options();
    let (board, native) = layer_census_fixture(true);
    let (queue, census) = proposal_queue(&board, &native, MAXIMUM_FRAGMENT_ANCHORS, Some(&options)).unwrap();
    assert_eq!(
        queue,
        proposals(&board, &native, MAXIMUM_FRAGMENT_ANCHORS).unwrap(),
        "observation preserves the exact queue"
    );
    let objects: Vec<_> = native_object_map(&board).unwrap().into_values().collect();
    let eligible = copper_pcb::zones::distribution_zone_nets(&board);
    let components = filled_component_roots(&board, &native, &eligible).unwrap();
    let raw = finding_proposals(
        &board,
        &native,
        &native.native_findings[0],
        &objects,
        &components,
        &eligible,
        MAXIMUM_FRAGMENT_ANCHORS,
    );
    let distance_first = interleave_proposals(vec![raw.clone()]);
    let old_back_rank = distance_first.iter().position(|p| p.2 == CopperLayer::Back).unwrap() + 1;
    assert!(old_back_rank > 8, "the former distance-first prefix must exhaust all eight trials on front geometry");
    assert!(distance_first.iter().take(8).all(|p| p.2 == CopperLayer::Front));
    assert_eq!(queue[0], distance_first[0], "keep the original closest first candidate");
    let key = |p: &Proposal| proposal_key(&p.0, p.2, p.3, p.4);
    let original: IndexMap<_, _> = distance_first.iter().map(|p| (key(p), p)).collect();
    assert_eq!(queue.len(), original.len(), "no geometry added or removed");
    for p in &queue {
        assert_eq!(original.get(&key(p)), Some(&p), "preserve the exact native payload and source-lattice direction");
    }
    for layer in [CopperLayer::Front, CopperLayer::Back] {
        let within_layer = |items: &[Proposal]| items.iter().filter(|p| p.2 == layer).cloned().collect::<Vec<_>>();
        assert_eq!(within_layer(&queue), within_layer(&distance_first), "retain distance order within each layer");
    }
    let census = census.unwrap();
    let back_rank = queue.iter().position(|p| p.2 == CopperLayer::Back).unwrap() + 1;
    assert_eq!(back_rank, 2, "the existing owned back-layer proposal now reaches the same bounded prefix");
    assert_eq!(census.generated, raw.len(), "census observes raw generation before layer dedup/reordering");
    let row = census.finding_layers.iter().find(|row| row.layer == CopperLayer::Back).unwrap();
    assert_eq!(row.queued, 1);
    assert_eq!(row.first_queue_rank, Some(back_rank));
    assert_eq!(row.prefix, 1);
    assert_eq!(row.samples[0].queue_rank, Some(back_rank));
    assert!(row.samples[0].retained_payload);
    let front = census.finding_layers.iter().find(|row| row.layer == CopperLayer::Front).unwrap();
    assert_eq!(front.generated, raw.iter().filter(|p| p.2 == CopperLayer::Front).count());
    assert!(front.generated > front.queued, "raw duplicate counts remain visible after layer deduplication");
    let document = census.document(&[]);
    let back = document
        .get("finding_layers")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row.get("layer").and_then(Value::as_str) == Some("B.Cu"))
        .unwrap();
    assert_eq!(back.get("attempted_trial_count"), Some(&int(0)));
    assert_eq!(
        back.get("samples").unwrap().as_array().unwrap()[0].get("attempted_geometry"),
        Some(&Value::Bool(false))
    );

    // This bounded unit search establishes geometry, not native acceptance.
    // Production census collection never performs this additional search.
    let search = |board: &PhysicalBoard, proposal: &Proposal| {
        crate::surface_path::surface_path_between_observed(
            board,
            &RoutingClearanceIndex::new(board).unwrap(),
            &proposal.0,
            proposal.3,
            proposal.4,
            mm(0.2),
            proposal.2,
            STEP_NM,
            MAXIMUM_DETOUR_NM,
            4000,
            true,
        )
        .unwrap()
    };
    assert!(
        distance_first.iter().take(8).all(|p| search(&board, p).path.is_none()),
        "the fixture's foreign front wall blocks every candidate in the old eight-trial prefix"
    );
    let alternate = &queue[back_rank - 1];
    let path =
        search(&board, alternate).path.expect("the actual queued back-layer proposal has a legal bounded bridge");
    let mut connected = board.clone();
    connected.tracks.extend(path);
    assert!(!hard_findings(&connected).unwrap(), "the retained width/via rules and exact geometry remain legal");
    let joined = evidence(&connected, 0, vec![]);
    let joined_components = filled_component_roots(&connected, &joined, &eligible).unwrap();
    let first = native_item_identity(&native.native_findings[0].items[0]).unwrap();
    let second = native_item_identity(&native.native_findings[0].items[1]).unwrap();
    assert_eq!(
        joined_components.get(&first).expect("first native contact root"),
        joined_components.get(&second).expect("second native contact root"),
        "the bridge joins both original native contacts through actual plated copper"
    );
    let (orphan, orphan_native) = layer_census_fixture(false);
    assert!(
        search(&orphan, alternate).path.is_some(),
        "a visible, legal back-layer segment is insufficient evidence of component access"
    );
    let (orphan_queue, orphan_census) =
        proposal_queue(&orphan, &orphan_native, MAXIMUM_FRAGMENT_ANCHORS, Some(&options)).unwrap();
    assert!(
        !orphan_queue.iter().any(|p| p.2 == CopperLayer::Back),
        "an orphan same-net via must not be queued as the native contact's anchor"
    );
    assert!(!orphan_census.unwrap().finding_layers.iter().any(|row| row.layer == CopperLayer::Back));
}

#[test]
fn layer_fair_proposals_preserve_outer_fairness_and_first_duplicate_payload() {
    let proposal = |contact: &str, x: f64, layer: CopperLayer| -> Proposal {
        ("GND".to_owned(), (contact.to_owned(), "destination".to_owned()), layer, pt(x, 1.0), pt(x, 2.0))
    };
    let a = proposal("first", 1.0, CopperLayer::Front);
    let c = proposal("first", 2.0, CopperLayer::Front);
    let b = proposal("first-back", 3.0, CopperLayer::Back);
    let d = proposal("second", 4.0, CopperLayer::Front);
    let mut reversed = c.clone();
    std::mem::swap(&mut reversed.3, &mut reversed.4);
    reversed.1 = ("duplicate-source".to_owned(), "duplicate-destination".to_owned());
    let first = vec![a.clone(), c.clone(), b.clone()];
    let second = vec![d.clone(), reversed];
    let mut supply = proposal("supply", 5.0, CopperLayer::Front);
    supply.0 = "D18".to_owned();
    let distance_first =
        interleave_proposals(vec![interleave_proposals(vec![first.clone(), second.clone()]), vec![supply.clone()]]);
    let fair = interleave_proposals(vec![
        interleave_proposals(vec![layer_fair_proposals(first), layer_fair_proposals(second)]),
        vec![supply.clone()],
    ]);
    assert_eq!(&fair[..3], &[a.clone(), supply, d.clone()], "both nets and both native findings keep their first turn");
    assert_eq!(
        fair,
        vec![a.clone(), distance_first[1].clone(), d, b.clone(), c.clone()],
        "the duplicate offered by finding two keeps finding one's original payload/direction"
    );
    let key = |p: &Proposal| proposal_key(&p.0, p.2, p.3, p.4);
    assert_eq!(fair.len(), distance_first.len());
    for expected in &distance_first {
        assert_eq!(fair.iter().find(|p| key(p) == key(expected)), Some(expected));
    }
    assert_eq!(
        layer_fair_proposals(vec![b.clone(), a.clone(), c.clone()]),
        vec![b, a, c],
        "layer order follows the first raw candidate, without hardcoded front priority"
    );
}

#[test]
fn proposal_census_globally_bounds_samples_and_attributes_only_exact_input_trials() {
    let options = census_options();
    let (board, mut native) = layer_census_fixture(true);
    native.native_findings = vec![native.native_findings[0].clone(); 17];
    native.unconnected_count = 17;
    let (queue, census) = proposal_queue(&board, &native, MAXIMUM_FRAGMENT_ANCHORS, Some(&options)).unwrap();
    assert_eq!(queue, proposals(&board, &native, MAXIMUM_FRAGMENT_ANCHORS).unwrap());
    let census = census.unwrap();
    assert_eq!(census.finding_identities.len(), MAXIMUM_CENSUS_FINDINGS);
    assert_eq!(census.sample_count, MAXIMUM_CENSUS_SAMPLES);
    assert!(census.finding_layers.len() <= MAXIMUM_CENSUS_LAYER_ROWS);
    assert!(census.generated > census.queued, "duplicate native findings must not multiply retained queue geometry");
    let p = &queue[0];
    // A synthetic completed record tests attribution without evaluating any
    // extra routing/native trial. One different input must remain uncovered.
    let trial = |input_digest: &str| FilledCopperTrial {
        index: 1,
        input_digest: input_digest.to_owned(),
        net: p.0.clone(),
        native_item_uuids: p.1.clone(),
        layer: p.2.value().to_owned(),
        outcome: "no_legal_surface_bridge".to_owned(),
        blockers: vec![],
        displaced_nets: vec![],
        dependency_expansions: 0,
        expanded_states: 1,
        searches: vec![SurfaceBridgeAttempt {
            snapshot: "all_copper",
            start: p.3,
            end: p.4,
            width_nm: mm(0.2),
            state_budget: 4000,
            search: crate::surface_path::SurfaceBridgeSearch {
                path: None,
                expanded_states: 1,
                outcome: "search_exhausted",
                blocker_records: vec![],
                diagnostics: Default::default(),
            },
        }],
        dependency_searches: 0,
        candidate_digest: None,
        verification: None,
        cleanup_removed_tracks: 0,
        cleanup_removed_vias: 0,
    };
    let document = census.document(&[trial(&native.board_digest), trial("later-accepted-input")]);
    assert_eq!(document.get("attempted_initial_input_trial_count"), Some(&int(1)));
    assert_eq!(document.get("later_input_trial_count_not_covered"), Some(&int(1)));
    assert_eq!(document.get("omitted_finding_identity_count"), Some(&int(1)));
    assert!(document.get("omitted_first_geometry_sample_count") != Some(&int(0)));
    let first = document
        .get("finding_layers")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.get("samples").unwrap().as_array().unwrap())
        .find(|sample| sample.get("queue_rank") == Some(&int(1)))
        .unwrap();
    assert_eq!(first.get("attempted_geometry"), Some(&Value::Bool(true)));
    assert_eq!(first.get("attempted_original_payload"), Some(&Value::Bool(true)));
}

#[test]
fn native_open_contacts_share_a_bounded_trial_prefix_before_target_variants() {
    use crate::test_support_routers::{rect, zone};
    // Two isolated pads share a large connected destination with enough anchor
    // variants for the old concatenation to spend all eight trials on pad 1.
    let tracks = (0..16)
        .map(|index| track("GND", (3.0 + f64::from(index) * 0.25, 3.0), (3.25 + f64::from(index) * 0.25, 3.0)))
        .collect();
    let mut board = pad_board(&[("GND", &[(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)])], 2, tracks);
    board.zones.push(zone("ground", "GND", &[CopperLayer::Back], rect(0.0, 0.0, 26.0, 26.0)));
    let objects = native_object_map(&board).unwrap();
    let pad =
        |reference: &str| objects.values().find(|item| item.component.as_deref() == Some(reference)).unwrap().clone();
    let findings: Vec<_> = ["GND_1", "GND_2"]
        .iter()
        .map(|reference| NativeFinding {
            category: "unconnected".to_owned(),
            kind: "unconnected_items".to_owned(),
            description: String::new(),
            items: vec![pad(reference), pad("GND_0")],
        })
        .collect();
    let native = evidence(&board, 2, findings);
    let eligible = copper_pcb::zones::distribution_zone_nets(&board);
    let components = filled_component_roots(&board, &native, &eligible).unwrap();
    let objects: Vec<_> = objects.into_values().collect();
    let per_contact: Vec<_> = native
        .native_findings
        .iter()
        .map(|finding| {
            finding_proposals(&board, &native, finding, &objects, &components, &eligible, MAXIMUM_FRAGMENT_ANCHORS)
        })
        .collect();
    assert!(per_contact[0].len() >= 8, "fixture must reproduce starvation under the former eight-trial prefix");
    assert!(!per_contact[1].is_empty());
    let scheduled = proposals(&board, &native, MAXIMUM_FRAGMENT_ANCHORS).unwrap();
    assert_eq!(&scheduled[..2], &[per_contact[0][0].clone(), per_contact[1][0].clone()]);
    let contacts: IndexSet<_> = scheduled.iter().take(8).map(|proposal| proposal.1.0.clone()).collect();
    assert_eq!(contacts.len(), 2, "both native open contacts receive a trial within the unchanged cap");
    let key = |proposal: &Proposal| proposal_key(&proposal.0, proposal.2, proposal.3, proposal.4);
    let original: IndexSet<_> = per_contact.iter().flatten().map(key).collect();
    let reordered: IndexSet<_> = scheduled.iter().map(key).collect();
    assert_eq!(original.len(), scheduled.len(), "scheduling adds no trials or duplicate geometry");
    assert!(original.iter().all(|key| reordered.contains(key)), "every existing proposal is preserved");
    assert_eq!(scheduled, proposals(&board, &native, MAXIMUM_FRAGMENT_ANCHORS).unwrap(), "deterministic ordering");
}

#[test]
fn contact_scheduling_preserves_net_fairness_and_deduplicates_reversed_geometry() {
    let proposal = |net: &str, contact: &str, x: f64| -> Proposal {
        (net.to_owned(), (contact.to_owned(), "destination".to_owned()), CopperLayer::Front, pt(x, 1.0), pt(x, 2.0))
    };
    let a = proposal("GND", "first", 1.0);
    let a_next = proposal("GND", "first", 2.0);
    let b = proposal("GND", "second", 3.0);
    let mut reversed = a.clone();
    std::mem::swap(&mut reversed.3, &mut reversed.4);
    let ground = interleave_proposals(vec![vec![a.clone(), a_next.clone()], vec![reversed, b.clone()], vec![]]);
    assert_eq!(ground, vec![a.clone(), b.clone(), a_next.clone()]);
    let supply = proposal("D18", "supply", 4.0);
    let scheduled = interleave_proposals(vec![vec![supply.clone()], ground]);
    assert_eq!(
        scheduled,
        vec![supply, a, b, a_next],
        "existing first supply candidate and between-net ordering stay intact"
    );
}

#[test]
fn contact_scheduling_preserves_original_payload_when_reversed_duplicate_is_scheduled_first() {
    let proposal = |contact: &str, x: f64| -> Proposal {
        ("GND".to_owned(), (contact.to_owned(), "destination".to_owned()), CopperLayer::Front, pt(x, 1.0), pt(x, 2.0))
    };
    let a = proposal("first", 1.0);
    let c = proposal("first", 2.0);
    let b = proposal("second", 3.0);
    let mut reversed = c.clone();
    std::mem::swap(&mut reversed.3, &mut reversed.4);
    reversed.1 = ("other-source".to_owned(), "other-destination".to_owned());
    // The second queue offers c before the first queue reaches it. Fairness
    // may advance that geometry, but must retain its original search direction
    // and native item identities, including through the outer net scheduler.
    let ground = interleave_proposals(vec![vec![a.clone(), c.clone()], vec![reversed, b.clone()]]);
    assert_eq!(ground, vec![a.clone(), c.clone(), b.clone()]);
    let mut supply = proposal("supply", 4.0);
    supply.0 = "D18".to_owned();
    assert_eq!(interleave_proposals(vec![vec![supply.clone()], ground]), vec![supply, a, c, b]);
}

#[test]
fn native_scope_distinguishes_unzoned_opens_from_search_failures() {
    use crate::test_support_routers::{rect, rule, zone};
    let mut board = fixture();
    let old = evidence(&board, 1, vec![]);
    let scope = native_open_scope(&board, Some(&old), &[]);
    assert_eq!(
        scope.as_array().unwrap()[0].get("scope").and_then(Value::as_str),
        Some("no_declared_distribution_zone")
    );
    assert_eq!(scope.as_array().unwrap()[0].get("trial_count"), Some(&int(0)));
    board.zones.push(zone("supply", "D18", &[CopperLayer::Back], rect(0.0, 0.0, 26.0, 26.0)));
    let scope = native_open_scope(&board, Some(&old), &[]);
    assert_eq!(scope.as_array().unwrap()[0].get("scope").and_then(Value::as_str), Some("eligible_general_zone"));
    let mut special = rule("D18");
    special.kind = RouteKind::Clock;
    board.net_routing_rules.push(special);
    let scope = native_open_scope(&board, Some(&old), &[]);
    assert_eq!(scope.as_array().unwrap()[0].get("scope").and_then(Value::as_str), Some("specialized_route"));
}

fn bridge_search(
    board: &PhysicalBoard,
    start: Point,
    end: Point,
    budget: i64,
    observed: bool,
) -> crate::surface_path::SurfaceBridgeSearch {
    crate::surface_path::surface_path_between_observed(
        board,
        &RoutingClearanceIndex::new(board).unwrap(),
        "D18",
        start,
        end,
        mm(0.2),
        CopperLayer::Front,
        STEP_NM,
        MAXIMUM_DETOUR_NM,
        budget,
        observed,
    )
    .unwrap()
}

#[test]
fn bridge_diagnostics_distinguish_budget_and_blocked_initial_access() {
    let mut board = pad_board(&[("D18", &[(0.0, 0.0), (8.0, 0.0)]), ("SIGNAL", &[(0.0, 8.0)])], 2, vec![]);
    let found = bridge_search(&board, pt(3.0, 3.0), pt(11.0, 3.0), 1, true);
    assert_eq!(found.outcome, "state_budget_exhausted");
    assert!(found.diagnostics.initial_legal_steps > 0);
    assert_eq!(found.expanded_states, 1);
    let plain = bridge_search(&board, pt(3.0, 3.0), pt(11.0, 3.0), 1, false);
    assert_eq!(
        (found.path.clone(), found.expanded_states, found.outcome),
        (plain.path, plain.expanded_states, plain.outcome)
    );
    board.tracks.push(track("SIGNAL", (3.0, 1.0), (3.0, 5.0)));
    let blocked = bridge_search(&board, pt(3.0, 3.0), pt(11.0, 3.0), 1000, true);
    assert_eq!(blocked.outcome, "search_exhausted");
    assert_eq!(blocked.diagnostics.initial_legal_steps, 0);
    assert_eq!(blocked.expanded_states, 1);
    assert!(blocked.diagnostics.clearance_rejections > 0);
    assert!(blocked.blocker_records.iter().any(|blocker| blocker.net.as_deref() == Some("SIGNAL")));
    assert!(blocked.blocker_records.len() <= 32);
    let report = SurfaceBridgeAttempt {
        snapshot: "protected_copper",
        start: pt(3.0, 3.0),
        end: pt(11.0, 3.0),
        width_nm: mm(0.2),
        state_budget: 1000,
        search: blocked,
    }
    .document();
    assert_eq!(report.get("failure_stage").and_then(Value::as_str), Some("no_legal_initial_grid_step"));
    assert_eq!(report.get("state_budget"), Some(&int(1000)));
}

#[test]
fn bridge_diagnostics_record_rejected_final_connection_without_changing_search() {
    let board = pad_board(&[("D18", &[(0.0, 0.0)]), ("SIGNAL", &[(2.0, 0.0)])], 2, vec![]);
    let found = bridge_search(&board, pt(3.0, 3.0), pt(5.0, 3.0), 4000, true);
    let plain = bridge_search(&board, pt(3.0, 3.0), pt(5.0, 3.0), 4000, false);
    assert!(found.path.is_none());
    assert_eq!(
        (found.path.clone(), found.expanded_states, found.outcome),
        (plain.path, plain.expanded_states, plain.outcome)
    );
    assert!(found.diagnostics.initial_legal_steps > 0);
    assert!(found.diagnostics.final_connection_attempts > 0);
    assert_eq!(found.diagnostics.final_connection_attempts, found.diagnostics.final_connection_rejections);
}

fn track(net: &str, a: (f64, f64), b: (f64, f64)) -> TrackSegment {
    TrackSegment {
        net: net.to_owned(),
        start: pt(a.0, a.1),
        end: pt(b.0, b.1),
        width_nm: mm(0.2),
        layer: CopperLayer::Front,
    }
}

fn fixture() -> PhysicalBoard {
    let mut board = pad_board(
        &[("D18", &[(0.0, 0.0), (8.0, 0.0)]), ("SIGNAL", &[(0.0, 8.0), (8.0, 8.0)])],
        2,
        vec![
            track("D18", (3.0, 3.0), (5.0, 3.0)),
            track("D18", (9.0, 3.0), (11.0, 3.0)),
            track("SIGNAL", (3.0, 11.0), (11.0, 11.0)),
            track("SIGNAL", (5.0, 11.0), (5.0, 13.0)),
        ],
    );
    board.vias.push(Via {
        net: "SIGNAL".to_owned(),
        position: pt(5.0, 13.0),
        size_nm: mm(0.6),
        drill_nm: mm(0.3),
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    });
    board
}

fn item(board: &PhysicalBoard, kind: &str, index: i64) -> NativeCopperItem {
    native_object_map(board).unwrap().into_values().find(|item| item.kind == kind && item.index == Some(index)).unwrap()
}

fn finding(board: &PhysicalBoard, kind: &str, index: i64) -> NativeFinding {
    NativeFinding {
        category: "violation".to_owned(),
        kind: format!("{kind}_dangling"),
        description: String::new(),
        items: vec![item(board, if kind == "track" { "segment" } else { kind }, index)],
    }
}

fn evidence(board: &PhysicalBoard, opens: i64, mut findings: Vec<NativeFinding>) -> PlaneVerification {
    let existing = findings.iter().filter(|finding| finding.category == "unconnected").count() as i64;
    for _ in existing..opens {
        findings.push(NativeFinding {
            category: "unconnected".to_owned(),
            kind: "unconnected_items".to_owned(),
            description: String::new(),
            items: vec![
                native_object_map(board)
                    .unwrap()
                    .into_values()
                    .find(|item| item.kind == "pad" && item.net.as_deref() == Some("D18"))
                    .unwrap(),
            ],
        });
    }
    PlaneVerification {
        passed: false,
        board_digest: copper_drc::digest::physical_board_digest(board),
        export_digest: kicad_export_digest(&copper_pcb::kicad_pcb::generate(board).unwrap()),
        filled_board_digest: String::new(),
        report_digest: String::new(),
        kicad_version: "10.0.0".to_owned(),
        unconnected_count: opens,
        island_count: 0,
        other_violation_count: findings.len() as i64,
        findings: Vec::new(),
        dangling_track_uuids: Vec::new(),
        expected_antenna_open_ends: Vec::new(),
        native_findings: findings,
        artifact_directory: None,
        filled_regions: Vec::new(),
        decoupling_assessments: Vec::new(),
    }
}

#[test]
fn cleanup_census_attributes_unique_occurrences_and_never_promotes_stale_or_duplicate_copper() {
    use crate::fixed_prefix::{AdmittedFixedPrefix, FixedContribution, FixedPrefixRole};
    let source = pad_board(&[("S", &[])], 2, vec![]);
    let first = track("S", (8.0, 8.0), (9.0, 8.0));
    let second = track("S", (8.0, 10.0), (9.0, 10.0));
    let prefix = AdmittedFixedPrefix::admit(
        &source,
        &[
            FixedContribution {
                id: "local".into(),
                role: FixedPrefixRole::OrdinaryEscape,
                tracks: vec![first.clone()],
                vias: vec![],
            },
            FixedContribution {
                id: "sites".into(),
                role: FixedPrefixRole::ViaLattice,
                tracks: vec![second],
                vias: vec![],
            },
        ],
    )
    .unwrap();
    let board = prefix.board();
    let native = evidence(board, 0, vec![finding(board, "track", 0), finding(board, "track", 1)]);
    let census = |b: &PhysicalBoard, n: &PlaneVerification| {
        crate::native_stub_cleanup::native_cleanup_census(
            b,
            n,
            &multiset(&board.tracks),
            &multiset(&board.vias),
            Some(&prefix),
        )
        .unwrap()
    };
    let result = census(board, &native);
    let items = result.get("items").unwrap().as_array().unwrap();
    assert_eq!(items[0].get("producer").and_then(Value::as_str), Some("ordinary_escape"));
    assert_eq!(items[1].get("producer").and_then(Value::as_str), Some("via_lattice"));
    assert!(items.iter().all(|item| item.get("caller_protected") == Some(&Value::Bool(true))));
    let mut duplicate = board.clone();
    duplicate.tracks.push(first);
    let stale = census(&duplicate, &native);
    assert_eq!(stale.get("fresh_native_binding"), Some(&Value::Bool(false)));
    assert!(
        stale
            .get("items")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("producer").and_then(Value::as_str) == Some("unresolved"))
    );
    let current = evidence(&duplicate, 0, vec![finding(&duplicate, "track", 0)]);
    assert_eq!(
        census(&duplicate, &current).get("items").unwrap().as_array().unwrap()[0]
            .get("producer")
            .and_then(Value::as_str),
        Some("unresolved")
    );
}

#[test]
fn cleanup_off_centre_via_whole_trials_keep_pad_partitions() {
    let mut board = fixture();
    board.vias[0].position.x_nm += mm(0.1);
    let native = evidence(&board, 0, vec![finding(&board, "track", 3), finding(&board, "via", 0)]);
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::native_stub_candidate(
        &board,
        &native,
        &multiset(&[]),
        Some(&multiset(&[])),
        &mut diagnostics,
    )
    .unwrap()
    .expect("nonessential whole spur removal");
    assert_eq!(candidate.tracks, board.tracks[..3]);
    assert!(candidate.vias.is_empty());
    let mut essential = board.clone();
    essential.tracks.pop();
    essential.vias[0].position = pt(7.0, 11.1);
    let native = evidence(&essential, 0, vec![finding(&essential, "track", 2)]);
    assert!(
        crate::native_stub_cleanup::native_stub_candidate(
            &essential,
            &native,
            &multiset(&[]),
            Some(&multiset(&[])),
            &mut diagnostics
        )
        .unwrap()
        .is_none(),
        "removal of the only pad-to-pad conductor must roll back"
    );
}

#[test]
fn cleanup_trims_at_a_contained_via_projection_but_does_not_guess_grazing_contacts() {
    use crate::test_support_routers::{track as segment, via};
    let mut board = pad_board(
        &[("S", &[(0.0, 0.0), (8.0, 0.0)])],
        2,
        vec![
            segment("S", (3.0, 3.0), (7.0, 3.0), 0.2, CopperLayer::Front),
            segment("S", (6.0, 3.1), (11.0, 3.0), 0.2, CopperLayer::Back),
        ],
    );
    board.placements[1].side = copper_physical::BoardSide::Back;
    board.vias.push(via("S", (6.0, 3.1), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back));
    let mut diagnostics = Vec::new();
    let native = evidence(&board, 0, vec![finding(&board, "track", 0)]);
    let candidate = crate::native_stub_cleanup::native_stub_candidate(
        &board,
        &native,
        &multiset(&[]),
        Some(&multiset(&board.vias)),
        &mut diagnostics,
    )
    .unwrap()
    .expect("contained off-centre via entry");
    assert_eq!(candidate.tracks[0].end, pt(6.0, 3.0));
    assert_eq!(candidate.tracks[1], board.tracks[1]);
    assert_eq!(candidate.vias, board.vias);
    board.vias[0].position = pt(6.0, 3.29);
    board.tracks[1].start = board.vias[0].position;
    let grazing = evidence(&board, 0, vec![finding(&board, "track", 0)]);
    assert!(
        crate::native_stub_cleanup::native_stub_candidate(
            &board,
            &grazing,
            &multiset(&[]),
            Some(&multiset(&board.vias)),
            &mut diagnostics
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn cleanup_keeps_a_verified_width_contact_neck_and_rejects_a_subminimum_overlap() {
    use crate::test_support_routers::{track as segment, via};
    let mut board = pad_board(
        &[("S", &[(0.0, 0.0), (8.0, 0.0)])],
        2,
        vec![
            segment("S", (3.0, 3.0), (7.0, 3.0), 0.2, CopperLayer::Front),
            segment("S", (6.0, 3.3), (6.0, 3.05), 0.2, CopperLayer::Front),
            segment("S", (6.0, 3.3), (11.0, 3.0), 0.2, CopperLayer::Back),
        ],
    );
    board.placements[1].side = copper_physical::BoardSide::Back;
    board.vias.push(via("S", (6.0, 3.3), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back));
    let native = evidence(&board, 0, vec![finding(&board, "track", 0)]);
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::native_stub_candidate(
        &board,
        &native,
        &multiset(&board.tracks[1..]),
        Some(&multiset(&board.vias)),
        &mut diagnostics,
    )
    .unwrap()
    .expect("minimum-width neck retained");
    assert_eq!(candidate.tracks[0].end, pt(6.0, 3.0));
    assert_eq!(&candidate.tracks[1..], &board.tracks[1..]);
    board.tracks[1].end = pt(6.0, 3.19);
    let native = evidence(&board, 0, vec![finding(&board, "track", 0)]);
    assert!(
        crate::native_stub_cleanup::native_stub_candidate(
            &board,
            &native,
            &multiset(&board.tracks[1..]),
            Some(&multiset(&board.vias)),
            &mut diagnostics
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn disposable_cleanup_trims_an_independent_tail_without_retiring_a_protected_via() {
    let mut board = fixture();
    board.tracks.push(track("SIGNAL", (7.0, 11.0), (7.0, 13.0)));
    let native = evidence(&board, 0, vec![finding(&board, "via", 0), finding(&board, "track", 4)]);
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::disposable_stub_candidate(
        &board,
        &native,
        &multiset(&board.tracks[..4]),
        Some(&multiset(&board.vias)),
        &mut diagnostics,
    )
    .unwrap()
    .expect("independent disposable tail");
    assert_eq!(candidate.tracks, board.tracks[..4]);
    assert_eq!(candidate.vias, board.vias);
    let after = evidence(&candidate, 0, vec![finding(&candidate, "via", 0)]);
    let census = crate::native_stub_cleanup::native_cleanup_census(
        &candidate,
        &after,
        &multiset(&board.tracks[..4]),
        &multiset(&board.vias),
        None,
    )
    .unwrap();
    assert_eq!(census.get("fresh_native_binding"), Some(&Value::Bool(true)));
    assert_eq!(census.get("items").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(native_cleanup_trial_outcome(&board, &native, &candidate, &after).unwrap(), "accepted");
    assert_eq!(native_cleanup_trial_outcome(&board, &native, &board, &native).unwrap(), "native_not_improved");
    let open = evidence(&candidate, 1, vec![finding(&candidate, "via", 0)]);
    assert_eq!(
        native_cleanup_trial_outcome(&board, &native, &candidate, &open).unwrap(),
        "native_cleanup_incomplete_nets"
    );
    let mut changed = candidate.clone();
    changed.vias[0].size_nm += mm(0.1);
    let changed_native = evidence(&changed, 0, vec![finding(&changed, "via", 0)]);
    assert_eq!(
        native_cleanup_trial_outcome(&board, &native, &changed, &changed_native).unwrap(),
        "native_new_violations"
    );
}

#[test]
fn disposable_cleanup_keeps_unselected_critical_copper_but_requires_its_exact_native_binding() {
    let mut board = fixture();
    board.tracks.push(track("SIGNAL", (7.0, 11.0), (7.0, 13.0)));
    board.tracks.push(track("CLOCK", (16.0, 5.0), (16.0, 7.0)));
    board.nets.push(copper_pcb::board::PhysicalNet { name: "CLOCK".into(), pads: vec![] });
    board.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        kind: copper_pcb::board::RouteKind::Clock,
        ..crate::routing_acceptance::general_rule("CLOCK")
    });
    let mut native =
        evidence(&board, 0, vec![finding(&board, "via", 0), finding(&board, "track", 4), finding(&board, "track", 5)]);
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::disposable_stub_candidate(
        &board,
        &native,
        &multiset(&board.tracks[..4]),
        Some(&multiset(&board.vias)),
        &mut diagnostics,
    )
    .unwrap()
    .expect("critical finding must not veto an unrelated tail");
    assert_eq!(candidate.tracks.last(), board.tracks.last());
    assert_eq!(candidate.tracks.len(), board.tracks.len() - 1);
    assert_eq!(candidate.vias, board.vias);
    native.native_findings[2].items[0].uuid.push_str("-unbound");
    assert!(
        crate::native_stub_cleanup::disposable_stub_candidate(
            &board,
            &native,
            &multiset(&board.tracks[..4]),
            Some(&multiset(&board.vias)),
            &mut diagnostics,
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn optional_retirement_protects_mandatory_prefix_even_when_caller_guards_are_empty() {
    use crate::fixed_prefix::{AdmittedFixedPrefix, FixedContribution, FixedPrefixRole};
    let board = fixture();
    let mut source = board.clone();
    source.tracks.clear();
    source.vias.clear();
    let prefix = AdmittedFixedPrefix::admit(
        &source,
        &[FixedContribution {
            id: "mandatory".into(),
            role: FixedPrefixRole::ViaLattice,
            tracks: board.tracks.clone(),
            vias: board.vias.clone(),
        }],
    )
    .unwrap();
    let native = evidence(&board, 0, vec![finding(&board, "via", 0), finding(&board, "track", 3)]);
    let result = crate::plane_verify::cleanup_native_dead_ends(
        &board,
        &native,
        &multiset(&[]),
        Some(&multiset(&[])),
        Some(&prefix),
        std::path::Path::new("must-not-run"),
        None,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(!result.changed);
    assert_eq!(result.board, board);
    assert_eq!(result.retained_prefix.unwrap().missing_copper_occurrences(&board), (0, 0));
}

#[test]
fn disposable_cleanup_does_not_exchange_a_dead_end_for_a_length_match_failure() {
    let mut board = pad_board(
        &[("A", &[(3.0, 10.0), (11.0, 10.0)]), ("B", &[(3.0, 17.0), (13.0, 17.0)])],
        2,
        vec![
            track("A", (6.0, 13.0), (14.0, 13.0)),
            track("A", (10.0, 13.0), (10.0, 15.0)),
            track("B", (6.0, 20.0), (16.0, 20.0)),
        ],
    );
    board.match_groups.push(copper_pcb::board::NetMatchGroup {
        id: "matched".into(),
        nets: vec!["A".into(), "B".into()],
        max_skew_nm: mm(1.0),
    });
    board.vias.push(crate::test_support_routers::via(
        "B",
        (6.0, 20.0),
        0.6,
        0.3,
        CopperLayer::Front,
        CopperLayer::Back,
    ));
    let native = evidence(&board, 0, vec![finding(&board, "track", 1), finding(&board, "via", 0)]);
    let mut candidate = board.clone();
    candidate.tracks.remove(1);
    let after = evidence(&candidate, 0, vec![finding(&candidate, "via", 0)]);
    assert_eq!(
        native_cleanup_trial_outcome(&board, &native, &candidate, &after).unwrap(),
        "native_cleanup_budget_regression"
    );
    let mut diagnostics = Vec::new();
    assert!(
        crate::native_stub_cleanup::disposable_stub_candidate(
            &board,
            &native,
            &multiset(&[board.tracks[0].clone(), board.tracks[2].clone()]),
            Some(&multiset(&board.vias)),
            &mut diagnostics,
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn disposable_cleanup_can_trial_a_fully_overlapped_segment_but_keeps_essential_connectors() {
    let mut board = fixture();
    board.tracks.push(track("SIGNAL", (6.0, 11.0), (6.05, 11.0)));
    let native = evidence(&board, 0, vec![finding(&board, "track", 4)]);
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::disposable_stub_candidate(
        &board,
        &native,
        &multiset(&board.tracks[..4]),
        Some(&multiset(&board.vias)),
        &mut diagnostics,
    )
    .unwrap()
    .expect("fully overlapped disposable segment");
    assert_eq!(candidate.tracks, board.tracks[..4]);
    assert_eq!(candidate.vias, board.vias);
    let essential = evidence(&board, 0, vec![finding(&board, "track", 2)]);
    let guards =
        multiset(&[board.tracks[0].clone(), board.tracks[1].clone(), board.tracks[3].clone(), board.tracks[4].clone()]);
    assert!(
        crate::native_stub_cleanup::disposable_stub_candidate(
            &board,
            &essential,
            &guards,
            Some(&multiset(&board.vias)),
            &mut diagnostics,
        )
        .unwrap()
        .is_none(),
        "even a reported dead end cannot authorize breaking the only pad-to-pad conductor"
    );
}

/// Real-board feasibility trial using the same typed checkpoints and native
/// verifier as route-board. It never adopts the candidate or rewrites the
/// incumbent prefix/witnesses. Paths must be supplied explicitly by the caller.
#[test]
#[ignore = "requires a final board checkpoint, matching report and KiCad 10"]
fn native_disposable_cleanup_preserves_other_findings_on_saved_board() {
    use std::path::PathBuf;
    let path = |name: &str| PathBuf::from(std::env::var(name).expect(name));
    let document =
        copper_state::StateDocument::read(&std::fs::read_to_string(path("COPPER_CLEANUP_FINAL")).unwrap()).unwrap();
    let board = copper_pcb::state_codec::board(&document.value).unwrap();
    let report = copper_json::parse(&std::fs::read_to_string(path("COPPER_CLEANUP_REPORT")).unwrap()).unwrap();
    let census = report.get("native_cleanup").unwrap().get("census").unwrap();
    assert_eq!(
        census.get("board_digest").and_then(Value::as_str),
        Some(copper_drc::digest::physical_board_digest(&board).as_str())
    );
    assert_eq!(census.get("fresh_native_binding"), Some(&Value::Bool(true)));
    let directory = path("COPPER_CLEANUP_OUTPUT");
    std::fs::create_dir_all(&directory).unwrap();
    let cli = path("COPPER_KICAD_CLI");
    let before = crate::plane_verify::verify_filled_planes(&board, &cli, 10, Some(&directory.join("before"))).unwrap();
    assert_eq!(before.unconnected_count, 0);
    let objects = native_object_map(&board).unwrap();
    let mut guards = multiset(&board.tracks);
    let via_guards = multiset(&board.vias);
    for row in census.get("items").unwrap().as_array().unwrap() {
        if row.get("caller_protected") != Some(&Value::Bool(false))
            || row.get("kind").and_then(Value::as_str) != Some("track_dangling")
        {
            continue;
        }
        assert_eq!(row.get("bound"), Some(&Value::Bool(true)));
        let object = &objects[row.get("uuid").and_then(Value::as_str).unwrap()];
        assert_eq!(object.kind, "segment");
        let index = object.index.unwrap() as usize;
        assert_eq!(row.get("index"), Some(&Value::from(index as i64)));
        assert_eq!(board.tracks.iter().filter(|track| *track == &board.tracks[index]).count(), 1);
        guards.shift_remove(&board.tracks[index]);
    }
    let mut diagnostics = Vec::new();
    let (candidate, after, accepted) = crate::plane_verify::remove_native_dangling_tracks(
        &board,
        &before,
        &guards,
        Some(&via_guards),
        &cli,
        Some(&directory.join("cleanup")),
        &mut diagnostics,
    )
    .unwrap();
    let result = Value::Object(copper_json::Object::from([
        ("adopted".into(), Value::Bool(false)),
        ("scope".into(), Value::from("Diagnostic only; production still derives ownership from its live pipeline")),
        ("native_before".into(), before.document()),
        ("native_after".into(), after.document()),
        ("accepted_trial".into(), Value::Bool(accepted)),
        ("diagnostics".into(), Value::Array(diagnostics)),
    ]));
    std::fs::write(directory.join("result.json"), copper_json::Dumps::new().indent(2).sort_keys(true).dumps(&result))
        .unwrap();
    assert!(accepted);
    assert_eq!(candidate.vias, board.vias);
    assert!(subtract(&guards, &multiset(&candidate.tracks)).is_empty());
    assert_eq!(after.unconnected_count, 0);
    assert!(after.other_violation_count < before.other_violation_count);
    eprintln!(
        "Native dead ends {} -> {}; vias unchanged; zero missing connections",
        before.other_violation_count, after.other_violation_count
    );
}

/// Real-board optional-prefix feasibility trial; never adopts its candidate.
#[test]
#[ignore = "requires saved final/critical checkpoints and KiCad 10"]
fn native_ordinary_retirement_renews_prefix_and_paths_on_saved_board() {
    use copper_state::{StateDocument, StateValue};
    use std::path::PathBuf;
    let path = |name: &str| PathBuf::from(std::env::var(name).expect(name));
    let read = |name: &str| StateDocument::read(&std::fs::read_to_string(path(name)).unwrap()).unwrap();
    let final_document = read("COPPER_RETIREMENT_FINAL");
    let map_field = |value: &StateValue, name: &str| -> Option<StateValue> {
        if let StateValue::Map { entries, .. } = value {
            entries.iter().find(|(k, _)| matches!(k, StateValue::Str(k) if k == name)).map(|(_, value)| value.clone())
        } else {
            None
        }
    };
    let board_value = map_field(&final_document.value, "board").unwrap_or_else(|| final_document.value.clone());
    let board = copper_pcb::state_codec::board(&board_value).unwrap();
    let prefix_value = map_field(&final_document.value, "retained_prefix").unwrap_or_else(|| {
        map_field(&read("COPPER_RETIREMENT_CRITICAL").value, "fixed_prefix").expect("critical prefix")
    });
    let prefix = crate::state_codec::fixed_prefix_input(&prefix_value).unwrap();
    assert_eq!(prefix.missing_copper_occurrences(&board), (0, 0));
    let directory = path("COPPER_RETIREMENT_OUTPUT");
    std::fs::create_dir_all(&directory).unwrap();
    let cli = path("KICAD_CLI");
    let before = crate::plane_verify::verify_filled_planes(&board, &cli, 10, Some(&directory.join("before"))).unwrap();
    let mut diagnostics = Vec::new();
    let result = crate::plane_verify::cleanup_native_dead_ends(
        &board,
        &before,
        &multiset(&board.tracks),
        Some(&multiset(&board.vias)),
        Some(&prefix),
        &cli,
        Some(&directory.join("cleanup")),
        &mut diagnostics,
    )
    .unwrap();
    let record = Value::Object(copper_json::Object::from([
        ("native_before".into(), before.document()),
        ("native_after".into(), result.verification.document()),
        ("changed".into(), Value::Bool(result.changed)),
        ("dependency_renewal".into(), result.dependency_renewal.clone().unwrap_or(Value::Null)),
        ("diagnostics".into(), Value::Array(diagnostics)),
    ]));
    std::fs::write(directory.join("result.json"), copper_json::Dumps::new().indent(2).sort_keys(true).dumps(&record))
        .unwrap();
    assert!(
        result.changed && result.dependency_renewal.is_some(),
        "must renew ownership and dependencies, not just test deletion"
    );
    assert_eq!(result.verification.unconnected_count, 0);
    assert!(result.verification.other_violation_count < before.other_violation_count);
    let fresh = result.retained_prefix.as_ref().unwrap();
    assert_eq!(fresh.source(), prefix.source());
    assert_eq!(fresh.missing_copper_occurrences(&result.board), (0, 0));
    assert_ne!(prefix.missing_copper_occurrences(&result.board), (0, 0));
    for part in prefix.contributions().iter().filter(|p| p.role == crate::fixed_prefix::FixedPrefixRole::ViaLattice) {
        assert!(fresh.contributions().contains(part));
    }
    let state_path = result.dependency_renewal.as_ref().unwrap().get("state_path").unwrap().as_str().unwrap();
    let saved = StateDocument::read(&std::fs::read_to_string(state_path).unwrap()).unwrap();
    let StateValue::Map { entries, .. } = &saved.value else { panic!("retirement state") };
    let field = |name: &str| &entries.iter().find(|(k, _)| matches!(k, StateValue::Str(k) if k == name)).unwrap().1;
    assert_eq!(copper_pcb::state_codec::board(field("board")).unwrap(), result.board);
    let saved_prefix = crate::state_codec::fixed_prefix_input(field("retained_prefix")).unwrap();
    assert_eq!(saved_prefix.contributions(), fresh.contributions());
    let StateValue::Tuple(witnesses) = field("renewed_witnesses") else { panic!("witnesses") };
    for value in witnesses {
        let witness = crate::bypass_reservations::saved_witness(value).unwrap().unwrap();
        assert_eq!(crate::decoupling_paths::validate_crosslayer_witness(&result.board, &witness, None).unwrap(), "");
    }
    let StateValue::List(feeds) = field("renewed_surface_feeds") else { panic!("surface feeds") };
    let mut decoded = crate::bypass_links::WitnessPaths::new();
    for feed in feeds {
        let link = crate::bypass_reservations::saved_link(&map_field(feed, "link").unwrap()).unwrap();
        let Some(StateValue::Tuple(tracks)) = map_field(feed, "tracks") else { panic!("feed tracks") };
        decoded.insert(
            link,
            tracks.iter().map(copper_pcb::state_codec::track).collect::<std::result::Result<Vec<_>, _>>().unwrap(),
        );
    }
    let nets = decoded.keys().map(|link| link.net.clone()).collect();
    let mut measured = crate::bypass_links::WitnessPaths::new();
    let verified =
        crate::bypass_links::verified_bypass_links(&result.board, &nets, Some(&mut measured), None, &[]).unwrap();
    for (link, tracks) in decoded {
        assert!(verified.contains(&link));
        assert_eq!(measured[&link], tracks);
    }
    eprintln!(
        "Native dead ends {} -> {}; fresh prefix and {} dependency witnesses",
        before.other_violation_count,
        result.verification.other_violation_count,
        witnesses.len()
    );
}

/// Real-board optional-prefix feasibility trial; never adopts its candidate.
#[test]
#[ignore = "requires saved final/ordinary/critical checkpoints and KiCad 10"]
fn native_optional_escape_retirement_feasibility() {
    use crate::decoupling_paths::PathEdge;
    use crate::fixed_prefix::FixedPrefixRole;
    use copper_state::{StateDocument, StateValue};
    use std::path::PathBuf;
    let path = |name: &str| PathBuf::from(std::env::var(name).expect(name));
    let read = |name: &str| StateDocument::read(&std::fs::read_to_string(path(name)).unwrap()).unwrap();
    let final_document = read("COPPER_RETIREMENT_FINAL");
    let board = copper_pcb::state_codec::board(&final_document.value).unwrap();
    let inputs_path = path("COPPER_RETIREMENT_INPUTS");
    let inputs = crate::state_codec::load_inputs(
        &std::fs::read_to_string(&inputs_path).unwrap(),
        &inputs_path.to_string_lossy(),
    )
    .unwrap();
    let critical = read("COPPER_RETIREMENT_CRITICAL");
    let StateValue::Map { entries, .. } = &critical.value else { panic!("critical inputs") };
    let field = |key: &str| {
        entries.iter().rev().find(|(k, _)| matches!(k, StateValue::Str(k) if k == key)).map(|(_, v)| v).unwrap()
    };
    let prefix = crate::state_codec::fixed_prefix_input(field("fixed_prefix")).unwrap();
    let held = crate::bypass_reservations::saved_reservation(field("reserved_accesses")).unwrap().unwrap();
    let (_, _, reason) =
        crate::bypass_reservations::verified_bypass_reservations_in_frame(prefix.source(), &prefix, &held).unwrap();
    assert!(reason.is_none(), "fresh original witness validation: {reason:?}");
    assert_eq!(prefix.missing_copper_occurrences(&board), (0, 0));
    let before_tracks = multiset(&inputs.board.tracks);
    let before_vias = multiset(&inputs.board.vias);
    let (mut guards, mut via_guards) = (before_tracks.clone(), before_vias.clone());
    for contribution in prefix.contributions().iter().filter(|part| part.role == FixedPrefixRole::OrdinaryEscape) {
        for track in &contribution.tracks {
            if board.tracks.iter().filter(|t| *t == track).count() == 1
                && prefix.board().tracks.iter().filter(|t| *t == track).count() == 1
            {
                guards.shift_remove(track);
            }
        }
        for via in &contribution.vias {
            if board.vias.iter().filter(|v| *v == via).count() == 1
                && prefix.board().vias.iter().filter(|v| *v == via).count() == 1
            {
                via_guards.shift_remove(via);
            }
        }
    }
    // A generated local launch may also carry a held power/ground path.
    // Preserve every original path edge, regardless of its producer role.
    for witness in &held.decoupling_witnesses {
        for step in witness.supply_path.steps.iter().chain(&witness.return_path.steps) {
            match &step.edge {
                PathEdge::Track(t) => {
                    guards.entry(t.clone()).or_insert(1);
                }
                PathEdge::Via(v) => {
                    via_guards.entry(v.clone()).or_insert(1);
                }
            }
        }
    }
    let directory = path("COPPER_RETIREMENT_OUTPUT");
    std::fs::create_dir_all(&directory).unwrap();
    let cli = path("KICAD_CLI");
    let native = crate::plane_verify::verify_filled_planes(&board, &cli, 10, Some(&directory.join("before"))).unwrap();
    assert_eq!(native.unconnected_count, 0, "retirement is only considered after native completion");
    let census =
        crate::native_stub_cleanup::native_cleanup_census(&board, &native, &before_tracks, &before_vias, Some(&prefix))
            .unwrap();
    let mut diagnostics = Vec::new();
    let candidate = crate::native_stub_cleanup::native_stub_candidate(
        &board,
        &native,
        &guards,
        Some(&via_guards),
        &mut diagnostics,
    )
    .unwrap();
    let mut report = copper_json::Object::from([
        ("native_before".into(), native.document()),
        ("census".into(), census),
        ("diagnostics".into(), Value::Array(diagnostics)),
        ("adopted".into(), Value::Bool(false)),
        (
            "qualification".into(),
            Value::from(
                "Feasibility trial only; retirement requires a new admitted prefix and freshly rebound dependencies before adoption",
            ),
        ),
    ]);
    if let Some(candidate) = candidate {
        let after = crate::plane_verify::verify_filled_planes(&candidate, &cli, 10, Some(&directory.join("candidate")))
            .unwrap();
        report.insert("native_before".into(), native.document());
        report.insert("native_after".into(), after.document());
        report.insert("removed_tracks".into(), Value::from((board.tracks.len() - candidate.tracks.len()) as i64));
        report.insert("removed_vias".into(), Value::from((board.vias.len() - candidate.vias.len()) as i64));
    }
    let mut groups: IndexMap<String, std::collections::BTreeSet<String>> = IndexMap::new();
    for finding in &native.native_findings {
        if let [item] = finding.items.as_slice()
            && let Some(net) = &item.net
        {
            groups.entry(net.clone()).or_default().insert(item.uuid.clone());
        }
    }
    let mut trials = Vec::new();
    let mut viable = std::collections::BTreeSet::new();
    for (index, (net, selection)) in groups.iter().enumerate() {
        let mut diagnostics = Vec::new();
        let candidate = crate::native_stub_cleanup::native_stub_candidate_for_items(
            &board,
            &native,
            &guards,
            Some(&via_guards),
            Some(selection),
            &mut diagnostics,
        )
        .unwrap();
        let mut row = copper_json::Object::from([
            ("net".into(), Value::from(net.as_str())),
            ("diagnostics".into(), Value::Array(diagnostics)),
        ]);
        if let Some(candidate) = candidate {
            let after = crate::plane_verify::verify_filled_planes(
                &candidate,
                &cli,
                10,
                Some(&directory.join(format!("net-{index:02}"))),
            )
            .unwrap();
            let outcome = native_cleanup_trial_outcome(&board, &native, &candidate, &after).unwrap();
            let safe = outcome == "accepted";
            row.insert("native_guard_outcome".into(), Value::from(outcome));
            row.insert("feasible".into(), Value::Bool(safe));
            row.insert("native_after".into(), after.document());
            if safe {
                viable.extend(selection.iter().cloned());
            }
        }
        eprintln!("retirement trial {net}: {}", copper_json::Dumps::new().dumps(&Value::Object(row.clone())));
        trials.push(Value::Object(row));
    }
    report.insert("per_net_trials".into(), Value::Array(trials));
    if !viable.is_empty() {
        let mut diagnostics = Vec::new();
        let combined = crate::native_stub_cleanup::native_stub_candidate_for_items(
            &board,
            &native,
            &guards,
            Some(&via_guards),
            Some(&viable),
            &mut diagnostics,
        )
        .unwrap();
        report.insert("combined_diagnostics".into(), Value::Array(diagnostics));
        if let Some(candidate) = combined {
            let after =
                crate::plane_verify::verify_filled_planes(&candidate, &cli, 10, Some(&directory.join("combined")))
                    .unwrap();
            report.insert(
                "combined_native_guard_outcome".into(),
                Value::from(native_cleanup_trial_outcome(&board, &native, &candidate, &after).unwrap()),
            );
            report.insert("combined_native_after".into(), after.document());
            report.insert(
                "combined_removed_tracks".into(),
                Value::from((board.tracks.len() - candidate.tracks.len()) as i64),
            );
            report
                .insert("combined_removed_vias".into(), Value::from((board.vias.len() - candidate.vias.len()) as i64));
            let (missing_tracks, missing_vias) = prefix.missing_copper_occurrences(&candidate);
            report.insert("retired_prefix_tracks".into(), Value::from(missing_tracks));
            report.insert("retired_prefix_vias".into(), Value::from(missing_vias));
        }
    }
    let text = copper_json::Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(report));
    std::fs::write(directory.join("feasibility.json"), text).unwrap();
}

fn outcome(
    before: &PhysicalBoard,
    old: &PlaneVerification,
    after: &PhysicalBoard,
    new: &PlaneVerification,
) -> &'static str {
    native_trial_outcome(before, old, after, new).unwrap()
}

#[test]
fn improving_bridge_accepts_unchanged_tracks_and_vias_after_reindexing() {
    let before = fixture();
    let old = evidence(&before, 2, vec![finding(&before, "track", 3), finding(&before, "via", 0)]);
    let mut after = before.clone();
    after.tracks.reverse();
    after.tracks.insert(0, track("D18", (5.0, 3.0), (9.0, 3.0)));
    let mut extra = after.vias[0].clone();
    extra.position = pt(7.0, 11.0);
    after.vias.insert(0, extra);
    let new = evidence(&after, 1, vec![finding(&after, "via", 1), finding(&after, "track", 1)]);
    assert_ne!(old.native_findings[0].items[0].uuid, new.native_findings[1].items[0].uuid);
    assert_ne!(old.native_findings[1].items[0].uuid, new.native_findings[0].items[0].uuid);
    assert_eq!(outcome(&before, &old, &after, &new), "accepted");
}

#[test]
fn moved_resized_or_reassigned_copper_is_a_new_violation_even_with_the_same_uuid() {
    let before = fixture();
    for change in 0..10 {
        let mut after = before.clone();
        match change {
            0 => after.tracks[3].start.x_nm += 1,
            1 => after.tracks[3].width_nm += 1,
            2 => after.tracks[3].net = "D18".to_owned(),
            3 => after.tracks[3].layer = CopperLayer::Back,
            4 => after.vias[0].position.x_nm += 1,
            5 => after.vias[0].size_nm += 1,
            6 => after.vias[0].drill_nm += 1,
            7 => after.vias[0].net = "D18".to_owned(),
            8 => after.vias[0].finish = "tented".to_owned(),
            9 => after.vias[0].technology = Some("other".to_owned()),
            _ => unreachable!(),
        }
        let (kind, index) = if change < 4 { ("track", 3) } else { ("via", 0) };
        let old = evidence(&before, 2, vec![finding(&before, kind, index)]);
        let new = evidence(&after, 1, vec![finding(&after, kind, index)]);
        assert_eq!(old.native_findings[0].items[0].uuid, new.native_findings[0].items[0].uuid);
        assert_eq!(outcome(&before, &old, &after, &new), "native_new_violations", "change {change}");
    }
}

#[test]
fn duplicate_occurrences_and_finding_multiplicity_are_not_collapsed() {
    let before = fixture();
    let old = evidence(&before, 2, vec![finding(&before, "track", 3)]);
    let mut after = before.clone();
    after.tracks.push(before.tracks[3].clone());
    // Even a single finding on the added duplicate cannot borrow the old one's identity.
    let new = evidence(&after, 1, vec![finding(&after, "track", 4)]);
    assert_eq!(outcome(&before, &old, &after, &new), "native_new_violations");
    // Prepending a duplicate must not let it borrow the old rank-1 identity.
    let mut after = before.clone();
    after.tracks.insert(3, before.tracks[3].clone());
    let new = evidence(&after, 1, vec![finding(&after, "track", 3)]);
    assert_eq!(outcome(&before, &old, &after, &new), "native_new_violations");
    let old_via = evidence(&before, 2, vec![finding(&before, "via", 0)]);
    after.vias.insert(0, before.vias[0].clone());
    let new = evidence(&after, 1, vec![finding(&after, "via", 0)]);
    assert_eq!(outcome(&before, &old_via, &after, &new), "native_new_violations");
    let new = evidence(&before, 1, vec![finding(&before, "track", 3), finding(&before, "track", 3)]);
    assert_eq!(outcome(&before, &old, &before, &new), "native_new_violations");
    let mut before = before;
    before.vias.push(before.vias[0].clone());
    let old = evidence(&before, 2, vec![finding(&before, "via", 0)]);
    let new = evidence(&before, 1, vec![finding(&before, "via", 1)]);
    assert_eq!(outcome(&before, &old, &before, &new), "native_new_violations");
}

#[test]
fn unknown_empty_and_inconsistent_bindings_fail_closed() {
    let board = fixture();
    let old = evidence(&board, 2, vec![]);
    for change in 0..4 {
        let mut bad = finding(&board, "track", 3);
        match change {
            0 => bad.items[0].uuid = "unknown".to_owned(),
            1 => bad.items.clear(),
            2 => bad.items[0].index = Some(0),
            3 => bad.items[0].positions_nm[0].0 += 1,
            _ => unreachable!(),
        }
        let new = evidence(&board, 1, vec![bad]);
        assert_eq!(outcome(&board, &old, &board, &new), "native_unsupported_violation");
    }
}

#[test]
fn native_acceptance_preserves_other_guards() {
    let board = fixture();
    let old = evidence(&board, 2, vec![]);
    let mut new = evidence(&board, 1, vec![]);
    new.export_digest.clear();
    assert_eq!(outcome(&board, &old, &board, &new), "native_evidence_mismatch");
    let mut after = board.clone();
    after.placements[0].position.x_nm += 1;
    assert_eq!(outcome(&board, &old, &after, &evidence(&after, 1, vec![])), "native_source_intent_changed");
    let mut new = evidence(&board, 1, vec![]);
    new.island_count = 1;
    assert_eq!(outcome(&board, &old, &board, &new), "native_islands");
    let mut open = finding(&board, "track", 3);
    open.category = "unconnected".to_owned();
    open.kind = "unconnected_items".to_owned();
    let new = evidence(&board, 1, vec![open]);
    assert_eq!(outcome(&board, &old, &board, &new), "native_new_open_nets");
    assert_eq!(outcome(&board, &old, &board, &old), "native_not_improved");
}

#[test]
fn only_library_copy_findings_are_exempt() {
    let board = fixture();
    let old = evidence(&board, 2, vec![]);
    for kind in ["lib_footprint_issues", "lib_footprint_mismatch"] {
        let finding = NativeFinding {
            category: "violation".to_owned(),
            kind: kind.to_owned(),
            description: String::new(),
            items: vec![],
        };
        assert_eq!(outcome(&board, &old, &board, &evidence(&board, 1, vec![finding])), "accepted");
    }
}

#[test]
#[ignore = "requires KiCad 10; set COPPER_KICAD_CLI and run explicitly"]
fn native_kicad_bridge_reduces_opens_despite_reindexed_dangling_copper() {
    let cli = std::env::var_os("COPPER_KICAD_CLI").expect("COPPER_KICAD_CLI");
    let mut before = fixture();
    // A via terminates the original branch, so give KiCad a separate, truly
    // dangling track that remains unchanged when the supply bridge is added.
    before.tracks.push(track("SIGNAL", (8.0, 11.0), (8.0, 13.0)));
    let old = verify_filled_planes(&before, Path::new(&cli), 10, None).unwrap();
    let mut after = before.clone();
    after.tracks.insert(0, track("D18", (5.0, 3.0), (9.0, 3.0)));
    let new = verify_filled_planes(&after, Path::new(&cli), 10, None).unwrap();
    assert!(old.unconnected_count > new.unconnected_count, "{} -> {}", old.unconnected_count, new.unconnected_count);
    assert!(new.native_findings.iter().any(|f| f.kind == "track_dangling"));
    assert_eq!(outcome(&before, &old, &after, &new), "accepted", "{:?}", new.native_findings);
    let uuid_keys = |evidence: &PlaneVerification| {
        evidence
            .native_findings
            .iter()
            .filter(|finding| finding.category == "violation" && finding.kind.ends_with("_dangling"))
            .map(|finding| {
                (finding.kind.clone(), finding.items.iter().map(|item| item.uuid.clone()).collect::<Vec<_>>())
            })
            .collect::<Vec<_>>()
    };
    assert!(
        !subtract(&multiset(&uuid_keys(&new)), &multiset(&uuid_keys(&old))).is_empty(),
        "fixture must reproduce the old UUID comparison failure"
    );
    eprintln!(
        "Fresh KiCad: opens {} -> {}, dangling violations {} -> {}; unchanged reindexed copper accepted",
        old.unconnected_count,
        new.unconnected_count,
        uuid_keys(&old).len(),
        uuid_keys(&new).len()
    );
    // Same report count/UUID does not excuse a changed dangling object.
    after.tracks.last_mut().unwrap().width_nm += mm(0.1);
    let changed = verify_filled_planes(&after, Path::new(&cli), 10, None).unwrap();
    assert_eq!(outcome(&before, &old, &after, &changed), "native_new_violations");
}

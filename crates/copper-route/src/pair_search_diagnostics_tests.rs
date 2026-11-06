use super::*;
use crate::test_support_critical::{MM, mm, track, usb_pair_board};

fn continuation(board: PhysicalBoard, end: Point) -> Continuation {
    let port =
        |center, heading, sign| Port { center, heading, sign, first: Vec::new(), second: Vec::new(), preference: 0 };
    let start = port(mm(10, 5), 0, 1);
    let end = port(end, 4, -1);
    Continuation {
        search: Some(SpineSearch::new(
            "USB_DP",
            "USB_DM",
            &start,
            &end,
            250_000,
            225_000,
            board.rules.minimum_clearance_nm,
            500_000,
        )),
        start,
        end,
        layer: CopperLayer::Front,
        pitch: 500_000,
        search_index: 1,
        registered: true,
        via_pairs: Vec::new(),
        return_vias: Vec::new(),
        active: true,
        resumed: false,
        expanded_states: 0,
        legal_advances: 0,
        minimum_goal_distance_nm: f64::INFINITY,
        goal: PairGoalDiagnostics::default(),
        board,
    }
}

#[test]
fn goal_tail_reports_exact_snapshot_blocker_without_changing_expansion_budget() {
    let mut board = usb_pair_board();
    board.tracks.push(track("USB_DM", pt(11_750_000, 4 * MM), pt(11_750_000, 6 * MM), 250_000, CopperLayer::Front));
    let mut search = continuation(board.clone(), mm(12, 5));
    let mut stats = PairSearchStats::default();
    assert!(search.advance(1, &mut stats).unwrap().is_none());
    assert_eq!(stats.expanded_states, 1);
    assert_eq!(search.goal.stop_reason, "state_slice_exhausted");
    assert_eq!(search.goal.nearest_position, Some(mm(10, 5)));
    assert_eq!(search.goal.nearest_heading, Some(0));
    assert_eq!(search.goal.tail_candidates, search.goal.rejection_counts.iter().map(|(_, count)| count).sum::<i64>());
    let rejected = search
        .goal
        .rejection_examples
        .iter()
        .find(|item| item.reason == "copper_clearance")
        .expect("blocked full spine plus tail");
    assert_eq!(rejected.position, mm(10, 5));
    assert!(rejected.tracks.iter().all(|track| track.net == "USB_DP"));
    assert!(rejected.blockers.iter().any(|blocker| blocker.net.as_deref() == Some("USB_DM")));
    assert!(rejected.blocker_error.is_none());
    let Value::Object(blocker) = blocker_document(&rejected.blockers[0]) else { panic!("blocker") };
    assert_eq!(blocker["immutable"], Value::Bool(true));
    assert_eq!(blocker["immutability_scope"], Value::from("routing_snapshot"));
    assert_eq!(blocker["authored_ownership"], Value::from("not_determined"));
    let before = search.goal.tail_candidates;
    assert!(search.advance(1, &mut stats).unwrap().is_none());
    assert_eq!(stats.expanded_states, 2);
    assert!(search.goal.tail_candidates > before);
    assert!(search.goal.rejection_examples.len() <= 8);
    assert!(search.goal.rejection_examples.iter().all(|example| example.blockers.len() <= 8));
    assert_eq!(search.board, board, "diagnostics must not mutate snapshot copper");
    let closest = search.goal.closest_rejection.as_ref().expect("closest rejected goal candidate").clone();
    assert_eq!(closest.position, pt(10_500_000, 5 * MM));
    assert_eq!(closest.reason, "copper_clearance");
    assert!(closest.blockers.is_empty(), "closer states must not trigger additional blocker queries during search");
    let counts = search.goal.rejection_counts.clone();
    stats.continuations.push(search);
    let attempts = pair_search_attempts(&stats);
    let examples = &attempts[0].goal.rejection_examples;
    // Several tail proposals at the closest frontier can fail differently.
    // The first-per-reason samples may tie its distance; verify the selected
    // closest exact rejection survived, rather than selecting the first tie.
    let nearest = examples
        .iter()
        .find(|example| {
            example.position == closest.position
                && example.reason == closest.reason
                && example.heading == closest.heading
                && example.tracks == closest.tracks
                && example.tail == closest.tail
        })
        .expect("selected closest rejection retained alongside first-per-reason samples");
    assert_eq!(nearest.position, pt(10_500_000, 5 * MM));
    assert_eq!(nearest.distance_to_goal_nm, 1_500_000);
    assert_eq!(nearest.reason, "copper_clearance");
    assert!(nearest.blockers.iter().any(|blocker| blocker.net.as_deref() == Some("USB_DM")));
    assert!(examples.len() <= 8);
    assert_eq!(attempts[0].goal.rejection_counts, counts);
    assert_eq!(stats.expanded_states, 2, "report assembly must not resume a search");
}

#[test]
fn frontier_exhaustion_and_found_candidate_have_distinct_stop_reasons() {
    let mut blocked = usb_pair_board();
    blocked.tracks.push(track("USB_DM", pt(10_250_000, 4 * MM), pt(10_250_000, 6 * MM), 250_000, CopperLayer::Front));
    let mut exhausted = continuation(blocked, mm(15, 5));
    let mut stats = PairSearchStats::default();
    assert!(exhausted.advance(10, &mut stats).unwrap().is_none());
    assert_eq!(stats.expanded_states, 1);
    assert_eq!(exhausted.goal.stop_reason, "frontier_exhausted");
    assert!(!exhausted.active);

    let mut open = continuation(usb_pair_board(), mm(12, 5));
    let found = open.advance(1, &mut PairSearchStats::default()).unwrap().expect("straight candidate");
    assert_eq!(open.goal.stop_reason, "candidate_found");
    assert_eq!(found.expanded_states, 1);
    assert_eq!(found.spine, vec![mm(10, 5), mm(12, 5)]);
    assert_eq!(
        found.first,
        vec![track("USB_DP", pt(10 * MM, 5_225_000), pt(12 * MM, 5_225_000), 250_000, CopperLayer::Front)]
    );
    assert_eq!(
        found.second,
        vec![track("USB_DM", pt(10 * MM, 4_775_000), pt(12 * MM, 4_775_000), 250_000, CopperLayer::Front)]
    );
}

#[test]
fn collar_compatibility_counts_are_retained_when_examples_are_bounded() {
    let mut stats = PairSearchStats::default();
    let first = stats.record_collars("surface", CopperLayer::Front, 500_000, 2, 3, 0).unwrap();
    assert_eq!(stats.collar_inventory[first].stop_reason, "incompatible_lane_order");
    assert_eq!(stats.collar_inventory[first].incompatible_pairs, 6);
    for _ in 0..30 {
        stats.record_collars("transition", CopperLayer::Back, 500_000, 2, 3, 4);
    }
    assert_eq!(stats.collar_inventory.len(), 24);
    assert_eq!(stats.incompatible_port_pairs, 66);
    stats.collar_stop(Some(first), "search_limit");
    let Value::Object(document) = collar_inventory_document(&stats.collar_inventory[first]) else { panic!("record") };
    assert_eq!(document["first_ports"], Value::from(2_i64));
    assert_eq!(document["compatible_pairs"], Value::from(0_i64));
    assert_eq!(document["stop_reason"], Value::from("search_limit"));
}

#[test]
fn bounded_attempt_report_always_includes_closest_frontier() {
    let mut stats = PairSearchStats::default();
    for i in 0..20 {
        let mut item = continuation(usb_pair_board(), mm(12, 5));
        item.search_index = i + 1;
        item.minimum_goal_distance_nm = (20 - i) as f64 * 100_000.0;
        stats.continuations.push(item);
    }
    let attempts = pair_search_attempts(&stats);
    assert_eq!(attempts.len(), 16);
    assert_eq!(attempts[14].search_index, 15);
    assert_eq!(attempts[15].search_index, 20);
    assert_eq!(attempts[15].minimum_goal_distance_nm, Some(100_000));
    assert_eq!(stats.continuations[15].search_index, 16, "reporting must not reorder frontiers");
}

#[test]
fn nearby_135_degree_goal_has_a_legal_family_missing_from_existing_tails() {
    let mut board = usb_pair_board();
    board.rules.minimum_clearance_nm = 150_000;
    board.rules.minimum_track_width_nm = 145_000;
    for rule in &mut board.net_routing_rules {
        rule.width_nm = Some(145_000);
        rule.pair_gap_nm = Some(150_000);
        rule.clearance_nm = Some(150_000);
    }
    let index = RoutingClearanceIndex::new(&board).unwrap();
    // An octilinear near-goal offset, exercised in every cardinal rotation and
    // both turn directions. These are free-space tail fixtures, not a claim
    // that the competing copper in any consumer permits the same connection.
    for rotation in 0..4 {
        for reflected in [false, true] {
            let transform = |mut x: i64, mut y: i64| {
                if reflected {
                    y = -y;
                }
                for _ in 0..rotation {
                    (x, y) = (-y, x);
                }
                pt(20 * MM + x, 10 * MM + y)
            };
            let transform_heading =
                |heading: usize| ((if reflected { (8 - heading) % 8 } else { heading }) + 2 * rotation) % 8;
            let start = transform(0, 0);
            let end = transform(-75_000, -655_000);
            let incoming = transform_heading(5);
            let inward = transform_heading(0);
            let added = goal_turn_paths(start, end, incoming, inward, MM);
            let all = goal_paths(start, end, incoming, inward, MM);
            assert!(!added.is_empty() && added.len() <= 2);
            assert_eq!(&all[all.len() - added.len()..], &added, "existing candidate order is the unchanged prefix");
            let exact = |path: &[Point]| {
                if heading(path[0], path[1]) != Some(incoming)
                    || heading(path[path.len() - 2], path[path.len() - 1]) != Some(inward)
                {
                    return false;
                }
                let Some((a, b)) = lane_paths(path, incoming, inward, 147_500, 1) else { return false };
                legal(
                    &board,
                    &index,
                    &tracks("USB_DP", &a, 145_000, CopperLayer::Front, None),
                    &tracks("USB_DM", &b, 145_000, CopperLayer::Front, None),
                    150_000,
                )
                .unwrap()
            };
            assert!(
                all[..all.len() - added.len()].iter().all(|path| !exact(path)),
                "the regression must demonstrate a missed legal family"
            );
            let expected = vec![
                start,
                transform(-405_000, -405_000),
                transform(-405_000, -530_000),
                transform(-280_000, -655_000),
                end,
            ];
            assert!(added.contains(&expected));
            assert!(exact(&expected), "both lanes must pass exact clearance, including their pair gap");
        }
    }
}

#[test]
fn nearby_135_degree_goal_connects_within_one_existing_expansion() {
    let mut board = usb_pair_board();
    board.rules.minimum_clearance_nm = 150_000;
    board.rules.minimum_track_width_nm = 145_000;
    for rule in &mut board.net_routing_rules {
        rule.width_nm = Some(145_000);
        rule.pair_gap_nm = Some(150_000);
        rule.clearance_nm = Some(150_000);
    }
    let mut search = continuation(board.clone(), pt(9_925_000, 4_345_000));
    search.start.heading = 5;
    search.pitch = MM;
    search.search =
        Some(SpineSearch::new("USB_DP", "USB_DM", &search.start, &search.end, 145_000, 147_500, 150_000, MM));
    let mut stats = PairSearchStats::default();
    let found = search.advance(1, &mut stats).unwrap().expect("new tail joins within the original state budget");
    assert_eq!(stats.expanded_states, 1);
    assert_eq!(search.goal.stop_reason, "candidate_found");
    assert_eq!(
        found.spine,
        vec![
            mm(10, 5),
            pt(9_595_000, 4_595_000),
            pt(9_595_000, 4_470_000),
            pt(9_720_000, 4_345_000),
            pt(9_925_000, 4_345_000)
        ]
    );
    let index = RoutingClearanceIndex::new(&board).unwrap();
    assert!(legal(&board, &index, &found.first, &found.second, 150_000).unwrap());
    let mut routed = board.clone();
    routed.tracks.extend(found.first.iter().chain(&found.second).cloned());
    assert!(crate::test_support_critical::free_of(&routed, &["DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH"]));
    assert!(search.via_pairs.is_empty() && search.return_vias.is_empty());
    assert_eq!(search.board, board, "tail construction must not edit existing copper or macro intent");
}

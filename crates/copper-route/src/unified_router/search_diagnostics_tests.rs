use std::time::{Duration, Instant};

use super::UnifiedRouterOptions;
use super::closure::ClosureSide;
use super::route::{adopt_route_diagnostics, route_diagnostics, route_unified};
use super::router::{OpenReachStop, SearchFailureReason};
use super::search::Targets;
use crate::test_support_routers::*;
use copper_physical::CopperLayer;
use indexmap::{IndexMap, IndexSet};

fn options() -> UnifiedRouterOptions {
    UnifiedRouterOptions {
        maximum_iterations: 1,
        commit_rounds: 1,
        fine_regions: "off".to_owned(),
        ..Default::default()
    }
}

#[test]
fn missing_group_access_names_source_or_target_and_exact_pads() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    for (group, expected) in
        [(0, SearchFailureReason::MissingSourceAccess), (1, SearchFailureReason::MissingTargetAccess)]
    {
        let mut router = router_all(&board, &options());
        let pads = router.specs[0].component_pads[group].clone();
        let locations = router.specs[0].anchors[group].clone();
        router.specs[0].components[group].clear();
        let route = router.route_net(0, 1.0).unwrap();
        assert!(!route.connected && !route.search_budget_exhausted());
        let failure = &route.required_search_failures[0];
        assert_eq!(failure.reason, expected);
        assert_eq!((failure.expanded, failure.searches), (0, 0));
        assert_eq!(failure.groups[0].access_count, 0);
        assert_eq!(failure.groups[0].pads, pads);
        assert_eq!(failure.groups[0].locations, locations);
        assert!(failure.diagnostic().contains(&format!("remaining_pads=[A_{group}.1]")));
    }
}

#[test]
fn expansion_failure_stays_on_failed_route_and_success_clears_it() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &UnifiedRouterOptions { maximum_expansions: 1, ..options() });
    let failed = router.route_net(0, 1.0).unwrap();
    assert!(!failed.connected && failed.search_budget_exhausted());
    let failure = &failed.required_search_failures[0];
    assert_eq!(failure.reason, SearchFailureReason::ExpansionLimit);
    assert_eq!(failure.expanded, failure.searches as i64);
    assert!(failure.source_access_count > 0 && failure.target_access_count > 0);
    assert_eq!(failure.groups.iter().map(|group| group.index).collect::<Vec<_>>(), [1]);
    router.options.maximum_expansions = 400_000;
    let succeeded = router.route_net(0, 1.0).unwrap();
    assert!(succeeded.connected && succeeded.required_search_failures.is_empty());
    assert!(!succeeded.search_budget_exhausted());
    // A metadata-only difference must restore the selected evidence while
    // leaving occupancy unchanged.
    router.routes.insert("A".to_owned(), failed.clone());
    router.occupancy.add(&failed, 1);
    let selected = router.routes.clone();
    router.routes["A"].required_search_failures[0].deadline_reached = true;
    let occupancy = (router.occupancy.occ.clone(), router.occupancy.via_occ.clone());
    router.restore_routes(selected);
    assert_eq!(router.routes["A"], failed);
    assert!(router.routes["A"].search_budget_exhausted());
    assert_eq!((router.occupancy.occ.clone(), router.occupancy.via_occ.clone()), occupancy);
    router.restore_routes(IndexMap::from([("A".to_owned(), succeeded)]));
    assert!(!router.routes["A"].search_budget_exhausted());
}

#[test]
fn deadline_skipping_retry_keeps_actual_kernel_stop_and_expansion_count() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &UnifiedRouterOptions { maximum_expansions: 1, ..options() });
    let start = xy(&router, 6.0, 13.0);
    let end = xy(&router, 20.0, 13.0);
    let first = router.search_with_outcome(0, &[(start, 0.0)], Targets { nodes: &[end], extra: None }, 1.0).unwrap();
    assert_eq!(first.failure, Some(SearchFailureReason::ExpansionLimit));
    let counts = (first.expanded, first.searches);
    router.deadline = Some(Instant::now() - Duration::from_secs(1));
    let skipped = router.search_with_outcome(0, &[(start, 0.0)], Targets { nodes: &[end], extra: None }, 1.0).unwrap();
    assert_eq!(skipped.failure, Some(SearchFailureReason::Deadline));
    let unresolved = skipped.following(first);
    assert_eq!(unresolved.failure, Some(SearchFailureReason::ExpansionLimit));
    assert_eq!((unresolved.expanded, unresolved.searches), counts);
    assert!(unresolved.deadline_reached && unresolved.path.is_none());
}

#[test]
fn selected_pour_diagnostics_survive_rejected_explicit_trial() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &UnifiedRouterOptions { maximum_expansions: 1, ..options() });
    let partial = router.route_net(0, 1.0).unwrap();
    let first = route_diagnostics(&router, &IndexMap::from([("A".to_owned(), partial)]), &IndexMap::new());
    assert!(first["A"].search_budget_exhausted);
    let mut selected = IndexMap::new();
    adopt_route_diagnostics(&mut selected, first.clone(), true);
    router.specs[0].components[1].clear();
    let explicit = router.route_net(0, 1.0).unwrap();
    let second = route_diagnostics(&router, &IndexMap::from([("A".to_owned(), explicit)]), &IndexMap::new());
    assert!(!second["A"].search_budget_exhausted);
    adopt_route_diagnostics(&mut selected, second.clone(), false);
    assert_eq!(selected, first, "a rejected explicit trial cannot overwrite accepted plane evidence");
    adopt_route_diagnostics(&mut selected, second.clone(), true);
    assert_eq!(selected, second, "a kept later trial must replace stale evidence");
}

#[test]
fn wider_window_success_discards_earlier_frontier_failure() {
    let wall = [CopperLayer::Front, CopperLayer::Back]
        .iter()
        .map(|&layer| track("BLOCK", (13.0, 11.0), (13.0, 15.0), 0.3, layer))
        .collect();
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("BLOCK", &[(10.0, 20.0)])], 2, wall);
    let opts = UnifiedRouterOptions { window_margin_nm: 0, ..options() };
    let mut router = router(&board, &opts, &[&board.nets[0]]);
    let start = xy(&router, 6.0, 13.0);
    let end = xy(&router, 20.0, 13.0);
    let found = router.search_with_outcome(0, &[(start, 0.0)], Targets { nodes: &[end], extra: None }, 1.0).unwrap();
    assert!(found.path.is_some() && found.failure.is_none());
    assert_eq!(found.searches, 3, "two narrow failures, then whole-board success");
    let route = router.route_net(0, 1.0).unwrap();
    assert!(route.connected && route.required_search_failures.is_empty());
}

/// Four tracks of another net on `layers` around `centre`, `half` mm to a side.
fn sealed_box(centre: (f64, f64), half: f64, layers: &[CopperLayer]) -> Vec<copper_pcb::board::TrackSegment> {
    let corners = [
        (centre.0 - half, centre.1 - half),
        (centre.0 + half, centre.1 - half),
        (centre.0 + half, centre.1 + half),
        (centre.0 - half, centre.1 + half),
    ];
    layers
        .iter()
        .flat_map(|&layer| (0..4).map(move |i| track("BLOCK", corners[i], corners[(i + 1) % 4], 0.3, layer)))
        .collect()
}

const BOTH: [CopperLayer; 2] = [CopperLayer::Front, CopperLayer::Back];

/// Board with net A from (6, 13) to a pad at (20, 13) in router coordinates.
fn closure_board(wall: Vec<copper_pcb::board::TrackSegment>) -> copper_pcb::board::PhysicalBoard {
    pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("BLOCK", &[(10.0, 20.0)])], 2, wall)
}

#[test]
fn closure_check_rejects_a_sealed_pocket_before_the_larger_windows() {
    let board = closure_board(sealed_box((20.0, 13.0), 1.0, &BOTH));
    let mut router = router(&board, &options(), &[&board.nets[0]]);
    let (start, end) = (xy(&router, 6.0, 13.0), xy(&router, 20.0, 13.0));
    let find = |router: &mut super::router::Router<'_, '_>| {
        router.search_with_outcome(0, &[(start, 0.0)], Targets { nodes: &[end], extra: None }, 1.0).unwrap()
    };
    // A net never seen to fail runs its first window, then the check ends the search.
    let first = find(&mut router);
    assert_eq!(first.failure, Some(SearchFailureReason::FrontierExhausted));
    assert_eq!(first.closure, Some(ClosureSide::Target));
    assert_eq!(first.searches, 1, "only the first window ran");
    assert!(first.expanded > 0);
    // The next search of that net is rejected before any A* work.
    let again = find(&mut router);
    assert_eq!(again.failure, Some(SearchFailureReason::FrontierExhausted));
    assert_eq!((again.closure, again.searches, again.expanded), (Some(ClosureSide::Target), 0, 0));
    // The failure record keeps the reason and the nodes, and says how it ended.
    let route = router.route_net(0, 1.0).unwrap();
    let failure = &route.required_search_failures[0];
    assert_eq!(failure.reason, SearchFailureReason::FrontierExhausted);
    assert!(
        failure.diagnostic().contains("expanded=0 searches=0")
            && failure.diagnostic().contains("; closure_check=target")
    );
    assert!(!failure.source_nodes.is_empty() && !failure.target_nodes.is_empty());
    let routes = IndexMap::from([("A".to_owned(), route)]);
    let text = route_diagnostics(&router, &routes, &IndexMap::new())["A"].search[0].clone();
    assert!(text.contains("; closure_check=target; walled_in=target;"), "{text}");
}

#[test]
fn closure_check_never_rejects_what_the_search_can_reach() {
    fn flagged(board: &copper_pcb::board::PhysicalBoard) -> super::router::Router<'_, '_> {
        let mut router = router(board, &options(), &[&board.nets[0]]);
        router.exhausted_nets.insert(0);
        router
    }
    let search = |router: &mut super::router::Router<'_, '_>| {
        let (start, end) = (xy(router, 6.0, 13.0), xy(router, 20.0, 13.0));
        router.search_with_outcome(0, &[(start, 0.0)], Targets { nodes: &[end], extra: None }, 1.0).unwrap()
    };
    // Nothing in the way.
    let board = closure_board(vec![]);
    let mut open = flagged(&board);
    let found = search(&mut open);
    assert!(found.path.is_some() && found.closure.is_none() && found.searches >= 1);

    // Only a via gets in: the box walls the front layer, the back layer is open.
    let board = closure_board(sealed_box((20.0, 13.0), 2.0, &[CopperLayer::Front]));
    let mut router = flagged(&board);
    let found = search(&mut router);
    let (path, _) = found.path.expect("reached through a via");
    let layer_of = |node: i64| crate::routing_lattice::Lattice::layer_of(&router.lattice, node);
    assert!(path.iter().any(|&n| layer_of(n) != layer_of(path[0])), "the route changes layer");
    assert!(found.closure.is_none());

    // Only a neck gets in: the wall zone is foreign copper, but the net's neck
    // may step onto it from its region. A strict-owner flood would close.
    let board = closure_board(sealed_box((20.0, 13.0), 1.0, &BOTH));
    let mut router = flagged(&board);
    let (width, clearance) = (router.specs[0].width, router.specs[0].clearance);
    let nid = router.specs[0].nid;
    let owner = router.rasters.track(width, clearance).0.to_vec();
    let mut strip = IndexSet::new();
    let mut x = 15.0;
    while x <= 21.0 {
        strip.insert(xy(&router, x, 13.0));
        x += 0.05;
    }
    let neck: IndexSet<i64> =
        strip.iter().copied().filter(|&n| owner[n as usize] != -1 && owner[n as usize] != nid).collect();
    assert!(neck.len() >= 2, "the strip crosses the wall zone");
    let (start, end) = (xy(&router, 6.0, 13.0), xy(&router, 20.0, 13.0));
    router.specs[0].region = strip;
    router.specs[0].neck = neck;
    let (closed, _) = router.closed_connection(0, &[start], &[end], 512);
    assert_eq!(closed, None, "the neck lets the kernel in");
    let found = search(&mut router);
    assert!(found.path.is_some() && found.closure.is_none(), "and it does get in");
    router.specs[0].neck.clear();
    let (closed, _) = router.closed_connection(0, &[start], &[end], 512);
    assert_eq!(closed, Some(ClosureSide::Target), "without the neck the pocket is closed");
    assert!(search(&mut router).path.is_none());
}

#[test]
fn final_diagnostic_names_the_copper_that_walls_in_an_exhausted_connection() {
    let board = pad_board(
        &[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("BLOCK", &[(10.0, 20.0)])],
        2,
        sealed_box((20.0, 13.0), 1.0, &BOTH),
    );
    let mut router = router(&board, &options(), &[&board.nets[0]]);
    let route = router.route_net(0, 1.0).unwrap();
    assert!(!route.connected);
    let failure = &route.required_search_failures[0];
    assert_eq!(failure.reason, SearchFailureReason::FrontierExhausted);
    // The search itself carries no wall text; only the final diagnostics do.
    assert!(!failure.diagnostic().contains("bounded_by"));
    let routes = IndexMap::from([("A".to_owned(), route)]);
    let text = route_diagnostics(&router, &routes, &IndexMap::new())["A"].search[0].clone();
    assert!(text.contains("remaining_pads=[A_1.1]"), "{text}");
    assert!(text.contains("; walled_in=target; reached_nodes="), "{text}");
    // The box is BLOCK copper on both layers; the pad's own via keep-out also
    // keeps the search off the second layer, which the summary counts apart.
    let wall = text.split_once("; bounded_by=[").unwrap().1;
    assert!(
        wall.contains("BLOCK:") && wall.contains("; via_blocked_columns=") && !wall.contains("board_edge"),
        "{text}"
    );
    // The existing fields keep their order and the new ones come last.
    let (head, tail) = text.split_once("; walled_in=").unwrap();
    assert_eq!(head, format!("unified: {}", routes["A"].required_search_failures[0].diagnostic()));
    assert!(!tail.contains("remaining_pads"));
}

#[test]
fn wall_shares_are_ranked_capped_and_rounded() {
    use super::walls::{WallReport, WallSide, rank_shares};
    let weights = IndexMap::from([
        ("GND".to_owned(), 20.0),
        ("RESET".to_owned(), 41.0),
        ("board_edge".to_owned(), 10.0),
        ("SWDCLK".to_owned(), 20.0),
        ("keepout_or_netless_copper".to_owned(), 5.0),
        ("VDD".to_owned(), 3.0),
        ("NRST".to_owned(), 1.0),
    ]);
    let shares = rank_shares(weights);
    let order: Vec<&str> = shares.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        order,
        ["RESET", "GND", "SWDCLK", "board_edge", "keepout_or_netless_copper", "VDD", "NRST"],
        "ties break by name"
    );
    let report = WallReport {
        side: WallSide::Source,
        reached: 12,
        boundary: 90,
        shares,
        via_columns: 7,
        layers: Some((2, 4)),
        bounds_nm: (1, 2, 3, 4),
    };
    assert_eq!(
        report.describe(),
        "; walled_in=source; reached_nodes=12; region_nm=(1,2)-(3,4); boundary_nodes=90; \
         bounded_by=[RESET:41%,GND:20%,SWDCLK:20%,board_edge:10%,keepout_or_netless_copper:5%,+2 more]; via_blocked_columns=7; layers_allowed=2/4"
    );
}

#[test]
fn netless_obstacles_are_told_apart_by_what_the_obstacle_records() {
    use super::rasters::{FOREIGN, Obstacle, WallCause};
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let obstacle = |owner, is_pad, at| Obstacle {
        owner,
        layers: vec![0],
        spine: vec![at],
        radius: 100_000,
        clearance: 0,
        is_pad,
        blocks_tracks: true,
        blocks_vias: true,
        polygon: false,
    };
    let (pad, region, net) = (pt(10.0, 10.0), pt(20.0, 20.0), pt(30.0, 30.0));
    router.rasters.obstacles =
        vec![obstacle(FOREIGN, true, pad), obstacle(FOREIGN, false, region), obstacle(3, true, net)];
    let causes = |at: copper_physical::Point| {
        router.rasters.foreign_causes(at.x_nm as f64, at.y_nm as f64, 0, 125_000.0, 150_000)
    };
    assert_eq!(causes(pad), [WallCause::UnconnectedPad]);
    assert_eq!(causes(region), [WallCause::NetlessObstacle]);
    assert_eq!(causes(net), [WallCause::Net(3)]);
    assert!(causes(pt(40.0, 5.0)).is_empty(), "only the outline reaches a point near no obstacle");
}

#[test]
fn optional_plane_failure_does_not_mark_required_connection_budget() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &UnifiedRouterOptions { maximum_expansions: 1, ..options() });
    let start = xy(&router, 6.0, 13.0);
    let other = xy(&router, 20.0, 13.0);
    let spec = &mut router.specs[0];
    spec.components = vec![vec![(start, Vec::new(), 0.0)], vec![(other, Vec::new(), 0.0)]];
    spec.plane = IndexSet::from([start]);
    spec.optional = 1;
    spec.islands = 1;
    let route = router.route_net(0, 1.0).unwrap();
    assert!(route.connected && route.required_search_failures.is_empty());
    assert!(!route.search_budget_exhausted());
    // The same failed contact is required in this variant.
    router.specs[0].optional = 0;
    let required = router.route_net(0, 1.0).unwrap();
    assert!(!required.connected && required.search_budget_exhausted());
    assert_eq!(required.required_search_failures.len(), 1);
    assert_eq!(required.required_search_failures[0].groups[0].index, 1);
    // A target excluded by the required layer contract is not a search cap.
    router.specs[0].optional = 1;
    router.specs[0].explicit.insert(0, IndexSet::from([0, 1]));
    let no_target = router.route_net(0, 1.0).unwrap();
    assert!(!no_target.connected && !no_target.search_budget_exhausted());
    assert_eq!(no_target.required_search_failures[0].reason, SearchFailureReason::MissingTargetAccess);
}

#[test]
fn expired_outer_deadline_has_no_kernel_expansion_failure_and_preserves_incumbent() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let before = Instant::now() - Duration::from_secs(1);
    let (routes, _, passes) = router.negotiate(Some(before), None, &mut Vec::new()).unwrap();
    assert_eq!(passes, 0);
    let failure = &routes["A"].required_search_failures[0];
    assert_eq!(failure.reason, SearchFailureReason::Deadline);
    assert_eq!((failure.expanded, failure.searches), (0, 0));
    assert!(!routes["A"].search_budget_exhausted());
    router.deadline = None;
    let incumbent = router.route_net(0, 1.0).unwrap();
    assert!(incumbent.connected);
    router.routes.insert("A".to_owned(), incumbent.clone());
    let (routes, _, _) = router.negotiate(Some(before), None, &mut Vec::new()).unwrap();
    assert_eq!(routes["A"], incumbent);
}

#[test]
fn detailed_result_reports_selected_search_limit_and_pad_focus() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let guides = routes::global(&board, 2.0);
    let opts = UnifiedRouterOptions { maximum_expansions: 1, ..options() };
    let result = route_unified(&board, &guides, &opts, None, None, None, None, None).unwrap();
    let net = &result.nets[0];
    assert!(!net.connected && net.search_budget_exhausted);
    let diagnostic = net.diagnostics.iter().find(|text| text.contains("search expansion limit exhausted")).unwrap();
    assert!(diagnostic.contains("remaining_pads=[A_1.1]") && diagnostic.contains("expanded="));
    assert!(net.diagnostics.iter().any(|text| text.contains("access_preparation group=1 width_nm=")));
    assert!(
        net.diagnostics.iter().any(|text| text.contains("candidates_chunk=0") && text.contains("nominal_owner_ok="))
    );
    assert!(net.diagnostics.iter().any(|text| text.contains("open_probe=") && text.contains("global_spent_before=")));
    assert!(net.diagnostics.len() <= 16 && net.diagnostics.iter().all(|text| text.len() < 2048));
    let good = route_unified(&board, &guides, &options(), None, None, None, None, None).unwrap();
    assert!(good.nets[0].connected && !good.nets[0].search_budget_exhausted);
}

#[test]
fn open_probe_distinguishes_real_open_region_from_optimistic_flood_cutoff() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let spec = router.specs[0].clone();
    let key = (
        if spec.neck_width != 0 { spec.neck_width } else { spec.width },
        if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance },
    );
    let node = xy(&router, 10.0, 10.0);
    router.open_cache.insert(key, vec![true; router.lattice.size as usize]);
    let absent = router.reaches_open(&[], &spec, Some(10));
    assert_eq!(absent.stop, OpenReachStop::NoSources);
    assert!(!absent.reaches_open());
    let open = router.reaches_open(&[node], &spec, Some(10));
    assert_eq!((open.stop, open.visited), (OpenReachStop::OpenRegion, 1));
    assert!(open.reaches_open());
    router.open_cache.insert(key, vec![false; router.lattice.size as usize]);
    let cutoff = router.reaches_open(&[node], &spec, Some(1));
    assert_eq!((cutoff.stop, cutoff.visited, cutoff.limit), (OpenReachStop::FloodLimit, 1, 1));
    assert!(cutoff.reaches_open(), "preserve historical optimistic cutoff decision");

    let mut blocked = board.clone();
    blocked.copper_keepouts.push(copper_keepout(
        "closed",
        &[CopperLayer::Front, CopperLayer::Back],
        rect(-10.0, -10.0, 40.0, 40.0),
    ));
    let mut router = router_all(&blocked, &options());
    let spec = router.specs[0].clone();
    let closed = router.reaches_open(&[0], &spec, Some(10));
    assert_eq!((closed.stop, closed.visited), (OpenReachStop::FrontierExhausted, 1));
    assert!(!closed.reaches_open());
}

#[test]
fn local_escape_reports_shared_allowance_and_actual_attempts_without_extra_spend() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let spec = router.specs[0].clone();
    let before = router.local_spent;
    router.options.local_escape_limit = before;
    let mut seen = IndexMap::new();
    let skipped = router.attach_local(pt(6.0, 13.0), 0, &spec, &mut seen, None).unwrap();
    assert_eq!((skipped.stop, skipped.tries, skipped.found), ("global_allowance_exhausted", 0, 0));
    assert_eq!(router.local_spent, before);
    assert!(seen.is_empty());

    router.options.local_escape_limit = before + 1;
    let key = (
        if spec.neck_width != 0 { spec.neck_width } else { spec.width },
        if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance },
    );
    let target = xy(&router, 7.0, 13.0);
    let mut open = vec![false; router.lattice.size as usize];
    open[target as usize] = true;
    router.open_cache.insert(key, open);
    let tried = router.attach_local(pt(6.0, 13.0), 0, &spec, &mut seen, None).unwrap();
    assert_eq!((tried.tries, tried.found), (1, 1));
    assert_eq!(tried.attempts.len(), 1);
    assert_eq!(tried.attempts[0].1.outcome, "direct");
    assert_eq!(tried.attempts[0].1.expanded_states, 0);
    assert_eq!(router.local_spent, before + 1);
    assert!(seen.contains_key(&target));
}

#[test]
fn biased_order_searches_in_radius_endpoint_after_outside_radius_candidates() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let spec = router.specs[0].clone();
    router.options.local_escape_radius_nm = mm(2.0);
    router.options.local_escape_limit = router.local_spent + 2;
    let key = (
        if spec.neck_width != 0 { spec.neck_width } else { spec.width },
        if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance },
    );
    let target = xy(&router, 8.5, 10.0);
    let mut open = vec![false; router.lattice.size as usize];
    open[target as usize] = true;
    router.open_cache.insert(key, open);
    let before = router.local_spent;
    let mut seen = IndexMap::new();
    let biased = router.attach_local(pt(10.0, 10.0), 0, &spec, &mut seen, Some((1, 0))).unwrap();
    assert_eq!((biased.stop, biased.tries, biased.found), ("candidate_list_exhausted", 1, 1));
    assert_eq!(biased.remaining_in_radius_eligible, Some(0));
    assert_eq!(biased.attempts[0].0, pt(8.5, 10.0));
    assert!(seen.contains_key(&target), "the eligible endpoint behind the package must be searched");
    assert_eq!(router.local_spent, before + 1);
    let biased_copper = seen.clone();
    seen.clear();
    // With one eligible in-radius endpoint, direction preference must not alter
    // admission or the exact legal path that the unweighted control finds.
    let plain = router.attach_local(pt(10.0, 10.0), 0, &spec, &mut seen, None).unwrap();
    assert_eq!((plain.tries, plain.found), (1, 1));
    assert_eq!(plain.remaining_in_radius_eligible, Some(0));
    assert!(seen.contains_key(&target));
    assert_eq!(seen, biased_copper);
}

#[test]
fn local_radius_filter_preserves_biased_order_found_budget_and_inclusive_reach() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let spec = router.specs[0].clone();
    router.options.local_escape_radius_nm = mm(2.0);
    router.options.local_escape_limit = router.local_spent + 2;
    let key = (
        if spec.neck_width != 0 { spec.neck_width } else { spec.width },
        if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance },
    );
    // Biased distances are 0.5, 1, 1.5, 2 and 2 mm. The final corner is
    // inside the square query window but outside the physical 2 mm radius.
    let targets = [(10.5, 10.0), (9.5, 10.0), (11.5, 10.0), (9.0, 10.0), (12.0, 10.0), (12.0, 12.0)];
    let mut open = vec![false; router.lattice.size as usize];
    for &(x, y) in &targets {
        open[xy(&router, x, y) as usize] = true;
    }
    router.open_cache.insert(key, open);
    let mut seen = IndexMap::new();
    let outcome = router.attach_local(pt(10.0, 10.0), 0, &spec, &mut seen, Some((1, 0))).unwrap();
    assert_eq!((outcome.stop, outcome.tries, outcome.found), ("found_limit", 3, 3));
    assert_eq!(
        outcome.attempts.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
        vec![pt(10.5, 10.0), pt(9.5, 10.0), pt(11.5, 10.0)]
    );
    assert_eq!(outcome.remaining_in_radius_eligible, Some(2), "outside-radius corner is not eligible");

    let boundary = xy(&router, 12.0, 10.0);
    let outside = xy(&router, 12.0, 12.0);
    let mut open = vec![false; router.lattice.size as usize];
    open[boundary as usize] = true;
    open[outside as usize] = true;
    router.open_cache.insert(key, open);
    seen.clear();
    let boundary_only = router.attach_local(pt(10.0, 10.0), 0, &spec, &mut seen, Some((1, 0))).unwrap();
    assert_eq!((boundary_only.tries, boundary_only.found), (1, 1));
    assert_eq!(boundary_only.attempts[0].0, pt(12.0, 10.0));
    assert_eq!(boundary_only.remaining_in_radius_eligible, Some(0));
    assert!(seen.contains_key(&boundary) && !seen.contains_key(&outside));
}

#[test]
fn access_preparation_is_bounded_for_an_existing_large_connected_group() {
    let points: Vec<(f64, f64)> = (0..30).map(|i| (1.0 + i as f64 * 0.5, 10.0)).collect();
    let board = pad_board(&[("A", &points)], 2, vec![track("A", (4.0, 13.0), (18.5, 13.0), 0.2, CopperLayer::Front)]);
    let router = router_all(&board, &options());
    assert_eq!(router.specs[0].components.len(), 1);
    let records = &router.specs[0].component_access_diagnostics[0];
    assert!(records[0].contains("shown=12") && !records[0].contains("omitted_events=0"));
    assert_eq!(records.iter().filter(|record| record.starts_with("anchor=")).count(), 4);
    assert!(records.len() <= 7 && records.iter().all(|record| record.len() < 2048));
}

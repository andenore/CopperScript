use indexmap::{IndexMap, IndexSet};

use super::UnifiedRouterOptions;
use super::router::{NetRoute, SearchFailureReason};
use crate::test_support_routers::*;
use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_physical::CopperLayer;
use std::time::{Duration, Instant};

fn options() -> UnifiedRouterOptions {
    UnifiedRouterOptions {
        maximum_iterations: 1,
        commit_rounds: 1,
        fine_regions: "off".to_owned(),
        ..Default::default()
    }
}

#[test]
fn partial_forest_keeps_other_joins_when_one_group_has_no_access() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (11.0, 6.0), (15.0, 10.0), (11.0, 14.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    router.specs[0].components[0].clear();
    let route = router.route_net(0, 1.0).unwrap();
    assert!(!route.connected);
    assert_eq!(route.paths.len(), 2);
    assert_eq!(route.required_search_failures.len(), 1);
    assert_eq!(route.required_search_failures[0].reason, SearchFailureReason::MissingSourceAccess);
    let (_, rejected, complete) = router.commit(&IndexMap::from([("A".to_owned(), route)]), &IndexSet::new()).unwrap();
    assert!(rejected.is_empty() && complete.is_empty());
}

#[test]
fn partial_forest_does_not_restart_after_an_expansion_limit_or_deadline() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (11.0, 6.0), (15.0, 10.0), (11.0, 14.0)])], 2, vec![]);
    for expired in [false, true] {
        let mut router = router_all(&board, &UnifiedRouterOptions { maximum_expansions: 1, ..options() });
        if expired {
            router.deadline = Some(Instant::now() - Duration::from_secs(1));
        }
        let route = router.route_net(0, 1.0).unwrap();
        assert!(!route.connected && route.paths.is_empty());
        assert_eq!(route.required_search_failures.len(), 1);
        let failure = &route.required_search_failures[0];
        assert_eq!(
            failure.reason,
            if expired { SearchFailureReason::Deadline } else { SearchFailureReason::ExpansionLimit }
        );
        assert_eq!(failure.source_groups[0].pads, [PadReference::new("A_0", "1")]);
        assert!(failure.diagnostic().contains("source_pads=[A_0.1]"));
        assert!(failure.searches <= 3);
    }
}

#[test]
fn partial_forest_copper_participates_in_negotiated_conflicts() {
    let board = pad_board(&[("A", &[(2.0, 10.0), (18.0, 10.0)]), ("B", &[(10.0, 2.0), (10.0, 18.0)])], 2, vec![]);
    let mut router = router_all(&board, &options());
    let paths = [
        ("A", vec![xy(&router, 5.0, 13.0), xy(&router, 21.0, 13.0)], false),
        ("B", vec![xy(&router, 13.0, 5.0), xy(&router, 13.0, 21.0)], true),
    ];
    let routes = paths
        .into_iter()
        .map(|(name, path, connected)| {
            (name.to_owned(), NetRoute { paths: vec![path], leads: vec![Vec::new()], ..NetRoute::empty(connected, "") })
        })
        .collect();
    assert_eq!(router.conflicts(&routes).unwrap(), IndexSet::from(["A".to_owned(), "B".to_owned()]));
}

fn boxed_board(points: &[(f64, f64)]) -> PhysicalBoard {
    let mut walls = Vec::new();
    for layer in [CopperLayer::Front, CopperLayer::Back] {
        for (start, end) in [
            ((5.0, 12.0), (7.0, 12.0)),
            ((7.0, 12.0), (7.0, 14.0)),
            ((7.0, 14.0), (5.0, 14.0)),
            ((5.0, 14.0), (5.0, 12.0)),
        ] {
            walls.push(track("BLOCK", start, end, 0.3, layer));
        }
    }
    pad_board(&[("A", points), ("BLOCK", &[(20.0, 20.0)])], 2, walls)
}

#[test]
fn partial_forest_joins_reachable_groups_when_a_pad_is_boxed_in_any_order() {
    let points = [(3.0, 10.0), (11.0, 6.0), (15.0, 10.0), (11.0, 14.0)];
    for rotation in 0..points.len() {
        let mut ordered = points.to_vec();
        ordered.rotate_left(rotation);
        let blocked = ordered.iter().position(|point| *point == points[0]).unwrap();
        let board = boxed_board(&ordered);
        let original_tracks = board.tracks.clone();
        let mut router = router(&board, &options(), &[&board.nets[0]]);
        let route = router.route_net(0, 1.0).unwrap();
        assert!(!route.connected && !route.search_budget_exhausted());
        assert!(!route.paths.is_empty(), "blocked pad index {blocked}: {}", route.reason);
        let routes = IndexMap::from([("A".to_owned(), route)]);
        let conflicts = router.conflicts(&routes).unwrap();
        let (accepted, rejected, complete) = router.commit(&routes, &conflicts).unwrap();
        assert!(rejected.is_empty() && complete.is_empty(), "{rejected:?}");
        let mut result = board.clone();
        for (tracks, vias) in accepted.values() {
            result.tracks.extend(tracks.iter().cloned());
            result.vias.extend(vias.iter().cloned());
        }
        assert_eq!(&result.tracks[..original_tracks.len()], original_tracks);
        let names = IndexSet::from(["A".to_owned()]);
        let graph =
            explicit_copper_connectivity(&result, Candidate { only_nets: Some(&names), ..Candidate::default() })
                .unwrap();
        let root = |index: usize| {
            let pad = PadReference::new(&format!("A_{index}"), "1");
            graph.roots[&graph.pad_nodes[&pad][0]].clone()
        };
        let reachable: Vec<_> = (0..points.len()).filter(|index| *index != blocked).map(root).collect();
        assert!(reachable.iter().all(|node| node == &reachable[0]), "blocked pad index {blocked}");
        assert_ne!(root(blocked), reachable[0], "blocked pad must remain disconnected");
        let drc = copper_drc::run_physical_drc(&result, copper_drc::Policy::default()).unwrap();
        assert!(
            drc.findings.iter().all(|finding| matches!(finding.code.as_str(), "DRC-OPEN-NET" | "DRC-ROUTE-INCOMPLETE")
                || finding.severity != copper_drc::Severity::Error),
            "{:?}",
            drc.findings
        );
    }
}

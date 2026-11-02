//! Concave keepouts preserve real regions without opening isolation cuts
//! (ported from the former `tests/test_global_polygon_keepouts.py`), and the
//! terminal-failure summary of `tests/test_global_access_diagnostics.py`.

mod support_routers;

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard};
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_route::global_terminals::terminal_access_failures;
use copper_route::routing::{AccessOptions, GridNode, PinAccess, build_graph};
use copper_route::{GlobalRouterOptions, route_global};
use indexmap::IndexMap;
use support_routers::*;

/// `tests/test_detailed_routing.py::_board`.
fn detailed_board() -> PhysicalBoard {
    board(
        "DetailedRoute",
        20.0,
        12.0,
        vec![one_pad()],
        vec![placement("J1", "test/one-pad", 3.0, 6.0), placement("J2", "test/one-pad", 17.0, 6.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    )
}

fn options() -> GlobalRouterOptions {
    GlobalRouterOptions { tile_size_nm: 2_000_000, maximum_iterations: 1, ..GlobalRouterOptions::default() }
}

fn island_board(across: bool) -> PhysicalBoard {
    let mut board = detailed_board();
    let barrier: Vec<Point> = [
        (0.0, 5.0),
        (5.0, 5.0),
        (5.0, 15.0),
        (15.0, 15.0),
        (15.0, 0.0),
        (17.0, 0.0),
        (17.0, 17.0),
        (3.0, 17.0),
        (3.0, 7.0),
        (0.0, 7.0),
    ]
    .iter()
    .map(|(x, y)| pt(*x, *y))
    .collect();
    board.outline = BoardOutline::rectangle(mm(22.0), mm(20.0), None).expect("outline");
    board.placements[0].position = pt(8.0, 3.0);
    board.placements[1].position = pt(if across { 19.0 } else { 12.0 }, 11.0);
    board.copper_keepouts = vec![copper_keepout("isolation", &board.stackup.copper_layers.clone(), barrier)];
    board.net_routing_rules =
        vec![NetRoutingRule { allowed_layers: vec![CopperLayer::Front], max_vias: Some(0), ..rule("SIGNAL") }];
    board
}

#[test]
fn concave_barrier_does_not_block_its_entire_bounding_rectangle() {
    let board = island_board(false);
    let graph = build_graph(&board, &options(), false).expect("graph");
    assert!(graph.legal_nodes.contains(&GridNode::new(0, 4, 4))); // (9, 9) mm, inside the island
    assert!(!graph.legal_nodes.contains(&GridNode::new(0, 7, 4))); // (15, 9) mm, the barrier
    let result = route_global(&board, &options(), None, None, None).expect("global");
    assert!(result.routes[0].connected, "{:?}", result.routes[0].diagnostics);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn concave_barrier_still_separates_the_two_domains() {
    let result = route_global(&island_board(true), &options(), None, None, None).expect("global");
    assert!(!result.routes[0].connected);
}

#[test]
fn a_thin_wall_between_coarse_centers_cannot_be_skipped() {
    let mut board = detailed_board();
    let layers = board.stackup.copper_layers.clone();
    board.copper_keepouts = vec![copper_keepout("thin", &layers, rect(7.2, 0.0, 7.3, 12.0))];
    let result = route_global(&board, &options(), None, None, None).expect("global");
    assert!(!result.routes[0].connected);
}

/// `test_global_access_diagnostics.py::test_partial_capacity_admission_is_not_reported_as_an_empty_terminal`;
/// Python's integer stand-ins are distinct accesses here.
#[test]
fn partial_capacity_admission_is_not_reported_as_an_empty_terminal() {
    let access = |n: i64| {
        PinAccess::new(
            PadReference::new("U", &n.to_string()),
            pt(1.0, 1.0),
            GridNode::new(0, n, 0),
            pt(1.0, 1.0),
            CopperLayer::Front,
        )
    };
    let key = |pad: &str| ("N".to_owned(), PadReference::new(pad, "1"));
    let generated: AccessOptions =
        IndexMap::from([(key("A"), vec![access(1), access(2)]), (key("B"), vec![]), (key("C"), vec![access(3)])]);
    let accepted: AccessOptions = IndexMap::from([(key("A"), vec![access(2)]), (key("B"), vec![]), (key("C"), vec![])]);
    let before = (generated.clone(), accepted.clone());
    assert_eq!(
        terminal_access_failures(&generated, &accepted),
        IndexMap::from([(key("B"), (0, 0)), (key("C"), (1, 0))])
    );
    assert_eq!((generated, accepted), before);
}

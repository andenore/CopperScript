//! `tests/test_pair_terminal_fallback.py`: exact paired routing retains the
//! real source terminals when coarse access fails (the route-board CLI case
//! is in `crates/copper/tests/critical_cli.rs`).

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet};
use copper_physical::CopperLayer::Front as F;
use copper_physical::footprint::Size;
use indexmap::IndexMap;

use crate::critical::{CriticalOptions, CriticalRoutingResult, pair_pin_stubs, route_critical_nets};
use crate::pair_terminals::{PairTerminalPreparation, joined_terminal_variants, prepare_pair_terminals};
use crate::routing::{GlobalNetRoute, GlobalRoutingResult, GridNode, PinAccess};
use crate::test_support_critical::*;

/// `fixture(duplicates)`: pair A/B from U (5, 10) to two one-pad terminals,
/// or to a connector with a duplicated contact per member.
fn fixture(duplicates: bool) -> PhysicalBoard {
    let land = |number: &str, x: i64, y: i64| smd_pad(number, at(x, y), 300_000);
    let source = footprint(
        "pair",
        vec![land("1", 0, -500_000), land("2", 0, 500_000)],
        Size { width_nm: MM, height_nm: 2 * MM },
    );
    let one = footprint("one", vec![land("1", 0, 0)], Size { width_nm: MM, height_nm: 600_000 });
    let (footprints, poses, nets) = if duplicates {
        let contacts = footprint(
            "contacts",
            vec![
                land("1", 0, -500_000),
                land("2", 0, 500_000),
                land("3", 1_700_000, -250_000),
                land("4", 1_700_000, 750_000),
            ],
            Size { width_nm: 4 * MM, height_nm: 4 * MM },
        );
        (
            vec![source, contacts],
            vec![pose("U", "pair", mm(5, 10)), pose("J", "contacts", mm(20, 10))],
            vec![net("A", &[("U", "1"), ("J", "1"), ("J", "3")]), net("B", &[("U", "2"), ("J", "2"), ("J", "4")])],
        )
    } else {
        (
            vec![source, one],
            vec![
                pose("U", "pair", mm(5, 10)),
                pose("C_A", "one", at(20 * MM, 9_250_000)),
                pose("C_B", "one", at(20 * MM, 10_750_000)),
            ],
            vec![net("A", &[("U", "1"), ("C_A", "1")]), net("B", &[("U", "2"), ("C_B", "1")])],
        )
    };
    let mut board = board("paired terminals", 30, 20, footprints, poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            allowed_layers: vec![F],
            max_vias: Some(0),
            max_skew_nm: Some(500_000),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(150_000),
            breakout_gap_nm: Some(150_000),
            breakout_clearance_nm: Some(100_000),
            ..differential_rule(name, partner, 200_000, 200_000)
        })
        .collect();
    board
}

/// `support_branch_fixture(distance)`: one-pad support lands R_A/R_B on each
/// member, `distance` mm right of U.
fn support_branch_fixture(distance_nm: i64) -> PhysicalBoard {
    let mut board = fixture(false);
    board.placements.push(pose("R_A", "one", at(5 * MM + distance_nm, 9_400_000)));
    board.placements.push(pose("R_B", "one", at(5 * MM + distance_nm, 10_600_000)));
    for net in &mut board.nets {
        net.pads.push(PadReference::new(&format!("R_{}", net.name), "1"));
    }
    board
}

/// The board's global route with every guide stripped and disconnected.
fn without_guides(board: &PhysicalBoard) -> GlobalRoutingResult {
    let mut global = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    for route in &mut global.routes {
        route.connected = false;
        route.accesses.clear();
        route.segments.clear();
        route.vias.clear();
    }
    global
}

fn route(board: &PhysicalBoard, global: &GlobalRoutingResult, limit: Option<i64>) -> CriticalRoutingResult {
    let options = CriticalOptions {
        pair_state_limit: limit,
        pair_state_escalation: limit.map(|_| 1),
        ..CriticalOptions::default()
    };
    route_critical_nets(board.clone(), global, &options, None).expect("critical routing")
}

/// Hard findings under `require_completed_detailed_route=False`.
fn errors(board: &PhysicalBoard) -> Vec<String> {
    let policy = copper_drc::Policy { require_completed_detailed_route: false, ..copper_drc::Policy::default() };
    let report = crate::drc_report::run_physical_drc(board, policy).expect("DRC");
    report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error)
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect()
}

fn all_connected(board: &PhysicalBoard, nets: &[PhysicalNet]) -> bool {
    nets.iter().all(|net| net_connected(board, &net.name))
}

fn unchanged(result: &CriticalRoutingResult, board: &PhysicalBoard) -> bool {
    result.board.tracks == board.tracks && result.board.vias == board.vias
}

#[test]
fn exact_search_without_coarse_guides_connects_every_real_terminal() {
    for duplicates in [false, true] {
        let board = fixture(duplicates);
        let result = route(&board, &without_guides(&board), None);
        assert!(result.nets.iter().all(|r| r.connected), "{duplicates}: {:?}", result.nets);
        assert!(all_connected(&result.board, &board.nets));
        assert_eq!(errors(&result.board), Vec::<String>::new());
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

#[test]
fn tiny_budget_does_not_adopt_partial_connector_ties() {
    let board = fixture(true);
    let result = route(&board, &without_guides(&board), Some(1));
    assert!(!result.nets.iter().any(|r| r.connected));
    assert!(unchanged(&result, &board));
}

#[test]
fn local_support_branches_preserve_all_contacts_and_atomic_acceptance() {
    for states in [1, 20_000] {
        let board = support_branch_fixture(1_800_000);
        let guide = without_guides(&board);
        let result = route(&board, &guide, Some(states));
        if states == 1 {
            assert!(!result.nets.iter().any(|r| r.connected));
            assert!(unchanged(&result, &board));
        } else {
            assert!(result.nets.iter().all(|r| r.connected), "{:?}", result.nets);
            assert!(all_connected(&result.board, &board.nets));
            assert_eq!(errors(&result.board), Vec::<String>::new());
        }
        assert!(!guide.routes.iter().any(|r| r.connected));
    }
}

#[test]
fn remote_third_component_is_not_absorbed_as_a_local_contact() {
    let board = support_branch_fixture(4 * MM);
    let result = route(&board, &without_guides(&board), None);
    assert!(!result.nets.iter().any(|r| r.connected));
    assert!(unchanged(&result, &board));
}

fn raw_routes(board: &PhysicalBoard) -> IndexMap<String, GlobalNetRoute> {
    guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm))
}

#[test]
fn joined_anchor_variants_retain_branches_and_original_net_contacts() {
    let board = support_branch_fixture(1_800_000);
    let rules = &board.net_routing_rules;
    let raw = raw_routes(&board);
    let prepared = prepare_pair_terminals(&board, &rules[0], &rules[1], &raw, &mut PairTerminalPreparation::default())
        .expect("prepared");
    assert!(!prepared.first_branches.is_empty() && !prepared.second_branches.is_empty());
    assert!(prepared.via_pairs.is_empty() && prepared.return_vias.is_empty());
    let mut held = board.clone();
    held.tracks = prepared.first_branches.iter().chain(&prepared.second_branches).cloned().collect();
    let variants = joined_terminal_variants(&held, &rules[0], &rules[1], &raw, &prepared.routes).expect("variants");
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0], prepared.routes);
    let connectivity =
        copper_drc::connectivity::explicit_copper_connectivity(&held, Default::default()).expect("connectivity");
    for rule in rules {
        let original = board.nets.iter().find(|n| n.name == rule.net).expect("net");
        let accesses = &variants[1][&rule.net].accesses;
        assert!(accesses.iter().all(|access| original.pads.contains(&access.pad)));
        let changed = accesses
            .iter()
            .map(|a| a.pad.clone())
            .find(|pad| pad.component.starts_with("R_"))
            .expect("a support anchor");
        let source = original.pads.iter().find(|p| p.component == "U").expect("source").clone();
        assert!(connectivity.net_connected(&PhysicalNet { name: rule.net.clone(), pads: vec![source, changed] }));
    }
    assert_eq!(held.nets, board.nets);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn remote_contacts_never_become_alternate_joined_anchors() {
    let board = support_branch_fixture(4 * MM);
    let rules = &board.net_routing_rules;
    let raw = raw_routes(&board);
    let prepared = prepare_pair_terminals(&board, &rules[0], &rules[1], &raw, &mut PairTerminalPreparation::default())
        .expect("prepared");
    assert!(prepared.first_branches.is_empty() && prepared.second_branches.is_empty());
    let variants = joined_terminal_variants(&board, &rules[0], &rules[1], &raw, &prepared.routes).expect("variants");
    assert_eq!(variants, vec![prepared.routes]);
}

#[test]
fn coarse_pair_stubs_retain_checked_source_lead_in_occurrences() {
    let board = fixture(false);
    let first = vec![
        track("A", at(5 * MM, 9_500_000), at(6 * MM, 9_500_000), 200_000, F),
        track("A", at(6 * MM, 9_500_000), at(6_500_000, 9_250_000), 200_000, F),
    ];
    let second = vec![track("A", at(20 * MM, 9_250_000), at(18_500_000, 9_250_000), 200_000, F)];
    let access = |component: &str,
                  pad: copper_physical::Point,
                  node: i64,
                  position: copper_physical::Point,
                  tracks: &[copper_pcb::board::TrackSegment]| PinAccess {
        tracks: tracks.to_vec(),
        ..PinAccess::new(PadReference::new(component, "1"), pad, GridNode::new(0, node, 0), position, F)
    };
    let guide = GlobalNetRoute {
        net: "A".to_owned(),
        connected: true,
        accesses: vec![
            access("U", at(5 * MM, 9_500_000), 0, at(6_500_000, 9_250_000), &first),
            access("C_A", at(20 * MM, 9_250_000), 1, at(18_500_000, 9_250_000), &second),
        ],
        segments: Vec::new(),
        vias: Vec::new(),
        length_nm: 0,
        diagnostics: Vec::new(),
        deferred_to_zone: false,
    };
    let paths = vec![(F, at(6_500_000, 9_250_000), at(18_500_000, 9_250_000))];
    let stubs = pair_pin_stubs(&board, "A", &guide, 200_000, &paths, 1, 400_000, true).expect("stubs");
    assert_eq!(stubs, [first, second].concat());
}

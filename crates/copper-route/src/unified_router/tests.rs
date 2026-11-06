//! The crate-private helpers of the unified router (ported from the former
//! `tests/test_unified_router.py`; the public-API cases are in
//! `tests/unified_router.rs`).

use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, PhysicalNet, TrackSegment};
use copper_physical::{Bounds, CopperLayer};
use indexmap::IndexMap;

use super::UnifiedRouterOptions;
use super::negotiate::Accepted;
use super::route::{open_count, rejection_text, tune_bus_skew};
use crate::routing_costs::LayerBias;
use crate::test_support_routers::*;

const F: CopperLayer = CopperLayer::Front;

fn opts() -> UnifiedRouterOptions {
    UnifiedRouterOptions::default()
}

fn a_board() -> PhysicalBoard {
    pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![])
}

#[test]
fn selected_seed_keeps_one_mutable_occupancy_frame_for_later_nets() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(10.0, 3.0), (10.0, 17.0)])], 2, vec![]);
    let options = UnifiedRouterOptions { maximum_iterations: 8, ..opts() };
    let mut router = router_all(&board, &options);
    let selected = ["A".to_owned()].into_iter().collect();
    let (_, _, first) = router.negotiate_bounded(None, Some(&selected), Some(2), true, &mut Vec::new()).unwrap();
    assert!(router.routes["A"].connected);
    assert!(!router.routes.contains_key("B"), "seeding does not manufacture failed routes for other nets");
    assert_eq!(router.board, &board, "no incumbent copper was frozen into source intent");
    let rest = ["B".to_owned()].into_iter().collect();
    let (routes, conflicts, next) =
        router.negotiate_bounded(None, Some(&rest), Some(8 - first), false, &mut Vec::new()).unwrap();
    assert!(first + next <= 8);
    let (_, rejected, complete) = router.commit(&routes, &conflicts).unwrap();
    assert!(rejected.is_empty() && complete.contains("A") && complete.contains("B"));
    assert_eq!(router.board, &board);
}

#[test]
fn rejection_diagnostic_names_the_blocker() {
    let board = a_board();
    let mut router = router_all(&board, &opts());
    router.rejections.insert(
        "A".to_owned(),
        vec![
            (pt(1.0, 2.0), "F.Cu".to_owned(), vec!["B".to_owned()], false, "copper clearance: t1".to_owned()),
            (pt(3.0, 4.0), "via".to_owned(), Vec::new(), true, String::new()),
        ],
    );
    assert_eq!(
        rejection_text(&router, "A"),
        "unified: rejected by exact commit check: blocked by B at (1.00,2.00) F.Cu [copper clearance: t1] (+1 more)"
    );
    assert_eq!(rejection_text(&router, "C"), "unified: rejected by exact commit check");
}

/// `a_board` plus an `n` x `n` 1 mm ball array (0.4 mm balls) as U at `at`.
fn with_array(mut board: PhysicalBoard, n: i64, at: (f64, f64)) -> PhysicalBoard {
    let name = format!("test/bga{}", n * n);
    board.footprints.insert(name.clone(), ball_array(&name, n, 1.0, 0.4));
    board.placements.push(placement("U", &name, at.0, at.1));
    board
}

#[test]
fn ball_field_via_mask_keeps_only_cell_centres() {
    // 3x3 ball array at 1 mm pitch; grid pitch 0.25 mm.
    let board = with_array(a_board(), 3, (10.0, 4.0));
    let options = UnifiedRouterOptions { pitch_nm: Some(mm(0.25)), ball_field_via_sites: true, ..opts() };
    let router = router_all(&board, &options);
    let mask = router.ball_field_via_mask().expect("mask").expect("3x3 array detected as a ball field");
    let at = |x: f64, y: f64| mask[xy(&router, x, y) as usize];
    assert!(at(9.5, 3.5) && at(10.5, 4.5), "cell centres");
    assert!(!at(10.0, 4.0) && !at(9.75, 3.5), "on a ball / between centres");
    assert!(at(15.0, 15.0), "outside the field");
}

#[test]
fn ball_field_band_keeps_the_outer_cell_rings_via_free() {
    // 5x5 ball array at 1 mm pitch centred on (10, 6): cell centres at +-0.5 and +-1.5 mm.
    let board = with_array(pad_board(&[("A", &[(3.0, 15.0), (17.0, 15.0)])], 2, vec![]), 5, (10.0, 6.0));
    let mask_at = |band: i64| {
        let options = UnifiedRouterOptions {
            pitch_nm: Some(mm(0.25)),
            ball_field_via_sites: true,
            ball_field_band: band,
            ..opts()
        };
        let router = router_all(&board, &options);
        let mask = router.ball_field_via_mask().expect("mask").expect("ball field");
        move |x: f64, y: f64| mask[xy(&router, x, y) as usize]
    };
    let plain = mask_at(0);
    let banded = mask_at(1);
    assert!(plain(8.5, 4.5) && plain(9.5, 5.5), "outer and inner cell centres");
    assert!(!banded(8.5, 4.5) && !banded(11.5, 5.5), "the outer ring of cells is closed");
    assert!(banded(9.5, 5.5) && banded(10.5, 6.5), "the inner 2x2 cells stay open");
    assert!(banded(15.0, 15.0), "outside the field");
}

#[test]
fn exit_corridors_cost_other_nets_and_not_their_owner() {
    // S leaves outer-ring ball 3 (column 2, row 0: top edge); T leaves
    // second-ring ball 8 (column 2, row 1).
    let mut board = with_array(pad_board(&[("X", &[(3.0, 18.0), (17.0, 18.0)])], 2, vec![]), 5, (10.0, 6.0));
    board.nets = vec![net("S", &[("U", "3"), ("X_0", "1")]), net("T", &[("U", "8"), ("X_1", "1")])];
    let options = UnifiedRouterOptions { pitch_nm: Some(mm(0.25)), exit_reservation_cost: 5.0, ..opts() };
    let mut router = router_all(&board, &options);
    // Straight above ball 3 (at (10, 4)), past the edge; and between outer
    // balls 2 and 3, in front of ball 8 (at (10, 5)).
    let above = xy(&router, 10.0, 3.5);
    let gap = xy(&router, 9.5, 4.0);
    assert!(router.exit_corridors["S"].contains(&above) && router.exit_corridors["T"].contains(&gap));
    assert!(router.occupancy.history[above as usize] >= 5.0 && router.occupancy.history[gap as usize] >= 5.0);
    let (nodes, extra) = router.escape_bias("S").expect("S bias");
    let bias: IndexMap<i64, f64> = nodes.iter().copied().zip(extra.iter().copied()).collect();
    assert_eq!(bias[&above], -5.0);
    assert!(!nodes.contains(&gap), "T's exit still costs S");
}

#[test]
fn plane_fill_width_sees_a_wide_rail_plane_and_taps_a_crowded_via() {
    // P: a 0.6 mm rail with a via into its small In1 zone, ringed by foreign vias 0.75 mm away.
    let mut board = pad_board(&[("P", &[(5.0, 5.0), (15.0, 15.0)]), ("X", &[(2.0, 20.0), (20.0, 20.0)])], 4, vec![]);
    let copper = board.stackup.copper_layers.clone();
    board.zones = vec![zone("p", "P", &[CopperLayer::In1], rect(7.0, 7.0, 10.0, 10.0))];
    let mut vias = vec![via("P", (8.5, 8.5), 0.45, 0.2, copper[0], copper[3])];
    for (dx, dy) in [(0.75, 0.0), (-0.75, 0.0), (0.0, 0.75), (0.0, -0.75)] {
        vias.push(via("X", (8.5 + dx, 8.5 + dy), 0.45, 0.2, copper[0], copper[3]));
    }
    board.vias = vias;
    board.net_routing_rules = vec![NetRoutingRule { width_nm: Some(mm(0.6)), ..rule("P") }];
    board.rules.minimum_track_width_nm = mm(0.1);
    board.rules.minimum_clearance_nm = mm(0.1);
    let plane_and_taps = |fill: bool| {
        let options = UnifiedRouterOptions {
            pitch_nm: Some(mm(0.125)),
            plane_fill_width: fill,
            reuse_plane_vias: true,
            ..opts()
        };
        let nets: Vec<&PhysicalNet> = board.nets.iter().filter(|n| n.name == "P").collect();
        let mut router = router(&board, &options, &nets);
        let spec = router.specs[0].clone();
        let (planes, _, _) = router.zone_regions("P", &spec);
        let taps = router.plane_via_taps("P", &spec, &planes).expect("taps");
        (planes.len(), taps.len())
    };
    let (narrow_plane, narrow_taps) = plane_and_taps(false);
    let (fill_plane, fill_taps) = plane_and_taps(true);
    assert!(fill_plane > narrow_plane, "the zone fill is not limited by the rail's 0.6 mm track width");
    assert!(narrow_taps == 0 && fill_taps > 0, "the crowded via counts as joined to its plane");
}

fn escape_bias(net: &str) -> LayerBias {
    LayerBias {
        net: net.to_owned(),
        layer: F,
        area: Bounds { min_x: mm(9.0), min_y: 0, max_x: mm(17.0), max_y: mm(26.0) },
        rate: 200,
    }
}

#[test]
fn escape_bias_is_restored_after_each_search() {
    // Python spied on `_search_unbiased`; here the bias is observed directly:
    // only the biased net has one, and no search leaves it in the history.
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 14.0), (17.0, 14.0)])], 2, vec![]);
    let options = UnifiedRouterOptions { escape_layer_biases: vec![escape_bias("A")], maximum_iterations: 1, ..opts() };
    let mut router = router_all(&board, &options);
    let (_, extra) = router.escape_bias("A").expect("A is biased");
    assert!(extra.iter().any(|amount| *amount > 0.0));
    assert!(router.escape_bias("B").is_none());
    for index in 0..router.specs.len() {
        assert!(router.route_net(index, 1.0).expect("route").connected);
        assert!(router.occupancy.history.iter().all(|h| *h == 0.0), "history after {}", router.specs[index].name);
    }
}

#[test]
fn planned_nets_negotiate_first() {
    let board = pad_board(
        &[
            ("A", &[(3.0, 10.0), (17.0, 10.0)]),
            ("B", &[(3.0, 14.0), (17.0, 14.0)]),
            ("C", &[(3.0, 18.0), (17.0, 18.0)]),
        ],
        2,
        vec![],
    );
    let options =
        UnifiedRouterOptions { escape_net_order: vec!["C".into(), "A".into()], maximum_iterations: 1, ..opts() };
    let mut router = router_all(&board, &options);
    router.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    // The routes are recorded in the order the nets were searched.
    assert_eq!(router.routes.keys().map(String::as_str).collect::<Vec<_>>(), ["C", "A", "B"]);
}

fn bus_board(amplitude: Option<f64>, max_length: Option<f64>) -> PhysicalBoard {
    let mut board = pad_board(&[("A", &[(3.0, 6.0), (11.0, 6.0)]), ("B", &[(3.0, 14.0), (17.0, 14.0)])], 2, vec![]);
    board.net_routing_rules = ["A", "B"]
        .iter()
        .map(|n| NetRoutingRule {
            width_nm: Some(mm(0.2)),
            tuning_amplitude_limit_nm: amplitude.map(mm),
            max_length_nm: max_length.map(mm),
            ..rule(n)
        })
        .collect();
    board.match_groups =
        vec![NetMatchGroup { id: "bus".to_owned(), nets: vec!["A".into(), "B".into()], max_skew_nm: mm(2.0) }];
    board
}

fn straight(net: &str, start: (f64, f64), end: (f64, f64)) -> TrackSegment {
    track(net, (start.0 + 3.0, start.1 + 3.0), (end.0 + 3.0, end.1 + 3.0), 0.2, F)
}

fn accepted(a_end: f64) -> Accepted {
    IndexMap::from([
        ("A".to_owned(), (vec![straight("A", (3.0, 10.0), (a_end, 10.0))], Vec::new())),
        ("B".to_owned(), (vec![straight("B", (3.0, 14.0), (17.0, 14.0))], Vec::new())),
    ])
}

fn rules(board: &PhysicalBoard) -> IndexMap<&str, &NetRoutingRule> {
    board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect()
}

#[test]
fn bus_skew_tuning_lengthens_the_short_member_within_max_skew() {
    let mut board = bus_board(Some(1.0), None);
    let before = accepted(11.0);
    let (tuned, notes) = tune_bus_skew(&board, &before, &rules(&board)).expect("tune");
    let length = |net: &str| net_length(&tuned[net].0, net);
    // 8 mm against 14 mm: 6 mm of skew over a 2 mm limit; A gains at least 4 mm and B is untouched.
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(tuned["B"], before["B"]);
    assert!(mm(14.0) as f64 - length("A") <= mm(2.0) as f64 && length("A") > mm(12.0) as f64);
    board.tracks = tuned["A"].0.iter().chain(&tuned["B"].0).cloned().collect();
    let report = copper_drc::run_physical_drc(&board, copper_drc::Policy::default()).expect("drc");
    assert!(routes::errors_except_opens(&report).is_empty(), "{:?}", routes::errors_except_opens(&report));
}

#[test]
fn bus_skew_tuning_needs_an_amplitude_contract_and_room_under_max_length() {
    let before = accepted(11.0);
    for (board, reason) in [
        (bus_board(None, None), "no tuning_amplitude_limit is declared"),
        (bus_board(Some(1.0), Some(9.0)), "max_length leaves no room"),
    ] {
        let (tuned, notes) = tune_bus_skew(&board, &before, &rules(&board)).expect("tune");
        assert_eq!(tuned, before);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains(reason) && notes[0].contains("bus: A is"), "{}", notes[0]);
    }
    // A group within its limit, or one with a member still unrouted, is left alone.
    let board = bus_board(Some(1.0), None);
    let close = accepted(16.0);
    assert_eq!(tune_bus_skew(&board, &close, &rules(&board)).expect("tune"), (close.clone(), Vec::new()));
    let only_b: Accepted = IndexMap::from([("B".to_owned(), before["B"].clone())]);
    assert_eq!(tune_bus_skew(&board, &only_b, &rules(&board)).expect("tune"), (only_b.clone(), Vec::new()));
}

/// `tests/test_unified_nested_routing.py::test_congestion_capacity_is_per_area_on_both_lattices`
/// (the empty-board half; the routed half is in `tests/unified_nested_routing.rs`).
#[test]
fn congestion_capacity_is_per_area_on_both_lattices() {
    use super::route::congestion_map;
    let mut board = board(
        "NestedBGA",
        20.0,
        20.0,
        vec![ball_array("test/bga25_0.75", 5, 0.75, 0.35)],
        vec![placement("U1", "test/bga25_0.75", 10.0, 10.0)],
        Vec::new(),
    );
    board.stackup = copper_pcb::board::Stackup::new(layers(4));
    board.rules = copper_physical::DesignRules {
        minimum_clearance_nm: mm(0.1),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.1),
        default_via_size_nm: mm(0.4),
        default_via_drill_nm: mm(0.2),
        minimum_hole_clearance_nm: mm(0.15),
        ..copper_physical::DesignRules::default()
    };
    let options = UnifiedRouterOptions { fine_regions: "dense".to_owned(), congestion_tile_nm: mm(1.0), ..opts() };
    let mut nested = router(&board, &options, &[]);
    let congestion = congestion_map(&mut nested, &[], mm(1.0));
    let patch = &nested.lattice.blocks()[1];
    // Tile (7, 7) lies in the patch's margin between its edge and the balls;
    // tile (1, 10) is free board far from every pad.
    assert!(patch.x0 < mm(7.0) && patch.y0 < mm(7.0));
    let (ix, iy) = congestion.tile_of(pt(7.5, 7.5));
    let (ox, oy) = congestion.tile_of(pt(1.5, 10.5));
    let (fine, coarse) = (congestion.capacity[iy][ix], congestion.capacity[oy][ox]);
    assert!(coarse > 0.0 && (fine - coarse).abs() <= 0.1 * coarse, "{fine} against {coarse}");
    let uniform_options = UnifiedRouterOptions { congestion_tile_nm: mm(1.0), ..opts() };
    let mut uniform = router(&board, &uniform_options, &[]);
    assert!(congestion_map(&mut uniform, &[], mm(1.0)).capacity.iter().flatten().all(|c| c.is_finite()));
}

/// `test_unified_router_options.py::test_complete_plane_net_without_new_copper_is_not_counted_open`
/// (the routed half is `pads_already_stitched_into_their_plane_count_as_joined`).
#[test]
fn complete_plane_net_without_new_copper_is_not_counted_open() {
    let board = stitched_pad_board();
    let gnd: Vec<&PhysicalNet> = board.nets.iter().filter(|net| net.name == "GND").collect();
    let plane: indexmap::IndexSet<String> = ["GND".to_owned()].into_iter().collect();
    // GND's pads reach the plane through existing vias: complete, no copper.
    assert_eq!(open_count(&board, &gnd, &Accepted::new(), &plane, &plane).expect("count"), (0, 0));
    let incomplete = indexmap::IndexSet::new();
    assert_eq!(open_count(&board, &gnd, &Accepted::new(), &plane, &incomplete).expect("count"), (0, 1));
}

/// `test_unified_search_costs.py::test_turn_table_charges_per_45_degrees`.
#[test]
fn turn_table_charges_per_45_degrees() {
    let board = a_board();
    let options = UnifiedRouterOptions { pitch_nm: Some(250_000), bend_cost: 0.5, ..opts() };
    let mut router = router_all(&board, &options);
    let table = router.turn_table();
    let at = |a: usize, b: usize| table[a * 8 + b];
    // Move order: E, W, S, N, SE, NE, SW, NW (`routing_lattice::MOVES`).
    assert_eq!(at(0, 0), 0.0);
    assert_eq!((at(0, 4), at(0, 5)), (0.5, 0.5)); // 45 degrees
    assert_eq!((at(0, 2), at(0, 3)), (1.0, 1.0)); // 90 degrees
    assert_eq!(at(0, 1), 2.0); // reversal
    assert!((0..8).all(|a| (0..8).all(|b| at(a, b) == at(b, a))));
}

// ---------------------------------------------------------------------------
// `tests/test_unified_via_budget_pressure.py`: a legal via-bounded route must
// be able to contest old congestion costs. One search in Rust: the `native`
// parametrization runs once.

fn wall(name: &str, layer: CopperLayer, x: f64) -> copper_physical::CopperKeepout {
    copper_keepout(name, &[layer], rect(x - 0.5, 0.0, x + 0.5, 26.0))
}

fn budget_rule(net: &str) -> NetRoutingRule {
    NetRoutingRule {
        width_nm: Some(mm(0.2)),
        allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
        max_vias: Some(2),
        ..rule(net)
    }
}

fn pressure_board(back_wall: bool) -> PhysicalBoard {
    let mut board = pad_board(&[("S", &[(2.0, 10.0), (18.0, 10.0)])], 2, vec![]);
    board.copper_keepouts = vec![wall("front-a", F, 10.0), wall("front-b", F, 16.0)];
    if back_wall {
        board.copper_keepouts.push(wall("real-back-wall", CopperLayer::Back, 13.0));
    }
    board.net_routing_rules = vec![budget_rule("S")];
    board
}

fn pressure_options(iterations: i64) -> UnifiedRouterOptions {
    UnifiedRouterOptions { maximum_iterations: iterations, commit_rounds: 1, ..opts() }
}

/// The B.Cu nodes whose x lies in 12.5..=13.5 mm.
fn back_corridor(router: &crate::unified_router::router::Router<'_, '_>) -> Vec<usize> {
    let g = &router.lattice;
    let layer = g.layers.iter().position(|l| *l == CopperLayer::Back).expect("B.Cu") as i64;
    let mut nodes = Vec::new();
    for x in 0..g.nx {
        let at = g.x0 + x * g.pitch;
        if mm(12.5) <= at && at <= mm(13.5) {
            nodes.extend((0..g.ny).map(|y| (layer * g.plane + y * g.nx + x) as usize));
        }
    }
    nodes
}

fn clean_commit(board: &PhysicalBoard, accepted: &Accepted) {
    let mut result = board.clone();
    for (tracks, vias) in accepted.values() {
        result.tracks.extend(tracks.iter().cloned());
        result.vias.extend(vias.iter().cloned());
    }
    let report = copper_drc::run_physical_drc(&result, copper_drc::Policy::default()).expect("drc");
    let summary = crate::route_summary::connection_summary(&result, Some(&report)).expect("summary");
    assert_eq!(summary_int(&summary, "signal", "opens"), 0);
    assert_eq!(summary_int(&summary, "drc", "errors"), 0, "{:?}", report.findings);
}

#[test]
fn via_budget_can_reclaim_a_legal_corridor_from_soft_costs() {
    let board = pressure_board(false);
    for cost_field in ["history", "occ"] {
        let mut control = router_all(&board, &pressure_options(3));
        assert!(control.route_net_budgeted(0, 0.6).expect("route").connected);
        let mut proposed = router_all(&board, &pressure_options(3));
        let corridor = back_corridor(&proposed);
        for &node in &corridor {
            if cost_field == "history" {
                proposed.occupancy.history[node] = 5000.0;
            } else {
                proposed.occupancy.occ[node] = 5000;
            }
        }
        let (history, occ) = (proposed.occupancy.history.clone(), proposed.occupancy.occ.clone());
        let route = proposed.route_net_budgeted(0, 0.6).expect("route");
        assert!(route.connected, "{cost_field}: {}", route.reason);
        assert!(route.via_xy.len() <= 2);
        // The retry changes only this proposal's costs, never global history.
        assert!(proposed.occupancy.history == history && proposed.occupancy.occ == occ);
        assert!(corridor.iter().any(|n| route.claims.contains(&(*n as i64))));
        assert!(!proposed.ignore_soft_costs);
        assert_eq!(proposed.specs[0].via_factor, 4.0);
        let routes = IndexMap::from([("S".to_owned(), route)]);
        let (accepted, rejected, complete) = proposed.commit(&routes, &indexmap::IndexSet::new()).expect("commit");
        assert!(complete.contains("S") && complete.len() == 1 && rejected.is_empty());
        assert!(accepted["S"].1.len() <= 2);
        clean_commit(&board, &accepted);
    }
}

#[test]
fn via_budget_retry_never_ignores_real_source_obstacles() {
    let board = pressure_board(true);
    let mut proposed = router_all(&board, &pressure_options(3));
    let route = proposed.route_net_budgeted(0, 0.6).expect("route");
    assert!(!route.connected);
    let routes = IndexMap::from([("S".to_owned(), route)]);
    let (accepted, _, complete) = proposed.commit(&routes, &indexmap::IndexSet::new()).expect("commit");
    assert!(!complete.contains("S"));
    assert!(accepted.values().map(|(_, vias)| vias.len()).sum::<usize>() <= 2);
}

#[test]
fn via_bounded_nets_still_negotiate_with_each_other_under_history() {
    let mut board = pad_board(&[("S", &[(2.0, 10.0), (18.0, 10.0)]), ("T", &[(2.0, 12.0), (18.0, 12.0)])], 2, vec![]);
    board.copper_keepouts = pressure_board(false).copper_keepouts;
    for (name, y0, y1) in [("back-upper", 0.0, 12.0), ("back-lower", 14.0, 26.0)] {
        board.copper_keepouts.push(copper_keepout(name, &[CopperLayer::Back], rect(0.0, y0, 26.0, y1)));
    }
    board.net_routing_rules = vec![budget_rule("S"), budget_rule("T")];
    let both: indexmap::IndexSet<String> = ["S".to_owned(), "T".to_owned()].into_iter().collect();
    let mut control = router_all(&board, &pressure_options(16));
    let (plain, plain_conflicts, _) = control.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    assert_eq!(control.commit(&plain, &plain_conflicts).expect("commit").2, both);
    let mut proposed = router_all(&board, &pressure_options(16));
    for node in back_corridor(&proposed) {
        proposed.occupancy.history[node] = 5000.0;
    }
    let (routes, conflicts, _) = proposed.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    let (accepted, rejected, complete) = proposed.commit(&routes, &conflicts).expect("commit");
    assert_eq!(complete, both, "{conflicts:?} {rejected:?}");
    assert!(routes.values().all(|route| route.via_xy.len() <= 2));
    clean_commit(&board, &accepted);
}

// ---------------------------------------------------------------------------
// `tests/test_unified_lead_conflicts.py`: off-grid access copper must take
// part in negotiated via clearance. One search: `native` runs once.

/// The node of `(x, y)` mm on `layer`, rounded to the lattice.
fn grid_node(router: &crate::unified_router::router::Router<'_, '_>, layer: CopperLayer, x: f64, y: f64) -> i64 {
    let g = &router.lattice;
    let l = g.layers.iter().position(|item| *item == layer).expect("layer") as i64;
    let ix = ((mm(x) - g.x0) as f64 / g.pitch as f64).round() as i64;
    let iy = ((mm(y) - g.y0) as f64 / g.pitch as f64).round() as i64;
    l * g.plane + iy * g.nx + ix
}

fn scripted(paths: Vec<Vec<i64>>, leads: Vec<Vec<TrackSegment>>) -> super::router::NetRoute {
    super::router::NetRoute { paths, leads, ..super::router::NetRoute::empty(true, "") }
}

fn two_pad_board() -> PhysicalBoard {
    pad_board(&[("A", &[(2.0, 2.0), (3.0, 2.0)]), ("B", &[(18.0, 18.0), (19.0, 18.0)])], 2, vec![])
}

/// Finish and occupy `routes` in order (spec `i` for route `i`), then commit.
fn settle(
    router: &mut crate::unified_router::router::Router<'_, '_>,
    mut routes: Vec<super::router::NetRoute>,
) -> IndexMap<String, super::router::NetRoute> {
    for (index, route) in routes.iter_mut().enumerate() {
        router.finish_route(route, index);
        router.occupancy.add(route, 1);
    }
    ["A", "B"].iter().map(|s| (*s).to_owned()).zip(routes).collect()
}

#[test]
fn short_off_grid_lead_reports_a_real_via_clearance_conflict() {
    let board = two_pad_board();
    for sign in [-1.0, 1.0] {
        for (offset, close) in [(0.375, true), (0.625, false), (0.875, false)] {
            let mut router = router_all(&board, &opts());
            let (f, b) = (CopperLayer::Front, CopperLayer::Back);
            let a = scripted(
                vec![vec![grid_node(&router, f, 13.0, 13.0), grid_node(&router, b, 13.0, 13.0)]],
                vec![vec![]],
            );
            let lead = TrackSegment {
                net: "B".to_owned(),
                start: pt(13.0 + sign * offset, 13.125),
                end: pt(13.0 + sign * (offset + 0.125), 13.25),
                width_nm: mm(0.2),
                layer: f,
            };
            let end = grid_node(&router, f, 13.0 + sign * (offset + 0.125), 13.25);
            let other = scripted(vec![vec![end]], vec![vec![lead]]);
            let routes = settle(&mut router, vec![a, other]);
            // The exact geometry oracle already rejects this via/lead pair.
            // Negotiation must also see it, before fixing one route and
            // rejecting the other.
            let (_, rejected, _) = router.commit(&routes, &indexmap::IndexSet::new()).expect("commit");
            assert_eq!(!rejected.is_empty(), close, "{sign} {offset}");
            let conflicts = router.conflicts(&routes).expect("conflicts");
            let both: indexmap::IndexSet<String> = ["A".to_owned(), "B".to_owned()].into_iter().collect();
            assert_eq!(conflicts, if close { both } else { indexmap::IndexSet::new() }, "{sign} {offset}");
            router.penalise(&conflicts);
            let xy = grid_node(&router, f, 13.0, 13.0) % router.lattice.plane;
            assert_eq!(router.occupancy.via_history[xy as usize] > 0.0, close, "{sign} {offset}");
        }
    }
}

#[test]
fn grid_diagonal_via_collision_is_not_hidden_between_raster_nodes() {
    let board = two_pad_board();
    let mut router = router_all(&board, &opts());
    let (f, b) = (CopperLayer::Front, CopperLayer::Back);
    let a = scripted(vec![vec![grid_node(&router, f, 13.0, 13.0), grid_node(&router, b, 13.0, 13.0)]], vec![vec![]]);
    let track =
        scripted(vec![vec![grid_node(&router, f, 13.5, 13.25), grid_node(&router, f, 13.25, 13.5)]], vec![vec![]]);
    let routes = settle(&mut router, vec![a, track]);
    // The exact copper oracle sees the real clash.
    assert!(!router.commit(&routes, &indexmap::IndexSet::new()).expect("commit").1.is_empty());
    let both: indexmap::IndexSet<String> = ["A".to_owned(), "B".to_owned()].into_iter().collect();
    assert_eq!(router.conflicts(&routes).expect("conflicts"), both);
    router.penalise(&both);
    let xy = grid_node(&router, f, 13.0, 13.0) % router.lattice.plane;
    assert!(router.occupancy.via_history[xy as usize] > 0.0);
}

#[test]
fn via_proposals_avoid_mutable_tracks_on_unvisited_inner_layers() {
    let mut board = pad_board(&[("A", &[(2.0, 10.0), (18.0, 10.0)]), ("B", &[(2.0, 2.0), (18.0, 2.0)])], 4, vec![]);
    for pose in &mut board.placements {
        if pose.reference == "A_1" {
            pose.side = copper_physical::BoardSide::Back;
        }
    }
    board.net_routing_rules = vec![NetRoutingRule {
        width_nm: Some(200_000),
        allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
        max_vias: Some(1),
        ..rule("A")
    }];
    let mut router = router_all(&board, &opts());
    let (a, b) = (
        router.specs.iter().position(|s| s.name == "A").expect("A"),
        router.specs.iter().position(|s| s.name == "B").expect("B"),
    );
    let baseline = router.route_net_budgeted(a, 100.0).expect("route");
    assert!(baseline.connected && baseline.via_xy.len() == 1);
    let g = router.lattice.clone();
    let xy = *baseline.via_xy.first().expect("via");
    let (y, x) = (xy / g.nx, xy % g.nx);
    let layer = g.layers.iter().position(|l| *l == CopperLayer::In1).expect("In1") as i64;
    let at = |dy: i64| layer * g.plane + (y + dy) * g.nx + x;
    let mut foreign = scripted(vec![vec![at(-3), at(3)]], vec![vec![]]);
    router.finish_route(&mut foreign, b);
    let (tracks, _) = router.copper(b, &foreign.paths[0], &Vec::new(), None).expect("copper");
    let mut index = crate::routing_clearance::RoutingClearanceIndex::new(&board).expect("index");
    for t in &tracks {
        assert!(index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).expect("track"));
    }
    for t in &tracks {
        index.add_track(t.clone(), false, None).expect("add");
    }
    router.occupancy.add(&foreign, 1);
    let proposed = router.route_net_budgeted(a, 100.0).expect("route");
    assert!(proposed.connected && proposed.via_xy.len() == 1);
    for (path, leads) in proposed.paths.iter().zip(&proposed.leads) {
        let (_, vias) = router.copper(a, path, leads, None).expect("copper");
        for v in &vias {
            let clear = index
                .can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, Some(v.drill_nm), false, None)
                .expect("via");
            assert!(clear, "via proposal ignored a mutable track on its internal barrel span");
        }
    }
}

/// A is 8 mm of track plus two 1.6 mm layer transitions; B is 14 mm
/// on F.Cu. Matching uses 11.2 vs 14 mm, while max_length remains a
/// track-only budget (A may tune up to 11 mm of track).
fn via_depth_bus(kind: copper_pcb::board::RouteKind) -> (PhysicalBoard, Accepted) {
    let mut board = pad_board(&[("A", &[(3.0, 10.0), (11.0, 10.0)]), ("B", &[(3.0, 14.0), (17.0, 14.0)])], 2, vec![]);
    board.stackup.thickness_nm = mm(1.6);
    board.stackup.physical_layers.clear();
    board.net_routing_rules = ["A", "B"]
        .iter()
        .map(|name| NetRoutingRule {
            kind: if *name == "A" { kind } else { copper_pcb::board::RouteKind::General },
            width_nm: Some(mm(0.2)),
            max_vias: Some(2),
            max_length_nm: Some(mm(if *name == "A" { 11.0 } else { 14.0 })),
            allowed_layers: vec![F, CopperLayer::Back],
            tuning_amplitude_limit_nm: Some(mm(1.0)),
            ..rule(name)
        })
        .collect();
    board.match_groups =
        vec![NetMatchGroup { id: "bus".to_owned(), nets: vec!["A".to_owned(), "B".to_owned()], max_skew_nm: mm(2.0) }];
    let accepted = IndexMap::from([
        (
            "A".to_owned(),
            (
                vec![
                    track("A", (6.0, 13.0), (8.0, 13.0), 0.2, F),
                    track("A", (8.0, 13.0), (12.0, 13.0), 0.2, CopperLayer::Back),
                    track("A", (12.0, 13.0), (14.0, 13.0), 0.2, F),
                ],
                vec![
                    via("A", (8.0, 13.0), 0.6, 0.3, F, CopperLayer::Back),
                    via("A", (12.0, 13.0), 0.6, 0.3, F, CopperLayer::Back),
                ],
            ),
        ),
        ("B".to_owned(), (vec![track("B", (6.0, 17.0), (20.0, 17.0), 0.2, F)], vec![])),
    ]);
    (board, accepted)
}

#[test]
fn bus_skew_targets_include_via_depth_for_owned_general_and_clock_copper() {
    for kind in [copper_pcb::board::RouteKind::General, copper_pcb::board::RouteKind::Clock] {
        for split_prefix in [false, true] {
            let (mut board, mut before) = via_depth_bus(kind);
            if split_prefix {
                let owned = before.get_mut("A").expect("A");
                board.tracks.push(owned.0.remove(0));
                board.vias.push(owned.1.remove(0));
            }
            let source = board.clone();
            let (tuned, notes) = tune_bus_skew(&board, &before, &rules(&board)).expect("tune");
            assert!(notes.is_empty(), "{kind:?}: {notes:?}");
            assert_eq!(tuned["A"].1, before["A"].1, "never move or replace vias");
            assert_eq!(tuned["B"], before["B"], "long member is untouched");
            let mut result = board.clone();
            for (tracks, vias) in tuned.values() {
                result.tracks.extend(tracks.iter().cloned());
                result.vias.extend(vias.iter().cloned());
            }
            let check = copper_drc::signal_integrity::verify_match_groups(&result).expect("match check");
            assert_eq!(check[0].status, "pass", "{kind:?}: {:?}", check[0].members);
            assert!(net_length(&result.tracks, "A") <= mm(11.0) as f64);
            assert_eq!(board, source, "fixed source tracks and vias are immutable");
            assert!(result.tracks.starts_with(&source.tracks) && result.vias.starts_with(&source.vias));
            assert_eq!(result.net_routing_rules, board.net_routing_rules);
            let drc = copper_drc::run_physical_drc(&result, copper_drc::Policy::default()).expect("DRC");
            assert!(routes::errors_except_opens(&drc).is_empty(), "{:?}", routes::errors_except_opens(&drc));
        }
    }
}

#[test]
fn bus_skew_read_only_clock_deficit_includes_source_via_depth() {
    let (mut board, mut before) = via_depth_bus(copper_pcb::board::RouteKind::Clock);
    let (tracks, vias) = before.shift_remove("A").expect("clock");
    board.tracks = tracks;
    board.vias = vias;
    let saved = board.clone();
    let (tuned, notes) = tune_bus_skew(&board, &before, &rules(&board)).expect("tune");
    assert_eq!(tuned, before);
    assert_eq!(notes, vec!["bus: A is 800000 nm short; its copper is not the bus router's"]);
    assert_eq!(board, saved, "input Clock copper remains read-only");
}

#[test]
fn bus_skew_tuning_combines_arched_copper_and_via_depth() {
    let (mut board, mut before) = via_depth_bus(copper_pcb::board::RouteKind::General);
    for rule in &mut board.net_routing_rules {
        rule.corner_style = copper_pcb::board::CornerStyle::Arc;
        rule.corner_radius_nm = Some(mm(0.3));
    }
    before.get_mut("B").expect("B").0 =
        vec![track("B", (6.0, 17.0), (14.0, 17.0), 0.2, F), track("B", (14.0, 17.0), (14.0, 23.0), 0.2, F)];
    let source = board.clone();
    let (tuned, notes) = tune_bus_skew(&board, &before, &rules(&board)).expect("tune");
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(tuned["A"].1, before["A"].1, "via contacts must survive tuning");
    assert_eq!(tuned["B"], before["B"], "the longer member stays untouched");
    let mut result = board.clone();
    for (tracks, vias) in tuned.values() {
        result.tracks.extend(tracks.iter().cloned());
        result.vias.extend(vias.iter().cloned());
    }
    let (arched, _) = crate::arc_corners::arch_board(&result).expect("arch").expect("arc policy");
    assert!(!arched.arcs.is_empty(), "exercise actual curved copper");
    let check = copper_drc::signal_integrity::verify_match_groups(&arched).expect("match check");
    assert_eq!(check[0].status, "pass", "{:?}", check[0].members);
    assert_eq!(board, source, "source copper remains immutable");
}

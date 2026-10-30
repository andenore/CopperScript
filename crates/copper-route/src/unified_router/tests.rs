//! The crate-private helpers of the unified router (ported from the former
//! `tests/test_unified_router.py`; the public-API cases are in
//! `tests/unified_router.rs`).

use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, PhysicalNet, TrackSegment};
use copper_physical::{Bounds, CopperLayer};
use indexmap::IndexMap;

use super::UnifiedRouterOptions;
use super::negotiate::Accepted;
use super::route::{rejection_text, tune_bus_skew};
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

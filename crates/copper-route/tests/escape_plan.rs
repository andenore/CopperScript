//! Escape planning (E0-E5): two-end crossings, the channel model, two-end
//! layer plans, array escapes and bus layers (ported from the former
//! `tests/test_escape_plan.py`).

mod support_routers;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Stackup};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardSide, Bounds, CopperLayer, DesignRules, Point};
use copper_route::escape_plan::{
    ArrayEscape, ClusterEscape, ClusterPairEscape, EscapePlanOptions, Gap, PadField, Tile, channel_rules,
    escape_diagnostics, pad_fields, plan_array_escapes, plan_two_end_escapes,
};
use copper_route::routing_costs::LayerBias;
use indexmap::{IndexMap, IndexSet};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;

fn rules() -> DesignRules {
    DesignRules {
        minimum_clearance_nm: mm(0.254),
        minimum_hole_clearance_nm: 200_000,
        minimum_track_width_nm: mm(0.254),
        default_track_width_nm: mm(0.254),
        default_via_size_nm: 800_000,
        default_via_drill_nm: 400_000,
        ..DesignRules::default()
    }
}

fn fine() -> DesignRules {
    DesignRules {
        minimum_clearance_nm: 100_000,
        minimum_hole_clearance_nm: 100_000,
        minimum_track_width_nm: 100_000,
        default_track_width_nm: 100_000,
        default_via_size_nm: 450_000,
        default_via_drill_nm: 200_000,
        ..DesignRules::default()
    }
}

fn defaults() -> EscapePlanOptions {
    EscapePlanOptions::default()
}

fn pref(component: &str, pad: &str) -> PadReference {
    PadReference::new(component, pad)
}

/// `count` SMD pads in one column, numbered from the top (smaller y).
fn row(name: &str, count: usize, pitch: f64) -> PhysicalFootprint {
    let pads = (0..count).map(|i| pad(&(i + 1).to_string(), 0.0, pitch * i as f64, 0.5, 0.5)).collect();
    footprint(name, pads, 1.0, count as f64)
}

struct Rows {
    rotation: i64,
    side: BoardSide,
    swap_sides: bool,
}

const PLAIN: Rows = Rows { rotation: 0, side: BoardSide::Front, swap_sides: false };

/// A four-pin column at x=10 and another at x=30; net `N{a}` joins A.a to B.(wiring[a]).
fn two_rows_with(wiring: &[(i64, i64)], shape: Rows) -> PhysicalBoard {
    let (first, second) = if shape.swap_sides { ((30.0, 10.0), (10.0, 10.0)) } else { ((10.0, 10.0), (30.0, 10.0)) };
    let mut a = rotated("A", "row", first.0, first.1, shape.rotation);
    a.side = shape.side;
    let nets =
        wiring.iter().map(|(x, y)| net(&format!("N{x}"), &[("A", &x.to_string()), ("B", &y.to_string())])).collect();
    let mut board =
        board("rows", 40.0, 30.0, vec![row("row", 4, 1.0)], vec![a, placement("B", "row", second.0, second.1)], nets);
    board.rules = rules();
    board
}

fn two_rows(wiring: &[(i64, i64)]) -> PhysicalBoard {
    two_rows_with(wiring, PLAIN)
}

const PARALLEL: [(i64, i64); 4] = [(1, 1), (2, 2), (3, 3), (4, 4)];
const SWAPPED: [(i64, i64); 4] = [(1, 2), (2, 1), (3, 3), (4, 4)];
const REVERSED: [(i64, i64); 4] = [(1, 4), (2, 3), (3, 2), (4, 1)];

fn pair(board: &PhysicalBoard) -> ClusterPairEscape {
    let pairs = escape_diagnostics(board, &defaults()).expect("diagnostics").pairs;
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    pairs.into_iter().next().expect("pair")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn parallel_bundle_does_not_cross() {
    let result = pair(&two_rows(&PARALLEL));
    assert_eq!((result.first.as_str(), result.second.as_str()), ("A", "B"));
    assert!(result.crossing_pairs == 0 && result.layer_moves.is_empty() && result.crossing_nets.is_empty());
}

#[test]
fn swapped_neighbours_cross_once_and_one_net_moves() {
    for swap_sides in [false, true] {
        let result = pair(&two_rows_with(&SWAPPED, Rows { swap_sides, ..PLAIN }));
        assert_eq!(result.crossing_pairs, 1);
        assert_eq!(result.crossing_nets, strings(&["N1", "N2"]));
        assert!(result.layer_moves.len() == 1 && ["N1", "N2"].contains(&result.layer_moves[0].as_str()));
    }
}

#[test]
fn reversed_bundle_crosses_every_pair_and_keeps_one_net() {
    let result = pair(&two_rows(&REVERSED));
    assert_eq!(result.crossing_pairs, 6);
    assert_eq!(result.layer_moves.len(), 3);
}

#[test]
fn order_is_read_on_the_placed_board() {
    // Rotating A by 180 degrees about its first pin reverses its column.
    assert_eq!(pair(&two_rows_with(&REVERSED, Rows { rotation: 180, ..PLAIN })).crossing_pairs, 0);
    assert_eq!(pair(&two_rows_with(&PARALLEL, Rows { rotation: 180, ..PLAIN })).crossing_pairs, 6);
}

fn through(number: &str, x: f64, y: f64, land: f64, shape: PadShape) -> FootprintPad {
    FootprintPad {
        kind: PadKind::ThroughHole,
        shape,
        drill: Some(Size { width_nm: mm(1.016), height_nm: mm(1.016) }),
        ..pad(number, x, y, land, land)
    }
}

#[test]
fn two_row_header_orders_pins_around_its_outline() {
    // A 2x4 header facing a single row: front-column nets in parallel do not
    // cross; a back-column net leaves around the header's end.
    let pads = (0..4)
        .flat_map(|y| (0..2).map(move |x| (x, y)))
        .map(|(x, y)| {
            through(&(2 * y + x + 1).to_string(), 2.54 * x as f64, 2.54 * y as f64, 1.7272, PadShape::Rectangle)
        })
        .collect();
    let header = footprint("header", pads, 5.0, 10.0);
    let mut nets = vec![net("TOP", &[("A", "1"), ("B", "1")])];
    for y in 0..4 {
        nets.push(net(&format!("F{y}"), &[("A", &(2 * y + 2).to_string()), ("B", &(y + 2).to_string())]));
    }
    let mut header_board = board(
        "header",
        40.0,
        30.0,
        vec![header, row("row", 5, 2.0)],
        vec![placement("A", "header", 10.0, 10.0), placement("B", "row", 30.0, 9.0)],
        nets,
    );
    header_board.rules = rules();
    let result = pair(&header_board);
    assert_eq!(result.nets, strings(&["F0", "F1", "F2", "F3", "TOP"]));
    assert_eq!(result.crossing_pairs, 0);
    let mut swapped = header_board.clone();
    swapped.nets[0] = net("TOP", &[("A", "1"), ("B", "2")]);
    swapped.nets[1] = net("F0", &[("A", "2"), ("B", "1")]);
    assert_eq!(pair(&swapped).crossing_pairs, 1);
}

#[test]
fn back_side_part_is_mirrored_before_ordering() {
    let front = pair(&two_rows(&SWAPPED));
    let back = pair(&two_rows_with(&SWAPPED, Rows { side: BoardSide::Back, ..PLAIN }));
    assert!(front.crossing_pairs == 1 && back.crossing_pairs == 1);
}

/// `two_rows(SWAPPED)` with `tree` also reaching a third single-pad part C at (20, 25).
fn with_tree(tree: &str) -> PhysicalBoard {
    let mut board = two_rows(&SWAPPED);
    board.footprints.insert("single".to_owned(), row("single", 1, 1.0));
    board.placements.push(placement("C", "single", 20.0, 25.0));
    for net in &mut board.nets {
        if net.name == tree {
            net.pads.push(pref("C", "1"));
        }
    }
    board
}

#[test]
fn bundles_hold_only_nets_between_the_two_parts() {
    // N3 also reaches a third part: it is a tree, not part of the A-B bundle.
    assert_eq!(pair(&with_tree("N3")).nets, strings(&["N1", "N2", "N4"]));
}

#[test]
fn net_with_several_pins_leaves_from_the_pin_nearest_the_other_part() {
    let mut board = two_rows(&[(2, 2), (3, 3)]);
    board.nets.push(net("N14", &[("A", "1"), ("A", "4"), ("B", "4")]));
    board.placements[1] = placement("B", "row", 30.0, 14.0);
    // A.4 is nearer B than A.1, so N14 leaves below N3 and meets B.4 below B.3: no crossing.
    assert_eq!(pair(&board).crossing_pairs, 0);
}

#[test]
fn pairs_need_the_minimum_shared_nets() {
    let board = two_rows(&[(1, 2), (2, 1)]);
    let with = |minimum: i64| {
        escape_diagnostics(&board, &EscapePlanOptions { cluster_pair_minimum_nets: minimum, ..defaults() })
    };
    assert_eq!(with(2).expect("diagnostics").pairs.len(), 1);
    assert!(with(3).expect("diagnostics").pairs.is_empty());
    assert!(with(0).is_err());
}

#[test]
fn crossing_causes_name_every_net_of_a_crossing_bundle() {
    let diagnostics = escape_diagnostics(&two_rows(&SWAPPED), &defaults()).expect("diagnostics");
    let causes = diagnostics.net_causes();
    assert_eq!(causes.len(), 4);
    for n in 1..=4 {
        assert_eq!(causes[&format!("N{n}")], strings(&["two_end_crossing"]));
    }
    assert!(escape_diagnostics(&two_rows(&PARALLEL), &defaults()).expect("diagnostics").net_causes().is_empty());
    let report = diagnostics.report();
    assert_eq!(report.get("crossing_pairs"), Some(&Value::from(1i64)));
    let components = report.get("pairs").and_then(Value::as_array).expect("pairs")[0].get("components").cloned();
    assert_eq!(components, Some(Value::Array(vec![Value::from("A"), Value::from("B")])));
}

// --------------------------------------------------------------------------- channel load

/// A 3x3 field at `pitch` mm with `land` mm pads.
fn grid(pitch: f64, land: f64, kind: PadKind, shape: PadShape) -> PhysicalFootprint {
    let pads = (0..3)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .map(|(x, y)| {
            let number = (y * 3 + x + 1).to_string();
            let (px, py) = (pitch * (x - 1) as f64, pitch * (y - 1) as f64);
            if kind == PadKind::ThroughHole {
                through(&number, px, py, land, shape)
            } else {
                FootprintPad { shape, ..pad(&number, px, py, land, land) }
            }
        })
        .collect();
    footprint("grid", pads, 8.0, 8.0)
}

fn smd_grid(pitch: f64, land: f64) -> PhysicalFootprint {
    grid(pitch, land, PadKind::Smd, PadShape::Circle)
}

/// A 3x3 field at (10, 10); the centre pin's net N5 and each ring pin's net reach a far part.
fn grid_board(field: PhysicalFootprint, centre_rule: Option<NetRoutingRule>) -> PhysicalBoard {
    let pads: Vec<String> = field.pads.iter().map(|p| p.number.clone()).collect();
    let far = footprint(
        "far",
        pads.iter().map(|n| pad(n, 0.0, n.parse::<f64>().expect("number"), 0.5, 0.5)).collect(),
        1.0,
        10.0,
    );
    let nets = pads.iter().map(|n| net(&format!("N{n}"), &[("U", n), ("J", n)])).collect();
    let mut board = board(
        "grid",
        40.0,
        30.0,
        vec![field, far],
        vec![placement("U", "grid", 10.0, 10.0), placement("J", "far", 30.0, 5.0)],
        nets,
    );
    board.rules = rules();
    board.net_routing_rules = centre_rule.into_iter().collect();
    board
}

fn cluster(board: &PhysicalBoard) -> ClusterEscape {
    escape_diagnostics(board, &defaults())
        .expect("diagnostics")
        .clusters
        .into_iter()
        .find(|item| item.component == "U")
        .expect("cluster U")
}

#[test]
fn header_gap_holds_one_track_per_layer() {
    // 2.54 mm pitch, 1.7272 mm lands: 0.8128 mm gaps hold 1 track each.
    let result = cluster(&grid_board(grid(2.54, 1.7272, PadKind::ThroughHole, PadShape::Rectangle), None));
    assert_eq!(result.inner_signal_pins, [pref("U", "5")]);
    let channels: Vec<_> = result.channels.iter().map(|c| (c.layer, c.demand, c.capacity, c.gaps)).collect();
    assert_eq!(channels, [(F, 1, 8, 8), (B, 1, 8, 8)]);
    assert_eq!(result.overflow(), 0);
}

#[test]
fn tight_ring_overflows_and_wider_rules_shrink_capacity() {
    // 1.0 mm pitch, 0.5 mm lands: 0.5 mm gaps hold no track.
    let tight = cluster(&grid_board(smd_grid(1.0, 0.5), None));
    let channels: Vec<_> = tight.channels.iter().map(|c| (c.layer, c.demand, c.capacity)).collect();
    assert_eq!(channels, [(F, 1, 0)]);
    assert_eq!(tight.overflow(), 1);
    let diagnostics = escape_diagnostics(&grid_board(smd_grid(1.0, 0.5), None), &defaults()).expect("diagnostics");
    assert!(diagnostics.net_causes()["N5"].contains(&"channel_overflow".to_owned()));
    // 1.6 mm pitch: 1.1 mm gaps hold 1 track each at default rules, none for a 0.6 mm net.
    assert_eq!(cluster(&grid_board(smd_grid(1.6, 0.5), None)).channels[0].capacity, 8);
    let wide = cluster(&grid_board(smd_grid(1.6, 0.5), Some(NetRoutingRule { width_nm: Some(mm(0.6)), ..rule("N5") })));
    assert_eq!(wide.channels[0].capacity, 0);
    assert_eq!(wide.overflow(), 1);
}

#[test]
fn through_hole_inner_pins_may_use_any_layer() {
    // 2.0 mm pitch, 1.6 mm lands: 0.4 mm gaps hold no 0.254 mm track, on either layer.
    let blocked = cluster(&grid_board(grid(2.0, 1.6, PadKind::ThroughHole, PadShape::Circle), None));
    let channels: Vec<_> = blocked.channels.iter().map(|c| (c.demand, c.capacity)).collect();
    assert_eq!(channels, [(1, 0), (1, 0)]);
    assert_eq!(blocked.overflow(), 1, "one pin short, not one per layer");
}

#[test]
fn inner_land_that_holds_a_via_is_not_ring_demand() {
    // A 1.0 mm centre land holds the 0.8 mm default via: an exposed pad leaves through its own via.
    let mut exposed = smd_grid(1.0, 0.5);
    exposed.pads[4].size = Size { width_nm: mm(1.0), height_nm: mm(1.0) };
    let result = cluster(&grid_board(exposed, None));
    assert!(result.inner_signal_pins.is_empty() && result.overflow() == 0);
}

#[test]
fn zone_nets_and_unconnected_pins_are_not_channel_demand() {
    let mut lone = grid_board(smd_grid(1.0, 0.5), None);
    lone.nets.retain(|net| net.name != "N5");
    let result = cluster(&lone);
    assert!(result.inner_signal_pins.is_empty() && result.channels.is_empty());
}

#[test]
fn diagnostics_are_deterministic_and_leave_the_board_unchanged() {
    let board = two_rows(&[(1, 3), (2, 1), (3, 4), (4, 2)]);
    let before = board.clone();
    let first = escape_diagnostics(&board, &defaults()).expect("diagnostics");
    assert_eq!(first, escape_diagnostics(&board, &defaults()).expect("diagnostics"));
    assert_eq!(first.report(), escape_diagnostics(&board, &defaults()).expect("diagnostics").report());
    assert_eq!(board, before);
}

// --------------------------------------------------------------------------- channel model (E1)

/// `footprint` placed alone as U at (20, 20), each pad on its own one-pad net.
fn placed(field: PhysicalFootprint, rotation: i64, rules: DesignRules) -> PhysicalBoard {
    let nets = field.pads.iter().map(|p| net(&format!("N{}", p.number), &[("U", &p.number)])).collect();
    let name = field.name.clone();
    let mut board = board("field", 40.0, 40.0, vec![field], vec![rotated("U", &name, 20.0, 20.0, rotation)], nets);
    board.rules = rules;
    board
}

/// `columns` x `rows` pads at `pitch` mm, numbered row by row, without `missing`.
fn lattice(
    columns: i64,
    rows: i64,
    pitch: f64,
    land: f64,
    shape: PadShape,
    kind: PadKind,
    missing: &[i64],
) -> PhysicalFootprint {
    let mut pads = Vec::new();
    for y in 0..rows {
        for x in 0..columns {
            let number = y * columns + x + 1;
            if missing.contains(&number) {
                continue;
            }
            let (px, py) = (pitch * x as f64, pitch * y as f64);
            pads.push(if kind == PadKind::ThroughHole {
                through(&number.to_string(), px, py, land, shape)
            } else {
                FootprintPad { shape, ..pad(&number.to_string(), px, py, land, land) }
            });
        }
    }
    footprint("lattice", pads, 10.0, 10.0)
}

fn fields(board: &PhysicalBoard) -> Vec<PadField> {
    pad_fields(board, "U", &defaults()).expect("fields")
}

fn only(board: &PhysicalBoard) -> PadField {
    let mut all = fields(board);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

fn minimum(field: &PadField, kind: &str, rules: (i64, i64)) -> i64 {
    let report = field.report(rules.0, rules.1);
    let value = report.get("minimum_capacity").and_then(|m| m.get(kind)).expect("minimum");
    int_of(value)
}

const WIDE: (i64, i64) = (254_000, 254_000);
const FINE: (i64, i64) = (100_000, 100_000);

#[test]
fn header_lattice_capacities_match_hand_calculation() {
    // 2x4 header, 2.54 mm pitch, 1.7272 mm square lands: orthogonal gaps
    // 0.8128 mm hold 1 track; the corner-to-corner diagonal gap 1.1495 mm also 1.
    for rotation in [0, 45, 90] {
        let header = lattice(2, 4, 2.54, 1.7272, PadShape::Rectangle, PadKind::ThroughHole, &[]);
        let field = only(&placed(header, rotation, rules()));
        assert_eq!(
            (field.kind.as_str(), field.columns, field.rows, field.pitch_nm),
            ("lattice", 2, 4, Some((2_540_000, 2_540_000)))
        );
        let count = |kind: &str| field.gaps.iter().filter(|g| g.kind == kind).count();
        assert_eq!((count("row"), count("column"), count("diagonal")), (4, 6, 6));
        assert_eq!(
            (minimum(&field, "row", WIDE), minimum(&field, "column", WIDE), minimum(&field, "diagonal", WIDE)),
            (1, 1, 1)
        );
        assert_eq!(field.tiles.len(), 3);
        assert!(field.gaps.iter().all(|g| g.layers.iter().collect::<IndexSet<_>>() == IndexSet::from([&F, &B])));
    }
}

#[test]
fn round_lands_leave_more_room_on_the_diagonal() {
    // Round 1.7272 mm lands: diagonal gap 1.8649 mm holds 3 tracks.
    let field = only(&placed(lattice(2, 4, 2.54, 1.7272, PadShape::Circle, PadKind::ThroughHole, &[]), 0, rules()));
    assert_eq!((minimum(&field, "row", WIDE), minimum(&field, "diagonal", WIDE)), (1, 3));
}

#[test]
fn bga_capacities_and_via_sites() {
    // 0.8 mm pitch, 0.4 mm balls, 0.1 mm rules: orthogonal gap 0.4 mm holds
    // 1 track, diagonal 0.7314 mm holds 3. A 0.45 mm via fits a tile centre; a 0.6 mm one does not.
    let bga = || lattice(4, 4, 0.8, 0.4, PadShape::Circle, PadKind::Smd, &[]);
    let field = only(&placed(bga(), 0, fine()));
    assert_eq!((minimum(&field, "row", FINE), minimum(&field, "diagonal", FINE)), (1, 3));
    assert!(field.tiles.len() == 9 && field.tiles.iter().all(|t| t.via_site));
    let wide_via = DesignRules { default_via_size_nm: 600_000, default_via_drill_nm: 300_000, ..fine() };
    assert!(!only(&placed(bga(), 0, wide_via)).tiles.iter().any(|t| t.via_site));
}

#[test]
fn missing_balls_leave_corners_and_their_gaps_open() {
    let field = only(&placed(lattice(4, 4, 0.8, 0.4, PadShape::Circle, PadKind::Smd, &[6]), 0, fine()));
    assert!(field.kind == "lattice" && field.pads.len() == 15);
    let first = field.tiles.iter().find(|t| (t.column, t.row) == (0, 0)).expect("tile");
    assert!(first.corners[2].is_none(), "SE corner: lattice position (1, 1), pad 6");
    assert!(first.sides[2].is_none() && first.sides[3].is_none(), "E and S sides end at the missing ball");
    assert!(first.diagonals[0].is_none() && first.diagonals[1].is_some());
    // Fewer than half the positions filled is not a lattice.
    let sparse = lattice(4, 4, 0.8, 0.4, PadShape::Circle, PadKind::Smd, &[6, 7, 8, 10, 11, 12, 14, 15, 16]);
    let sparse = only(&placed(sparse, 0, fine()));
    assert!(sparse.kind == "perimeter" && sparse.tiles.is_empty());
}

#[test]
fn perimeter_row_and_split_fields() {
    let mut sides: Vec<(f64, f64)> = Vec::new();
    sides.extend((0..5).map(|k| (0.5 * k as f64 - 1.0, -2.0)));
    sides.extend((0..5).map(|k| (0.5 * k as f64 - 1.0, 2.0)));
    sides.extend((0..5).map(|k| (-2.0, 0.5 * k as f64 - 1.0)));
    sides.extend((0..5).map(|k| (2.0, 0.5 * k as f64 - 1.0)));
    let qfn = footprint(
        "qfn",
        sides.iter().enumerate().map(|(i, (x, y))| pad(&(i + 1).to_string(), *x, *y, 0.25, 0.25)).collect(),
        5.0,
        5.0,
    );
    let field = only(&placed(qfn, 0, rules()));
    assert!(field.kind == "perimeter" && field.gaps.iter().all(|g| g.kind == "ring") && field.gaps.len() == 20);
    let single = only(&placed(row("row", 4, 1.0), 0, rules()));
    assert!(single.kind == "row" && single.gaps.len() == 3);
    // Two 1 mm-pitch columns 12 mm apart are separate fields; 2 mm apart, one.
    let edges = |spacing: f64, width: f64| {
        let pads =
            (0..6).map(|i| pad(&(i + 1).to_string(), spacing * (i / 3) as f64, (i % 3) as f64, 0.5, 0.5)).collect();
        footprint("edges", pads, width, 4.0)
    };
    assert_eq!(fields(&placed(edges(12.0, 14.0), 0, rules())).iter().map(|f| f.pads.len()).collect::<Vec<_>>(), [3, 3]);
    let near = placed(edges(2.0, 4.0), 0, rules());
    assert_eq!(fields(&near).iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), ["lattice"]);
    assert!(pad_fields(&near, "missing", &defaults()).is_err());
}

#[test]
fn escape_rules_neck_down_inside_the_breakout_region() {
    let board = placed(row("row", 2, 1.0), 0, rules());
    assert_eq!(channel_rules(&board, "N1", None), (mm(0.254), mm(0.254)));
    let wide = NetRoutingRule { width_nm: Some(mm(0.5)), clearance_nm: Some(mm(0.3)), ..rule("N1") };
    assert_eq!(channel_rules(&board, "N1", Some(&wide)), (mm(0.5), mm(0.3)));
    let necked = NetRoutingRule {
        breakout_length_nm: Some(mm(1.0)),
        breakout_width_nm: Some(mm(0.15)),
        breakout_clearance_nm: Some(mm(0.1)),
        ..wide
    };
    // The breakout clearance is still floored at the board minimum.
    assert_eq!(channel_rules(&board, "N1", Some(&necked)), (mm(0.15), mm(0.254)));
}

fn tile(sides: [i64; 4], diagonals: [i64; 2], corners: bool) -> Tile {
    // 0.1 mm track and clearance: capacity n needs a (0.2n + 0.1) mm gap.
    let gap = |capacity: i64, kind: &str| Gap {
        first: pref("U", "1"),
        second: pref("U", "2"),
        kind: kind.to_owned(),
        gap_nm: 200_000 * capacity + 100_000,
        layers: vec![F],
    };
    let corner = corners.then(|| pref("U", "1"));
    Tile {
        column: 0,
        row: 0,
        corners: [corner.clone(), corner.clone(), corner.clone(), corner],
        sides: sides.map(|c| Some(gap(c, "row"))),
        diagonals: diagonals.map(|c| Some(gap(c, "diagonal"))),
        centre: Point::new(0, 0),
        via_site: false,
    }
}

fn admits(t: &Tile, flows: &[((&str, &str), i64)]) -> bool {
    t.admits(flows, F, 100_000, 100_000).expect("flows")
}

const ONE: [i64; 4] = [1, 1, 1, 1];

#[test]
fn tile_diagonal_gaps_bound_tracks_between_their_halves() {
    // Turning round opposite corners both tracks pass between NW and SE.
    assert!(!admits(&tile(ONE, [1, 1], true), &[(("W", "N"), 1), (("S", "E"), 1)]));
    assert!(admits(&tile(ONE, [2, 1], true), &[(("W", "N"), 1), (("S", "E"), 1)]));
    // Two tracks from a two-track west side turning north and south use different diagonals.
    assert!(admits(&tile([2, 1, 1, 1], [1, 1], true), &[(("W", "N"), 1), (("W", "S"), 1)]));
    // A straight track crosses both diagonals.
    assert!(admits(&tile(ONE, [1, 1], true), &[(("W", "E"), 1)]));
    assert!(!admits(&tile(ONE, [1, 0], true), &[(("W", "E"), 1)]));
    // Side gaps bound every track through them; a pin starts one track, a missing pin none.
    assert!(!admits(&tile(ONE, [1, 1], true), &[(("W", "N"), 1), (("W", "E"), 1)]));
    assert!(admits(&tile(ONE, [1, 1], true), &[(("NW", "E"), 1)]));
    assert!(!admits(&tile(ONE, [1, 1], true), &[(("NW", "E"), 1), (("NW", "S"), 1)]));
    assert!(!admits(&tile(ONE, [1, 1], false), &[(("NW", "E"), 1)]));
    // The NW pin lies on the NW-SE diagonal: its track to S crosses only NE-SW.
    assert!(admits(&tile(ONE, [0, 1], true), &[(("NW", "S"), 1)]));
    // Gaps on another layer do not bound this one.
    assert!(tile([0; 4], [0, 0], true).admits(&[(("W", "E"), 1)], B, 100_000, 100_000).expect("flows"));
    assert!(tile(ONE, [1, 1], true).admits(&[(("W", "W"), 1)], F, 100_000, 100_000).is_err());
}

#[test]
fn cluster_reports_carry_field_summaries() {
    // 1 mm pitch, 0.5 mm balls: orthogonal gaps hold no track, diagonal ones one.
    let report = escape_diagnostics(&grid_board(smd_grid(1.0, 0.5), None), &defaults()).expect("diagnostics").report();
    let clusters = report.get("clusters").and_then(Value::as_array).expect("clusters");
    assert_eq!(clusters.len(), 1);
    let expected = copper_json::parse(
        r#"[{"pads": 9, "kind": "lattice", "columns": 3, "rows": 3, "pitch_nm": [1000000, 1000000],
             "gaps": {"column": 6, "diagonal": 8, "row": 6}, "minimum_capacity": {"column": 0, "diagonal": 1, "row": 0},
             "tiles": 4, "via_sites": 0}]"#,
    )
    .expect("JSON");
    assert_eq!(clusters[0].get("fields"), Some(&expected));
}

// --------------------------------------------------------------------------- two-end plan (E3)

fn plan(board: &PhysicalBoard) -> copper_route::escape_plan::EscapePlan {
    plan_two_end_escapes(board, &defaults(), None).expect("plan")
}

#[test]
fn plan_keeps_the_planar_subset_and_moves_the_rest() {
    let plan = plan(&two_rows(&SWAPPED));
    assert_eq!(plan.pairs.len(), 1);
    let pair = &plan.pairs[0];
    assert_eq!((pair.first.as_str(), pair.second.as_str(), pair.planar_layer), ("A", "B", F));
    assert!(pair.moved.len() == 1 && pair.moved[0].1 == B);
    let moved = pair.moved[0].0.clone();
    let kept: IndexSet<&str> = pair.kept.iter().map(String::as_str).collect();
    let expected: IndexSet<&str> = ["N1", "N2", "N3", "N4"].into_iter().filter(|n| *n != moved).collect();
    assert_eq!(kept, expected);
    assert!(pair.unmoved.is_empty());
    // Moved nets first, then kept nets from the outside of the bundle in.
    assert_eq!(plan.net_order[0], moved);
    assert_eq!(plan.net_order[1..], [pair.kept[0].clone(), pair.kept[2].clone(), pair.kept[1].clone()]);
    // A moved net pays off its planned layer, only inside the corridor around both parts' bundle pins.
    let biases: IndexSet<(&str, CopperLayer)> = plan.layer_biases.iter().map(|b| (b.net.as_str(), b.layer)).collect();
    assert_eq!(biases, IndexSet::from([(moved.as_str(), F)]));
    assert_eq!(pair.corridor, Bounds { min_x: mm(9.0), min_y: mm(9.0), max_x: mm(31.0), max_y: mm(14.0) });
    let report = plan.report();
    let moved_report = report.get("pairs").and_then(Value::as_array).expect("pairs")[0].get("moved").cloned();
    assert_eq!(moved_report, Some(Value::Object([(moved.clone(), Value::from("B.Cu"))].into_iter().collect())));
}

#[test]
fn plan_covers_every_net_joining_a_crossing_pair() {
    // N4 also reaches a third part, so it is no bundle net for the
    // diagnostics, but the plan orders it and gives it a layer with the bundle.
    let board = with_tree("N4");
    assert_eq!(pair(&board).nets, strings(&["N1", "N2", "N3"]));
    let plan = plan(&board);
    assert_eq!(plan.pairs.len(), 1);
    let pair = &plan.pairs[0];
    assert!(pair.kept.contains(&"N4".to_owned()) && plan.net_order.contains(&"N4".to_owned()));
    // Kept bundle nets escape on their surface; the tree keeps its dogbone towards its third pad.
    let kept_bundle: IndexSet<String> = pair.kept.iter().filter(|n| *n != "N4").map(|n| n[1..].to_owned()).collect();
    let surface: IndexSet<String> =
        plan.surface_pads.iter().filter(|p| p.component == "A").map(|p| p.pad.clone()).collect();
    assert_eq!(surface, kept_bundle);
    assert!(!plan.surface_pads.contains(&pref("A", "4")));
}

#[test]
fn plan_ignores_bundles_that_do_not_cross_or_have_no_shared_surface() {
    assert_eq!(plan(&two_rows(&PARALLEL)), plan(&two_rows(&[])));
    // A on the back, B on the front: the bundle has no layer both ends reach without a via.
    let opposite = plan(&two_rows_with(&SWAPPED, Rows { side: BoardSide::Back, ..PLAIN }));
    assert!(opposite.pairs.is_empty() && opposite.net_order.is_empty());
}

#[test]
fn net_without_another_legal_layer_stays_unmoved() {
    let mut board = two_rows(&SWAPPED);
    board.net_routing_rules =
        board.nets.iter().map(|n| NetRoutingRule { allowed_layers: vec![F], ..rule(&n.name) }).collect();
    let plan = plan(&board);
    assert!(plan.pairs[0].moved.is_empty() && plan.pairs[0].unmoved.len() == 1);
    assert!(plan.layer_biases.is_empty());
}

#[test]
fn layer_bias_charges_only_track_whose_midpoint_is_in_its_area() {
    let bias = LayerBias {
        net: "SIG".to_owned(),
        layer: F,
        area: Bounds { min_x: 0, min_y: 0, max_x: mm(10.0), max_y: mm(10.0) },
        rate: 20,
    };
    // 20 cost units per millimetre: twice the cost of the track itself (10 per mm).
    assert_eq!(bias.cost(pt(1.0, 1.0), pt(3.0, 1.0)), 2 * 20 * 1_000_000);
    assert_eq!(bias.cost(pt(12.0, 1.0), pt(14.0, 1.0)), 0);
    assert!(bias.cost(pt(9.0, 1.0), pt(10.0, 1.0)) > 0 && bias.cost(pt(10.0, 1.0), pt(12.0, 1.0)) == 0);
}

#[test]
fn fanout_keeps_planned_surface_pins_off_dogbone_vias() {
    use copper_route::fanout::{FanoutOptions, route_fanout};
    let board = two_rows(&PARALLEL);
    let plain = route_fanout(&board, Some(&FanoutOptions::default()), None).expect("fanout");
    assert!(plain.created_vias.iter().any(|v| v.net == "N2"));
    let options = FanoutOptions { surface_pads: [pref("A", "2")].into_iter().collect(), ..FanoutOptions::default() };
    let planned = route_fanout(&board, Some(&options), None).expect("fanout");
    assert!(!planned.created_vias.iter().any(|v| v.net == "N2" && v.position.x_nm < mm(20.0)));
    assert!(!planned.pending_pads.contains(&pref("A", "2")));
    assert!(planned.created_vias.iter().any(|v| v.net == "N3" && v.position.x_nm < mm(20.0)));
}

// --------------------------------------------------------------------------- array escape (E2)

/// A `size` x `size` 0.8 mm BGA of round SMD balls; every ball's net reaches
/// a far connector pad. Balls numbered in `plane` are on the GND plane net instead.
fn ball_grid(size: i64, ball_mm: f64, plane: &[i64]) -> PhysicalBoard {
    let count = size * size;
    let bga = lattice(size, size, 0.8, ball_mm, PadShape::Circle, PadKind::Smd, &[]);
    let bga =
        PhysicalFootprint { name: "bga".to_owned(), body_size: Size { width_nm: mm(8.0), height_nm: mm(8.0) }, ..bga };
    let far = footprint(
        "far",
        (0..count).map(|n| pad(&(n + 1).to_string(), 0.5 * n as f64, 0.0, 0.3, 0.3)).collect(),
        40.0,
        1.0,
    );
    let mut nets: Vec<PhysicalNet> = (1..=count)
        .filter(|n| !plane.contains(n))
        .map(|n| net(&format!("N{n}"), &[("U", &n.to_string()), ("J", &n.to_string())]))
        .collect();
    let mut zones = Vec::new();
    if !plane.is_empty() {
        let mut sorted = plane.to_vec();
        sorted.sort_unstable();
        let pads: Vec<String> = sorted.iter().map(|n| n.to_string()).collect();
        let refs: Vec<(&str, &str)> = pads.iter().map(|p| ("U", p.as_str())).collect();
        nets.push(net("GND", &refs));
        zones.push(zone("gnd", "GND", &[B], rect(0.0, 0.0, 60.0, 40.0)));
    }
    let mut board = board(
        "bga",
        60.0,
        40.0,
        vec![bga, far],
        vec![placement("U", "bga", 10.0, 10.0), placement("J", "far", 10.0, 30.0)],
        nets,
    );
    board.rules = fine();
    board.zones = zones;
    board
}

fn array_plan(board: &PhysicalBoard) -> ArrayEscape {
    let mut plans = plan_array_escapes(board, &defaults()).expect("array plans");
    assert_eq!(plans.len(), 1);
    plans.remove(0)
}

fn pad_numbers(pads: &[PadReference]) -> IndexSet<String> {
    pads.iter().map(|p| p.pad.clone()).collect()
}

#[test]
fn one_track_per_gap_lets_a_four_by_four_grid_escape_on_the_surface() {
    // 0.45 mm balls leave 0.35 mm gaps holding one track: the 12 edge balls
    // leave directly; the 4 inner balls each reach one of 12 outer gaps.
    let plan = array_plan(&ball_grid(4, 0.45, &[]));
    assert!(plan.layer == F && plan.surface.len() == 16 && plan.vias.is_empty());
    assert!(plan.tile_load.values().sum::<i64>() >= 4);
}

#[test]
fn closed_gaps_leave_only_the_edge_balls_on_the_surface() {
    // 0.55 mm balls leave 0.25 mm gaps: no track fits, so every inner ball needs a via.
    let plan = array_plan(&ball_grid(4, 0.55, &[]));
    assert_eq!(plan.surface.len(), 12);
    assert_eq!(
        pad_numbers(&plan.vias),
        ["6", "7", "10", "11"].iter().map(|s| s.to_string()).collect::<IndexSet<String>>()
    );
    assert!(plan.tile_load.values().all(|load| *load == 0));
}

#[test]
fn outer_gaps_bound_the_surface_escapes_of_an_eight_by_eight_grid() {
    // 28 edge balls leave directly; the 36 inner balls must cross the 28 outer gaps.
    let plan = array_plan(&ball_grid(8, 0.45, &[]));
    assert!(plan.surface.len() == 56 && plan.vias.len() == 8);
    let inner: IndexSet<String> = (2..6).flat_map(|y| (2..6).map(move |x| (y * 8 + x + 1).to_string())).collect();
    assert!(pad_numbers(&plan.vias).is_subset(&inner));
}

#[test]
fn plane_balls_do_not_count_and_free_their_gaps() {
    let plan = array_plan(&ball_grid(8, 0.45, &[28, 29, 36, 37]));
    assert!(plan.surface.len() + plan.vias.len() == 60 && plan.vias.len() == 4);
    let all: IndexSet<String> = pad_numbers(&plan.surface).union(&pad_numbers(&plan.vias)).cloned().collect();
    assert!(["28", "29", "36", "37"].iter().all(|p| !all.contains(*p)));
}

#[test]
fn array_plans_are_deterministic_and_skip_other_fields() {
    let board = ball_grid(8, 0.45, &[]);
    assert_eq!(
        plan_array_escapes(&board, &defaults()).expect("plans"),
        plan_array_escapes(&board, &defaults()).expect("plans")
    );
    assert!(plan_array_escapes(&two_rows(&[(1, 1), (2, 2)]), &defaults()).expect("plans").is_empty());
}

#[test]
fn bga_fanout_follows_the_array_plan() {
    use copper_route::bga_fanout::plan_bga_fanouts;
    let board = ball_grid(8, 0.45, &[]);
    let plan = array_plan(&board);
    let rings = plan_bga_fanouts(&board, 2, None, None).expect("fanout");
    let plans = IndexMap::from([("U".to_owned(), plan.clone())]);
    let planned = plan_bga_fanouts(&board, 2, None, Some(&plans)).expect("fanout");
    // The ring rule fans out every ball deeper than two rings (16); the plan only the 8 it needs.
    assert_eq!(rings.fanned.len(), 16);
    assert_eq!(planned.fanned.iter().collect::<IndexSet<_>>(), plan.vias.iter().collect::<IndexSet<_>>());
    assert_eq!(planned.surface.iter().collect::<IndexSet<_>>(), plan.surface.iter().collect::<IndexSet<_>>());
    let origin = copper_pcb::transform::transformed_local_point(&board.placements[0], Point::new(0, 0));
    let cell = |p: Point| ((p.x_nm - origin.x_nm).div_euclid(mm(0.8)), (p.y_nm - origin.y_nm).div_euclid(mm(0.8)));
    // Every planned via sits in a cell no planned surface track crosses, when one is free.
    assert!(!planned.vias.iter().any(|v| plan.tile_load.get(&cell(v.position)).copied().unwrap_or(0) != 0));
    // With a large plane block in the middle, some vias move to a free cell.
    let plane_board = ball_grid(8, 0.45, &(19..47).collect::<Vec<_>>());
    let plane_plan = array_plan(&plane_board);
    let plans = IndexMap::from([("U".to_owned(), plane_plan.clone())]);
    let plane_fanout = plan_bga_fanouts(&plane_board, 2, None, Some(&plans)).expect("fanout");
    // A cell centre serves four balls: two same-net neighbours never share one via site.
    let sites: Vec<Point> = plane_fanout.vias.iter().map(|v| v.position).collect();
    assert_eq!(sites.len(), 28);
    assert_eq!(sites.iter().collect::<IndexSet<_>>().len(), 28);
    // A via lands in a loaded cell only when each of its ball's four cells is loaded or already holds a via.
    let held: IndexSet<(i64, i64)> = sites.iter().map(|p| cell(*p)).collect();
    for (ball, via) in plane_fanout.fanned.iter().zip(&plane_fanout.vias) {
        if plane_plan.tile_load.get(&cell(via.position)).copied().unwrap_or(0) != 0 {
            let index: i64 = ball.pad.parse::<i64>().expect("number") - 1;
            let (column, row) = (index % 8, index / 8);
            for around in [(column - 1, row - 1), (column, row - 1), (column - 1, row), (column, row)] {
                assert!(plane_plan.tile_load.get(&around).copied().unwrap_or(0) != 0 || held.contains(&around));
            }
        }
    }
}

// --------------------------------------------------------------------------- bus layers (E5)

fn four(mut board: PhysicalBoard) -> PhysicalBoard {
    board.stackup = Stackup::new(layers(4));
    board
}

fn bus_options() -> EscapePlanOptions {
    EscapePlanOptions { two_end_order: true, bus_layers: true, ..defaults() }
}

#[test]
fn bus_layers_give_each_layer_a_non_crossing_subset() {
    // A fully reversed bundle of four: E3 sends three nets to one layer,
    // where they still cross; bus layers put one on each further layer.
    let board = four(two_rows(&REVERSED));
    let single = plan(&board).pairs.remove(0);
    assert_eq!(single.moved.iter().map(|m| m.1).collect::<IndexSet<_>>(), IndexSet::from([CopperLayer::In1]));
    assert_eq!(single.moved.len(), 3);
    let layered = plan_two_end_escapes(&board, &bus_options(), None).expect("plan").pairs.remove(0);
    assert!(layered.kept.len() == 1 && layered.unmoved.is_empty());
    let mut moved: Vec<&str> = layered.moved.iter().map(|m| m.1.value()).collect();
    moved.sort_unstable();
    assert_eq!(moved, ["B.Cu", "In1.Cu", "In2.Cu"]);
}

#[test]
fn bus_layers_keep_each_layer_free_of_crossings() {
    // Two interleaved halves: N1..N3 parallel, N4..N6 reversed among themselves.
    let mut board = four(two_rows(&PARALLEL));
    board.footprints = IndexMap::from([("row".to_owned(), row("row", 6, 1.0))]);
    board.nets = [(1, 1), (2, 3), (3, 5), (4, 6), (5, 4), (6, 2)]
        .iter()
        .map(|(a, b)| net(&format!("N{a}"), &[("A", &a.to_string()), ("B", &b.to_string())]))
        .collect();
    let plan = plan_two_end_escapes(&board, &bus_options(), None).expect("plan");
    let pair = &plan.pairs[0];
    let mut by_layer: IndexMap<CopperLayer, Vec<String>> = IndexMap::from([(F, pair.kept.clone())]);
    for (net, layer) in &pair.moved {
        by_layer.entry(*layer).or_default().push(net.clone());
    }
    let diagnostics = self::pair(&board);
    let second: IndexMap<&String, _> = diagnostics.order.iter().zip(diagnostics.second_keys.iter()).collect();
    for nets in by_layer.values() {
        let ranked: Vec<_> = diagnostics.order.iter().filter(|n| nets.contains(n)).map(|n| second[n].clone()).collect();
        let mut sorted = ranked.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
        assert_eq!(ranked, sorted);
    }
    assert_eq!(by_layer.values().map(Vec::len).sum::<usize>(), 6);
    assert!(pair.unmoved.is_empty());
}

#[test]
fn bus_layers_respect_the_via_budget() {
    // Both ends are surface lands: any other layer costs two vias.
    let budget = |vias: i64| {
        let mut board = four(two_rows(&REVERSED));
        board.net_routing_rules =
            board.nets.iter().map(|n| NetRoutingRule { max_vias: Some(vias), ..rule(&n.name) }).collect();
        plan_two_end_escapes(&board, &bus_options(), None).expect("plan").pairs.remove(0)
    };
    let one = budget(1);
    assert!(one.moved.is_empty() && one.unmoved.len() == 3);
    let two = budget(2);
    assert!(two.moved.len() == 3 && two.unmoved.is_empty());
}

#[test]
fn bus_layer_groups_route_layer_by_layer() {
    let plan = plan_two_end_escapes(&four(two_rows(&REVERSED)), &bus_options(), None).expect("plan");
    let pair = &plan.pairs[0];
    let layer_of: IndexMap<&String, CopperLayer> = pair.moved.iter().map(|(n, l)| (n, *l)).collect();
    let moved: Vec<&str> = plan.net_order.iter().filter_map(|n| layer_of.get(n)).map(|l| l.value()).collect();
    assert_eq!(moved, ["In1.Cu", "In2.Cu", "B.Cu"]);
    assert_eq!(plan.net_order.last(), Some(&pair.kept[0]));
}

#[test]
fn a_part_without_copper_pads_has_no_pad_field() {
    let hole = FootprintPad {
        number: String::new(),
        kind: PadKind::NonPlatedThroughHole,
        shape: PadShape::Circle,
        drill: Some(Size { width_nm: mm(3.0), height_nm: mm(3.0) }),
        ..pad("", 0.0, 0.0, 3.0, 3.0)
    };
    let mut board = two_rows(&[(1, 2), (2, 1)]);
    board.footprints.insert("hole".to_owned(), footprint("hole", vec![hole], 4.0, 4.0));
    board.placements.push(placement("H1", "hole", 20.0, 25.0));
    assert!(pad_fields(&board, "H1", &defaults()).expect("fields").is_empty());
    assert!(
        !escape_diagnostics(&board, &defaults()).expect("diagnostics").pairs.is_empty(),
        "the rest is still analysed"
    );
}

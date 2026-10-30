//! Congestion maps, the RUDY estimate, move proposals and the screen
//! (ported from the former `tests/test_congestion_feedback.py`). The Rust
//! module keeps only the API the pipeline uses: Python's `value_at`,
//! `combined`, `to_json`, `gradient_at(radius_tiles=)`, `estimate_demand(nets=)`
//! and `propose_moves(steps=, rotations=, limit=)` have no counterpart, so
//! those assertions are dropped (see `PORTED-routers.md`).

mod support_routers;

use copper_pcb::board::{
    ComponentPlacementRule, PhysicalBoard, PhysicalNet, Placement, RigidPlacementCluster, RigidPlacementMember, Stackup,
};
use copper_physical::footprint::footprint_geometry_digest;
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_place::PlannerOptions;
use copper_route::GlobalRouterOptions;
use copper_route::congestion_feedback::{
    CongestionMap, Proposal, UTILISATION_SENTINEL, combine_normalised, estimate_demand, propose_moves, screen,
};
use copper_units::Decimal;
use indexmap::IndexSet;
use support_routers::*;

const MM: i64 = 1_000_000;

fn grid(rows: &[&[f64]]) -> Vec<Vec<f64>> {
    rows.iter().map(|row| row.to_vec()).collect()
}

fn ones(ny: usize, nx: usize, value: f64) -> Vec<Vec<f64>> {
    vec![vec![value; nx]; ny]
}

fn cmap(demand: Vec<Vec<f64>>, capacity: Option<Vec<Vec<f64>>>) -> CongestionMap {
    let (ny, nx) = (demand.len(), demand[0].len());
    CongestionMap {
        tile_nm: MM,
        x0_nm: 0,
        y0_nm: 0,
        nx,
        ny,
        capacity: capacity.unwrap_or_else(|| ones(ny, nx, 1.0)),
        demand,
        source: "rudy".to_owned(),
        failed_points: Vec::new(),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * b.abs().max(1e-12)
}

// ---------------------------------------------------------------- CongestionMap

#[test]
fn utilisation_sentinel_and_empty_tiles() {
    let m = cmap(grid(&[&[1.0, 0.0], &[0.0, 2.0]]), Some(grid(&[&[0.0, 0.0], &[1.0, 4.0]])));
    let u = m.utilisation();
    assert_eq!(u[0][0], UTILISATION_SENTINEL);
    assert!(u.iter().flatten().all(|v| v.is_finite()));
    assert!(u[0][1] == 0.0 && u[1][0] == 0.0 && u[1][1] == 0.5);
}

#[test]
fn tile_lookup_and_bounds() {
    let demand: Vec<Vec<f64>> = (0..2).map(|y| (0..3).map(|x| (y * 3 + x) as f64).collect()).collect();
    let m = CongestionMap { x0_nm: 10 * MM, y0_nm: 20 * MM, ..cmap(demand, None) };
    assert_eq!(m.tile_of(pt(11.5, 21.2)), (1, 1));
    assert_eq!(m.tile_of(pt(-50.0, 100.0)), (0, 1), "clipped to the lattice");
    assert_eq!(m.tile_bounds(2, 1), (12 * MM, 21 * MM, 13 * MM, 22 * MM));
}

fn ramp(transpose: bool) -> Vec<Vec<f64>> {
    (0..9).map(|y| (0..9).map(|x| if transpose { y as f64 } else { x as f64 }).collect()).collect()
}

#[test]
fn gradient_points_down_a_ramp_and_is_zero_when_flat() {
    let (dx, dy) = cmap(ramp(false), None).gradient_at(pt(4.5, 4.5));
    assert!(close(dx, -1.0) && dy.abs() < 1e-9);
    let (dx, dy) = cmap(ramp(true), None).gradient_at(pt(4.5, 4.5));
    assert!(close(dy, -1.0) && dx.abs() < 1e-9);
    // The diagonal ramp at a clipped corner window (Python used radius 3; the
    // fit of an exact plane does not depend on the window).
    let diagonal: Vec<Vec<f64>> = (0..9).map(|y| (0..9).map(|x| (x + y) as f64).collect()).collect();
    let (dx, dy) = cmap(diagonal, None).gradient_at(pt(0.5, 0.5));
    assert!(close(dx, -(2f64.powf(-0.5))) && close(dy, -(2f64.powf(-0.5))));
    assert!(close(dx.hypot(dy), 1.0));
    assert_eq!(cmap(ones(5, 5, 0.3), None).gradient_at(pt(2.5, 2.5)), (0.0, 0.0));
}

#[test]
fn gradient_ignores_tiles_without_capacity() {
    // Rises toward the board edge at x = 3; off the board: zero demand, but no capacity either.
    let demand: Vec<Vec<f64>> = (0..5).map(|_| vec![0.0, 1.0, 2.0, 0.0, 0.0]).collect();
    let capacity: Vec<Vec<f64>> = (0..5).map(|_| vec![1.0, 1.0, 1.0, 0.0, 0.0]).collect();
    let (dx, _) = cmap(demand, Some(capacity)).gradient_at(pt(2.5, 2.5));
    assert!(close(dx, -1.0), "{dx}");
}

#[test]
fn hot_tiles_order_is_utilisation_then_row_then_column() {
    let mut demand = ones(3, 3, 0.0);
    demand[2][0] = 0.5;
    demand[0][2] = 0.5;
    demand[1][1] = 0.5;
    demand[2][2] = 0.9;
    let m = cmap(demand, None);
    assert_eq!(m.hot_tiles(10), [(2, 2, 0.9), (2, 0, 0.5), (1, 1, 0.5), (0, 2, 0.5)]);
    assert_eq!(m.hot_tiles(2), [(2, 2, 0.9), (2, 0, 0.5)]);
    assert!(m.hot_tiles(0).is_empty());
}

#[test]
fn combined_maps_require_the_same_lattice_and_keep_the_primary_capacity() {
    // Python's raw `combined` is not ported; the pipeline's
    // `combine_normalised` has the same lattice, capacity and failure-point contract.
    let (p, q) = (pt(1.0, 1.0), pt(2.0, 2.0));
    let a = CongestionMap {
        source: "unified_router".to_owned(),
        failed_points: vec![("A".to_owned(), p)],
        ..cmap(ones(2, 2, 1.0), Some(ones(2, 2, 4.0)))
    };
    let b = CongestionMap {
        failed_points: vec![("A".to_owned(), p), ("B".to_owned(), q)],
        ..cmap(ones(2, 2, 2.0), Some(ones(2, 2, 9.0)))
    };
    let c = combine_normalised(&a, &b, 0.5).expect("combined");
    assert_eq!(c.source, "combined");
    assert_eq!(c.capacity, a.capacity);
    assert_eq!(c.failed_points, [("A".to_owned(), p), ("B".to_owned(), q)]);
    assert!(combine_normalised(&a, &CongestionMap { x0_nm: 1, ..cmap(ones(2, 2, 1.0), None) }, 1.0).is_err());
    assert!(combine_normalised(&a, &cmap(ones(2, 3, 1.0), None), 1.0).is_err());
}

// ---------------------------------------------------------------- estimator

fn small_pad() -> copper_physical::footprint::PhysicalFootprint {
    footprint("pad", vec![pad("1", 0.0, 0.0, 0.4, 0.4)], 1.0, 1.0)
}

/// One 0.4 mm pad part per terminal at `(x, y)` mm, named `NET_i`.
fn estimate_board(nets: &[(&str, &[(f64, f64)])]) -> PhysicalBoard {
    let mut placements = Vec::new();
    let mut physical = Vec::new();
    for (name, points) in nets {
        let mut pads = Vec::new();
        for (index, (x, y)) in points.iter().enumerate() {
            let reference = format!("{name}_{index}");
            placements.push(placement(&reference, "pad", *x, *y));
            pads.push(copper_pcb::board::PadReference::new(&reference, "1"));
        }
        physical.push(PhysicalNet { name: (*name).to_owned(), pads });
    }
    board("Estimate", 30.0, 30.0, vec![small_pad()], placements, physical)
}

fn estimate(board: &PhysicalBoard) -> CongestionMap {
    estimate_demand(board, MM, None).expect("estimate")
}

fn sum(rows: &[Vec<f64>]) -> f64 {
    rows.iter().flatten().sum()
}

#[test]
fn estimate_peaks_where_bounding_boxes_overlap() {
    let board = estimate_board(&[("X", &[(4.5, 4.5), (16.5, 16.5)]), ("Y", &[(10.5, 10.5), (25.5, 25.5)])]);
    let m = estimate(&board);
    assert!(m.source == "rudy" && (m.nx, m.ny) == (30, 30));
    let mut demand = m.demand.clone();
    for placement in &board.placements {
        let (ix, iy) = m.tile_of(placement.position);
        demand[iy][ix] = 0.0; // leave out the pin-density tiles
    }
    let (mut best, mut at) = (f64::NEG_INFINITY, (0, 0));
    for (iy, row) in demand.iter().enumerate() {
        for (ix, value) in row.iter().enumerate() {
            if *value > best {
                (best, at) = (*value, (ix, iy));
            }
        }
    }
    assert!((10..=16).contains(&at.0) && (10..=16).contains(&at.1), "{at:?}");
    assert!(close(demand[13][13], demand[6][6] + demand[20][20]));
    assert!(m.demand[6][6] > 0.0 && m.demand[2][25] == 0.0);
}

#[test]
fn estimate_is_in_lane_cells() {
    let m = estimate(&estimate_board(&[("S", &[(5.5, 10.5), (20.5, 10.5)])]));
    // Box 15 mm + 1 mm by 1 mm (clamped): (16 + 1) mm of lane-wide wire, plus
    // a 0.4 mm pin stub per pad.
    assert!(close(sum(&m.demand), (16.0 + 1.0) / 0.45 + 2.0 * 0.4 / 0.45), "{}", sum(&m.demand));
    // Two signal layers of empty 1 mm tiles.
    assert!(close(m.capacity[25][25], 2.0 * (1.0f64 / 0.45).powi(2)));
}

#[test]
fn zone_net_is_skipped_but_its_pads_keep_a_tap() {
    let plain_board = estimate_board(&[("GND", &[(4.5, 4.5), (25.5, 25.5)])]);
    let mut zoned_board = plain_board.clone();
    zoned_board.zones = vec![zone("gnd", "GND", &[CopperLayer::Back], rect(0.0, 0.0, 30.0, 30.0))];
    let (plain, zoned) = (estimate(&plain_board), estimate(&zoned_board));
    assert!(plain.demand[15][15] > 0.0 && zoned.demand[15][15] == 0.0);
    assert!(zoned.demand[4][4] > 0.0 && sum(&zoned.demand) < sum(&plain.demand));
}

#[test]
fn capacity_zero_outside_outline_and_lower_under_pads_and_planes() {
    let mut triangle = estimate_board(&[("S", &[(3.5, 3.5), (8.5, 3.5)])]);
    triangle.outline =
        BoardOutline::new(vec![pt(0.0, 0.0), pt(20.0, 0.0), pt(0.0, 20.0)], Vec::new(), None, None).expect("outline");
    let m = estimate(&triangle);
    let full = 2.0 * (1.0f64 / 0.45).powi(2);
    assert!(m.capacity[18][18] == 0.0 && close(m.capacity[10][1], full));
    assert!(0.0 < m.capacity[9][10] && m.capacity[9][10] < full, "on the diagonal edge");
    let pads = (0..4)
        .flat_map(|i| (0..4).map(move |j| (i, j)))
        .map(|(i, j)| pad(&(i * 4 + j).to_string(), -0.75 + 0.5 * i as f64, -0.75 + 0.5 * j as f64, 0.35, 0.35))
        .collect();
    let field = board(
        "Pads",
        10.0,
        10.0,
        vec![footprint("grid", pads, 2.5, 2.5)],
        vec![placement("U", "grid", 5.0, 5.0)],
        Vec::new(),
    );
    let under = estimate(&field);
    assert!(under.capacity[4][4] < under.capacity[1][1] && close(under.capacity[1][1], full));
    // 0.15 mm gaps hold no 0.25 mm sample spacing: the front is full, the back is free.
    assert!(close(under.capacity[4][4], full / 2.0));
    let mut four = field.clone();
    four.stackup = Stackup::new(layers(4));
    four.nets = vec![net("GND", &[])];
    four.zones = vec![zone("gnd", "GND", &[CopperLayer::In1], rect(0.0, 0.0, 10.0, 10.0))];
    assert!(close(estimate(&four).capacity[1][1], 3.0 * (1.0f64 / 0.45).powi(2)));
}

// ---------------------------------------------------------------- proposer

fn crowded_board() -> PhysicalBoard {
    let r = footprint("r", vec![pad("1", -0.5, 0.0, 0.5, 0.5), pad("2", 0.5, 0.0, 0.5, 0.5)], 2.0, 1.0);
    let placements = [("A", 6.0, 8.0), ("B", 6.0, 10.0), ("C", 9.0, 9.0), ("D", 32.0, 12.0)]
        .iter()
        .map(|(r, x, y)| placement(r, "r", *x, *y))
        .collect();
    let board = board(
        "Crowded",
        40.0,
        20.0,
        vec![r],
        placements,
        vec![
            net("AD", &[("A", "2"), ("D", "1")]),
            net("BC", &[("B", "2"), ("C", "1")]),
            net("AC", &[("A", "1"), ("C", "2")]),
        ],
    );
    let poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    assert!(copper_drc::legality::placement_solution_is_legal(&board, &poses).expect("legal"));
    board
}

fn hot_map() -> CongestionMap {
    let (cx, cy) = (7.0, 9.0);
    let demand = (0..20)
        .map(|y| {
            (0..40)
                .map(|x| 3.0 * (-((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)) / 18.0).exp())
                .collect()
        })
        .collect();
    cmap(demand, None)
}

fn failed(names: &[&str]) -> IndexSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn propose(
    board: &PhysicalBoard,
    congestion: &CongestionMap,
    nets: &[&str],
    options: &PlannerOptions,
) -> Vec<Proposal> {
    propose_moves(board, congestion, &failed(nets), options, MM).expect("proposals")
}

fn pose<'b>(board: &'b PhysicalBoard, reference: &str) -> &'b Placement {
    board.placements.iter().find(|p| p.reference == reference).expect("placement")
}

/// The step multiplier of a move proposal (`...x{multiplier}` in its reason).
fn multiplier(proposal: &Proposal) -> i64 {
    proposal.reason.rsplit('x').next().and_then(|m| m.parse().ok()).expect("a move reason")
}

#[test]
fn failed_net_part_moves_away_from_the_hot_tile() {
    let board = crowded_board();
    let congestion = estimate(&board);
    let (hx, hy, _) = congestion.hot_tiles(1)[0];
    assert!((4..=10).contains(&hx) && (7..=11).contains(&hy), "the crowded side is hot");
    let proposals = propose(&board, &congestion, &["AD"], &PlannerOptions::default());
    assert!(!proposals.is_empty() && proposals[0].reference == "A" && proposals[0].reason.starts_with("failed_net:"));
    let centre = Point::new((2 * hx as i64 + 1) * MM / 2, (2 * hy as i64 + 1) * MM / 2);
    let old = pose(&board, "A").position;
    let distance = |p: Point| ((p.x_nm - centre.x_nm) as f64).hypot((p.y_nm - centre.y_nm) as f64);
    for proposal in proposals.iter().filter(|p| p.reference == "A" && p.delta_nm != (0, 0)) {
        let new = pose(&proposal.board, "A").position;
        assert!(distance(new) > distance(old));
        assert_eq!(proposal.delta_nm, (new.x_nm - old.x_nm, new.y_nm - old.y_nm));
        assert!(proposal.delta_nm.0 % MM == 0 && proposal.delta_nm.1 % MM == 0, "placement grid");
    }
    // Synthetic hotspot: the move is down its gradient, shortest steps first.
    let moved: Vec<Proposal> = propose(&board, &hot_map(), &["AD"], &PlannerOptions::default())
        .into_iter()
        .filter(|p| p.reference == "A" && p.delta_nm != (0, 0))
        .collect();
    assert!(!moved.is_empty() && moved.iter().all(|p| p.delta_nm.0 < 0 && p.delta_nm.1 < 0));
    let steps: Vec<i64> = moved.iter().map(multiplier).collect();
    let mut sorted = steps.clone();
    sorted.sort_unstable();
    assert_eq!(steps, sorted);
    // Rotations stay at the current position; hot-tile parts follow failed-net parts.
    let rotated: Vec<&Proposal> = proposals.iter().filter(|p| p.delta_nm == (0, 0)).collect();
    assert!(!rotated.is_empty() && rotated.iter().all(|p| p.rotation_degrees != Decimal::from_i64(0)));
    let groups: Vec<i64> = proposals.iter().map(|p| i64::from(p.reason.starts_with("hot_tile:"))).collect();
    let mut sorted = groups.clone();
    sorted.sort_unstable();
    assert_eq!(groups, sorted);
    assert!(groups.contains(&1));
}

#[test]
fn fixed_parts_are_never_proposed() {
    let mut board = crowded_board();
    board.placement_rules =
        vec![ComponentPlacementRule { fixed_position: Some(pt(6.0, 10.0)), ..ComponentPlacementRule::new("B") }];
    let options =
        PlannerOptions { fixed_references: ["C".to_owned()].into_iter().collect(), ..PlannerOptions::default() };
    let proposals = propose(&board, &hot_map(), &["AD", "BC", "AC"], &options);
    assert!(!proposals.is_empty() && proposals.iter().all(|p| p.reference == "A" || p.reference == "D"));
    for proposal in &proposals {
        assert_eq!(pose(&proposal.board, "B"), &board.placements[1]);
        assert_eq!(pose(&proposal.board, "C"), &board.placements[2]);
    }
    assert!(propose(&board, &hot_map(), &["AD"], &options).len() <= 24, "the proposal limit");
}

#[test]
fn rigid_cluster_member_moves_with_its_cluster() {
    let mut board = crowded_board();
    let digest = footprint_geometry_digest(&board.footprints["r"]);
    let d = Decimal::from_i64;
    let cluster = RigidPlacementCluster {
        name: "unit".to_owned(),
        anchor_reference: "A".to_owned(),
        anchor_pad: None,
        members: vec![
            RigidPlacementMember::new("A", "r", &digest, Point::new(0, 0), &d(0)).expect("member"),
            RigidPlacementMember::new("E", "r", &digest, pt(0.0, -3.0), &d(0)).expect("member"),
        ],
        source: "synthetic fixture".to_owned(),
        allowed_rotations: vec![d(0), d(180)],
        keepouts: Vec::new(),
        internal_clearance_nm: None,
        edge_openings: Vec::new(),
    }
    .normalized()
    .expect("cluster");
    // B steps aside so the unit's 180 degree turn (E to the far side of A) is legal.
    board.placements[1].position = pt(10.0, 13.0);
    board.placements.push(placement("E", "r", 6.0, 5.0));
    board.rigid_clusters = vec![cluster];
    let proposals: Vec<Proposal> = propose(&board, &hot_map(), &["AD"], &PlannerOptions::default())
        .into_iter()
        .filter(|p| p.reference == "A")
        .collect();
    let kinds: IndexSet<bool> = proposals.iter().map(|p| p.delta_nm == (0, 0)).collect();
    assert_eq!(kinds.len(), 2, "moves and a rotation");
    for proposal in &proposals {
        let e = pose(&proposal.board, "E");
        if proposal.delta_nm != (0, 0) {
            assert_eq!(e.position, Point::new(6 * MM + proposal.delta_nm.0, 5 * MM + proposal.delta_nm.1));
        } else {
            // A 180 degree unit rotation swings E about A.
            assert!(e.position == pt(6.0, 11.0) && e.rotation_degrees == d(180));
        }
    }
}

#[test]
fn nothing_is_proposed_beneath_accepted_copper_and_proposals_are_deterministic() {
    let board = crowded_board();
    let mut routed = board.clone();
    routed.tracks = vec![track("AD", (6.5, 8.0), (31.5, 12.0), 0.25, CopperLayer::Front)];
    assert!(propose(&routed, &hot_map(), &["AD"], &PlannerOptions::default()).is_empty());
    let first = propose(&board, &estimate(&board), &["AD"], &PlannerOptions::default());
    let second = propose(&board, &estimate(&board), &["AD"], &PlannerOptions::default());
    let key = |p: &Proposal| {
        (p.reference.clone(), p.delta_nm, p.rotation_degrees.clone(), p.reason.clone(), p.board.placements.clone())
    };
    assert_eq!(first.iter().map(key).collect::<Vec<_>>(), second.iter().map(key).collect::<Vec<_>>());
    assert!(first.len() > 1);
    assert!(propose_moves(&board, &estimate(&board), &failed(&["AD"]), &PlannerOptions::default(), 0).is_err());
}

// ---------------------------------------------------------------- screener

#[test]
fn screen_runs_real_global_routing_and_prefers_fewer_overflows() {
    let board = crowded_board();
    let options = GlobalRouterOptions { tile_size_nm: 2 * MM, maximum_iterations: 2, ..GlobalRouterOptions::default() };
    let mut proposals = propose(&board, &hot_map(), &["AD"], &PlannerOptions::default());
    proposals.truncate(4);
    let mut events = Events::default();
    let mut progress: Option<&mut dyn copper_route::Progress> = Some(&mut events);
    let best = screen(&board, proposals.clone(), &options, 2, MM, &mut progress).expect("screen");
    assert!((1..=2).contains(&best.len()));
    let scores: Vec<[i64; 7]> = best.iter().map(|(_, route)| copper_route::routeflow::route_score(route)).collect();
    let mut sorted = scores.clone();
    sorted.sort();
    assert_eq!(scores, sorted);
    let kinds: Vec<(&str, &str)> = events.events.iter().map(|(p, e, _)| (p.as_str(), e.as_str())).collect();
    assert_eq!(kinds, [("congestion_screen", "started"), ("congestion_screen", "finished")]);
    assert_eq!(int_of(detail(&events.events[0].2, "proposal_count")), 4);
    assert_eq!(int_of(detail(&events.events[1].2, "kept_count")), best.len() as i64);
    assert!(screen(&board, proposals.clone(), &options, 0, MM, &mut None).expect("screen").is_empty());
    assert_eq!(screen(&board, proposals[..1].to_vec(), &options, 3, MM, &mut None).expect("screen").len(), 1);
}

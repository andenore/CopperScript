//! The unified search's cost terms, `bend_cost` and `guide_cost` (ported from
//! the former `tests/test_unified_search_costs.py`). Rust has one search, so
//! the `native` parametrizations run once and the native-vs-Python
//! equivalence tests are skipped (see `PORTED-routers.md`); the turn table is
//! a unit test in `src/unified_router/tests.rs`.

mod support_routers;

use copper_pcb::board::{PhysicalBoard, PhysicalNet};
use copper_physical::{CopperLayer, Point};
use copper_route::routing::{GlobalNetRoute, GlobalRouteSegment};
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::router::Router;
use copper_route::unified_router::search::Targets;
use copper_route::{GlobalRoutingResult, GlobalRoutingStatus};
use indexmap::IndexSet;
use support_routers::routes::guides;
use support_routers::*;

fn options() -> UnifiedRouterOptions {
    UnifiedRouterOptions { pitch_nm: Some(250_000), ..UnifiedRouterOptions::default() }
}

/// `_Router(board, options, nets, None, guides=guides)` at a 0.25 mm pitch.
fn router<'p>(
    board: &'p PhysicalBoard,
    guides: Option<&GlobalRoutingResult>,
    options: &UnifiedRouterOptions,
) -> Router<'p, 'p> {
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let index = RoutingClearanceIndex::new(board).expect("index");
    let mut router = Router::new(board, options, &nets, IndexSet::new(), None, index).expect("router");
    router.set_guides(guides);
    router
}

fn unpack(router: &Router<'_, '_>, node: i64) -> (i64, i64, i64) {
    let g = &router.lattice;
    (node / g.plane, node % g.plane % g.nx, node % g.plane / g.nx)
}

fn node(router: &Router<'_, '_>, layer: i64, ix: i64, iy: i64) -> i64 {
    layer * router.lattice.plane + iy * router.lattice.nx + ix
}

fn bends(router: &Router<'_, '_>, path: &[i64]) -> usize {
    let (mut count, mut previous) = (0, None);
    for pair in path.windows(2) {
        let ((la, xa, ya), (lb, xb, yb)) = (unpack(router, pair[0]), unpack(router, pair[1]));
        let heading = (la == lb).then_some((xb - xa, yb - ya));
        if heading.is_some() && previous.is_some() && heading != previous {
            count += 1;
        }
        previous = heading;
    }
    count
}

fn length(router: &Router<'_, '_>, path: &[i64]) -> f64 {
    path.windows(2)
        .map(|pair| {
            let ((_, xa, ya), (_, xb, yb)) = (unpack(router, pair[0]), unpack(router, pair[1]));
            ((xb - xa) as f64).hypot((yb - ya) as f64)
        })
        .sum()
}

/// A connected global route of F.Cu guide segments through `points`.
fn corridor_guide(board: &PhysicalBoard, net: &str, points: &[Point]) -> GlobalRoutingResult {
    let segments = points
        .windows(2)
        .enumerate()
        .map(|(i, pair)| GlobalRouteSegment {
            net: net.to_owned(),
            layer: CopperLayer::Front,
            start: pair[0],
            end: pair[1],
            guide_half_width_nm: mm(0.2),
            resource_id: format!("r{i}"),
        })
        .collect();
    let mut result = guides(board);
    result.status = GlobalRoutingStatus::Success;
    result.routes = vec![GlobalNetRoute {
        net: net.to_owned(),
        connected: true,
        accesses: Vec::new(),
        segments,
        vias: Vec::new(),
        length_nm: 0,
        diagnostics: Vec::new(),
        deferred_to_zone: false,
    }];
    result
}

fn search(router: &mut Router<'_, '_>, source: i64, target: i64, present: f64) -> Vec<i64> {
    router
        .search(0, &[(source, 0.0)], Targets { nodes: &[target], extra: None }, present)
        .expect("search")
        .expect("a path")
        .0
}

#[test]
fn cost_options_defaults_and_reject_negative_values() {
    let o = UnifiedRouterOptions::default();
    assert_eq!((o.bend_cost, o.guide_cost, o.guide_margin_nm), (0.25, 0.0, mm(1.0)));
    for invalid in [
        UnifiedRouterOptions { bend_cost: -1.0, ..o.clone() },
        UnifiedRouterOptions { guide_cost: -1.0, ..o.clone() },
        UnifiedRouterOptions { guide_margin_nm: -1, ..o.clone() },
    ] {
        assert!(invalid.validated().is_err());
    }
}

#[test]
fn bend_cost_removes_staircases_at_equal_length() {
    let board = pad_board(&[("A", &[(1.0, 1.0), (21.0, 21.0)])], 2, vec![]);
    let mut plain = router(&board, None, &UnifiedRouterOptions { bend_cost: 0.0, ..options() });
    let mut bent = router(&board, None, &UnifiedRouterOptions { bend_cost: 1.0, ..options() });
    // Light, uneven history makes equal-length paths differ by a hair, so
    // without a turn cost the search follows a staircase of 45-degree jogs.
    let mut rng = Lcg(3);
    let plane = plain.lattice.plane;
    for _ in 0..3000 {
        let node = rng.below(plane) as usize;
        let value = [0.05, 0.1, 0.2][rng.below(3) as usize];
        plain.occupancy.history[node] = value;
        bent.occupancy.history[node] = value;
    }
    let (source, target) = (node(&plain, 0, 20, 30), node(&plain, 0, 68, 46));
    let present = plain.present;
    let stairs = search(&mut plain, source, target, present);
    let turned = search(&mut bent, source, target, present);
    let octile = 32.0 + 16.0 * std::f64::consts::SQRT_2;
    assert!((length(&plain, &stairs) - octile).abs() < 1e-9);
    assert!((length(&bent, &turned) - octile).abs() < 1e-9);
    assert!(bends(&plain, &stairs) > 5, "{}", bends(&plain, &stairs));
    assert_eq!(bends(&bent, &turned), 1);
}

#[test]
fn guide_corridor_matches_segments_and_skips_unguided_nets() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 14.0), (17.0, 14.0)])], 2, vec![]);
    let guide = corridor_guide(&board, "A", &[pt(6.0, 13.0), pt(20.0, 13.0)]);
    let guided = UnifiedRouterOptions { guide_cost: 1.0, ..options() };
    let mut r = router(&board, Some(&guide), &guided);
    assert!(r.guide_corridor("B").expect("corridor").is_none());
    let corridor = r.guide_corridor("A").expect("corridor").expect("A's corridor");
    assert!(!corridor.nodes.is_empty());
    let reach = (mm(0.2) + mm(1.0)) as f64;
    let g = &r.lattice;
    for &n in &corridor.nodes {
        let (layer, ix, iy) = unpack(&r, n);
        let (x, y) = (g.x0 + ix * g.pitch, g.y0 + iy * g.pitch);
        assert_eq!(layer, 0);
        let dx = (mm(6.0) - x).max(0).max(x - mm(20.0));
        assert!((dx as f64).hypot((y - mm(13.0)) as f64) <= reach);
    }
    // Off by default: no corridor is ever built.
    let mut off = router(&board, Some(&guide), &options());
    assert!(off.guide_corridor("A").expect("corridor").is_none());
}

#[test]
fn guide_cost_pulls_the_path_into_the_corridor() {
    let board = pad_board(&[("A", &[(1.0, 1.0), (21.0, 21.0)])], 2, vec![]);
    let probe = router(&board, None, &options());
    let (source, target) = (node(&probe, 0, 20, 40), node(&probe, 0, 80, 40));
    let g = &probe.lattice;
    let point = |ix: i64, iy: i64| Point::new(g.x0 + ix * g.pitch, g.y0 + iy * g.pitch);
    // A detour corridor: up 20 pitches (5 mm), across, and back down.
    let guide = corridor_guide(&board, "A", &[point(20, 40), point(20, 60), point(80, 60), point(80, 40)]);
    let inside: IndexSet<i64> = router(&board, Some(&guide), &UnifiedRouterOptions { guide_cost: 1.0, ..options() })
        .guide_corridor("A")
        .expect("corridor")
        .expect("A's corridor")
        .nodes
        .iter()
        .copied()
        .collect();
    let outside = |cost: f64| {
        let mut r = router(&board, Some(&guide), &UnifiedRouterOptions { guide_cost: cost, ..options() });
        let present = r.present;
        let path = search(&mut r, source, target, present);
        path.iter().filter(|n| !inside.contains(*n)).count() as f64 / path.len() as f64
    };
    assert!(outside(0.0) > 0.5);
    assert!(outside(2.0) < 0.1);
}

/// `test_native_and_python_routes_agree_with_both_cost_terms` compared the
/// native kernel with the Python search; the route with both cost terms is
/// still checked to be complete and clean.
#[test]
fn routes_with_both_cost_terms_are_complete_and_clean() {
    let mut nets: Vec<(String, Vec<(f64, f64)>)> =
        (0..4).map(|i| (format!("H{i}"), vec![(2.0, 4.0 + 2.0 * i as f64), (18.0, 4.0 + 2.0 * i as f64)])).collect();
    nets.extend((0..4).map(|i| (format!("V{i}"), vec![(4.0 + 3.0 * i as f64, 2.0), (4.0 + 3.0 * i as f64, 18.0)])));
    let spec: Vec<(&str, &[(f64, f64)])> = nets.iter().map(|(n, p)| (n.as_str(), p.as_slice())).collect();
    let board = pad_board(&spec, 2, vec![]);
    let options = UnifiedRouterOptions { bend_cost: 0.5, guide_cost: 1.0, ..UnifiedRouterOptions::default() };
    let routed = support_routers::routes::route(&board, &options);
    assert_eq!(routed.opens(), 0);
    assert_eq!(routed.errors(), 0);
    let again = support_routers::routes::route(&board, &options);
    assert_eq!(again.result.board.tracks, routed.result.board.tracks);
}

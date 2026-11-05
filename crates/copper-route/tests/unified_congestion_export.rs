//! The congestion map exported by the unified router (ported from the former
//! `tests/test_unified_congestion_export.py`). Python's `value_at` has no
//! Rust counterpart; lookups are checked through `tile_of`/`utilisation`.

mod support_routers;

use copper_pcb::board::PhysicalBoard;
use copper_physical::{CopperLayer, Point};
use copper_route::congestion_feedback::{CongestionMap, UTILISATION_SENTINEL};
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use support_routers::routes::{Routed, guides, route, route_with};
use support_routers::*;

const TILE: i64 = 2_000_000;
const BOTH: [CopperLayer; 2] = [CopperLayer::Front, CopperLayer::Back];

/// H and V each span the board edge to edge on one routable layer, so their
/// copper must cross at (13, 13) mm, the centre of 2 mm tile (6, 6).
fn crossing_board() -> PhysicalBoard {
    let mut board = pad_board(&[("H", &[(-2.5, 10.0), (22.5, 10.0)]), ("V", &[(10.0, -2.5), (10.0, 22.5)])], 2, vec![]);
    board.net_routing_rules = ["H", "V"]
        .iter()
        .map(|name| {
            let mut rule = rule(name);
            rule.allowed_layers = vec![CopperLayer::Front];
            rule
        })
        .collect();
    board
}

fn crossing_options() -> UnifiedRouterOptions {
    UnifiedRouterOptions { maximum_iterations: 10, congestion_tile_nm: TILE, ..UnifiedRouterOptions::default() }
}

fn crossing_route(options: UnifiedRouterOptions) -> Routed {
    route(&crossing_board(), &options)
}

fn congestion(routed: &Routed) -> &CongestionMap {
    routed.result.congestion.as_ref().expect("congestion")
}

#[test]
fn lattice_covers_the_board_and_lookups_agree() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let routed = route(&board, &UnifiedRouterOptions { congestion_tile_nm: TILE, ..UnifiedRouterOptions::default() });
    let map = congestion(&routed);
    assert_eq!(map.source, "unified_router");
    assert_eq!(map.tile_nm, TILE);
    for grid in [&map.demand, &map.capacity] {
        assert_eq!(grid.len(), map.ny);
        assert!(grid.iter().all(|row| row.len() == map.nx));
    }
    let outline = &routed.result.board.outline.vertices;
    assert!(map.x0_nm <= outline.iter().map(|p| p.x_nm).min().expect("x"));
    assert!(map.y0_nm <= outline.iter().map(|p| p.y_nm).min().expect("y"));
    assert!(map.x0_nm + map.nx as i64 * TILE > outline.iter().map(|p| p.x_nm).max().expect("x"));
    assert!(map.y0_nm + map.ny as i64 * TILE > outline.iter().map(|p| p.y_nm).max().expect("y"));
    for point in [pt(0.0, 0.0), pt(12.9, 7.1), pt(13.0, 13.0), pt(26.0, 26.0)] {
        let (ix, iy) = map.tile_of(point);
        assert!(ix < map.nx && iy < map.ny, "{point:?}");
    }
    assert_eq!(map.tile_of(pt(13.0, 13.0)), (6, 6));
    // Board material has track capacity; nothing outside it does.
    assert!(map.capacity[6][6] > 0.0);
    assert!(map.capacity.iter().flatten().all(|c| *c >= 0.0));
}

#[test]
fn forced_crossing_is_the_hottest_tile() {
    let routed = crossing_route(crossing_options());
    let map = congestion(&routed);
    assert!(!routed.result.nets.iter().all(|net| net.connected));
    let hot = map.hot_tiles(1);
    assert_eq!(hot.len(), 1);
    let (ix, iy, utilisation) = hot[0];
    assert_eq!((ix, iy), map.tile_of(pt(13.0, 13.0)));
    assert!(utilisation > 0.0);
    // The blocked net is named where the exact check stopped it.
    let failed: indexmap::IndexSet<&String> = map.failed_points.iter().map(|(net, _)| net).collect();
    let open: indexmap::IndexSet<&String> =
        routed.result.nets.iter().filter(|net| !net.connected).map(|net| &net.net).collect();
    assert_eq!(failed, open);
}

#[test]
fn gradient_points_away_from_the_hot_tile() {
    let routed = crossing_route(crossing_options());
    let map = congestion(&routed);
    let (ix, iy, _) = map.hot_tiles(1)[0];
    let centre = |tx: i64, ty: i64| Point::new(map.x0_nm + tx * TILE + TILE / 2, map.y0_nm + ty * TILE + TILE / 2);
    for (ox, oy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let (gx, gy) = map.gradient_at(centre(ix as i64 + ox, iy as i64 + oy));
        assert!(gx * ox as f64 + gy * oy as f64 > 0.0, "{ox},{oy}: {gx},{gy}");
        assert!((gx.hypot(gy) - 1.0).abs() < 1e-9);
    }
}

#[test]
fn boxed_in_net_appears_in_failed_points() {
    // A's first pad (6, 13) mm is walled in on both layers: no track or via
    // can leave the box.
    let walls = [(4.4, 11.4, 7.6, 11.8), (4.4, 14.2, 7.6, 14.6), (4.4, 11.4, 4.8, 14.6), (7.2, 11.4, 7.6, 14.6)];
    let mut board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 4.0), (17.0, 4.0)])], 2, vec![]);
    board.copper_keepouts = walls
        .iter()
        .enumerate()
        .map(|(i, (x0, y0, x1, y1))| copper_keepout(&format!("wall-{i}"), &BOTH, rect(*x0, *y0, *x1, *y1)))
        .collect();
    let routed = route(&board, &UnifiedRouterOptions::default());
    let open: Vec<&String> = routed.result.nets.iter().filter(|net| !net.connected).map(|net| &net.net).collect();
    assert_eq!(open, ["A"]);
    let map = congestion(&routed);
    assert!(map.failed_points.iter().any(|(net, point)| net == "A" && *point == pt(6.0, 13.0)));
    assert!(map.failed_points.iter().all(|(net, _)| net == "A"));
}

/// Dropped: Python's `off == on` (the result has no equality; the copper is compared).
#[test]
fn export_off_leaves_no_map_and_identical_copper() {
    let on = crossing_route(crossing_options());
    let off = crossing_route(UnifiedRouterOptions { export_congestion: false, ..crossing_options() });
    assert!(off.result.congestion.is_none() && on.result.congestion.is_some());
    assert_eq!(off.result.board.tracks, on.result.board.tracks);
    assert_eq!(off.result.board.vias, on.result.board.vias);
    assert_eq!(off.result.nets.len(), on.result.nets.len());
    for (a, b) in off.result.nets.iter().zip(&on.result.nets) {
        assert_eq!((&a.net, a.connected), (&b.net, b.connected));
    }
}

#[test]
fn utilisation_is_finite_without_capacity() {
    let routed = crossing_route(crossing_options());
    let map = congestion(&routed);
    let utilisation = map.utilisation();
    assert!(utilisation.iter().flatten().all(|u| u.is_finite()));
    // The lattice's last column lies beyond the board edge: no capacity and,
    // without demand, no utilisation either.
    assert!(map.capacity.iter().all(|row| *row.last().expect("column") == 0.0));
    assert!(utilisation.iter().all(|row| *row.last().expect("column") == 0.0));
    // Demand on a tile without capacity: a large finite sentinel.
    let blocked = CongestionMap {
        tile_nm: TILE,
        x0_nm: 0,
        y0_nm: 0,
        nx: 2,
        ny: 1,
        demand: vec![vec![3.0, 3.0]],
        capacity: vec![vec![0.0, 6.0]],
        source: "unified_router".to_owned(),
        failed_points: Vec::new(),
    };
    let values = blocked.utilisation();
    assert!(values[0].iter().all(|v| v.is_finite()));
    assert!(values[0][0] >= UTILISATION_SENTINEL && values[0][1] == 0.5);
}

#[test]
fn tile_must_cover_a_pitch() {
    let fine = UnifiedRouterOptions { pitch_nm: Some(250_000), congestion_tile_nm: 100_000, ..Default::default() };
    assert!(fine.validated().is_err());
    assert!(UnifiedRouterOptions { congestion_tile_nm: 0, ..Default::default() }.validated().is_err());
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let global_route = guides(&board);
    let options = UnifiedRouterOptions { congestion_tile_nm: 100_000, ..Default::default() };
    let error = route_unified(&board, &global_route, &options, None, None, None, None, None).unwrap_err();
    assert!(matches!(error, copper_route::unified_error::Error::Value(_)), "{error:?}");
}

#[test]
fn progress_reports_the_export() {
    let board = crossing_board();
    let mut events = Events::default();
    let result = route_with(&board, &guides(&board), &crossing_options(), &mut events);
    let phases: Vec<&String> =
        events.events.iter().filter(|(phase, _, _)| phase == "unified_congestion").map(|(_, e, _)| e).collect();
    assert_eq!(phases, ["finished"]);
    let data = events.of("unified_congestion", "finished")[0];
    let map = result.congestion.as_ref().expect("congestion");
    let utilisation = map.utilisation();
    let hot = (0..map.ny)
        .flat_map(|iy| (0..map.nx).map(move |ix| (ix, iy)))
        .filter(|(ix, iy)| utilisation[*iy][*ix] > 1.0 && map.demand[*iy][*ix] > 0.0)
        .count() as i64;
    assert_eq!(
        (int_of(detail(data, "tiles")), int_of(detail(data, "hot_tiles")), int_of(detail(data, "failed_points"))),
        (14 * 14, hot, map.failed_points.len() as i64)
    );
    assert!(int_of(detail(data, "failed_points")) >= 1);
}

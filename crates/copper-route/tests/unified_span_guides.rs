//! Physical via spans in search and the via-transition masks (ported from
//! the former `tests/test_unified_span_guides.py`). Rust has one search, so
//! the `backend` parametrization runs once.
//! `test_guides_survive_every_repair_router` subclassed `_Router` to observe
//! each repair router and is skipped (see `PORTED-routers.md`).

mod support_routers;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, PhysicalNet, ViaKind};
use copper_physical::{CopperKeepout, CopperLayer, Point};
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::rasters::{FOREIGN, FREE};
use copper_route::unified_router::route::route_unified;
use copper_route::unified_router::router::Router;
use indexmap::IndexSet;
use support_routers::routes::{drc, errors_except_opens, guides};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;
const IN1: CopperLayer = CopperLayer::In1;
const IN2: CopperLayer = CopperLayer::In2;

fn through() -> copper_pcb::board::ViaTechnology {
    technology("through", ViaKind::Through, F, B)
}

fn blind() -> copper_pcb::board::ViaTechnology {
    technology("blind-f1", ViaKind::Blind, F, IN1)
}

fn back_blind() -> copper_pcb::board::ViaTechnology {
    technology("blind-b2", ViaKind::Blind, IN2, B)
}

fn keepout(id: &str, layers: &[CopperLayer], bounds: (f64, f64, f64, f64), tracks: bool) -> CopperKeepout {
    let (x0, y0, x1, y1) = bounds;
    CopperKeepout {
        block_tracks: tracks,
        block_vias: !tracks,
        block_zones: false,
        ..copper_keepout(id, layers, rect(x0, y0, x1, y1))
    }
}

fn router_of(board: &PhysicalBoard) -> Router<'_, '_> {
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let options = UnifiedRouterOptions::default();
    Router::new(board, &options, &nets, IndexSet::new(), None, RoutingClearanceIndex::new(board).expect("index"))
        .expect("router")
}

fn spec_of(router: &Router<'_, '_>, name: &str) -> usize {
    router.specs.iter().position(|s| s.name == name).expect("spec")
}

/// The `(low, high)` transition mask of `masks` at column `xy`.
fn mask_at(masks: &[((usize, usize), Vec<i32>)], span: (usize, usize), xy: i64) -> i32 {
    masks.iter().find(|(key, _)| *key == span).expect("span mask").1[xy as usize]
}

fn column_of(router: &Router<'_, '_>, point: Point) -> i64 {
    let g = &router.lattice;
    (point.y_nm - g.y0).div_euclid(g.pitch) * g.nx + (point.x_nm - g.x0).div_euclid(g.pitch)
}

#[test]
fn blind_vias_check_only_their_copper_span() {
    for (blocked_layer, connected) in [(B, true), (IN1, false)] {
        let mut board = four_layer(vec![through(), blind()], Some(vec![F, IN1]));
        let wall = keepout("wall", &[F], (12.8, 0.0, 13.2, 26.0), true);
        let blocker = keepout("via-blocker", &[blocked_layer], (0.0, 0.0, 26.0, 26.0), false);
        board.tracks.clear();
        board.copper_keepouts = vec![wall.clone()];
        let options =
            UnifiedRouterOptions { maximum_iterations: 3, commit_rounds: 1, ..UnifiedRouterOptions::default() };
        let route = |board: &PhysicalBoard| {
            route_unified(board, &guides(board), &options, None, None, None, None, None).expect("route")
        };
        let control = route(&board);
        assert!(control.nets.iter().find(|n| n.net == "A").expect("A").connected);
        assert!(!control.board.vias.is_empty() && control.board.vias.iter().all(|v| v.to_layer == IN1));
        board.copper_keepouts = vec![wall, blocker];
        let exact = RoutingClearanceIndex::new(&board).expect("index");
        let legal = control.board.vias.iter().all(|v| {
            exact
                .can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, Some(v.drill_nm), false, None)
                .expect("can_via")
        });
        assert_eq!(legal, connected, "{blocked_layer:?}");
        let result = route(&board);
        assert_eq!(result.nets.iter().find(|n| n.net == "A").expect("A").connected, connected);
        assert!(errors_except_opens(&drc(&result.board)).is_empty());
        assert_eq!(result.board.tracks[..board.tracks.len()], board.tracks[..]);
        assert_eq!(result.board.vias[..board.vias.len()], board.vias[..]);
    }
}

#[test]
fn through_fallback_still_checks_unvisited_layers() {
    let mut board = four_layer(vec![through(), blind()], None);
    board.copper_keepouts = vec![keepout("back", &[B], (0.0, 0.0, 26.0, 26.0), false)];
    let mut router = router_of(&board);
    let masks = router.via_transitions(0).expect("masks");
    let g = &router.lattice;
    let xy = (g.ny / 2) * g.nx + g.nx / 2;
    assert_eq!(mask_at(&masks, (0, 1), xy), FREE); // blind F-In1
    assert_eq!(mask_at(&masks, (0, 2), xy), FOREIGN); // F-In2 requires the F-B through via
    assert_eq!(mask_at(&masks, (0, 3), xy), FOREIGN);
}

#[test]
fn blind_via_drill_still_clears_holes_on_other_layers() {
    let mut board = four_layer(vec![through(), blind(), back_blind()], None);
    let mut hole = via("OTHER", (13.0, 13.0), 0.6, 0.3, IN2, B);
    hole.technology = Some("blind-b2".to_owned());
    board.vias = vec![hole.clone()];
    board.nets.push(net("OTHER", &[]));
    let mut router = router_of(&board);
    let index = spec_of(&router, "A");
    let masks = router.via_transitions(index).expect("masks");
    assert_eq!(mask_at(&masks, (0, 1), column_of(&router, hole.position)), FOREIGN);
}

#[test]
fn return_via_coverage_uses_the_proposed_barrel() {
    for covered in [true, false] {
        let mut board = four_layer(vec![through(), blind(), back_blind()], None);
        let signal = pt(13.0, 13.0);
        let (from, to, technology) = if covered { (F, IN1, "blind-f1") } else { (IN2, B, "blind-b2") };
        let mut returns = via("RETURN", (13.0, 14.0), 0.6, 0.3, from, to);
        returns.technology = Some(technology.to_owned());
        board.vias = vec![returns];
        board.nets.push(net("RETURN", &[]));
        board.net_routing_rules = vec![NetRoutingRule {
            require_return_vias: true,
            return_via_net: Some("RETURN".to_owned()),
            maximum_return_via_distance_nm: Some(mm(1.2)),
            ..rule("A")
        }];
        let mut router = router_of(&board);
        let index = spec_of(&router, "A");
        let masks = router.via_transitions(index).expect("masks");
        assert_eq!(mask_at(&masks, (0, 1), column_of(&router, signal)) != FOREIGN, covered, "{covered}");
    }
}

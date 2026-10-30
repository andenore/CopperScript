//! The general-router test builders of `tests/support_routers` for the
//! crate-private unit tests.

#[path = "../tests/support_routers/mod.rs"]
mod support;

pub(crate) use support::*;

use copper_pcb::board::{PhysicalBoard, PhysicalNet};
use indexmap::IndexSet;

use crate::routing_clearance::RoutingClearanceIndex;
use crate::unified_router::UnifiedRouterOptions;
use crate::unified_router::router::Router;

/// `_Router(board, options, nets, None)` with a fresh clearance index.
pub(crate) fn router<'p>(
    board: &'p PhysicalBoard,
    options: &UnifiedRouterOptions,
    nets: &[&PhysicalNet],
) -> Router<'p, 'p> {
    let index = RoutingClearanceIndex::new(board).expect("index");
    Router::new(board, options, nets, IndexSet::new(), None, index).expect("router")
}

/// `_Router` over every net of `board`.
pub(crate) fn router_all<'p>(board: &'p PhysicalBoard, options: &UnifiedRouterOptions) -> Router<'p, 'p> {
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    router(board, options, &nets)
}

/// The base-plane index of the node nearest `(x, y)` mm on a uniform lattice.
pub(crate) fn xy(router: &Router<'_, '_>, x: f64, y: f64) -> i64 {
    let g = &router.lattice;
    let ix = ((mm(x) - g.x0) as f64 / g.pitch as f64).round() as i64;
    let iy = ((mm(y) - g.y0) as f64 / g.pitch as f64).round() as i64;
    iy * g.nx + ix
}

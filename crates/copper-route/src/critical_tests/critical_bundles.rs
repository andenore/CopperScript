//! `tests/test_critical_bundles.py`: D-PHY plan R6, bundle-aware critical
//! ordering and bounded rip-up repair.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::CopperLayer;
use indexmap::IndexMap;

use crate::test_support_critical::*;

/// Pair names put the middle pair first by name: LANE_B (top), LANE_A
/// (middle), LANE_C (bottom) along both terminal rows.
pub(super) const LANES: [&str; 3] = ["LANE_B", "LANE_A", "LANE_C"];

/// A front-and-back keep-out over a rectangle in fixed-point millimetres.
pub(super) fn both_sides(id: &str, x0: &str, y0: &str, x1: &str, y1: &str) -> copper_physical::CopperKeepout {
    keepout(id, &[CopperLayer::Front, CopperLayer::Back], rectangle(nm(x0), nm(y0), nm(x1), nm(y1)))
}

/// `_bundle_board`: one 0.2/0.2 mm pair per lane, priority 100 unless
/// `priorities` names the lane.
pub(super) fn bundle_board(
    package_rows: &[(i64, i64)],
    connector_rows: &[(i64, i64)],
    keepouts: Vec<copper_physical::CopperKeepout>,
    names: &[&str],
    priorities: &IndexMap<&str, i64>,
) -> PhysicalBoard {
    let mut board = row_pair_board("PairBundle", package_rows, connector_rows, names, |lane, net, partner| {
        NetRoutingRule {
            priority: priorities.get(lane).copied().unwrap_or(100),
            ..differential_rule(net, partner, 200_000, 200_000)
        }
    });
    board.copper_keepouts = keepouts;
    board
}

/// `_blocked_middle_board`: three aligned pairs; a keep-out on the middle
/// lane, a wall above the top lane. The middle pair must detour. Above is
/// shorter, but only fits while the top pair is unrouted; below fits
/// between the obstacle and the bottom pair.
pub(super) fn blocked_middle_board() -> PhysicalBoard {
    let centres = ["-1.0", "0", "2.0"];
    bundle_board(
        &rows(&centres, "0.5"),
        &rows(&centres, "0.4"),
        vec![
            both_sides("wall-above-top-lane", "10", "0", "20", "8.55"),
            both_sides("middle-lane-obstacle", "14", "9.7", "16", "10.8"),
        ],
        &LANES,
        &IndexMap::new(),
    )
}

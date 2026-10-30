//! Two-pad package escape scenes (`test_fanout_access_pressure.py`,
//! `test_two_leg_fanout.py`).

use copper_pcb::board::PhysicalBoard;
use copper_physical::{CopperKeepout, CopperLayer, Point};

use super::*;

/// A two-pad package `U` ("two", pads at (0, -1) and (0, 1)) at (5, 7) and a
/// one-pad terminal footprint "one" on a 14 mm board, without placements of
/// the terminal or nets.
pub fn two_pad_package(name: &str) -> PhysicalBoard {
    let mut board = board(name, 14.0, 14.0);
    add_footprint(
        &mut board,
        footprint(
            "two",
            vec![pad("1", mm(0.0, -1.0), size(0.3, 0.3)), pad("2", mm(0.0, 1.0), size(0.3, 0.3))],
            size(2.0, 3.0),
        ),
    );
    add_footprint(&mut board, footprint("one", vec![pad("1", Point::new(0, 0), size(0.3, 0.3))], size(1.0, 1.0)));
    board.placements = vec![place("U", "two", mm(5.0, 7.0))];
    board
}

/// `test_fanout_access_pressure.board()`: nets A (U.1 to JA) and B (U.2 to JB).
pub fn trap_board() -> PhysicalBoard {
    let mut board = two_pad_package("trap");
    board.placements.extend([place("JA", "one", mm(11.0, 6.0)), place("JB", "one", mm(11.0, 8.0))]);
    board.nets = vec![net("A", &[("U", "1"), ("JA", "1")]), net("B", &[("U", "2"), ("JB", "1")])];
    board
}

/// Via-only keepouts tiling the board on the grid `xs` x `ys` on both
/// layers, except the cells in `open` (the Python rectangle tilings).
pub fn via_windows(xs: &[f64], ys: &[f64], open: &[(usize, usize)]) -> Vec<CopperKeepout> {
    let mut keepouts = Vec::new();
    for (i, columns) in xs.windows(2).enumerate() {
        for (j, rows) in ys.windows(2).enumerate() {
            if open.contains(&(i, j)) {
                continue;
            }
            let mut cell = keepout(
                &format!("cell-{i}-{j}"),
                &[CopperLayer::Front, CopperLayer::Back],
                rect(columns[0], rows[0], columns[1], rows[1]),
            );
            cell.block_tracks = false;
            cell.block_zones = false;
            keepouts.push(cell);
        }
    }
    keepouts
}

/// `test_two_leg_fanout.base_board`: net A (U.1 to J) and a single via
/// window at (4, 4) that no radial ray from U.1 at (5, 6) reaches; with
/// `block_first_order` a small F.Cu keepout at (4.4, 5.4) blocks the
/// diagonal-first elbow.
pub fn two_leg_board(block_first_order: bool) -> PhysicalBoard {
    let mut board = two_pad_package("two-leg");
    board.placements.push(place("J", "one", mm(11.0, 6.0)));
    board.nets = vec![net("A", &[("U", "1"), ("J", "1")])];
    let axes = [0.0, 3.55, 4.45, 14.0];
    board.copper_keepouts = via_windows(&axes, &axes, &[(1, 1)]);
    if block_first_order {
        let mut blocked = keepout("first diagonal blocked", &[CopperLayer::Front], rect(4.35, 5.35, 4.45, 5.45));
        blocked.block_vias = false;
        blocked.block_zones = false;
        board.copper_keepouts.push(blocked);
    }
    board
}

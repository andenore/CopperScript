//! Package via-site lattices (escape plan E7); ported from
//! `tests/test_via_lattice.py`.

mod support_planes;

use std::collections::BTreeSet;

use copper_pcb::board::PhysicalBoard;
use copper_physical::{CopperKeepout, Point};
use copper_route::via_lattice::{LatticeResult, plan_via_lattices};
use support_planes::*;

/// `plan_via_lattices(board, surface_rings=..., band_rings=...)` (Python
/// defaults 2 and 1).
fn plan(board: &PhysicalBoard, surface_rings: i64, band_rings: i64) -> LatticeResult {
    plan_via_lattices(board, None, surface_rings, band_rings).unwrap()
}

/// Board position of cell (cx, cy) of `ball_grid` (0.8 mm pitch, ball (0, 0) at (10, 10) mm).
fn cell_centre(cx: i64, cy: i64) -> Point {
    Point::new(10_000_000 + 800_000 * cx + 400_000, 10_000_000 + 800_000 * cy + 400_000)
}

/// Keepouts on every copper layer over 0.2 mm squares around `points`.
fn blocking(board: &PhysicalBoard, name: &str, points: &[Point]) -> Vec<CopperKeepout> {
    let half = 100_000;
    points
        .iter()
        .enumerate()
        .map(|(number, p)| {
            let ring = vec![
                Point::new(p.x_nm - half, p.y_nm - half),
                Point::new(p.x_nm + half, p.y_nm - half),
                Point::new(p.x_nm + half, p.y_nm + half),
                Point::new(p.x_nm - half, p.y_nm + half),
            ];
            keepout(&format!("{name}-{number}"), &board.stackup.copper_layers, ring)
        })
        .collect()
}

#[test]
fn every_plane_ball_gets_its_own_outward_site_and_the_lattice_is_legal() {
    let plane: Vec<i64> = (19..47).collect();
    let board = ball_grid(8, 0.45, &plane);
    let result = plan(&board, 2, 1);
    assert_eq!(result.lattices.len(), 1);
    let lattice = &result.lattices[0];
    // The 28 plane balls need a via (the edge signal balls escape on the surface).
    assert_eq!((lattice.sites.len(), result.vias.len()), (28, 28));
    assert!(lattice.unsited.is_empty(), "{:?}", lattice.unsited);
    let positions: BTreeSet<(i64, i64)> = lattice.sites.iter().map(|s| (s.position.x_nm, s.position.y_nm)).collect();
    assert_eq!(positions.len(), 28); // one via per cell
    assert!(lattice.sites.iter().all(|site| site.preferred)); // quadrant to corner
    let after = drc_codes(&result.board);
    let before = drc_codes(&board);
    assert!(after.is_subset(&before), "new findings {:?}", after.difference(&before).collect::<Vec<_>>());
}

#[test]
fn an_edge_supply_ball_takes_the_cell_outside_the_array() {
    let result = plan(&ball_grid(6, 0.45, &[1]), 2, 1);
    let sites: Vec<_> = result.lattices[0].sites.iter().filter(|site| site.pad.pad == "1").collect();
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].cell, (-1, -1));
    assert!(sites[0].preferred);
}

#[test]
fn the_assignment_gives_a_contested_cell_to_the_ball_that_has_no_other() {
    // Ball 29 (column 4, row 4) keeps only cell (3, 3), the outward cell of ball 22 (3, 3).
    let mut board = ball_grid(6, 0.45, &[]);
    board.copper_keepouts = blocking(&board, "block", &[cell_centre(4, 4), cell_centre(4, 3), cell_centre(3, 4)]);
    let result = plan(&board, 1, 0);
    let site = |number: &str| result.lattices[0].sites.iter().find(|site| site.pad.pad == number);
    assert_eq!(site("29").expect("ball 29 sited").cell, (3, 3));
    assert!(!site("22").expect("ball 22 sited").preferred);
}

#[test]
fn the_outer_band_keeps_signal_vias_out_when_inner_cells_are_free() {
    let in_outer_ring = |band: i64| {
        let result = plan(&ball_grid(8, 0.45, &[]), 1, band);
        let lattice = &result.lattices[0];
        assert_eq!(lattice.sites.len(), 36);
        assert!(lattice.unsited.is_empty());
        lattice.sites.iter().filter(|s| s.cell.0.min(s.cell.1).min(6 - s.cell.0).min(6 - s.cell.1) == 0).count()
    };
    assert!(in_outer_ring(1) < in_outer_ring(0));
}

#[test]
fn a_ball_without_a_legal_site_is_reported() {
    let mut board = ball_grid(6, 0.45, &[]);
    let centres: Vec<Point> = [(1, 1), (1, 2), (2, 1), (2, 2)].iter().map(|(x, y)| cell_centre(*x, *y)).collect();
    board.copper_keepouts = blocking(&board, "block", &centres);
    let result = plan(&board, 2, 1);
    let reason = result.lattices[0].unsited.iter().find(|(pad, _)| pad.pad == "15").map(|(_, r)| r.as_str());
    assert_eq!(reason, Some("no legal site")); // ball (2, 2): all four of its cells are blocked
}

//! `tests/test_pair_recessed_via_ports.py`: via sites displaced from the
//! terminal midpoint ray remain reachable.

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::Point;
use copper_units::Decimal;

use super::pair_outer_transitions::{Fixture, fixture, front_to_back, legal_on};
use crate::pair_search::PairSearchStats;
use crate::pair_vias::{TransitionTrace, local_via_ports};
use crate::test_support_critical::*;

/// `window_board(renamed, front_wall)`: B.Cu closed everywhere but the
/// window x = 8.6..10 mm, y = 5.8..7.2 mm.
fn window_board(renamed: bool, front_wall: bool) -> Fixture {
    let mut f = fixture(renamed, front_wall);
    let wall = |id: &str, x0: &str, y0: &str, x1: &str, y1: &str| {
        keepout(id, &[BACK], rectangle(nm(x0), nm(y0), nm(x1), nm(y1)))
    };
    f.board.copper_keepouts.extend([
        wall("above", "0", "0", "30", "5.8"),
        wall("below", "0", "7.2", "30", "20"),
        wall("left", "0", "0", "8.6", "20"),
        wall("right", "10", "0", "30", "20"),
    ]);
    f
}

#[test]
fn staggered_legal_via_window_is_reached_without_moving_components() {
    for renamed in [false, true] {
        let f = window_board(renamed, false);
        let found = front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut PairSearchStats::default());
        assert!(!found.is_empty(), "{renamed}");
        assert_eq!(found, front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut PairSearchStats::default()));
        for t in &found {
            assert!(t.vias.iter().all(|v| {
                (8_600_001..10_000_000).contains(&v.position.x_nm) && (5_800_001..7_200_000).contains(&v.position.y_nm)
            }));
            assert!(t.port.first[0].start == mm(5, 5) && t.port.second[0].start == mm(5, 6));
            assert!(legal_on(&f.board, t));
        }
        assert!(f.board.tracks.is_empty() && f.board.vias.is_empty());
    }
}

#[test]
fn stagger_cannot_cross_a_front_keepout() {
    let f = window_board(false, true);
    let ports = local_via_ports(
        &f.board,
        &f.rules.0,
        &f.rules.1,
        mm(5, 5),
        mm(5, 6),
        f.reference,
        f.reference,
        FRONT,
        BACK,
        &mut TransitionTrace::default(),
    )
    .expect("ports");
    assert!(ports.is_empty());
}

#[test]
fn stagger_rotates_with_the_package_and_its_obstacles() {
    let mut f = window_board(true, false);
    let rotated = |p: Point| at(20 * MM - p.y_nm, p.x_nm);
    f.board.outline = outline(20 * MM, 30 * MM);
    for pose in &mut f.board.placements {
        pose.position = rotated(pose.position);
        pose.rotation_degrees = Decimal::from_i64(90);
    }
    for keepout in &mut f.board.copper_keepouts {
        keepout.vertices = keepout.vertices.iter().map(|p| rotated(*p)).collect();
    }
    let (a, b) = (rotated(mm(5, 5)), rotated(mm(5, 6)));
    let found = front_to_back(&f, &f.board, a, b, &mut PairSearchStats::default());
    assert!(!found.is_empty());
    for t in &found {
        assert!(t.port.first[0].start == a && t.port.second[0].start == b);
        assert!(legal_on(&f.board, t));
    }
}

#[test]
fn failed_transition_diagnostics_do_not_change_geometry_or_candidates() {
    let f = window_board(true, true);
    let before: PhysicalBoard = f.board.clone();
    let mut stats = PairSearchStats::default();
    let traced = front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut stats);
    assert!(traced.is_empty());
    assert!(front_to_back(&f, &f.board, mm(5, 5), mm(5, 6), &mut PairSearchStats::default()).is_empty());
    assert_eq!(stats.terminal_port_rejections.len(), 1);
    assert!(
        stats.terminal_port_rejections[0].starts_with("LEFT F.Cu to B.Cu: no exact transition;"),
        "{:?}",
        stats.terminal_port_rejections
    );
    assert!(f.board == before && f.board.tracks.is_empty() && f.board.vias.is_empty());
    assert!(stats.searches == 0 && stats.expanded_states == 0);
}

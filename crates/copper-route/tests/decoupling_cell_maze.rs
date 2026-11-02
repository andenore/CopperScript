//! A source decoupling launch may use the actual array cells around its
//! land when the straight launch is blocked; ported from
//! `tests/test_decoupling_cell_maze.py`. The no-work cases call the
//! crate-private `cell_maze_launches` (`src/test_support_planes.rs`).

mod support_planes;

use copper_pcb::board::{DecouplingLink, PhysicalBoard};
use copper_physical::CopperLayer;
use copper_route::decoupling_paths::{
    DecouplingPathResult, ProposalBounds, propose_crosslayer_decoupling, validate_crosslayer_witness,
};
use copper_route::fanout::FanoutOptions;
use support_planes::decoupling::*;
use support_planes::*;

fn propose(board: &PhysicalBoard, link: &DecouplingLink, maze: bool) -> DecouplingPathResult {
    let options = FanoutOptions { two_leg_escapes: false, maze_escapes: maze, ..FanoutOptions::default() };
    let bounds = ProposalBounds { maximum_site_candidates: 4, ..ProposalBounds::default() };
    propose_crosslayer_decoupling(board, link, Some(&options), &bounds, &[], None).unwrap()
}

#[test]
fn actual_array_cell_maze_recovers_blocked_straight_launch() {
    for renamed in [false, true] {
        let (board, link) = cell_grid(renamed);
        // Python stubbed `_cell_maze_launches` away; the maze only runs with
        // `maze_escapes`, so disabling it is the same blocked baseline.
        let failed = propose(&board, &link, false);
        assert!(failed.witness.is_none(), "renamed={renamed}");
        assert_eq!(failed.board, board);
        let result = propose(&board, &link, true);
        let witness = result.witness.as_ref().unwrap_or_else(|| panic!("{}", result.reason));
        assert_eq!(validate_crosslayer_witness(&result.board, witness, None).unwrap(), "");
        let errors = hard_findings(&result.board);
        assert!(errors.is_empty(), "{errors:?}");
    }
}

#[test]
fn source_limits_and_failed_return_keep_half_routes_disposable() {
    for restriction in ["supply_vias", "supply_layers", "return_vias"] {
        let (mut board, link) = cell_grid(false);
        for rule in &mut board.net_routing_rules {
            match restriction {
                "supply_vias" if rule.net == link.net => rule.max_vias = Some(0),
                "supply_layers" if rule.net == link.net => rule.allowed_layers = vec![CopperLayer::Front],
                "return_vias" if rule.net == link.return_net => rule.max_vias = Some(0),
                _ => {}
            }
        }
        let result = propose(&board, &link, true);
        assert!(result.witness.is_none(), "{restriction}");
        assert_eq!(result.board, board, "{restriction}");
        assert!(result.created_tracks.is_empty() && result.created_vias.is_empty(), "{restriction}");
    }
}

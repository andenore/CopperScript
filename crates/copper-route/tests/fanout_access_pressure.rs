//! Physical nearest-choice trap: a scarce exit must survive an easy
//! neighbour; ported from `tests/test_fanout_access_pressure.py`.
//!
//! The Python tests monkeypatched `fanout._candidates` to hand each pin a
//! scripted via-site list. Rust has no such seam, so the scripted domains are
//! built physically instead: via keepouts tile the board and leave only the
//! named via windows open (as the Python physical-window test did), and
//! `FanoutPinAnalysis` (not modelled in Rust) is replaced by the observable
//! accesses, pending pads and copper.

mod support_planes;

use copper_pcb::board::{PadReference, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;
use copper_route::fanout::{FanoutOptions, FanoutResult, route_fanout};
use indexmap::IndexSet;
use support_planes::fanout::*;
use support_planes::*;

/// The Python physical-window board: U.1 at (5, 6) and U.2 at (3, 8), with
/// whole-via windows only at (3, 6) and (5, 4). U.1 reaches both, U.2 only
/// (3, 6), which is also U.1's straight outward ray, so a greedy U.1 takes
/// U.2's only exit.
fn windows_board(open: &[(usize, usize)]) -> PhysicalBoard {
    let mut board = trap_board();
    let package = board.footprints.get_mut("two").unwrap();
    package.pads[0].position = mm(-1.0, -1.0);
    package.pads[1].position = mm(-3.0, 1.0);
    board.placements[0].position = mm(6.0, 7.0);
    // Rectangle tiling avoids the router's deliberately conservative handling
    // of keepouts with holes; only the open cells accept a whole via.
    board.copper_keepouts = via_windows(&[0.0, 2.3, 3.7, 4.3, 5.7, 14.0], &[0.0, 3.3, 4.7, 5.3, 6.7, 14.0], open);
    board
}

const BOTH_WINDOWS: [(usize, usize); 2] = [(1, 3), (3, 1)];

fn options() -> FanoutOptions {
    FanoutOptions {
        minimum_component_pads: 2,
        two_leg_escapes: false,
        joint_escapes: false,
        maximum_neighbor_distance_nm: nm(5.0),
        ..FanoutOptions::default()
    }
}

fn fanout(board: &PhysicalBoard, options: &FanoutOptions, only: Option<&[&str]>) -> FanoutResult {
    let only: Option<IndexSet<String>> = only.map(|nets| nets.iter().map(|n| (*n).to_owned()).collect());
    route_fanout(board, Some(options), only.as_ref()).unwrap()
}

fn assert_same(first: &FanoutResult, second: &FanoutResult) {
    assert_eq!(first.board, second.board);
    assert_eq!(first.accesses, second.accesses);
    assert_eq!(first.pending_pads, second.pending_pads);
    assert_eq!(first.created_vias, second.created_vias);
    assert_eq!(first.created_tracks, second.created_tracks);
}

fn u(pad: &str) -> PadReference {
    pad_ref("U", pad)
}

#[test]
fn low_slack_exit_survives_earlier_easy_neighbor() {
    let base = windows_board(&BOTH_WINDOWS);
    let greedy = fanout(&base, &FanoutOptions { constrained_pins_first: false, ..options() }, None);
    let selected = fanout(&base, &options(), None);
    assert_eq!(greedy.pending_pads, [u("2")]);
    assert!(selected.pending_pads.is_empty(), "{:?}", selected.pending_pads);
    assert_eq!(selected.accesses[&u("2")], mm(3.0, 6.0));
    assert_eq!(selected.accesses[&u("1")], mm(5.0, 4.0));
    // The one-site pin is chosen first (Python: pin analysis order "2", "1").
    assert_eq!(selected.accesses.keys().collect::<Vec<_>>(), [&u("2"), &u("1")]);
    assert_same(&selected, &fanout(&base, &options(), None));
    assert_eq!(selected.board.placements, base.placements);
    assert_drc_clear(&selected.board, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING"]);
}

#[test]
fn subset_preserves_input_copper_and_excludes_unselected_pins() {
    let base = windows_board(&BOTH_WINDOWS);
    let first = fanout(&base, &options(), Some(&["B"]));
    assert_eq!(first.accesses[&u("2")], mm(3.0, 6.0));
    let second = fanout(&first.board, &options(), Some(&["A"]));
    assert!(second.pending_pads.is_empty(), "{:?}", second.pending_pads);
    assert_eq!(second.board.tracks[..first.board.tracks.len()], first.board.tracks[..]);
    assert_eq!(second.board.vias[..first.board.vias.len()], first.board.vias[..]);
    assert_eq!(second.accesses.keys().collect::<Vec<_>>(), [&u("1")]);
    let empty = fanout(&second.board, &options(), Some(&[]));
    assert_eq!(empty.board, second.board);
    assert!(empty.accesses.is_empty() && empty.created_vias.is_empty());
    let missing: IndexSet<String> = ["missing".to_owned()].into_iter().collect();
    let error = route_fanout(&base, Some(&options()), Some(&missing)).unwrap_err();
    assert!(error.to_string().contains("unknown fanout nets"), "{error}");
}

#[test]
fn locked_input_exit_retained_when_domain_is_empty() {
    // Only the (3, 6) window: routing A first takes it on its outward ray.
    let base = windows_board(&[(1, 3)]);
    let first = fanout(&base, &options(), Some(&["A"]));
    assert_eq!(first.accesses[&u("1")], mm(3.0, 6.0));
    // This measures the exhausted dogbone domain, not the surface domain.
    let second = fanout(&first.board, &FanoutOptions { surface_fallback: false, ..options() }, Some(&["B"]));
    assert_eq!(second.pending_pads, [u("2")]);
    assert_eq!(second.board, first.board);
    assert!(second.created_vias.is_empty() && second.created_tracks.is_empty() && second.accesses.is_empty());
}

#[test]
fn critical_and_zone_nets_never_enter_candidate_domains() {
    let mut base = windows_board(&BOTH_WINDOWS);
    base.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { kind: RouteKind::RfFeed, ..rule("A") }];
    base.zones = vec![zone("ground", "B", &[CopperLayer::Back], base.outline.vertices.clone())];
    let result = fanout(&base, &options(), None);
    assert_eq!(result.board, base);
    assert!(result.accesses.is_empty() && result.pending_pads.is_empty() && result.created_vias.is_empty());
}

#[test]
fn real_candidate_generation_with_two_physical_via_windows() {
    let base = windows_board(&BOTH_WINDOWS);
    let greedy = fanout(&base, &FanoutOptions { constrained_pins_first: false, ..options() }, None);
    let selected = fanout(&base, &options(), None);
    assert_eq!(greedy.pending_pads, [u("2")]);
    assert!(selected.pending_pads.is_empty());
    assert_eq!(selected.accesses[&u("2")], mm(3.0, 6.0));
    assert_eq!(selected.accesses[&u("1")], mm(5.0, 4.0));
    assert!(!drc_codes(&selected.board).contains("DRC-COPPER-KEEPOUT"));
}

#[test]
fn adaptive_radial_sampling_finds_narrow_off_grid_via_window() {
    let mut base = trap_board();
    base.footprints.get_mut("two").unwrap().pads[0].position = mm(-1.0, -1.0);
    // A 0.8 mm via fits only near (4.9, 6), missed by 0.5 mm rays from (4, 6).
    base.copper_keepouts = via_windows(&[0.0, 4.49, 5.31, 14.0], &[0.0, 5.59, 6.41, 14.0], &[(1, 1)]);
    let settings = FanoutOptions {
        minimum_component_pads: 2,
        maximum_neighbor_distance_nm: nm(5.0),
        two_leg_escapes: false,
        joint_escapes: false,
        ..FanoutOptions::default()
    };
    let coarse = fanout(
        &base,
        &FanoutOptions { refinement_step_nm: settings.step_nm, surface_fallback: false, ..settings.clone() },
        None,
    );
    assert!(coarse.pending_pads.contains(&u("1")), "{:?}", coarse.accesses);
    let fine = fanout(&base, &settings, None);
    // Off the coarse 0.5 mm grid, so only the refined samples reach it
    // (Python also checked `refined_candidate_count`, not modelled in Rust).
    assert_eq!(fine.accesses[&u("1")], mm(4.9, 6.0));
    assert_eq!(fanout(&base, &settings, None).board, fine.board);
    assert_drc_clear(&fine.board, &["DRC-COPPER-KEEPOUT", "DRC-VIA-PAD-OVERLAP", "DRC-CLEARANCE"]);
}

//! One late completion pass preserves ordinary owners and exact source IDs;
//! ported from `tests/test_fanout_late_bypass_completion.py`. Rust does not
//! model the `LateBypassCompletionReport` (see `fanout_bypass_completion`), so
//! a completed pass shows as a cleared pending obligation with appended
//! semantic copper and an unchanged outcome as the incumbent itself.

mod support_planes;

use copper_pcb::board::{DecouplingLink, PadReference, PhysicalBoard};
use copper_physical::footprint::Size;
use copper_physical::{CopperLayer, Point};
use copper_route::bypass_reservations::{BypassReservation, verified_bypass_reservations};
use copper_route::decoupling_paths::{
    DecouplingCrossLayerWitness, ProposalBounds, propose_crosslayer_decoupling, validate_crosslayer_witness,
    verify_crosslayer_decoupling,
};
use copper_route::fanout::{FanoutOptions, FanoutResult, route_fanout};
use copper_route::fanout_bypass_completion::complete_late_bypasses;
use copper_route::pin_escape::RoutingAccess;
use indexmap::{IndexMap, IndexSet};
use support_planes::decoupling::*;
use support_planes::*;

fn reservation(result: &FanoutResult) -> BypassReservation {
    BypassReservation {
        board: result.board.clone(),
        created_tracks: result.created_tracks.clone(),
        created_vias: result.created_vias.clone(),
        added_track_count: result.added_track_count,
        added_via_count: result.added_via_count,
        decoupling_tracks: result.decoupling_tracks.clone(),
        decoupling_vias: result.decoupling_vias.clone(),
        decoupling_witnesses: result.decoupling_witnesses.clone(),
    }
}

/// `verified_bypass_reservations(source, result)[2]`: why the final semantic
/// ledger is not covered by actual paths, if it is not.
fn reservation_fault(source: &PhysicalBoard, result: &FanoutResult) -> Option<String> {
    verified_bypass_reservations(source, &reservation(result)).unwrap().2
}

fn witness(board: &PhysicalBoard, link: &DecouplingLink) -> Option<DecouplingCrossLayerWitness> {
    verify_crosslayer_decoupling(board, link, None, 10_000, None).unwrap().witness
}

/// Every field of a fanout result (it has no `PartialEq`; `assignment` is
/// `None` throughout these scenes).
fn assert_same(result: &FanoutResult, incumbent: &FanoutResult) {
    assert_eq!(result.board, incumbent.board);
    assert_eq!(result.accesses, incumbent.accesses);
    assert_eq!(result.pending_pads, incumbent.pending_pads);
    assert_eq!(
        (result.added_track_count, result.added_via_count),
        (incumbent.added_track_count, incumbent.added_via_count)
    );
    assert_eq!(result.created_tracks, incumbent.created_tracks);
    assert_eq!(result.created_vias, incumbent.created_vias);
    assert!(result.assignment.is_none() && incumbent.assignment.is_none());
    assert_eq!(result.surface_accesses, incumbent.surface_accesses);
    assert_eq!(result.decoupling_tracks, incumbent.decoupling_tracks);
    assert_eq!(result.decoupling_vias, incumbent.decoupling_vias);
    assert_eq!(result.decoupling_witnesses, incumbent.decoupling_witnesses);
    assert_eq!(result.decoupling_pending_links, incumbent.decoupling_pending_links);
}

/// The scene's single late pass completes: no pending obligation remains and
/// only new back-side semantic tracks are appended to the incumbent copper.
fn assert_completed(result: &FanoutResult, incumbent: &FanoutResult) {
    assert!(result.decoupling_pending_links.is_empty(), "{:?}", result.decoupling_pending_links);
    assert!(!result.decoupling_tracks.is_empty() && result.decoupling_vias.is_empty());
    assert!(result.decoupling_tracks.iter().all(|t| !incumbent.board.tracks.contains(t)));
    assert_eq!(result.board.tracks[..incumbent.board.tracks.len()], incumbent.board.tracks[..]);
    assert_eq!(result.board.vias, incumbent.board.vias);
}

#[test]
fn normal_fanout_completes_the_bypass_over_real_ordinary_launches() {
    for renamed in [false, true] {
        let (mut source, link) = crosslayer(renamed, true);
        let chip = source.footprints.get_mut("chip").unwrap();
        chip.pads.push(pad("NC", mm(-1.0, 0.0), size(0.4, 0.4)));
        for rule in &mut source.net_routing_rules {
            rule.max_vias = Some(1);
        }
        let options = FanoutOptions {
            minimum_component_pads: 3,
            maximum_neighbor_distance_nm: 3_000_000,
            joint_escapes: false,
            ..FanoutOptions::default()
        };
        let result = route_fanout(&source, Some(&options), None).unwrap();
        assert!(result.decoupling_pending_links.is_empty(), "{renamed}: {:?}", result.decoupling_pending_links);
        assert!(!result.decoupling_witnesses.is_empty());
        assert!(witness(&result.board, &link).is_some(), "{renamed}");
        assert!(!result.decoupling_tracks.is_empty());
        // Python forced the early bypass pass to fail (`_legal_choices`
        // patched to nothing) so the late pass reused the ordinary launches;
        // without that seam the early pass completes the link with its own
        // vias, so only the shared end state is checked here.
        assert_eq!(result.created_tracks, result.board.tracks);
        assert_eq!(result.created_vias, result.board.vias);
        assert_eq!(result.added_track_count as usize, result.created_tracks.len());
        assert_eq!(result.added_via_count as usize, result.created_vias.len());
        assert_eq!(reservation_fault(&source, &result), None, "{renamed}");
    }
}

#[test]
fn real_completion_preserves_surface_handoff_and_ordinary_occurrence_owners() {
    let (source, link, mut incumbent, pending) = incumbent_scene();
    let path = incumbent.board.tracks[..3].to_vec();
    let surface = RoutingAccess {
        position: path[path.len() - 1].end,
        layer: CopperLayer::Front,
        launch_position: path[0].start,
        path: path.clone(),
    };
    incumbent.accesses.shift_remove(&link.target);
    incumbent.surface_accesses = Some([(link.target.clone(), surface.clone())].into_iter().collect());
    let result = complete_late_bypasses(incumbent.clone(), &pending, &FanoutOptions::default(), None).unwrap();
    assert_completed(&result, &incumbent);
    assert_eq!(result.surface_accesses, incumbent.surface_accesses);
    assert_eq!(result.surface_accesses.as_ref().unwrap()[&link.target], surface);
    assert!(!result.accesses.contains_key(&link.target));
    assert_eq!(reservation_fault(&source, &result), None);
}

#[test]
fn only_nets_intersection_and_same_face_do_not_start_a_late_pass() {
    let (_, link, incumbent, pending) = incumbent_scene();
    let only: IndexSet<String> = [link.return_net.clone()].into_iter().collect();
    let result = complete_late_bypasses(incumbent.clone(), &pending, &FanoutOptions::default(), Some(&only)).unwrap();
    assert_same(&result, &incumbent);
    assert_eq!(result.decoupling_pending_links, pending);
    let mut same = incumbent.clone();
    let front = same.board.placements[0].side;
    for placement in &mut same.board.placements {
        if placement.reference == link.capacitor.component {
            placement.side = front;
        }
    }
    let result = complete_late_bypasses(same.clone(), &pending, &FanoutOptions::default(), None).unwrap();
    assert_same(&result, &same);
}

#[test]
fn shared_capacitor_source_is_rejected_and_pending_ids_are_not_guessed() {
    let (_, link, incumbent, pending) = incumbent_scene();
    let extra = pad_ref(&link.target.component, "FAR");
    let other = DecouplingLink { target: extra.clone(), ..link.clone() };
    // One capacitor bypassing two targets is not a board (checked when a
    // board state is decoded in Rust).
    let mut board = incumbent.board.clone();
    board.footprints.get_mut("chip").unwrap().pads.push(pad(
        "FAR",
        mm(14.0, 7.0),
        Size { width_nm: 400_000, height_nm: 400_000 },
    ));
    for net in &mut board.nets {
        if net.name == link.net {
            net.pads.push(extra.clone());
        }
    }
    board.decoupling_links = vec![link.clone(), other.clone()];
    let state = copper_route::state_codec::encode_board(&board);
    let error = copper_route::state_codec::physical_board(&state).unwrap_err().to_string();
    assert!(error.contains("one explicit protected pin association"), "{error}");
    let forged = [(other, "forged same capacitor different target".to_owned())];
    let error = match complete_late_bypasses(incumbent.clone(), &forged, &FanoutOptions::default(), None) {
        Ok(_) => panic!("a forged obligation was accepted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("undeclared original obligation"), "{error}");
    assert_eq!(incumbent.decoupling_pending_links, pending);
}

#[test]
fn pending_pad_resolution_clears_only_the_resolved_pad() {
    let (_, link, incumbent, pending) = incumbent_scene();
    let result = complete_late_bypasses(incumbent.clone(), &pending, &FanoutOptions::default(), None).unwrap();
    assert_completed(&result, &incumbent);
    assert!(!result.pending_pads.contains(&link.capacitor), "{:?}", result.pending_pads);
    // A pad leaves `pending_pads` only through a new verified port.
    for pad in &incumbent.pending_pads {
        assert_eq!(result.pending_pads.contains(pad), !result.accesses.contains_key(pad), "{pad:?}");
    }
}

#[test]
fn old_semantic_witness_paths_survive_a_late_completion() {
    let (_, link, mut incumbent, pending) = incumbent_scene();
    let (mut other, other_link) = crosslayer(true, true);
    for placement in &mut other.placements {
        placement.position = Point::new(placement.position.x_nm, placement.position.y_nm + 9_000_000);
    }
    let full = propose_crosslayer_decoupling(&other, &other_link, None, &ProposalBounds::default(), &[], None).unwrap();
    assert!(full.witness.is_some(), "{}", full.reason);
    let mut board = incumbent.board.clone();
    board.placements.extend(other.placements.iter().cloned());
    board.nets.extend(other.nets.iter().cloned());
    board.net_routing_rules.extend(other.net_routing_rules.iter().cloned());
    board.decoupling_links = vec![link.clone(), other_link.clone()];
    board.tracks.extend(full.board.tracks.iter().cloned());
    board.vias.extend(full.board.vias.iter().cloned());
    let protected = witness(&board, &other_link).expect("protected witness");
    incumbent.created_tracks = board.tracks.clone();
    incumbent.created_vias = board.vias.clone();
    incumbent.added_track_count = board.tracks.len() as i64;
    incumbent.added_via_count = board.vias.len() as i64;
    incumbent.decoupling_tracks = full.board.tracks.clone();
    incumbent.decoupling_vias = full.board.vias.clone();
    incumbent.decoupling_witnesses = vec![protected.clone()];
    incumbent.board = board.clone();
    let result = complete_late_bypasses(incumbent.clone(), &pending, &FanoutOptions::default(), None).unwrap();
    assert!(result.decoupling_pending_links.is_empty(), "{:?}", result.decoupling_pending_links);
    assert_eq!(validate_crosslayer_witness(&result.board, &protected, None).unwrap(), "");
    assert_eq!(result.decoupling_tracks[..full.board.tracks.len()], full.board.tracks[..]);
    assert_eq!(result.board.tracks[..board.tracks.len()], board.tracks[..]);
    let mut original = board;
    original.tracks.clear();
    original.vias.clear();
    assert_eq!(reservation_fault(&original, &result), None);
    // The Python case that obstructs the protected path needed a substituted
    // `route_decouplers` proposal; see PORTED-planes.md.
}

#[test]
fn no_ordinary_progress_does_not_repeat_the_early_bypass_work() {
    let (source, link) = crosslayer(false, true);
    let pending = vec![(link.clone(), "no source launch".to_owned())];
    let incumbent = FanoutResult {
        board: source,
        accesses: IndexMap::<PadReference, Point>::new(),
        pending_pads: Vec::new(),
        added_track_count: 0,
        added_via_count: 0,
        created_vias: Vec::new(),
        created_tracks: Vec::new(),
        assignment: None,
        surface_accesses: None,
        decoupling_tracks: Vec::new(),
        decoupling_vias: Vec::new(),
        decoupling_witnesses: Vec::new(),
        decoupling_pending_links: pending.clone(),
    };
    let result = complete_late_bypasses(incumbent.clone(), &pending, &FanoutOptions::default(), None).unwrap();
    assert_same(&result, &incumbent);
    assert_eq!(result.decoupling_pending_links, pending);
}

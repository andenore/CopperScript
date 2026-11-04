//! `tests/test_pair_tuning_span_copper_contacts.py`: finite-width held
//! contacts preserve exact candidate and source occurrences.

use copper_drc::connectivity::{Candidate, CopperContact, copper_contacts_overlap, explicit_copper_contacts};
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::RoundedConvexShape;
use copper_physical::geometry::segment_distance_squared;
use indexmap::IndexSet;
use num_traits::Zero;

use super::pair_tuning_spans::{AMP, SKEW, fragment, plain_tuner, standard};
use crate::pair_tuning_spans::tune_virtual_pair_spans;
use crate::test_support_critical::*;

fn target_contact(i: usize, t: &TrackSegment) -> CopperContact {
    CopperContact {
        identity: format!("candidate:{i}"),
        net: t.net.clone(),
        layers: vec![t.layer],
        shape: RoundedConvexShape::new(vec![t.start, t.end], t.width_nm / 2).expect("a shape"),
        drill: None,
        capped_layers: Vec::new(),
    }
}

/// `_explicit_copper_contacts(held, only_nets={"A"}, include_internal_connections=False)`.
fn track_contacts(held: &PhysicalBoard) -> Vec<CopperContact> {
    let only = IndexSet::from(["A".to_owned()]);
    let candidate = Candidate { only_nets: Some(&only), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, _, _) = explicit_copper_contacts(held, candidate).expect("contacts");
    contacts.into_iter().filter(|c| c.identity.starts_with("track:")).collect()
}

fn span_tuning(
    a: &mut Vec<TrackSegment>,
    b: &mut Vec<TrackSegment>,
    source: &PhysicalBoard,
    held: &PhysicalBoard,
) -> i64 {
    tune_virtual_pair_spans(a, b, SKEW, AMP, 32, source, held, &[], &[], &plain_tuner).expect("span tuning")
}

#[test]
fn finite_width_held_contacts_lock_exact_candidate_occurrences() {
    for shape in ["parallel", "endpoint_disks"] {
        for source_owned in [false, true] {
            let case = format!("{shape} {source_owned}");
            let (board, lanes) = standard();
            let parts = (fragment(&lanes.0), fragment(&lanes.1));
            let edges: Vec<TrackSegment> = if shape == "parallel" {
                // Offset centrelines never intersect, but finite same-net
                // copper joins every shorter-lane fragment. Duplicate held
                // occurrences stay explicit.
                let rail = track("A", at(5 * MM, 4_950_000), at(15 * MM, 4_950_000), 100_000, FRONT);
                vec![rail.clone(), rail]
            } else {
                // Each held stub starts beyond a fragment end; endpoint disks
                // contact without collinear overlap or a centreline
                // intersection.
                parts
                    .0
                    .iter()
                    .map(|t| {
                        let (x, y) = (t.end.x_nm, t.end.y_nm - 50_000);
                        track("A", at(x + 10_000, y), at(x + 20_000, y), 100_000, FRONT)
                    })
                    .collect()
            };
            let mut with_edges = board.clone();
            with_edges.tracks = edges.clone();
            let (source, held) =
                if source_owned { (with_edges.clone(), with_edges) } else { (board.clone(), with_edges) };
            let contacts = track_contacts(&held);
            let identities: IndexSet<String> = contacts.iter().map(|c| c.identity.clone()).collect();
            assert_eq!(identities, (0..edges.len()).map(|i| format!("track:{i}")).collect::<IndexSet<_>>(), "{case}");
            // This is the exact gap in the original draft's centreline-only guard.
            for (i, t) in parts.0.iter().enumerate() {
                assert!(edges.iter().all(|h| !segment_distance_squared(t.start, t.end, h.start, h.end).is_zero()));
                assert!(contacts.iter().any(|c| copper_contacts_overlap(&target_contact(i, t), c)), "{case}");
            }
            let (mut a, mut b) = parts.clone();
            assert_eq!(plain_tuner(&mut a.clone(), &mut b.clone(), SKEW, AMP, 32).added_nm, 0);
            assert_eq!(span_tuning(&mut a, &mut b, &source, &held), 0, "{case}");
            assert!(a == parts.0 && b == parts.1);
            assert_eq!(held.tracks, edges);
            assert!(source.placements == board.placements && source.net_routing_rules == board.net_routing_rules);
        }
    }
}

#[test]
fn identical_back_copper_is_not_a_front_contact() {
    let (board, lanes) = standard();
    let parts = (fragment(&lanes.0), fragment(&lanes.1));
    let rail = track("A", at(5 * MM, 4_950_000), at(15 * MM, 4_950_000), 100_000, BACK);
    let mut held = board.clone();
    held.tracks = vec![rail.clone()];
    let contacts = track_contacts(&held);
    assert!(
        !parts
            .0
            .iter()
            .enumerate()
            .any(|(i, t)| contacts.iter().any(|c| copper_contacts_overlap(&target_contact(i, t), c)))
    );
    let (mut a, mut b) = parts.clone();
    assert_eq!(plain_tuner(&mut a.clone(), &mut b.clone(), SKEW, AMP, 32).added_nm, 0);
    assert!(span_tuning(&mut a, &mut b, &board, &held) > 0);
    assert_eq!(held.tracks, vec![rail]);
}

#[test]
fn pad_contact_ids_are_actual_footprint_indices() {
    let (board, _) = standard();
    let only = IndexSet::from(["A".to_owned()]);
    let candidate = Candidate { only_nets: Some(&only), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, pads, _) = explicit_copper_contacts(&board, candidate).expect("contacts");
    let expected = IndexSet::from(["pad:J1.1:0".to_owned(), "pad:J2.1:0".to_owned()]);
    assert_eq!(contacts.iter().map(|c| c.identity.clone()).collect::<IndexSet<_>>(), expected);
    assert_eq!(pads.values().flatten().cloned().collect::<IndexSet<_>>(), expected);
}

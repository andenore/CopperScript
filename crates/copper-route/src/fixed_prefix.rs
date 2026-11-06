//! Explicit generated fixed copper on an unchanged canonical source frame.
//!
//! Local retained dogbones/via sites are distinct from generated mutable
//! channels. Role labels and fresh DRC do not prove a complete package escape
//! or channel to the boundary. Original-pad access is separately checked by
//! `escape_promises`. This module performs no routing or source repair.

use std::ops::Range;

use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::{Error, Result};
use indexmap::IndexSet;

/// Derive an admission context from an explicitly declared producer context.
/// The baseline must be the caller's canonical materialized clean source.
/// Unlike repeated-stage mask removal, every declared temporary occurrence
/// must be present above all authored occurrences. This only removes masks;
/// the caller must still freshly admit the complete derived source/copper.
pub(crate) fn strip_declared_temporary_keepouts(
    canonical_baseline: &PhysicalBoard,
    raw: &PhysicalBoard,
    declared: &[copper_physical::CopperKeepout],
) -> Result<PhysicalBoard> {
    let mut inspected = Vec::new();
    for mask in canonical_baseline.copper_keepouts.iter().chain(declared) {
        if inspected.contains(mask) {
            continue;
        }
        inspected.push(mask.clone());
        let authored = canonical_baseline.copper_keepouts.iter().filter(|item| *item == mask).count();
        let owned = declared.iter().filter(|item| *item == mask).count();
        let present = raw.copper_keepouts.iter().filter(|item| *item == mask).count();
        let required = authored
            .checked_add(owned)
            .ok_or_else(|| Error::Value("producer keepout occurrence count is unrepresentable".to_owned()))?;
        if present < required {
            return Err(Error::Value(
                "producer context is missing authored or declared temporary keepout occurrences".to_owned(),
            ));
        }
    }
    let mut derived = raw.clone();
    for mask in declared.iter().rev() {
        let occurrence = derived
            .copper_keepouts
            .iter()
            .rposition(|item| item == mask)
            .expect("all exact declared occurrences were counted before removal");
        derived.copper_keepouts.remove(occurrence);
    }
    Ok(derived)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedPrefixRole {
    /// An explicitly retained local pad launch/site, not its mutable onward
    /// channel and not a claim that the package boundary is reachable.
    OrdinaryEscape,
    /// Explicitly allocated package via-site/dogbone copper only.
    ViaLattice,
}

#[cfg(test)]
#[path = "fixed_prefix_admission_tests.rs"]
mod admission_tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedContribution {
    pub id: String,
    pub role: FixedPrefixRole,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
}

/// Exact ordered occurrence ranges in the final frame, never a geometry set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContributionRange {
    pub id: String,
    pub role: FixedPrefixRole,
    pub tracks: Range<usize>,
    pub vias: Range<usize>,
}

/// Constructible only by fresh admission. No mutable board/range/digest access.
#[derive(Clone, Debug)]
pub struct AdmittedFixedPrefix {
    original_source: PhysicalBoard,
    baseline: PhysicalBoard,
    board: PhysicalBoard,
    contributions: Vec<FixedContribution>,
    ranges: Vec<ContributionRange>,
    observational_digest: String,
}

impl AdmittedFixedPrefix {
    pub fn admit(source: &PhysicalBoard, contributions: &[FixedContribution]) -> Result<Self> {
        // macro_source rejects undeclared ordinary/critical copper. Never use
        // a routed board as a new source or strip masks by naming convention.
        // Every temporary halo/keepout context must be supplied explicitly as
        // clean source intent by the caller; this helper removes nothing.
        let baseline =
            copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(source.clone())?)?;
        let mut board = baseline.clone();
        let mut ids = IndexSet::new();
        let mut ranges = Vec::new();
        for contribution in contributions {
            if contribution.id.is_empty() || !ids.insert(contribution.id.as_str()) {
                return Err(Error::Value("fixed-prefix contribution IDs must be nonempty and unique".to_owned()));
            }
            let (track_start, via_start) = (board.tracks.len(), board.vias.len());
            // Multiplicity and declaration order are intentional. No contains,
            // net-based owner inference or coordinate deduplication is allowed.
            board.tracks.extend(contribution.tracks.iter().cloned());
            board.vias.extend(contribution.vias.iter().cloned());
            ranges.push(ContributionRange {
                id: contribution.id.clone(),
                role: contribution.role,
                tracks: track_start..board.tracks.len(),
                vias: via_start..board.vias.len(),
            });
        }
        let observational_digest = crate::global_reserved_access::admit_fixed_prefix(&baseline, &board)?;
        Ok(Self {
            original_source: source.clone(),
            baseline,
            board,
            contributions: contributions.to_vec(),
            ranges,
            observational_digest,
        })
    }

    pub fn baseline(&self) -> &PhysicalBoard {
        &self.baseline
    }
    /// Exact clean caller context, including its source-authored keepouts.
    pub fn source(&self) -> &PhysicalBoard {
        &self.original_source
    }
    /// Ordered declarations for typed replay; reconstruct through `admit`.
    pub fn contributions(&self) -> &[FixedContribution] {
        &self.contributions
    }
    pub fn board(&self) -> &PhysicalBoard {
        &self.board
    }
    pub fn ranges(&self) -> &[ContributionRange] {
        &self.ranges
    }
    pub fn observational_digest(&self) -> &str {
        &self.observational_digest
    }

    /// Audit exact copper occurrence preservation after consumer cleanup.
    /// Presence alone does not certify geometry or complete package escapes.
    pub fn missing_copper_occurrences(&self, current: &PhysicalBoard) -> (i64, i64) {
        let tracks = crate::route_cleanup::counter(&current.tracks);
        let vias = crate::route_cleanup::via_counter(&current.vias);
        let missing_tracks = crate::route_cleanup::counter(&self.board.tracks)
            .iter()
            .map(|(track, count)| (count - tracks.get(track).copied().unwrap_or(0)).max(0))
            .sum();
        let missing_vias = crate::route_cleanup::via_counter(&self.board.vias)
            .iter()
            .map(|(via, count)| (count - vias.get(via).copied().unwrap_or(0)).max(0))
            .sum();
        (missing_tracks, missing_vias)
    }

    /// Freshly prove the same source and contribution frame. Changed source
    /// intent (including keepouts/metadata) requires explicit new admission;
    /// this is not a rebind granting old world-space copper new authority.
    pub fn revalidate_for(&self, current_source: &PhysicalBoard) -> Result<Self> {
        if current_source != &self.original_source {
            return Err(Error::Value("fixed-prefix exact source changed; explicitly re-admit its context".to_owned()));
        }
        let fresh = Self::admit(current_source, &self.contributions)?;
        if fresh.baseline != self.baseline || fresh.board != self.board || fresh.ranges != self.ranges {
            return Err(Error::Value("fixed-prefix frame differs from its exact declared occurrences".to_owned()));
        }
        // A cached or forged digest is never an admission credential.
        Ok(fresh)
    }

    /// Renew optional generated ownership after a removal/shortening trial.
    /// The original clean source remains the admission source. Every changed
    /// occurrence must have one unique ordinary owner; all other copper stays
    /// exact. This is proposal admission, not native/dependency acceptance.
    pub(crate) fn readmit_ordinary_retirement(&self, before: &PhysicalBoard, after: &PhysicalBoard) -> Result<Self> {
        use crate::dependency_reroute::{multiset, subtract};
        use copper_physical::geometry::point_on_segment;
        let reject = || Error::Value("optional retirement changed unowned, ambiguous or mandatory copper".to_owned());
        self.revalidate_for(self.source())?;
        if self.missing_copper_occurrences(before) != (0, 0) {
            return Err(reject());
        }
        let mut intent = after.clone();
        intent.tracks = before.tracks.clone();
        intent.vias = before.vias.clone();
        if intent != *before {
            return Err(reject());
        }
        let (bt, bv) = (multiset(&before.tracks), multiset(&before.vias));
        let (pt, pv) = (multiset(&self.board.tracks), multiset(&self.board.vias));
        let removed_tracks = subtract(&bt, &multiset(&after.tracks));
        let removed_vias = subtract(&bv, &multiset(&after.vias));
        let added_tracks = subtract(&multiset(&after.tracks), &bt);
        if !subtract(&multiset(&after.vias), &bv).is_empty() {
            return Err(reject());
        }
        let mut parts = self.contributions.clone();
        let mut owners = Vec::new();
        for (track, count) in &removed_tracks {
            let matches: Vec<_> = parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.role == FixedPrefixRole::OrdinaryEscape && p.tracks.contains(track))
                .map(|(i, _)| i)
                .collect();
            let [owner] = matches.as_slice() else { return Err(reject()) };
            if *count != 1 || bt.get(track) != Some(&1) || pt.get(track) != Some(&1) {
                return Err(reject());
            }
            owners.push((track.clone(), *owner));
        }
        for (via, count) in &removed_vias {
            let matches: Vec<_> = parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.role == FixedPrefixRole::OrdinaryEscape && p.vias.contains(via))
                .map(|(i, _)| i)
                .collect();
            let [owner] = matches.as_slice() else { return Err(reject()) };
            if *count != 1 || bv.get(via) != Some(&1) || pv.get(via) != Some(&1) {
                return Err(reject());
            }
            parts[*owner].vias.retain(|v| v != via);
        }
        let mut replacements = Vec::new();
        for (track, count) in &added_tracks {
            let matches: Vec<_> = owners
                .iter()
                .filter(|(old, _)| {
                    old.net == track.net
                        && old.layer == track.layer
                        && old.width_nm == track.width_nm
                        && track.start != track.end
                        && point_on_segment(track.start, old.start, old.end)
                        && point_on_segment(track.end, old.start, old.end)
                })
                .collect();
            let [(old, owner)] = matches.as_slice() else { return Err(reject()) };
            if *count != 1 || bt.contains_key(track) || replacements.iter().any(|(original, _, _)| original == old) {
                return Err(reject());
            }
            replacements.push((old.clone(), *owner, track.clone()));
        }
        for (old, owner) in owners {
            let part = &mut parts[owner];
            let position = part.tracks.iter().position(|t| *t == old).ok_or_else(reject)?;
            match replacements.iter().find(|(original, _, _)| *original == old) {
                Some((_, _, replacement)) => part.tracks[position] = replacement.clone(),
                None => {
                    part.tracks.remove(position);
                }
            }
        }
        let fresh = Self::admit(self.source(), &parts)?;
        if fresh.missing_copper_occurrences(after) != (0, 0) {
            return Err(reject());
        }
        Ok(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support_routers as routers;
    use copper_pcb::board::{PadReference, PadViaInPadRule, Stackup};
    use copper_physical::footprint::PadShape;
    use copper_physical::{CopperLayer, DesignRules};

    fn source() -> PhysicalBoard {
        routers::pad_board(&[("S", &[(2.0, 2.0), (18.0, 2.0)]), ("T", &[])], 2, Vec::new())
    }

    fn contribution(id: &str, role: FixedPrefixRole, tracks: Vec<TrackSegment>, vias: Vec<Via>) -> FixedContribution {
        FixedContribution { id: id.to_owned(), role, tracks, vias }
    }

    #[test]
    fn empty_contributions_are_the_exact_legacy_materialized_frame_and_keep_all_context() {
        let mut source = source();
        source.copper_keepouts.push(routers::copper_keepout(
            "authored-or-explicit-context",
            &[CopperLayer::Front],
            routers::rect(1.0, 20.0, 3.0, 22.0),
        ));
        let legacy = copper_drc::macros::materialize_hard_macros(
            copper_pcb::hard_macros::macro_source(source.clone()).expect("clean source"),
        )
        .expect("legacy frame");
        let frame = AdmittedFixedPrefix::admit(&source, &[]).expect("empty admission");
        assert_eq!(frame.board(), &legacy);
        assert_eq!(frame.baseline(), &legacy);
        assert!(frame.ranges().is_empty());
        assert_eq!(frame.board().copper_keepouts, source.copper_keepouts);
        assert_eq!(frame.revalidate_for(&source).expect("fresh proof").board(), &legacy);
    }

    #[test]
    fn ordered_ranges_preserve_duplicate_track_occurrences_and_role_is_not_bypass_ownership() {
        let source = source();
        let track = routers::track("S", (5.0, 5.0), (7.0, 5.0), 0.2, CopperLayer::Front);
        let via = routers::via("S", (7.0, 5.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
        let contributions = vec![
            contribution(
                "ordinary",
                FixedPrefixRole::OrdinaryEscape,
                vec![track.clone(), track.clone()],
                vec![via.clone()],
            ),
            contribution(
                "lattice",
                FixedPrefixRole::ViaLattice,
                vec![routers::track("T", (16.0, 16.0), (18.0, 16.0), 0.2, CopperLayer::Front)],
                Vec::new(),
            ),
        ];
        let frame = AdmittedFixedPrefix::admit(&source, &contributions).expect("admission");
        assert_eq!(frame.ranges()[0].tracks, 0..2);
        assert_eq!(frame.ranges()[0].vias, 0..1);
        assert_eq!(frame.ranges()[1].tracks, 2..3);
        assert_eq!(frame.ranges()[1].vias, 1..1);
        assert_eq!(&frame.board().tracks[frame.ranges()[0].tracks.clone()], &[track.clone(), track.clone()]);
        assert_eq!(&frame.board().vias[frame.ranges()[0].vias.clone()], &[via]);
        let fake_bypass = crate::bypass_reservations::BypassReservation::new(frame.board(), &[track], &[], &[]);
        assert!(crate::bypass_reservations::restore_bypass_reservation_edges(&fake_bypass, false).is_err());
        let (_, _, reason) = crate::bypass_reservations::verified_bypass_reservations(frame.board(), &fake_bypass)
            .expect("strict bypass proof");
        assert!(reason.expect("cannot claim fixed occurrence").contains("immutable"));
    }

    #[test]
    fn arbitrary_source_copper_duplicate_ids_and_real_cross_net_collisions_are_rejected() {
        let source = source();
        let track = routers::track("S", (5.0, 5.0), (10.0, 5.0), 0.2, CopperLayer::Front);
        let mut routed_source = source.clone();
        routed_source.tracks.push(track.clone());
        assert!(AdmittedFixedPrefix::admit(&routed_source, &[]).is_err());
        let first = contribution("same", FixedPrefixRole::OrdinaryEscape, vec![track.clone()], Vec::new());
        assert!(AdmittedFixedPrefix::admit(&source, &[first.clone(), first]).is_err());
        let crossing = routers::track("T", (7.0, 4.0), (7.0, 6.0), 0.2, CopperLayer::Front);
        let error = AdmittedFixedPrefix::admit(
            &source,
            &[
                contribution("one", FixedPrefixRole::OrdinaryEscape, vec![track], Vec::new()),
                contribution("two", FixedPrefixRole::ViaLattice, vec![crossing], Vec::new()),
            ],
        )
        .expect_err("fresh exact DRC");
        assert!(error.to_string().contains("fresh physical DRC"), "{error}");
    }

    #[test]
    fn physical_role_admission_does_not_certify_original_pad_access() {
        let source = source();
        let via = routers::via("S", (12.0, 12.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
        let frame = AdmittedFixedPrefix::admit(
            &source,
            &[contribution("unproven-pad-promise", FixedPrefixRole::OrdinaryEscape, Vec::new(), vec![via.clone()])],
        )
        .expect("physically legal fixed copper, not access proof");
        let clearance = crate::routing_clearance::RoutingClearanceIndex::new(frame.board()).expect("clearance");
        let path = crate::pin_escape::verified_fanout_path(
            frame.board(),
            &PadReference::new("S_0", "1"),
            "S",
            via.position,
            &clearance,
        )
        .expect("fresh original pad access check");
        assert!(path.is_none(), "a role label must not substitute for a real original-pad copper path");
    }

    #[test]
    fn fresh_revalidation_rejects_changed_source_context_and_never_trusts_digest() {
        let source = source();
        let frame = AdmittedFixedPrefix::admit(&source, &[]).expect("admit");
        for change in ["pose", "rule", "metadata", "keepout"] {
            let mut changed = source.clone();
            match change {
                "pose" => changed.placements[0].position.x_nm += 1,
                "rule" => changed.rules.minimum_clearance_nm += 1,
                "metadata" => {
                    changed.metadata.insert("unproved".to_owned(), "true".to_owned());
                }
                _ => changed.copper_keepouts.push(routers::copper_keepout(
                    "new-context",
                    &[CopperLayer::Front],
                    routers::rect(1.0, 20.0, 3.0, 22.0),
                )),
            }
            assert!(frame.revalidate_for(&changed).is_err(), "{change}");
        }
        let expected = frame.observational_digest().to_owned();
        let mut forged_digest = frame;
        forged_digest.observational_digest = "forged".to_owned();
        let fresh = forged_digest.revalidate_for(&source).expect("fresh computation");
        assert_eq!(fresh.observational_digest(), expected);
        assert_ne!(fresh.observational_digest(), "forged");
    }

    fn array_source() -> PhysicalBoard {
        let ep = routers::footprint(
            "test/prefix-array",
            vec![copper_physical::footprint::FootprintPad {
                shape: PadShape::Rectangle,
                ..routers::pad("1", 0.0, 0.0, 3.1, 3.1)
            }],
            4.0,
            4.0,
        );
        let mut board = routers::board(
            "prefix-array",
            30.0,
            25.0,
            vec![ep.clone()],
            vec![routers::placement("U1", &ep.name, 10.0, 10.0)],
            vec![routers::net("GND", &[("U1", "1")]), routers::net("S", &[])],
        );
        board.stackup = Stackup::new(routers::layers(6));
        board.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
        board.rules = DesignRules {
            minimum_clearance_nm: 90_000,
            minimum_track_width_nm: 90_000,
            default_track_width_nm: 200_000,
            ..DesignRules::default()
        };
        board.zones.push(routers::zone("ground", "GND", &[CopperLayer::In1], routers::rect(1.0, 1.0, 29.0, 24.0)));
        board.via_in_pad_rules.push(PadViaInPadRule {
            pad: PadReference::new("U1", "1"),
            rows: Some(3),
            columns: Some(3),
            pitch_nm: Some(750_000),
        });
        board
    }

    #[test]
    fn required_array_prefix_stays_exact_and_cannot_be_dropped_reordered_or_claimed() {
        crate::install_hooks();
        let source = array_source();
        let append = routers::via("S", (20.0, 20.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
        let frame = AdmittedFixedPrefix::admit(
            &source,
            &[contribution("lattice", FixedPrefixRole::ViaLattice, Vec::new(), vec![append.clone()])],
        )
        .expect("array and fixed contribution");
        assert_eq!(frame.baseline().vias.len(), 9);
        assert_eq!(&frame.board().vias[..9], frame.baseline().vias.as_slice());
        assert_eq!(frame.ranges()[0].vias, 9..10);
        assert_eq!(frame.board().vias[9], append);
        for damage in ["drop", "duplicate", "reorder", "rule"] {
            let mut damaged = frame.clone();
            match damage {
                "drop" => {
                    damaged.board.vias.remove(0);
                }
                "duplicate" => damaged.board.vias.push(damaged.board.vias[0].clone()),
                "reorder" => damaged.board.vias.swap(0, 1),
                _ => damaged.original_source.via_in_pad_rules[0].rows = Some(2),
            }
            assert!(damaged.revalidate_for(&source).is_err(), "{damage}");
        }
        let fake = crate::bypass_reservations::BypassReservation::new(
            frame.board(),
            &[],
            &[frame.board().vias[0].clone()],
            &[],
        );
        assert!(crate::bypass_reservations::restore_bypass_reservation_edges(&fake, false).is_err());
        assert_eq!(frame.revalidate_for(&source).expect("fresh intact frame").board(), frame.board());
        assert!(source.tracks.is_empty() && source.vias.is_empty());
    }

    #[test]
    fn actual_plane_stitching_cannot_take_an_admitted_signal_escape_site() {
        let fp = routers::footprint(
            "test/adjacent-signal-ground",
            // World pads remain S(10,10), GND(10.75,10). Ground-centered
            // package origin avoids favoring an east tap beyond its neighbor.
            vec![routers::pad("S", -0.75, 0.0, 0.25, 0.25), routers::pad("G", 0.0, 0.0, 0.25, 0.25)],
            2.0,
            1.0,
        );
        let mut source = routers::board(
            "plane-versus-fixed-escape",
            26.0,
            26.0,
            vec![fp.clone()],
            vec![routers::placement("U1", &fp.name, 10.75, 10.0)],
            vec![routers::net("S", &[("U1", "S")]), routers::net("GND", &[("U1", "G")])],
        );
        source.stackup = Stackup::new(routers::layers(6));
        source.rules = DesignRules {
            minimum_clearance_nm: 100_000,
            minimum_track_width_nm: 100_000,
            default_track_width_nm: 150_000,
            default_via_size_nm: 450_000,
            default_via_drill_nm: 200_000,
            minimum_hole_clearance_nm: 200_000,
            ..DesignRules::default()
        };
        source.zones.push(routers::zone("ground", "GND", &[CopperLayer::In1], routers::rect(1.0, 1.0, 25.0, 25.0)));
        let ground_pad = PadReference::new("U1", "G");
        let options = crate::plane::PlaneStitchOptions {
            candidate_bias: Some("north".to_owned()),
            only_pads: Some([ground_pad.clone()].into_iter().collect()),
            ..Default::default()
        };
        let source_only = crate::plane::stitch_zone_pads(&source, Some(&options)).expect("source-only plane tap");
        assert!(source_only.stitched_pads.contains(&ground_pad), "the unprotected plane contact must actually route");
        let original_ground_via =
            source_only.board.vias.iter().find(|via| via.net == "GND").expect("actual source-only GND via").clone();
        let mut signal_via = original_ground_via.clone();
        signal_via.net = "S".to_owned();
        let index = crate::routing_clearance::RoutingClearanceIndex::new(&source).expect("source clearance");
        let tracks = crate::pin_escape::checked_access_path(
            &source,
            &index,
            "S",
            routers::pt(10.0, 10.0),
            signal_via.position,
            150_000,
            CopperLayer::Front,
            false,
            true,
            None,
        )
        .expect("signal path check")
        .expect("the fixture must support a real legal signal escape");
        let frame = AdmittedFixedPrefix::admit(
            &source,
            &[contribution("ordinary-signal", FixedPrefixRole::OrdinaryEscape, tracks, vec![signal_via.clone()])],
        )
        .expect("freshly legal signal prefix at the former plane contact site");
        let stitched =
            crate::plane::stitch_zone_pads(frame.board(), Some(&options)).expect("plane tap beside fixed signal");
        assert!(stitched.board.tracks.starts_with(&frame.board().tracks));
        assert!(stitched.board.vias.starts_with(&frame.board().vias));
        assert!(
            !stitched.board.vias.iter().any(|via| via.net == "GND" && via.position == original_ground_via.position),
            "the plane must not reuse the signal's retained site"
        );
        assert!(
            stitched.stitched_pads.contains(&ground_pad) || stitched.pending_pads.contains(&ground_pad),
            "the GND obligation remains explicit if no alternative survives"
        );
        crate::global_reserved_access::admit_fixed_prefix(frame.board(), &stitched.board)
            .expect("complete merged frame must freshly pass physical DRC");
        let fresh = crate::routing_clearance::RoutingClearanceIndex::new(&stitched.board).expect("merged clearance");
        assert!(
            crate::pin_escape::verified_fanout_path(
                &stitched.board,
                &PadReference::new("U1", "S"),
                "S",
                signal_via.position,
                &fresh
            )
            .expect("fresh original-pad proof")
            .is_some(),
            "the original signal pad must still own a complete legal escape after actual stitching"
        );
        assert!(source.tracks.is_empty() && source.vias.is_empty());
    }
}

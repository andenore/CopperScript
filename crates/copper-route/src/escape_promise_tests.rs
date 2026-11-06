use super::*;
use crate::fixed_prefix::FixedContribution;
use crate::pin_escape::RoutingAccess;
use crate::test_support_routers as routers;
use copper_pcb::board::{NetRoutingRule, TrackSegment};
use copper_physical::Point;
use indexmap::IndexMap;

fn source() -> PhysicalBoard {
    routers::pad_board(&[("S", &[(2.0, 2.0), (18.0, 2.0)]), ("T", &[])], 2, Vec::new())
}

fn pad() -> PadReference {
    PadReference::new("S_0", "1")
}

fn launch(at: (f64, f64)) -> (Vec<TrackSegment>, Vec<Via>) {
    (
        vec![routers::track("S", (5.0, 5.0), at, 0.2, CopperLayer::Front)],
        vec![routers::via("S", at, 0.6, 0.3, CopperLayer::Front, CopperLayer::Back)],
    )
}

fn fanout(
    source: &PhysicalBoard,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    anchor: Option<Point>,
    surface: Option<RoutingAccess>,
    pending: Vec<PadReference>,
) -> FanoutResult {
    let mut board = source.clone();
    board.tracks.extend(tracks.iter().cloned());
    board.vias.extend(vias.iter().cloned());
    FanoutResult {
        board,
        accesses: anchor.map_or_else(IndexMap::new, |anchor| IndexMap::from([(pad(), anchor)])),
        pending_pads: pending,
        added_track_count: tracks.len() as i64,
        added_via_count: vias.len() as i64,
        created_tracks: tracks,
        created_vias: vias,
        assignment: None,
        surface_accesses: surface.map(|access| IndexMap::from([(pad(), access)])),
        decoupling_tracks: Vec::new(),
        decoupling_vias: Vec::new(),
        decoupling_witnesses: Vec::new(),
        decoupling_pending_links: Vec::new(),
    }
}

fn frame(
    source: &PhysicalBoard,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    role: FixedPrefixRole,
) -> AdmittedFixedPrefix {
    AdmittedFixedPrefix::admit(
        source,
        &[FixedContribution { id: "explicit-current-batch".to_owned(), role, tracks, vias }],
    )
    .expect("fresh current physical frame")
}

#[test]
fn actual_fanout_fields_are_retained_and_freshly_verified_without_inferred_promises() {
    let dense = routers::footprint(
        "test/dense-promises",
        (0..12).map(|i| routers::pad(&(i + 1).to_string(), -2.75 + 0.5 * f64::from(i), 0.0, 0.25, 0.5)).collect(),
        6.0,
        2.0,
    );
    let terminal = routers::one_pad();
    let source = routers::board(
        "real-fanout-promises",
        25.0,
        20.0,
        vec![dense.clone(), terminal.clone()],
        vec![routers::placement("U1", &dense.name, 8.0, 10.0), routers::placement("J1", &terminal.name, 20.0, 10.0)],
        vec![routers::net("S", &[("U1", "1"), ("J1", "1")])],
    );
    let result = crate::fanout::route_fanout(&source, None, None).expect("actual fanout");
    assert!(
        !result.accesses.is_empty() || result.surface_accesses.as_ref().is_some_and(|accesses| !accesses.is_empty())
    );
    let ledger = EscapePromiseLedger::capture(&source, &result).expect("direct producer capture");
    assert_eq!(ledger.pending(), result.pending_pads.as_slice());
    assert_eq!(
        ledger.promises().len(),
        result.accesses.len() + result.surface_accesses.as_ref().map_or(0, IndexMap::len)
    );
    for promise in ledger.promises() {
        match promise.provenance {
            AnchorProvenance::FanoutViaAccess => {
                assert_eq!(promise.original_anchor, Anchor::Point(result.accesses[&promise.pad]))
            }
            AnchorProvenance::FanoutSurfaceAccess => assert_eq!(
                promise.original_anchor,
                Anchor::Access(result.surface_accesses.as_ref().expect("surface producer")[&promise.pad].clone())
            ),
        }
    }
    let current = frame(&source, result.created_tracks, result.created_vias, FixedPrefixRole::OrdinaryEscape);
    let checks = ledger.validate(&current, current.board()).expect("fresh current proof");
    assert!(checks.promises.iter().all(|check| check.state == PromiseState::VerifiedOriginal));
}

#[test]
fn stale_original_via_site_can_only_adapt_to_a_fresh_owned_ordinary_site() {
    let source = source();
    let (old_tracks, old_vias) = launch((5.5, 5.5));
    let original = fanout(&source, old_tracks, old_vias, Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("capture");
    let (tracks, vias) = launch((5.5, 4.5));
    let current = frame(&source, tracks.clone(), vias.clone(), FixedPrefixRole::OrdinaryEscape);
    let checks = ledger.validate(&current, current.board()).expect("fresh resited proof");
    assert_eq!(checks.promises[0].state, PromiseState::VerifiedResitedVia);
    assert_eq!(checks.promises[0].original.original_anchor, Anchor::Point(routers::pt(5.5, 5.5)));
    assert_eq!(checks.promises[0].verified_anchor, Some(Anchor::Point(routers::pt(5.5, 4.5))));
    assert_eq!(checks.promises[0].owned_candidates_checked, 1);
    let lattice = frame(&source, tracks.clone(), vias.clone(), FixedPrefixRole::ViaLattice);
    let unowned = ledger.validate(&lattice, lattice.board()).expect("roles remain explicit");
    assert_eq!(unowned.promises[0].state, PromiseState::NoOwnedOrdinaryVia);
    assert!(unowned.promises[0].verified_anchor.is_none());
    // Even real mutable same-net copper must not invent the retained pad path.
    let floating = frame(&source, Vec::new(), vias, FixedPrefixRole::OrdinaryEscape);
    let mut with_mutable_path = floating.board().clone();
    with_mutable_path.tracks.extend(tracks);
    let missing = ledger.validate(&floating, &with_mutable_path).expect("complete context");
    assert_eq!(missing.promises[0].state, PromiseState::OriginalPadAccessUnresolved);
    assert!(missing.promises[0].verified_anchor.is_none());
}

#[test]
fn collisions_reject_without_mutating_the_original_or_current_ledger() {
    let source = source();
    let (tracks, vias) = launch((5.5, 5.5));
    let original = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("capture");
    let current = frame(&source, tracks, vias, FixedPrefixRole::OrdinaryEscape);
    let saved = current.board().clone();
    let mut collision = saved.clone();
    let mut foreign = collision.vias[0].clone();
    foreign.net = "T".to_owned();
    collision.vias.push(foreign);
    assert!(ledger.validate(&current, &collision).is_err());
    assert_eq!(current.board(), &saved);
    assert_eq!(ledger.promises()[0].original_anchor, Anchor::Point(routers::pt(5.5, 5.5)));
    assert_eq!(
        ledger.validate(&current, current.board()).expect("incumbent still valid").promises[0].state,
        PromiseState::VerifiedOriginal
    );
}

#[test]
fn missing_pad_missing_provenance_and_pending_records_cannot_create_certified_promises() {
    let source = source();
    let (tracks, vias) = launch((5.5, 5.5));
    let mut original = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    original.accesses = IndexMap::from([(PadReference::new("S_0", "missing"), routers::pt(5.5, 5.5))]);
    assert!(EscapePromiseLedger::capture(&source, &original).is_err());
    original.accesses.clear();
    original.pending_pads = vec![pad()];
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("unproven pending source data");
    let current = frame(&source, tracks, vias, FixedPrefixRole::OrdinaryEscape);
    let checks = ledger.validate(&current, current.board()).expect("pending remains visible");
    assert!(checks.promises.is_empty());
    assert_eq!(checks.pending, vec![pad()]);
    // A claimed original point without its real original launch is not a
    // preserved promise, even if a later explicit batch can reach a new site.
    original.accesses.insert(pad(), routers::pt(9.0, 9.0));
    original.pending_pads.clear();
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("capture unverified original claim");
    assert!(!ledger.promises()[0].original_verified);
    let checks = ledger.validate(&current, current.board()).expect("fresh current frame");
    assert_eq!(checks.promises[0].state, PromiseState::OriginalAnchorUnverified);
    assert!(checks.promises[0].verified_anchor.is_none());
}

#[test]
fn original_surface_path_is_checked_but_changed_surface_ports_are_not_guessed() {
    let mut source = source();
    source.net_routing_rules.push(NetRoutingRule {
        width_nm: Some(200_000),
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..routers::rule("S")
    });
    let old = routers::track("S", (5.0, 5.0), (7.0, 5.0), 0.2, CopperLayer::Front);
    let access = RoutingAccess::new(old.end, old.layer, old.start, vec![old.clone()]).expect("surface producer");
    let original = fanout(&source, vec![old.clone()], Vec::new(), None, Some(access), Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("surface capture");
    let current = frame(&source, vec![old], Vec::new(), FixedPrefixRole::OrdinaryEscape);
    assert_eq!(
        ledger.validate(&current, current.board()).expect("surface proof").promises[0].state,
        PromiseState::VerifiedOriginal
    );
    let adapted = frame(
        &source,
        vec![routers::track("S", (5.0, 5.0), (5.0, 7.0), 0.2, CopperLayer::Front)],
        Vec::new(),
        FixedPrefixRole::OrdinaryEscape,
    );
    let checks = ledger.validate(&adapted, adapted.board()).expect("explicit unsupported adaptation");
    assert_eq!(checks.promises[0].state, PromiseState::SurfaceAdaptationUnsupported);
    assert!(checks.promises[0].verified_anchor.is_none());
}

#[test]
fn same_number_multiple_physical_lands_remain_explicitly_unsupported() {
    let mut source = source();
    let name = source.placements[0].footprint.clone();
    let footprint = source.footprints.get_mut(&name).expect("source footprint");
    let mut second = footprint.pads[0].clone();
    second.position.x_nm += 3_000_000;
    footprint.pads.push(second);
    let (tracks, vias) = launch((5.5, 5.5));
    let original = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("unsupported fact retained");
    let current = frame(&source, tracks, vias, FixedPrefixRole::OrdinaryEscape);
    let checks = ledger.validate(&current, current.board()).expect("unsupported result");
    assert_eq!(checks.promises[0].state, PromiseState::UnsupportedMultiplePhysicalLands);
    assert!(checks.promises[0].verified_anchor.is_none());
}

#[test]
fn newly_owned_signal_via_must_satisfy_the_original_return_rule() {
    let mut source = source();
    source.net_routing_rules.push(NetRoutingRule {
        max_vias: Some(1),
        require_return_vias: true,
        return_via_net: Some("T".to_owned()),
        maximum_return_via_distance_nm: Some(2_000_000),
        ..routers::rule("S")
    });
    let (tracks, mut vias) = launch((5.5, 5.5));
    vias.push(routers::via("T", (7.0, 5.5), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back));
    let original = fanout(&source, tracks, vias, Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("capture original with real return barrel");
    assert!(ledger.promises()[0].original_verified);
    let (tracks, vias) = launch((5.5, 4.5));
    // The existing physical admission already refuses this candidate before
    // any pad ledger can certify it; retain that stronger boundary.
    let missing = AdmittedFixedPrefix::admit(
        &source,
        &[FixedContribution {
            id: "missing-required-return".to_owned(),
            role: FixedPrefixRole::OrdinaryEscape,
            tracks: tracks.clone(),
            vias: vias.clone(),
        }],
    )
    .expect_err("a missing required return cannot become an admitted current frame");
    assert!(missing.to_string().contains("DRC-RETURN-VIA"), "{missing}");
    let mut returned = vias;
    returned.push(routers::via("T", (7.0, 4.5), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back));
    let current = frame(&source, tracks, returned, FixedPrefixRole::OrdinaryEscape);
    assert_eq!(
        ledger.validate(&current, current.board()).expect("fresh owned site and return proof").promises[0].state,
        PromiseState::VerifiedResitedVia
    );
}

#[test]
fn declared_context_removal_retains_authored_equal_occurrences_and_raw_producer_evidence() {
    let mut source = source();
    let authored =
        routers::copper_keepout("authored-equal-halo", &[CopperLayer::Front], routers::rect(18.0, 18.0, 20.0, 20.0));
    let other =
        routers::copper_keepout("producer-plane-halo", &[CopperLayer::Front], routers::rect(13.0, 18.0, 14.0, 19.0));
    source.copper_keepouts.push(authored.clone());
    let (tracks, vias) = launch((5.5, 5.5));
    let mut raw = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let declared = vec![authored.clone(), other];
    raw.board.copper_keepouts.extend(declared.iter().cloned());
    let saved_raw = raw.board.clone();
    assert!(EscapePromiseLedger::capture(&source, &raw).is_err(), "undeclared producer context cannot be ignored");
    let ledger = EscapePromiseLedger::capture_in_context(&source, &raw, &declared).expect("exact declared derivation");
    assert_eq!(ledger.source().copper_keepouts, vec![authored]);
    assert_eq!(ledger.declared_temporary_keepouts(), declared);
    assert_eq!(ledger.original_fanout().board, saved_raw);
    assert_eq!(raw.board, saved_raw);
    assert!(ledger.promises()[0].original_verified);
    let current = frame(&source, tracks, vias, FixedPrefixRole::OrdinaryEscape);
    assert_eq!(
        ledger.validate(&current, current.board()).expect("same authoritative clean source").promises[0].state,
        PromiseState::VerifiedOriginal
    );
}

#[test]
fn context_cleanup_cannot_invent_original_pad_access_that_raw_halos_blocked() {
    let source = source();
    let (tracks, vias) = launch((5.5, 5.5));
    let mut raw = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let blocker = routers::copper_keepout(
        "raw-producer-blocker",
        &[CopperLayer::Front, CopperLayer::Back],
        routers::rect(5.4, 5.4, 5.6, 5.6),
    );
    raw.board.copper_keepouts.push(blocker.clone());
    let ledger = EscapePromiseLedger::capture_in_context(&source, &raw, &[blocker])
        .expect("clean admission is legal, raw original promise remains unverified");
    assert!(!ledger.promises()[0].original_verified, "original proof must use raw producer geometry");
    let current = frame(&source, tracks, vias, FixedPrefixRole::OrdinaryEscape);
    let check = ledger.validate(&current, current.board()).expect("fresh current site");
    assert_eq!(check.promises[0].state, PromiseState::OriginalAnchorUnverified);
    assert!(check.promises[0].verified_anchor.is_none());
    assert_eq!(ledger.original_fanout().board.copper_keepouts.len(), 1);
}

#[test]
fn strict_context_derivation_rejects_missing_owned_masks_unknown_extras_and_source_contract_drift() {
    let mut source = source();
    let authored = routers::copper_keepout("authored", &[CopperLayer::Front], routers::rect(18.0, 18.0, 20.0, 20.0));
    let second =
        routers::copper_keepout("second-authored", &[CopperLayer::Front], routers::rect(14.0, 18.0, 16.0, 20.0));
    let temporary = routers::copper_keepout("temporary", &[CopperLayer::Front], routers::rect(10.0, 18.0, 11.0, 19.0));
    source.copper_keepouts = vec![authored.clone(), second];
    source.metadata.insert("authored-context".to_owned(), "unchanged".to_owned());
    let (tracks, vias) = launch((5.5, 5.5));
    let original = fanout(&source, tracks, vias, Some(routers::pt(5.5, 5.5)), None, Vec::new());
    assert!(
        EscapePromiseLedger::capture_in_context(&source, &original, &[authored]).is_err(),
        "an authored occurrence alone cannot satisfy a claimed temporary occurrence"
    );
    for damage in [
        "missing_owned",
        "missing_authored",
        "undeclared",
        "extra_equal",
        "reordered_authored",
        "same_id_changed_geometry",
        "pose",
        "rule",
        "authored_metadata",
    ] {
        let mut raw = original.clone();
        raw.board.copper_keepouts.push(temporary.clone());
        match damage {
            "missing_owned" => {
                raw.board.copper_keepouts.pop();
            }
            "missing_authored" => {
                raw.board.copper_keepouts.remove(0);
            }
            "undeclared" => raw.board.copper_keepouts.push(routers::copper_keepout(
                "unknown",
                &[CopperLayer::Front],
                routers::rect(5.0, 18.0, 6.0, 19.0),
            )),
            "extra_equal" => raw.board.copper_keepouts.push(temporary.clone()),
            "reordered_authored" => raw.board.copper_keepouts.swap(0, 1),
            "same_id_changed_geometry" => {
                raw.board.copper_keepouts.last_mut().expect("temporary").vertices[0].x_nm += 1
            }
            "pose" => raw.board.placements[0].position.x_nm += 1,
            "rule" => raw.board.rules.default_track_width_nm += 1,
            _ => {
                raw.board.metadata.insert("authored-context".to_owned(), "changed".to_owned());
            }
        }
        assert!(
            EscapePromiseLedger::capture_in_context(&source, &raw, std::slice::from_ref(&temporary)).is_err(),
            "{damage}"
        );
    }
}

#[test]
fn original_lattice_site_requires_exact_occurrence_and_retained_pad_path_without_allowing_lattice_resites() {
    let source = source();
    let (tracks, vias) = launch((5.5, 5.5));
    let original = fanout(&source, tracks.clone(), vias.clone(), Some(routers::pt(5.5, 5.5)), None, Vec::new());
    let ledger = EscapePromiseLedger::capture(&source, &original).expect("original producer");
    let retained = frame(&source, tracks.clone(), vias.clone(), FixedPrefixRole::ViaLattice);
    let check = ledger.validate(&retained, retained.board()).expect("exact original owned lattice site");
    assert_eq!(check.promises[0].state, PromiseState::VerifiedOriginal);
    assert_eq!(check.promises[0].verified_anchor, Some(Anchor::Point(routers::pt(5.5, 5.5))));
    assert_eq!(check.promises[0].original.provenance, AnchorProvenance::FanoutViaAccess);
    assert_eq!(check.promises[0].original.original_anchor, ledger.promises()[0].original_anchor);
    let (new_tracks, new_vias) = launch((5.5, 4.5));
    let resited = frame(&source, new_tracks, new_vias, FixedPrefixRole::ViaLattice);
    let check = ledger.validate(&resited, resited.board()).expect("lattice replacement remains unsupported");
    assert_eq!(check.promises[0].state, PromiseState::NoOwnedOrdinaryVia);
    assert!(check.promises[0].verified_anchor.is_none());
    let mut changed_barrel = vias.clone();
    changed_barrel[0].size_nm += 50_000;
    let coordinate_only = frame(&source, tracks.clone(), changed_barrel, FixedPrefixRole::ViaLattice);
    assert!(
        ledger
            .validate(&coordinate_only, coordinate_only.board())
            .expect("same coordinates do not grant original identity")
            .promises[0]
            .verified_anchor
            .is_none()
    );
    let floating = frame(&source, Vec::new(), vias, FixedPrefixRole::ViaLattice);
    let mut mutable_path = floating.board().clone();
    mutable_path.tracks.extend(tracks);
    let check =
        ledger.validate(&floating, &mutable_path).expect("mutable same-net path cannot establish retained launch");
    assert_eq!(check.promises[0].state, PromiseState::OriginalPadAccessUnresolved);
    assert!(check.promises[0].verified_anchor.is_none());
}

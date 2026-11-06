//! Complete-baseline admission, distinct from complete package escape proof.
use super::*;
use crate::critical::{CriticalOptions, route_critical_nets};
use crate::test_support_routers as routers;
use copper_pcb::board::{PadReference, RouteKind, Stackup};
use copper_physical::{CopperLayer, DesignRules};

fn contribution(id: &str, role: FixedPrefixRole, tracks: Vec<TrackSegment>, vias: Vec<Via>) -> FixedContribution {
    FixedContribution { id: id.to_owned(), role, tracks, vias }
}

#[test]
fn empty_explicit_frame_matches_the_legacy_critical_input_path() {
    let mut source = routers::pad_board(&[("C", &[(2.0, 2.0), (15.0, 2.0)])], 2, Vec::new());
    source.net_routing_rules.push(copper_pcb::board::NetRoutingRule {
        kind: RouteKind::Clock,
        width_nm: Some(200_000),
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..routers::rule("C")
    });
    let global =
        crate::route_global(&source, &crate::GlobalRouterOptions::default(), None, None, None).expect("global guides");
    let legacy =
        route_critical_nets(source.clone(), &global, &CriticalOptions::default(), None).expect("legacy critical");
    let prefix = AdmittedFixedPrefix::admit(&source, &[]).expect("empty explicit frame");
    let explicit = route_critical_nets(
        source.clone(),
        &global,
        &CriticalOptions { fixed_prefix: Some(&prefix), ..Default::default() },
        None,
    )
    .expect("explicit empty critical");
    assert_eq!(legacy.to_json().expect("legacy report"), explicit.to_json().expect("explicit report"));
    assert_eq!(legacy.board, explicit.board);
    assert_eq!(legacy.locked_tracks, explicit.locked_tracks);
    assert_eq!(legacy.locked_vias, explicit.locked_vias);
    let mut changed = source;
    changed.metadata.insert("unproved-context".to_owned(), "different".to_owned());
    assert!(
        route_critical_nets(
            changed,
            &global,
            &CriticalOptions { fixed_prefix: Some(&prefix), ..Default::default() },
            None
        )
        .is_err()
    );
}

#[test]
fn first_critical_plane_stitch_respects_real_local_signal_site_without_macro_relabeling() {
    let fp = routers::footprint(
        "test/critical-adjacent-pads",
        vec![routers::pad("S", -0.75, 0.0, 0.25, 0.25), routers::pad("G", 0.0, 0.0, 0.25, 0.25)],
        2.0,
        1.0,
    );
    let mut source = routers::board(
        "critical-local-sites",
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
    let ground = PadReference::new("U1", "G");
    let options = crate::plane::PlaneStitchOptions {
        candidate_bias: Some("north".to_owned()),
        only_pads: Some([ground.clone()].into_iter().collect()),
        ..Default::default()
    };
    let unprotected = crate::plane::stitch_zone_pads(&source, Some(&options)).expect("real source-only plane tap");
    assert!(unprotected.stitched_pads.contains(&ground));
    let original_ground_via = unprotected.board.vias.iter().find(|via| via.net == "GND").expect("actual GND tap");
    let mut site = original_ground_via.clone();
    site.net = "S".to_owned();
    let clearance = crate::routing_clearance::RoutingClearanceIndex::new(&source).expect("source clearance");
    let tracks = crate::pin_escape::checked_access_path(
        &source,
        &clearance,
        "S",
        routers::pt(10.0, 10.0),
        site.position,
        150_000,
        CopperLayer::Front,
        false,
        true,
        None,
    )
    .expect("local path check")
    .expect("legal signal launch");
    let prefix = AdmittedFixedPrefix::admit(
        &source,
        &[contribution("signal-site", FixedPrefixRole::OrdinaryEscape, tracks, vec![site.clone()])],
    )
    .expect("fresh local site frame");
    let global =
        crate::route_global(&source, &crate::GlobalRouterOptions::default(), None, None, None).expect("guides");
    let result = route_critical_nets(
        source.clone(),
        &global,
        &CriticalOptions { fixed_prefix: Some(&prefix), plane_stitch_options: Some(&options), ..Default::default() },
        None,
    )
    .expect("actual first-critical stage");
    assert!(result.board.tracks.starts_with(&prefix.board().tracks));
    assert!(result.board.vias.starts_with(&prefix.board().vias));
    assert!(result.locked_tracks.starts_with(&prefix.board().tracks));
    assert!(result.locked_vias.starts_with(&prefix.board().vias));
    assert!(!result.board.vias.iter().any(|via| via.net == "GND" && via.position == site.position));
    assert_eq!(result.board.hard_macros, source.hard_macros);
    assert_eq!(result.board.materialized_macros, source.materialized_macros);
    crate::global_reserved_access::admit_fixed_prefix(prefix.board(), &result.board)
        .expect("fresh merged physical DRC");
    let fresh = crate::routing_clearance::RoutingClearanceIndex::new(&result.board).expect("result clearance");
    assert!(
        crate::pin_escape::verified_fanout_path(
            &result.board,
            &PadReference::new("U1", "S"),
            "S",
            site.position,
            &fresh
        )
        .expect("local original-pad proof")
        .is_some()
    );
}

#[test]
fn frame_aware_bypass_requires_the_real_fixed_edge_and_preserves_bypass_only_generated_ownership() {
    use crate::bypass_reservations::{
        BypassReservation, bypass_copper_reservations, bypass_copper_reservations_in_frame,
        verified_bypass_reservations_in_frame,
    };
    let (source, _) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
    let routed = crate::decoupling::route_decouplers(&source, None, None, None, None).expect("real cross-layer bypass");
    assert!(!routed.admitted_path_witnesses.is_empty());
    let fixed_via = routed.created_vias.first().expect("essential physical bypass barrel").clone();
    let rest_vias = &routed.created_vias[1..];
    let prefix = AdmittedFixedPrefix::admit(
        &source,
        &[contribution("allocated-site", FixedPrefixRole::ViaLattice, Vec::new(), vec![fixed_via.clone()])],
    )
    .expect("fresh local fixed site");
    let legacy_missing =
        bypass_copper_reservations(&source, &routed.created_tracks, rest_vias, &routed.admitted_path_witnesses)
            .expect("old source-only admission");
    assert!(legacy_missing.0.is_none());
    assert!(legacy_missing.1.expect("missing prefix edge must stay explicit").contains("missing or ambiguous"));
    let (bound, reason) = bypass_copper_reservations_in_frame(
        &source,
        &prefix,
        &routed.created_tracks,
        rest_vias,
        &routed.admitted_path_witnesses,
    )
    .expect("complete prefix admission");
    assert!(reason.is_none(), "{reason:?}");
    let bound = bound.expect("fresh witnessed bypass binding");
    assert_eq!(bound.created_vias, rest_vias);
    assert!(!bound.created_vias.contains(&fixed_via));
    assert_eq!(&bound.board.vias[..prefix.board().vias.len()], prefix.board().vias.as_slice());
    assert!(
        verified_bypass_reservations_in_frame(&source, &prefix, &bound)
            .expect("fresh strict binding proof")
            .2
            .is_none()
    );
    let global =
        crate::route_global(&source, &crate::GlobalRouterOptions::default(), None, None, None).expect("guides");
    let result = route_critical_nets(
        source.clone(),
        &global,
        &CriticalOptions { fixed_prefix: Some(&prefix), reserved_accesses: Some(&bound), ..Default::default() },
        None,
    )
    .expect("critical complete-frame admission");
    assert!(result.board.vias.starts_with(&bound.board.vias));
    let fake = BypassReservation::new(
        prefix.board(),
        &routed.created_tracks,
        &routed.created_vias,
        &routed.admitted_path_witnesses,
    );
    assert!(
        verified_bypass_reservations_in_frame(&source, &prefix, &fake).expect("forged ownership report").2.is_some()
    );
    assert!(
        route_critical_nets(
            source.clone(),
            &global,
            &CriticalOptions { fixed_prefix: Some(&prefix), reserved_accesses: Some(&fake), ..Default::default() },
            None
        )
        .is_err()
    );
    // An empty explicit prefix must preserve the old factory's exact result.
    let empty = AdmittedFixedPrefix::admit(&source, &[]).expect("empty frame");
    let (old, old_reason) = bypass_copper_reservations(
        &source,
        &routed.created_tracks,
        &routed.created_vias,
        &routed.admitted_path_witnesses,
    )
    .expect("legacy binding");
    let (new, new_reason) = bypass_copper_reservations_in_frame(
        &source,
        &empty,
        &routed.created_tracks,
        &routed.created_vias,
        &routed.admitted_path_witnesses,
    )
    .expect("empty explicit binding");
    assert_eq!(old_reason, new_reason);
    let (old, new) = (old.expect("legacy accepted"), new.expect("explicit accepted"));
    assert_eq!(old.board, new.board);
    assert_eq!(old.created_tracks, new.created_tracks);
    assert_eq!(old.created_vias, new.created_vias);
    assert_eq!(old.decoupling_witnesses, new.decoupling_witnesses);
}

#[test]
fn typed_critical_replay_freshly_readmits_local_prefix_and_rejects_drift_collision_or_admission_flags() {
    use copper_state::{StateDocument, StateValue};
    fn entry_mut<'a>(value: &'a mut StateValue, key: &str) -> &'a mut StateValue {
        let StateValue::Map { entries, .. } = value else { panic!("saved input map") };
        &mut entries.iter_mut().find(|(name, _)| *name == StateValue::Str(key.to_owned())).expect("saved input field").1
    }
    let source = routers::pad_board(&[("S", &[(2.0, 2.0), (18.0, 2.0)]), ("T", &[])], 2, Vec::new());
    let via = routers::via("S", (5.5, 5.5), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
    let prefix = AdmittedFixedPrefix::admit(
        &source,
        &[contribution(
            "local-site",
            FixedPrefixRole::OrdinaryEscape,
            vec![routers::track("S", (5.0, 5.0), (5.5, 5.5), 0.2, CopperLayer::Front)],
            vec![via.clone()],
        )],
    )
    .expect("local frame");
    let global =
        crate::route_global(&source, &crate::GlobalRouterOptions::default(), None, None, None).expect("guides");
    let encoded =
        crate::state_codec::critical_inputs_state_text_with_prefix(&source, &global, None, None, Some(&prefix));
    let saved = StateDocument::read(&encoded).expect("typed input").value;
    let result = crate::critical::replay_critical(&saved, None, 1000).expect("fresh replay");
    assert!(result.board.vias.contains(&via));
    assert!(result.board.tracks.starts_with(&prefix.board().tracks));
    let mut drift = saved.clone();
    let mut changed = source.clone();
    changed.metadata.insert("changed-source-intent".to_owned(), "true".to_owned());
    *entry_mut(&mut drift, "board") = crate::state_codec::encode_board(&changed);
    assert!(crate::critical::replay_critical(&drift, None, 1000).is_err());
    let mut forged = saved.clone();
    let StateValue::Dataclass { fields, .. } = entry_mut(&mut forged, "fixed_prefix") else { panic!("prefix input") };
    fields.insert("admitted".to_owned(), StateValue::Bool(true));
    assert!(crate::critical::replay_critical(&forged, None, 1000).is_err(), "a saved flag cannot grant admission");
    let mut collision = saved.clone();
    let StateValue::Dataclass { fields, .. } = entry_mut(&mut collision, "fixed_prefix") else {
        panic!("prefix input")
    };
    let StateValue::Tuple(parts) = fields.get_mut("contributions").expect("contributions") else {
        panic!("ordered declarations")
    };
    let StateValue::Dataclass { fields, .. } = &mut parts[0] else { panic!("contribution") };
    let StateValue::Tuple(vias) = fields.get_mut("vias").expect("vias") else { panic!("ordered via occurrences") };
    let mut foreign = vias[0].clone();
    let StateValue::Dataclass { fields, .. } = &mut foreign else { panic!("via") };
    fields.insert("net".to_owned(), StateValue::Str("T".to_owned()));
    vias.push(foreign);
    assert!(
        crate::critical::replay_critical(&collision, None, 1000).is_err(),
        "fresh physical collision is never trusted"
    );
    let mut wrong_kind = saved;
    *entry_mut(&mut wrong_kind, "fixed_prefix") = StateValue::Str("already-admitted".to_owned());
    assert!(
        crate::critical::replay_critical(&wrong_kind, None, 1000).is_err(),
        "wrong typed input cannot fall back to None"
    );
    let legacy = crate::state_codec::critical_inputs_state_text(&source, &global, None, None);
    let mut null = StateDocument::read(&legacy).expect("legacy state").value;
    let absent = crate::critical::replay_critical(&null, None, 1000).expect("absent prefix");
    let StateValue::Map { entries, .. } = &mut null else { panic!("legacy map") };
    entries.push((StateValue::Str("fixed_prefix".to_owned()), StateValue::Null));
    let explicit_null = crate::critical::replay_critical(&null, None, 1000).expect("null prefix");
    assert_eq!(absent.to_json().expect("absent report"), explicit_null.to_json().expect("null report"));
    assert_eq!(absent.board, explicit_null.board);
}

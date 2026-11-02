//! Fanout legality and native plane verification, ported from
//! `tests/test_fanout_and_plane_verify.py`.

mod support_planes;

use copper_pcb::board::{PadReference, PhysicalBoard, Via};
use copper_physical::footprint::PadKind;
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::fanout::{FanoutOptions, route_fanout};
use copper_route::pin_escape::verified_fanout_path;
use copper_route::route_quality::proper_same_net_crossing;
use support_planes::*;

/// `_dense_board`: a twelve-pad 0.5 mm pitch row and a one-pad terminal.
fn dense_board() -> PhysicalBoard {
    let mut board = board("Fanout", 20.0, 20.0);
    let pads = (0..12).map(|index| pad(&(index + 1).to_string(), mm(-2.75 + 0.5 * index as f64, 0.0), size(0.25, 0.5)));
    add_footprint(&mut board, footprint("dense", pads.collect(), size(6.0, 2.0)));
    add_footprint(&mut board, footprint("terminal", vec![pad("1", Point::new(0, 0), size(0.6, 0.6))], size(1.0, 1.0)));
    board.placements = vec![place("U1", "dense", mm(8.0, 10.0)), place("J1", "terminal", mm(16.0, 10.0))];
    board.nets = vec![net("SIGNAL", &[("U1", "1"), ("J1", "1")])];
    board
}

fn fanout(board: &PhysicalBoard, options: Option<&FanoutOptions>) -> copper_route::fanout::FanoutResult {
    route_fanout(board, options, None).expect("fanout")
}

#[test]
fn dense_fanout_is_legal_deterministic_and_exposes_anchor() {
    let board = dense_board();
    let first = fanout(&board, None);
    let second = fanout(&board, None);
    assert_eq!(first.board, second.board);
    assert_eq!(first.accesses, second.accesses);
    assert_eq!(first.pending_pads, second.pending_pads);
    assert_eq!((first.added_track_count, first.added_via_count), (1, 1));
    let via = &first.board.vias[0];
    assert_eq!(first.accesses[&pad_ref("U1", "1")], via.position);
    assert_eq!(first.board.tracks[0].end, via.position);
    assert!(first.pending_pads.is_empty());
    assert_ne!(via.position, first.board.tracks[0].start);
}

#[test]
fn same_net_fanout_drills_respect_hole_spacing() {
    let mut board = dense_board();
    board.nets = vec![net("SIGNAL", &[("U1", "1"), ("U1", "2"), ("J1", "1")])];
    // Bound the launch radius so this fixture retains two distinct drills.
    // Unrestricted straight outward launches can legitimately share one via.
    let result = fanout(&board, Some(&FanoutOptions { maximum_radius_nm: nm(1.0), ..FanoutOptions::default() }));
    assert_eq!(result.board.vias.len(), 2);
    assert!(result.pending_pads.is_empty());
    let vias = &result.board.vias;
    for (i, first) in vias.iter().enumerate() {
        for second in vias.iter().skip(i + 1) {
            let dx = i128::from(first.position.x_nm - second.position.x_nm);
            let dy = i128::from(first.position.y_nm - second.position.y_nm);
            let required = i128::from(first.drill_nm / 2 + second.drill_nm / 2 + board.rules.minimum_hole_clearance_nm);
            assert!(dx * dx + dy * dy >= required * required);
        }
    }
}

#[test]
fn adjacent_same_net_pads_share_a_verified_straight_launch() {
    let mut board = dense_board();
    board.nets = vec![net("SIGNAL", &[("U1", "1"), ("U1", "2"), ("J1", "1")])];
    let result = fanout(&board, None);
    assert!(result.pending_pads.is_empty());
    assert_eq!(result.board.vias.len(), 1);
    let launched: std::collections::BTreeSet<&PadReference> = result.accesses.keys().collect();
    assert_eq!(launched, [pad_ref("U1", "1"), pad_ref("U1", "2")].iter().collect());
    let index = RoutingClearanceIndex::new(&result.board).unwrap();
    for (pad, anchor) in &result.accesses {
        assert!(verified_fanout_path(&result.board, pad, "SIGNAL", *anchor, &index).unwrap().is_some(), "{pad}");
    }
    let created = &result.created_tracks;
    assert!(!created.iter().any(|a| created.iter().any(|b| proper_same_net_crossing(a, b))));
}

#[test]
fn same_net_drill_collision_is_rejected_by_router_and_native_drc() {
    let mut board = dense_board();
    let existing = via("SIGNAL", mm(4.0, 4.0), nm(0.8), nm(0.4));
    board.vias = vec![existing.clone()];
    let too_close = mm(4.5, 4.0);
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let (front, back) = (CopperLayer::Front, CopperLayer::Back);
    assert!(!index.can_via("SIGNAL", too_close, existing.size_nm, front, back, None, false, None).unwrap());
    assert!(index.can_via("SIGNAL", existing.position, existing.size_nm, front, back, None, false, None).unwrap());
    let mut bad = board.clone();
    bad.vias.push(Via { position: too_close, ..existing });
    assert!(drc_codes(&bad).contains("DRC-DRILL-SPACING"));
}

#[test]
fn via_near_plated_pad_drill_is_checked_without_invalid_pad_copy() {
    let mut plated = pad("1", Point::new(0, 0), size(1.0, 1.0));
    plated.kind = PadKind::ThroughHole;
    plated.drill = Some(size(0.5, 0.5));
    let mut board = board("Plated", 12.0, 12.0);
    add_footprint(&mut board, footprint("plated", vec![plated], size(2.0, 2.0)));
    board.placements = vec![place("J1", "plated", mm(6.0, 6.0))];
    board.nets = vec![net("SIGNAL", &[("J1", "1")])];
    board.vias = vec![via("SIGNAL", mm(6.6, 6.0), nm(0.8), nm(0.4))];
    assert!(drc_codes(&board).contains("DRC-DRILL-SPACING"));
}

#[test]
fn fanout_reports_unavailable_layer_without_mutating_board() {
    // A front land on a back-only net. A front-only net escapes on its surface (R13).
    let mut board = dense_board();
    board.net_routing_rules =
        vec![copper_pcb::board::NetRoutingRule { allowed_layers: vec![CopperLayer::Back], ..rule("SIGNAL") }];
    let result = fanout(&board, Some(&FanoutOptions::default()));
    assert_eq!(result.board, board);
    assert_eq!(result.pending_pads, vec![pad_ref("U1", "1")]);
}

#[cfg(unix)]
mod native {
    use super::*;
    use copper_route::plane_verify::{PlaneVerification, verify_filled_planes};
    use copper_route::route_closure::reconcile_zone_lands;

    fn verify(board: &PhysicalBoard, name: &str, report: &str, status: i32) -> Result<PlaneVerification, String> {
        let cli = fake_kicad_cli(&scratch(name), report, status);
        verify_filled_planes(board, &cli, 10, None).map_err(|error| error.to_string())
    }

    const CLEAN: &str = r#"{"violations": [], "unconnected_items": []}"#;

    #[test]
    fn plane_verification_detects_open_island_and_stale_geometry() {
        let board = dense_board();
        let evidence = verify(
            &board,
            "open-island",
            r#"{"violations": [{"type": "isolated_copper", "description": "island"}],
                "unconnected_items": [{"type": "unconnected_items", "description": "open"}]}"#,
            0,
        )
        .unwrap();
        assert!(!evidence.passed);
        assert_eq!((evidence.unconnected_count, evidence.island_count), (1, 1));
        assert!(evidence.matches(&board).unwrap());
        let mut changed = board.clone();
        changed.name = "Changed".to_owned();
        assert!(!evidence.matches(&changed).unwrap());
    }

    #[test]
    fn plane_verification_fails_closed_on_tool_and_schema_errors() {
        let board = dense_board();
        let error = verify(&board, "tool-error", CLEAN, 2).unwrap_err();
        assert!(error.contains("refill/DRC failed"), "{error}");
        let error = verify(&board, "schema-error", r#"{"violations": []}"#, 0).unwrap_err();
        assert!(error.contains("lacks violations"), "{error}");
    }

    #[test]
    fn library_findings_do_not_hide_verified_connectivity_or_pass_signoff() {
        let board = dense_board();
        let evidence = verify(
            &board,
            "library",
            r#"{"violations": [{"type": "lib_footprint_mismatch", "description": "library copy"}],
                "unconnected_items": []}"#,
            0,
        )
        .unwrap();
        assert!(!evidence.passed);
        assert!(evidence.zone_connectivity_verified(&board).unwrap());
        let mut stale = board.clone();
        stale.name = "Stale".to_owned();
        assert!(!evidence.zone_connectivity_verified(&stale).unwrap());
        let uncounted = PlaneVerification { other_violation_count: 0, ..evidence };
        assert!(!uncounted.zone_connectivity_verified(&board).unwrap());
    }

    #[test]
    fn other_violations_prevent_zone_land_reconciliation() {
        let board = dense_board();
        for kind in ["clearance", "shorting_items", "isolated_copper", "unknown"] {
            let report = format!(
                r#"{{"violations": [{{"type": "{kind}", "description": "unresolved"}}], "unconnected_items": []}}"#
            );
            let evidence = verify(&board, &format!("other-{kind}"), &report, 0).unwrap();
            assert!(!evidence.zone_connectivity_verified(&board).unwrap(), "{kind}");
        }
    }

    #[test]
    fn reconciliation_only_resolves_zone_lands_with_fresh_matching_evidence() {
        let mut board = dense_board();
        board.zones = vec![zone("signal-plane", "SIGNAL", &[CopperLayer::Front], rect(1.0, 1.0, 19.0, 19.0))];
        let pending = vec![pad_ref("U1", "1"), pad_ref("unknown", "1")];
        let evidence = verify(&board, "reconcile", CLEAN, 0).unwrap();
        let split = |board: &PhysicalBoard, evidence: Option<&PlaneVerification>| {
            reconcile_zone_lands(board, &pending, evidence).unwrap()
        };
        assert_eq!(split(&board, Some(&evidence)), (pending[1..].to_vec(), pending[..1].to_vec()));
        assert_eq!(split(&board, None), (pending.clone(), Vec::new()));
        let mut changed = board.clone();
        changed.name = "Changed".to_owned();
        assert_eq!(split(&changed, Some(&evidence)), (pending.clone(), Vec::new()));
        let open = PlaneVerification { unconnected_count: 1, ..evidence.clone() };
        assert_eq!(split(&board, Some(&open)), (pending.clone(), Vec::new()));
    }
}

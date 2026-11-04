//! Early plane contacts from real copper geometry, with no project-specific
//! repair selectors or search mocks; ported from
//! `tests/test_autonomous_plane_access.py`. The CLI default case is the
//! in-crate `route_board::option_tests::package_access_is_reserved_by_default_with_explicit_opt_out`
//! of the `copper` binary.

mod support_planes;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::Point;
use copper_route::critical::{CriticalOptions, CriticalRoutingResult, route_critical_nets};
use copper_route::plane::PlaneStitchOptions;
use copper_route::{GlobalRouterOptions, route_global};
use support_planes::*;

/// `constrained_contact_board()`: DEVICE's rail land (RAIL) and clock land
/// (CLOCK) beside a POWER terminal; the only In1.Cu RAIL copper is a narrow
/// 0.1 mm contact window at (4, 6) mm, a real source-defined window rather
/// than a mocked via generator.
fn constrained_contact_board() -> PhysicalBoard {
    let mut board = board("scarce-contact", 16.0, 16.0);
    add_footprint(
        &mut board,
        footprint(
            "paired-lands",
            vec![pad("1", mm(0.0, -1.0), size(0.3, 0.3)), pad("2", mm(0.0, -0.4), size(0.3, 0.3))],
            size(2.0, 3.0),
        ),
    );
    add_footprint(&mut board, footprint("terminal", vec![pad("1", Point::new(0, 0), size(0.3, 0.3))], size(1.0, 1.0)));
    board.placements = vec![
        place("DEVICE", "paired-lands", mm(5.0, 7.0)),
        place("POWER", "terminal", mm(3.0, 6.0)),
        place("REMOTE", "terminal", mm(13.0, 6.6)),
    ];
    board.nets =
        vec![net("RAIL", &[("DEVICE", "1"), ("POWER", "1")]), net("CLOCK", &[("DEVICE", "2"), ("REMOTE", "1")])];
    board.stackup = four_layers();
    board.zones =
        vec![zone("contact-window", "RAIL", &[copper_physical::CopperLayer::In1], rect(3.95, 5.95, 4.05, 6.05))];
    board
}

/// The constrained board plus a DP/DM pair between PAIR_A and PAIR_B.
fn pair_board() -> PhysicalBoard {
    let mut board = constrained_contact_board();
    add_footprint(
        &mut board,
        footprint(
            "synthetic-pair",
            vec![pad("1", mm(0.0, -0.5), size(0.4, 0.4)), pad("2", mm(0.0, 0.5), size(0.4, 0.4))],
            size(1.0, 2.0),
        ),
    );
    board.placements.push(place("PAIR_A", "synthetic-pair", mm(7.0, 12.0)));
    board.placements.push(place("PAIR_B", "synthetic-pair", mm(13.0, 12.0)));
    for (name, number) in [("DP", "1"), ("DM", "2")] {
        board.nets.push(net(name, &[("PAIR_A", number), ("PAIR_B", number)]));
    }
    board.net_routing_rules = [("DP", "DM"), ("DM", "DP")]
        .iter()
        .map(|(name, partner)| NetRoutingRule {
            kind: RouteKind::Differential,
            width_nm: Some(nm(0.25)),
            differential_partner: Some((*partner).to_owned()),
            pair_gap_nm: Some(nm(0.2)),
            max_skew_nm: Some(nm(10.0)),
            maximum_uncoupled_length_nm: Some(nm(10.0)),
            ..rule(name)
        })
        .collect();
    board
}

/// The `nets` of the result document: each group's routing identity.
fn net_documents(result: &CriticalRoutingResult) -> copper_json::Value {
    field(&copper_json::parse(&result.to_json().unwrap()).unwrap(), "nets").clone()
}

#[test]
fn early_plane_contacts_preserve_specialized_coupled_pair_routing() {
    let board = pair_board();
    assert!(hard_findings(&board).is_empty(), "{:?}", hard_findings(&board));
    let guides = route_global(
        &board,
        &GlobalRouterOptions { tile_size_nm: nm(2.0), ..GlobalRouterOptions::default() },
        None,
        None,
        None,
    )
    .unwrap();
    let ordinary_order = route_critical_nets(board.clone(), &guides, &CriticalOptions::default(), None).unwrap();
    let stitch = PlaneStitchOptions::default();
    let early = CriticalOptions { plane_stitch_options: Some(&stitch), ..CriticalOptions::default() };
    let result = route_critical_nets(board.clone(), &guides, &early, None).unwrap();
    assert_eq!(net_documents(&result), net_documents(&ordinary_order));
    let pair = &result.nets[0];
    assert!(pair.connected && pair.strategy == "aligned_pair", "{} {:?}", pair.strategy, pair.diagnostics);
    assert!(pair.coupled_length_nm > 0);
    assert_eq!(result.plane_reservation_via_count, 1);
    assert_eq!(result.reserved_via_count, 0);
    assert!(ordinary_order.locked_tracks.iter().all(|t| result.locked_tracks.contains(t)));
    assert!(result.board.vias.iter().filter(|v| v.net == "RAIL").all(|v| result.locked_vias.contains(v)));
    assert!(hard_findings(&result.board).is_empty(), "{:?}", hard_findings(&result.board));
    // The Rust board has no zone fills: no copper polygon is invented.
    assert!(result.board.polygons.is_empty());
    let again = route_critical_nets(board, &guides, &early, None).unwrap();
    assert_eq!(again.to_json().unwrap(), result.to_json().unwrap());
    assert_eq!(again.board, result.board);
    assert_eq!((&again.locked_tracks, &again.locked_vias), (&result.locked_tracks, &result.locked_vias));
}

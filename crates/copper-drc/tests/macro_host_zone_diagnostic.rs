//! Host-zone ownership is checked before fill, with the actual owner identified.

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use boards::*;
use copper_pcb::board::*;
use copper_pcb::hard_macros::{
    host_zone_macro_overlap, host_zone_overlaps_macro, resolved_macro_geometry, resolved_macro_zones,
    validate_hard_macros,
};
use copper_physical::CopperLayer::{Front, In1};
use copper_physical::footprint::footprint_geometry_digest;
use copper_units::Decimal;

fn fixture() -> PhysicalBoard {
    let fp = one_pad();
    let digest = footprint_geometry_digest(&fp);
    let mut board = board(
        "host-ownership",
        30.0,
        30.0,
        vec![fp.clone()],
        vec![placement("U_OWNER", &fp.name, 10.0, 12.0)],
        vec![net("GND", &[("U_OWNER", "1")])],
    );
    board.stackup.copper_layers.insert(1, In1);
    board.rigid_clusters.push(RigidPlacementCluster {
        name: "power-owner".to_owned(),
        anchor_reference: "U_OWNER".to_owned(),
        anchor_pad: None,
        members: vec![
            RigidPlacementMember::new("U_OWNER", &fp.name, &digest, pt(0.0, 0.0), &Decimal::from_i64(0)).unwrap(),
        ],
        source: "synthetic ownership regression".to_owned(),
        allowed_rotations: vec![Decimal::from_i64(0)],
        keepouts: Vec::new(),
        internal_clearance_nm: None,
        edge_openings: Vec::new(),
    });
    let mut private = copper_keepout("private-ground", &[Front], rect(-2.0, -2.0, 2.0, 2.0));
    private.block_zones = false;
    board.hard_macros.push(PhysicalHardMacro {
        cluster: "power-owner".to_owned(),
        asset_sha256: "a".repeat(64),
        tracks: vec![track("GND", (0.0, 0.0), (1.0, 0.0), 0.2, Front)],
        vias: Vec::new(),
        ports: Vec::new(),
        protected_regions: vec![private],
        required_layers: vec![Front],
        pad_bindings: vec![(PadReference::new("U_OWNER", "1"), "GND".to_owned())],
        isolated_pads: Vec::new(),
        zones: vec![zone("owned-ground", "GND", &[Front], rect(-1.5, -1.5, 1.5, 1.5))],
        plane_returns: Vec::new(),
        polygons: Vec::new(),
        width_contracts: Vec::new(),
    });
    board.zones.push(zone("host-ground", "GND", &[Front, In1], rect(0.5, 0.5, 29.5, 29.5)));
    board
}

#[test]
fn preflight_and_materialized_validation_identify_the_same_host_owner_conflict() {
    let board = fixture();
    let original = board.clone();
    let conflict = host_zone_macro_overlap(&board, &board.hard_macros[0], &[&board.zones[0]]).unwrap().unwrap();
    assert_eq!(conflict.zone, "host-ground");
    assert_eq!(conflict.net, "GND");
    assert_eq!(conflict.layer, Front);
    assert_eq!(conflict.cluster, "power-owner");
    assert_eq!(conflict.anchor, "U_OWNER");
    assert_eq!(conflict.region, "private-ground");
    assert_eq!(conflict.vertices, rect(8.0, 10.0, 12.0, 14.0));
    let message = conflict.message();
    assert!(message.contains("bounds (8000000, 10000000)..(12000000, 14000000) nm"));
    let error = copper_drc::macros::materialize_hard_macros(board.clone()).unwrap_err().to_string();
    assert!(error.contains(&message), "{error}");
    assert_eq!(board, original, "diagnosis never modifies the source");
    let mut materialized = board.clone();
    let poses = copper_pcb::keepouts::placement_map(&board);
    materialized.tracks = resolved_macro_geometry(&board, &board.hard_macros[0], &poses).unwrap().tracks;
    materialized.zones.extend(resolved_macro_zones(&board, &board.hard_macros[0], &poses).unwrap());
    materialized.materialized_macros.push("power-owner".to_owned());
    let error = validate_hard_macros(&materialized).unwrap_err().to_string();
    assert!(error.contains(&message), "{error}");
}

#[test]
fn layer_and_explicit_sharing_rules_are_unchanged_and_keepouts_do_not_clip_host_ownership() {
    let mut board = fixture();
    let overlap =
        |board: &PhysicalBoard| host_zone_macro_overlap(board, &board.hard_macros[0], &[&board.zones[0]]).unwrap();
    assert!(host_zone_overlaps_macro(&board, &board.hard_macros[0], &[&board.zones[0]]).unwrap());
    board.zones[0].layers = vec![In1];
    assert!(overlap(&board).is_none(), "inner-only host does not own the surface macro region");
    board.zones[0].layers = vec![Front];
    board.zones[0].allow_same_net_hard_macro_overlap = true;
    assert!(overlap(&board).is_none(), "explicit matching-net zone sharing remains supported");
    board.zones[0].net = "OTHER".to_owned();
    assert_eq!(overlap(&board).unwrap().reason, "macro has no shareable zone on this net and layer");
    board.zones[0].net = "GND".to_owned();
    board.hard_macros[0].protected_regions[0].block_zones = true;
    assert_eq!(overlap(&board).unwrap().reason, "private region blocks zones");
    board.hard_macros[0].protected_regions[0].block_zones = false;
    let mut exclusion = copper_keepout("quiet-pour", &[Front], rect(9.0, 11.0, 11.0, 13.0));
    exclusion.block_tracks = false;
    exclusion.block_vias = false;
    board.copper_keepouts.push(exclusion);
    assert!(overlap(&board).unwrap().reason.contains("zone-blocking keepout 'quiet-pour'"));
    board.zones[0].allow_same_net_hard_macro_overlap = false;
    assert!(overlap(&board).is_some(), "fill exclusions do not change declared host ownership");
}

#[test]
fn a_declared_surface_notch_preserves_private_ownership_and_materializes_normally() {
    let mut board = fixture();
    let mut inner = board.zones[0].clone();
    inner.id = "inner-ground".to_owned();
    inner.layers = vec![In1];
    board.zones[0].layers = vec![Front];
    board.zones[0].outer =
        [(0.5, 0.5), (7.5, 0.5), (7.5, 14.5), (12.5, 14.5), (12.5, 0.5), (29.5, 0.5), (29.5, 29.5), (0.5, 29.5)]
            .into_iter()
            .map(|(x, y)| pt(x, y))
            .collect();
    board.zones.push(inner);
    assert!(!host_zone_overlaps_macro(&board, &board.hard_macros[0], &board.zones.iter().collect::<Vec<_>>()).unwrap());
    let result = copper_drc::macros::materialize_hard_macros(board)
        .expect("a real outline notch excludes the private surface region");
    assert_eq!(result.materialized_macros, vec!["power-owner"]);
    validate_hard_macros(&result).unwrap();
}

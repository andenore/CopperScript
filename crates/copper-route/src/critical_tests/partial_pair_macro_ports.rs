//! `tests/test_partial_pair_macro_ports.py`: prescribed package exits remain
//! the endpoints of public contact preparation.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::footprint::{FootprintPad, Size, footprint_geometry_digest};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::critical::{CriticalOptions, route_critical_nets};
use crate::pair_terminals::{PairTerminalPreparation, macro_terminal_guides_proved, prepare_pair_terminals};
use crate::test_support_critical::*;

/// `test_local_pair_branches.fixture()`: chip U (5, 12) and a two-column
/// contact bank J (30, 12) whose crossed lands A/C (N) and B/D (P) need a
/// matched crossover.
fn crossed_contacts() -> PhysicalBoard {
    let land = |number: &str, x: i64, y: i64| FootprintPad {
        size: Size { width_nm: 700_000, height_nm: 300_000 },
        ..smd_pad(number, at(x - 5 * MM, y), 700_000)
    };
    let mut lands =
        vec![land("A", 0, -250_000), land("B", 0, 250_000), land("C", 1_700_000, 500_000), land("D", 1_700_000, 0)];
    for i in -6..=6i64 {
        for (column, y) in [(0, i * 500_000 + 250_000), (1_700_000, i * 500_000)] {
            if (column == 0 && [-250_000, 250_000].contains(&y)) || (column != 0 && [0, 500_000].contains(&y)) {
                continue;
            }
            let number = format!("unused{}", lands.len());
            lands.push(land(&number, column, y));
        }
    }
    let chip = footprint(
        "chip",
        vec![smd_pad("1", at(0, -500_000), 400_000), smd_pad("2", at(0, 500_000), 400_000)],
        Size { width_nm: MM, height_nm: 2 * MM },
    );
    let contacts = footprint("contacts", lands, Size { width_nm: 10 * MM, height_nm: 8 * MM });
    let nets = vec![net("N", &[("U", "1"), ("J", "A"), ("J", "C")]), net("P", &[("U", "2"), ("J", "B"), ("J", "D")])];
    let poses = vec![pose("U", "chip", mm(5, 12)), pose("J", "contacts", mm(30, 12))];
    let mut board = board("crossed contacts", 40, 25, vec![chip, contacts], poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    board.rules.minimum_hole_clearance_nm = 200_000;
    board.net_routing_rules = [("N", "P"), ("P", "N")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            allowed_layers: vec![FRONT, BACK],
            max_vias: Some(6),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(120_000),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            tuning_amplitude_limit_nm: Some(MM),
            max_skew_nm: Some(500_000),
            ..differential_rule(name, partner, 250_000, 150_000)
        })
        .collect();
    board
}

/// `bound_pair(tmp_path)`: U bound to a synthetic hard macro whose 0.12 mm
/// exits end at prescribed F.Cu ports 1 mm east of each land.
fn bound_pair() -> PhysicalBoard {
    let board = crossed_contacts();
    let digest = footprint_geometry_digest(&board.footprints["chip"]);
    let roles = [("negative", "1", -500_000), ("positive", "2", 500_000)];
    let tracks: Vec<String> = roles
        .iter()
        .map(|(role, pad, y)| {
            format!(r#"{{"net": "{role}", "width_nm": 120000, "layer": "F.Cu", "points": [{{"pad": ["chip", "{pad}"]}}, [1000000, {y}]]}}"#)
        })
        .collect();
    let ports: Vec<String> = roles
        .iter()
        .map(|(role, pad, y)| {
            format!(r#"{{"name": "{role}", "net": "{role}", "layer": "F.Cu", "point": [1000000, {y}], "pads": [["chip", "{pad}"]]}}"#)
        })
        .collect();
    let asset = format!(
        r#"{{"schema": "copperlib-physical-hard-macro/v0.1", "source": "Synthetic package exit contract",
            "production_publishable": false, "anchor": "chip",
            "members": [{{"reference": "chip", "footprint": "chip", "footprint_digest": "{digest}", "center_nm": [0, 0],
                "rotation_degrees": "0", "edge_clearance_nm": 250000}}],
            "pad_nets": [["chip", "1", "negative"], ["chip", "2", "positive"]], "isolated_pads": [], "vias": [],
            "tracks": [{}], "ports": [{}],
            "protected_regions": [{{"id": "package", "layers": ["F.Cu"],
                "vertices": [[-500000, -1000000], [500000, -1000000], [500000, 1000000], [-500000, 1000000]],
                "block_tracks": true, "block_vias": true, "block_zones": true}}],
            "keepouts": [], "required_layers": ["F.Cu"], "allowed_rotations": [0], "internal_clearance_nm": 0,
            "unresolved": ["Synthetic only"]}}"#,
        tracks.join(", "),
        ports.join(", ")
    );
    let raw = asset.into_bytes();
    let sha: String = Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
    let bindings = IndexMap::from([("chip".to_owned(), "U".to_owned())]);
    let nets = IndexMap::from([("negative".to_owned(), "N".to_owned()), ("positive".to_owned(), "P".to_owned())]);
    copper_pcb::hard_macros::bind_hard_macro(
        board,
        std::path::Path::new("pair.json"),
        &raw,
        &sha,
        "prescribed",
        &bindings,
        &nets,
    )
    .expect("bound macro")
}

fn materialized(board: &PhysicalBoard) -> PhysicalBoard {
    copper_drc::macros::materialize_hard_macros(board.clone()).expect("materialized")
}

#[test]
fn public_connector_crossover_keeps_the_other_ends_proved_macro_ports() {
    let board = materialized(&bound_pair());
    let rules = &board.net_routing_rules;
    let prepared =
        prepare_pair_terminals(&board, &rules[0], &rules[1], &IndexMap::new(), &mut PairTerminalPreparation::default())
            .expect("prepared");
    assert!(
        !prepared.first_branches.is_empty() && !prepared.second_branches.is_empty() && !prepared.via_pairs.is_empty()
    );
    let guides = [prepared.routes.get("N"), prepared.routes.get("P")];
    assert!(macro_terminal_guides_proved(&board, guides).expect("proof"));
    for route in prepared.routes.values() {
        let endpoint = route.accesses.iter().find(|a| a.pad.component == "U").expect("a U endpoint");
        assert_eq!(endpoint.pad_position.x_nm, 6 * MM);
        assert_eq!(endpoint.access_position, endpoint.pad_position);
    }
    assert!(
        prepared
            .first_branches
            .iter()
            .chain(&prepared.second_branches)
            .all(|t| t.start.x_nm > 20 * MM && t.end.x_nm > 20 * MM)
    );
    let mut stale = prepared.routes["N"].clone();
    let port = stale.accesses.iter_mut().find(|a| a.pad.component == "U").expect("a port");
    port.pad_position = at(5 * MM, 11_500_000);
    assert!(!macro_terminal_guides_proved(&board, [Some(&stale), prepared.routes.get("P")]).expect("proof"));
}

#[test]
fn failed_coarse_guides_use_boundary_ports_and_keep_all_owner_occurrences() {
    let source = bound_pair();
    let owner = materialized(&source);
    let mut guides = global_route(&source, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    for route in &mut guides.routes {
        route.connected = false;
        route.accesses.clear();
        route.segments.clear();
        route.vias.clear();
    }
    let result = route_critical_nets(source.clone(), &guides, &CriticalOptions::default(), None).expect("critical");
    assert!(result.nets.iter().all(|n| n.connected), "{:?}", result.nets);
    assert!(crate::pair_terminals::contains_all(&result.board.tracks, &owner.tracks));
    assert_eq!(result.board.hard_macros, owner.hard_macros);
    assert!(source.nets.iter().all(|n| net_connected(&result.board, &n.name)));
}

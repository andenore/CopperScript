//! Required via-in-pad arrays are fixed, fully checked owner copper; ported
//! from `tests/test_pad_via_arrays.py`.

mod support_planes;

use copper_drc::digest::physical_board_digest;
use copper_drc::legality::{Checker, Policy};
use copper_drc::macros::materialize_hard_macros;
use copper_pcb::board::{PadViaInPadRule, PhysicalBoard, Placement, Stackup};
use copper_pcb::hard_macros::macro_source;
use copper_pcb::pad_via_arrays::{via_in_pad_array_report, via_in_pad_array_vias};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_physical::footprint::PadShape;
use copper_physical::{BoardSide, CopperLayer, DesignRules, Point};
use copper_route::pad_via_arrays::{is_site_rejection, materialize_via_in_pad_arrays};
use copper_route::plane::stitch_zone_pads;
use copper_units::Decimal;
use indexmap::IndexMap;
use support_planes::*;

fn ep() -> copper_pcb::board::PadReference {
    pad_ref("U1", "17")
}

fn six() -> Stackup {
    use CopperLayer::*;
    Stackup::new(vec![Front, In1, In2, In3, In4, Back])
}

fn array_rule(rows: Option<i64>, columns: Option<i64>, pitch_nm: Option<i64>) -> PadViaInPadRule {
    PadViaInPadRule { pad: ep(), rows, columns, pitch_nm }
}

/// `_array_board`: a QFN-16 with an exposed GND land between two resistors.
fn array_board(ep_mm: f64, rule: Option<PadViaInPadRule>) -> PhysicalBoard {
    let reach = ep_mm / 2.0 + 0.55; // 0.15 mm from the exposed land
    let mut pads = Vec::new();
    for (side, (x, y)) in [(-1.0, 0.0), (0.0, 1.0), (1.0, 0.0), (0.0, -1.0)].into_iter().enumerate() {
        for index in 0..4 {
            let offset = (index as f64 - 1.5) * 0.65;
            let (at, dimensions) = if x != 0.0 {
                (mm(x * reach, offset), size(0.8, 0.3))
            } else {
                (mm(offset, y * reach), size(0.3, 0.8))
            };
            pads.push(pad(&(side * 4 + index + 1).to_string(), at, dimensions));
        }
    }
    let mut exposed = pad("17", Point::new(0, 0), size(ep_mm, ep_mm));
    exposed.shape = PadShape::Rectangle;
    pads.push(exposed);
    let mut board = board("ViaArray", 30.0, 20.0);
    add_footprint(&mut board, footprint("test/qfn16-ep", pads, size(2.0 * reach + 2.0, 2.0 * reach + 2.0)));
    add_footprint(
        &mut board,
        footprint(
            "test/0603",
            vec![pad("1", mm(-0.8, 0.0), size(0.8, 0.9)), pad("2", mm(0.8, 0.0), size(0.8, 0.9))],
            size(3.2, 1.8),
        ),
    );
    board.placements = vec![
        place("U1", "test/qfn16-ep", mm(15.0, 10.0)),
        place("R1", "test/0603", mm(7.0, 10.0)),
        place("R2", "test/0603", mm(23.0, 10.0)),
    ];
    board.nets = vec![
        net("GND", &[("U1", "17"), ("U1", "4"), ("R1", "1")]),
        net("A", &[("U1", "1"), ("R1", "2")]),
        net("B", &[("U1", "9"), ("R2", "1")]),
    ];
    board.stackup = six();
    board.zones = vec![zone("ground", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 29.0, 19.0))];
    board.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    board.rules = DesignRules {
        minimum_clearance_nm: nm(0.09),
        minimum_track_width_nm: nm(0.09),
        default_track_width_nm: nm(0.2),
        ..DesignRules::default()
    };
    board.via_in_pad_rules = rule.into_iter().collect();
    board
}

fn grid_board(rows: i64, columns: i64, pitch: Option<f64>) -> PhysicalBoard {
    array_board(3.1, Some(array_rule(Some(rows), Some(columns), pitch.map(nm))))
}

fn hard(board: &PhysicalBoard) -> Vec<String> {
    hard_findings(board).into_iter().collect()
}

/// `_lower`: a one-capacitor `tiny` design with a via-in-pad constraint.
fn lower(extra: &str) -> Result<PhysicalBoard, String> {
    let source = format!(
        "board Test {{ use library \"tiny\";\n    component BT1: CAPACITOR;\n    net GND {{ BT1.2; }}\n    \
         constraint copper_zone(GND) {{ layers = \"In1.Cu\"; }}\n    \
         constraint via_in_pad(BT1.2) {{ process = \"filled-capped\"; {extra} }}\n}}\n"
    );
    let digest: String = extra.bytes().map(|b| format!("{b:02x}")).collect();
    let directory = scratch(&format!("pad-via-arrays-{digest}"));
    let path = directory.join("board.copper");
    std::fs::write(&path, source).unwrap();
    let design = copper_lang::compiler::load_design(&path, false, false).map_err(|error| format!("{error:?}"))?;
    let options = PhysicalOptions::new(100.0, 80.0, 6, "jlcpcb-six-layer").unwrap();
    physicalize(&design, Footprints::Proxy, &options, false, false).map_err(|error| error.to_string())
}

fn checker_legal(board: &PhysicalBoard, candidate: &Placement, held: &[&Placement]) -> bool {
    let placed: IndexMap<String, Placement> = held.iter().map(|p| (p.reference.clone(), (*p).clone())).collect();
    Checker::new(board, Policy::default()).legal(candidate, &placed, None).unwrap()
}

#[test]
fn source_lowers_array_parameters_onto_the_permission() {
    let board = lower("rows = 2; columns = 2; pitch = 0.5mm;").unwrap();
    assert_eq!(
        board.via_in_pad_rules,
        vec![PadViaInPadRule { pad: pad_ref("BT1", "2"), rows: Some(2), columns: Some(2), pitch_nm: Some(nm(0.5)) }]
    );
    assert_eq!(lower("rows = 1; columns = 1;").unwrap().via_in_pad_rules[0].pitch_nm, None);
}

#[test]
fn placement_reserves_required_array_against_backside_lands() {
    for rotation in [0, 90, 180, 270] {
        let board = grid_board(3, 3, Some(0.8));
        let (owner, resistor, other) = (&board.placements[0], &board.placements[1], &board.placements[2]);
        let mut flipped = resistor.clone();
        flipped.position = owner.position;
        flipped.side = BoardSide::Back;
        flipped.rotation_degrees = Decimal::from_i64(rotation);
        assert!(!checker_legal(&board, &flipped, &[owner, other]), "rotation {rotation}");
        // A permission alone does not invent a required array.
        let mut permission_only = board.clone();
        permission_only.via_in_pad_rules = vec![array_rule(None, None, None)];
        assert!(checker_legal(&permission_only, &flipped, &[owner, other]));
        let clear = Placement { position: mm(15.0, 15.0), ..flipped.clone() };
        assert!(checker_legal(&board, &clear, &[owner, other]));
        // Moving the array owner must also see the held backside component.
        assert!(!checker_legal(&board, owner, &[&flipped, other]));
    }
}

/// Python's `ViaArrayPlacementError` also carried the posed board and its
/// components; Rust reports the rejected site by name and never changes
/// the borrowed board.
#[test]
fn array_collision_rejects_the_site_and_keeps_the_posed_board() {
    let mut board = grid_board(3, 3, Some(0.8));
    board.placements[1].position = board.placements[0].position;
    board.placements[1].side = BoardSide::Back;
    let before = board.clone();
    let error = materialize_via_in_pad_arrays(&board).unwrap_err().to_string();
    assert!(is_site_rejection(&error) && error.contains("U1.17"), "{error}");
    assert_eq!(board, before);
    assert!(board.vias.is_empty());
}

#[test]
fn required_array_forbids_same_net_and_unassigned_pad_contact() {
    for owner_net in [Some("GND"), Some("A"), None] {
        let mut board = grid_board(3, 3, Some(0.8));
        add_footprint(&mut board, footprint("tiny", vec![pad("1", Point::new(0, 0), size(0.1, 0.1))], size(0.2, 0.2)));
        let mut obstacle = place("C1", "tiny", mm(15.8, 10.0));
        obstacle.side = BoardSide::Back;
        board.placements.push(obstacle.clone());
        for net in &mut board.nets {
            if Some(net.name.as_str()) == owner_net {
                net.pads.push(pad_ref("C1", "1"));
            }
        }
        let held: Vec<&Placement> = board.placements.iter().filter(|p| p.reference != "C1").collect();
        assert!(!checker_legal(&board, &obstacle, &held), "{owner_net:?}");
        if owner_net == Some("A") {
            // Separate copper, but a foreign land remains too close to the drill.
            let moved = Placement { position: mm(16.18, 10.0), ..obstacle.clone() };
            assert!(!checker_legal(&board, &moved, &held));
        }
    }
}

#[test]
fn invalid_array_parameters_fail_closed() {
    let cases = [
        ("rows = 2;", "together"),
        ("columns = 2;", "together"),
        ("rows = 0; columns = 1;", "positive integers"),
        ("rows = 2; columns = 1.5;", "positive integers"),
        ("pitch = 0.5mm;", "requires rows and columns"),
        ("rows = 1; columns = 1; spacing = 1mm;", "unknown via_in_pad parameter"),
        // The proxy land is 1 x 1 mm: these fail its fit or the 0.45 mm drill spacing.
        ("rows = 2; columns = 2; pitch = 0.8mm;", "BT1.2 site row 1 column 1 does not fit"),
        ("rows = 3; columns = 3;", "below the 0.45 mm drill spacing"),
        ("rows = 2; columns = 2; pitch = 0.4mm;", "below the 0.45 mm drill spacing"),
    ];
    for (extra, message) in cases {
        let error = lower(extra).expect_err(extra);
        assert!(error.contains(message), "{extra}: {error}");
    }
}

/// The dataclass-level count checks of `PadViaInPadRule` are source-lowering
/// checks in Rust (see `invalid_array_parameters_fail_closed`); the land
/// check remains a board check.
#[test]
fn array_requires_exactly_one_smd_land() {
    let mut board = array_board(3.1, None);
    let footprint = board.footprints["test/qfn16-ep"].clone();
    let mut split = footprint.clone();
    let mut second = footprint.pads.last().unwrap().clone();
    second.position = mm(0.0, 4.0);
    split.pads.push(second);
    add_footprint(&mut board, split);
    board.via_in_pad_rules = vec![array_rule(Some(1), Some(1), None)];
    let error = copper_pcb::physicalize::revalidate(&mut board).unwrap_err().to_string();
    assert!(error.contains("exactly one SMD land"), "{error}");
}

#[test]
fn default_grid_is_centred_and_evenly_spread() {
    for (ep_mm, count, pitch_mm) in [(3.1, 3_i64, 1.03), (5.1, 4, 1.27)] {
        let source = array_board(ep_mm, Some(array_rule(Some(count), Some(count), None)));
        let board = materialize_via_in_pad_arrays(&source).unwrap();
        let pitch = nm(pitch_mm);
        let offsets: Vec<i64> = (0..count).map(|index| (2 * index - count + 1) * pitch / 2).collect();
        let expected: Vec<Point> = offsets
            .iter()
            .flat_map(|dy| offsets.iter().map(move |dx| Point::new(nm(15.0) + dx, nm(10.0) + dy)))
            .collect();
        assert_eq!(board.vias.iter().map(|via| via.position).collect::<Vec<_>>(), expected);
        assert!(
            board
                .vias
                .iter()
                .all(|via| (via.size_nm, via.drill_nm, via.finish.as_str()) == (nm(0.30), nm(0.20), "filled-capped"))
        );
        assert_eq!(board.metadata["via_in_pad_count"], (count * count).to_string());
        assert!(hard(&board).is_empty(), "{:?}", hard(&board));
    }
}

#[test]
fn explicit_pitch_and_rotation_follow_the_land() {
    let board = grid_board(2, 3, Some(0.9));
    let sites = |board: &PhysicalBoard| -> std::collections::BTreeSet<(i64, i64)> {
        via_in_pad_array_vias(board, None).unwrap().iter().map(|v| (v.position.x_nm, v.position.y_nm)).collect()
    };
    let grid = |swap: bool| -> std::collections::BTreeSet<(i64, i64)> {
        [-900_000, 0, 900_000]
            .iter()
            .flat_map(|dx| [-450_000, 450_000].map(|dy| if swap { (dy, *dx) } else { (*dx, dy) }))
            .map(|(dx, dy)| (nm(15.0) + dx, nm(10.0) + dy))
            .collect()
    };
    assert_eq!(sites(&board), grid(false));
    let mut turned = board.clone();
    turned.placements[0].rotation_degrees = Decimal::from_i64(90);
    assert_eq!(sites(&turned), grid(true));
}

#[test]
fn permission_without_array_is_unchanged() {
    let board = array_board(3.1, None);
    let permitted = array_board(3.1, Some(array_rule(None, None, None)));
    assert_eq!(materialize_via_in_pad_arrays(&permitted).unwrap(), permitted);
    assert_eq!(materialize_hard_macros(permitted.clone()).unwrap(), permitted);
    assert_eq!(macro_source(permitted.clone()).unwrap(), permitted);
    assert!(via_in_pad_array_vias(&permitted, None).unwrap().is_empty());
    assert!(via_in_pad_array_report(&permitted).unwrap().is_empty());
    // Placement and routing fingerprints keep the historical digest of a
    // permission-only rule (the Python test pinned its repr).
    assert_eq!(physical_board_digest(&permitted), physical_board_digest(&board));
    assert_ne!(physical_board_digest(&grid_board(1, 1, None)), physical_board_digest(&board));
    // A permission alone never adds array copper to a stitched board.
    assert_eq!(
        stitch_zone_pads(&permitted, None).unwrap().board.vias,
        stitch_zone_pads(&board, None).unwrap().board.vias
    );
}

fn blocked(board: &PhysicalBoard, message: &str) {
    let error = materialize_via_in_pad_arrays(board).unwrap_err().to_string();
    assert!(error.contains(message), "expected {message:?}, got {error:?}");
}

#[test]
fn any_failed_site_rejects_the_whole_array_by_name() {
    let board = grid_board(3, 3, None);
    let corner = "U1.17: site row 1 column 1 at (13.97, 8.97) mm";
    let mut probe = keepout("probe", &[CopperLayer::In3], rect(13.8, 8.8, 14.1, 9.1));
    probe.block_tracks = false;
    probe.block_zones = false;
    blocked(&PhysicalBoard { copper_keepouts: vec![probe], ..board.clone() }, &format!("{corner} violates a keepout"));
    let foreign = track("A", mm(13.0, 8.97), mm(14.0, 8.97), nm(0.1), CopperLayer::In2);
    blocked(
        &PhysicalBoard { tracks: vec![foreign], ..board.clone() },
        &format!("{corner} violates a keepout, copper or drill clearance"),
    );
    let existing = via("GND", mm(13.97, 8.97), nm(0.3), nm(0.2));
    blocked(
        &PhysicalBoard { vias: vec![existing], ..board.clone() },
        &format!("{corner} coincides with an existing via"),
    );
    let mut small = board.zones[0].clone();
    small.outer = rect(14.0, 1.0, 29.0, 19.0);
    blocked(
        &PhysicalBoard { zones: vec![small], ..board.clone() },
        &format!("{corner} lies outside the inner GND zone"),
    );
    // A same-net land is still another land: the annulus may touch only its own.
    let mut spur = board.clone();
    let mut footprint = spur.footprints["test/qfn16-ep"].clone();
    footprint.pads.push(pad("18", mm(-1.03, -1.25), size(0.3, 0.3)));
    add_footprint(&mut spur, footprint);
    spur.nets[0].pads.push(pad_ref("U1", "18"));
    blocked(&spur, &format!("{corner} touches another land"));
    assert!(board.vias.is_empty());
}

#[test]
fn hard_macro_pad_array_is_committed_with_owner_copper() {
    use sha2::{Digest, Sha256};
    // As the `copper` binary does at start-up.
    copper_drc::macros::register_array_materializer(materialize_via_in_pad_arrays);
    // `test_hard_macros.fixture` with a 1 mm land and a GND plane.
    let land = footprint("test", vec![pad("1", Point::new(0, 0), size(1.0, 1.0))], size(0.5, 0.5));
    let digest = copper_physical::footprint::footprint_geometry_digest(&land);
    let region = r#""id": "private", "layers": ["F.Cu"], "vertices": [[1000000, -500000], [3000000, -500000], [3000000, 500000], [1000000, 500000]], "block_tracks": BT, "block_vias": true, "block_zones": BZ"#;
    let asset = format!(
        r#"{{"allowed_rotations": [0, 45, 90], "anchor": "chip", "internal_clearance_nm": 0,
  "isolated_pads": [], "keepouts": [{{{keepout}}}],
  "members": [{{"center_nm": [0, 0], "edge_clearance_nm": 250000, "footprint": "test", "footprint_digest": "{digest}", "reference": "chip", "rotation_degrees": "0"}},
              {{"center_nm": [2000000, 0], "edge_clearance_nm": 250000, "footprint": "test", "footprint_digest": "{digest}", "reference": "passive", "rotation_degrees": "0"}}],
  "pad_nets": [["chip", "1", "signal"], ["passive", "1", "signal"]],
  "ports": [{{"layer": "F.Cu", "name": "signal", "net": "signal", "pads": [["chip", "1"], ["passive", "1"]], "point": {{"pad": ["chip", "1"]}}}}],
  "production_publishable": false, "protected_regions": [{{{protected}}}], "required_layers": ["F.Cu", "B.Cu"],
  "schema": "copperlib-physical-hard-macro/v0.1", "source": "synthetic test",
  "tracks": [{{"layer": "F.Cu", "net": "signal", "points": [{{"pad": ["chip", "1"]}}, {{"pad": ["passive", "1"]}}], "width_nm": 200000}}],
  "unresolved": ["synthetic, not fabrication"], "vias": []}}
"#,
        keepout = region.replace("BT", "false").replace("BZ", "true"),
        protected = region.replace("BT", "true").replace("BZ", "false"),
    );
    let directory = scratch("pad-via-arrays-hard-macro");
    let path = directory.join("ground.json");
    std::fs::write(&path, &asset).unwrap();
    let sha: String = Sha256::digest(asset.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let mut base = board("MacroTest", 30.0, 30.0);
    add_footprint(&mut base, land);
    base.placements = vec![place("U1", "test", mm(10.0, 10.0)), place("R1", "test", mm(12.0, 10.0))];
    base.nets = vec![net("GND", &[("U1", "1"), ("R1", "1")])];
    base.stackup = six();
    base.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    base.zones = vec![zone("ground", "GND", &[CopperLayer::In1], rect(1.0, 1.0, 29.0, 29.0))];
    let bindings: IndexMap<String, String> =
        [("chip", "U1"), ("passive", "R1")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let net_bindings: IndexMap<String, String> = [("signal".to_owned(), "GND".to_owned())].into_iter().collect();
    let bind = |component: &str, rows: i64, columns: i64| {
        let mut ruled = base.clone();
        ruled.via_in_pad_rules = vec![PadViaInPadRule {
            pad: pad_ref(component, "1"),
            rows: Some(rows),
            columns: Some(columns),
            pitch_nm: None,
        }];
        copper_pcb::hard_macros::bind_hard_macro(
            ruled,
            &path,
            asset.as_bytes(),
            &sha,
            "ground",
            &bindings,
            &net_bindings,
        )
        .expect("bind")
    };

    let result = materialize_hard_macros(bind("U1", 2, 2)).unwrap();
    assert_eq!(result.materialized_macros, vec!["ground".to_owned()]);
    assert_eq!(result.tracks.len(), 1);
    assert_eq!(result.vias, via_in_pad_array_vias(&result, None).unwrap());
    assert_eq!(result.vias.len(), 4);
    assert_eq!(materialize_hard_macros(macro_source(result.clone()).unwrap()).unwrap(), result);
    assert!(hard(&result).is_empty(), "{:?}", hard(&result));
    assert_eq!(stitch_zone_pads(&result, None).unwrap().stitched_pads, vec![pad_ref("R1", "1"), pad_ref("U1", "1")]);
    // Array copper obeys the macro's private region like any other new copper.
    let error = materialize_hard_macros(bind("R1", 1, 1)).unwrap_err().to_string();
    assert!(error.contains("R1.1: site row 1 column 1") && error.contains("violates a keepout"), "{error}");
}

#[test]
fn back_fill_keepout_does_not_block_inner_plane_via_array() {
    let mut board = grid_board(1, 1, None);
    board.zones[0].layers = vec![CopperLayer::In1, CopperLayer::Back];
    let mut fill_only = keepout("back-fill-only", &[CopperLayer::Back], board.zones[0].outer.clone());
    fill_only.block_tracks = false;
    fill_only.block_vias = false;
    board.copper_keepouts = vec![fill_only];
    assert_eq!(materialize_via_in_pad_arrays(&board).unwrap().vias.len(), 1);
}

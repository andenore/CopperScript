//! Pad-scoped permissions never enable via-in-pad on neighbouring lands;
//! ported from `tests/test_pad_via_in_pad.py`. The Python `replace(board,
//! via_in_pad_rules=...)` validation is `copper_pcb::physicalize::revalidate`.

mod support_planes;

use copper_pcb::board::{PadViaInPadRule, PhysicalBoard, Stackup};
use copper_pcb::physicalize::revalidate;
use copper_physical::CopperLayer;
use copper_route::plane::stitch_zone_pads;
use support_planes::*;

/// `qualified_board`: 8 mm GND lands on a six-layer JLCPCB board.
fn qualified_board() -> PhysicalBoard {
    let mut board = plane_board();
    board.footprints.get_mut("test/one-pad").unwrap().pads[0].size = size(8.0, 8.0);
    board.stackup = Stackup::new(vec![
        CopperLayer::Front,
        CopperLayer::In1,
        CopperLayer::In2,
        CopperLayer::In3,
        CopperLayer::In4,
        CopperLayer::Back,
    ]);
    board.metadata = [("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned())].into_iter().collect();
    board
}

fn permission(component: &str, number: &str) -> PadViaInPadRule {
    PadViaInPadRule { pad: pad_ref(component, number), rows: None, columns: None, pitch_nm: None }
}

/// `replace(board, via_in_pad_rules=rules)`.
fn permitted(mut board: PhysicalBoard, rules: Vec<PadViaInPadRule>) -> Result<PhysicalBoard, String> {
    board.via_in_pad_rules = rules;
    revalidate(&mut board).map_err(|error| error.to_string())?;
    Ok(board)
}

fn stitch(board: &PhysicalBoard) -> copper_route::plane::PlaneStitchResult {
    stitch_zone_pads(board, None).unwrap()
}

#[test]
fn only_explicit_ground_pad_gets_filled_capped_via() {
    let base = qualified_board();
    let pending: std::collections::BTreeSet<_> = stitch(&base).pending_pads.into_iter().collect();
    assert_eq!(pending, pads(&[("J1", "1"), ("J2", "1")]));
    let board = permitted(base, vec![permission("J1", "1")]).unwrap();
    let result = stitch(&board);
    assert_eq!(result.stitched_pads, [pad_ref("J1", "1")]);
    assert_eq!(result.pending_pads, [pad_ref("J2", "1")]);
    assert_eq!(result.board.vias.len(), 1);
    assert_eq!(result.board.vias[0].finish, "filled-capped");
    assert_eq!(result.board.vias[0].position, board.placements[0].position);
    assert_eq!(stitch(&result.board).board.vias.len(), 1);
}

#[test]
fn missing_unconnected_pad_is_rejected() {
    for (component, number) in [("MISSING", "1"), ("J1", "9")] {
        let error = permitted(qualified_board(), vec![permission(component, number)]).unwrap_err();
        assert!(error.contains("connected GND"), "{component}.{number}: {error}");
    }
}

#[test]
fn permission_rejects_wrong_process_profile_and_duplicate() {
    // `PadViaInPadRule` has no process field in Rust: a non-`filled-capped`
    // process is rejected at source lowering (`invalid_source_permission_fails_closed`).
    let mut unqualified = qualified_board();
    unqualified.metadata.clear();
    let error = permitted(unqualified, vec![permission("J1", "1")]).unwrap_err();
    assert!(error.contains("six-layer"), "{error}");
    let error = permitted(qualified_board(), vec![permission("J1", "1"), permission("J1", "1")]).unwrap_err();
    assert!(error.contains("multiple"), "{error}");
}

#[test]
fn permission_does_not_allow_contact_with_another_same_net_pad() {
    let mut board = qualified_board();
    let mut tiny = board.footprints["test/one-pad"].clone();
    tiny.name = "tiny".to_owned();
    tiny.pads[0].size = size(0.2, 0.2);
    add_footprint(&mut board, tiny);
    board.placements[1] = place("J2", "tiny", mm(3.2, 6.0));
    let board = permitted(board, vec![permission("J1", "1")]).unwrap();
    let result = stitch(&board);
    assert!(result.pending_pads.contains(&pad_ref("J1", "1")), "{:?}", result.stitched_pads);
    assert!(!result.board.vias.iter().any(|via| via.finish == "filled-capped"));
}

fn source(target: &str, process: &str, extra: &str) -> String {
    format!(
        "board Test {{ use library \"tiny\";\n    component BT1: CAPACITOR;\n    net GND {{ BT1.2; }}\n    \
         constraint copper_zone(GND) {{ layers = \"In1.Cu\"; }}\n    \
         constraint via_in_pad({target}) {{ process = \"{process}\"; {extra} }}\n}}\n"
    )
}

fn lower(target: &str, process: &str, extra: &str) -> Result<PhysicalBoard, String> {
    physicalize_source("board.copper", &source(target, process, extra), 6, "jlcpcb-six-layer")
}

#[test]
fn source_constraint_lowers_semantic_pin_to_typed_physical_permission() {
    assert_eq!(lower("BT1.2", "filled-capped", "").unwrap().via_in_pad_rules, [permission("BT1", "2")]);
}

#[test]
fn invalid_source_permission_fails_closed() {
    for (target, process, extra) in [
        ("BT1", "filled-capped", ""),
        ("NOPE.2", "filled-capped", ""),
        ("BT1.1", "filled-capped", ""),
        ("BT1.2", "open", ""),
        ("BT1.2", "filled-capped", "typo = 1;"),
    ] {
        assert!(lower(target, process, extra).is_err(), "{target} {process} {extra}");
    }
}

#[test]
fn editor_projection_digest_keeps_via_in_pad_profile() {
    use copper_editor::overlay::{projection_digest, write_intent};
    let board = permitted(qualified_board(), vec![permission("J1", "1")]).unwrap();
    // Stripping all metadata would fail the via-in-pad profile validation.
    let mut run = board.clone();
    run.metadata.insert("run".to_owned(), "x".to_owned());
    assert_eq!(projection_digest(&board), projection_digest(&run));
    let directory = scratch("pad-via-in-pad-editor");
    let source = directory.join("board.copper");
    std::fs::write(&source, "board B {}\n").unwrap();
    write_intent(&source, &board, &stitch(&board).board, &directory.join("board.editor-intent.json"), None, None)
        .unwrap();
    // Boards without permissions keep the metadata-free digest they had before.
    let plain = qualified_board();
    let mut stripped = plain.clone();
    stripped.tracks.clear();
    stripped.vias.clear();
    stripped.materialized_macros.clear();
    stripped.metadata.clear();
    assert_eq!(projection_digest(&plain), copper_drc::digest::physical_board_digest(&stripped));
}

//! Mechanical intent survives both board decoders and unified-input replay.
mod support_routers;

use copper_lang::mechanical::assembly::{AssemblyEnvelope, BodyOverhang};
use copper_pcb::board::{ComponentPlacementRule, PhysicalAttachment, PhysicalBoard};
use copper_physical::{BoardDatum, BoardEdge, BoardSide, Point};
use copper_route::state_codec::{encode_board, inputs_state_text, load_inputs, physical_board};
use copper_route::unified_error::Error;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_state::StateValue;
use copper_units::Decimal;
use support_routers::*;

fn mechanical_board() -> PhysicalBoard {
    let mut board = board(
        "Mechanical checkpoint",
        140.0,
        35.0,
        vec![footprint("connector", vec![], 1.0, 1.0)],
        vec![placement("J_USB", "connector", 139.0, 17.5)],
        vec![],
    );
    board.boundary_edges.push(BoardEdge::new("USB_END", pt(140.0, 0.0), pt(140.0, 35.0)).unwrap());
    board.body_overhangs.push(BodyOverhang {
        id: "USB_NOSE".to_owned(),
        reference: "J_USB".to_owned(),
        edge: "USB_END".to_owned(),
        start_nm: 11_500_000,
        end_nm: 23_500_000,
        distance_nm: 2_000_000,
        reason: "  Drawing permits body only; copper remains inside.  ".to_owned(),
    });
    board.placement_rules.push(ComponentPlacementRule {
        fixed_position: Some(board.placements[0].position),
        fixed_rotation_degrees: Some(Decimal::zero()),
        side: Some(BoardSide::Front),
        ..ComponentPlacementRule::new("J_USB")
    });
    board
}

fn assert_both_roundtrip(board: &PhysicalBoard) {
    let encoded = encode_board(board);
    for decoded in [physical_board(&encoded).unwrap(), copper_pcb::state_codec::board(&encoded).unwrap()] {
        assert_eq!(decoded.body_overhangs, board.body_overhangs);
        assert_eq!(decoded.boundary_edges, board.boundary_edges);
        assert_eq!(decoded.outline, board.outline);
        assert_eq!(decoded.placement_rules, board.placement_rules);
        assert_eq!(decoded.assembly_envelopes, board.assembly_envelopes);
        // Avoid dumping a megabyte-sized real fixture if another saved field drifts.
        assert!(encode_board(&decoded) == encoded, "board encode/decode changed saved fields");
    }
}

#[test]
fn named_edge_and_audited_body_allowance_roundtrip_without_widening_board() {
    assert_both_roundtrip(&mechanical_board());
    let mut reversed = mechanical_board();
    let edge = &mut reversed.boundary_edges[0];
    std::mem::swap(&mut edge.start, &mut edge.end);
    assert_both_roundtrip(&reversed);
}

/// `mechanical_board` under an enclosure height limit on its back side.
fn enveloped_board() -> PhysicalBoard {
    let mut board = mechanical_board();
    board.assembly_envelopes.push(AssemblyEnvelope {
        id: "LID".to_owned(),
        outline: board.outline.clone(),
        side: BoardSide::Back,
        maximum_height_nm: 3_500_000,
    });
    board
}

#[test]
fn assembly_envelopes_roundtrip_and_replay() {
    assert_both_roundtrip(&enveloped_board());
    let board = enveloped_board();
    let global = routes::global(&board, 5.0);
    let text = inputs_state_text(&board, &global, &UnifiedRouterOptions::default(), None, false).unwrap();
    let decoded = load_inputs(&text, "synthetic.state.json").unwrap();
    assert_eq!(decoded.board.assembly_envelopes, board.assembly_envelopes);
}

#[test]
fn malformed_and_duplicate_assembly_envelopes_are_rejected() {
    let mut duplicate = enveloped_board();
    duplicate.assembly_envelopes.push(duplicate.assembly_envelopes[0].clone());
    assert!(physical_board(&encode_board(&duplicate)).is_err());
    let mut encoded = encode_board(&enveloped_board());
    let StateValue::Dataclass { fields, .. } = &mut encoded else { panic!("board") };
    let StateValue::Tuple(items) = fields.get_mut("assembly_envelopes").unwrap() else { panic!("envelopes") };
    let StateValue::Dataclass { fields, .. } = &mut items[0] else { panic!("envelope") };
    fields.insert("unrecognized_constraint".to_owned(), StateValue::Bool(true));
    assert!(physical_board(&encoded).is_err());
    assert!(copper_pcb::state_codec::board(&encoded).is_err());
}

#[test]
fn unused_named_edges_still_require_an_actual_boundary_segment() {
    for start in [pt(139.0, 0.0), pt(140.0, 1.0)] {
        let mut board = mechanical_board();
        board.body_overhangs.clear();
        board.boundary_edges[0].start = start;
        let encoded = encode_board(&board);
        assert!(physical_board(&encoded).is_err());
        assert!(copper_pcb::state_codec::board(&encoded).is_err());
    }
}

#[test]
fn unified_inputs_preserve_mechanical_and_fixed_placement_intent() {
    let board = mechanical_board();
    let global = routes::global(&board, 5.0);
    let text = inputs_state_text(&board, &global, &UnifiedRouterOptions::default(), None, false).unwrap();
    let decoded = load_inputs(&text, "synthetic.state.json").unwrap();
    assert!(encode_board(&decoded.board) == encode_board(&board));
    assert_both_roundtrip(&decoded.board);
}

#[test]
fn route_decoder_validates_overhang_owner_interval_and_boundary() {
    for fault in 0..5 {
        let mut board = mechanical_board();
        match fault {
            0 => board.body_overhangs[0].reference = "missing".to_owned(),
            1 => board.body_overhangs[0].end_nm = 36_000_000,
            2 => board.body_overhangs[0].distance_nm = 0,
            3 => board.boundary_edges[0].start = pt(139.0, 0.0),
            4 => board.boundary_edges.push(board.boundary_edges[0].clone()),
            _ => unreachable!(),
        }
        assert!(matches!(physical_board(&encode_board(&board)), Err(Error::Value(_))), "fault {fault}");
    }
}

#[test]
fn route_decoder_does_not_drop_unknown_overhang_fields() {
    let mut encoded = encode_board(&mechanical_board());
    let StateValue::Dataclass { fields, .. } = &mut encoded else { panic!("board") };
    let StateValue::Tuple(items) = fields.get_mut("body_overhangs").unwrap() else { panic!("allowances") };
    let StateValue::Dataclass { fields, .. } = &mut items[0] else { panic!("allowance") };
    fields.insert("unrecognized_constraint".to_owned(), StateValue::Bool(true));
    assert!(matches!(physical_board(&encoded), Err(Error::Value(_))));
}

#[test]
fn unvalidated_datum_and_attachment_semantics_remain_explicitly_unsupported() {
    let mut board = mechanical_board();
    board.datums.push(BoardDatum {
        id: "origin".to_owned(),
        position: Point::new(0, 0),
        relative_to: None,
        offset: Point::new(0, 0),
    });
    assert!(matches!(physical_board(&encode_board(&board)), Err(Error::NotImplemented(_))));
    board.datums.clear();
    board.attachments.push(PhysicalAttachment {
        id: "mount".to_owned(),
        reference: "J_USB".to_owned(),
        target: None,
        position: board.placements[0].position,
        offset: Point::new(0, 0),
        anchor: "origin".to_owned(),
        anchor_point: Point::new(0, 0),
        rotation: Decimal::zero(),
        side: BoardSide::Front,
    });
    assert!(matches!(physical_board(&encode_board(&board)), Err(Error::NotImplemented(_))));
}

#[test]
#[ignore = "set COPPER_UNIFIED_CHECKPOINT to a saved CopperProbe unified-inputs.state.json"]
fn actual_unified_checkpoint_preserves_all_board_fields() {
    let path = std::env::var("COPPER_UNIFIED_CHECKPOINT").expect("COPPER_UNIFIED_CHECKPOINT");
    let text = std::fs::read_to_string(&path).expect("checkpoint");
    let document = copper_state::StateDocument::read(&text).expect("state document");
    let StateValue::Map { entries, .. } = &document.value else { panic!("saved inputs mapping") };
    let original_board =
        &entries.iter().find(|(key, _)| key == &StateValue::Str("board".into())).expect("board entry").1;
    let saved = load_inputs(&text, &path).expect("decode actual unified checkpoint");
    assert!(!saved.board.body_overhangs.is_empty(), "fixture must exercise mechanical allowance");
    assert!(encode_board(&saved.board) == *original_board, "decoder changed original saved board fields");
    assert_both_roundtrip(&saved.board);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

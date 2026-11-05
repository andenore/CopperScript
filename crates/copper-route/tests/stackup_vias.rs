//! A stack-up via catalog validates span, ring and aspect ratio, and the
//! KiCad backend keeps the multilayer stack and via kinds; ported from
//! `tests/test_stackup_vias.py`. Python validated a board at construction;
//! Rust validates a board when its state is decoded, so the ring case
//! round-trips the board through the state codec.

mod support_planes;

use copper_lang::mechanical::stackup::StackupLayer;
use copper_pcb::board::{PhysicalBoard, Stackup, Via, ViaKind, ViaTechnology};
use copper_physical::CopperLayer;
use copper_route::routing_vias::select_via_technology;
use copper_route::state_codec::{encode_board, physical_board};
use copper_units::Decimal;
use support_planes::*;

const COPPER: [CopperLayer; 4] = [CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back];

fn four_layer_stackup() -> Stackup {
    let copper = |id: &str, layer: CopperLayer| StackupLayer {
        id: id.to_owned(),
        copper_layer: Some(layer),
        thickness_nm: nm(0.035),
        material: None,
        relative_permittivity: None,
        loss_tangent: None,
        dielectric_type: None,
    };
    let dielectric = |id: &str, thickness: f64| StackupLayer {
        id: id.to_owned(),
        copper_layer: None,
        thickness_nm: nm(thickness),
        material: Some("FR-4".to_owned()),
        relative_permittivity: Some(Decimal::parse("4.2").unwrap()),
        loss_tangent: Some(Decimal::parse("0.02").unwrap()),
        dielectric_type: None,
    };
    let technology = |id: &str, kind, to_layer, drill: f64, ring: f64, ratio: i64| ViaTechnology {
        id: id.to_owned(),
        kind,
        from_layer: COPPER[0],
        to_layer,
        minimum_drill_nm: nm(drill),
        minimum_annular_ring_nm: nm(ring),
        maximum_aspect_ratio: Decimal::from_i64(ratio),
    };
    Stackup {
        copper_layers: COPPER.to_vec(),
        thickness_nm: nm(1.6),
        physical_layers: vec![
            copper("L1", COPPER[0]),
            dielectric("D1", 0.1),
            copper("L2", COPPER[1]),
            dielectric("D2", 0.66),
            copper("L3", COPPER[2]),
            dielectric("D3", 0.7),
            copper("L4", COPPER[3]),
        ],
        via_technologies: vec![
            technology("TH", ViaKind::Through, COPPER[3], 0.3, 0.15, 6),
            technology("UV1", ViaKind::Microvia, COPPER[1], 0.1, 0.075, 2),
        ],
    }
}

fn four_layer_board(via: Via) -> PhysicalBoard {
    let mut board = board("FourLayer", 20.0, 10.0);
    board.nets = vec![net("N", &[])];
    board.stackup = four_layer_stackup();
    board.vias = vec![via];
    board
}

fn catalog_via(size_mm: f64, drill_mm: f64, to_layer: CopperLayer, technology: &str) -> Via {
    Via { to_layer, technology: Some(technology.to_owned()), ..via("N", mm(5.0, 5.0), nm(size_mm), nm(drill_mm)) }
}

fn decoded(board: &PhysicalBoard) -> Result<PhysicalBoard, String> {
    physical_board(&encode_board(board)).map_err(|error| format!("{error:?}"))
}

#[test]
fn stackup_via_catalog_validates_span_ring_and_aspect_ratio() {
    let board = four_layer_board(catalog_via(0.7, 0.4, CopperLayer::Back, "TH"));
    assert_eq!(decoded(&board).unwrap().vias[0].technology.as_deref(), Some("TH"));
    let stackup = &board.stackup;
    assert_eq!(
        select_via_technology(stackup, CopperLayer::Front, CopperLayer::Back, nm(0.7), nm(0.4)),
        Ok(Some("TH".to_owned()))
    );
    let thin = PhysicalBoard { vias: vec![Via { size_nm: nm(0.5), ..board.vias[0].clone() }], ..board.clone() };
    let error = decoded(&thin).unwrap_err();
    assert!(error.contains("annular ring"), "{error}");
}

#[test]
fn kicad_backend_maps_multilayer_stack_and_microvia() {
    let board = four_layer_board(catalog_via(0.3, 0.1, CopperLayer::In1, "UV1"));
    let text = copper_pcb::kicad_pcb::generate(&board).unwrap().artifacts[0].content.clone();
    assert!(text.contains("(1 \"In1.Cu\" signal)"));
    assert!(text.contains("(2 \"In2.Cu\" signal)"));
    assert!(text.contains("(via micro"));
}

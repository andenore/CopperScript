//! `tests/test_critical_search_order.py`: internal SMD pair lands try
//! matched via portals first; the order is reported.

use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_physical::CopperLayer::Front as FRONT;
use copper_physical::Point;
use copper_physical::footprint::Size;
use copper_units::Decimal;

use crate::critical::{
    CriticalNetResult, CriticalRoutingResult, CriticalRoutingStatus, critical_progress, pair_prefers_via_escape,
};
use crate::routing::{GlobalNetRoute, GridNode, PinAccess};
use crate::test_support_critical::*;

/// `setup(perimeter)`: package U at (10, 10) with lands 1 and 2 internal
/// (or on the east perimeter) and corner lands 3 and 4; one guide per land.
fn setup(perimeter: bool) -> (PhysicalBoard, [GlobalNetRoute; 2]) {
    let x = if perimeter { 3 * MM } else { 0 };
    let (one, two) = (at(x, 0), at(if perimeter { x } else { 500_000 }, -500_000));
    let pads = vec![
        smd_pad("1", one, 300_000),
        smd_pad("2", two, 300_000),
        smd_pad("3", mm(-3, -3), 300_000),
        smd_pad("4", mm(3, 3), 300_000),
    ];
    let package = footprint("package", pads, Size { width_nm: 7 * MM, height_nm: 7 * MM });
    let board = board("order", 20, 20, vec![package], vec![pose("U", "package", mm(10, 10))], Vec::new());
    let guide = |pad: &str| {
        let node = GridNode { layer_index: 0, x_index: 0, y_index: 0 };
        GlobalNetRoute {
            net: pad.to_owned(),
            connected: false,
            accesses: vec![PinAccess::new(
                PadReference::new("U", pad),
                Point::new(0, 0),
                node,
                Point::new(0, 0),
                FRONT,
            )],
            segments: Vec::new(),
            vias: Vec::new(),
            length_nm: 0,
            diagnostics: Vec::new(),
            deferred_to_zone: false,
        }
    };
    (board, [guide("1"), guide("2")])
}

fn prefers(board: &PhysicalBoard, guides: &[GlobalNetRoute; 2]) -> bool {
    pair_prefers_via_escape(board, &guides[0], &guides[1]).expect("classification")
}

#[test]
fn internal_pair_prefers_vias_but_perimeter_pair_retains_surface_first() {
    let (board, guides) = setup(false);
    assert!(prefers(&board, &guides));
    let (board, guides) = setup(true);
    assert!(!prefers(&board, &guides));
}

#[test]
fn duplicate_land_on_perimeter_blocks_internal_classification() {
    let (mut board, guides) = setup(false);
    board.footprints["package"].pads.push(smd_pad("1", mm(3, 0), 300_000));
    assert!(!prefers(&board, &guides));
}

#[test]
fn pose_does_not_change_local_internal_land_classification() {
    let (mut board, guides) = setup(false);
    board.placements[0].rotation_degrees = Decimal::from_i64(45);
    assert!(prefers(&board, &guides));
}

struct Events(Vec<(String, String, Vec<(&'static str, copper_json::Value)>)>);

impl crate::progress::Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, copper_json::Value)>) {
        self.0.push((phase.to_owned(), event.to_owned(), details));
    }
}

#[test]
fn ordering_is_in_report_and_progress_telemetry() {
    let (board, _) = setup(false);
    let net = CriticalNetResult {
        pair_search_order: Some("via_first_internal_lands".to_owned()),
        pair_state_limit: Some(6000),
        pair_budget_exhausted: false,
        ..CriticalNetResult::new(vec!["P".to_owned(), "N".to_owned()], true, 0, 0, Vec::new(), 0, Vec::new())
    };
    let result = CriticalRoutingResult {
        status: CriticalRoutingStatus::Success,
        board,
        nets: vec![net.clone()],
        locked_tracks: Vec::new(),
        locked_vias: Vec::new(),
        global_routing_fingerprint: "g".to_owned(),
        routing_fingerprint: "r".to_owned(),
        reserved_track_count: 0,
        reserved_via_count: 0,
        bundles: Vec::new(),
        match_tuning: Vec::new(),
        plane_reservation_track_count: 0,
        plane_reservation_via_count: 0,
    };
    let Ok(copper_json::Value::Object(document)) = copper_json::parse(&result.to_json().expect("JSON")) else {
        panic!("a document")
    };
    let copper_json::Value::Array(nets) = &document["nets"] else { panic!("nets") };
    let copper_json::Value::Object(first) = &nets[0] else { panic!("a net") };
    assert_eq!(first["pair_search_order"], copper_json::Value::from("via_first_internal_lands"));
    assert_eq!(first["pair_state_limit"], copper_json::Value::from(6000i64));
    let mut events = Events(Vec::new());
    critical_progress(&mut Some(&mut events), "finished", &net.nets, Some(&net));
    let details = &events.0[0].2;
    assert!(details.contains(&("pair_search_order", copper_json::Value::from("via_first_internal_lands"))));
    assert!(details.contains(&("pair_state_limit", copper_json::Value::from(6000i64))));
    assert!(details.contains(&("pair_budget_exhausted", copper_json::Value::Bool(false))));
}

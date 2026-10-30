//! Corridor-aware placement for `reserve_corridor` differential pairs
//! (ported from the former `tests/test_corridor_reservation.py`; the
//! `plan-layout` CLI case is in `crates/copper/tests/corridor_cli.rs`).

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use boards::*;
use copper_drc::legality::{
    Checker, corridor_reservation, placement_polygon, placement_rejection_reasons, polygons_too_close,
    reserved_corridors,
};
use copper_json::Value;
use copper_pcb::board::{ComponentPlacementRule, NetRoutingRule, PhysicalBoard, Placement, RouteKind};
use copper_physical::{BoardSide, Point};
use copper_place::layout::Status;
use copper_place::scene::Poses;
use copper_place::{PlannerOptions, plan_placement};
use copper_units::Decimal;

fn options() -> PlannerOptions {
    PlannerOptions { candidate_count: 1, ..PlannerOptions::default() }.validated().expect("options")
}

struct Shape {
    reserve: bool,
    fix_connector: bool,
    fixed_capacitor: Option<Point>,
    pair_kind: RouteKind,
    profile: bool,
}

const DEFAULT: Shape = Shape {
    reserve: true,
    fix_connector: true,
    fixed_capacitor: None,
    pair_kind: RouteKind::Differential,
    profile: true,
};

fn fixed(reference: &str, at: Point, side: Option<BoardSide>) -> ComponentPlacementRule {
    ComponentPlacementRule {
        fixed_position: Some(at),
        fixed_rotation_degrees: Some(Decimal::from_i64(0)),
        side,
        ..ComponentPlacementRule::new(reference)
    }
}

/// A fixed chip and connector joined by DP/DN, plus a movable capacitor
/// beside the pair that wire length pulls into its corridor.
fn corridor_board(shape: Shape) -> PhysicalBoard {
    let lands = |x: f64| vec![pad("1", x, -0.25, 0.6, 0.25), pad("2", x, 0.25, 0.6, 0.25), pad("3", x, 1.0, 0.6, 0.25)];
    let chip = footprint("test/chip", lands(1.4), 3.0, 3.0);
    let connector = footprint("test/connector", lands(-1.4), 3.0, 3.0);
    let capacitor =
        footprint("test/capacitor", vec![pad("1", -0.5, 0.0, 0.5, 0.5), pad("2", 0.5, 0.0, 0.5, 0.5)], 1.6, 0.8);
    let mut rules = vec![fixed("U1", pt(8.0, 10.0), Some(BoardSide::Front))];
    if shape.fix_connector {
        rules.push(fixed("J1", pt(24.0, 10.0), Some(BoardSide::Front)));
    }
    if let Some(at) = shape.fixed_capacitor {
        rules.push(fixed("C1", at, None));
    }
    let mut board = board(
        "CorridorReservation",
        32.0,
        20.0,
        vec![chip, connector, capacitor],
        vec![
            placement("U1", "test/chip", 8.0, 10.0),
            placement("J1", "test/connector", 24.0, 10.0),
            placement("C1", "test/capacitor", 16.0, 4.0),
        ],
        vec![
            net("DP", &[("U1", "1"), ("J1", "1")]),
            net("DN", &[("U1", "2"), ("J1", "2")]),
            net("A", &[("U1", "3"), ("C1", "1")]),
            net("B", &[("J1", "3"), ("C1", "2")]),
        ],
    );
    board.placement_rules = rules;
    let member = |name: &str, partner: &str, reserve: bool| NetRoutingRule {
        kind: shape.pair_kind,
        differential_partner: Some(partner.to_owned()),
        width_nm: shape.profile.then(|| mm(0.15)),
        clearance_nm: shape.profile.then(|| mm(0.25)),
        pair_gap_nm: shape.profile.then(|| mm(0.15)),
        reserve_corridor: reserve,
        ..rule(name)
    };
    // Only one member requests the reservation; it covers the pair.
    board.net_routing_rules = vec![member("DN", "DP", false), member("DP", "DN", shape.reserve)];
    board
}

fn poses(board: &PhysicalBoard) -> Poses {
    board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect()
}

fn pose<'b>(board: &'b PhysicalBoard, reference: &str) -> &'b Placement {
    board.placements.iter().find(|p| p.reference == reference).expect("placement")
}

fn overlaps_corridor(board: &PhysicalBoard, placement: &Placement, corridor_board: &PhysicalBoard) -> bool {
    let corridor = reserved_corridors(corridor_board).expect("corridors").remove(0);
    polygons_too_close(&placement_polygon(board, placement), &corridor.keepout.outline.vertices, 0)
}

fn legal(board: &PhysicalBoard, placements: &Poses, options: &PlannerOptions) -> bool {
    Checker::new(board, options.policy()).placement_solution_is_legal(placements).expect("legal")
}

fn with_c1(placements: &Poses, x: f64, y: f64, side: BoardSide) -> Poses {
    let mut inside = placements.clone();
    inside["C1"].position = pt(x, y);
    inside["C1"].side = side;
    inside
}

/// `prototype_physicalize(compile_source(source))`.
fn prototype(source: &str) -> Result<PhysicalBoard, String> {
    let design = copper_lang::compiler::compile_design_source(source, "test.copper", false, true)
        .map_err(|e| format!("{e:?}"))?;
    let options =
        copper_pcb::physicalize::PhysicalOptions::new(100.0, 80.0, 2, "generic").map_err(|e| e.to_string())?;
    copper_pcb::physicalize::physicalize(&design, copper_pcb::physicalize::Footprints::Proxy, &options, false, true)
        .map_err(|e| e.to_string())
}

fn routed_source(body: &str) -> String {
    format!(
        r#"board Routed {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "0402"; }}
            component R2: RESISTOR {{ footprint = "0402"; }}
            net USB_DP {{ R1.1; R2.1; }}
            net USB_DM {{ R1.2; R2.2; }}
            constraint routing(USB_DP) {{ {body} }}
        }}"#
    )
}

#[test]
fn reserve_corridor_lowers_from_source_and_defaults_off() {
    let pair = "kind = differential; partner = USB_DM; pair_gap = 0.2mm;";
    let reserved = prototype(&routed_source(&format!("{pair} reserve_corridor = true;"))).expect("board");
    let default = prototype(&routed_source(pair)).expect("board");
    assert!(reserved.net_routing_rules[0].reserve_corridor);
    assert!(!default.net_routing_rules[0].reserve_corridor);
    use copper_drc::digest::physical_board_digest;
    assert_ne!(physical_board_digest(&reserved), physical_board_digest(&default));
}

#[test]
fn reserve_corridor_rejects_invalid_source() {
    for (body, message) in [
        ("reserve_corridor = true;", "requires a differential partner"),
        ("kind = differential; partner = USB_DM; pair_gap = 0.2mm; reserve_corridor = 1;", "must be boolean"),
    ] {
        let error = prototype(&routed_source(body)).unwrap_err();
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn corridor_covers_terminal_lands_expanded_by_pair_margin() {
    let (corridors, skipped) = corridor_reservation(&corridor_board(DEFAULT)).expect("reservation");
    assert!(skipped.is_empty());
    assert_eq!(corridors.len(), 1);
    let corridor = &corridors[0];
    assert_eq!(corridor.nets, ("DN".to_owned(), "DP".to_owned()));
    assert_eq!(corridor.keepout.name, "reserved-corridor:DN/DP");
    assert_eq!(corridor.keepout.side, Some(BoardSide::Front));
    assert_eq!(corridor.terminal_references, ["J1", "U1"]);
    // width 0.15 + pair gap 0.15 + rule clearance 0.25 (above the 0.2 board minimum).
    assert_eq!(corridor.margin_nm, mm(0.55));
    let vertices = &corridor.keepout.outline.vertices;
    let xs: Vec<i64> = vertices.iter().map(|p| p.x_nm).collect();
    let ys: Vec<i64> = vertices.iter().map(|p| p.y_nm).collect();
    assert_eq!((*xs.iter().min().expect("x"), *xs.iter().max().expect("x")), (mm(8.55), mm(23.45)));
    assert_eq!((*ys.iter().min().expect("y"), *ys.iter().max().expect("y")), (mm(9.075), mm(10.925)));
}

#[test]
fn corridor_margin_falls_back_to_board_rules() {
    let board = corridor_board(Shape { pair_kind: RouteKind::General, profile: false, ..DEFAULT });
    let corridors = reserved_corridors(&board).expect("corridors");
    assert_eq!(
        corridors[0].margin_nm,
        board.rules.default_track_width_nm + board.rules.minimum_clearance_nm + board.rules.minimum_clearance_nm
    );
}

#[test]
fn corridor_spans_both_sides_when_terminals_are_on_different_sides() {
    let mut board = corridor_board(DEFAULT);
    board.placements[1].side = BoardSide::Back;
    for rule in &mut board.placement_rules {
        if rule.reference == "J1" {
            rule.side = Some(BoardSide::Back);
        }
    }
    assert_eq!(reserved_corridors(&board).expect("corridors")[0].keepout.side, None);
    let placements = poses(&board);
    assert!(legal(&board, &placements, &options()));
    for side in [BoardSide::Front, BoardSide::Back] {
        assert!(!legal(&board, &with_c1(&placements, 16.0, 10.0, side), &options()));
    }
}

#[test]
fn movable_capacitor_is_placed_outside_the_reserved_corridor() {
    let board = corridor_board(DEFAULT);
    let plan = plan_placement(&board, &options()).expect("plan");
    assert!(!overlaps_corridor(&plan.board, pose(&plan.board, "C1"), &board));
    assert!(legal(&board, &poses(&plan.board), &options()));
    let codes: Vec<&str> = plan.report.findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&"RESERVED_CORRIDORS") && !codes.contains(&"CORRIDOR_NOT_RESERVED"));
}

#[test]
fn without_reserve_corridor_only_the_automatic_corridor_protects_the_pair() {
    let unreserved = corridor_board(Shape { reserve: false, ..DEFAULT });
    let manual_only = PlannerOptions { auto_pair_corridors: false, ..options() };
    let plan = plan_placement(&unreserved, &manual_only).expect("plan");
    assert!(reserved_corridors(&unreserved).expect("corridors").is_empty());
    // The same placement would block the pair: a decoupling capacitor in the breakout.
    assert!(overlaps_corridor(&plan.board, pose(&plan.board, "C1"), &corridor_board(DEFAULT)));
    assert!(plan.report.document().get("reserved_corridors").is_some());
    // The pair's terminals are fixed, so the automatic corridor keeps C1 out.
    let automatic = plan_placement(&unreserved, &options()).expect("plan");
    assert!(!overlaps_corridor(&automatic.board, pose(&automatic.board, "C1"), &corridor_board(DEFAULT)));
}

#[test]
fn corridor_is_enforced_by_shared_legality_and_named_in_rejections() {
    let board = corridor_board(DEFAULT);
    let placements = poses(&board);
    let inside = with_c1(&placements, 16.0, 10.0, BoardSide::Front);
    assert!(legal(&board, &placements, &options()));
    assert!(!legal(&board, &inside, &options()));
    let reasons = placement_rejection_reasons(&board, &inside).expect("reasons");
    assert!(reasons.iter().any(|r| r.contains("reserved-corridor:DN/DP")), "{reasons:?}");
    // The corridor is side-specific, like a hand-drawn keepout.
    assert!(legal(&board, &with_c1(&placements, 16.0, 10.0, BoardSide::Back), &options()));
    // Without the property, only the automatic pair corridor rejects the pose.
    let unreserved = corridor_board(Shape { reserve: false, ..DEFAULT });
    assert!(!legal(&unreserved, &inside, &options()));
    assert!(legal(&unreserved, &inside, &PlannerOptions { auto_pair_corridors: false, ..options() }));
}

#[test]
fn fixed_non_terminal_component_in_corridor_is_an_error() {
    let board = corridor_board(Shape { fixed_capacitor: Some(pt(16.0, 10.0)), ..DEFAULT });
    let error = plan_placement(&board, &options()).err().expect("an error").to_string();
    assert!(
        error.contains(
            "reserved corridor 'reserved-corridor:DN/DP' for differential pair DN/DP overlaps fixed component 'C1'"
        ),
        "{error}"
    );
}

#[test]
fn session_locked_component_in_corridor_is_an_error() {
    let mut board = corridor_board(DEFAULT);
    board.placements[2].position = pt(16.0, 10.0);
    let locked = PlannerOptions { fixed_references: ["C1".to_owned()].into_iter().collect(), ..options() };
    let error = plan_placement(&board, &locked).err().expect("an error").to_string();
    assert!(error.contains("overlaps fixed component 'C1'"), "{error}");
}

#[test]
fn corridor_is_skipped_with_reason_when_a_terminal_is_movable() {
    let board = corridor_board(Shape { fix_connector: false, ..DEFAULT });
    let (corridors, skipped) = corridor_reservation(&board).expect("reservation");
    assert!(corridors.is_empty() && skipped.len() == 1);
    let (nets, reason) = &skipped[0];
    assert_eq!(nets, &["DN", "DP"]);
    assert!(reason.contains("J1") && reason.contains("not fixed"), "{reason}");
    assert!(legal(&board, &with_c1(&poses(&board), 16.0, 10.0, BoardSide::Front), &options()));
    let plan = plan_placement(&board, &options()).expect("plan");
    let findings: Vec<_> = plan.report.findings.iter().filter(|f| f.code == "CORRIDOR_NOT_RESERVED").collect();
    assert!(findings.len() == 1 && findings[0].status == Status::Warning);
    assert_eq!(findings[0].subject.as_deref(), Some("DN/DP"));
    assert_eq!(plan.report.gates[0].status, Status::Warning);
    let document = plan.report.document();
    assert_eq!(document.get("reserved_corridors"), Some(&Value::Array(Vec::new())));
    let expected = copper_json::parse(&format!(
        r#"[{{"nets": ["DN", "DP"], "reason": {}}}]"#,
        copper_json::Dumps::default().dumps(&Value::from(reason.as_str()))
    ))
    .expect("JSON");
    assert_eq!(document.get("skipped_corridors"), Some(&expected));
}

#[test]
fn corridor_report_is_deterministic_and_reviewable() {
    let first = plan_placement(&corridor_board(DEFAULT), &options()).expect("plan").report.document();
    let second = plan_placement(&corridor_board(DEFAULT), &options()).expect("plan").report.document();
    assert_eq!(first, second);
    let corridors = first.get("reserved_corridors").and_then(Value::as_array).expect("corridors");
    assert_eq!(corridors.len(), 1);
    let corridor = &corridors[0];
    assert_eq!(corridor.get("name").and_then(Value::as_str), Some("reserved-corridor:DN/DP"));
    assert_eq!(corridor.get("nets"), Some(&Value::Array(vec![Value::from("DN"), Value::from("DP")])));
    assert_eq!(corridor.get("side").and_then(Value::as_str), Some("front"));
    assert_eq!(corridor.get("margin_nm"), Some(&Value::from(mm(0.55))));
    assert_eq!(corridor.get("terminal_components"), Some(&Value::Array(vec![Value::from("J1"), Value::from("U1")])));
    assert!(corridor.get("polygon_nm").and_then(Value::as_array).expect("polygon").len() >= 4);
    assert_eq!(first.get("skipped_corridors"), Some(&Value::Array(Vec::new())));
}

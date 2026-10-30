//! `tests/test_planned_crossings.py`: D-PHY plan R5, planned paired layer
//! swaps for crossing pairs of a bundle.

use copper_json::Value;
use copper_pcb::board::{
    ComponentPlacementRule, CopperZone, NetMatchGroup, NetRoutingRule, PadReference, PhysicalBoard, ReturnViaPolicy,
    Stackup, Via,
};
use copper_physical::CopperLayer::{self, Back as BACK, Front as FRONT, In1 as IN1, In2 as IN2, In3 as IN3, In4 as IN4};
use copper_physical::footprint::{FootprintPad, Size, ZoneConnection};
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::critical::{CriticalNetResult, CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus};
use crate::critical_bundles::{BUNDLE_REPAIR_LIMIT, CriticalBundle, crossing_line, plan_bundles};
use crate::test_support_critical::*;

const RETURN_DISTANCE: i64 = 1_500_000;

/// `_crossing_board`: one 0.2/0.2 mm pair per lane; package rows at 0.5 mm
/// pitch, connector rows at 0.4 mm. Surface keep-outs behind both rows mean
/// an inverted lane order cannot be wound around a component on F.Cu. Four
/// layers: In1.Cu is a GND plane, pairs may use F.Cu and In2.Cu and require
/// GND return vias within 1.5 mm.
pub(super) fn crossing_board(package: &[&str], connector: &[&str], names: &[&str], max_vias: Option<i64>) -> PhysicalBoard {
    let mut board = row_pair_board("PlannedCrossing", &rows(package, "0.5"), &rows(connector, "0.4"), names, |_, net, partner| {
        NetRoutingRule {
            priority: 100,
            allowed_layers: vec![FRONT, IN2],
            max_vias,
            require_return_vias: true,
            return_via_net: Some("GND".to_owned()),
            maximum_return_via_distance_nm: Some(RETURN_DISTANCE),
            ..differential_rule(net, partner, 200_000, 200_000)
        }
    });
    board.nets.push(net("GND", &[]));
    board.stackup = Stackup::new(vec![FRONT, IN1, IN2, BACK]);
    board.zones = vec![CopperZone {
        pad_connection: ZoneConnection::Solid,
        ..zone("ground", "GND", &[IN1], board.outline.vertices.clone())
    }];
    board.copper_keepouts = vec![
        front_wall("behind-package", 0, 0, 9 * MM, 20 * MM),
        front_wall("behind-connector", 21 * MM, 0, 30 * MM, 20 * MM),
    ];
    board
}

/// Two lanes whose order is inverted: A above B at U1, B above A at J1.
fn swapped(max_vias: Option<i64>) -> PhysicalBoard {
    crossing_board(&["-1.0", "1.0"], &["1.0", "-1.0"], &["LANE_A", "LANE_B"], max_vias)
}

/// Three lanes A B C at U1 and C B A at J1: every lane crosses every other.
fn reversed(six_layers: bool) -> PhysicalBoard {
    let mut board = crossing_board(&["-1.5", "0", "1.5"], &["1.5", "0", "-1.5"], &["LANE_A", "LANE_B", "LANE_C"], None);
    if six_layers {
        // In2.Cu references In1.Cu and In3.Cu references In4.Cu, both GND planes.
        board.stackup = Stackup::new(vec![FRONT, IN1, IN2, IN3, IN4, BACK]);
        let lower = CopperZone { id: "ground-lower".to_owned(), layers: vec![IN4], ..board.zones[0].clone() };
        board.zones.push(lower);
        map_rules(&mut board, |rule| NetRoutingRule { allowed_layers: vec![FRONT, IN2, IN3], ..rule });
    }
    board
}

fn map_rules(board: &mut PhysicalBoard, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    board.net_routing_rules = board.net_routing_rules.iter().cloned().map(change).collect();
}

fn shared_reference(mut board: PhysicalBoard) -> PhysicalBoard {
    map_rules(&mut board, |rule| NetRoutingRule {
        return_via_policy: ReturnViaPolicy::ReferenceChange,
        shared_reference_layer: Some(IN1),
        ..rule
    });
    board
}

fn breakout(mut board: PhysicalBoard, length_mm: i64) -> PhysicalBoard {
    map_rules(&mut board, |rule| NetRoutingRule {
        breakout_length_nm: Some(length_mm * MM),
        breakout_clearance_nm: Some(100_000),
        ..rule
    });
    board
}

pub(super) fn group(name: &str) -> Vec<String> {
    vec![format!("{name}_N"), format!("{name}_P")]
}

fn rules_of(board: &PhysicalBoard) -> IndexMap<&str, &NetRoutingRule> {
    board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect()
}

fn bundles(board: &PhysicalBoard, plan_crossings: bool) -> Vec<CriticalBundle> {
    let jobs = crate::critical::critical_jobs(board);
    plan_bundles(board, &jobs, &rules_of(board), BUNDLE_REPAIR_LIMIT, plan_crossings, true).expect("bundles")
}

fn bundle(board: &PhysicalBoard, plan_crossings: bool) -> CriticalBundle {
    let mut found = bundles(board, plan_crossings);
    assert_eq!(found.len(), 1);
    found.remove(0)
}

/// Hard (error) DRC codes other than incomplete routing.
fn hard(board: &PhysicalBoard) -> Vec<String> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("DRC");
    report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect()
}

fn route(board: &PhysicalBoard, pair_state_limit: Option<i64>) -> CriticalRoutingResult {
    let guides = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let options = CriticalOptions { pair_state_limit, ..CriticalOptions::default() };
    crate::critical::route_critical_nets(board.clone(), &guides, &options, None).expect("critical routing")
}

fn result<'r>(result: &'r CriticalRoutingResult, name: &str) -> &'r CriticalNetResult {
    result.nets.iter().find(|item| item.nets == group(name)).expect("the pair's result")
}

fn order(result: &CriticalRoutingResult) -> Vec<Vec<String>> {
    result.nets.iter().map(|item| item.nets.clone()).collect()
}

/// Exactly one paired swap: two vias per member, matched, surface -> layer -> surface.
fn assert_one_paired_swap(result: &CriticalRoutingResult, name: &str, layer: CopperLayer) {
    let pair = self::result(result, name);
    assert!(pair.connected && pair.strategy == "planned_crossing", "{name}: {pair:?}");
    assert_eq!(pair.pair_search_order.as_deref(), Some("planned_crossing"));
    assert_eq!(pair.paired_via_transitions, 2);
    for net in group(name) {
        assert_eq!(result.board.vias.iter().filter(|via| via.net == net).count(), 2, "{net}");
        let mut layers: Vec<CopperLayer> =
            result.board.tracks.iter().filter(|t| t.net == net).map(|t| t.layer).collect();
        layers.sort();
        layers.dedup();
        let mut expected = vec![FRONT, layer];
        expected.sort();
        assert_eq!(layers, expected, "{net}");
    }
}

fn json(result: &CriticalRoutingResult) -> Value {
    copper_json::parse(&result.to_json().expect("JSON")).expect("parsed")
}

fn bundle_documents(result: &CriticalRoutingResult) -> Vec<copper_json::Object> {
    let Value::Object(document) = json(result) else { panic!("object") };
    let Value::Array(bundles) = &document["bundles"] else { panic!("bundles") };
    bundles.iter().map(|item| if let Value::Object(item) = item { item.clone() } else { panic!("bundle") }).collect()
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn distance(a: copper_physical::Point, b: copper_physical::Point) -> f64 {
    ((a.x_nm - b.x_nm) as f64).hypot((a.y_nm - b.y_nm) as f64)
}

#[test]
fn inverted_lane_order_plans_the_fewest_crossings_on_referenced_layers() {
    let planned = bundle(&swapped(None), true);
    assert_eq!(planned.crossings.len(), 1);
    let crossing = &planned.crossings[0];
    // Both lanes are outermost; the inner one in bundle order (LANE_A) stays.
    assert_eq!(crossing.group, group("LANE_B"));
    assert_eq!(crossing.crosses, vec![group("LANE_A")]);
    assert_eq!((crossing.surface, crossing.layer, crossing.reference_planes.clone()), (FRONT, Some(IN2), vec![IN1]));
    assert_eq!(crossing.status, "planned");
    assert_eq!(planned.order, vec![group("LANE_B"), group("LANE_A")]);
    // Without planning the bundle keeps its R6 order and plans nothing.
    let plain = bundle(&swapped(None), false);
    assert!(plain.crossings.is_empty());
    assert_eq!(plain.order, planned.order);

    // Every lane of a full reversal crosses every other: the middle lane
    // stays, the two outer lanes cross each other too, so they need two layers.
    let six = bundle(&reversed(true), true);
    let [outer, inner] = &six.crossings[..] else { panic!("two crossings: {:?}", six.crossings) };
    assert_eq!((&outer.group, outer.layer, &outer.reference_planes), (&group("LANE_C"), Some(IN2), &vec![IN1]));
    assert_eq!((&inner.group, inner.layer, &inner.reference_planes), (&group("LANE_A"), Some(IN3), &vec![IN4]));
    assert_eq!(outer.crosses, vec![group("LANE_A"), group("LANE_B")]);
    let four = bundle(&reversed(false), true);
    let [planned, impossible] = &four.crossings[..] else { panic!("two crossings") };
    assert_eq!(planned.layer, Some(IN2));
    assert!(impossible.layer.is_none() && impossible.status == "impossible");
    assert_eq!(impossible.reason, "it crosses moving pairs on In2.Cu and no other allowed layer is left");

    // Aligned rows, and rows that only change pitch, need no crossing.
    let aligned = crossing_board(&["-1.0", "1.0"], &["-1.0", "1.0"], &["LANE_A", "LANE_B"], None);
    assert!(bundle(&aligned, true).crossings.is_empty());
    let spread = crossing_board(&["-1.0", "1.0"], &["-2.0", "3.0"], &["LANE_A", "LANE_B"], None);
    assert!(bundle(&spread, true).crossings.is_empty());
}

#[test]
fn the_pair_that_can_swap_layers_is_the_one_moved() {
    let mut board = swapped(None);
    // LANE_B (otherwise moved) may not use vias: LANE_A moves instead.
    map_rules(&mut board, |rule| {
        if rule.net.starts_with("LANE_B") { NetRoutingRule { max_vias: Some(0), ..rule } } else { rule }
    });
    let planned = bundle(&board, true);
    assert_eq!(planned.crossings.len(), 1);
    assert_eq!(planned.crossings[0].group, group("LANE_A"));
    assert_eq!(planned.crossings[0].layer, Some(IN2));
}

#[test]
fn crossing_pair_routes_with_one_planned_paired_swap_and_return_vias() {
    // Python first routed with `plan_crossings=False` and saw the surface-first
    // search find a legal recessed via escape; `route_critical_nets` always
    // plans crossings now, so only the planned route is specified.
    let board = swapped(None);
    let result = route(&board, Some(2000));
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    assert_eq!(order(&result), vec![group("LANE_B"), group("LANE_A")]);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    let surface = self::result(&result, "LANE_A");
    assert!(surface.connected && surface.via_count == 0);
    assert!(result.board.tracks.iter().filter(|t| t.net.starts_with("LANE_A")).all(|t| t.layer == FRONT));
    assert_eq!(hard(&result.board), Vec::<String>::new());

    let [crossing] = &result.bundles[0].crossings[..] else { panic!("one crossing") };
    assert!(crossing.status == "routed" && crossing.reason.is_empty(), "{crossing:?}");
    let components: Vec<&str> = crossing.transitions.iter().map(|item| item.component.as_str()).collect();
    assert_eq!(components, ["J1", "U1"]);
    let rules = rules_of(&board);
    let spacing = crate::pair_vias::transition_spacing(&board, rules["LANE_B_N"], rules["LANE_B_P"]);
    for transition in &crossing.transitions {
        let nets: Vec<&str> = transition.vias.iter().map(|(net, _)| net.as_str()).collect();
        assert_eq!(nets, ["LANE_B_N", "LANE_B_P"]);
        let (a, b) = (transition.vias[0].1, transition.vias[1].1);
        assert!((distance(a, b) - spacing as f64).abs() <= 4.0);
        assert!(transition.reference == "return_vias" && transition.shared_reference_layer.is_none());
        let [returned] = transition.return_vias[..] else { panic!("one return via") };
        assert!(distance(returned, a).max(distance(returned, b)) <= RETURN_DISTANCE as f64);
        assert!(result.board.vias.iter().any(|via| via.net == "GND" && via.position == returned));
    }
    let document = &bundle_documents(&result)[0];
    let Value::Array(crossings) = &document["crossings"] else { panic!("crossings") };
    let Value::Object(first) = &crossings[0] else { panic!("crossing") };
    assert_eq!(first["group"], texts(&group("LANE_B")));
    assert_eq!(first["layer"], Value::from("In2.Cu"));
    assert_eq!(first["reference_planes"], texts(&["In1.Cu".to_owned()]));
    assert_eq!(first["status"], Value::from("routed"));
    let Value::Array(transitions) = &first["transitions"] else { panic!("transitions") };
    let Value::Object(second) = &transitions[1] else { panic!("transition") };
    let mut vias = copper_json::Object::new();
    for (net, point) in &crossing.transitions[1].vias {
        vias.insert(net.clone(), Value::Array(vec![Value::from(point.x_nm), Value::from(point.y_nm)]));
    }
    assert_eq!(second["vias"], Value::Object(vias));
}

#[test]
fn full_reversal_gives_each_crossing_pair_its_own_layer() {
    let result = route(&reversed(true), None);
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    assert_eq!(order(&result), vec![group("LANE_C"), group("LANE_A"), group("LANE_B")]);
    assert_one_paired_swap(&result, "LANE_C", IN2);
    assert_one_paired_swap(&result, "LANE_A", IN3);
    assert_eq!(self::result(&result, "LANE_B").via_count, 0);
    let statuses: Vec<&str> = result.bundles[0].crossings.iter().map(|item| item.status.as_str()).collect();
    assert_eq!(statuses, ["routed", "routed"]);
    assert_eq!(hard(&result.board), Vec::<String>::new());

    // With one crossing layer the second crossing pair is impossible: it is
    // reported and fails without any search, and the rest still route.
    let limited = route(&reversed(false), None);
    let impossible = self::result(&limited, "LANE_A");
    assert!(!impossible.connected && impossible.search_states == 0 && impossible.pair_searches == 0);
    assert_eq!(
        impossible.diagnostics,
        ["planned crossing is impossible: it crosses moving pairs on In2.Cu and no other allowed layer is left"]
    );
    assert!(self::result(&limited, "LANE_C").connected && self::result(&limited, "LANE_B").connected);
    assert!(!limited.board.tracks.iter().any(|t| t.net.starts_with("LANE_A")));
    let statuses: Vec<&str> = limited.bundles[0].crossings.iter().map(|item| item.status.as_str()).collect();
    assert_eq!(statuses, ["routed", "impossible"]);
    assert_eq!(limited.status, CriticalRoutingStatus::Failed);
}

/// `_crossing_beside`: lanes 0.45 mm apart outside 2 mm breakouts, too close
/// for a via pair. Only `moving` may change layer.
fn crossing_beside(package: &[&str], connector: &[&str], names: &[&str], moving: &str) -> PhysicalBoard {
    let mut board = crossing_board(package, connector, names, None);
    map_rules(&mut board, |rule| NetRoutingRule {
        clearance_nm: Some(450_000),
        breakout_length_nm: Some(2 * MM),
        breakout_clearance_nm: Some(100_000),
        max_vias: Some(if rule.net.starts_with(moving) { 2 } else { 0 }),
        ..rule
    });
    board
}

#[test]
fn a_crossing_bundle_routes_from_its_moving_pairs_outward() {
    // Four lanes A B C D at U1; C and D swap at J1. The moving pair comes
    // first, then the pair it crosses, then B before the outer A.
    let board = crossing_beside(
        &["-1.5", "-0.5", "0.5", "1.5"],
        &["-2.25", "-0.75", "2.25", "0.75"],
        &["LANE_A", "LANE_B", "LANE_C", "LANE_D"],
        "LANE_C",
    );
    let planned = bundle(&board, true);
    let crossings: Vec<_> = planned.crossings.iter().map(|item| (item.group.clone(), item.crosses.clone())).collect();
    assert_eq!(crossings, vec![(group("LANE_C"), vec![group("LANE_D")])]);
    assert_eq!(planned.order, vec![group("LANE_C"), group("LANE_D"), group("LANE_B"), group("LANE_A")]);
    // Without crossings the order stays outside-in.
    assert_eq!(bundle(&board, false).order[..2], [group("LANE_A"), group("LANE_D")]);
}

#[test]
fn the_crossed_pair_steps_aside_for_wide_transition_vias_without_repair() {
    // B and C swap. C's transition vias (0.8 mm, 1.0 mm apart) are wider
    // than the 1 mm lane pitch with 0.45 mm clearance, so B must step aside
    // for them. Routed before the outer A, it can.
    let board = crossing_beside(&["-1", "0", "1"], &["-1.5", "1.5", "0"], &["LANE_A", "LANE_B", "LANE_C"], "LANE_C");
    let result = route(&board, None);
    assert_one_paired_swap(&result, "LANE_C", IN2);
    assert_eq!(order(&result), vec![group("LANE_C"), group("LANE_B"), group("LANE_A")]);
    assert!(result.nets.iter().all(|item| item.connected));
    assert_eq!(result.bundles.len(), 1);
    assert!(result.bundles[0].repairs.is_empty());
    assert_eq!(hard(&result.board), Vec::<String>::new());
}

#[test]
fn a_via_budget_below_two_reports_the_crossing_as_impossible() {
    for max_vias in [0, 1] {
        let result = route(&swapped(Some(max_vias)), None);
        let [crossing] = &result.bundles[0].crossings[..] else { panic!("one crossing") };
        let reason = format!("max_vias {max_vias} on LANE_B_N allows no paired layer swap; it needs 2 vias per member");
        assert_eq!((crossing.status.as_str(), crossing.layer, crossing.reason.as_str()), ("impossible", None, reason.as_str()));
        let pair = self::result(&result, "LANE_B");
        // No silent fallback: no coarse candidate, no surface search, no copper.
        assert!(!pair.connected);
        assert_eq!(pair.diagnostics, [format!("planned crossing is impossible: {reason}")]);
        assert!(pair.search_states == 0 && pair.pair_searches == 0 && pair.track_count == 0);
        assert!(!result.board.tracks.iter().any(|t| t.net.starts_with("LANE_B")));
        assert!(!result.board.vias.iter().any(|v| v.net.starts_with("LANE_B")));
        assert!(self::result(&result, "LANE_A").connected);
        assert_eq!(result.status, CriticalRoutingStatus::Failed);
        let document = &bundle_documents(&result)[0];
        let mut expected = copper_json::Object::new();
        // The document is written with sorted keys.
        expected.insert("crosses".to_owned(), Value::Array(vec![texts(&group("LANE_A"))]));
        expected.insert("group".to_owned(), texts(&group("LANE_B")));
        expected.insert("layer".to_owned(), Value::Null);
        expected.insert("reason".to_owned(), Value::from(reason.as_str()));
        expected.insert("reference_planes".to_owned(), Value::Array(Vec::new()));
        expected.insert("status".to_owned(), Value::from("impossible"));
        expected.insert("surface".to_owned(), Value::from("F.Cu"));
        expected.insert("transitions".to_owned(), Value::Array(Vec::new()));
        assert_eq!(document["crossings"], Value::Array(vec![Value::Object(expected)]));
        assert_eq!(
            crossing_line(&result.bundles[0], crossing),
            format!("bundle J1-U1 crossing LANE_B_N/LANE_B_P under LANE_A_N/LANE_A_P: impossible ({reason})")
        );
    }
}

#[test]
fn a_declared_shared_reference_replaces_the_return_vias() {
    let result = route(&shared_reference(swapped(None)), None);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    assert!(!result.board.vias.iter().any(|via| via.net == "GND"));
    assert_eq!(self::result(&result, "LANE_B").shared_reference_transition_count, 2);
    let [crossing] = &result.bundles[0].crossings[..] else { panic!("one crossing") };
    for item in &crossing.transitions {
        assert_eq!((item.reference.as_str(), item.shared_reference_layer, item.return_vias.len()), ("shared_reference", Some(IN1), 0));
    }
    assert_eq!(crossing.transitions.len(), 2);
    assert_eq!(hard(&result.board), Vec::<String>::new());
}

#[test]
fn crossing_transitions_stay_outside_breakout_regions() {
    let board = breakout(swapped(None), 2);
    let rules = rules_of(&board);
    // Members of a breakout pair are spaced by the pair gap (plan R1), so are
    // their transition vias: 0.8 mm vias 0.2 mm apart.
    assert_eq!(crate::pair_vias::transition_spacing(&board, rules["LANE_B_N"], rules["LANE_B_P"]), MM + 4);
    let result = route(&board, None);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    let regions = copper_drc::breakout::BreakoutRegions::new(&board);
    let crossing_vias: Vec<&Via> = result.board.vias.iter().filter(|via| via.net != "LANE_A").collect();
    assert_eq!(crossing_vias.len(), 6);
    assert!(!crossing_vias.iter().any(|via| regions.inside_any(via.position)));
    assert_eq!(hard(&result.board), Vec::<String>::new());
}

#[test]
fn crossing_collars_widen_to_via_spacing_beyond_flanking_ground_lands() {
    // Ground lands sit 0.4 mm either side of the moving pair's connector lands,
    // as a connector's ground pins do. Its transition vias need 1.0 mm
    // spacing; the collar must leave the lands straight and widen further out.
    let mut board = breakout(swapped(None), 2);
    let connector = board.footprints.get_mut("test/connector-row").expect("connector");
    for (number, y) in [("G1", -1_600_000), ("G2", -400_000)] {
        connector.pads.push(FootprintPad {
            size: Size { width_nm: 600_000, height_nm: 200_000 },
            ..smd_pad(number, at(-1_400_000, y), 600_000)
        });
    }
    for net in board.nets.iter_mut().filter(|net| net.name == "GND") {
        net.pads = vec![PadReference::new("J1", "G1"), PadReference::new("J1", "G2")];
    }
    let result = route(&board, None);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    let regions = copper_drc::breakout::BreakoutRegions::new(&board);
    let crossing_vias: Vec<&Via> = result.board.vias.iter().filter(|via| via.net.starts_with("LANE_B")).collect();
    assert!(!crossing_vias.is_empty() && !crossing_vias.iter().any(|via| regions.inside_any(via.position)));
    // The ground lands wait for plane stitching; nothing else may be open.
    let report = crate::drc_report::run_physical_drc(&result.board, copper_drc::Policy::default()).expect("DRC");
    let errors: Vec<(String, Vec<String>)> = report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| (f.code.clone(), f.nets.clone()))
        .collect();
    assert_eq!(errors, vec![("DRC-OPEN-NET".to_owned(), vec!["GND".to_owned()])]);
}

#[test]
fn crossing_falls_back_to_transitions_inside_breakout_regions() {
    // Breakouts long enough to cover the whole board leave no site outside
    // every region; the crossing pair may then change layer inside one.
    let board = breakout(swapped(None), 15);
    let regions = copper_drc::breakout::BreakoutRegions::new(&board);
    assert!((0..31).all(|x| (0..21).all(|y| regions.inside_any(mm(x, y)))));
    let result = route(&board, None);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    assert_eq!(hard(&result.board), Vec::<String>::new());
}

#[test]
fn transition_sites_skip_breakout_regions_and_prefer_the_reserved_corridor() {
    let mut board = swapped(None);
    let zero = Decimal::zero();
    board.placement_rules = [("U1", mm(8, 10)), ("J1", mm(22, 10))]
        .into_iter()
        .map(|(reference, position)| ComponentPlacementRule {
            fixed_position: Some(position),
            fixed_rotation_degrees: Some(zero.clone()),
            allowed_orientations: vec![zero.clone()],
            ..ComponentPlacementRule::new(reference)
        })
        .collect();
    map_rules(&mut board, |rule| NetRoutingRule {
        reserve_corridor: true,
        breakout_length_nm: Some(MM),
        breakout_clearance_nm: Some(100_000),
        ..rule
    });
    let lane = group("LANE_B");
    let rank = crate::critical::crossing_site_rank(&board, &lane).expect("rank");
    let vias = |centres: &[(i64, i64)]| -> Vec<Via> {
        centres
            .iter()
            .map(|(x, y)| Via {
                net: "LANE_B_N".to_owned(),
                position: at(*x, *y),
                size_nm: 800_000,
                drill_nm: 400_000,
                from_layer: FRONT,
                to_layer: BACK,
                technology: None,
                finish: "standard".to_owned(),
            })
            .collect()
    };
    // LANE_B runs from (9.4, 11.0) at U1 to (20.6, 9.0) at J1.
    assert_eq!(rank(&vias(&[(15 * MM, 10 * MM), (15 * MM, 10_200_000)])), Some(0), "inside its corridor");
    assert_eq!(rank(&vias(&[(15 * MM, 10 * MM), (15 * MM, 13 * MM)])), Some(1), "one via outside it");
    assert_eq!(rank(&vias(&[(10_200_000, 11 * MM), (15 * MM, 10 * MM)])), Some(2), "in a land's region");
    assert_eq!(rank(&vias(&[(10_200_000, 9 * MM), (15 * MM, 10 * MM)])), Some(2), "another net's region");
    let plain_board = swapped(None);
    let plain = crate::critical::crossing_site_rank(&plain_board, &lane).expect("rank");
    assert_eq!(plain(&vias(&[(15 * MM, 13 * MM), (10_200_000, 11 * MM)])), Some(0));
}

#[test]
fn bundle_repair_reroutes_a_crossing_pair_with_its_plan() {
    let board = swapped(None);
    let global = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let routes = guides_by_net(&global);
    let rules = rules_of(&board);
    let routed = route(&board, None);
    let [crossing] = &routed.bundles[0].crossings[..] else { panic!("one crossing") };
    let surface: Vec<_> = routed.board.tracks.iter().filter(|t| t.net.starts_with("LANE_A")).cloned().collect();
    // Rip up the surface pair, route the crossing pair first, then the surface pair.
    let committed = vec![(group("LANE_A"), surface, Vec::new())];
    let plan = crate::critical_bundles::PlannedCrossing {
        status: "planned".to_owned(),
        transitions: Vec::new(),
        ..crossing.clone()
    };
    let crossings = IndexMap::from([(group("LANE_B"), plan)]);
    let (failed, blocker) = (group("LANE_B"), group("LANE_A"));
    let (nested, mut turns) = (IndexMap::new(), IndexMap::new());
    let (record, repaired) = crate::critical::bundle_repair(
        &board,
        &routes,
        &rules,
        None,
        (&[], &[]),
        &committed,
        (rules["LANE_B_N"], rules["LANE_B_P"]),
        &failed,
        &blocker,
        (&crossings, &nested, &mut turns),
        1,
    )
    .expect("repair");
    assert!(record.accepted, "{record:?}");
    let ((crossed, _, crossed_vias), (rerouted, _, _)) = repaired.expect("repaired pairs");
    assert!(crossed.strategy == "planned_crossing" && crossed.paired_via_transitions == 2, "{crossed:?}");
    assert_eq!(crossed_vias.iter().filter(|via| group("LANE_B").contains(&via.net)).count(), 4);
    assert!(rerouted.connected && rerouted.via_count == 0);
}

#[test]
fn bundles_without_crossings_route_byte_identically() {
    // Python compared `plan_crossings=True` and `False` routes; the router
    // always plans now, so the planner itself must add nothing, and the
    // bundle document carries no `crossings` key.
    for board in [
        super::critical_bundles::blocked_middle_board(),
        crossing_board(&["-1.0", "1.0"], &["-1.0", "1.0"], &["LANE_A", "LANE_B"], None),
    ] {
        let planned = bundles(&board, true);
        let plain = bundles(&board, false);
        assert_eq!(planned, plain);
        let result = route(&board, None);
        assert!(bundle_documents(&result).iter().all(|item| !item.contains_key("crossings")));
    }
}

#[test]
fn planned_crossings_are_deterministic() {
    let board = reversed(true);
    let (first, second) = (route(&board, None), route(&board, None));
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert_eq!(first.board, second.board);
    assert_eq!(first.bundles, second.bundles);
}

#[test]
fn a_length_match_group_with_a_crossing_pair_is_tuned() {
    // The crossing pair's two via paths to In2.Cu count toward its length,
    // so the straight pair needs taller bumps than the crossing alone needs.
    let mut board = shared_reference(swapped(None));
    let nets: Vec<String> = board.nets.iter().filter(|net| net.name != "GND").map(|net| net.name.clone()).collect();
    board.match_groups = vec![NetMatchGroup { id: "lanes".to_owned(), nets, max_skew_nm: 100_000 }];
    map_rules(&mut board, |rule| NetRoutingRule { tuning_amplitude_limit_nm: Some(1_200_000), ..rule });
    let result = route(&board, None);
    let [Value::Object(tuning)] = &result.match_tuning[..] else { panic!("one tuning record") };
    let int = |key: &str| match &tuning[key] {
        Value::Int(value) => i64::try_from(value.clone()).expect("i64"),
        other => panic!("{key}: {other:?}"),
    };
    assert_eq!(tuning["status"], Value::from("tuned"));
    assert!(int("skew_before_nm") > 100_000);
    assert!(int("skew_after_nm") <= 100_000);
    assert_one_paired_swap(&result, "LANE_B", IN2);
    assert_eq!(result.bundles[0].crossings[0].status, "routed");
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    assert_eq!(hard(&result.board), Vec::<String>::new());
}

#[test]
fn preflight_prints_each_planned_crossing() {
    // Python ran the preflight CLI on a patched router; the printed line is
    // `crossing_line`, checked here on the routed crossing.
    let routed = route(&swapped(None), None);
    let [crossing] = &routed.bundles[0].crossings[..] else { panic!("one crossing") };
    assert_eq!(crossing.status, "routed");
    let mm_text = |points: &[copper_physical::Point]| {
        let parts: Vec<String> = points
            .iter()
            .map(|p| format!("({:.3}, {:.3})", p.x_nm as f64 / 1e6, p.y_nm as f64 / 1e6))
            .collect();
        format!("{} mm", parts.join("/"))
    };
    let sites: Vec<(String, String)> = crossing
        .transitions
        .iter()
        .map(|item| {
            let vias: Vec<_> = item.vias.iter().map(|(_, point)| *point).collect();
            (mm_text(&vias), mm_text(&item.return_vias))
        })
        .collect();
    assert_eq!(
        crossing_line(&routed.bundles[0], crossing),
        format!(
            "bundle J1-U1 crossing LANE_B_N/LANE_B_P under LANE_A_N/LANE_A_P: routed, F.Cu -> In2.Cu -> F.Cu \
             (reference planes In1.Cu); J1 vias {}, return vias {}; U1 vias {}, return vias {}",
            sites[0].0, sites[0].1, sites[1].0, sites[1].1
        )
    );
}

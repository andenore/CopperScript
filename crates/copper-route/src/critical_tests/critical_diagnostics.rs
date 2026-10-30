//! `tests/test_critical_diagnostics.py`: D-PHY plan W1, rejection reasons,
//! coarse transitions and the lane table.

use copper_json::Value;
use copper_lang::mechanical::stackup::StackupLayer;
use copper_pcb::board::{ComponentPlacementRule, NetRoutingRule, PhysicalBoard, RouteKind, Stackup, Via};
use copper_physical::CopperLayer::{self, Back as BACK, Front as FRONT, In1 as IN1, In2 as IN2};
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::critical::{
    CriticalNetResult, CriticalOptions, CriticalRoutingResult, budget_diagnostics, budget_warnings,
    critical_lane_table, reject_reserved_plane_tracks, rejection_gate, rejection_summary, route_critical_nets,
    route_pair, route_pair_exact,
};
use crate::routing::GlobalRoutingResult;
use crate::test_support_critical::*;

/// `_bent_pair_board`: J2 rotated a quarter turn at (20, 20) mm, so the
/// joint lanes bend and differ in length.
fn bent_pair_board(change: impl Fn(NetRoutingRule) -> NetRoutingRule) -> PhysicalBoard {
    let mut board = usb_pair_board();
    let quarter = Decimal::from_i64(90);
    board.placements[1].position = mm(20, 20);
    board.placements[1].rotation_degrees = quarter.clone();
    board.placement_rules =
        vec![ComponentPlacementRule { allowed_orientations: vec![quarter], ..ComponentPlacementRule::new("J2") }];
    board.net_routing_rules = board
        .net_routing_rules
        .iter()
        .cloned()
        .map(|rule| change(NetRoutingRule { maximum_uncoupled_length_nm: Some(10 * MM), ..rule }))
        .collect();
    board
}

fn coarse(board: &PhysicalBoard) -> GlobalRoutingResult {
    global_route(board, 2_500_000)
}

fn routed(board: &PhysicalBoard, global: &GlobalRoutingResult, limit: Option<i64>) -> CriticalRoutingResult {
    let options = CriticalOptions { pair_state_limit: limit, ..CriticalOptions::default() };
    route_critical_nets(board.clone(), global, &options, None).expect("critical routing")
}

fn gates(items: &[String]) -> Vec<String> {
    items.iter().map(|item| rejection_gate(std::slice::from_ref(item)).0).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

// D1 ---------------------------------------------------------------------------

#[test]
fn every_candidate_gate_has_a_named_first_failing_reason() {
    let rule = NetRoutingRule {
        max_vias: Some(0),
        max_length_nm: Some(MM),
        allowed_layers: vec![FRONT],
        ..rule("A", RouteKind::Critical)
    };
    let back = track("A", mm(0, 0), mm(5, 0), 250_000, BACK);
    let via = Via { from_layer: FRONT, to_layer: BACK, ..through_via("A", mm(5, 0), 800_000, 400_000) };
    let [vias] = &budget_diagnostics(&rule, &[&via])[..] else { panic!("one via diagnostic") };
    // A length miss warns; it is not a hard gate.
    let [length] = &budget_warnings(&rule, &[&back])[..] else { panic!("one length warning") };
    assert_eq!(rejection_gate(std::slice::from_ref(length)).0, "length_budget");
    assert_eq!(rejection_gate(std::slice::from_ref(vias)).0, "via_budget");
    let board = usb_pair_board();
    let empty = CriticalNetResult::new(vec!["A".to_owned()], true, 1, 0, Vec::new(), 0, Vec::new());
    let rules = IndexMap::from([("A", &rule)]);
    let (plane, _, _) = reject_reserved_plane_tracks(&board, empty, vec![back.clone()], Vec::new(), &rules);
    assert_eq!(rejection_gate(&plane.diagnostics), ("plane_reservation".to_owned(), plane.diagnostics[0].clone()));

    let mut sorted = board.net_routing_rules.clone();
    sorted.sort_by(|a, b| a.net.cmp(&b.net));
    let (first, second) = (&sorted[0], &sorted[1]);
    let tight: Vec<NetRoutingRule> = [first, second]
        .iter()
        .map(|item| NetRoutingRule {
            max_skew_nm: Some(100_000),
            maximum_uncoupled_length_nm: Some(MM),
            ..(*item).clone()
        })
        .collect();
    let lanes = (
        vec![track(&first.net, at(5 * MM, 12_500_000), at(30 * MM, 12_500_000), 250_000, FRONT)],
        vec![track(&second.net, at(5 * MM, 11_500_000), at(35 * MM, 11_500_000), 250_000, FRONT)],
    );
    let guides = guides_by_net(&coarse(&board));
    let exact = |first: &NetRoutingRule, second: &NetRoutingRule, pairs: &[(Via, Via)]| {
        route_pair_exact(&board, first, second, &guides, &lanes, &board, (pairs, &[])).expect("exact pair").0
    };
    let skewed = exact(&tight[0], &tight[1], &[]);
    // Skew is a review warning; the uncoupled length stays a hard gate.
    assert_eq!(gates(&skewed.warnings), ["skew"]);
    assert_eq!(gates(&skewed.diagnostics), ["uncoupled_length"]);
    let pair = [(Via { net: first.net.clone(), ..via.clone() }, Via { net: second.net.clone(), ..via.clone() })];
    let unpaired = exact(first, second, &pair);
    assert!(gates(&unpaired.diagnostics).contains(&"via_pairing".to_owned()), "{:?}", unpaired.diagnostics);
    let returns: Vec<NetRoutingRule> = [first, second]
        .iter()
        .map(|item| NetRoutingRule {
            require_return_vias: true,
            return_via_net: Some("GND".to_owned()),
            maximum_return_via_distance_nm: Some(MM),
            ..(*item).clone()
        })
        .collect();
    let missing = exact(&returns[0], &returns[1], &pair);
    assert!(gates(&missing.diagnostics).contains(&"return_via".to_owned()), "{:?}", missing.diagnostics);

    // Native DRC codes are reported verbatim. An open pair net is the
    // connectivity gate, reported only when no geometric gate failed.
    let clearance = "DRC-CLEARANCE: tracks 1 and 2".to_owned();
    assert_eq!(rejection_gate(std::slice::from_ref(&clearance)), ("DRC-CLEARANCE".to_owned(), clearance.clone()));
    assert_eq!(rejection_gate(&strings(&["DRC-OPEN-NET: A is open", "DRC-SHORT: tracks 3 and 4"])).0, "DRC-SHORT");
    assert_eq!(
        rejection_gate(&strings(&["DRC-OPEN-NET: A is open"])),
        ("connectivity".to_owned(), "DRC-OPEN-NET: A is open".to_owned())
    );
    assert_eq!(rejection_gate(&strings(&["hard-macro copper cannot be modified"])).0, "hard_macro");
}

#[test]
fn rejection_summary_is_counted_ordered_and_bounded() {
    let pairs = |items: &[(&str, &str)]| -> Vec<(String, String)> {
        items.iter().map(|(gate, message)| (gate.to_string(), message.to_string())).collect()
    };
    let rejected = pairs(&[
        ("skew", "pair skew 5 nm exceeds 1 nm"),
        ("DRC-CLEARANCE", "DRC-CLEARANCE: a"),
        ("DRC-CLEARANCE", "DRC-CLEARANCE: b"),
        ("connectivity", "DRC-OPEN-NET: x"),
        ("DRC-CLEARANCE", "DRC-CLEARANCE: a"),
        ("skew", "pair skew 6 nm exceeds 1 nm"),
    ]);
    let (summary, examples) = rejection_summary(&rejected);
    let counted =
        |items: &[(&str, i64)]| -> Vec<(String, i64)> { items.iter().map(|(g, n)| (g.to_string(), *n)).collect() };
    assert_eq!(summary, counted(&[("DRC-CLEARANCE", 3), ("skew", 2), ("connectivity", 1)]));
    // One example per gate first, in summary order; never more than three.
    assert_eq!(examples, strings(&["DRC-CLEARANCE: a", "pair skew 5 nm exceeds 1 nm", "DRC-OPEN-NET: x"]));
    assert_eq!(rejection_summary(&rejected), (summary, examples));
    let single =
        rejection_summary(&pairs(&[("skew", "s1"), ("skew", "s2"), ("skew", "s1"), ("skew", "s3"), ("skew", "s4")]));
    assert_eq!(single, (counted(&[("skew", 5)]), strings(&["s1", "s2", "s3"])));
    assert_eq!(rejection_summary(&[]), (Vec::new(), Vec::new()));
}

#[derive(Default)]
struct Events(Vec<(String, String, Vec<(&'static str, Value)>)>);

impl crate::Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        self.0.push((phase.to_owned(), event.to_owned(), details));
    }
}

fn counts(items: &[(String, i64)]) -> Value {
    let mut object = copper_json::Object::new();
    for (gate, count) in items {
        object.insert(gate.clone(), Value::from(*count));
    }
    Value::Object(object)
}

#[test]
fn rejected_exact_pair_candidates_are_counted_in_report_and_progress() {
    let board = bent_pair_board(|rule| NetRoutingRule { max_skew_nm: Some(100_000), ..rule });
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let mut events = Events::default();
    let options = CriticalOptions { pair_state_limit: Some(2000), ..CriticalOptions::default() };
    let result = route_critical_nets(board.clone(), &guides, &options, Some(&mut events)).expect("routed");
    let pair = &result.nets[0];
    // Without tuning every exact candidate misses the skew budget: the search
    // counts each, and keeps the best one connected with a skew warning.
    assert!(pair.connected && pair.candidate_attempts > 0 && pair.diagnostics.is_empty(), "{pair:?}");
    assert!(pair.warnings.iter().any(|warning| warning.starts_with("pair skew ")));
    // Candidates that also fail a hard gate (uncoupled length) count under it.
    let gates: IndexMap<&str, i64> = pair.rejections.iter().map(|(gate, count)| (gate.as_str(), *count)).collect();
    assert_eq!(gates.values().sum::<i64>(), pair.candidate_attempts);
    assert!(gates.get("skew").copied().unwrap_or(0) > 0);
    assert!(gates.keys().all(|gate| ["skew", "uncoupled_length"].contains(gate)), "{gates:?}");
    assert!((1..=3).contains(&pair.rejection_examples.len()));
    assert!(pair.rejection_examples[0].starts_with("pair skew "));
    let Ok(Value::Object(document)) = copper_json::parse(&result.to_json().unwrap()) else { panic!("document") };
    let Value::Array(nets) = &document["nets"] else { panic!("nets") };
    let Value::Object(net) = &nets[0] else { panic!("net") };
    assert_eq!(net["rejections"], counts(&pair.rejections));
    let examples = Value::Array(pair.rejection_examples.iter().map(|e| Value::from(e.as_str())).collect());
    assert_eq!(net["rejection_examples"], examples);
    let finished = &events
        .0
        .iter()
        .rfind(|(phase, event, _)| phase == "critical_group" && event == "finished")
        .expect("finished")
        .2;
    let reported = finished.iter().find(|(key, _)| *key == "rejections").expect("rejections");
    assert_eq!(reported.1, counts(&pair.rejections));
    let again = routed(&board, &guides, Some(2000));
    assert_eq!(again.to_json().unwrap(), result.to_json().unwrap());
}

#[test]
fn drc_rejections_are_recorded_even_when_a_later_candidate_is_accepted() {
    // One 45-degree-cornered bump of at most 0.5 mm compensates the bend.
    let mut board = bent_pair_board(|rule| NetRoutingRule {
        max_skew_nm: Some(100_000),
        tuning_amplitude_limit_nm: Some(500_000),
        ..rule
    });
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    // A keep-out exactly where the surface candidates place their skew bump.
    board.copper_keepouts = vec![front_wall("bump-site", 6_300_000, 10_900_000, 7_500_000, 11_450_000)];
    let result = routed(&board, &guides, Some(2000));
    let pair = &result.nets[0];
    assert!(pair.connected, "{pair:?}");
    let keepout = pair.rejections.iter().find(|(gate, _)| gate == "DRC-COPPER-KEEPOUT").map_or(0, |(_, n)| *n);
    assert!(keepout > 0, "{:?}", pair.rejections);
    assert!(pair.rejection_examples[0].starts_with("DRC-COPPER-KEEPOUT: "), "{:?}", pair.rejection_examples);
    assert!(!finding_codes(&result.board).contains("DRC-COPPER-KEEPOUT"));
}

// D3 ---------------------------------------------------------------------------

fn with_guide_via(mut global: GlobalRoutingResult, net: &str, position: copper_physical::Point) -> GlobalRoutingResult {
    for route in global.routes.iter_mut().filter(|route| route.net == net) {
        route.vias = vec![via_proposal(&route.net, position, FRONT, BACK)];
    }
    global
}

fn coarse_attempt(board: &PhysicalBoard, global: &GlobalRoutingResult) -> crate::critical::Proposal {
    let rules = &board.net_routing_rules;
    route_pair(board, &rules[0], &rules[1], &guides_by_net(global), false, board).expect("coarse pair")
}

#[test]
fn coarse_guide_emits_no_transitions_for_a_via_free_or_single_layer_pair() {
    for via_free in [true, false] {
        let mut board = usb_pair_board();
        for rule in &mut board.net_routing_rules {
            if via_free {
                rule.max_vias = Some(0);
            } else {
                rule.allowed_layers = vec![FRONT];
            }
        }
        let guides = with_guide_via(coarse(&board), "USB_DM", mm(20, 12));
        let (proposed, tracks, vias) = coarse_attempt(&board, &guides);
        assert!(!proposed.connected && tracks.is_empty() && vias.is_empty());
        assert_eq!(
            proposed.diagnostics,
            ["coarse pair guide proposes 1 layer transition(s) but the pair profile allows none"]
        );
        let result = routed(&board, &guides, None);
        assert!(result.nets[0].connected && result.locked_vias.is_empty());
    }
}

#[test]
fn coarse_guide_transition_beside_pads_is_never_emitted() {
    let board = usb_pair_board();
    // Next to J1's lands: the offset signal vias would overlap the pads.
    let guides = with_guide_via(coarse(&board), "USB_DM", at(5_500_000, 12 * MM));
    let (proposed, tracks, vias) = coarse_attempt(&board, &guides);
    assert!(!proposed.connected && tracks.is_empty() && vias.is_empty());
    assert_eq!(
        proposed.diagnostics,
        ["coarse guide transition for USB_DP at (5275000, 12000000) nm violates pad clearance or a via keep-out"]
    );
    let result = routed(&board, &guides, None);
    assert!(result.nets[0].connected);
    assert!(free_of(&result.board, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-HOLE-CLEARANCE"]));
}

#[test]
fn coarse_guide_transition_in_a_via_keepout_is_never_emitted() {
    let mut board = usb_pair_board();
    let mut fence = keepout("via-fence", &[FRONT, BACK], rectangle(19 * MM, 11 * MM, 21 * MM, 13 * MM));
    fence.block_tracks = false;
    fence.block_vias = true;
    board.copper_keepouts = vec![fence];
    let guides = with_guide_via(coarse(&board), "USB_DM", mm(20, 12));
    let (proposed, _, vias) = coarse_attempt(&board, &guides);
    assert!(vias.is_empty() && proposed.diagnostics[0].contains("violates pad clearance or a via keep-out"));
}

// D4 ---------------------------------------------------------------------------

fn lanes_of(result: &CriticalRoutingResult) -> Vec<copper_json::Object> {
    let Ok(Value::Object(document)) = copper_json::parse(&result.to_json().unwrap()) else { panic!("document") };
    let Value::Array(lanes) = &document["lanes"] else { panic!("lanes") };
    lanes.iter().map(|lane| if let Value::Object(lane) = lane { lane.clone() } else { panic!("lane") }).collect()
}

fn texts(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(*item)).collect())
}

#[test]
fn lane_table_lists_length_vias_and_layers_without_inventing_delay() {
    let board = usb_pair_board();
    let result = routed(&board, &coarse(&board), None);
    let lanes = lanes_of(&result);
    assert_eq!(
        lanes.iter().map(|lane| lane["net"].clone()).collect::<Vec<_>>(),
        [Value::from("USB_DM"), Value::from("USB_DP")]
    );
    for (lane, length) in lanes.iter().zip(&result.nets[0].lengths_nm) {
        assert_eq!(lane["group"], texts(&["USB_DM", "USB_DP"]));
        assert_eq!(lane["connected"], Value::Bool(true));
        assert!(*length > 0);
        assert_eq!(lane["routed_length_nm"], Value::from(*length));
        assert_eq!(lane["layers"], texts(&["F.Cu"]));
        let mut per_layer = copper_json::Object::new();
        per_layer.insert("F.Cu".to_owned(), Value::from(*length));
        assert_eq!(lane["layer_lengths_nm"], Value::Object(per_layer));
        assert_eq!(lane["via_count"], Value::from(0i64));
        assert_eq!(lane["estimated_delay_ps"], Value::Null);
        assert_eq!(lane["delay_evidence_grade"], Value::Null);
        assert_eq!(lane["delay_reason"], Value::from("no stack-up declared"));
    }
}

fn four_layer_stackup(prepreg: Option<&str>) -> Stackup {
    let layer = |id: &str, copper: Option<CopperLayer>, thickness: &str, er: Option<&str>| StackupLayer {
        id: id.to_owned(),
        copper_layer: copper,
        thickness_nm: nm(thickness),
        material: None,
        relative_permittivity: er.map(|value| Decimal::parse(value).expect("decimal")),
        loss_tangent: None,
        dielectric_type: None,
    };
    let layers = vec![
        layer("F.Cu", Some(FRONT), "0.035", None),
        layer("P1", None, "0.1", prepreg),
        layer("In1.Cu", Some(IN1), "0.0175", None),
        layer("C1", None, "1.2", Some("4.5")),
        layer("In2.Cu", Some(IN2), "0.0175", None),
        layer("P2", None, "0.1", Some("4.1")),
        layer("B.Cu", Some(BACK), "0.035", None),
    ];
    Stackup {
        thickness_nm: layers.iter().map(|layer| layer.thickness_nm).sum(),
        physical_layers: layers,
        ..Stackup::new(vec![FRONT, IN1, IN2, BACK])
    }
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Float(value) => *value,
        Value::Int(value) => i64::try_from(value.clone()).expect("i64") as f64,
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn lane_table_estimates_screening_delay_from_the_declared_stackup() {
    let width = 250_000;
    let mut board = usb_pair_board();
    board.stackup = four_layer_stackup(Some("4.1"));
    board.nets.push(net("GND", &[]));
    board.tracks = vec![
        track("USB_DP", at(5 * MM, 11_500_000), at(10 * MM, 11_500_000), width, FRONT),
        track("USB_DP", at(10 * MM, 11_500_000), at(30 * MM, 11_500_000), width, IN1),
    ];
    board.vias = vec![
        through_via("USB_DP", at(10 * MM, 11_500_000), 800_000, 400_000),
        through_via("GND", mm(10, 10), 800_000, 400_000),
    ];
    let names = vec!["USB_DM".to_owned(), "USB_DP".to_owned()];
    let pair = CriticalNetResult::new(names, true, 2, 1, vec![0, 25 * MM], 0, Vec::new());
    let batch = CriticalNetResult::new(vec!["<critical-batch>".to_owned()], false, 0, 0, Vec::new(), 0, Vec::new());
    let table = critical_lane_table(&board, &[pair.clone(), batch]).expect("lanes");
    let [Value::Object(dm), Value::Object(dp)] = &table[..] else { panic!("two lanes: {table:?}") };
    assert_eq!(dm["routed_length_nm"], Value::from(0i64));
    assert_eq!(dm["layers"], Value::Array(Vec::new()));
    assert_eq!(dm["via_count"], Value::from(0i64));
    assert_eq!(dm["estimated_delay_ps"], Value::Null);
    assert_eq!(dm["delay_reason"], Value::from("no routed copper"));
    assert_eq!(dp["routed_length_nm"], Value::from(25 * MM));
    assert_eq!(dp["via_count"], Value::from(1i64));
    assert_eq!(dp["layers"], texts(&["F.Cu", "In1.Cu"]));
    let mut per_layer = copper_json::Object::new();
    per_layer.insert("F.Cu".to_owned(), Value::from(5 * MM));
    per_layer.insert("In1.Cu".to_owned(), Value::from(20 * MM));
    assert_eq!(dp["layer_lengths_nm"], Value::Object(per_layer));
    // Outer layer: Hammerstad-Jensen microstrip over the 0.1 mm prepreg.
    // Inner layer: thickness-weighted permittivity of the prepreg and core.
    let microstrip = (4.1 + 1.0) / 2.0 + (4.1 - 1.0) / 2.0 / (1.0 + 12.0 * 0.1 / 0.25f64).sqrt();
    let stripline = (4.1 * 0.1 + 4.5 * 1.2) / 1.3;
    let round = |value: f64, places: i32| (value * 10f64.powi(places)).round() / 10f64.powi(places);
    let Value::Object(permittivity) = &dp["effective_permittivity"] else { panic!("permittivity") };
    assert!((number(&permittivity["F.Cu"]) - round(microstrip, 4)).abs() < 1e-9);
    assert!((number(&permittivity["In1.Cu"]) - round(stripline, 4)).abs() < 1e-9);
    // `propagation_delay(length, er)`: length * sqrt(er) / c, er to six places.
    let expected: f64 = [(5e-3, microstrip), (20e-3, stripline)]
        .iter()
        .map(|(metres, er)| metres * round(*er, 6).sqrt() / 299_792_458.0)
        .sum::<f64>()
        * 1e12;
    let delay = number(&dp["estimated_delay_ps"]);
    assert!((delay - round(expected, 3)).abs() <= 0.0011, "{delay} vs {expected}");
    assert!(170.0 < delay && delay < 172.0);
    assert_eq!(dp["delay_evidence_grade"], Value::from("screening"));
    assert_eq!(dp["delay_reason"], Value::Null);
    let mut missing = board.clone();
    missing.stackup = four_layer_stackup(None);
    let table = critical_lane_table(&missing, &[pair]).expect("lanes");
    let Value::Object(lane) = &table[1] else { panic!("lane") };
    assert_eq!(lane["estimated_delay_ps"], Value::Null);
    assert_eq!(lane["delay_reason"], Value::from("dielectric P1 declares no relative permittivity"));
}

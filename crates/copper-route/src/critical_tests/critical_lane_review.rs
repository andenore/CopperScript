//! `tests/test_critical_lane_review.py`: D-PHY plan D6, the routed
//! critical-lane review of the route report (`critical_lane_review`).

use copper_json::{Object, Value};
use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::footprint::{PadShape, Size};

use crate::critical::CriticalNetResult;
use crate::critical_review::{critical_lane_review, lane_review_line};
use crate::test_support_critical::*;

const WIDTH: i64 = 200_000;

fn lane(net: &str, start: (&str, &str), end: (&str, &str)) -> TrackSegment {
    track(net, at(nm(start.0), nm(start.1)), at(nm(end.0), nm(end.1)), WIDTH, FRONT)
}

/// `_review_board()`: two pairs on F.Cu (0.2 mm wide, 0.2 mm gap) and a
/// single-ended net.
///
/// Pair A runs straight from J1 to J2 at y = 10 / 10.4 mm with 1 mm
/// breakout regions around its lands (cut at x = 6 and 30 mm). Pair B runs
/// at y = 10.9 / 11.3 mm over x = 10..20 mm, 0.3 mm edge to edge from A_N,
/// then bends 90 degrees; B_N is 0.8 mm shorter. C (a clock) bends 45
/// degrees and ends on a via. S is a signal net next to J1's breakout; GND
/// owns a zone.
fn review_board() -> PhysicalBoard {
    let land = |number: &str, y: i64| copper_physical::footprint::FootprintPad {
        shape: PadShape::Circle,
        ..smd_pad(number, at(0, y), 200_000)
    };
    let lands =
        footprint("test/pair-lands", vec![land("1", 0), land("2", 400_000)], Size { width_nm: MM, height_nm: MM });
    let member = |net: &str, partner: &str, skew: &str, breakout: bool| NetRoutingRule {
        max_skew_nm: Some(nm(skew)),
        breakout_length_nm: breakout.then_some(MM),
        breakout_gap_nm: breakout.then_some(100_000),
        ..differential_rule(net, partner, WIDTH, 200_000)
    };
    let mut nets = vec![net("A_P", &[("J1", "1"), ("J2", "1")]), net("A_N", &[("J1", "2"), ("J2", "2")])];
    nets.extend(["B_P", "B_N", "C", "S", "GND"].map(|name| net(name, &[])));
    let poses = vec![pose("J1", "test/pair-lands", mm(5, 10)), pose("J2", "test/pair-lands", mm(31, 10))];
    let mut board = board("LaneReview", 40, 25, vec![lands], poses, nets);
    board.net_routing_rules = vec![
        member("A_P", "A_N", "0.5", true),
        member("A_N", "A_P", "0.5", true),
        member("B_P", "B_N", "0.1", false),
        member("B_N", "B_P", "0.1", false),
        NetRoutingRule { width_nm: Some(WIDTH), ..rule("C", RouteKind::Clock) },
    ];
    board.tracks = vec![
        lane("A_P", ("5", "10"), ("6", "10")),
        lane("A_P", ("6", "10"), ("30", "10")),
        lane("A_P", ("30", "10"), ("31", "10")),
        lane("A_N", ("5", "10.4"), ("6", "10.4")),
        lane("A_N", ("6", "10.4"), ("30", "10.4")),
        lane("A_N", ("30", "10.4"), ("31", "10.4")),
        lane("B_P", ("10", "10.9"), ("20", "10.9")),
        lane("B_P", ("20", "10.9"), ("20", "12.9")),
        lane("B_N", ("10", "11.3"), ("19.6", "11.3")),
        lane("B_N", ("19.6", "11.3"), ("19.6", "12.9")),
        lane("C", ("10", "16"), ("14", "16")),
        lane("C", ("14", "16"), ("17", "19")),
        lane("S", ("4", "9.5"), ("5.5", "9.5")),
        // 0.2 mm from A_P, but GND owns a zone and is ignored.
        lane("GND", ("8", "9.6"), ("9", "9.6")),
    ];
    board.vias = vec![through_via("C", mm(17, 19), 600_000, 300_000)];
    board.zones = vec![zone("ground", "GND", &[BACK], rectangle(0, 0, 40 * MM, 25 * MM))];
    board.match_groups = vec![NetMatchGroup {
        id: "lanes".to_owned(),
        nets: ["A_P", "A_N", "C"].map(str::to_owned).to_vec(),
        max_skew_nm: 20 * MM,
    }];
    board
}

fn results() -> Vec<CriticalNetResult> {
    let names = |items: &[&str]| items.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    vec![
        CriticalNetResult::new(names(&["A_N", "A_P"]), true, 6, 0, vec![26 * MM; 2], 0, Vec::new()),
        CriticalNetResult::new(names(&["B_N", "B_P"]), true, 4, 0, vec![nm("11.2"), 12 * MM], nm("0.8"), Vec::new()),
        CriticalNetResult::new(names(&["C"]), true, 2, 1, vec![8_242_641], 0, Vec::new()),
    ]
}

/// `TUNING`: the critical stage's tuning document of group `lanes`.
fn tuning() -> Vec<Value> {
    vec![json(r#"{"id": "lanes", "max_skew_nm": 20000000, "status": "within_limit"}"#)]
}

fn json(text: &str) -> Value {
    copper_json::parse(text).expect("JSON")
}

fn review(board: &PhysicalBoard, results: &[CriticalNetResult], tuning: &[Value]) -> Object {
    critical_lane_review(board, results, tuning).expect("review")
}

fn nets(review: &Object) -> Vec<&Object> {
    let Value::Array(items) = &review["nets"] else { panic!("nets") };
    items.iter().map(|item| if let Value::Object(item) = item { item } else { panic!("a lane") }).collect()
}

fn lane_of<'a>(review: &'a Object, net: &str) -> &'a Object {
    nets(review).into_iter().find(|item| item["net"] == Value::from(net)).expect("a reviewed net")
}

fn keys(value: &Value) -> Vec<String> {
    let Value::Object(object) = value else { panic!("an object: {value:?}") };
    let mut keys: Vec<String> = object.keys().cloned().collect();
    keys.sort();
    keys
}

fn sorted(items: &[&str]) -> Vec<String> {
    let mut items: Vec<String> = items.iter().map(|s| (*s).to_owned()).collect();
    items.sort();
    items
}

/// `_minimum(distance, neighbour, at, kind)` as JSON text.
fn minimum(distance: i64, neighbour: &str, point: (&str, &str)) -> String {
    format!(
        r#"{{"distance_nm": {distance}, "neighbour": "{neighbour}", "object": "track", "pad": null, "layer": "F.Cu", "at_nm": [{}, {}]}}"#,
        nm(point.0),
        nm(point.1)
    )
}

fn spacing(inside: (Option<String>, Option<String>), outside: (Option<String>, Option<String>)) -> Value {
    let or_null = |item: Option<String>| item.unwrap_or_else(|| "null".to_owned());
    json(&format!(
        r#"{{"inside_breakout": {{"critical": {}, "signal": {}}}, "outside_breakout": {{"critical": {}, "signal": {}}}}}"#,
        or_null(inside.0),
        or_null(inside.1),
        or_null(outside.0),
        or_null(outside.1)
    ))
}

#[test]
fn review_reports_lengths_skew_spacing_coupling_and_bends() {
    let review = review(&review_board(), &results(), &tuning());
    assert_eq!(review["search_radius_nm"], Value::from(MM));
    assert!(review["coupling_factor"] == Value::from(2i64) && review["sharp_bend_degrees"] == Value::from(45i64));
    assert_eq!(review["ignored_zone_nets"], json(r#"["GND"]"#));
    let lanes = nets(&review);
    let names: Vec<&Value> = lanes.iter().map(|item| &item["net"]).collect();
    assert_eq!(names, ["A_N", "A_P", "B_N", "B_P", "C"].map(Value::from).iter().collect::<Vec<_>>());
    let column = |key: &str| lanes.iter().map(|item| item[key].clone()).collect::<Vec<_>>();

    // Lengths are tracks only; C's via barrel is not counted.
    assert_eq!(
        column("routed_length_nm"),
        [26 * MM, 26 * MM, nm("11.2"), 12 * MM, 4 * MM + 4_242_641].map(Value::from)
    );
    assert_eq!(column("via_count"), [0i64, 0, 0, 0, 1].map(Value::from));
    assert!(column("layers").iter().all(|layers| *layers == json(r#"["F.Cu"]"#)));
    assert_eq!(column("partner"), json(r#"["A_P", "A_N", "B_P", "B_N", null]"#).as_array().expect("partners").to_vec());
    assert_eq!(column("breakout"), [true, true, false, false, false].map(Value::Bool));

    assert_eq!(
        review["pairs"],
        json(&format!(
            r#"[{{"nets": ["A_N", "A_P"], "lengths_nm": [26000000, 26000000], "skew_nm": 0, "max_skew_nm": 500000, "status": "pass"}},
                {{"nets": ["B_N", "B_P"], "lengths_nm": [11200000, 12000000], "skew_nm": 800000, "max_skew_nm": 100000, "status": "fail"}}]"#
        ))
    );
    assert_eq!(
        review["match_groups"],
        json(
            r#"[{"id": "lanes", "max_skew_nm": 20000000, "skew_nm": 17757359, "status": "pass",
                "members": [
                    {"net": "A_P", "length_nm": 26000000, "routed": true, "shortfall_nm": 0},
                    {"net": "A_N", "length_nm": 26000000, "routed": true, "shortfall_nm": 0},
                    {"net": "C", "length_nm": 8242641, "routed": true, "shortfall_nm": 17757359}],
                "tuning_status": "within_limit"}]"#
        )
    );

    // A_P: S is 0.3 mm from the J1 breakout piece and 0.5071 mm
    // (diagonally) from the channel; B_P is 0.7 mm away. The partner and GND
    // are ignored.
    let diagonal = |squared: f64| (squared.sqrt() * 1_000_000.0).round() as i64 - 200_000;
    assert_eq!(
        lane_of(&review, "A_P")["spacing"],
        spacing(
            (None, Some(minimum(300_000, "S", ("5", "10")))),
            (Some(minimum(700_000, "B_P", ("10", "10"))), Some(minimum(diagonal(0.5), "S", ("6", "10")))),
        )
    );
    assert_eq!(
        lane_of(&review, "A_N")["spacing"],
        spacing(
            (None, Some(minimum(700_000, "S", ("5", "10.4")))),
            (Some(minimum(300_000, "B_P", ("10", "10.4"))), Some(minimum(diagonal(1.06), "S", ("6", "10.4")))),
        )
    );
    // Without breakout properties, all copper is outside; S is beyond 1 mm.
    assert_eq!(
        lane_of(&review, "B_P")["spacing"],
        spacing((None, None), (Some(minimum(300_000, "A_N", ("10", "10.9"))), None))
    );
    let Value::Object(b_n) = &lane_of(&review, "B_N")["spacing"] else { panic!("spacing") };
    assert_eq!(
        b_n["outside_breakout"],
        json(&format!(r#"{{"critical": {}, "signal": null}}"#, minimum(700_000, "A_N", ("10", "11.3"))))
    );
    assert_eq!(lane_of(&review, "C")["spacing"], spacing((None, None), (None, None)));

    // Coupled: closer than 2 x 0.2 mm to the other pair, outside the
    // breakout. A_N's channel is within reach of B_P's 10 mm run plus
    // sqrt(0.6^2 - 0.5^2) mm past each end; B_P couples over its run and
    // 0.1 mm of its bend leg.
    let reach = (10_000_000.0 + 2.0 * (600_000f64.powi(2) - 500_000f64.powi(2)).sqrt()).round() as i64;
    let coupling: Vec<(Value, Value, Value)> = lanes
        .iter()
        .map(|item| {
            (item["coupling_threshold_nm"].clone(), item["coupled_length_nm"].clone(), item["coupled_nets"].clone())
        })
        .collect();
    assert_eq!(
        coupling,
        vec![
            (Value::from(400_000i64), Value::from(reach), json(r#"["B_P"]"#)),
            (Value::from(400_000i64), Value::from(0i64), json("[]")),
            (Value::from(400_000i64), Value::from(0i64), json("[]")),
            (Value::from(400_000i64), Value::from(nm("10.1")), json(r#"["A_N"]"#)),
            (Value::Null, Value::Null, json("[]")),
        ]
    );

    // Straight joins are 0 degrees; B turns 90 degrees, C 45 (not sharp).
    assert_eq!(lane_of(&review, "A_P")["bends"], json(r#"{"sharpest_degrees": 0.0, "sharp_count": 0, "sharp": []}"#));
    assert_eq!(
        lane_of(&review, "B_P")["bends"],
        json(
            r#"{"sharpest_degrees": 90.0, "sharp_count": 1, "sharp": [
                {"at_nm": [20000000, 10900000], "layer": "F.Cu", "degrees": 90.0}]}"#
        )
    );
    let Value::Object(b_n) = &lane_of(&review, "B_N")["bends"] else { panic!("bends") };
    assert_eq!(b_n["sharp"], json(r#"[{"at_nm": [19600000, 11300000], "layer": "F.Cu", "degrees": 90.0}]"#));
    assert_eq!(lane_of(&review, "C")["bends"], json(r#"{"sharpest_degrees": 45.0, "sharp_count": 0, "sharp": []}"#));
}

#[test]
fn review_json_shape() {
    let review = review(&review_board(), &results(), &tuning());
    let text = copper_json::Dumps::new().dumps(&Value::Object(review.clone()));
    assert_eq!(json(&text), Value::Object(review.clone()));
    assert_eq!(
        keys(&Value::Object(review.clone())),
        sorted(&[
            "search_radius_nm",
            "coupling_factor",
            "sharp_bend_degrees",
            "ignored_zone_nets",
            "nets",
            "pairs",
            "match_groups"
        ])
    );
    let lane_keys = [
        "net",
        "group",
        "connected",
        "routed_length_nm",
        "layer_lengths_nm",
        "layers",
        "via_count",
        "estimated_delay_ps",
        "effective_permittivity",
        "delay_evidence_grade",
        "delay_reason",
        "partner",
        "breakout",
        "spacing",
        "coupling_threshold_nm",
        "coupled_length_nm",
        "coupled_nets",
        "bends",
    ];
    for item in nets(&review) {
        assert_eq!(keys(&Value::Object(item.clone())), sorted(&lane_keys));
        assert_eq!(keys(&item["spacing"]), sorted(&["inside_breakout", "outside_breakout"]));
        let Value::Object(regions) = &item["spacing"] else { panic!("spacing") };
        for region in regions.values() {
            assert_eq!(keys(region), sorted(&["critical", "signal"]));
            let Value::Object(region) = region else { panic!("region") };
            for minimum in region.values() {
                assert!(
                    *minimum == Value::Null
                        || keys(minimum) == sorted(&["distance_nm", "neighbour", "object", "pad", "layer", "at_nm"])
                );
            }
        }
        assert_eq!(keys(&item["bends"]), sorted(&["sharpest_degrees", "sharp_count", "sharp"]));
    }
    let Value::Array(pairs) = &review["pairs"] else { panic!("pairs") };
    assert!(pairs.iter().all(|pair| keys(pair) == sorted(&["nets", "lengths_nm", "skew_nm", "max_skew_nm", "status"])));
    let Value::Array(groups) = &review["match_groups"] else { panic!("groups") };
    assert_eq!(keys(&groups[0]), sorted(&["id", "max_skew_nm", "skew_nm", "status", "members", "tuning_status"]));
    // Results select the reviewed nets; neighbour classes come from the rules.
    let partial = critical_lane_review(&review_board(), &results()[..1], &[]).expect("review");
    let partial_nets: Vec<Object> = nets(&partial).into_iter().cloned().collect();
    assert_eq!(
        partial_nets.iter().map(|item| item["net"].clone()).collect::<Vec<_>>(),
        ["A_N", "A_P"].map(Value::from)
    );
    assert_eq!(partial_nets, nets(&review)[..2].iter().map(|item| (*item).clone()).collect::<Vec<_>>());
    let empty = critical_lane_review(&review_board(), &[], &[]).expect("review");
    assert!(empty["nets"] == json("[]") && empty["pairs"] == json("[]"));
    let Value::Array(groups) = &empty["match_groups"] else { panic!("groups") };
    let Value::Object(group) = &groups[0] else { panic!("group") };
    assert_eq!(group["tuning_status"], Value::Null);
}

#[test]
fn review_names_pad_neighbours_and_ignores_pads_without_a_net() {
    let mut board = review_board();
    // J3.2 (net T) is 0.4 mm from A_P's channel; J4's lands have no net and
    // one of them is 0.15 mm away.
    board.placements.push(pose("J3", "test/pair-lands", mm(12, 9)));
    board.placements.push(pose("J4", "test/pair-lands", at(16 * MM, 9_250_000)));
    board.nets.push(net("T", &[("J3", "2")]));
    let review = review(&board, &results(), &[]);
    let Value::Object(spacing) = &lane_of(&review, "A_P")["spacing"] else { panic!("spacing") };
    let Value::Object(outside) = &spacing["outside_breakout"] else { panic!("outside") };
    assert_eq!(
        outside["signal"],
        json(
            r#"{"distance_nm": 400000, "neighbour": "T", "object": "pad", "pad": "J3.2",
                "layer": "F.Cu", "at_nm": [12000000, 10000000]}"#
        )
    );
}

#[test]
fn console_line_summarises_the_review() {
    let review = review(&review_board(), &results(), &tuning());
    assert_eq!(
        lane_review_line(&review),
        "CRITICAL LANES: 5 nets; pairs=2, worst skew=0.800 mm, over max_skew=1; \
         match groups=1, over max_skew=0; min spacing outside breakout: \
         critical=0.300 mm (A_N to B_P), signal=0.507 mm (A_P to S); \
         coupled length=20.763 mm; bends>45deg=2 (sharpest 90.0 deg)"
    );
    let single = critical_lane_review(&review_board(), &results()[2..], &tuning()).expect("review");
    assert_eq!(
        lane_review_line(&single),
        "CRITICAL LANES: 1 nets; pairs=0, worst skew=n/a, over max_skew=0; \
         match groups=1, over max_skew=0; min spacing outside breakout: \
         critical=none within 1.000 mm, signal=none within 1.000 mm; \
         coupled length=0.000 mm; bends>45deg=0 (sharpest 45.0 deg)"
    );
}

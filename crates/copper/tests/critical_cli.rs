//! Critical-routing CLI cases of the former Python suite (d39067a):
//! `copper replay-critical` on saved source-only inputs
//! (`test_critical_replay.py`), `route-board` computational bounds
//! (`test_pair_terminal_fallback.py`), critical-lane review
//! (`test_critical_lane_review.py`) and the loose-pair repair issue
//! (`test_loose_pair_repair.py`).

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalNet, Placement, RouteKind};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardOutline, Point};
use copper_route::routing_acceptance::{board, general_rule};
use copper_state::{StateDocument, StateValue};
use copper_units::Decimal;
use indexmap::IndexMap;

const MM: i64 = 1_000_000;

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("critical-replay-cli").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn pad(number: &str, y: i64) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position: Point::new(0, y),
        size: Size { width_nm: 400_000, height_nm: 400_000 },
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::zero(),
        drill: None,
        roundrect_ratio_ppm: 250_000,
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    }
}

/// `test_critical_routing.py::_pair_board`.
fn pair_board() -> copper_pcb::board::PhysicalBoard {
    let footprint = PhysicalFootprint {
        name: "test/two-pad-pair".to_owned(),
        pads: vec![pad("1", -500_000), pad("2", 500_000)],
        body_size: Size { width_nm: MM, height_nm: 2 * MM },
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    };
    let zero = Decimal::zero();
    let poses = vec![
        Placement::new("J1", &footprint.name, Point::new(5 * MM, 12 * MM), &zero).unwrap(),
        Placement::new("J2", &footprint.name, Point::new(35 * MM, 12 * MM), &zero).unwrap(),
    ];
    let net = |name: &str, pad: &str| PhysicalNet {
        name: name.to_owned(),
        pads: vec![PadReference::new("J1", pad), PadReference::new("J2", pad)],
    };
    let mut board = board(
        "CriticalPair",
        BoardOutline::rectangle(40 * MM, 25 * MM, None).unwrap(),
        IndexMap::from([(footprint.name.clone(), footprint)]),
        poses,
        vec![net("USB_DP", "1"), net("USB_DM", "2")],
    );
    board.net_routing_rules = [("USB_DP", "USB_DM"), ("USB_DM", "USB_DP")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            kind: RouteKind::Differential,
            priority: 100,
            width_nm: Some(250_000),
            differential_partner: Some(partner.to_owned()),
            pair_gap_nm: Some(200_000),
            max_skew_nm: Some(10 * MM),
            maximum_uncoupled_length_nm: Some(10 * MM),
            target_impedance_ohms: Some(90),
            ..general_rule(name)
        })
        .collect();
    board
}

fn run(args: &[&str]) -> i32 {
    run_output(args).0
}

/// The exit status and standard error of `copper ARGS`.
fn run_output(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_copper")).args(args).output().expect("the copper binary runs");
    (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn critical_replay_cli_is_explicitly_diagnostic_and_writes_locatable_report() {
    let directory = scratch("diagnostic");
    let board = pair_board();
    let global = copper_route::route_global(&board, &copper_route::GlobalRouterOptions::default(), None, None, None)
        .expect("global route");
    let value = StateValue::Map {
        entries: vec![
            (StateValue::Str("board".to_owned()), copper_route::state_codec::encode_board(&board)),
            (StateValue::Str("global_route".to_owned()), copper_route::state_codec::encode_global_route(&global)),
        ],
        proxy: false,
    };
    let state = directory.join("inputs.state.json");
    std::fs::write(&state, StateDocument::new("dict", copper_json::Object::new(), value).to_text()).expect("state");
    let (report, output) = (directory.join("route-report.json"), directory.join("board.kicad_pcb"));
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let status =
        run(&["replay-critical", &path(&state), "--only", "USB_DP", "--report", &path(&report), "-o", &path(&output)]);
    assert_eq!(status, 0);
    let Ok(Value::Object(document)) = copper_json::parse(&std::fs::read_to_string(&report).expect("report")) else {
        panic!("a report object")
    };
    assert_eq!(document["fabrication_ready"], Value::Bool(false));
    assert_eq!(document["native_verification"], Value::from("not_run"));
    assert_eq!(document["pair_state_limit"], Value::from(6000i64));
    let Value::Object(critical) = &document["critical"] else { panic!("critical") };
    let Value::Array(nets) = &critical["nets"] else { panic!("nets") };
    assert_eq!(nets.len(), 1);
    let Value::Object(net) = &nets[0] else { panic!("net") };
    assert_eq!(net["connected"], Value::Bool(true));
    assert!(output.is_file());
    assert_eq!(run(&["replay-critical", &path(&state), "--only", "missing"]), 2);
    // argparse rejects a negative bound (Python's SystemExit, status 2).
    assert_eq!(run(&["replay-critical", &path(&state), "--pair-states", "-1"]), 2);
}

#[test]
fn cli_exposes_positive_computational_pin_access_step() {
    let (status, stderr) = run_output(&["route-board", "missing.copper", "--pin-access-step-mm", "0"]);
    assert_eq!(status, 2);
    assert!(stderr.contains("positive"), "{stderr}");
}

/// `test_critical_lane_review.py::test_route_board_reports_the_review_of_the_exported_copper`.
#[test]
fn route_board_reports_the_review_of_the_exported_copper() {
    let directory = scratch("lane-review");
    let source = directory.join("board.copper");
    std::fs::write(
        &source,
        r#"board Lanes {
        use library "tiny";
        component R1: RESISTOR { footprint = "0402"; }
        component R2: RESISTOR { footprint = "0402"; }
        net DP { R1.1; R2.1; }
        net DN { R1.2; R2.2; }
        constraint routing(DP) {
            kind = differential; partner = DN; width = 0.2mm; pair_gap = 0.2mm; max_skew = 1mm;
        }
        constraint routing(DN) {
            kind = differential; partner = DP; width = 0.2mm; pair_gap = 0.2mm; max_skew = 1mm;
        }
    }
    "#,
    )
    .expect("source");
    let (report, output) = (directory.join("route-report.json"), directory.join("board.kicad_pcb"));
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let run = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args([
            "route-board",
            &path(&source),
            "--allow-proxy-footprints",
            "--layers",
            "2",
            "--fab-profile",
            "generic",
            "--candidates",
            "1",
            "--report",
            &path(&report),
            "-o",
            &path(&output),
        ])
        .output()
        .expect("the copper binary runs");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(matches!(run.status.code(), Some(0 | 1)), "{stdout}{}", String::from_utf8_lossy(&run.stderr));
    let Ok(Value::Object(document)) = copper_json::parse(&std::fs::read_to_string(&report).expect("report")) else {
        panic!("a report object")
    };
    let Value::Object(review) = &document["critical_lane_review"] else { panic!("a lane review") };
    let Value::Array(nets) = &review["nets"] else { panic!("nets") };
    let field = |item: &Value, key: &str| match item {
        Value::Object(object) => object[key].clone(),
        _ => panic!("an object"),
    };
    assert_eq!(nets.iter().map(|item| field(item, "net")).collect::<Vec<_>>(), [Value::from("DN"), Value::from("DP")]);
    let Value::Array(pairs) = &review["pairs"] else { panic!("pairs") };
    assert_eq!(pairs.len(), 1);
    assert_eq!(field(&pairs[0], "nets"), Value::Array(vec![Value::from("DN"), Value::from("DP")]));
    assert_eq!(field(&pairs[0], "status"), Value::from("pass"));
    let lengths: Vec<Value> = nets.iter().map(|item| field(item, "routed_length_nm")).collect();
    assert_eq!(field(&pairs[0], "lengths_nm"), Value::Array(lengths));
    let line = copper_route::critical_review::lane_review_line(review);
    assert!(stdout.lines().any(|item| item == line), "{stdout}");
}

/// `test_loose_pair_repair.py::_loose_board`: U1 sends LANE_A to J1 and
/// LANE_B to the sealed J2; no bundle joins them.
fn loose_board() -> copper_pcb::board::PhysicalBoard {
    let land = |number: &str, x: i64, y: i64, height: i64| FootprintPad {
        number: number.to_owned(),
        position: Point::new(x, y),
        size: Size { width_nm: 600_000, height_nm: height },
        ..pad(number, 0)
    };
    let footprint = |name: &str, pads: Vec<FootprintPad>, height: i64| PhysicalFootprint {
        name: name.to_owned(),
        pads,
        body_size: Size { width_nm: 3 * MM, height_nm: height },
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    };
    let package = footprint(
        "test/package-two-pairs",
        vec![
            land("1", 1_400_000, -1_250_000, 250_000),
            land("2", 1_400_000, -750_000, 250_000),
            land("3", 1_400_000, 750_000, 250_000),
            land("4", 1_400_000, 1_250_000, 250_000),
        ],
        8 * MM,
    );
    let connector = footprint(
        "test/connector-pair",
        vec![land("1", -1_400_000, -200_000, 200_000), land("2", -1_400_000, 200_000, 200_000)],
        2 * MM,
    );
    let zero = Decimal::zero();
    let poses = vec![
        Placement::new("U1", &package.name, Point::new(8 * MM, 10 * MM), &zero).unwrap(),
        Placement::new("J1", &connector.name, Point::new(22 * MM, 6 * MM), &zero).unwrap(),
        Placement::new("J2", &connector.name, Point::new(22 * MM, 14 * MM), &zero).unwrap(),
    ];
    let net = |name: &str, pads: [(&str, &str); 2]| PhysicalNet {
        name: name.to_owned(),
        pads: pads.iter().map(|(component, pad)| PadReference::new(component, pad)).collect(),
    };
    let nets = vec![
        net("LANE_A_P", [("U1", "1"), ("J1", "1")]),
        net("LANE_A_N", [("U1", "2"), ("J1", "2")]),
        net("LANE_B_P", [("U1", "3"), ("J2", "1")]),
        net("LANE_B_N", [("U1", "4"), ("J2", "2")]),
    ];
    let mut board = board(
        "LoosePairs",
        BoardOutline::rectangle(30 * MM, 20 * MM, None).unwrap(),
        IndexMap::from([(package.name.clone(), package), (connector.name.clone(), connector)]),
        poses,
        nets,
    );
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    let sealed = [(18, 11), (26, 11), (26, 17), (18, 17)].map(|(x, y)| Point::new(x * MM, y * MM)).to_vec();
    board.copper_keepouts = vec![copper_route::routing_acceptance::keepout(
        "sealed-connector",
        vec![copper_physical::CopperLayer::Front, copper_physical::CopperLayer::Back],
        sealed,
    )];
    board.net_routing_rules = [("LANE_A", 110), ("LANE_B", 100)]
        .into_iter()
        .flat_map(|(name, priority)| {
            let (p, n) = (format!("{name}_P"), format!("{name}_N"));
            [(p.clone(), n.clone()), (n, p)].map(|(net, partner)| NetRoutingRule {
                kind: RouteKind::Differential,
                priority,
                width_nm: Some(200_000),
                differential_partner: Some(partner),
                pair_gap_nm: Some(200_000),
                ..general_rule(&net)
            })
        })
        .collect();
    board
}

/// `test_loose_pair_repair.py::test_a_failed_loose_pair_records_the_rip_up_it_tried`:
/// the issue list names the rip-up the failed loose pair tried.
#[test]
fn issues_name_the_rip_up_a_failed_loose_pair_tried() {
    let directory = scratch("loose-pair-repair");
    let board = loose_board();
    let global = copper_route::route_global(&board, &copper_route::GlobalRouterOptions::default(), None, None, None)
        .expect("global route");
    let options = copper_route::critical::CriticalOptions { pair_state_limit: Some(2000), ..Default::default() };
    let result = copper_route::critical::route_critical_nets(board, &global, &options, None).expect("critical");
    let lane_b = &result.nets[1];
    assert!(!lane_b.connected && lane_b.repairs.len() == 1);
    let states = lane_b.repairs[0].search_states;
    let document = copper_json::parse(&result.to_json().expect("JSON")).expect("a document");
    let report = Value::Object(copper_json::Object::from([("critical".to_owned(), document)]));
    std::fs::write(directory.join("route-report.json"), copper_json::Dumps::new().dumps(&report)).expect("report");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["issues", &directory.to_string_lossy(), "--json"])
        .output()
        .expect("the copper binary runs");
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let Ok(Value::Object(listing)) = copper_json::parse(&String::from_utf8_lossy(&output.stdout)) else {
        panic!("an issue listing")
    };
    let Value::Array(issues) = &listing["issues"] else { panic!("issues") };
    assert_eq!(issues.len(), 1);
    let Value::Object(issue) = &issues[0] else { panic!("an issue") };
    assert_eq!(issue["code"], Value::from("CRITICAL-UNROUTED"));
    assert_eq!(issue["nets"], Value::Array(vec![Value::from("LANE_B_N"), Value::from("LANE_B_P")]));
    let title = issue["title"].as_str().expect("a title");
    let tail = format!("; ripping up LANE_A_N/LANE_A_P did not help ({states} states)");
    assert!(title.ends_with(&tail), "{title}");
}

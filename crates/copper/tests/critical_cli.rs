//! Critical-routing CLI cases of the former Python suite (d39067a):
//! `copper replay-critical` on saved source-only inputs
//! (`test_critical_replay.py`) and `route-board` computational bounds
//! (`test_pair_terminal_fallback.py`).

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

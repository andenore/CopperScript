//! `copper replay-unified` on inputs saved by the unified router (ported
//! from `tests/test_unified_router.py::test_replay_cli_routes_saved_inputs_with_overrides`).

#[path = "../../copper-route/tests/support_routers/mod.rs"]
mod support_routers;

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::Value;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use copper_state::StateValue;
use support_routers::{pad_board, routes::guides};

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-unified-cli").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn copper(args: &[&str]) -> i32 {
    let output = Command::new(env!("CARGO_BIN_EXE_copper")).args(args).output().expect("the copper binary runs");
    output.status.code().unwrap_or(-1)
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn json(path: &Path) -> Value {
    copper_json::parse(&std::fs::read_to_string(path).expect("text")).expect("JSON")
}

fn field<'a>(value: &'a StateValue, name: &str) -> &'a StateValue {
    match value {
        StateValue::Dataclass { fields, .. } => &fields[name],
        other => panic!("not a dataclass: {other:?}"),
    }
}

#[test]
fn replay_cli_routes_saved_inputs_with_overrides() {
    let directory = scratch("overrides");
    let mut board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    board.boundary_edges.push(
        copper_physical::BoardEdge::new("connector_edge", board.outline.vertices[0], board.outline.vertices[1])
            .unwrap(),
    );
    board.body_overhangs.push(copper_lang::mechanical::assembly::BodyOverhang {
        id: "connector_nose".into(),
        reference: board.placements[0].reference.clone(),
        edge: "connector_edge".into(),
        start_nm: 1_000_000,
        end_nm: 2_000_000,
        distance_nm: 500_000,
        reason: "Connector body projects beyond the board edge".into(),
    });
    let state = directory.join("inputs.json");
    let options = UnifiedRouterOptions { save_inputs_path: Some(s(&state)), ..UnifiedRouterOptions::default() };
    route_unified(&board, &guides(&board), &options, None, None, None, None, None).expect("route");
    let result_state = directory.join("routed.state.json");
    let report = directory.join("replay.json");
    let args =
        ["replay-unified", &s(&state), "--set", "maximum_iterations=3", "--report", &s(&report), "--save-result"];
    assert_eq!(copper(&[&args[..], &[&s(&result_state)]].concat()), 0);
    let saved = json(&result_state);
    let value = StateValue::decode(saved.get("value").expect("value")).expect("state value");
    let placements = field(field(&value, "board"), "placements");
    let expected = copper_route::state_codec::encode_board(&board);
    assert_eq!(placements, field(&expected, "placements"));
    for name in ["boundary_edges", "body_overhangs"] {
        assert_eq!(field(field(&value, "board"), name), field(&expected, name));
    }
    // An existing result state is preserved, not overwritten.
    let previous = std::fs::read(&result_state).expect("bytes");
    assert_eq!(copper(&["replay-unified", &s(&state), "--save-result", &s(&result_state)]), 2);
    assert_eq!(std::fs::read(&result_state).expect("bytes"), previous);
    let document = json(&report);
    let quality = document.get("routing_quality").expect("retained per-net metrics");
    assert!(document.get("drc").unwrap().get("findings").unwrap().as_array().is_some());
    let metrics = quality.get("nets").unwrap().get("A").unwrap();
    assert!(metrics.get("track_length_nm").is_some());
    assert_eq!(metrics.get("explicit_copper_connected"), Some(&Value::Bool(true)));
    let detailed = document.get("detailed").expect("same detailed diagnostics as route-board");
    let net = &detailed.get("nets").unwrap().as_array().unwrap()[0];
    assert_eq!(net.get("search_budget_exhausted"), Some(&Value::Bool(false)));
    assert_eq!(net.get("connected"), Some(&Value::Bool(true)));
    let overrides = document.get("overrides").expect("overrides");
    assert_eq!(overrides.get("maximum_iterations"), Some(&Value::Int(3.into())));
    let signal = document.get("signal").expect("signal");
    assert_eq!(signal.get("opens"), Some(&Value::Int(0.into())));
    assert_eq!(document.get("unrouted").and_then(Value::as_object).map(|o| o.len()), Some(0));
    assert_eq!(copper(&["replay-unified", &s(&state), "--set", "no_such_option=1"]), 2);
    let output = directory.join("replay.kicad_pcb");
    assert_eq!(copper(&["replay-unified", &s(&state), "--only", "A", "-o", &s(&output)]), 0);
    assert!(output.is_file());
}

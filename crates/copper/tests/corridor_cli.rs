//! `copper plan-layout` reports a reserved corridor (ported from
//! `tests/test_corridor_reservation.py::test_plan_layout_cli_reports_reserved_corridor`).

use std::path::Path;
use std::process::Command;

use copper_json::Value;

const SOURCE: &str = r#"
board CorridorCli {
    use library "tiny";
    component R1: RESISTOR { footprint = "0402"; }
    component R2: RESISTOR { footprint = "0402"; }
    component R3: RESISTOR { footprint = "0402"; }
    component C1: CAPACITOR { footprint = "0402"; }
    net DP { R1.1; R2.1; }
    net DN { R1.2; R2.2; }
    net SENSE_A { R3.1; C1.1; }
    net SENSE_B { R3.2; C1.2; }
    constraint fixed_placement(R1) { x = 40mm; y = 40mm; rotation = 90; side = front; }
    constraint fixed_placement(R2) { x = 60mm; y = 40mm; rotation = 90; side = front; }
    constraint fixed_placement(R3) { x = 50mm; y = 44mm; rotation = 0; side = front; }
    constraint routing(DP) {
        kind = differential;
        partner = DN;
        width = 0.15mm;
        pair_gap = 0.15mm;
        clearance = 0.15mm;
        reserve_corridor = true;
    }
}
"#;

#[test]
fn plan_layout_cli_reports_reserved_corridor() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("corridor-cli");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch");
    let source = directory.join("board.copper");
    std::fs::write(&source, SOURCE).expect("source");
    let report = directory.join("layout.json");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("plan-layout")
        .arg(&source)
        .arg("-o")
        .arg(directory.join("planned.kicad_pcb"))
        .arg("--report")
        .arg(&report)
        .args(["--allow-proxy-footprints", "--candidates", "1", "--offline"])
        .output()
        .expect("the copper binary runs");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let document = copper_json::parse(&std::fs::read_to_string(&report).expect("report")).expect("JSON");
    let corridors = document.get("reserved_corridors").and_then(Value::as_array).expect("corridors");
    assert_eq!(corridors.len(), 1);
    let corridor = &corridors[0];
    assert_eq!(corridor.get("nets"), Some(&Value::Array(vec![Value::from("DN"), Value::from("DP")])));
    assert_eq!(corridor.get("terminal_components"), Some(&Value::Array(vec![Value::from("R1"), Value::from("R2")])));
    assert_eq!(corridor.get("side").and_then(Value::as_str), Some("front"));
    // 0.15 width + 0.15 gap + 0.2 board minimum clearance.
    assert_eq!(corridor.get("margin_nm"), Some(&Value::from(500_000i64)));
    let findings = document.get("findings").and_then(Value::as_array).expect("findings");
    assert!(findings.iter().any(|f| f.get("code").and_then(Value::as_str) == Some("RESERVED_CORRIDORS")));
}

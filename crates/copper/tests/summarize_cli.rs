//! Per-net summaries project retained copper, including nonzero open stubs.
use std::{path::Path, process::Command};

use copper_json::Value;

// Retained metric and critical-candidate fragments from the PHY-cap full run.
// Candidate length deliberately differs from complete retained copper length.
const REPORT: &str = r#"{
  "schema":"copperscript-route-board/v0.1",
  "critical":{"nets":[{"nets":["USB_TX1M_IC","USB_TX1P_IC"],"connected":true,
    "lengths_nm":[3208767,3208767]}]},
  "routing_quality":{"track_count":2532,"track_length_nm":2804126566,"via_count":357,
    "nets":{
      "USB_TX1M_IC":{"explicit_copper_connected":true,"track_count":23,"track_length_nm":6938113,"via_count":0},
      "USB_TX1P_IC":{"explicit_copper_connected":true,"track_count":15,"track_length_nm":6938117,"via_count":0},
      "USB_TX2M_IC":{"explicit_copper_connected":false,"track_count":16,"track_length_nm":3729347,"via_count":0}
    }}
}"#;

fn invoke(report: &Path, arguments: &[&str]) -> String {
    let output =
        Command::new(env!("CARGO_BIN_EXE_copper")).arg("summarize").arg(report).args(arguments).output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn selected_retained_net_json_and_text_ignore_other_nets_and_candidate_lengths() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("summarize-retained-net-cli");
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    std::fs::write(&report, REPORT).unwrap();
    // The last selector wins, matching the other report CLI valued options.
    let rendered = invoke(&report, &["--net", "USB_TX1P_IC", "--net", "USB_TX1M_IC", "--json"]);
    let json = copper_json::parse(&rendered).unwrap();
    assert_eq!(json.get("net").unwrap().as_str(), Some("USB_TX1M_IC"));
    assert_eq!(json.get("status").unwrap().as_str(), Some("available"));
    assert_eq!(json.get("source_path").unwrap().as_str(), Some("routing_quality.nets[\"USB_TX1M_IC\"]"));
    let metrics = json.get("metrics").unwrap();
    assert_eq!(metrics.get("track_length_nm"), Some(&Value::from(6_938_113i64)));
    assert_eq!(metrics.get("track_count"), Some(&Value::from(23i64)));
    assert_eq!(metrics.get("via_count"), Some(&Value::from(0i64)));
    assert_eq!(metrics.get("explicit_copper_connected"), Some(&Value::Bool(true)));
    assert!(!rendered.contains("6938117"));
    assert!(!rendered.contains("3208767"));
    let text = invoke(&report, &["--net", "USB_TX1M_IC"]);
    assert!(text.contains("Track length: 6.938113 mm; tracks: 23; vias: 0"), "{text}");
    assert!(text.contains("Explicit copper connectivity: connected"), "{text}");
    assert!(text.contains("retained explicit copper"), "{text}");

    let stub = invoke(&report, &["--net", "USB_TX2M_IC", "--json"]);
    let stub = copper_json::parse(&stub).unwrap();
    let metrics = stub.get("metrics").unwrap();
    assert_eq!(metrics.get("track_length_nm"), Some(&Value::from(3_729_347i64)));
    assert_eq!(metrics.get("explicit_copper_connected"), Some(&Value::Bool(false)));
    let text = invoke(&report, &["--net", "USB_TX2M_IC"]);
    assert!(text.contains("Track length: 3.729347 mm"), "{text}");
    assert!(text.contains("Explicit copper connectivity: not connected"), "{text}");

    // No selector continues through the existing engineering-summary path.
    let ordinary = copper_json::parse(&invoke(&report, &["--json", "--issues", "0"])).unwrap();
    let shown_path = ordinary.get("report_path").unwrap().as_str().unwrap();
    let expected = copper_report::summary::read_summary(REPORT.as_bytes(), shown_path, 0).unwrap();
    // CLI JSON sorts object keys; canonicalize both without converting integer
    // metrics or nulls through a floating-point JSON representation.
    let canonical = |value: &Value| copper_json::Dumps::new().sort_keys(true).dumps(value);
    assert_eq!(canonical(&ordinary), canonical(&expected));
    let text = invoke(&report, &["--issues", "0"]);
    assert_eq!(text.trim_end(), copper_report::summary::summary_lines(&expected).unwrap().join("\n"));
}

#[test]
fn legacy_report_has_unavailable_net_metrics_not_zero_or_connected() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("summarize-legacy-net-cli");
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    std::fs::write(&report, r#"{"schema":"copperscript-route-board/v0.1"}"#).unwrap();
    let json = copper_json::parse(&invoke(&report, &["--net", "USB", "--json"])).unwrap();
    assert_eq!(json.get("status").unwrap().as_str(), Some("unavailable"));
    let metrics = json.get("metrics").unwrap();
    for key in ["track_length_nm", "track_count", "via_count", "explicit_copper_connected"] {
        assert_eq!(metrics.get(key), Some(&Value::Null));
    }
    let text = invoke(&report, &["--net", "USB"]);
    assert!(text.contains("Track length: unavailable; tracks: unavailable; vias: unavailable"), "{text}");
    assert!(text.contains("Explicit copper connectivity: unknown"), "{text}");
}

//! Targeted solver diagnostics keep rejected candidates distinct from retained copper.
use std::{path::Path, process::Command};

#[test]
fn selected_issue_json_exposes_solver_provenance() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("issues-diagnostics-cli");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("route-report.json"),
        r#"{
      "critical":{"nets":[{"nets":["USB"],"connected":false,"candidate_rejected":true,
        "diagnostics":["candidate clearance rejection"],"pair_searches":2,"search_states":17,
        "pair_budget_exhausted":false,"pair_collars":[{"stop_reason":"incompatible_lane_signs"}]}]}
    }"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("issues")
        .arg(&directory)
        .args(["--issue", "C1", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let json = copper_json::parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
    let issues = json.get("issues").unwrap().as_array().unwrap();
    assert_eq!(issues.len(), 1);
    let issue = &issues[0];
    assert_eq!(issue.get("id").unwrap().as_str(), Some("C1"));
    assert!(issue.get("title").unwrap().as_str().unwrap().contains("not retained-board DRC"));
    let details = issue.get("details").unwrap();
    assert_eq!(details.get("scope").unwrap().as_str(), Some("rejected_candidate"));
    assert!(details.get("records").unwrap().get("pair_collars").is_some());
    let missing = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("issues")
        .arg(&directory)
        .args(["--issue", "missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}

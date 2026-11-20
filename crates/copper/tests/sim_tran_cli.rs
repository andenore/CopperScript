//! `copper sim` on a transient bench: the precision rectifier example.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

#[test]
fn sim_precision_rectifier_example_passes() {
    let out = std::env::temp_dir().join(format!("copper-sim-tran-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["sim", "examples/simulation/precision_rectifier/board.copper", "--out", out.to_str().unwrap()])
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        stdout.starts_with("BENCH HalfWave (PrecisionRectifier): tran 0 to 3 ms, recorded from 900 us, max step 1 us"),
        "{stdout}"
    );
    assert!(stdout.contains("PASS: 6 of 6"), "{stdout}");
    let csv = std::fs::read_to_string(out.join("HalfWave.csv")).unwrap();
    assert!(csv.starts_with("step,label,time_s,V(VIN) [V],V(OUT) [V],V(AMP) [V]"));
    let page = std::fs::read_to_string(out.join("HalfWave.html")).unwrap();
    assert!(page.contains("data-axis=\"time\"") && !page.contains("src=\"http"));
    let _ = std::fs::remove_dir_all(&out);
}

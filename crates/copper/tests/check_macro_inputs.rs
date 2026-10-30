//! `copper check --macro-inputs`: the board's physical macro assets are
//! prepared and every input they bind is listed for a build runner.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn check(board: &str, listing: &Path) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["check", board, "--locked", "--offline", "--macro-inputs"])
        .arg(listing)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn macro_inputs_list_the_sources_and_footprints_a_macro_binds() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check-macro-inputs");
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let listing = directory.join("inputs.txt");
    let (status, stdout) = check("examples/macro_definition/board.copper", &listing);
    assert_eq!(status, 0, "{stdout}");
    let listed = std::fs::read_to_string(&listing).expect("the listing").replace('\\', "/");
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 2, "{listed}");
    assert!(lines.iter().any(|line| line.ends_with("examples/macro_definition/layouts/link.copper")), "{listed}");
    assert!(lines.iter().any(|line| line.ends_with("examples/macro_definition/test.kicad_mod")), "{listed}");
}

#[test]
fn boards_without_macros_list_nothing_and_unpreparable_macros_fail() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check-macro-inputs-empty");
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let listing = directory.join("inputs.txt");
    let (status, stdout) = check("examples/valid_board/board.copper", &listing);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(std::fs::read_to_string(&listing).expect("the listing"), "");
    // This corpus board imports a JSON asset that is not on disk.
    let (status, stdout) = check("tests/rust-port/export-kicad-pcb/macros/contracts.copper", &listing);
    assert_eq!(status, 2, "{stdout}");
    assert!(stdout.contains("MAC005"), "{stdout}");
}

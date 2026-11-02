//! `copper assembly bom` for a board fitted entirely by hand: no supplier BOM,
//! but the hand-assembly and ordering lists are still written.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Dumps, Value};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn an_all_consigned_board_writes_its_lists_and_no_supplier_bom() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("assembly-all-consigned");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let board = "examples/valid_board/board.copper";
    let lock = directory.join("assembly.lock");
    let lock_text = lock.to_str().expect("utf-8 path");
    let (status, stdout) = copper(&["assembly", "snapshot", board, "--locked", "--offline", "-o", lock_text]);
    assert_eq!(status, 0, "{stdout}");

    // Every selection fitted by hand with a full identity, or left empty
    // where the board gives it no footprint.
    let mut document = copper_json::parse(&std::fs::read_to_string(&lock).expect("the snapshot")).expect("json");
    let Value::Object(root) = &mut document else { panic!("a lock object") };
    let Some(Value::Array(selections)) = root.get_mut("selections") else { panic!("selections") };
    let count = selections.len();
    for selection in selections.iter_mut() {
        let Value::Object(selection) = selection else { panic!("a selection object") };
        let reference = selection.get("reference").and_then(Value::as_str).expect("reference").to_owned();
        let unset = |value: Option<&Value>| value.is_none_or(|value| *value == Value::Null);
        selection.insert("reviewed".into(), Value::from(true));
        if unset(selection.get("footprint")) {
            selection.insert("populate".into(), Value::from("none"));
            continue;
        }
        if unset(selection.get("manufacturer")) {
            selection.insert("manufacturer".into(), Value::from("Test Manufacturer"));
        }
        selection.insert("mpn".into(), Value::from(format!("MPN-{reference}").as_str()));
        selection.insert("populate".into(), Value::from("consigned"));
    }
    std::fs::write(&lock, Dumps::new().indent(2).dumps(&document)).expect("the lock");

    // A supplier BOM left over from an earlier run must not survive.
    let bom = directory.join("bom.csv");
    std::fs::write(&bom, "stale\n").expect("a stale BOM");
    let ordering = directory.join("ordering.csv");
    let hand = directory.join("hand-assembly.csv");
    let (status, stdout) = copper(&[
        "assembly",
        "bom",
        board,
        "--locked",
        "--offline",
        "--lock",
        lock_text,
        "-o",
        bom.to_str().expect("utf-8 path"),
        "--hand-assembly",
        hand.to_str().expect("utf-8 path"),
        "--ordering",
        ordering.to_str().expect("utf-8 path"),
    ]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.contains("No supplier-assembled components"), "{stdout}");
    assert!(!bom.exists(), "the stale supplier BOM was removed");
    let ordered = std::fs::read_to_string(&ordering).expect("the ordering list");
    let fitted = std::fs::read_to_string(&hand).expect("the hand-assembly list");
    assert!(ordered.lines().count() > 1 && fitted.lines().count() == count + 1, "{ordered}\n{fitted}");
}

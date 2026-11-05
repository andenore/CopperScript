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

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn path(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

fn read_lock(lock: &Path) -> Value {
    copper_json::parse(&std::fs::read_to_string(lock).expect("the lock")).expect("json")
}

fn selections(document: &mut Value) -> &mut Vec<Value> {
    let Value::Object(root) = document else { panic!("a lock object") };
    let Some(Value::Array(selections)) = root.get_mut("selections") else { panic!("selections") };
    selections
}

/// The reviewed flag of each selection, by reference.
fn reviewed(lock: &Path) -> Vec<(String, bool)> {
    selections(&mut read_lock(lock))
        .iter()
        .map(|selection| {
            let field = |key: &str| selection.get(key).cloned().expect("field");
            (field("reference").as_str().expect("reference").to_owned(), field("reviewed") == Value::from(true))
        })
        .collect()
}

/// A copy of the example board with `edit` applied, its snapshot with every
/// selection marked reviewed, and the paths of both.
fn reviewed_snapshot(name: &str) -> (PathBuf, PathBuf) {
    let directory = scratch(name);
    let board = directory.join("board.copper");
    std::fs::copy(workspace().join("examples/valid_board/board.copper"), &board).expect("the example board");
    let lock = directory.join("assembly.lock");
    let (status, stdout) = copper(&["assembly", "snapshot", path(&board), "--locked", "--offline", "-o", path(&lock)]);
    assert_eq!(status, 0, "{stdout}");
    let mut document = read_lock(&lock);
    for selection in selections(&mut document).iter_mut() {
        let Value::Object(selection) = selection else { panic!("a selection object") };
        selection.insert("reviewed".into(), Value::from(true));
    }
    std::fs::write(&lock, Dumps::new().indent(2).dumps(&document)).expect("the lock");
    (board, lock)
}

fn edit_board(board: &Path, edits: &[(&str, &str)]) {
    let mut source = std::fs::read_to_string(board).expect("the board");
    for (from, to) in edits {
        assert!(source.contains(from), "{from} in the example board");
        source = source.replace(from, to);
    }
    std::fs::write(board, source).expect("the edited board");
}

fn check(board: &Path, lock: &Path) -> String {
    copper(&["assembly", "check", path(board), "--locked", "--offline", "--lock", path(lock)]).1
}

fn update(board: &Path, lock: &Path) -> String {
    let (status, stdout) = copper(&["assembly", "update", path(board), "--locked", "--offline", "--lock", path(lock)]);
    assert_eq!(status, 0, "{stdout}");
    stdout
}

#[test]
fn snapshots_bind_each_selection_to_its_own_component() {
    let (_, lock) = reviewed_snapshot("assembly-v04-snapshot");
    let mut document = read_lock(&lock);
    let Value::Object(root) = &document else { panic!("a lock object") };
    assert_eq!(root.get("schema"), Some(&Value::from("copperscript-assembly-lock/v0.4")));
    assert!(!root.contains_key("electrical_sha256"), "no whole-board digest");
    let digests: Vec<String> = selections(&mut document)
        .iter()
        .map(|selection| selection.get("electrical_sha256").and_then(Value::as_str).expect("a digest").to_owned())
        .collect();
    assert!(digests.iter().all(|digest| digest.len() == 64));
    let mut unique = digests.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), digests.len(), "R1 and R2 differ by their connections");
}

#[test]
fn a_changed_component_alone_goes_stale_and_update_resets_only_its_review() {
    let (board, lock) = reviewed_snapshot("assembly-v04-changed");
    assert!(!check(&board, &lock).contains("STALE"));
    edit_board(&board, &[("value = 100nF;", "value = 1uF;")]);
    let stdout = check(&board, &lock);
    assert!(stdout.contains("STALE_SELECTION: C1:"), "{stdout}");
    assert_eq!(stdout.matches("STALE_SELECTION").count(), 1, "{stdout}");
    let stdout = update(&board, &lock);
    assert!(stdout.contains("1 changed (1 review(s) reset)"), "{stdout}");
    assert!(stdout.contains("CHANGED: C1: value, part data or connections changed (review reset)"), "{stdout}");
    for (reference, reviewed) in reviewed(&lock) {
        assert_eq!(reviewed, reference != "C1", "{reference}");
    }
    assert!(!check(&board, &lock).contains("STALE"));
}

#[test]
fn update_adds_new_components_and_drops_removed_ones() {
    let (board, lock) = reviewed_snapshot("assembly-v04-added");
    // Rename C1 everywhere: one component leaves the board and another arrives.
    edit_board(&board, &[("component C1:", "component C9:"), ("C1.", "C9."), ("max_distance(C1,", "max_distance(C9,")]);
    let stdout = update(&board, &lock);
    assert!(stdout.contains("1 added, 1 removed"), "{stdout}");
    assert!(stdout.contains("ADDED: C9:") && stdout.contains("REMOVED: C1:"), "{stdout}");
    let flags = reviewed(&lock);
    assert!(flags.contains(&("C9".to_owned(), false)) && !flags.iter().any(|(reference, _)| reference == "C1"));
    assert!(flags.iter().filter(|(reference, _)| reference != "C9").all(|(_, reviewed)| *reviewed));
}

#[test]
fn a_stale_legacy_lock_is_migrated_with_every_review_reset() {
    let (board, lock) = reviewed_snapshot("assembly-legacy");
    let mut document = read_lock(&lock);
    for selection in selections(&mut document).iter_mut() {
        let Value::Object(selection) = selection else { panic!("a selection object") };
        selection.shift_remove("electrical_sha256");
    }
    let Value::Object(root) = &mut document else { panic!("a lock object") };
    root.insert("schema".into(), Value::from("copperscript-assembly-lock/v0.2"));
    root.insert("electrical_sha256".into(), Value::from("0".repeat(64).as_str()));
    std::fs::write(&lock, Dumps::new().indent(2).dumps(&document)).expect("the legacy lock");
    assert!(check(&board, &lock).contains("STALE_LOCK"));
    let stdout = update(&board, &lock);
    assert!(stdout.contains("not verifiable: the legacy whole-board digest changed (review reset)"), "{stdout}");
    assert!(reviewed(&lock).iter().all(|(_, reviewed)| !reviewed));
    let stdout = check(&board, &lock);
    assert!(!stdout.contains("STALE") && stdout.contains("REVIEW_REQUIRED"), "{stdout}");
}

#[test]
fn a_layout_only_change_leaves_every_selection_current() {
    let (board, lock) = reviewed_snapshot("assembly-v04-layout");
    let source = std::fs::read_to_string(&board).expect("the board");
    let distance = source.split("max_distance(C1, U2.VDD)").nth(1).and_then(|rest| rest.split("distance = ").nth(1));
    let value = distance.and_then(|rest| rest.split(';').next()).expect("the decoupling distance").to_owned();
    edit_board(&board, &[(&format!("distance = {value};"), "distance = 7mm;")]);
    let stdout = check(&board, &lock);
    assert!(!stdout.contains("STALE"), "{stdout}");
    assert!(update(&board, &lock).contains(": 7 unchanged, 0 changed"));
    assert!(reviewed(&lock).iter().all(|(_, reviewed)| *reviewed));
}

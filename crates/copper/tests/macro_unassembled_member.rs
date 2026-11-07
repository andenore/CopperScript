//! A not-fitted part (`assembled = false`) binds a hard-macro member like a
//! fitted one: population is not footprint geometry. Physicalization renames
//! an unassembled footprint and excludes it from the BOM and position files,
//! which the macro's footprint identity check used to read as a different land.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a scratch directory");
    for entry in std::fs::read_dir(from).expect("the example directory") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a copied file");
        }
    }
}

#[test]
fn an_unassembled_part_binds_a_hard_macro_member() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("macro-unassembled-member");
    let _ = std::fs::remove_dir_all(&directory);
    copy_dir(&workspace().join("examples/macro_definition"), &directory);
    std::fs::write(
        directory.join("parts/unfitted.copper"),
        "part TestLandUnfitted {\n    footprint = \"../test.kicad_mod\";\n    assembled = false;\n    \
         pin P { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }\n}\n",
    )
    .expect("the unfitted part");
    let board = std::fs::read_to_string(directory.join("board.copper")).expect("the example board");
    let board = board.replace("component R1: parts.TestLand;", "component R1: parts.TestLandUnfitted;");
    assert!(board.contains("parts.TestLandUnfitted"), "{board}");
    std::fs::write(directory.join("board.copper"), board).expect("the edited board");

    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["route-board", "board.copper", "-o", "build/board.kicad_pcb", "--report", "build/route.json"])
        .args(["--stop-after", "placement", "--offline"])
        .current_dir(&directory)
        .output()
        .expect("the copper binary runs");
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    // A deliberately partial run: the macro bound and placement completed.
    assert!(!text.contains("footprint identity mismatch"), "{text}");
    assert!(text.contains("STOPPED after placement"), "{text}");
}

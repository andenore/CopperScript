//! A part's 3D fields (`height`, `model`, a connector category) bind a
//! hard-macro member like a plain part: they change how the footprint looks in
//! 3D, not its land. Physicalization renames such a footprint and records the
//! height, model and connector mark on it, which the macro's footprint identity
//! check used to read as a different land.

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
fn a_part_with_3d_fields_binds_a_hard_macro_member() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("macro-part-3d-member");
    let _ = std::fs::remove_dir_all(&directory);
    copy_dir(&workspace().join("examples/macro_definition"), &directory);
    std::fs::write(
        directory.join("parts/shaped.copper"),
        "part TestLandShaped {\n    category = \"connector.header\";\n    footprint = \"../test.kicad_mod\";\n    \
         height = 2.5 mm;\n    pin P { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }\n}\n",
    )
    .expect("the shaped part");
    let board = std::fs::read_to_string(directory.join("board.copper")).expect("the example board");
    let board = board.replace("component R1: parts.TestLand;", "component R1: parts.TestLandShaped;");
    assert!(board.contains("parts.TestLandShaped"), "{board}");
    std::fs::write(directory.join("board.copper"), board).expect("the edited board");

    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["route-board", "board.copper", "-o", "build/board.kicad_pcb", "--report", "build/route.json"])
        .args(["--stop-after", "placement", "--offline"])
        .current_dir(&directory)
        .output()
        .expect("the copper binary runs");
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(!text.contains("footprint identity mismatch"), "{text}");
    assert!(text.contains("STOPPED after placement"), "{text}");
    let pcb = std::fs::read_to_string(directory.join("build/board.kicad_pcb")).expect("the placed board");
    assert!(pcb.contains("\"CopperScriptHeightMM\" \"2.5\""), "the part height reaches the footprint");
    assert!(pcb.contains("\"CopperScriptBodyKind\" \"connector\""), "the connector mark reaches the footprint");
}

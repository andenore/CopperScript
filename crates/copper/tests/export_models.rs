//! `export-kicad-pcb` references a 3D model for every footprint: a generated
//! approximate body when the source has none.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

#[test]
fn generated_bodies_are_written_and_referenced() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("export-models");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let pcb = directory.join("b.kicad_pcb");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args([
            "export-kicad-pcb",
            "tests/rust-port/export-kicad-pcb/placement.copper",
            "--allow-proxy-footprints",
            "-o",
        ])
        .arg(&pcb)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("generated approximate bodies"), "{stdout}");

    let models: Vec<_> = std::fs::read_dir(directory.join("CopperScript.3dshapes"))
        .expect("the 3D model directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(models.len(), 6, "{models:?}");
    for model in &models {
        let text = std::fs::read_to_string(model).expect("a VRML file");
        assert!(text.starts_with("#VRML V2.0 utf8"), "{}", model.display());
        let reference = format!("${{KIPRJMOD}}/CopperScript.3dshapes/{}", model.file_name().unwrap().to_string_lossy());
        for file in [
            pcb.clone(),
            directory
                .join("CopperScript.pretty")
                .join(model.file_name().unwrap().to_string_lossy().replace(".wrl", ".kicad_mod")),
        ] {
            let content = std::fs::read_to_string(&file).expect("an exported file");
            assert!(content.contains(&reference), "{} lacks {reference}", file.display());
        }
    }
    let board = std::fs::read_to_string(&pcb).expect("the board");
    assert!(board.contains("(scale (xyz 0.39370078740157477"), "millimetre VRML needs KiCad's 1/2.54 scale");
}

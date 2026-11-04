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
    assert!(board.contains("(property \"CopperScriptBody\" \"${KIPRJMOD}/CopperScript.3dshapes/"));
    assert!(board.contains("(scale (xyz 0.39370078740157477"), "millimetre VRML needs KiCad's 1/2.54 scale");
}

fn export(name: &str, board: &str, files: &[(&str, &str)]) -> (String, PathBuf) {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("parts/models")).expect("a scratch directory");
    for (path, text) in files {
        std::fs::write(directory.join(path), text).expect("a fixture file");
    }
    std::fs::write(directory.join("board.copper"), board).expect("the board");
    let pcb = directory.join("out/b.kicad_pcb");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["export-kicad-pcb"])
        .arg(directory.join("board.copper"))
        .args(["--allow-proxy-footprints", "-o"])
        .arg(&pcb)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs");
    let stdout = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{stdout}");
    (stdout, pcb.parent().unwrap().to_path_buf())
}

const BOARD: &str = "board B {\n    import p \"./parts\";\n    component U1: p.Widget { footprint = \"0805\"; }\n    component U2: p.Widget { footprint = \"0805\"; }\n    net N { U1.A; U2.A; }\n    net M { U1.B; U2.B; }\n}\n";

fn widget(extra: &str) -> String {
    format!(
        "part Widget {{\n{extra}\n    pin A {{ number = \"1\"; domains = \"analog\"; directions = \"passive\"; }}\n    pin B {{ number = \"2\"; domains = \"analog\"; directions = \"passive\"; }}\n}}\n"
    )
}

#[test]
fn a_part_model_file_is_copied_and_its_height_reaches_the_footprint() {
    let vrml = "#VRML V2.0 utf8\nShape { geometry IndexedFaceSet { coord Coordinate { point [ 0 0 0, 1 0 0, 0 1 0 ] } coordIndex [ 0, 1, 2, -1 ] } }\n";
    let part = widget("    model = \"models/w.wrl\";\n    height = 3 mm;");
    let (stdout, out) =
        export("export-part-model", BOARD, &[("parts/widget.copper", part.as_str()), ("parts/models/w.wrl", vrml)]);
    assert!(!stdout.contains("not found"), "{stdout}");
    let shapes: Vec<_> = std::fs::read_dir(out.join("CopperScript.3dshapes"))
        .expect("the model directory")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    let copied: Vec<&String> = shapes.iter().filter(|name| name.ends_with("_w.wrl")).collect();
    assert_eq!(copied.len(), 1, "{shapes:?}");
    assert_eq!(std::fs::read_to_string(out.join("CopperScript.3dshapes").join(copied[0])).unwrap(), vrml);
    let board = std::fs::read_to_string(out.join("b.kicad_pcb")).unwrap();
    assert!(board.contains(&format!("(model \"${{KIPRJMOD}}/CopperScript.3dshapes/{}\"", copied[0])), "{board}");
    assert!(board.contains("(scale (xyz 1 1 1))"), "a user model is not rescaled");
    assert!(board.contains("\"CopperScriptHeightMM\" \"3\""), "part height reaches the footprint");
    assert!(!stdout.contains("generated approximate bodies"), "{stdout}");
}

#[test]
fn a_missing_part_model_warns_and_falls_back_to_a_generated_body() {
    let part = widget("    model = \"models/absent.step\";");
    let (stdout, out) = export("export-part-model-missing", BOARD, &[("parts/widget.copper", part.as_str())]);
    assert!(stdout.contains("not found; using a generated body"), "{stdout}");
    assert!(stdout.contains("generated approximate bodies"), "{stdout}");
    let board = std::fs::read_to_string(out.join("b.kicad_pcb")).unwrap();
    assert!(board.contains("(scale (xyz 0.39370078740157477"));
}

#[test]
fn a_part_model_outside_its_part_directory_is_not_copied() {
    // `secret.wrl` sits beside the board, one level above the part file.
    let vrml = "#VRML V2.0 utf8\n# secret\n";
    let part = widget("    model = \"../secret.wrl\";");
    let (stdout, out) =
        export("export-part-model-escape", BOARD, &[("parts/widget.copper", part.as_str()), ("secret.wrl", vrml)]);
    assert!(stdout.contains("outside its part file's directory; using a generated body"), "{stdout}");
    for entry in std::fs::read_dir(out.join("CopperScript.3dshapes")).expect("the model directory") {
        let path = entry.expect("entry").path();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), vrml, "{} is the escaped file", path.display());
    }
}

#[test]
fn a_part_model_that_is_not_a_model_file_is_not_copied() {
    let part = widget("    model = \"models/notes.txt\";");
    let (stdout, out) = export(
        "export-part-model-text",
        BOARD,
        &[("parts/widget.copper", part.as_str()), ("parts/models/notes.txt", "private\n")],
    );
    assert!(stdout.contains("is not a STEP or VRML file; using a generated body"), "{stdout}");
    let shapes: Vec<_> = std::fs::read_dir(out.join("CopperScript.3dshapes")).expect("the model directory").collect();
    assert!(
        shapes.iter().all(|entry| !entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".txt")),
        "{shapes:?}"
    );
}

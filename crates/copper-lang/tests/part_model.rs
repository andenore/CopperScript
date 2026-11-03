//! A part's `model` and `height` fields reach `PartDefinition.metadata`.

use std::fs;
use std::path::PathBuf;

use copper_lang::compiler::compile_design_file;
use copper_lang::lower::{PART_HEIGHT_KEY, PART_MODEL_KEY};

fn board(name: &str, part_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-part-model-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    fs::write(
        dir.join("parts/widget.copper"),
        format!(
            "part Widget {{\n{part_body}\n    pin A {{ number = \"1\"; domains = \"analog\"; directions = \"passive\"; }}\n    pin B {{ number = \"2\"; domains = \"analog\"; directions = \"passive\"; }}\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("board.copper"),
        "board B {\n    import p \"./parts\";\n    component U1: p.Widget;\n    net N { U1.A; }\n    net M { U1.B; }\n}\n",
    )
    .unwrap();
    dir.join("board.copper")
}

fn error_of(name: &str, body: &str) -> String {
    match compile_design_file(&board(name, body), false, false) {
        Ok(_) => panic!("expected an error"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn model_and_height_are_kept_on_the_part() {
    let design =
        compile_design_file(&board("ok", "    model = \"models/widget.step\";\n    height = 1.25 mm;"), false, false)
            .expect("compiles");
    let part = design.electrical.library.values().next().expect("one part");
    assert_eq!(part.metadata.get(PART_MODEL_KEY).map(String::as_str), Some("models/widget.step"));
    assert_eq!(part.metadata.get(PART_HEIGHT_KEY).map(String::as_str), Some("1250000"));
}

#[test]
fn absent_fields_add_no_metadata() {
    let design = compile_design_file(&board("none", ""), false, false).expect("compiles");
    assert!(design.electrical.library.values().all(|part| part.metadata.is_empty()));
}

#[test]
fn height_must_be_a_positive_length() {
    assert!(error_of("h0", "    height = 0 mm;").contains("part height"));
    assert!(error_of("hv", "    height = 1 V;").contains("part height"));
    assert!(error_of("hs", "    height = \"tall\";").contains("part height"));
}

#[test]
fn model_must_be_a_string() {
    assert!(error_of("m1", "    model = 3;").contains("part model"));
}

//! Footprint `(model ...)` references against a required KiCad 3D library: current and legacy variables,
//! `.wrl` to `.step` fallback, and what happens without a library.

use std::path::{Path, PathBuf};

use copper_lang::footprints::FootprintResolver;
use copper_physical::model3d::footprint_models;

const MODULE: &str = "github.com/KiCad/kicad-packages3D";
const STEP: &str = "ISO-10303-21;\nEND-ISO-10303-21;\n";

fn footprint(models: &[&str]) -> String {
    let lines: String = models.iter().map(|path| format!(" (model {path})\n")).collect();
    format!(
        "(footprint \"Part\" (version 20240108) (generator \"test\") (layer \"F.Cu\")\n (attr smd)\n \
         (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n{lines})\n"
    )
}

fn project(name: &str, with_library: bool, models: &[&str]) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("models/Lib.3dshapes")).unwrap();
    std::fs::write(directory.join("models/Lib.3dshapes/Only.step"), STEP).unwrap();
    std::fs::write(directory.join("Part.kicad_mod"), footprint(models)).unwrap();
    let mut module = String::from("module github.com/example/project\n\n");
    if with_library {
        module.push_str(&format!("require {MODULE} 10.0.8\nreplace {MODULE} => ./models\n"));
    }
    std::fs::write(directory.join("copper.mod"), module).unwrap();
    directory
}

fn models_of(directory: &Path) -> (Vec<String>, Vec<String>) {
    let mut resolver = FootprintResolver::new(directory, &[], false, false, true);
    let result = resolver.resolve("Part.kicad_mod").expect("the footprint resolves");
    (footprint_models(&result.footprint).into_iter().map(|model| model.path).collect(), result.warnings)
}

#[test]
fn a_legacy_wrl_reference_resolves_to_the_librarys_step_and_is_locked() {
    let directory = project("fp-models-legacy", true, &["\"${KISYS3DMOD}/Lib.3dshapes/Only.wrl\""]);
    let (paths, warnings) = models_of(&directory);
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(paths[0].replace('\\', "/").ends_with("models/Lib.3dshapes/Only.step"), "{paths:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
    let lock = std::fs::read_to_string(directory.join("copper.lock")).expect("the lock is written");
    assert!(lock.contains("Lib.3dshapes/Only.step"), "{lock}");
}

#[test]
fn a_current_reference_resolves_through_the_library_too() {
    let directory = project("fp-models-current", true, &["\"${KICAD10_3DMODEL_DIR}/Lib.3dshapes/Only.step\""]);
    let (paths, _) = models_of(&directory);
    assert!(paths[0].replace('\\', "/").ends_with("models/Lib.3dshapes/Only.step"), "{paths:?}");
}

#[test]
fn a_model_the_library_lacks_is_dropped_with_a_warning() {
    let directory = project("fp-models-missing", true, &["\"${KISYS3DMOD}/Lib.3dshapes/Gone.wrl\""]);
    let (paths, warnings) = models_of(&directory);
    assert!(paths.is_empty(), "{paths:?}");
    assert!(
        warnings.iter().any(|warning| warning.contains("unavailable") && warning.contains("generated body")),
        "{warnings:?}"
    );
}

#[test]
fn without_a_library_legacy_references_are_dropped_and_current_ones_kept() {
    let directory = project(
        "fp-models-no-library",
        false,
        &["\"${KISYS3DMOD}/Lib.3dshapes/Only.wrl\"", "\"${KICAD10_3DMODEL_DIR}/Lib.3dshapes/Only.step\""],
    );
    let (paths, warnings) = models_of(&directory);
    assert_eq!(paths, vec!["${KICAD10_3DMODEL_DIR}/Lib.3dshapes/Only.step".to_owned()]);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!directory.join("copper.lock").exists(), "nothing to lock without a library");
}

#[test]
fn an_unquoted_legacy_path_is_read_like_a_quoted_one() {
    let directory = project("fp-models-unquoted", true, &["${KISYS3DMOD}/Lib.3dshapes/Only.wrl (at (xyz 0 0 0))"]);
    let (paths, _) = models_of(&directory);
    assert_eq!(paths.len(), 1, "{paths:?}");
}

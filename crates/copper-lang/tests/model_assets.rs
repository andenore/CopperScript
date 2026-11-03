//! Single-file 3D model resolution from a module too large to clone: the lock
//! lists exactly the models used, and locked builds check them.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_lang::packages::{PackageResolver, is_sparse_module};
use copper_lang::syntax::Location;

const MODULE: &str = "github.com/KiCad/kicad-packages3D";
const STEP: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nEND-ISO-10303-21;\n";

fn project(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("models/Connector_USB.3dshapes")).unwrap();
    std::fs::write(directory.join("models/Connector_USB.3dshapes/USB.step"), STEP).unwrap();
    std::fs::write(directory.join("models/Connector_USB.3dshapes/Other.step"), format!("{STEP}-- other\n")).unwrap();
    std::fs::write(
        directory.join("copper.mod"),
        format!("module github.com/example/project\n\nrequire {MODULE} 10.0.8\nreplace {MODULE} => ./models\n"),
    )
    .unwrap();
    std::fs::write(directory.join("board.copper"), "board B {}\n").unwrap();
    directory
}

fn resolver(directory: &Path, locked: bool) -> PackageResolver {
    PackageResolver::for_source(&directory.join("board.copper"), locked, true).expect("a resolver")
}

fn location(directory: &Path) -> Location {
    Location {
        filename: directory.join("board.copper").to_string_lossy().as_ref().into(),
        offset: 0,
        line: 1,
        column: 1,
    }
}

fn identity(file: &str) -> String {
    format!("{MODULE}/Connector_USB.3dshapes/{file}")
}

#[test]
fn only_the_kicad_model_library_is_sparse() {
    assert!(is_sparse_module("github.com/KiCad/kicad-packages3D"));
    assert!(is_sparse_module("gitlab.com/kicad/libraries/kicad-packages3D"));
    assert!(!is_sparse_module("github.com/KiCad/kicad-footprints"));
    assert!(!is_sparse_module("github.com/andenore/CopperLib"));
}

#[test]
fn a_used_model_is_locked_alone_and_checked_afterwards() {
    let directory = project("model-assets-lock");
    let at = location(&directory);
    let asset = resolver(&directory, false).resolve_model_asset(&identity("USB.step"), &at).expect("resolves");
    assert!(asset.path.ends_with("USB.step"));
    let lock = std::fs::read_to_string(directory.join("copper.lock")).expect("the lock is written");
    assert!(lock.contains("Connector_USB.3dshapes/USB.step"), "{lock}");
    assert!(!lock.contains("Other.step"), "unused models stay out of the lock: {lock}");

    // Locked: the recorded model passes, an unrecorded one is refused, a changed one is refused.
    let mut locked = resolver(&directory, true);
    assert_eq!(locked.resolve_model_asset(&identity("USB.step"), &at).expect("locked").sha256, asset.sha256);
    let missing = locked.resolve_model_asset(&identity("Other.step"), &at).expect_err("not in the lock");
    assert_eq!(missing.code, "PKG008", "{missing}");
    assert!(missing.message.contains("run copper lock"), "{missing}");
    std::fs::write(directory.join("models/Connector_USB.3dshapes/USB.step"), "changed").unwrap();
    let changed = resolver(&directory, true).resolve_model_asset(&identity("USB.step"), &at).expect_err("changed");
    assert_eq!(changed.code, "PKG008", "{changed}");
}

#[test]
fn unlocked_resolution_adds_models_one_by_one_in_sorted_order() {
    let directory = project("model-assets-grow");
    let at = location(&directory);
    let mut first = resolver(&directory, false);
    first.resolve_model_asset(&identity("USB.step"), &at).unwrap();
    first.resolve_model_asset(&identity("Other.step"), &at).unwrap();
    let lock = std::fs::read_to_string(directory.join("copper.lock")).unwrap();
    assert!(lock.find("Other.step").unwrap() < lock.find("USB.step").unwrap(), "{lock}");
    // Both are now accepted under --locked.
    let mut locked = resolver(&directory, true);
    assert!(locked.resolve_model_asset(&identity("Other.step"), &at).is_ok());
    assert!(locked.resolve_model_asset(&identity("USB.step"), &at).is_ok());
}

#[test]
fn bad_model_identities_are_refused() {
    let directory = project("model-assets-bad");
    let at = location(&directory);
    let mut resolver = resolver(&directory, false);
    for (path, code) in [
        (identity("USB.txt"), "PKG004"),
        (identity("../USB.step"), "PKG004"),
        (MODULE.to_owned(), "PKG004"),
        (format!("{MODULE}/Connector_USB.3dshapes/Missing.step"), "PKG004"),
        ("github.com/other/module/Lib.3dshapes/a.step".to_owned(), "PKG002"),
        ("/etc/passwd".to_owned(), "PKG004"),
    ] {
        let error = resolver.resolve_model_asset(&path, &at).expect_err(&path);
        assert_eq!(error.code, code, "{path}: {error}");
    }
}

#[test]
fn a_remote_model_comes_from_a_blobless_clone_one_file_at_a_time() {
    let directory = project("model-assets-remote");
    let source = directory.join("source");
    std::fs::create_dir_all(source.join("Lib.3dshapes")).unwrap();
    std::fs::write(source.join("Lib.3dshapes/a.step"), STEP).unwrap();
    std::fs::write(source.join("Lib.3dshapes/b.step"), "b").unwrap();
    let git = |arguments: &[&str]| {
        let output = Command::new("git").args(arguments).current_dir(&source).output().expect("git runs");
        assert!(output.status.success(), "git {arguments:?}: {}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "--quiet", "-b", "main"]);
    git(&["config", "uploadpack.allowFilter", "true"]);
    git(&["config", "user.name", "T"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "models"]);
    let url = format!("file://{}", source.to_string_lossy().replace('\\', "/"));
    let cache = directory.join("cache");
    let at = location(&directory);
    let bytes = copper_lang::packages::sparse_git_blob_for_tests(&url, "main", "Lib.3dshapes/a.step", &cache, &at)
        .expect("one blob");
    assert_eq!(bytes, STEP.as_bytes());
    let again = copper_lang::packages::sparse_git_blob_for_tests(&url, "main", "Lib.3dshapes/b.step", &cache, &at)
        .expect("a second file reuses the clone");
    assert_eq!(again, b"b");
    let missing = copper_lang::packages::sparse_git_blob_for_tests(&url, "main", "Lib.3dshapes/none.step", &cache, &at)
        .expect_err("absent");
    assert_eq!(missing.code, "PKG004");
    // The clone holds no checked-out files: it is a store, not a copy of the library.
    assert!(!cache.join("repo/Lib.3dshapes").exists());
}

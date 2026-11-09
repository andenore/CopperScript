//! `docs/schemas.md` lists exactly the schema identifiers the crates' sources name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("a readable directory").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Each `"copperscript…/vN"` or `"copper-…/vN"` string literal in `text`.
fn identifiers(text: &str, into: &mut BTreeSet<String>) {
    for (index, _) in text.match_indices('"') {
        let rest = &text[index + 1..];
        let Some(end) = rest.find('"') else { continue };
        let literal = &rest[..end];
        let Some((name, version)) = literal.split_once("/v") else { continue };
        let named = (name.starts_with("copperscript") || name.starts_with("copper-"))
            && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte));
        let versioned = version.starts_with(|c: char| c.is_ascii_digit())
            && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.');
        if named && versioned {
            into.insert(literal.to_owned());
        }
    }
}

#[test]
fn the_schema_registry_matches_the_sources() {
    let root = workspace();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        let source = entry.path().join("src");
        if source.is_dir() {
            sources(&source, &mut files);
        }
    }
    let mut named = BTreeSet::new();
    for file in &files {
        identifiers(&std::fs::read_to_string(file).unwrap(), &mut named);
    }
    let registry = std::fs::read_to_string(root.join("docs/schemas.md")).unwrap();
    let listed: BTreeSet<String> = registry
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split_once('`').map(|(schema, _)| schema.to_owned()))
        .collect();
    let missing: Vec<&String> = named.difference(&listed).collect();
    let stale: Vec<&String> = listed.difference(&named).collect();
    assert!(missing.is_empty(), "schemas the sources name but docs/schemas.md does not list: {missing:?}");
    assert!(stale.is_empty(), "schemas docs/schemas.md lists but no source names: {stale:?}");
}

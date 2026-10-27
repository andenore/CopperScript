//! Package resolution against what Python recorded: every `copper.lock` in
//! the repository must round-trip byte for byte, and module inventories must
//! reproduce the locked file lists and checksums.

use std::path::{Path, PathBuf};

use copper_lang::packages::{
    PackageResolver, inventory_checksum, lock_text, module_inventory, read_lock, read_manifest,
};
use copper_lang::syntax::Location;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn lock_files() -> Vec<PathBuf> {
    let mut found = vec![repository().join("copper.lock")];
    for entry in std::fs::read_dir(repository().join("examples")).unwrap() {
        let candidate = entry.unwrap().path().join("copper.lock");
        if candidate.is_file() {
            found.push(candidate);
        }
    }
    found.sort();
    found
}

#[test]
fn locks_written_by_python_round_trip() {
    for path in lock_files() {
        let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let entries = read_lock(&path).unwrap();
        assert!(!entries.is_empty(), "{}", path.display());
        assert_eq!(lock_text(&entries), text, "{} re-serialized differently", path.display());
    }
}

#[test]
fn inventories_match_python_checksums() {
    let mut checked = 0;
    for lock in lock_files() {
        let root = lock.parent().unwrap();
        let manifest = read_manifest(&root.join("copper.mod")).unwrap();
        for ((module, version), entry) in read_lock(&lock).unwrap() {
            let directory = if let Some(replacement) = manifest.replacements.get(&module) {
                root.join(replacement)
            } else {
                // A cached Git module, present only after an online resolution.
                let resolver = PackageResolver::new(manifest.clone(), true, true).unwrap();
                let location = Location { filename: lock.to_string_lossy().into(), offset: 0, line: 1, column: 1 };
                let mut resolver = resolver;
                match resolver.resolve(&module, &location) {
                    Ok(package) => package.directory,
                    Err(error) if error.code == "PKG011" => continue,
                    Err(error) => panic!("{module}@{version}: {error}"),
                }
            };
            let files = module_inventory(&directory).unwrap();
            assert_eq!(files, entry.files, "{module}@{version} inventory in {}", directory.display());
            assert_eq!(inventory_checksum(&files), entry.checksum, "{module}@{version}");
            checked += 1;
        }
    }
    assert!(checked >= 1, "no module inventory was available to check");
}

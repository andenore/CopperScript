//! The build identity of `copper`: the crate version and a SHA-256 of the
//! workspace's Rust sources (`crates/**/*.rs`, LF-normalised, in path order),
//! so a placement checkpoint records which build made it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                sources(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates/").to_path_buf();
    println!("cargo:rerun-if-changed={}", crates.display());
    let mut found = Vec::new();
    sources(&crates, &mut found);
    let mut named: Vec<(String, PathBuf)> = found
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&crates).expect("under crates/");
            let name = relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>();
            (name.join("/"), path)
        })
        .collect();
    named.sort();
    let mut digest = Sha256::new();
    for (name, path) in named {
        let bytes = std::fs::read(&path).expect("a readable source");
        let mut normalised = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
                continue;
            }
            normalised.push(bytes[index]);
            index += 1;
        }
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(&normalised);
        digest.update([0]);
    }
    let hex: String = digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    println!("cargo:rustc-env=COPPER_SOURCE_DIGEST=sha256:{hex}");
}

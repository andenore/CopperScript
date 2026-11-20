//! SPICE model files that parts name in `sim spice` blocks.
//!
//! A local `file = "..."` is the user's own file, resolved against the
//! directory of the source file that declares the part. A vendor model is a
//! `source = "https://..."` URL pinned by `sha256`: vendor licences generally
//! forbid redistribution, so the file is never part of a repository. `copper
//! models fetch BOARD` downloads it into the project's `.copper-cache/spice/`
//! (beside the nearest `copper.mod`, like the 3D models of
//! [`crate::remote_models`]), refusing bytes whose SHA-256 differs; `copper
//! sim` only reads that cache and never downloads, so a run without the
//! cached file fails and names the command to run. With `member`, the URL is
//! a ZIP archive (as Texas Instruments distributes its models): the hash
//! pins the archive, and the member is extracted into the same cache
//! directory on first use ([`crate::zip_member`]).

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::model::PartDefinition;
use crate::remote_models::{sha256_hex, url_file_name};
use crate::simulation::{SimKind, SpiceModel, SpiceSource};

/// `.copper-cache/spice` beside the nearest `copper.mod` at or above `start`,
/// else beside `start` itself.
pub fn cache_directory(start: &Path) -> PathBuf {
    let directory = if start.is_dir() { start } else { start.parent().unwrap_or(start) };
    let root = crate::packages::find_manifest(directory)
        .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| directory.to_path_buf());
    root.join(".copper-cache").join("spice")
}

/// A pinned vendor model file and the parts that name it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteSpiceModel {
    pub url: String,
    pub sha256: String,
    pub parts: Vec<String>,
    /// The directory the cache is found from (the declaring file's).
    pub directory: String,
}

impl RemoteSpiceModel {
    /// The cache directory for this model's declaring file.
    pub fn cache(&self) -> PathBuf {
        cache_directory(Path::new(&self.directory))
    }

    /// Where the fetched file lives in `cache`: `<sha256>/<URL file name>`.
    pub fn cache_path(&self, cache: &Path) -> PathBuf {
        cache.join(&self.sha256).join(url_file_name(&self.url))
    }

    /// The cached file if present and still matching `sha256`.
    pub fn cached(&self, cache: &Path) -> Result<Option<PathBuf>, String> {
        let path = self.cache_path(cache);
        match std::fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == self.sha256 => Ok(Some(path)),
            Ok(bytes) => Err(format!(
                "cached SPICE model {} has SHA-256 {}, not the sha256 {} the part declares; delete it and run copper models fetch",
                crate::paths::display(&path),
                sha256_hex(&bytes),
                self.sha256
            )),
            Err(_) => Ok(None),
        }
    }

    /// Check `bytes` against `sha256` and write them to the cache atomically.
    pub fn store(&self, cache: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
        let actual = sha256_hex(bytes);
        if actual != self.sha256 {
            return Err(format!(
                "{} has SHA-256 {actual}, not the sha256 {} the part's sim spice block declares; nothing was stored",
                self.url, self.sha256
            ));
        }
        let path = self.cache_path(cache);
        let directory = path.parent().expect("a cache directory");
        std::fs::create_dir_all(directory)
            .map_err(|failure| format!("cannot create {}: {failure}", crate::paths::display(directory)))?;
        let temporary = directory.join(format!(".download-{}.part", std::process::id()));
        std::fs::write(&temporary, bytes)
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|failure| format!("cannot write {}: {failure}", crate::paths::display(&path)))?;
        Ok(path)
    }
}

/// Every pinned vendor model the parts of `library` name, merged by URL and hash.
pub fn library_spice_models(library: &IndexMap<String, PartDefinition>) -> Vec<RemoteSpiceModel> {
    let mut models: Vec<RemoteSpiceModel> = Vec::new();
    for (name, part) in library {
        for block in &part.sim {
            let Some(SpiceModel { source: SpiceSource::Url { url, sha256, .. }, directory, .. }) = &block.spec.spice
            else {
                continue;
            };
            if block.spec.kind != SimKind::Spice {
                continue;
            }
            match models.iter_mut().find(|m| m.url == *url && m.sha256 == *sha256) {
                Some(known) => {
                    if !known.parts.contains(name) {
                        known.parts.push(name.clone());
                    }
                }
                None => models.push(RemoteSpiceModel {
                    url: url.clone(),
                    sha256: sha256.clone(),
                    parts: vec![name.clone()],
                    directory: directory.clone(),
                }),
            }
        }
    }
    models
}

/// The file a `sim spice` block's model is read from, and its SHA-256:
/// a local file resolved against the declaring directory, or the cached
/// vendor file. Errors say what is missing and how to get it.
pub fn resolve(model: &SpiceModel) -> Result<(PathBuf, String), String> {
    match &model.source {
        SpiceSource::File(file) => {
            let path = Path::new(&model.directory).join(file);
            let bytes = std::fs::read(&path).map_err(|failure| {
                format!("the SPICE model file {} cannot be read: {failure}", crate::paths::display(&path))
            })?;
            Ok((path, sha256_hex(&bytes)))
        }
        SpiceSource::Url { url, sha256, member } => {
            let remote = RemoteSpiceModel {
                url: url.clone(),
                sha256: sha256.clone(),
                parts: Vec::new(),
                directory: model.directory.clone(),
            };
            let cache = remote.cache();
            let Some(path) = remote.cached(&cache)? else {
                return Err(format!(
                    "the vendor SPICE model {url} is not in the cache {}; run copper models fetch BOARD (copper sim never downloads)",
                    crate::paths::display(&remote.cache_path(&cache))
                ));
            };
            let Some(member) = member else { return Ok((path, sha256.clone())) };
            let leaf = member.rsplit(['/', '\\']).next().unwrap_or(member);
            let extracted = path.with_file_name(format!("member-{}", url_file_name(leaf)));
            let bytes = match std::fs::read(&extracted) {
                Ok(bytes) => bytes,
                Err(_) => {
                    let archive = std::fs::read(&path)
                        .map_err(|failure| format!("cannot read {}: {failure}", crate::paths::display(&path)))?;
                    let bytes = crate::zip_member::zip_member(&archive, member)
                        .map_err(|failure| format!("{url}: {failure}"))?;
                    std::fs::write(&extracted, &bytes)
                        .map_err(|failure| format!("cannot write {}: {failure}", crate::paths::display(&extracted)))?;
                    bytes
                }
            };
            Ok((extracted, sha256_hex(&bytes)))
        }
    }
}

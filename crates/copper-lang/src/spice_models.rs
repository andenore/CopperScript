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
//!
//! A vendor file that cannot be fetched automatically (a site that refuses
//! scripted downloads, a licence to accept first) is declared with `sha256`
//! and a `download = "..."` note saying where to get it. The user downloads
//! it by hand and registers it with `copper models add BOARD FILE`, which
//! checks the hash and copies the file into the same cache layout,
//! `.copper-cache/spice/<sha256>/<file name>`; from there it resolves like a
//! fetched file, so the board builds the same everywhere the same bytes are
//! registered.

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
    /// The URL `copper models fetch` downloads; empty for a file registered by hand.
    pub url: String,
    pub sha256: String,
    pub parts: Vec<String>,
    /// The directory the cache is found from (the declaring file's).
    pub directory: String,
    /// For a file registered by hand: the part's note of where to download it.
    pub download: Option<String>,
}

impl RemoteSpiceModel {
    /// The cache directory for this model's declaring file.
    pub fn cache(&self) -> PathBuf {
        cache_directory(Path::new(&self.directory))
    }

    /// True for a file the user downloads by hand and registers with `copper models add`.
    pub fn registered(&self) -> bool {
        self.download.is_some()
    }

    /// Where the fetched file lives in `cache`: `<sha256>/<URL file name>`
    /// (for a registered file, the directory `<sha256>/` it is registered in).
    pub fn cache_path(&self, cache: &Path) -> PathBuf {
        if self.registered() {
            return cache.join(&self.sha256);
        }
        cache.join(&self.sha256).join(url_file_name(&self.url))
    }

    /// The cached file if present and still matching `sha256`.
    pub fn cached(&self, cache: &Path) -> Result<Option<PathBuf>, String> {
        if self.registered() {
            return registered_file(cache, &self.sha256);
        }
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

    /// Check `bytes` against `sha256` and write them to the cache atomically
    /// (a fetched file under its URL's file name).
    pub fn store(&self, cache: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
        let name = url_file_name(&self.url);
        self.store_as(cache, bytes, &name, &self.url)
    }

    /// Register a file downloaded by hand: check `bytes` against `sha256` and
    /// copy them into the cache as `<sha256>/<name>`.
    pub fn register(&self, cache: &Path, bytes: &[u8], name: &str) -> Result<PathBuf, String> {
        // A file name of the cache: no directories, no names the cache uses itself.
        let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let leaf = if leaf.is_empty() || leaf.starts_with('.') || leaf.starts_with("member-") {
            format!("model-{leaf}")
        } else {
            leaf.to_owned()
        };
        self.store_as(cache, bytes, &url_file_name(&leaf), name)
    }

    fn store_as(&self, cache: &Path, bytes: &[u8], name: &str, what: &str) -> Result<PathBuf, String> {
        let actual = sha256_hex(bytes);
        if actual != self.sha256 {
            return Err(format!(
                "{what} has SHA-256 {actual}, not the sha256 {} the part's sim spice block declares; nothing was stored",
                self.sha256
            ));
        }
        let directory = cache.join(&self.sha256);
        let path = directory.join(name);
        std::fs::create_dir_all(&directory)
            .map_err(|failure| format!("cannot create {}: {failure}", crate::paths::display(&directory)))?;
        let temporary = directory.join(format!(".download-{}.part", std::process::id()));
        std::fs::write(&temporary, bytes)
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|failure| format!("cannot write {}: {failure}", crate::paths::display(&path)))?;
        Ok(path)
    }
}

/// The file registered by hand in `cache/<sha256>/` whose content has that
/// hash (extracted archive members and temporary files aside); an error when
/// the directory holds files but none of them matches.
fn registered_file(cache: &Path, sha256: &str) -> Result<Option<PathBuf>, String> {
    let directory = cache.join(sha256);
    let Ok(entries) = std::fs::read_dir(&directory) else { return Ok(None) };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            !name.starts_with('.') && !name.starts_with("member-")
        })
        .collect();
    files.sort();
    let mut seen = Vec::new();
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let actual = sha256_hex(&bytes);
        if actual == sha256 {
            return Ok(Some(path));
        }
        seen.push(format!("{} (SHA-256 {actual})", crate::paths::display(&path)));
    }
    if seen.is_empty() {
        return Ok(None);
    }
    Err(format!(
        "the cache directory {} holds {} but no file with the sha256 {sha256} the part declares; delete it and run copper models add BOARD FILE with the right file",
        crate::paths::display(&directory),
        seen.join(", ")
    ))
}

/// Every pinned vendor model the parts of `library` name (fetched by URL or
/// registered by hand), merged by URL and hash.
pub fn library_spice_models(library: &IndexMap<String, PartDefinition>) -> Vec<RemoteSpiceModel> {
    let mut models: Vec<RemoteSpiceModel> = Vec::new();
    for (name, part) in library {
        for block in &part.sim {
            if block.spec.kind != SimKind::Spice {
                continue;
            }
            let Some(model @ SpiceModel { source, .. }) = &block.spec.spice else { continue };
            let directory = cache_root(model);
            let (url, sha256, download) = match source {
                SpiceSource::Url { url, sha256, .. } => (url.clone(), sha256.clone(), None),
                SpiceSource::Registered { sha256, download, .. } => (String::new(), sha256.clone(), Some(download)),
                SpiceSource::File(_) => continue,
            };
            match models.iter_mut().find(|m| m.url == url && m.sha256 == sha256) {
                Some(known) => {
                    if !known.parts.contains(name) {
                        known.parts.push(name.clone());
                    }
                }
                None => models.push(RemoteSpiceModel {
                    url,
                    sha256,
                    parts: vec![name.clone()],
                    directory,
                    download: download.cloned(),
                }),
            }
        }
    }
    models
}

/// The directory a vendor model's cache is found from: the bound board's
/// project ([`SpiceModel::project`]), else the declaring file's.
pub fn cache_root(model: &SpiceModel) -> String {
    model.project.clone().unwrap_or_else(|| model.directory.clone())
}

/// The file a `sim spice` block's model is read from, and its SHA-256:
/// a local file resolved against the declaring directory, or the cached
/// vendor file. Errors say what is missing and how to get it; a missing
/// file registered by hand is `SIM108` with the part's download note.
pub fn resolve(model: &SpiceModel) -> Result<(PathBuf, String), String> {
    let (url, sha256, member, download) = match &model.source {
        SpiceSource::File(file) => {
            let path = Path::new(&model.directory).join(file);
            let bytes = std::fs::read(&path).map_err(|failure| {
                format!("the SPICE model file {} cannot be read: {failure}", crate::paths::display(&path))
            })?;
            return Ok((path, sha256_hex(&bytes)));
        }
        SpiceSource::Url { url, sha256, member } => (url.clone(), sha256, member, None),
        SpiceSource::Registered { sha256, download, member } => (String::new(), sha256, member, Some(download)),
    };
    let remote = RemoteSpiceModel {
        url: url.clone(),
        sha256: sha256.clone(),
        parts: Vec::new(),
        directory: cache_root(model),
        download: download.cloned(),
    };
    let cache = remote.cache();
    let Some(path) = remote.cached(&cache)? else {
        if let Some(download) = download {
            return Err(format!(
                "SIM108: the vendor SPICE model {} (sha256 {sha256}) is registered by hand and is not in the cache {}; download it yourself: {download}; then run copper models add BOARD FILE (it checks the hash and copies the file into the cache; copper sim never downloads)",
                model.subckt,
                crate::paths::display(&remote.cache_path(&cache))
            ));
        }
        return Err(format!(
            "the vendor SPICE model {url} is not in the cache {}; run copper models fetch BOARD (copper sim never downloads)",
            crate::paths::display(&remote.cache_path(&cache))
        ));
    };
    let Some(member) = member else { return Ok((path, sha256.clone())) };
    let leaf = member.rsplit(['/', '\\']).next().unwrap_or(member);
    let extracted = path.with_file_name(format!("member-{}", url_file_name(leaf)));
    let what = if url.is_empty() { crate::paths::display(&path) } else { url.clone() };
    let bytes = match std::fs::read(&extracted) {
        Ok(bytes) => bytes,
        Err(_) => {
            let archive = std::fs::read(&path)
                .map_err(|failure| format!("cannot read {}: {failure}", crate::paths::display(&path)))?;
            let bytes =
                crate::zip_member::zip_member(&archive, member).map_err(|failure| format!("{what}: {failure}"))?;
            std::fs::write(&extracted, &bytes)
                .map_err(|failure| format!("cannot write {}: {failure}", crate::paths::display(&extracted)))?;
            bytes
        }
    };
    Ok((extracted, sha256_hex(&bytes)))
}

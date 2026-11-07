//! 3D models a part names by an `https://` URL.
//!
//! Vendor and aggregator models usually allow use in your own designs but not
//! redistribution. So such a model is never fetched by a build: `copper models
//! fetch` downloads it on request into the project's `.copper-cache/models/`,
//! checked against the part's `model_sha256`, and builds only read that cache.
//! Unless the part says `model_redistributable = true`, the KiCad export keeps
//! the file in a separate local-only folder that manufacturing packages leave
//! out (see `docs/3d-models.md`).

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::lower::{
    PART_MODEL_KEY, PART_MODEL_LICENSE_KEY, PART_MODEL_REDISTRIBUTABLE_KEY, PART_MODEL_SHA256_KEY,
    PART_MODEL_SOURCE_KEY,
};
use crate::model::PartDefinition;

/// Model file types a URL may name, as in the KiCad export.
pub const EXTENSIONS: [&str; 4] = ["step", "stp", "wrl", "vrml"];

/// Whether `path` is a model URL rather than a file or module path.
pub fn is_url(path: &str) -> bool {
    path.starts_with("https://") || path.starts_with("http://")
}

/// The lower-cased model extension of an `https://` URL's path, if allowed.
pub fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next()?;
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    EXTENSIONS.contains(&extension.as_str()).then_some(extension)
}

/// The file name a URL ends in, without its query, reduced to letters,
/// digits, `.`, `_` and `-`.
pub fn url_file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let leaf = path.rsplit('/').next().unwrap_or("model");
    let name: String =
        leaf.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if name.trim_matches('.').is_empty() { "model".to_owned() } else { name }
}

/// A 64-digit SHA-256, lower-cased; `None` if `text` is not one.
pub fn normalized_sha256(text: &str) -> Option<String> {
    (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `.copper-cache/models` beside the nearest `copper.mod` at or above
/// `start`, else beside `start` itself.
pub fn cache_directory(start: &Path) -> PathBuf {
    let directory = if start.is_dir() { start } else { start.parent().unwrap_or(start) };
    let root = crate::packages::find_manifest(directory)
        .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| directory.to_path_buf());
    root.join(".copper-cache").join("models")
}

/// One URL model and the parts that name it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteModel {
    pub url: String,
    pub sha256: String,
    pub extension: String,
    pub license: Option<String>,
    pub redistributable: bool,
    pub parts: Vec<String>,
    /// The files that declared those parts.
    pub sources: Vec<String>,
}

impl RemoteModel {
    /// Where the fetched file lives in `cache`: `<sha256>/<URL file name>`,
    /// so exports keep the vendor's name.
    pub fn cache_path(&self, cache: &Path) -> PathBuf {
        cache.join(&self.sha256).join(url_file_name(&self.url))
    }

    /// The cached file if present and still matching `sha256`.
    pub fn cached(&self, cache: &Path) -> Result<Option<PathBuf>, String> {
        let path = self.cache_path(cache);
        match std::fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == self.sha256 => Ok(Some(path)),
            Ok(_) => Err(format!(
                "cached 3D model {} does not match model_sha256 {}; delete it and run copper models fetch",
                crate::paths::display(&path),
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
                "{} has SHA-256 {actual}, not the model_sha256 {} the part declares; nothing was stored",
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

/// The model a part's metadata names by URL, if any.
pub fn part_model(metadata: &IndexMap<String, String>) -> Option<RemoteModel> {
    let url = metadata.get(PART_MODEL_KEY).filter(|path| is_url(path))?;
    Some(RemoteModel {
        url: url.clone(),
        sha256: metadata.get(PART_MODEL_SHA256_KEY)?.clone(),
        extension: url_extension(url)?,
        license: metadata.get(PART_MODEL_LICENSE_KEY).cloned(),
        redistributable: metadata.get(PART_MODEL_REDISTRIBUTABLE_KEY).is_some_and(|value| value == "true"),
        parts: Vec::new(),
        sources: Vec::new(),
    })
}

/// Every URL model the parts of `library` name, merged by URL and hash.
/// Pass an elaborated board's library to include module parts.
pub fn library_models(library: &IndexMap<String, PartDefinition>) -> Vec<RemoteModel> {
    let mut models: Vec<RemoteModel> = Vec::new();
    for (name, part) in library {
        let Some(model) = part_model(&part.metadata) else { continue };
        let source = part.metadata.get(PART_MODEL_SOURCE_KEY).cloned().unwrap_or_default();
        let entry = match models.iter_mut().find(|known| known.url == model.url && known.sha256 == model.sha256) {
            Some(known) => known,
            None => {
                models.push(model);
                models.last_mut().expect("just pushed")
            }
        };
        entry.parts.push(name.clone());
        if !entry.sources.contains(&source) {
            entry.sources.push(source);
        }
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> RemoteModel {
        RemoteModel {
            url: "https://example.com/models/W.step?download=1".to_owned(),
            sha256: sha256_hex(b"ISO-10303-21;"),
            extension: "step".to_owned(),
            license: Some("Vendor terms: no redistribution".to_owned()),
            redistributable: false,
            parts: Vec::new(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn url_extensions_and_hashes_are_checked() {
        assert_eq!(url_extension("https://x.com/a/B.STEP?token=1").as_deref(), Some("step"));
        assert_eq!(url_extension("https://x.com/a/b.wrl#v2").as_deref(), Some("wrl"));
        assert_eq!(url_extension("https://x.com/a/b.zip"), None);
        assert_eq!(url_extension("https://x.com/download?id=3"), None);
        assert_eq!(normalized_sha256(&"AB".repeat(32)).as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(normalized_sha256("abc"), None);
        assert_eq!(url_file_name("https://x.com/a/My Part (rev2).STEP?x=1"), "My_Part__rev2_.STEP");
        assert_eq!(url_file_name("https://x.com/"), "model");
    }

    #[test]
    fn the_cache_stores_only_the_declared_bytes() {
        let cache = std::env::temp_dir().join(format!("copper-remote-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let model = model();
        assert_eq!(model.cached(&cache), Ok(None));
        let refused = model.store(&cache, b"something else").unwrap_err();
        assert!(refused.contains("nothing was stored"), "{refused}");
        assert_eq!(model.cached(&cache), Ok(None));
        let path = model.store(&cache, b"ISO-10303-21;").unwrap();
        assert_eq!(path, cache.join(&model.sha256).join("W.step"));
        assert_eq!(model.cached(&cache), Ok(Some(path.clone())));
        std::fs::write(&path, b"tampered").unwrap();
        assert!(model.cached(&cache).unwrap_err().contains("does not match model_sha256"));
        let _ = std::fs::remove_dir_all(&cache);
    }
}

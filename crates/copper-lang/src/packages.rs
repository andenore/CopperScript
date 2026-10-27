//! Go-style package manifests and deterministic dependency resolution
//! (`pcbir.packages`).
//!
//! Import paths are stable source identities. Versions, local replacements and
//! remote source replacements live in `copper.mod`; content inventories live in
//! `copper.lock`. Remote modules are shallow Git clones in a project-local
//! cache (`.copper-cache/pkg`); package code is never executed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use copper_json::{Dumps, Object, Value, python_str_repr};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::error::CopperScriptError;
use crate::paths;
use crate::syntax::Location;

type Result<T> = std::result::Result<T, CopperScriptError>;

/// An editor's snapshot read of a package source (`source_reader`): open
/// buffers instead of disk. Builds always read the filesystem.
pub type SourceReader = Arc<dyn Fn(&Path) -> std::io::Result<String> + Send + Sync>;

pub const LOCK_SCHEMA: &str = "copperscript.package-lock/v1";
const LOCKED_SUFFIXES: [&str; 7] = [".copper", ".kicad_mod", ".step", ".stp", ".wrl", ".json", ".csv"];
const IGNORED_PARTS: [&str; 5] = [".git", ".copper-cache", "cache", "__pycache__", ".pytest_cache"];

pub(crate) fn location(filename: &str, line: usize) -> Location {
    Location { filename: Arc::from(filename), offset: 0, line, column: 1 }
}

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

/// A parsed `copper.mod`.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleManifest {
    pub path: String,
    pub requirements: IndexMap<String, String>,
    pub replacements: IndexMap<String, String>,
    /// The resolved manifest file.
    pub filename: PathBuf,
    pub footprint_libraries: IndexMap<String, String>,
    /// Module path -> (source module path, revision).
    pub remote_replacements: IndexMap<String, (String, String)>,
}

impl ModuleManifest {
    pub fn root(&self) -> PathBuf {
        self.filename.parent().map(Path::to_path_buf).unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPackage {
    pub import_path: String,
    pub module_path: String,
    pub version: String,
    pub directory: PathBuf,
    pub checksum: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAsset {
    pub path: PathBuf,
    pub identity: String,
    pub module_path: String,
    pub version: String,
    pub checksum: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedModule {
    pub module_path: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
    pub files: Vec<LockedFile>,
}

fn lock_difference(expected: Option<&LockedModule>, actual: &LockedModule) -> String {
    match expected {
        None => format!("expected missing, got {}", actual.checksum),
        Some(expected) if expected.checksum != actual.checksum => {
            format!("expected {}, got {}", expected.checksum, actual.checksum)
        }
        Some(expected) if expected.source != actual.source => {
            format!("expected source {}, got {}", expected.source, actual.source)
        }
        Some(_) => "file inventory differs".to_owned(),
    }
}

/// The nearest `copper.mod` at or above `start` (a file's directory).
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let directory = if start.is_dir() { start } else { start.parent()? };
    directory.ancestors().map(|root| root.join("copper.mod")).find(|candidate| candidate.is_file())
}

fn manifest_error(path: &Path, line: usize, message: impl Into<String>) -> CopperScriptError {
    error("PKG009", message, &location(&paths::display(path), line))
}

/// Strip a `//` or `#` comment that starts the line or follows whitespace.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, _) in line.char_indices() {
        let starts = line[index..].starts_with("//") || line[index..].starts_with('#');
        if !starts {
            continue;
        }
        if index == 0 {
            return "";
        }
        let previous = line[..index].chars().next_back().expect("index > 0");
        if previous.is_whitespace() {
            return &line[..index - previous.len_utf8()];
        }
        let _ = bytes;
    }
    line
}

fn is_git_module_path(module_path: &str) -> bool {
    let parts: Vec<&str> = module_path.split('/').collect();
    let name_ok = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    let shape = match parts.first() {
        Some(&"github.com") => parts.len() == 3 && parts[1..].iter().all(|part| name_ok(part)),
        Some(&"gitlab.com") => parts.len() >= 3 && parts[1..].iter().all(|part| name_ok(part)),
        _ => false,
    };
    shape && !parts.iter().any(|part| *part == "." || *part == "..")
}

fn is_footprint_library_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && characters.all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c))
}

/// `pcbir.packages.read_manifest`.
pub fn read_manifest(path: &Path) -> Result<ModuleManifest> {
    let text = std::fs::read_to_string(path).map_err(|failure| {
        manifest_error(
            path,
            1,
            format!("cannot read manifest: {}", paths::python_os_error(&failure, Some(&paths::display(path)))),
        )
    })?;
    let mut module_path: Option<String> = None;
    let mut requirements = IndexMap::new();
    let mut replacements = IndexMap::new();
    let mut remote_replacements = IndexMap::new();
    let mut footprint_libraries = IndexMap::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        match (fields[0], fields.len()) {
            ("module", 2) => {
                if module_path.is_some() {
                    return Err(manifest_error(path, line_number, "duplicate module directive"));
                }
                module_path = Some(fields[1].to_owned());
            }
            ("require", 3) => {
                if requirements.contains_key(fields[1]) {
                    return Err(manifest_error(
                        path,
                        line_number,
                        format!("duplicate requirement {}", python_str_repr(fields[1])),
                    ));
                }
                requirements.insert(fields[1].to_owned(), fields[2].to_owned());
            }
            ("replace", 4 | 5) if fields[2] == "=>" => {
                if replacements.contains_key(fields[1]) || remote_replacements.contains_key(fields[1]) {
                    return Err(manifest_error(
                        path,
                        line_number,
                        format!("duplicate replacement {}", python_str_repr(fields[1])),
                    ));
                }
                if fields.len() == 4 {
                    replacements.insert(fields[1].to_owned(), fields[3].to_owned());
                } else {
                    let source = fields[3].strip_prefix("https://").unwrap_or(fields[3]);
                    let source = source.strip_suffix(".git").unwrap_or(source);
                    let revision = fields[4];
                    if !is_git_module_path(source) || source == fields[1] {
                        return Err(manifest_error(
                            path,
                            line_number,
                            format!("invalid remote replacement module {}", python_str_repr(fields[3])),
                        ));
                    }
                    if revision.starts_with('-') {
                        return Err(manifest_error(
                            path,
                            line_number,
                            format!("invalid remote replacement revision {}", python_str_repr(revision)),
                        ));
                    }
                    remote_replacements.insert(fields[1].to_owned(), (source.to_owned(), revision.to_owned()));
                }
            }
            ("footprint-library", 3) => {
                let (name, target) = (fields[1], fields[2]);
                if !is_footprint_library_name(name) || footprint_libraries.contains_key(name) {
                    return Err(manifest_error(
                        path,
                        line_number,
                        format!("invalid or duplicate footprint library {}", python_str_repr(name)),
                    ));
                }
                let target = target.strip_prefix("https://").unwrap_or(target);
                let bad_part = target.split('/').any(|part| part.is_empty() || part == "." || part == "..");
                if !(target.starts_with("github.com/") || target.starts_with("gitlab.com/"))
                    || target.contains('\\')
                    || bad_part
                    || target.chars().any(|c| "?#%:".contains(c))
                {
                    return Err(manifest_error(
                        path,
                        line_number,
                        format!("invalid footprint library target {}", python_str_repr(target)),
                    ));
                }
                footprint_libraries.insert(name.to_owned(), target.to_owned());
            }
            _ => return Err(manifest_error(path, line_number, format!("invalid directive: {}", raw_line.trim()))),
        }
    }
    let Some(module_path) = module_path else {
        return Err(manifest_error(path, 1, "manifest must declare its module path"));
    };
    let mut unknown: Vec<&String> = replacements
        .keys()
        .chain(remote_replacements.keys())
        .filter(|name| !requirements.contains_key(*name))
        .collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return Err(manifest_error(
            path,
            1,
            format!("replacement has no matching requirement: {}", python_str_repr(name)),
        ));
    }
    for (name, target) in &footprint_libraries {
        let provided = requirements.keys().any(|module| target == module || target.starts_with(&format!("{module}/")));
        if !provided {
            return Err(manifest_error(
                path,
                1,
                format!("footprint library {} has no matching requirement", python_str_repr(name)),
            ));
        }
    }
    Ok(ModuleManifest {
        path: module_path,
        requirements,
        replacements,
        filename: paths::resolve(path),
        footprint_libraries,
        remote_replacements,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `_module_inventory`: every locked file under `root`, sorted by path.
pub fn module_inventory(root: &Path) -> std::result::Result<Vec<LockedFile>, String> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|failure| paths::python_os_error(&failure, Some(&paths::display(&directory))))?;
        for entry in entries {
            let entry = entry.map_err(|failure| paths::python_os_error(&failure, None))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if IGNORED_PARTS.contains(&name.as_str()) {
                continue;
            }
            let relative = format!("{prefix}{name}");
            let kind = entry.file_type().map_err(|failure| paths::python_os_error(&failure, None))?;
            if kind.is_symlink() {
                return Err(format!("package assets may not be symlinks: {relative}"));
            }
            if kind.is_dir() {
                pending.push((entry.path(), format!("{relative}/")));
            } else if kind.is_file()
                && (name == "copper.mod" || LOCKED_SUFFIXES.contains(&paths::extension(&name).to_lowercase().as_str()))
            {
                candidates.push((relative, entry.path()));
            }
        }
    }
    candidates.sort();
    candidates
        .into_iter()
        .map(|(relative, path)| {
            let data = std::fs::read(&path)
                .map_err(|failure| paths::python_os_error(&failure, Some(&paths::display(&path))))?;
            Ok(LockedFile { path: relative, size: data.len() as u64, sha256: hex(&Sha256::digest(&data)) })
        })
        .collect()
}

/// `_inventory_checksum`.
pub fn inventory_checksum(files: &[LockedFile]) -> String {
    let mut digest = Sha256::new();
    for file in files {
        digest.update(file.path.as_bytes());
        digest.update(b"\0");
        digest.update(file.size.to_string().as_bytes());
        digest.update(b"\0");
        digest.update(file.sha256.as_bytes());
        digest.update(b"\0");
    }
    format!("sha256:{}", hex(&digest.finalize()))
}

type LockEntries = IndexMap<(String, String), LockedModule>;

/// `_read_lock`.
pub fn read_lock(path: &Path) -> Result<LockEntries> {
    if !path.exists() {
        return Ok(LockEntries::new());
    }
    let failure = |message: String| {
        error("PKG007", format!("cannot read copper.lock: {message}"), &location(&paths::display(path), 1))
    };
    let text =
        std::fs::read_to_string(path).map_err(|e| failure(paths::python_os_error(&e, Some(&paths::display(path)))))?;
    let document = copper_json::parse(&text).map_err(|e| failure(e.python_message(&text)))?;
    let modules =
        match (document.get("schema").and_then(Value::as_str), document.get("modules").and_then(Value::as_array)) {
            (Some(LOCK_SCHEMA), Some(modules)) => modules,
            _ => return Err(failure("unsupported schema".to_owned())),
        };
    let text_field = |raw: &Value, key: &str| -> std::result::Result<String, String> {
        raw.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| python_str_repr(key))
    };
    let mut entries = LockEntries::new();
    for raw in modules {
        let parsed = (|| -> std::result::Result<LockedModule, String> {
            let files = raw
                .get("files")
                .and_then(Value::as_array)
                .ok_or_else(|| python_str_repr("files"))?
                .iter()
                .map(|item| {
                    let size = match item.get("size") {
                        Some(Value::Int(size)) => u64::try_from(size.clone()).map_err(|_| "invalid size".to_owned())?,
                        _ => return Err(python_str_repr("size")),
                    };
                    Ok(LockedFile { path: text_field(item, "path")?, size, sha256: text_field(item, "sha256")? })
                })
                .collect::<std::result::Result<Vec<_>, String>>()?;
            Ok(LockedModule {
                module_path: text_field(raw, "module")?,
                version: text_field(raw, "version")?,
                source: text_field(raw, "source")?,
                checksum: text_field(raw, "checksum")?,
                files,
            })
        })()
        .map_err(failure)?;
        let key = (parsed.module_path.clone(), parsed.version.clone());
        if entries.contains_key(&key) {
            return Err(failure(format!("duplicate module {}@{}", key.0, key.1)));
        }
        entries.insert(key, parsed);
    }
    Ok(entries)
}

/// `_write_lock`: sorted modules, `indent=2`, LF newlines on every platform.
pub fn lock_text(entries: &LockEntries) -> String {
    let mut keys: Vec<&(String, String)> = entries.keys().collect();
    keys.sort();
    let modules: Vec<Value> = keys
        .into_iter()
        .map(|key| {
            let entry = &entries[key];
            let files: Vec<Value> = entry
                .files
                .iter()
                .map(|file| {
                    Value::Object(Object::from([
                        ("path".to_owned(), Value::from(file.path.as_str())),
                        ("size".to_owned(), Value::from(file.size)),
                        ("sha256".to_owned(), Value::from(file.sha256.as_str())),
                    ]))
                })
                .collect();
            Value::Object(Object::from([
                ("module".to_owned(), Value::from(entry.module_path.as_str())),
                ("version".to_owned(), Value::from(entry.version.as_str())),
                ("source".to_owned(), Value::from(entry.source.as_str())),
                ("checksum".to_owned(), Value::from(entry.checksum.as_str())),
                ("files".to_owned(), Value::Array(files)),
            ]))
        })
        .collect();
    let document = Value::Object(Object::from([
        ("schema".to_owned(), Value::from(LOCK_SCHEMA)),
        ("modules".to_owned(), Value::Array(modules)),
    ]));
    format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&document))
}

fn write_lock(path: &Path, entries: &LockEntries) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let temporary = directory.join(format!(".copper-lock-{}", std::process::id()));
    let result = std::fs::write(&temporary, lock_text(entries)).and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn starts_with_any(text: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| text.starts_with(prefix))
}

fn bad_identity(identity: &str) -> bool {
    identity.contains('\\')
        || identity.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || identity.chars().any(|c| "?#%:".contains(c))
}

/// Whether `path` or a parent below `root` is a symlink.
fn crosses_symlink(unresolved: &Path, root: &Path) -> bool {
    unresolved
        .ancestors()
        .filter(|candidate| paths::is_relative_to(candidate, root))
        .any(|candidate| candidate.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()))
}

/// `pcbir.packages.PackageResolver`.
pub struct PackageResolver {
    pub manifest: ModuleManifest,
    pub cache_root: PathBuf,
    pub locked: bool,
    pub offline: bool,
    pub lock_path: PathBuf,
    lock_entries: LockEntries,
    /// Module roots resolved so far -> (module path, version, checksum).
    source_roots: IndexMap<PathBuf, (String, String, String)>,
    pub source_directory: PathBuf,
    /// Reads package sources instead of the filesystem (the language server).
    pub source_reader: Option<SourceReader>,
}

impl PackageResolver {
    pub fn new(manifest: ModuleManifest, locked: bool, offline: bool) -> Result<Self> {
        let root = manifest.root();
        let lock_path = root.join("copper.lock");
        let lock_entries = read_lock(&lock_path)?;
        Ok(PackageResolver {
            cache_root: root.join(".copper-cache").join("pkg"),
            locked,
            offline,
            lock_path,
            lock_entries,
            source_roots: IndexMap::new(),
            source_directory: paths::resolve(&root),
            manifest,
            source_reader: None,
        })
    }

    /// `PackageResolver.for_source`: the manifest at or above `source`, or an
    /// empty workspace manifest beside it.
    pub fn for_source(source: &Path, locked: bool, offline: bool) -> Result<Self> {
        let manifest = match find_manifest(source) {
            Some(path) => read_manifest(&path)?,
            None => ModuleManifest {
                path: "workspace".to_owned(),
                requirements: IndexMap::new(),
                replacements: IndexMap::new(),
                filename: paths::resolve(source).parent().unwrap_or(Path::new("/")).join("copper.mod"),
                footprint_libraries: IndexMap::new(),
                remote_replacements: IndexMap::new(),
            },
        };
        let mut resolver = PackageResolver::new(manifest, locked, offline)?;
        let resolved = paths::resolve(source);
        resolver.source_directory =
            if resolved.is_dir() { resolved } else { resolved.parent().map(Path::to_path_buf).unwrap_or(resolved) };
        Ok(resolver)
    }

    pub fn lock_entries(&self) -> &LockEntries {
        &self.lock_entries
    }

    /// The deepest resolved module root containing `importer`, else the
    /// manifest root.
    fn owning_root(&self, importer: &Path) -> PathBuf {
        self.source_roots
            .keys()
            .filter(|root| paths::is_relative_to(importer, root))
            .max_by_key(|root| root.components().count())
            .cloned()
            .unwrap_or_else(|| paths::resolve(&self.manifest.root()))
    }

    /// `qualify_footprint`: keep a relative `.kicad_mod` attached to its module.
    pub fn qualify_footprint(&self, reference: &str, location: &Location) -> Result<String> {
        if starts_with_any(reference, &["github.com/", "gitlab.com/", "https://"])
            || !reference.to_lowercase().ends_with(".kicad_mod")
        {
            return Ok(reference.to_owned());
        }
        if Path::new(reference).is_absolute() || reference.contains('\\') || reference.contains(':') {
            return Err(error("PKG004", "package footprint must be a module-relative .kicad_mod path", location));
        }
        let importer = paths::resolve(Path::new(&*location.filename));
        let root = self.owning_root(&importer);
        let asset = paths::resolve(&importer.parent().unwrap_or(Path::new("/")).join(reference));
        if !paths::is_relative_to(&asset, &root) {
            return Err(error("PKG004", "package footprint path must stay inside its source module", location));
        }
        if let Some((module, _, _)) = self.source_roots.get(&root) {
            return Ok(format!("{module}/{}", paths::as_posix(asset.strip_prefix(&root).expect("inside root"))));
        }
        Ok(paths::as_posix(&paths::relpath(&asset, &self.source_directory)))
    }

    /// `qualify_macro_asset`: qualify a data import without fetching it.
    pub fn qualify_macro_asset(&self, reference: &str, location: &Location) -> Result<String> {
        let identity = reference.strip_prefix("https://").unwrap_or(reference);
        if starts_with_any(identity, &["github.com/", "gitlab.com/"]) {
            if !(identity.ends_with(".json") || identity.ends_with(".copper")) || bad_identity(identity) {
                return Err(error("PKG004", "invalid macro module asset path", location));
            }
            if self.matching_module(identity).is_none() {
                return Err(error(
                    "PKG002",
                    format!("no requirement in copper.mod provides macro asset {}", python_str_repr(identity)),
                    location,
                ));
            }
            return Ok(identity.to_owned());
        }
        if !(reference.ends_with(".json") || reference.ends_with(".copper"))
            || !starts_with_any(reference, &["./", "../"])
            || reference.contains('\\')
            || reference.contains(':')
        {
            return Err(error("PKG004", "local macro asset must be a relative .json or .copper path", location));
        }
        let importer = paths::resolve(Path::new(&*location.filename));
        let root = self.owning_root(&importer);
        let unresolved = importer.parent().unwrap_or(Path::new("/")).join(reference);
        let asset = paths::resolve(&unresolved);
        if !paths::is_relative_to(&asset, &root) || crosses_symlink(&unresolved, &root) {
            return Err(error("PKG004", "macro path must stay inside its source module without symlinks", location));
        }
        if let Some((module, _, _)) = self.source_roots.get(&root) {
            return Ok(format!("{module}/{}", paths::as_posix(asset.strip_prefix(&root).expect("inside root"))));
        }
        Ok(paths::as_posix(&paths::relpath(&asset, &self.source_directory)))
    }

    /// `resolve_asset`: checked module data, hashed against the lock.
    pub fn resolve_asset(&mut self, import_path: &str, location: &Location) -> Result<ResolvedAsset> {
        self.resolve_asset_with(import_path, location, None)
    }

    /// `resolve_asset` reusing (and recording) verified module snapshots.
    pub fn resolve_asset_with(
        &mut self,
        import_path: &str,
        location: &Location,
        verified: Option<&mut IndexMap<String, ResolvedPackage>>,
    ) -> Result<ResolvedAsset> {
        let identity = import_path.strip_prefix("https://").unwrap_or(import_path);
        if !starts_with_any(identity, &["github.com/", "gitlab.com/"]) || bad_identity(identity) {
            return Err(error("PKG004", "invalid module asset path", location));
        }
        let Some(module) = self.matching_module(identity) else {
            return Err(error(
                "PKG002",
                format!("no requirement in copper.mod provides asset {}", python_str_repr(identity)),
                location,
            ));
        };
        let package = match verified {
            Some(verified) => match verified.get(&module) {
                Some(package) => package.clone(),
                None => {
                    let package = self.resolve(&module, location)?;
                    verified.insert(module.clone(), package.clone());
                    package
                }
            },
            None => self.resolve(&module, location)?,
        };
        let relative = identity[module.len()..].trim_start_matches('/');
        let unresolved = package.directory.join(relative);
        let asset = paths::resolve(&unresolved);
        if relative.is_empty()
            || !paths::is_relative_to(&asset, &package.directory)
            || !asset.is_file()
            || crosses_symlink(&unresolved, &package.directory)
        {
            return Err(error(
                "PKG004",
                format!("module asset does not exist or escapes its module: {identity}"),
                location,
            ));
        }
        let key = (module.clone(), package.version.clone());
        let entry = self
            .lock_entries
            .get(&key)
            .and_then(|entry| entry.files.iter().find(|file| file.path == relative))
            .cloned();
        let actual = std::fs::read(&asset).map(|data| hex(&Sha256::digest(&data))).ok();
        match (entry, actual) {
            (Some(entry), Some(actual)) if entry.sha256 == actual => Ok(ResolvedAsset {
                path: asset,
                identity: identity.to_owned(),
                module_path: module,
                version: package.version,
                checksum: package.checksum,
                sha256: entry.sha256,
            }),
            _ => Err(error(
                "PKG008",
                format!("lock mismatch: asset differs from verified module inventory: {identity}"),
                location,
            )),
        }
    }

    /// `PackageResolver.resolve`.
    pub fn resolve(&mut self, import_path: &str, location: &Location) -> Result<ResolvedPackage> {
        if starts_with_any(import_path, &["./", "../"]) {
            return self.resolve_relative(import_path, location);
        }
        if !self.manifest.filename.is_file() {
            return Err(error(
                "PKG001",
                "URL package imports require a copper.mod in this directory or a parent",
                location,
            ));
        }
        let Some(module_path) = self.matching_module(import_path) else {
            return Err(error(
                "PKG002",
                format!("no requirement in copper.mod provides package {}", python_str_repr(import_path)),
                location,
            ));
        };
        let version = self.manifest.requirements[&module_path].clone();
        let replacement = self.manifest.replacements.get(&module_path).cloned();
        let remote = self.manifest.remote_replacements.get(&module_path).cloned();
        let key = (module_path.clone(), version.clone());
        if self.locked && !self.lock_entries.contains_key(&key) {
            return Err(error(
                "PKG010",
                format!("locked resolution requires {module_path}@{version} in copper.lock"),
                location,
            ));
        }
        let module_root = if let Some(replacement) = &replacement {
            let mut root = PathBuf::from(replacement);
            if !root.is_absolute() {
                root = self.manifest.root().join(root);
            }
            let root = paths::resolve(&root);
            if !root.is_dir() {
                return Err(error(
                    "PKG003",
                    format!("replacement directory does not exist: {}", paths::display(&root)),
                    location,
                ));
            }
            root
        } else if let Some((source, revision)) = &remote {
            self.fetch_git_module(source, revision, location)?
        } else {
            self.fetch_git_module(&module_path, &version, location)?
        };
        let suffix = import_path[module_path.len()..].trim_start_matches('/');
        let package_dir = paths::resolve(&module_root.join(suffix));
        if !package_dir.is_dir() || !paths::is_relative_to(&package_dir, &module_root) {
            return Err(error(
                "PKG004",
                format!(
                    "package {} does not exist in module {}",
                    python_str_repr(import_path),
                    python_str_repr(&module_path)
                ),
                location,
            ));
        }
        let files = module_inventory(&module_root).map_err(|message| {
            error("PKG012", format!("cannot inventory {module_path}@{version}: {message}"), location)
        })?;
        let checksum = inventory_checksum(&files);
        let source = match (&replacement, &remote) {
            (Some(replacement), _) => format!("replace:{replacement}"),
            (None, Some((source, revision))) => format!("replace:git:https://{source}.git@{revision}"),
            (None, None) => format!("git:https://{module_path}.git"),
        };
        self.verify_or_record_lock(
            LockedModule {
                module_path: module_path.clone(),
                version: version.clone(),
                source,
                checksum: checksum.clone(),
                files,
            },
            location,
        )?;
        self.source_roots.insert(module_root, (module_path.clone(), version.clone(), checksum.clone()));
        Ok(ResolvedPackage {
            import_path: import_path.to_owned(),
            module_path,
            version,
            directory: package_dir,
            checksum,
        })
    }

    fn resolve_relative(&self, import_path: &str, location: &Location) -> Result<ResolvedPackage> {
        let importer = paths::resolve(Path::new(&*location.filename));
        let root = self.owning_root(&importer);
        let unresolved = importer.parent().unwrap_or(Path::new("/")).join(import_path);
        let directory = paths::resolve(&unresolved);
        if import_path.contains('\\')
            || !paths::is_relative_to(&importer, &root)
            || !paths::is_relative_to(&directory, &root)
            || !directory.is_dir()
            || crosses_symlink(&unresolved, &root)
        {
            return Err(error("PKG004", "relative package import must stay inside its source module", location));
        }
        let checksum = module_inventory(&directory)
            .map(|files| inventory_checksum(&files))
            .map_err(|message| error("PKG012", format!("cannot inventory local package: {message}"), location))?;
        let external = self.source_roots.get(&root).cloned();
        let (module, version, root_checksum) =
            external.clone().unwrap_or((self.manifest.path.clone(), "workspace".to_owned(), checksum.clone()));
        if external.is_some() {
            let current = module_inventory(&root).map(|files| inventory_checksum(&files)).ok();
            if current.as_deref() != Some(root_checksum.as_str()) {
                return Err(error("PKG008", "source module changed during relative import resolution", location));
            }
        }
        let relative = paths::as_posix(directory.strip_prefix(&root).expect("inside root"));
        let identity = format!("{module}/{}", if relative.is_empty() { "." } else { &relative });
        Ok(ResolvedPackage {
            import_path: identity,
            module_path: module,
            version,
            directory,
            checksum: if external.is_some() { root_checksum } else { checksum },
        })
    }

    /// The longest required module that `import_path` falls under.
    pub fn matching_module(&self, import_path: &str) -> Option<String> {
        self.manifest
            .requirements
            .keys()
            .filter(|module| import_path == module.as_str() || import_path.starts_with(&format!("{module}/")))
            .max_by_key(|module| module.len())
            .cloned()
    }

    fn fetch_git_module(&self, module_path: &str, version: &str, location: &Location) -> Result<PathBuf> {
        if !starts_with_any(module_path, &["github.com/", "gitlab.com/"]) {
            return Err(error(
                "PKG005",
                "remote fetching supports github.com and gitlab.com module paths; use replace for other sources",
                location,
            ));
        }
        if !is_git_module_path(module_path) {
            return Err(error("PKG005", "invalid Git module path", location));
        }
        if version.is_empty() || version.starts_with('-') {
            return Err(error("PKG006", "invalid Git revision", location));
        }
        let cache_key = &hex(&Sha256::digest(format!("{module_path}@{version}").as_bytes()))[..20];
        let destination = self.cache_root.join(cache_key);
        if destination.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(error("PKG012", "package cache roots may not be symlinks", location));
        }
        if destination.is_dir() {
            return Ok(paths::resolve(&destination));
        }
        if self.offline {
            return Err(error(
                "PKG011",
                format!("offline resolution cache miss for {module_path}@{version}"),
                location,
            ));
        }
        std::fs::create_dir_all(&self.cache_root)
            .map_err(|failure| error("PKG006", paths::python_os_error(&failure, None), location))?;
        let temporary = self.cache_root.join(format!("{cache_key}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temporary);
        let is_commit = version.len() == 40 && version.chars().all(|c| c.is_ascii_hexdigit());
        let git = |arguments: &[&str]| -> Result<()> {
            let output = Command::new("git")
                .args(["-c", "core.autocrlf=false", "-c", "core.eol=lf", "-c", "core.longpaths=true"])
                .args(arguments)
                .output()
                .map_err(|failure| error("PKG006", paths::python_os_error(&failure, None), location))?;
            if output.status.success() {
                Ok(())
            } else {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                Err(error("PKG006", if detail.is_empty() { "git clone failed".to_owned() } else { detail }, location))
            }
        };
        let temporary_text = paths::display(&temporary);
        let url = format!("https://{module_path}.git");
        let mut clone = vec!["clone", "--quiet", "--depth", "1"];
        if !is_commit {
            clone.extend(["--branch", version]);
        }
        clone.extend([url.as_str(), temporary_text.as_str()]);
        let outcome = git(&clone).and_then(|()| {
            if is_commit {
                git(&["-C", &temporary_text, "fetch", "--quiet", "--depth", "1", "origin", version])?;
                git(&["-C", &temporary_text, "checkout", "--quiet", "--detach", version])?;
            }
            std::fs::rename(&temporary, &destination)
                .map_err(|failure| error("PKG006", paths::python_os_error(&failure, None), location))
        });
        if temporary.exists() {
            let _ = std::fs::remove_dir_all(&temporary);
        }
        outcome?;
        Ok(paths::resolve(&destination))
    }

    fn verify_or_record_lock(&mut self, entry: LockedModule, location: &Location) -> Result<()> {
        let key = (entry.module_path.clone(), entry.version.clone());
        let declared: Vec<(String, String)> =
            self.manifest.requirements.iter().map(|(module, version)| (module.clone(), version.clone())).collect();
        let expected = self.lock_entries.get(&key);
        let only_declared = self.lock_entries.keys().all(|known| declared.contains(known));
        if expected == Some(&entry) && (self.locked || only_declared) {
            return Ok(());
        }
        if self.locked {
            return Err(error(
                "PKG008",
                format!(
                    "lock mismatch for {}@{}: {}",
                    entry.module_path,
                    entry.version,
                    lock_difference(expected, &entry)
                ),
                location,
            ));
        }
        self.lock_entries.insert(key, entry);
        self.lock_entries.retain(|known, _| declared.contains(known));
        write_lock(&self.lock_path, &self.lock_entries).map_err(|failure| {
            error("PKG007", format!("cannot write copper.lock: {}", paths::python_os_error(&failure, None)), location)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_strip_like_the_python_regex() {
        assert_eq!(strip_comment("require a b // note"), "require a b");
        assert_eq!(strip_comment("# whole line"), "");
        assert_eq!(
            strip_comment("footprint-library X https://github.com/a/b"),
            "footprint-library X https://github.com/a/b"
        );
        assert_eq!(strip_comment("x\t#y"), "x");
    }

    #[test]
    fn git_module_paths() {
        assert!(is_git_module_path("github.com/andenore/CopperLib"));
        assert!(!is_git_module_path("github.com/a/b/c"));
        assert!(is_git_module_path("gitlab.com/group/sub/repo"));
        assert!(!is_git_module_path("github.com/../b"));
    }
}

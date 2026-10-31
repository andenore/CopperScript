//! Bounded, read-only electrical source snapshots. The ordinary compiler owns
//! import semantics; a host only supplies requested UTF-8 files. Remote source
//! bytes are checked against the native lock inventory. Physical assets are not
//! loaded or claimed verified by this electrical-review boundary.
use crate::{
    CopperScriptError,
    compiler::is_macro_import,
    packages::{self, LockEntries, LockedFile, ModuleManifest, PackageResolver},
    parse, paths,
};
use copper_json::{Object, Value};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_SNAPSHOT_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 512 * 1024;
pub const MAX_LOADED_BYTES: usize = 8 * 1024 * 1024;
const MAX_PATHS: usize = 8192;
fn object(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(Object::from_iter(entries.into_iter().map(|(k, v)| (k.to_owned(), v))))
}

fn fail(message: impl Into<String>) -> CopperScriptError {
    CopperScriptError::new("WEB_PROJECT", message, packages::location("copper.mod", 1))
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, CopperScriptError> {
    v.get(key).and_then(Value::as_str).ok_or_else(|| fail(format!("missing project field {key}")))
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
fn hash(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Clone, Default)]
pub struct SourceSnapshot {
    pub files: BTreeMap<PathBuf, String>,
    pub requests: BTreeMap<PathBuf, Value>,
    pub module_roots: IndexMap<String, PathBuf>,
    pub inventories: BTreeMap<PathBuf, Vec<LockedFile>>,
}
impl SourceSnapshot {
    pub fn read(&self, path: &Path) -> std::io::Result<String> {
        self.files
            .get(path)
            .map(|s| s.replace("\r\n", "\n").replace('\r', "\n"))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "source missing from snapshot"))
    }
    pub fn sources(&self, directory: &Path) -> Vec<PathBuf> {
        self.requests
            .keys()
            .filter(|p| p.parent() == Some(directory) && p.extension().is_some_and(|e| e == "copper"))
            .cloned()
            .collect()
    }
    pub fn is_dir(&self, directory: &Path) -> bool {
        self.requests.keys().any(|p| p.starts_with(directory) && p != directory)
    }
    pub fn inventory(&self, directory: &Path) -> Result<Vec<LockedFile>, String> {
        if let Some(files) = self.inventories.get(directory) {
            return Ok(files.clone());
        }
        // Workspace checksums identify loaded electrical sources, not physical assets.
        Ok(self
            .files
            .iter()
            .filter(|(p, _)| p.starts_with(directory))
            .map(|(p, s)| LockedFile {
                path: paths::as_posix(p.strip_prefix(directory).expect("inside directory")),
                size: s.len() as u64,
                sha256: hash(s.as_bytes()),
            })
            .collect())
    }
}

pub struct ProjectSnapshot {
    pub entry: PathBuf,
    pub source: Arc<SourceSnapshot>,
    pub manifest: ModuleManifest,
    pub locks: LockEntries,
}
impl ProjectSnapshot {
    pub fn from_json(input: &str) -> Result<Self, CopperScriptError> {
        if input.len() > MAX_SNAPSHOT_BYTES {
            return Err(fail("project snapshot exceeds 24 MiB"));
        }
        let raw = copper_json::parse(input).map_err(|e| fail(e.to_string()))?;
        if text(&raw, "schema")? != "copperscript.project-snapshot/v1" {
            return Err(fail("unsupported project snapshot"));
        }
        // Absolute virtual roots work in both the native tests and wasm (no cwd).
        let root =
            PathBuf::from(if cfg!(windows) { "C:/copper-browser/workspace" } else { "/copper-browser/workspace" });
        let entry = text(&raw, "entry")?;
        if !safe_path(entry) || !entry.ends_with(".copper") {
            return Err(fail("invalid project entry path"));
        }
        let manifest_text = text(&raw, "manifest")?;
        let manifest = packages::parse_manifest(
            if manifest_text.is_empty() { "module workspace" } else { manifest_text },
            &root.join("copper.mod"),
        )?;
        let lock_text = text(&raw, "lock")?;
        let locks = if lock_text.is_empty() {
            LockEntries::new()
        } else {
            packages::parse_lock(lock_text, &root.join("copper.lock"))?
        };
        let mut source = SourceSnapshot::default();
        let workspace = raw
            .get("workspace_files")
            .and_then(Value::as_array)
            .ok_or_else(|| fail("missing workspace file inventory"))?;
        if workspace.len() > MAX_PATHS {
            return Err(fail("workspace has too many source files"));
        }
        for path in workspace {
            let path = path.as_str().ok_or_else(|| fail("invalid workspace path"))?;
            if !safe_path(path) || !path.ends_with(".copper") {
                return Err(fail("unsafe workspace source path"));
            }
            if source
                .requests
                .insert(
                    root.join(path),
                    object([("id", Value::from(format!("workspace/{path}"))), ("path", Value::from(path))]),
                )
                .is_some()
            {
                return Err(fail("duplicate workspace source path"));
            }
        }
        if !source.requests.contains_key(&root.join(entry)) {
            return Err(fail("entry is missing from workspace inventory"));
        }
        for (module, version) in &manifest.requirements {
            if !safe_path(module) {
                return Err(fail("unsafe module identity"));
            }
            let Some(locked) = locks.get(&(module.clone(), version.clone())) else { continue };
            let local = manifest.replacements.get(module);
            let remote = manifest.remote_replacements.get(module);
            let (repository, revision, expected_source) = match (local, remote) {
                (Some(path), _) => (String::new(), String::new(), format!("replace:{path}")),
                (_, Some((repository, revision))) => {
                    (repository.clone(), revision.clone(), format!("replace:git:https://{repository}.git@{revision}"))
                }
                _ => (module.clone(), version.clone(), format!("git:https://{module}.git")),
            };
            if locked.source != expected_source {
                return Err(fail(format!("locked source differs for {module}")));
            }
            let mut previous = "";
            for file in &locked.files {
                if !safe_path(&file.path) || !digest(&file.sha256) || file.path.as_str() <= previous {
                    return Err(fail(format!("invalid or unsorted lock inventory for {module}")));
                }
                previous = &file.path;
            }
            if packages::inventory_checksum(&locked.files) != locked.checksum {
                return Err(fail(format!("lock inventory checksum differs for {module}")));
            }
            let module_root = if let Some(local) = local {
                let path = local.strip_prefix("./").unwrap_or(local);
                if !safe_path(path) {
                    return Err(fail("local replacements must stay inside the downloaded workspace"));
                }
                root.join(path)
            } else {
                // Unsupported providers fail when imported, without blocking unused footprint requirements.
                root.parent().expect("virtual root").join("dependencies").join(module)
            };
            source.module_roots.insert(module.clone(), module_root.clone());
            source.inventories.insert(module_root.clone(), locked.files.clone());
            for file in locked.files.iter().filter(|f| f.path.ends_with(".copper")) {
                let path = module_root.join(&file.path);
                let mut request = if local.is_some() {
                    source
                        .requests
                        .get(&path)
                        .cloned()
                        .ok_or_else(|| fail(format!("replacement source missing: {}", file.path)))?
                } else {
                    object([
                        ("id", Value::from(format!("dependency/{module}/{}", file.path))),
                        ("path", Value::from(file.path.clone())),
                        ("repository", Value::from(repository.clone())),
                        ("revision", Value::from(revision.clone())),
                        ("version", Value::from(version.clone())),
                    ])
                };
                let Value::Object(fields) = &mut request else { unreachable!() };
                fields.insert("sha256".to_owned(), Value::from(file.sha256.clone()));
                fields.insert("size".to_owned(), Value::from(file.size));
                fields.insert("module".to_owned(), Value::from(module.clone()));
                source.requests.insert(path, request);
            }
        }
        if source.requests.len() > MAX_PATHS {
            return Err(fail("project has too many source files"));
        }
        let supplied =
            raw.get("files").and_then(Value::as_object).ok_or_else(|| fail("missing project source files"))?;
        if supplied.len() > 512 {
            return Err(fail("project exceeds 512 loaded source files"));
        }
        let by_id: BTreeMap<&str, &PathBuf> = source
            .requests
            .iter()
            .map(|(p, r)| (r.get("id").and_then(Value::as_str).expect("request id"), p))
            .collect();
        let mut size = 0;
        for (id, data) in supplied {
            let path = by_id.get(id.as_str()).ok_or_else(|| fail(format!("unrequested source file {id}")))?;
            let data = data.as_str().ok_or_else(|| fail("source must be UTF-8 text"))?;
            size += data.len();
            if data.len() > MAX_SOURCE_BYTES || size > MAX_LOADED_BYTES {
                return Err(fail("project source exceeds browser size limits"));
            }
            let request = &source.requests[*path];
            if let Some(expected) = request.get("sha256").and_then(Value::as_str)
                && (hash(data.as_bytes()) != expected || request.get("size") != Some(&Value::from(data.len() as u64)))
            {
                return Err(fail(format!("source checksum mismatch: {id}")));
            }
            source.files.insert((*path).clone(), data.to_owned());
        }
        Ok(Self { entry: root.join(entry), source: Arc::new(source), manifest, locks })
    }
    pub fn resolver(&self) -> PackageResolver {
        PackageResolver::from_snapshot(self.manifest.clone(), self.locks.clone(), self.source.clone())
    }
    /// Discover transitive imports with the real parser and package resolver.
    pub fn plan(&self) -> Result<Vec<Value>, CopperScriptError> {
        let mut resolver = self.resolver();
        let mut pending = vec![self.entry.clone()];
        let mut seen = BTreeSet::new();
        let mut missing = BTreeMap::new();
        while let Some(path) = pending.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(source) = self.source.files.get(&path) else {
                let request = self
                    .source
                    .requests
                    .get(&path)
                    .ok_or_else(|| fail("import is absent from the source inventory"))?;
                if let Some(repository) = request.get("repository").and_then(Value::as_str)
                    && (!repository.starts_with("github.com/") || repository.split('/').count() != 3)
                {
                    return Err(fail("browser package downloads currently support public github.com modules"));
                }
                missing.insert(path, request.clone());
                continue;
            };
            let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
            let document = parse(&normalized, &paths::display(&path))?;
            for import in &document.imports {
                if is_macro_import(&import.path) {
                    resolver.qualify_macro_asset(&import.path, &import.location)?;
                    continue;
                }
                let package = resolver.resolve(&import.path, &import.location)?;
                let sources = resolver.source_paths(&package.directory);
                if sources.is_empty() {
                    return Err(fail(format!("package {} exports no .copper files", import.path)));
                }
                pending.extend(sources);
            }
        }
        Ok(missing.into_values().collect())
    }
    pub fn provenance(&self) -> Value {
        object([
            ("scope", Value::from("loaded electrical source files; physical assets not fetched or verified")),
            (
                "files",
                Value::Array(
                    self.source
                        .files
                        .iter()
                        .map(|(path, source)| {
                            object([
                                ("id", self.source.requests[path].get("id").expect("source id").clone()),
                                ("sha256", Value::from(hash(source.as_bytes()))),
                                ("size", Value::from(source.len() as u64)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile_design_source_with;
    const MODULE: &str = "github.com/acme/parts";
    const VERSION: &str = "0123456789012345678901234567890123456789";
    const BOARD: &str = "board Test { use library \"tiny\"; import local \"./src\"; module X: local.Wrapper; }";
    const LOCAL: &str =
        "module Wrapper { use library \"tiny\"; import part \"github.com/acme/parts/pkg\"; module Inner: part.Unit; }";
    const REMOTE: &str =
        "module Unit { use library \"tiny\"; import other \"../support\"; module D: other.Passive; }\r\n";
    const SUPPORT: &str = "module Passive { use library \"tiny\"; component R: RESISTOR; }";
    fn fixture() -> Value {
        let files = vec![
            LockedFile { path: "pkg/unit.copper".into(), size: REMOTE.len() as u64, sha256: hash(REMOTE.as_bytes()) },
            LockedFile {
                path: "support/passive.copper".into(),
                size: SUPPORT.len() as u64,
                sha256: hash(SUPPORT.as_bytes()),
            },
        ];
        let locked = packages::LockedModule {
            module_path: MODULE.into(),
            version: VERSION.into(),
            source: format!("git:https://{MODULE}.git"),
            checksum: packages::inventory_checksum(&files),
            files,
        };
        object([
            ("schema", Value::from("copperscript.project-snapshot/v1")),
            ("entry", Value::from("board.copper")),
            ("manifest", Value::from(format!("module github.com/acme/board\nrequire {MODULE} {VERSION}"))),
            ("lock", Value::from(packages::lock_text(&IndexMap::from([((MODULE.into(), VERSION.into()), locked)])))),
            ("workspace_files", Value::Array(vec![Value::from("board.copper"), Value::from("src/wrapper.copper")])),
            ("files", object([("workspace/board.copper", Value::from(BOARD))])),
        ])
    }
    fn snapshot(raw: &Value) -> ProjectSnapshot {
        ProjectSnapshot::from_json(&copper_json::Dumps::compact().dumps(raw)).unwrap()
    }
    fn supply(raw: &mut Value, id: &str, source: &str) {
        let Value::Object(fields) = raw else { panic!() };
        let Value::Object(files) = fields.get_mut("files").unwrap() else { panic!() };
        files.insert(id.into(), Value::from(source));
    }
    #[test]
    fn transitive_local_remote_and_relative_imports_use_native_compiler_without_disk() {
        let mut raw = fixture();
        for (id, source) in [
            ("workspace/src/wrapper.copper", LOCAL),
            ("dependency/github.com/acme/parts/pkg/unit.copper", REMOTE),
            ("dependency/github.com/acme/parts/support/passive.copper", SUPPORT),
        ] {
            let requests = snapshot(&raw).plan().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].get("id").and_then(Value::as_str), Some(id));
            supply(&mut raw, id, source);
        }
        let project = snapshot(&raw);
        assert!(project.plan().unwrap().is_empty());
        let source = project.source.read(&project.entry).unwrap();
        let filename = paths::display(&project.entry);
        let design = compile_design_source_with(&source, &filename, true, true, Some(&mut project.resolver())).unwrap();
        let flat = crate::elaborate::elaborate(&design.electrical).unwrap();
        assert_eq!(flat.components.len(), 1);
        assert_eq!(flat.components[0].reference, "X/Inner/D/R");
        assert_eq!(project.provenance().get("files").and_then(Value::as_array).unwrap().len(), 4);
    }
    #[test]
    fn lock_source_bytes_and_inventory_are_not_silently_changed() {
        let mut raw = fixture();
        supply(&mut raw, "dependency/github.com/acme/parts/pkg/unit.copper", &REMOTE.replace("\r\n", "\n"));
        let error = ProjectSnapshot::from_json(&copper_json::Dumps::compact().dumps(&raw)).err().unwrap();
        assert!(error.message.contains("checksum mismatch"));
        let mut raw = fixture();
        if let Value::Object(fields) = &mut raw {
            fields.insert("lock".into(), Value::from(""));
        }
        supply(&mut raw, "workspace/src/wrapper.copper", LOCAL);
        let error = snapshot(&raw).plan().unwrap_err();
        assert_eq!(error.code, "PKG010");
        let mut raw = fixture();
        if let Value::Object(fields) = &mut raw {
            let lock = fields["lock"]
                .as_str()
                .unwrap()
                .replace("git:https://github.com/acme/parts.git", "git:https://github.com/other/parts.git");
            fields.insert("lock".into(), Value::from(lock));
        }
        assert!(
            ProjectSnapshot::from_json(&copper_json::Dumps::compact().dumps(&raw))
                .err()
                .unwrap()
                .message
                .contains("locked source differs")
        );
    }
    #[test]
    fn relative_imports_cannot_escape_workspace_and_cycles_remain_compiler_errors() {
        let mut raw = fixture();
        supply(&mut raw, "workspace/board.copper", &BOARD.replace("./src", "../../outside"));
        assert_eq!(snapshot(&raw).plan().unwrap_err().code, "PKG004");
        let mut raw = fixture();
        supply(&mut raw, "workspace/src/wrapper.copper", "module Wrapper { import self_ref \"./\"; }");
        let project = snapshot(&raw);
        assert!(project.plan().unwrap().is_empty());
        let source = project.source.read(&project.entry).unwrap();
        assert_eq!(
            compile_design_source_with(
                &source,
                &paths::display(&project.entry),
                true,
                true,
                Some(&mut project.resolver())
            )
            .unwrap_err()
            .code,
            "CMP021"
        );
    }
    #[test]
    fn manifest_replacements_keep_source_locks_and_module_boundaries() {
        for local in [false, true] {
            let mut raw = fixture();
            let Value::Object(fields) = &mut raw else { panic!() };
            let (directive, locked_source) = if local {
                (format!("replace {MODULE} => ./vendor"), "replace:./vendor".to_owned())
            } else {
                (
                    format!("replace {MODULE} => github.com/acme/fork fork-tag"),
                    "replace:git:https://github.com/acme/fork.git@fork-tag".to_owned(),
                )
            };
            fields.insert(
                "manifest".into(),
                Value::from(format!("module github.com/acme/board\nrequire {MODULE} {VERSION}\n{directive}")),
            );
            let lock = fields["lock"].as_str().unwrap().replace(&format!("git:https://{MODULE}.git"), &locked_source);
            fields.insert("lock".into(), Value::from(lock));
            if local {
                let Value::Array(files) = fields.get_mut("workspace_files").unwrap() else { panic!() };
                files.extend([Value::from("vendor/pkg/unit.copper"), Value::from("vendor/support/passive.copper")]);
            }
            for _ in 0..4 {
                let requests = snapshot(&raw).plan().unwrap();
                if requests.is_empty() {
                    break;
                }
                for request in requests {
                    let id = request.get("id").and_then(Value::as_str).unwrap();
                    let source = if id.ends_with("wrapper.copper") {
                        LOCAL
                    } else if id.ends_with("unit.copper") {
                        REMOTE
                    } else {
                        SUPPORT
                    };
                    if !local && id.starts_with("dependency/") {
                        assert_eq!(request.get("repository").and_then(Value::as_str), Some("github.com/acme/fork"));
                        assert_eq!(request.get("revision").and_then(Value::as_str), Some("fork-tag"));
                    }
                    supply(&mut raw, id, source);
                }
            }
            let project = snapshot(&raw);
            assert!(project.plan().unwrap().is_empty());
            let source = project.source.read(&project.entry).unwrap();
            let design = compile_design_source_with(
                &source,
                &paths::display(&project.entry),
                true,
                true,
                Some(&mut project.resolver()),
            )
            .unwrap();
            assert_eq!(crate::elaborate::elaborate(&design.electrical).unwrap().components.len(), 1);
        }
    }
}

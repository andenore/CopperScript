//! Resolve footprint references into physical footprints (`pcbir.footprints`)
//! and audit a board's selections (`physicalize.audit_resolved_footprints`).
//!
//! Managed URLs and manifest library bindings use the package cache and the
//! inventory lock; explicit local files and search roots stay available.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use copper_json::python_str_repr as repr;
use copper_physical::footprint::PhysicalFootprint;
use copper_physical::kicad_mod::{FootprintImportResult, load_kicad_mod, suffix};

use crate::elaborate::elaborate;
use crate::error::CopperScriptError;
use crate::model::{Board, ComponentInstance, PartDefinition};
use crate::packages::{PackageResolver, ResolvedAsset, ResolvedPackage, find_manifest, read_manifest};
use crate::paths;
use crate::syntax::Location;

/// `FootprintResolutionError` (and the `ValueError`s that reach the same callers).
pub type Result<T> = std::result::Result<T, String>;

const REMOTE: [&str; 3] = ["github.com/", "gitlab.com/", "https://"];

fn remote(reference: &str) -> bool {
    REMOTE.iter().any(|prefix| reference.starts_with(prefix))
}

fn is_kicad_mod(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    suffix(&name).to_lowercase() == ".kicad_mod"
}

fn compile_error(error: CopperScriptError) -> String {
    error.to_string()
}

/// `FootprintResolver`.
pub struct FootprintResolver {
    pub base_directory: PathBuf,
    pub search_roots: Vec<PathBuf>,
    pub strict: bool,
    pub locked: bool,
    pub offline: bool,
    packages: Option<PackageResolver>,
    verified: IndexMap<String, ResolvedPackage>,
    /// The last failed `resolve`: its message and Python error class
    /// (`FootprintResolutionError`, or the `ValueError` of an import's
    /// dataclass check that Python lets escape unwrapped).
    pub failure: Option<(String, &'static str)>,
}

impl FootprintResolver {
    pub fn new(base_directory: &Path, search_roots: &[PathBuf], strict: bool, locked: bool, offline: bool) -> Self {
        FootprintResolver {
            base_directory: paths::resolve(base_directory),
            search_roots: search_roots.iter().map(|root| paths::resolve(root)).collect(),
            strict,
            locked,
            offline,
            packages: None,
            verified: IndexMap::new(),
            failure: None,
        }
    }

    fn location(&self) -> Location {
        Location { filename: paths::display(&self.base_directory).into(), offset: 0, line: 1, column: 1 }
    }

    fn package_resolver(&mut self) -> std::result::Result<&mut PackageResolver, CopperScriptError> {
        if self.packages.is_none() {
            self.packages = Some(PackageResolver::for_source(&self.base_directory, self.locked, self.offline)?);
        }
        Ok(self.packages.as_mut().expect("just created"))
    }

    fn module(&mut self, module: &str) -> std::result::Result<ResolvedPackage, CopperScriptError> {
        if let Some(package) = self.verified.get(module) {
            return Ok(package.clone());
        }
        let location = self.location();
        let package = self.package_resolver()?.resolve(module, &location)?;
        self.verified.insert(module.to_owned(), package.clone());
        Ok(package)
    }

    fn asset(&mut self, reference: &str) -> std::result::Result<ResolvedAsset, CopperScriptError> {
        let location = self.location();
        let mut verified = std::mem::take(&mut self.verified);
        let result = self
            .package_resolver()
            .and_then(|packages| packages.resolve_asset_with(reference, &location, Some(&mut verified)));
        self.verified = verified;
        result
    }

    /// `managed_reference`: a managed asset identity, without fetching.
    pub fn managed_reference(&self, reference: &str) -> Result<Option<String>> {
        if reference.contains("://") && !reference.starts_with("https://") {
            return Err("managed footprint URLs must use HTTPS".to_owned());
        }
        if remote(reference) {
            if !reference.to_lowercase().ends_with(".kicad_mod") {
                return Err("managed footprint references must name a .kicad_mod file".to_owned());
            }
            return Ok(Some(reference.strip_prefix("https://").unwrap_or(reference).to_owned()));
        }
        if reference.contains(':')
            && !Path::new(reference).is_absolute()
            && !reference.contains('/')
            && !reference.contains('\\')
        {
            let (library, name) = reference.split_once(':').expect("contains ':'");
            if library.is_empty() || name.is_empty() || name.contains(':') || name == "." || name == ".." {
                return Err(format!("invalid KiCad footprint identifier {}", repr(reference)));
            }
            if let Some(manifest) = find_manifest(&self.base_directory) {
                let manifest = read_manifest(&manifest).map_err(compile_error)?;
                if let Some(binding) = manifest.footprint_libraries.get(library).filter(|binding| !binding.is_empty()) {
                    return Ok(Some(format!("{binding}/{name}.kicad_mod")));
                }
            }
        }
        Ok(None)
    }

    /// `_candidate_paths`: where a local reference may live.
    fn candidate_paths(&self, reference: &str, roots: Option<&[PathBuf]>) -> Result<Vec<PathBuf>> {
        let default: Vec<PathBuf> =
            std::iter::once(self.base_directory.clone()).chain(self.search_roots.iter().cloned()).collect();
        let roots = roots.unwrap_or(&default);
        let reference_path = Path::new(reference);
        let mut candidates: Vec<PathBuf> = Vec::new();
        if reference_path.is_absolute() {
            if !is_kicad_mod(reference_path) {
                return Err("absolute footprint references must name a .kicad_mod file".to_owned());
            }
            candidates.push(reference_path.to_path_buf());
        } else if is_kicad_mod(reference_path) || reference.contains('/') || reference.contains('\\') {
            if !is_kicad_mod(reference_path) {
                return Err("path-like footprint references must name a .kicad_mod file".to_owned());
            }
            candidates.extend(roots.iter().map(|root| root.join(reference_path)));
        } else if let Some((library, name)) = reference.split_once(':') {
            if library.is_empty() || name.is_empty() || name.contains(':') {
                return Err(format!("invalid KiCad footprint identifier {}", repr(reference)));
            }
            for root in roots {
                candidates.push(root.join(format!("{library}.pretty")).join(format!("{name}.kicad_mod")));
                candidates.push(root.join(library).join(format!("{name}.kicad_mod")));
            }
        } else {
            for root in roots {
                candidates.push(root.join(format!("{reference}.kicad_mod")));
                if root.is_dir() {
                    let mut libraries: Vec<PathBuf> = std::fs::read_dir(root)
                        .map(|entries| {
                            entries
                                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                                .filter(|path| {
                                    path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".pretty"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    libraries.sort();
                    candidates.extend(
                        libraries
                            .into_iter()
                            .filter(|path| path.is_dir())
                            .map(|path| path.join(format!("{reference}.kicad_mod"))),
                    );
                }
            }
        }
        let mut unique: IndexMap<String, PathBuf> = IndexMap::new();
        for candidate in candidates {
            let resolved = paths::resolve(&candidate);
            unique.entry(paths::display(&resolved).to_lowercase()).or_insert(resolved);
        }
        Ok(unique.into_values().collect())
    }

    /// `resolve`: load one reference, keeping it as the footprint's identity.
    pub fn resolve(&mut self, reference: &str) -> Result<FootprintImportResult> {
        let mut value_error = false;
        let result = self.resolve_reference(reference, &mut value_error);
        if let Err(message) = &result {
            let class = if value_error { "ValueError" } else { "FootprintResolutionError" };
            self.failure = Some((message.clone(), class));
        }
        result
    }

    fn resolve_reference(&mut self, reference: &str, value_error: &mut bool) -> Result<FootprintImportResult> {
        if reference.is_empty() || reference.contains('\0') {
            return Err("footprint reference cannot be empty".to_owned());
        }
        let target = self.managed_reference(reference)?;
        let mut asset: Option<ResolvedAsset> = None;
        let mut resolution = "local";
        let (candidates, matches): (Vec<PathBuf>, Vec<PathBuf>) = if let Some(target) = &target {
            let candidates = if remote(reference) || self.locked {
                Vec::new()
            } else {
                self.candidate_paths(reference, Some(&self.search_roots.clone()))?
            };
            let matches: Vec<PathBuf> = candidates.iter().filter(|path| path.is_file()).cloned().collect();
            if !matches.is_empty() {
                resolution = "local_override";
                (candidates, matches)
            } else {
                let found = self.asset(target).map_err(compile_error)?;
                let paths = vec![found.path.clone()];
                asset = Some(found);
                resolution = "managed";
                (paths.clone(), paths)
            }
        } else {
            let candidates = self.candidate_paths(reference, None)?;
            let mut matches: Vec<PathBuf> = candidates.iter().filter(|path| path.is_file()).cloned().collect();
            if matches.is_empty() && find_manifest(&self.base_directory).is_some() {
                let modules: Vec<String> =
                    self.package_resolver().map_err(compile_error)?.manifest.requirements.keys().cloned().collect();
                for module in modules {
                    let package = self.module(&module).map_err(compile_error)?;
                    let root = vec![package.directory.join("footprints")];
                    for path in self.candidate_paths(reference, Some(&root))? {
                        if path.is_file() && !matches.contains(&path) {
                            matches.push(path);
                        }
                    }
                }
                if let [path] = matches.as_slice() {
                    let owner =
                        self.verified.values().find(|package| paths::is_relative_to(path, &package.directory)).cloned();
                    if let Some(owner) = owner {
                        let relative = paths::as_posix(path.strip_prefix(&owner.directory).expect("inside module"));
                        asset = Some(self.asset(&format!("{}/{relative}", owner.module_path)).map_err(compile_error)?);
                        resolution = "managed";
                    }
                }
            }
            (candidates, matches)
        };
        if matches.is_empty() {
            let searched: Vec<String> = candidates.iter().take(6).map(|path| paths::display(path)).collect();
            let suffix =
                if searched.is_empty() { String::new() } else { format!("; searched {}", searched.join(", ")) };
            return Err(format!("cannot resolve footprint {}{suffix}", repr(reference)));
        }
        if matches.len() > 1 {
            let joined: Vec<String> = matches.iter().map(|path| paths::display(path)).collect();
            return Err(format!("footprint {} is ambiguous: {}", repr(reference), joined.join(", ")));
        }
        let path = &matches[0];
        let imported = load_kicad_mod(path, &paths::display(path), self.strict).map_err(|failure| {
            *value_error = matches!(failure, copper_physical::kicad_mod::ImportError::Value(_));
            failure.to_string()
        })?;
        if let Some(asset) = &asset
            && imported.footprint.metadata.get("source_sha256") != Some(&asset.sha256)
        {
            return Err(format!("footprint changed after module verification: {}", repr(reference)));
        }
        let expected = reference_name(reference);
        if imported.footprint.name != expected {
            return Err(format!(
                "footprint reference {} resolved to {}, but the file declares {}; expected {}",
                repr(reference),
                paths::display(path),
                repr(&imported.footprint.name),
                repr(&expected)
            ));
        }
        let mut metadata = imported.footprint.metadata.clone();
        metadata.insert("resolved_reference".to_owned(), reference.to_owned());
        metadata.insert("source_footprint_name".to_owned(), imported.footprint.name.clone());
        metadata.insert("resolution".to_owned(), resolution.to_owned());
        if let Some(target) = target {
            metadata.insert("managed_reference".to_owned(), target);
        }
        if let Some(asset) = asset {
            metadata.insert("source_asset".to_owned(), asset.identity);
            metadata.insert("module_path".to_owned(), asset.module_path);
            metadata.insert("module_version".to_owned(), asset.version);
            metadata.insert("module_checksum".to_owned(), asset.checksum);
        }
        let footprint = PhysicalFootprint {
            name: reference.to_owned(),
            source_library_id: Some(reference.to_owned()),
            metadata,
            ..imported.footprint
        };
        Ok(FootprintImportResult { footprint, warnings: imported.warnings, source_version: imported.source_version })
    }
}

/// `_reference_name`: the footprint name a reference must declare.
fn reference_name(reference: &str) -> String {
    if remote(reference) {
        let leaf = reference.rsplit('/').next().unwrap_or(reference);
        return leaf[..leaf.len().saturating_sub(".kicad_mod".len())].to_owned();
    }
    if reference.contains(':') && !Path::new(reference).is_absolute() {
        return reference.split_once(':').expect("contains ':'").1.to_owned();
    }
    let path = Path::new(reference);
    if is_kicad_mod(path) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        return name[..name.len() - suffix(&name).len()].to_owned();
    }
    reference.to_owned()
}

/// A component's selected footprint: its own, else its part's first.
pub fn selected_footprint(component: &ComponentInstance, part: &PartDefinition) -> Option<String> {
    component.footprint.clone().filter(|footprint| !footprint.is_empty()).or_else(|| part.footprints.first().cloned())
}

/// The natural sort key of `_natural`: digit runs compare as numbers.
fn natural(value: &str) -> Vec<(u8, num_bigint::BigInt, String)> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut digits = false;
    for c in value.chars() {
        if c.is_ascii_digit() != digits && !current.is_empty() {
            pieces.push((digits, std::mem::take(&mut current)));
        }
        digits = c.is_ascii_digit();
        current.push(c);
    }
    if !current.is_empty() {
        pieces.push((digits, current));
    }
    pieces
        .into_iter()
        .map(|(digits, piece)| {
            if digits {
                (0, piece.parse().expect("digits"), String::new())
            } else {
                (1, num_bigint::BigInt::from(0), piece.to_lowercase())
            }
        })
        .collect()
}

/// `_validate_footprint_pins`: every part pin has a pad, and no pad is extra.
fn validate_footprint_pins(
    part: &PartDefinition,
    component: &ComponentInstance,
    footprint: &PhysicalFootprint,
) -> Result<()> {
    let part_numbers: indexmap::IndexSet<&str> = part.pins.values().map(|pin| pin.number.as_str()).collect();
    let footprint_numbers: indexmap::IndexSet<&str> =
        footprint.pads.iter().map(|pad| pad.number.as_str()).filter(|number| !number.is_empty()).collect();
    let mut missing: Vec<&str> = part_numbers.difference(&footprint_numbers).copied().collect();
    let mut extra: Vec<&str> = footprint_numbers.difference(&part_numbers).copied().collect();
    missing.sort_by_key(|number| natural(number));
    extra.sort_by_key(|number| natural(number));
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing pads {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        details.push(format!("unknown pads {}", extra.join(", ")));
    }
    let identity = footprint.source_library_id.as_deref().filter(|id| !id.is_empty()).unwrap_or(&footprint.name);
    Err(format!(
        "component {} part {} is incompatible with footprint {}: {}",
        repr(&component.reference),
        repr(&part.name),
        repr(identity),
        details.join("; ")
    ))
}

/// `FootprintAuditEntry`.
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintAuditEntry {
    pub reference: String,
    pub components: Vec<String>,
    pub source_path: Option<String>,
    pub source_sha256: Option<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub provenance: Vec<(String, String)>,
}

impl FootprintAuditEntry {
    pub fn passed(&self) -> bool {
        self.errors.is_empty()
    }
}

/// `audit_resolved_footprints`: resolve every selected footprint, reporting all gaps.
pub fn audit_resolved_footprints(board: &Board, resolver: &mut FootprintResolver) -> Result<Vec<FootprintAuditEntry>> {
    let flat = elaborate(board)?;
    let mut selected: IndexMap<String, Vec<(&ComponentInstance, &PartDefinition)>> = IndexMap::new();
    for component in &flat.components {
        let Some(part) = flat.library.get(&component.part) else {
            return Err(format!(
                "component {} uses unknown part {}",
                repr(&component.reference),
                repr(&component.part)
            ));
        };
        if let Some(reference) = selected_footprint(component, part) {
            selected.entry(reference).or_default().push((component, part));
        }
    }
    selected.sort_keys();
    let mut entries = Vec::new();
    for (reference, uses) in &selected {
        let mut entry = FootprintAuditEntry {
            reference: reference.clone(),
            components: uses.iter().map(|(component, _)| component.reference.clone()).collect(),
            source_path: None,
            source_sha256: None,
            warnings: Vec::new(),
            errors: Vec::new(),
            provenance: Vec::new(),
        };
        match resolver.resolve(reference) {
            Ok(result) => {
                let metadata = &result.footprint.metadata;
                entry.warnings = result.warnings.clone();
                entry.source_path = metadata.get("source_path").cloned();
                entry.source_sha256 = metadata.get("source_sha256").cloned();
                let keys = [
                    "resolution",
                    "managed_reference",
                    "source_asset",
                    "module_path",
                    "module_version",
                    "module_checksum",
                ];
                entry.provenance = metadata
                    .iter()
                    .filter(|(key, _)| keys.contains(&key.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                entry.provenance.sort();
                for (component, part) in uses {
                    if let Err(message) = validate_footprint_pins(part, component, &result.footprint) {
                        entry.errors.push(message);
                    }
                }
            }
            Err(message) => entry.errors.push(message),
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// `prepare_footprint_dependencies`: resolve the board's managed footprints.
pub fn prepare_footprint_dependencies(board: &Board, resolver: &mut FootprintResolver) -> Result<Vec<String>> {
    let flat = elaborate(board)?;
    let mut references: Vec<String> = Vec::new();
    for component in &flat.components {
        let selected = match component.footprint.clone().filter(|footprint| !footprint.is_empty()) {
            Some(footprint) => Some(footprint),
            None => {
                let Some(part) = flat.library.get(&component.part) else {
                    return Err(format!(
                        "component {} uses unknown part {}",
                        repr(&component.reference),
                        repr(&component.part)
                    ));
                };
                part.footprints.first().cloned()
            }
        };
        if let Some(reference) = selected
            && !references.contains(&reference)
        {
            references.push(reference);
        }
    }
    references.sort();
    let mut managed = Vec::new();
    for reference in references {
        if resolver.managed_reference(&reference)?.is_some() {
            resolver.resolve(&reference)?;
            managed.push(reference);
        }
    }
    Ok(managed)
}

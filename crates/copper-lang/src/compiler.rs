//! The compiler entry points and package loading (`pcbir.compiler`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use copper_json::python_str_repr as repr;
use indexmap::IndexMap;

use crate::elaborate::elaborate;
use crate::error::CopperScriptError;
use crate::lower::{self, Result, error};
use crate::model::*;
use crate::packages::{PackageResolver, ResolvedPackage};
use crate::parser::parse;
use crate::paths;
use crate::syntax::*;

/// A board profile exported by a package: its qualified name and items.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalProfileDefinition {
    pub name: String,
    pub location: Location,
    pub items: Vec<MechanicalItem>,
}

/// Everything a package (or a board's imports) contributes.
#[derive(Clone, Debug, Default)]
pub struct PackageContents {
    pub parts: IndexMap<String, PartDefinition>,
    pub devices: IndexMap<String, DeviceDefinition>,
    pub modules: IndexMap<String, ModuleDefinition>,
    pub dependencies: Vec<Dependency>,
    pub profiles: IndexMap<String, MechanicalProfileDefinition>,
    pub macro_assets: IndexMap<String, MacroAssetReference>,
}

/// A compiled design: electrical, mechanical and artwork intent.
#[derive(Clone, Debug)]
pub struct Design {
    pub electrical: Board,
    pub mechanical: Option<crate::mechanical::MechanicalDesign>,
    pub silkscreen: crate::silkscreen::SilkscreenDesign,
}

/// `is_macro_import`: a `.json` data import.
pub fn is_macro_import(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let path = path.split('#').next().unwrap_or(path);
    path.to_lowercase().ends_with(".json")
}

/// Python's `Path.read_text`: UTF-8 with universal newlines.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let text =
        String::from_utf8(bytes).map_err(|failure| std::io::Error::new(std::io::ErrorKind::InvalidData, failure))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn read_document(path: &Path, resolver: &PackageResolver) -> Result<Document> {
    let filename = paths::display(path);
    let read = match &resolver.source_reader {
        Some(reader) => reader(path),
        None => read_source(path),
    };
    let source = read.map_err(|failure| {
        error(
            "CMP001",
            format!("cannot read source: {}", paths::python_os_error(&failure, Some(&filename))),
            &crate::packages::location(&filename, 1),
        )
    })?;
    parse(&source, &filename)
}

fn merge_parts(
    destination: &mut IndexMap<String, PartDefinition>,
    incoming: &IndexMap<String, PartDefinition>,
    location: &Location,
) -> Result<()> {
    for (name, part) in incoming {
        match destination.get(name) {
            Some(existing) if existing != part => {
                return Err(error("CMP003", format!("conflicting definitions for part {}", repr(name)), location));
            }
            Some(_) => {}
            None => {
                destination.insert(name.clone(), part.clone());
            }
        }
    }
    Ok(())
}

fn merge_devices(
    destination: &mut IndexMap<String, DeviceDefinition>,
    incoming: &IndexMap<String, DeviceDefinition>,
    location: &Location,
) -> Result<()> {
    for (name, device) in incoming {
        match destination.get(name) {
            Some(existing) if existing != device => {
                return Err(error("CMP053", format!("conflicting definitions for device {}", repr(name)), location));
            }
            Some(_) => {}
            None => {
                destination.insert(name.clone(), device.clone());
            }
        }
    }
    Ok(())
}

fn merge_profiles(
    destination: &mut IndexMap<String, MechanicalProfileDefinition>,
    incoming: &IndexMap<String, MechanicalProfileDefinition>,
    location: &Location,
) -> Result<()> {
    for (name, definition) in incoming {
        if destination.get(name).is_some_and(|existing| existing != definition) {
            return Err(error("MEC004", format!("conflicting board profile {}", repr(name)), location));
        }
        destination.insert(name.clone(), definition.clone());
    }
    Ok(())
}

fn register_module(
    modules: &mut IndexMap<String, ModuleDefinition>,
    definition: &ModuleDefinition,
    location: &Location,
) -> Result<()> {
    if modules.get(&definition.name).is_some_and(|existing| existing != definition) {
        return Err(error(
            "CMP023",
            format!("conflicting definitions for module {}", repr(&definition.name)),
            location,
        ));
    }
    modules.entry(definition.name.clone()).or_insert_with(|| definition.clone());
    Ok(())
}

/// `_unique_dependencies`: one entry per identity, sorted.
fn unique_dependencies(dependencies: Vec<Dependency>) -> Vec<Dependency> {
    let unique: BTreeMap<(String, String, String, String), Dependency> = dependencies
        .into_iter()
        .map(|item| {
            ((item.import_path.clone(), item.module_path.clone(), item.version.clone(), item.checksum.clone()), item)
        })
        .collect();
    unique.into_values().collect()
}

fn asset_reference(
    resolver: &PackageResolver,
    asset: String,
    location: &Location,
    symbol: &str,
) -> MacroAssetReference {
    MacroAssetReference {
        asset,
        base_directory: paths::display(&resolver.source_directory),
        location: location.clone(),
        locked: resolver.locked,
        offline: resolver.offline,
        symbol: symbol.to_owned(),
    }
}

/// `_load_imports`.
fn load_imports(
    document: &Document,
    resolver: &mut PackageResolver,
    stack: &[String],
    namespace_prefix: &str,
) -> Result<PackageContents> {
    let mut contents = PackageContents::default();
    let mut dependencies = Vec::new();
    let mut aliases: Vec<&str> = Vec::new();
    for declaration in &document.imports {
        let location = &declaration.location;
        if aliases.contains(&declaration.alias.as_str()) {
            return Err(error("CMP039", format!("duplicate import alias {}", repr(&declaration.alias)), location));
        }
        aliases.push(&declaration.alias);
        if is_macro_import(&declaration.path) {
            let identity = resolver.qualify_macro_asset(&declaration.path, location)?;
            contents.macro_assets.insert(declaration.alias.clone(), asset_reference(resolver, identity, location, ""));
            continue;
        }
        let resolved = resolver.resolve(&declaration.path, location)?;
        if stack.contains(&resolved.import_path) {
            let chain: Vec<&str> = stack.iter().map(String::as_str).chain([resolved.import_path.as_str()]).collect();
            return Err(error("CMP021", format!("cyclic package import: {}", chain.join(" -> ")), location));
        }
        let namespace = if namespace_prefix.is_empty() {
            declaration.alias.clone()
        } else {
            format!("{namespace_prefix}.{}", declaration.alias)
        };
        let mut nested_stack = stack.to_vec();
        nested_stack.push(resolved.import_path.clone());
        let package = load_package(&resolved, &namespace, resolver, &nested_stack)?;
        merge_parts(&mut contents.parts, &package.parts, location)?;
        merge_devices(&mut contents.devices, &package.devices, location)?;
        for definition in package.modules.values() {
            register_module(&mut contents.modules, definition, location)?;
        }
        merge_profiles(&mut contents.profiles, &package.profiles, location)?;
        let prefix = format!("{namespace}.");
        for (name, reference) in &package.macro_assets {
            let local = name.strip_prefix(&prefix).unwrap_or(name);
            contents.macro_assets.insert(format!("{}.{local}", declaration.alias), reference.clone());
        }
        dependencies.extend(package.dependencies);
        dependencies.push(Dependency {
            import_path: resolved.import_path,
            module_path: resolved.module_path,
            version: resolved.version,
            checksum: resolved.checksum,
        });
    }
    contents.dependencies = unique_dependencies(dependencies);
    Ok(contents)
}

/// Package sources, sorted like Python's `sorted(directory.glob("*.copper"))`.
pub fn package_sources(directory: &Path) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    if cfg!(windows) { name.to_lowercase().ends_with(".copper") } else { name.ends_with(".copper") }
                })
                .collect()
        })
        .unwrap_or_default();
    if cfg!(windows) {
        sources.sort_by_key(|path| paths::display(path).to_lowercase());
    } else {
        sources.sort();
    }
    sources
}

fn qualify_alias(aliases: &IndexMap<String, String>, raw: &str) -> Option<String> {
    let (head, tail) = raw.split_once('.')?;
    Some(format!("{}.{tail}", aliases.get(head).map(String::as_str).unwrap_or(head)))
}

struct LocalModules<'a> {
    namespace: &'a str,
    documents: IndexMap<String, &'a Document>,
    part_names: Vec<String>,
    imports: IndexMap<String, PackageContents>,
    macros: IndexMap<String, MacroAssetReference>,
    macro_names: Vec<String>,
    compiling: Vec<String>,
}

impl LocalModules<'_> {
    /// `compile_local` in `_load_package`.
    fn compile(
        &mut self,
        name: &str,
        modules: &mut IndexMap<String, ModuleDefinition>,
        parts: &IndexMap<String, PartDefinition>,
        devices: &IndexMap<String, DeviceDefinition>,
        resolver: &PackageResolver,
    ) -> Result<ModuleDefinition> {
        let qualified = format!("{}.{name}", self.namespace);
        if let Some(existing) = modules.get(&qualified) {
            return Ok(existing.clone());
        }
        let document = self.documents[name];
        if self.compiling.iter().any(|entry| entry == name) {
            let chain: Vec<&str> = self.compiling.iter().map(String::as_str).chain([name]).collect();
            return Err(error(
                "CMP021",
                format!("cyclic module dependency: {}", chain.join(" -> ")),
                &document.location,
            ));
        }
        self.compiling.push(name.to_owned());
        let aliases: IndexMap<String, String> =
            document.imports.iter().map(|d| (d.alias.clone(), format!("{}.{}", self.namespace, d.alias))).collect();
        // Python iterates a set here; sorted order keeps the result stable.
        let mut local_dependencies: Vec<String> = document
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::ModuleInstance(instance)
                    if !instance.module.contains('.') && self.documents.contains_key(&instance.module) =>
                {
                    Some(instance.module.clone())
                }
                _ => None,
            })
            .collect();
        local_dependencies.sort();
        local_dependencies.dedup();
        let mut available = modules.clone();
        for dependency in local_dependencies {
            let definition = self.compile(&dependency, modules, parts, devices, resolver)?;
            available.insert(definition.name.clone(), definition);
        }
        let imported = self.imports[name].clone();
        let mut extra_parts = parts.clone();
        merge_parts(&mut extra_parts, &imported.parts, &document.location)?;
        for (key, definition) in &imported.modules {
            available.insert(key.clone(), definition.clone());
        }
        let mut assets: IndexMap<String, MacroAssetReference> = self
            .macro_names
            .iter()
            .map(|macro_name| (macro_name.clone(), self.macros[&format!("{}.{macro_name}", self.namespace)].clone()))
            .collect();
        for (key, reference) in &imported.macro_assets {
            assets.insert(key.clone(), reference.clone());
        }
        let namespace = self.namespace.to_owned();
        let module_names: Vec<String> = self.documents.keys().cloned().collect();
        let part_names = self.part_names.clone();
        let resolve_part = |raw: &str| {
            qualify_alias(&aliases, raw).unwrap_or_else(|| {
                if part_names.iter().any(|part| part == raw) { format!("{namespace}.{raw}") } else { raw.to_owned() }
            })
        };
        let resolve_module = |raw: &str| {
            qualify_alias(&aliases, raw).unwrap_or_else(|| {
                if module_names.iter().any(|module| module == raw) {
                    format!("{namespace}.{raw}")
                } else {
                    raw.to_owned()
                }
            })
        };
        let mut definition = compile_module(
            document,
            &available,
            &extra_parts,
            &qualified,
            &resolve_part,
            &resolve_module,
            devices,
            &assets,
        )?;
        for component in &mut definition.components {
            if let Some(footprint) = &component.footprint {
                component.footprint = Some(resolver.qualify_footprint(footprint, &document.location)?);
            }
        }
        register_module(modules, &definition, &document.location)?;
        self.compiling.pop();
        Ok(definition)
    }
}

/// `_load_package`.
fn load_package(
    resolved: &ResolvedPackage,
    namespace: &str,
    resolver: &mut PackageResolver,
    stack: &[String],
) -> Result<PackageContents> {
    let sources = package_sources(&resolved.directory);
    if sources.is_empty() {
        let location = crate::packages::location(&paths::display(&resolved.directory), 1);
        return Err(error(
            "CMP022",
            format!("package {} exports no .copper files", repr(&resolved.import_path)),
            &location,
        ));
    }
    let documents: Vec<Document> = sources.iter().map(|path| read_document(path, resolver)).collect::<Result<_>>()?;
    for document in &documents {
        if !matches!(
            document.kind,
            DocumentKind::Device
                | DocumentKind::Part
                | DocumentKind::Module
                | DocumentKind::BoardProfile
                | DocumentKind::Macro
        ) {
            return Err(error(
                "CMP022",
                format!(
                    "package source declares a {}, expected device, part, module, board_profile, or macro",
                    document.kind.as_str()
                ),
                &document.location,
            ));
        }
    }
    let by_kind = |kind: DocumentKind| -> IndexMap<String, &Document> {
        documents
            .iter()
            .filter(|document| document.kind == kind)
            .map(|document| (document.name.clone(), document))
            .collect()
    };
    let count = |kind: DocumentKind| documents.iter().filter(|document| document.kind == kind).count();
    let device_documents = by_kind(DocumentKind::Device);
    let part_documents = by_kind(DocumentKind::Part);
    let module_documents = by_kind(DocumentKind::Module);
    let profile_documents = by_kind(DocumentKind::BoardProfile);
    let macro_documents = by_kind(DocumentKind::Macro);
    let first = &documents[0].location;
    if macro_documents.len() != count(DocumentKind::Macro) {
        return Err(error("MAC010", "package contains duplicate macro definition names", first));
    }
    let mut macros = IndexMap::new();
    for (name, document) in &macro_documents {
        let file_name = Path::new(&*document.location.filename)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let identity = resolver.qualify_macro_asset(&format!("./{file_name}"), &document.location)?;
        macros.insert(format!("{namespace}.{name}"), asset_reference(resolver, identity, &document.location, name));
    }
    if profile_documents.len() != count(DocumentKind::BoardProfile) {
        return Err(error("MEC004", "package contains duplicate board profile names", first));
    }
    if part_documents.len() != count(DocumentKind::Part) {
        return Err(error("CMP040", "package contains duplicate part names", first));
    }
    if module_documents.len() != count(DocumentKind::Module) {
        return Err(error("CMP041", "package contains duplicate module names", first));
    }
    if device_documents.len() != count(DocumentKind::Device) {
        return Err(error("CMP052", "package contains duplicate device names", first));
    }
    let mut devices = IndexMap::new();
    for (name, document) in &device_documents {
        let qualified = format!("{namespace}.{name}");
        devices.insert(qualified.clone(), lower::compile_device(document, &qualified)?);
    }
    let device_names: Vec<String> = device_documents.keys().cloned().collect();
    let resolve_device = |raw: &str| {
        if device_names.iter().any(|name| name == raw) { format!("{namespace}.{raw}") } else { raw.to_owned() }
    };
    let mut parts = IndexMap::new();
    for (name, document) in &part_documents {
        let qualified = format!("{namespace}.{name}");
        parts.insert(qualified.clone(), lower::compile_part(document, &qualified, &resolve_device, &devices)?);
    }
    for (name, document) in &part_documents {
        let part = parts.get_mut(&format!("{namespace}.{name}")).expect("compiled above");
        let footprints = std::mem::take(&mut part.footprints);
        for footprint in footprints {
            part.footprints.push(resolver.qualify_footprint(&footprint, &document.location)?);
        }
    }
    let mut modules: IndexMap<String, ModuleDefinition> = IndexMap::new();
    let mut profiles: IndexMap<String, MechanicalProfileDefinition> = IndexMap::new();
    let mut dependencies = Vec::new();
    let mut imports: IndexMap<String, PackageContents> = IndexMap::new();
    let unit_documents: Vec<(String, &Document)> = module_documents
        .iter()
        .map(|(name, document)| (name.clone(), *document))
        .chain(profile_documents.iter().map(|(name, document)| (format!("profile:{name}"), *document)))
        .collect();
    for (name, document) in unit_documents {
        let imported = load_imports(document, resolver, stack, namespace)?;
        merge_parts(&mut parts, &imported.parts, &document.location)?;
        merge_devices(&mut devices, &imported.devices, &document.location)?;
        for definition in imported.modules.values() {
            register_module(&mut modules, definition, &document.location)?;
        }
        merge_profiles(&mut profiles, &imported.profiles, &document.location)?;
        dependencies.extend(imported.dependencies.clone());
        imports.insert(name, imported);
    }
    let mut local = LocalModules {
        namespace,
        documents: module_documents.clone(),
        part_names: part_documents.keys().cloned().collect(),
        imports,
        macros: macros.clone(),
        macro_names: macro_documents.keys().cloned().collect(),
        compiling: Vec::new(),
    };
    for name in module_documents.keys() {
        local.compile(name, &mut modules, &parts, &devices, resolver)?;
    }
    for (name, document) in &profile_documents {
        let aliases: IndexMap<String, String> =
            document.imports.iter().map(|d| (d.alias.clone(), format!("{namespace}.{}", d.alias))).collect();
        let mut items = Vec::new();
        for declaration in &document.declarations {
            let Declaration::MechanicalItem(item) = declaration else { continue };
            let item = match item {
                MechanicalItem::ProfileUse(usage) => {
                    let profile = qualify_alias(&aliases, &usage.profile)
                        .unwrap_or_else(|| format!("{namespace}.{}", usage.profile));
                    MechanicalItem::ProfileUse(MechanicalProfileUseDecl { profile, ..usage.clone() })
                }
                MechanicalItem::Feature(feature) => {
                    let mut feature = feature.clone();
                    if let Some(Value::Scalar(Scalar::Str(footprint))) = feature.parameters.get("footprint").cloned() {
                        let qualified = resolver.qualify_footprint(&footprint, &feature.location)?;
                        feature.parameters.insert("footprint".to_owned(), Value::Scalar(Scalar::Str(qualified)));
                    }
                    MechanicalItem::Feature(feature)
                }
                other => other.clone(),
            };
            items.push(item);
        }
        let definition = MechanicalProfileDefinition {
            name: format!("{namespace}.{name}"),
            location: document.location.clone(),
            items,
        };
        merge_profiles(
            &mut profiles,
            &IndexMap::from([(definition.name.clone(), definition.clone())]),
            &document.location,
        )?;
    }
    Ok(PackageContents {
        parts,
        devices,
        modules,
        dependencies: unique_dependencies(dependencies),
        profiles,
        macro_assets: macros,
    })
}

/// The board or module body before instance validation (`_lower_unit`).
struct Unit {
    board: Board,
    ports: IndexMap<String, PinType>,
    instances: Vec<ModuleInstanceDecl>,
}

fn load_libraries(document: &Document) -> Result<IndexMap<String, PartDefinition>> {
    let mut library = IndexMap::new();
    for name in &document.libraries {
        let Some(parts) = crate::library::library(name) else {
            return Err(error("CMP002", format!("unknown library {}", repr(name)), &document.location));
        };
        merge_parts(&mut library, &parts, &document.location)?;
    }
    Ok(library)
}

/// `compile_macro_bindings`.
fn compile_macro_bindings(
    document: &Document,
    assets: &IndexMap<String, MacroAssetReference>,
) -> Result<Vec<MacroBinding>> {
    let mut result = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for declaration in &document.declarations {
        let Declaration::PhysicalMacro(declaration) = declaration else { continue };
        if names.contains(&declaration.name.as_str()) {
            return Err(error(
                "MAC001",
                format!("duplicate physical macro {}", repr(&declaration.name)),
                &declaration.location,
            ));
        }
        names.push(&declaration.name);
        let Some(asset) = assets.get(&declaration.asset_alias) else {
            return Err(error(
                "MAC001",
                format!("{} is not a macro asset import", repr(&declaration.asset_alias)),
                &declaration.location,
            ));
        };
        result.push(MacroBinding {
            name: declaration.name.clone(),
            asset: asset.clone(),
            scope: declaration.scope.clone(),
            mappings: declaration.mappings.clone(),
            location: declaration.location.clone(),
        });
    }
    Ok(result)
}

fn lower_unit(
    document: &Document,
    imported_parts: &IndexMap<String, PartDefinition>,
    resolve_part: &dyn Fn(&str) -> String,
    resolve_module: &dyn Fn(&str) -> String,
    imported_devices: &IndexMap<String, DeviceDefinition>,
    macro_assets: &IndexMap<String, MacroAssetReference>,
) -> Result<Unit> {
    let mut library = load_libraries(document)?;
    merge_parts(&mut library, imported_parts, &document.location)?;
    let devices = imported_devices.clone();
    let mut board = Board { name: document.name.clone(), ..Board::default() };
    let mut ports = IndexMap::new();
    let mut instances = Vec::new();
    let mut configurations = Vec::new();
    for declaration in &document.declarations {
        match declaration {
            Declaration::Component(component) => {
                if component.reference == "port" {
                    return Err(error(
                        "CMP026",
                        "'port' is reserved and cannot be a component reference",
                        &component.location,
                    ));
                }
                board.components.push(lower::component(component, &resolve_part(&component.part))?);
            }
            Declaration::Port(port) => {
                if ports.contains_key(&port.name) {
                    return Err(error("CMP027", format!("duplicate port {}", repr(&port.name)), &port.location));
                }
                let Some(pin_type) = PinType::parse(&port.pin_type) else {
                    let allowed: Vec<&str> = PinType::ALL.iter().map(|item| item.value()).collect();
                    return Err(error(
                        "CMP028",
                        format!("unknown port type {}; expected one of: {}", repr(&port.pin_type), allowed.join(", ")),
                        &port.location,
                    ));
                };
                ports.insert(port.name.clone(), pin_type);
            }
            Declaration::ModuleInstance(instance) => {
                if instance.reference == "port" {
                    return Err(error(
                        "CMP026",
                        "'port' is reserved and cannot be a module reference",
                        &instance.location,
                    ));
                }
                instances.push(ModuleInstanceDecl { module: resolve_module(&instance.module), ..instance.clone() });
            }
            Declaration::Net(net) => {
                let endpoints = net
                    .endpoints
                    .iter()
                    .map(|text| Endpoint::parse(text).map_err(|message| error("CMP010", message, &net.location)))
                    .collect::<Result<_>>()?;
                board.nets.push(Net { name: net.name.clone(), endpoints });
            }
            Declaration::NoConnect(item) => board.no_connects.extend(lower::no_connects(item)?),
            Declaration::Supply(supply) => board.supplies.push(lower::supply(supply)?),
            Declaration::Interface(interface) => board.interfaces.push(lower::interface(interface)?),
            Declaration::Constraint(constraint) => board.constraints.push(lower::constraint(constraint)?),
            Declaration::Configuration(configuration) => configurations.push(configuration),
            Declaration::PowerState(state) => board.power_states.push(lower::power_state(state)?),
            _ => {}
        }
    }
    for configuration in configurations {
        let selection = lower::configuration(configuration, &board.components, &library, &devices)?;
        board.peripheral_selections.push(selection);
    }
    board.physical_macros = compile_macro_bindings(document, macro_assets)?;
    board.library = library;
    board.devices = devices;
    Ok(Unit { board, ports, instances })
}

fn validate_module_ports(body: &Board, ports: &IndexMap<String, PinType>, location: &Location) -> Result<()> {
    let mut counts: IndexMap<&str, usize> = IndexMap::new();
    for net in &body.nets {
        for endpoint in &net.endpoints {
            if endpoint.component != "port" {
                continue;
            }
            if !ports.contains_key(&endpoint.pin) {
                return Err(error(
                    "CMP029",
                    format!("net references undeclared port {}", repr(&endpoint.pin)),
                    location,
                ));
            }
            *counts.entry(&endpoint.pin).or_default() += 1;
        }
    }
    for port in ports.keys() {
        match counts.get(port.as_str()).copied().unwrap_or(0) {
            0 => {
                return Err(error(
                    "CMP030",
                    format!("port {} is not connected to an internal net", repr(port)),
                    location,
                ));
            }
            1 => {}
            _ => {
                return Err(error(
                    "CMP031",
                    format!("port {} appears on multiple internal nets", repr(port)),
                    location,
                ));
            }
        }
    }
    Ok(())
}

fn validate_instances(
    body: &Board,
    declarations: &[ModuleInstanceDecl],
    modules: &IndexMap<String, ModuleDefinition>,
) -> Result<Vec<ModuleInstance>> {
    let mut known: Vec<String> = body.components.iter().map(|component| component.reference.clone()).collect();
    let mut by_owner: IndexMap<&str, Vec<(&str, &str)>> = IndexMap::new();
    for net in &body.nets {
        for endpoint in &net.endpoints {
            by_owner.entry(&endpoint.component).or_default().push((&endpoint.pin, &net.name));
        }
    }
    let mut result = Vec::new();
    for declaration in declarations {
        let location = &declaration.location;
        if known.contains(&declaration.reference) {
            return Err(error(
                "CMP032",
                format!("instance reference {} conflicts with another instance", repr(&declaration.reference)),
                location,
            ));
        }
        let Some(definition) = modules.get(&declaration.module) else {
            return Err(error("CMP033", format!("unknown module {}", repr(&declaration.module)), location));
        };
        let mut connections: IndexMap<&str, Vec<&str>> = IndexMap::new();
        for (port, net) in by_owner.get(declaration.reference.as_str()).cloned().unwrap_or_default() {
            if !definition.ports.contains_key(port) {
                return Err(error(
                    "CMP034",
                    format!("module {} has no port {}", definition.name, repr(port)),
                    location,
                ));
            }
            connections.entry(port).or_default().push(net);
        }
        for port in definition.ports.keys() {
            let mut nets = connections.get(port.as_str()).cloned().unwrap_or_default();
            nets.sort();
            nets.dedup();
            if nets.is_empty() {
                return Err(error(
                    "CMP038",
                    format!("module port {}.{port} is not connected", declaration.reference),
                    location,
                ));
            }
            if nets.len() > 1 {
                return Err(error(
                    "CMP035",
                    format!(
                        "module port {}.{port} is connected to multiple nets: {}",
                        declaration.reference,
                        nets.join(", ")
                    ),
                    location,
                ));
            }
        }
        result.push(ModuleInstance { reference: declaration.reference.clone(), module: declaration.module.clone() });
        known.push(declaration.reference.clone());
    }
    Ok(result)
}

fn component_path_exists(
    target: &str,
    components: &[ComponentInstance],
    instances: &[ModuleInstance],
    modules: &IndexMap<String, ModuleDefinition>,
) -> bool {
    let segments: Vec<&str> = target.split('/').collect();
    let (leaf, path) = segments.split_last().expect("split yields one segment");
    let (mut components, mut instances) = (components, instances);
    for segment in path {
        let Some(module) =
            instances.iter().find(|instance| instance.reference == *segment).map(|instance| &instance.module)
        else {
            return false;
        };
        let definition = &modules[module];
        components = &definition.components;
        instances = &definition.module_instances;
    }
    components.iter().any(|component| component.reference == *leaf)
}

fn module_hole_clearances(definition: &ModuleDefinition, modules: &IndexMap<String, ModuleDefinition>) -> Vec<String> {
    let mut result: Vec<String> = definition
        .constraints
        .iter()
        .filter(|constraint| constraint.kind == ConstraintKind::HoleClearance)
        .map(|constraint| constraint.targets[0].clone())
        .collect();
    for instance in &definition.module_instances {
        for reference in module_hole_clearances(&modules[&instance.module], modules) {
            result.push(format!("{}/{reference}", instance.reference));
        }
    }
    result
}

fn validate_hole_clearance_targets(
    document: &Document,
    body: &Board,
    instances: &[ModuleInstance],
    modules: &IndexMap<String, ModuleDefinition>,
) -> Result<()> {
    let mut covered: Vec<String> = instances
        .iter()
        .flat_map(|instance| {
            module_hole_clearances(&modules[&instance.module], modules)
                .into_iter()
                .map(move |reference| format!("{}/{reference}", instance.reference))
        })
        .collect();
    for declaration in &document.declarations {
        let Declaration::Constraint(constraint) = declaration else { continue };
        if constraint.kind != ConstraintKind::HoleClearance.value() {
            continue;
        }
        let target = &constraint.targets[0];
        if !component_path_exists(target, &body.components, instances, modules) {
            return Err(error(
                "CMP113",
                format!("hole_clearance target {} is not a component", repr(target)),
                &constraint.location,
            ));
        }
        if covered.contains(target) {
            return Err(error(
                "CMP114",
                format!("component {} already has a hole_clearance constraint", repr(target)),
                &constraint.location,
            ));
        }
        covered.push(target.clone());
    }
    Ok(())
}

fn validate_courtyard_gap_targets(
    document: &Document,
    body: &Board,
    instances: &[ModuleInstance],
    modules: &IndexMap<String, ModuleDefinition>,
) -> Result<()> {
    for declaration in &document.declarations {
        let Declaration::Constraint(constraint) = declaration else { continue };
        let applies = constraint.kind == ConstraintKind::CourtyardGap.value()
            || (constraint.kind == ConstraintKind::PlacementRegion.value()
                && constraint.parameters.contains_key("courtyard_gap"));
        if !applies {
            continue;
        }
        for target in &constraint.targets {
            if !component_path_exists(target, &body.components, instances, modules) {
                return Err(error(
                    "CMP123",
                    format!("courtyard_gap target {} is not a component", repr(target)),
                    &constraint.location,
                ));
            }
        }
    }
    Ok(())
}

/// `resolve_macro_bindings`: mappings through elaborated aliases.
pub fn resolve_macro_bindings(flat: &FlatElectricalView) -> Result<Vec<MacroBinding>> {
    let components: Vec<&str> = flat.components.iter().map(|component| component.reference.as_str()).collect();
    let modules: IndexMap<&str, &ElaboratedModuleInstance> =
        flat.module_instances.iter().map(|module| (module.path.as_str(), module)).collect();
    let mut aliases: IndexMap<&str, &str> =
        flat.nets.iter().map(|net| (net.name.as_str(), net.name.as_str())).collect();
    for (alias, net) in &flat.net_aliases {
        aliases.insert(alias, net);
    }
    let mut names: Vec<&str> = Vec::new();
    let mut owners: IndexMap<String, &MacroBinding> = IndexMap::new();
    let mut result = Vec::new();
    for binding in &flat.physical_macros {
        if names.contains(&binding.name.as_str()) {
            return Err(error(
                "MAC001",
                format!("duplicate physical macro {}", repr(&binding.name)),
                &binding.location,
            ));
        }
        names.push(&binding.name);
        if !binding.scope.is_empty() && !modules.contains_key(binding.scope.as_str()) {
            return Err(error(
                "MAC002",
                format!("unknown macro module scope {}", repr(&binding.scope)),
                &binding.location,
            ));
        }
        let mut mappings = Vec::new();
        let mut members: Vec<String> = Vec::new();
        let mut nets: Vec<String> = Vec::new();
        for mapping in &binding.mappings {
            let target = &mapping.target;
            let qualified =
                if binding.scope.is_empty() { target.clone() } else { format!("{}/{target}", binding.scope) };
            let value = if mapping.kind == "member" {
                if !components.contains(&qualified.as_str()) {
                    return Err(error(
                        "MAC002",
                        format!("unknown macro component {}", repr(&qualified)),
                        &mapping.location,
                    ));
                }
                if members.contains(&qualified) || owners.contains_key(&qualified) {
                    let previous = owners.get(&qualified).copied().unwrap_or(binding);
                    return Err(error(
                        "MAC003",
                        format!(
                            "component {} already owned by macro {} at {}",
                            repr(&qualified),
                            repr(&previous.name),
                            previous.location
                        ),
                        &mapping.location,
                    ));
                }
                members.push(qualified.clone());
                owners.insert(qualified.clone(), binding);
                Some(qualified.clone())
            } else {
                let found = if let Some((owner, port)) = target.split_once('.') {
                    if owner == "port" {
                        if !binding.scope.is_empty() {
                            modules[binding.scope.as_str()].connections.get(port).cloned()
                        } else {
                            let found: Vec<&String> = flat
                                .nets
                                .iter()
                                .filter(|net| net.endpoints.iter().any(|e| e.component == "port" && e.pin == port))
                                .map(|net| &net.name)
                                .collect();
                            if found.len() == 1 { Some(found[0].clone()) } else { None }
                        }
                    } else {
                        let path = if binding.scope.is_empty() {
                            owner.to_owned()
                        } else {
                            format!("{}/{owner}", binding.scope)
                        };
                        modules.get(path.as_str()).and_then(|module| module.connections.get(port).cloned())
                    }
                } else {
                    aliases.get(qualified.as_str()).map(|net| (*net).to_owned())
                };
                let Some(found) = found else {
                    return Err(error(
                        "MAC002",
                        format!("unknown macro net or module port {}", repr(&qualified)),
                        &mapping.location,
                    ));
                };
                if nets.contains(&found) {
                    return Err(error(
                        "MAC003",
                        format!("distinct macro net roles resolve to {}", repr(&found)),
                        &mapping.location,
                    ));
                }
                nets.push(found.clone());
                Some(found)
            };
            mappings.push(MacroMappingDecl { target: value.expect("resolved above"), ..mapping.clone() });
        }
        result.push(MacroBinding { scope: String::new(), mappings, ..binding.clone() });
    }
    Ok(result)
}

fn elaborated(board: &Board, location: &Location) -> Result<FlatElectricalView> {
    elaborate(board).map_err(|message| error("CMP036", message, location))
}

#[allow(clippy::too_many_arguments)]
fn compile_module(
    document: &Document,
    modules: &IndexMap<String, ModuleDefinition>,
    imported_parts: &IndexMap<String, PartDefinition>,
    name: &str,
    resolve_part: &dyn Fn(&str) -> String,
    resolve_module: &dyn Fn(&str) -> String,
    imported_devices: &IndexMap<String, DeviceDefinition>,
    macro_assets: &IndexMap<String, MacroAssetReference>,
) -> Result<ModuleDefinition> {
    let unit = lower_unit(document, imported_parts, resolve_part, resolve_module, imported_devices, macro_assets)?;
    let body = unit.board;
    validate_module_ports(&body, &unit.ports, &document.location)?;
    if !body.power_states.is_empty() {
        return Err(error("CMP072", "power states may only be declared on a board", &document.location));
    }
    let instances = validate_instances(&body, &unit.instances, modules)?;
    validate_hole_clearance_targets(document, &body, &instances, modules)?;
    validate_courtyard_gap_targets(document, &body, &instances, modules)?;
    if !body.physical_macros.is_empty() {
        let view = Board { module_instances: instances.clone(), module_definitions: modules.clone(), ..body.clone() };
        resolve_macro_bindings(&elaborated(&view, &document.location)?)?;
    }
    Ok(ModuleDefinition {
        name: name.to_owned(),
        ports: unit.ports,
        library: body.library,
        components: body.components,
        module_instances: instances,
        nets: body.nets,
        supplies: body.supplies,
        interfaces: body.interfaces,
        constraints: body.constraints,
        devices: body.devices,
        peripheral_selections: body.peripheral_selections,
        physical_macros: body.physical_macros,
        no_connects: body.no_connects,
    })
}

fn compile_board(document: &Document, imported: &PackageContents) -> Result<Board> {
    let identity = |raw: &str| raw.to_owned();
    let unit = lower_unit(document, &imported.parts, &identity, &identity, &imported.devices, &imported.macro_assets)?;
    if !unit.ports.is_empty() {
        return Err(error("CMP024", "ports may only be declared inside a module", &document.location));
    }
    let instances = validate_instances(&unit.board, &unit.instances, &imported.modules)?;
    validate_hole_clearance_targets(document, &unit.board, &instances, &imported.modules)?;
    validate_courtyard_gap_targets(document, &unit.board, &instances, &imported.modules)?;
    for net in &unit.board.nets {
        if net.endpoints.iter().any(|endpoint| endpoint.component == "port") {
            return Err(error("CMP025", "board nets cannot reference module boundary ports", &document.location));
        }
    }
    Ok(Board {
        module_instances: instances,
        module_definitions: imported.modules.clone(),
        dependencies: imported.dependencies.clone(),
        ..unit.board
    })
}

/// `_resolve_board_imports`.
pub fn resolve_board_imports(document: &Document, locked: bool, offline: bool) -> Result<PackageContents> {
    resolve_board_imports_with(document, locked, offline, None)
}

/// `_resolve_board_imports` with an editor's resolver (`package_resolver`).
pub fn resolve_board_imports_with(
    document: &Document,
    locked: bool,
    offline: bool,
    resolver: Option<&mut PackageResolver>,
) -> Result<PackageContents> {
    let filename = &*document.location.filename;
    if document.kind != DocumentKind::Board {
        return Err(error("CMP019", "compiler entry source must declare a board", &document.location));
    }
    if !document.imports.is_empty() && filename.starts_with('<') {
        return Err(error("CMP020", "package imports require a source filename", &document.location));
    }
    if document.imports.is_empty() {
        return Ok(PackageContents::default());
    }
    match resolver {
        Some(resolver) => load_imports(document, resolver, &[], ""),
        None => {
            let mut resolver = PackageResolver::for_source(&paths::resolve(Path::new(filename)), locked, offline)?;
            load_imports(document, &mut resolver, &[], "")
        }
    }
}

/// `compile_design_source`.
pub fn compile_design_source(source: &str, filename: &str, locked: bool, offline: bool) -> Result<Design> {
    compile_design_source_with(source, filename, locked, offline, None)
}

/// `compile_design_source(..., package_resolver=...)`: the editor's snapshot
/// resolver, whose `source_reader` supplies open buffers.
pub fn compile_design_source_with(
    source: &str,
    filename: &str,
    locked: bool,
    offline: bool,
    resolver: Option<&mut PackageResolver>,
) -> Result<Design> {
    let document = parse(source, filename)?;
    let imported = resolve_board_imports_with(&document, locked, offline, resolver)?;
    let mechanical = crate::mechanical::lower_mechanical(&document, &imported.profiles)?;
    let silkscreen = crate::silkscreen::lower_silkscreen(&document)?;
    let electrical = compile_board(&document, &imported)?;
    let has_macros = !electrical.physical_macros.is_empty()
        || electrical.module_definitions.values().any(|module| !module.physical_macros.is_empty());
    if has_macros {
        resolve_macro_bindings(&elaborated(&electrical, &document.location)?)?;
    }
    if let Some(mechanical) = &mechanical {
        let references: Vec<String> = elaborated(&electrical, &document.location)?
            .components
            .into_iter()
            .map(|component| component.reference)
            .collect();
        for (reference, location) in mechanical.bound_references() {
            if !crate::pyvalue::as_str(&reference).is_some_and(|name| references.iter().any(|known| known == name)) {
                return Err(error(
                    "MEC005",
                    format!("mechanical binding references unknown component {}", crate::pyvalue::repr(&reference)),
                    location.unwrap_or(&document.location),
                ));
            }
        }
    }
    Ok(Design { electrical, mechanical, silkscreen })
}

/// `compile_design_file`: read the resolved path and compile it.
pub fn compile_design_file(path: &Path, locked: bool, offline: bool) -> Result<Design> {
    let resolved = paths::resolve(path);
    let filename = paths::display(&resolved);
    let source = read_source(&resolved).map_err(|failure| {
        error("CMP001", paths::python_os_error(&failure, Some(&filename)), &crate::packages::location(&filename, 1))
    })?;
    compile_design_source(&source, &filename, locked, offline)
}

/// `pcbir.loader.load_design`'s failure: a load error or a located diagnostic.
#[derive(Debug)]
pub enum LoadError {
    /// `BoardLoadError` raised before compiling (missing file, wrong suffix).
    Load(String),
    Compile(CopperScriptError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Load(message) => formatter.write_str(message),
            LoadError::Compile(failure) => failure.fmt(formatter),
        }
    }
}

/// `pcbir.loader.load_design`.
pub fn load_design(path: &Path, locked: bool, offline: bool) -> std::result::Result<Design, LoadError> {
    let resolved = paths::resolve(path);
    if !resolved.is_file() {
        return Err(LoadError::Load(format!("board file does not exist: {}", paths::display(&resolved))));
    }
    let suffix =
        paths::extension(&resolved.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())
            .to_owned();
    if suffix != ".copper" {
        return Err(LoadError::Load(format!("unsupported source format {}; expected .copper", repr(&suffix))));
    }
    compile_design_file(&resolved, locked, offline).map_err(LoadError::Compile)
}

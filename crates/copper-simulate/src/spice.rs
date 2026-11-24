//! `sim spice` blocks: the model file resolved ([`copper_lang::spice_models`]),
//! read once per run by `copper-spice` and expanded per step.
//!
//! Errors carry the codes of the language reference: `SIM077` for a model
//! file that cannot be found, is not cached or does not match its hash,
//! `SIM078` for a netlist the reader rejects (with the file, line, element
//! and construct).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use copper_lang::simulation::{SpiceModel, SpiceSource};
use copper_sim::{Circuit, NodeId};
use copper_spice::{Dialect, Instance, Library};

/// A library read from a file, with the files it included and their SHA-256
/// at the time it was read.
struct Cached {
    library: Arc<Library>,
    includes: Vec<(PathBuf, String)>,
}

/// File, dialect and content hash -> the library read from it. The root
/// file's hash is in the key; an entry is used only while every file it
/// included still has the hash it was read with, so the cache holds on the
/// whole dependency closure, not the root file alone.
type Libraries = Mutex<BTreeMap<(PathBuf, String, String), Cached>>;

fn libraries() -> &'static Libraries {
    static CACHE: OnceLock<Libraries> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The SHA-256 of a file's bytes, `None` when it cannot be read.
fn file_sha256(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| copper_lang::remote_models::sha256_hex(&bytes))
}

/// The model's library, read once per file, dialect and content hash of the
/// file and of every file it includes.
pub fn library(model: &SpiceModel) -> Result<(Arc<Library>, PathBuf, String), String> {
    let (library, path, hash, _) = load(model)?;
    Ok((library, path, hash))
}

/// The files the model is read from, each with its SHA-256: the model file
/// first, then the files it includes (`.INCLUDE`, `.LIB FILE`), in the order
/// they were read. For a vendor model the one cached file.
pub fn dependencies(model: &SpiceModel) -> Result<Vec<(PathBuf, String)>, String> {
    let (_, path, hash, includes) = load(model)?;
    Ok(std::iter::once((path, hash)).chain(includes).collect())
}

/// A loaded library, its model file and that file's hash, and the files it included with theirs.
type Loaded = (Arc<Library>, PathBuf, String, Vec<(PathBuf, String)>);

fn load(model: &SpiceModel) -> Result<Loaded, String> {
    // A file registered by hand carries its own code (`SIM108`), every other missing file is `SIM077`.
    let (path, hash) = copper_lang::spice_models::resolve(model)
        .map_err(|message| if message.starts_with("SIM108") { message } else { format!("SIM077: {message}") })?;
    let dialect = Dialect::parse(&model.dialect).unwrap_or(Dialect::Spice3);
    let key = (path.clone(), model.dialect.clone(), hash.clone());
    let mut cache = libraries().lock().map_err(|_| "the SPICE library cache is poisoned".to_string())?;
    if let Some(cached) = cache.get(&key)
        && cached.includes.iter().all(|(file, sha256)| file_sha256(file).as_ref() == Some(sha256))
    {
        return Ok((cached.library.clone(), path, hash, cached.includes.clone()));
    }
    // A fetched vendor file is one file; a local file may include its neighbours.
    let local = matches!(model.source, SpiceSource::File(_));
    // A failure is not kept: a file it is missing may appear without the root file changing.
    let library = Arc::new(Library::load(&path, dialect, local).map_err(|e| format!("SIM078: {e}"))?);
    let includes: Vec<(PathBuf, String)> = library
        .files()
        .iter()
        .filter(|file| **file != path)
        .map(|file| (file.clone(), file_sha256(file).unwrap_or_default()))
        .collect();
    cache.insert(key, Cached { library: library.clone(), includes: includes.clone() });
    Ok((library, path, hash, includes))
}

/// Expand the model's subcircuit as `instance` on `ports` (in terminal order)
/// at `celsius` degrees Celsius.
pub fn instantiate(
    c: &mut Circuit,
    instance: &str,
    model: &SpiceModel,
    ports: &[NodeId],
    celsius: f64,
) -> Result<Instance, String> {
    let (lib, path, _) = library(model)?;
    if let Some(declared) = lib.ports(&model.subckt)
        && declared.len() != ports.len()
    {
        return Err(format!(
            "SIM078: .SUBCKT {} in {} has {} nodes ({}), but the sim spice block lists {} terminals",
            model.subckt.to_ascii_uppercase(),
            copper_lang::paths::display(&path),
            declared.len(),
            declared.join(" "),
            ports.len()
        ));
    }
    lib.instantiate_at(c, instance, &model.subckt, ports, &model.params, celsius).map_err(|e| format!("SIM078: {e}"))
}

/// `spice OPA810 (opa810_a.lib, sha256 1a2b3c4d...)` for the model list.
pub fn describe(model: &SpiceModel) -> String {
    let file = match &model.source {
        SpiceSource::File(file) => file.clone(),
        SpiceSource::Url { url, .. } => url.clone(),
        SpiceSource::Registered { member, .. } => match member {
            Some(member) => format!("registered by hand, {member}"),
            None => "registered by hand".to_owned(),
        },
    };
    let params: String = model.params.iter().map(|(name, value)| format!(", {name} = {value}")).collect();
    match library(model) {
        Ok((lib, _, hash)) => {
            let notes: String = lib.notes().iter().map(|note| format!("; {note}")).collect();
            format!("spice {} ({file}, {} dialect, sha256 {hash}{params}{notes})", model.subckt, model.dialect)
        }
        Err(_) => format!("spice {} ({file}, {} dialect{params})", model.subckt, model.dialect),
    }
}

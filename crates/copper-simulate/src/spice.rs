//! `sim spice` blocks: the model file resolved ([`copper_lang::spice_models`]),
//! read once per run by `copper-spice` and expanded per step.
//!
//! Errors carry the codes of the language reference: `SIM077` for a model
//! file that cannot be found, is not cached or does not match its hash,
//! `SIM078` for a netlist the reader rejects (with the file, line, element
//! and construct).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use copper_lang::simulation::{SpiceModel, SpiceSource};
use copper_sim::{Circuit, NodeId};
use copper_spice::{Dialect, Instance, Library};

type Loaded = Result<Arc<Library>, String>;
/// File, dialect and content hash -> the library read from it.
type Libraries = Mutex<BTreeMap<(PathBuf, String, String), Loaded>>;

fn libraries() -> &'static Libraries {
    static CACHE: OnceLock<Libraries> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The model's library, read once per file, dialect and content hash.
pub fn library(model: &SpiceModel) -> Result<(Arc<Library>, PathBuf, String), String> {
    let (path, hash) = copper_lang::spice_models::resolve(model).map_err(|message| format!("SIM077: {message}"))?;
    let dialect = Dialect::parse(&model.dialect).unwrap_or(Dialect::Spice3);
    let key = (path.clone(), model.dialect.clone(), hash.clone());
    let mut cache = libraries().lock().map_err(|_| "the SPICE library cache is poisoned".to_string())?;
    let loaded = cache
        .entry(key)
        .or_insert_with(|| {
            // A fetched vendor file is one file; a local file may include its neighbours.
            let local = matches!(model.source, SpiceSource::File(_));
            Library::load(&path, dialect, local).map(Arc::new).map_err(|e| format!("SIM078: {e}"))
        })
        .clone();
    Ok((loaded?, path, hash))
}

/// Expand the model's subcircuit as `instance` on `ports` (in terminal order).
pub fn instantiate(c: &mut Circuit, instance: &str, model: &SpiceModel, ports: &[NodeId]) -> Result<Instance, String> {
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
    lib.instantiate(c, instance, &model.subckt, ports, &model.params).map_err(|e| format!("SIM078: {e}"))
}

/// `spice OPA810 (opa810_a.lib, sha256 1a2b3c4d...)` for the model list.
pub fn describe(model: &SpiceModel) -> String {
    let file = match &model.source {
        SpiceSource::File(file) => file.clone(),
        SpiceSource::Url { url, .. } => url.clone(),
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

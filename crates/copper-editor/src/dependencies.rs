//! `pcbir.editor.dependencies`: initial-open dependency preparation,
//! separate from offline editing.

use std::path::{Path, PathBuf};

use copper_lang::packages::{PrepareFailure, find_manifest, prepare_dependencies};
use copper_lang::paths;
use copper_lang::syntax::DocumentKind;

use crate::source::SourceSnapshot;
use crate::{Error, Result, value};

/// `dependency_context`: the manifest a managed definition is pinned by.
pub fn dependency_context(source: &Path, project: Option<&Path>) -> Result<PathBuf> {
    let source = paths::resolve(source);
    if let Some(project) = project {
        return match find_manifest(&paths::resolve(project)) {
            Some(manifest) => Ok(manifest),
            None => value("the selected --project has no copper.mod"),
        };
    }
    let parts: Vec<_> = source.components().collect();
    for index in 0..parts.len().saturating_sub(2) {
        let part = parts[index].as_os_str().to_string_lossy();
        if part.to_lowercase() == ".copper-cache" && parts[index + 1].as_os_str() == "pkg" {
            let prefix: PathBuf = parts[..index].iter().collect();
            if let Some(manifest) = find_manifest(&prefix) {
                return Ok(manifest);
            }
        }
    }
    Ok(source)
}

/// `Path.read_bytes()` with Python's `OSError` text.
pub fn read_source(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| Error::Os(paths::python_os_error(&error, Some(&paths::display(path)))))
}

/// `prepare_editor_dependencies` for a source read from disk.
pub fn prepare_editor_dependencies(
    source: &Path,
    offline: bool,
    locked: bool,
    project: Option<&Path>,
    progress: &mut dyn FnMut(&str),
) -> Result<()> {
    let suffix = source.extension().map(|e| e.to_string_lossy().to_lowercase());
    if suffix.as_deref() == Some("kicad_mod") {
        return value(
            "A .kicad_mod file is a KiCad footprint. Open the hard macro .copper definition, or open its board and select Open definition.",
        );
    }
    let filename = paths::display(source);
    let snapshot = SourceSnapshot::new(read_source(source)?, &filename)?;
    let document = copper_lang::parse(&snapshot.text, &filename)?;
    if !matches!(document.kind, DocumentKind::Board | DocumentKind::Macro | DocumentKind::BoardProfile) {
        return value("physical editor supports board, macro and board_profile documents");
    }
    let context = dependency_context(source, if document.kind != DocumentKind::Board { project } else { None })?;
    match prepare_dependencies(&context, offline, locked, progress) {
        Ok(_) => Ok(()),
        Err(PrepareFailure::Copper(error)) => Err(Error::Copper(error)),
        Err(PrepareFailure::Os(message)) => Err(Error::Os(message)),
    }
}

//! Write a complete generated PCB project (`backends.kicad_project.write_kicad_project`).

use std::path::{Component, Path, PathBuf};

use copper_json::{Dumps, Value as Json};
use copper_lang::paths;

use crate::kicad_pcb::{KICAD_RULES_MARKER, Manifest};

/// Why writing failed: Python's `OSError` or `ValueError`, both printed as
/// `OUTPUT ERROR: ...`.
pub type WriteResult<T> = std::result::Result<T, String>;

/// `PurePath.suffix`.
fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

/// `PurePath.with_suffix`.
pub fn with_suffix(path: &Path, new: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let old = suffix(&name);
    let stem = &name[..name.len() - old.len()];
    path.with_file_name(format!("{stem}{new}"))
}

fn os_error(error: &std::io::Error, path: &Path) -> String {
    paths::python_os_error(error, Some(&paths::display(path)))
}

/// An `os.mkdir` or `os.unlink` failure, which Windows reports as `[WinError N]`.
fn os_call_error(error: &std::io::Error, path: &Path) -> String {
    paths::python_os_call_error(error, Some(&paths::display(path)))
}

fn casefold(path: &Path) -> String {
    paths::display(&paths::resolve(path)).to_lowercase()
}

/// Write every artifact beside `output`; generated library assets go below
/// the output directory. Returns the written destinations.
pub fn write_kicad_project(manifest: &Manifest, output: &Path) -> WriteResult<Vec<PathBuf>> {
    if manifest.artifacts.len() < 3 {
        return Err("a complete KiCad PCB artifact manifest is required".to_owned());
    }
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let root = paths::resolve(parent);
    let mut destinations = Vec::new();
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        let parts: Vec<&str> = artifact.name.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty()
            || artifact.name.starts_with('/')
            || parts.iter().any(|p| *p == ".." || *p == ".")
            || artifact.name.contains('\\')
            || artifact.name.contains(':')
        {
            return Err(format!("unsafe generated artifact path: {}", copper_json::python_str_repr(&artifact.name)));
        }
        let single = parts.len() == 1;
        let path = if index == 0 {
            output.to_path_buf()
        } else if index == 1 {
            with_suffix(output, ".kicad_pro")
        } else if single && suffix(parts[0]) == ".kicad_dru" {
            with_suffix(output, ".kicad_dru")
        } else if single && parts[0].ends_with(".via-process.json") {
            with_suffix(output, ".via-process.json")
        } else {
            parts.iter().fold(root.clone(), |path, part| path.join(part))
        };
        if !paths::resolve(&path).starts_with(&root) {
            return Err(format!("artifact escapes output directory: {}", copper_json::python_str_repr(&artifact.name)));
        }
        destinations.push(path);
    }
    let folded: Vec<String> = destinations.iter().map(|p| casefold(p)).collect();
    if crate::has_duplicates(&folded) {
        return Err("generated artifact destinations collide".to_owned());
    }
    let Some(table) = manifest.artifacts.iter().find(|a| a.name == "fp-lib-table") else {
        return Err("KiCad export omitted its footprint library table".to_owned());
    };
    let existing_table = root.join("fp-lib-table");
    if existing_table.exists() {
        let current = std::fs::read(&existing_table).map_err(|e| os_error(&e, &existing_table))?;
        if current != table.content.as_bytes() {
            return Err(
                "refusing to overwrite an unrelated fp-lib-table; export to a separate build directory".to_owned()
            );
        }
    }
    let rules = with_suffix(output, ".kicad_dru");
    let stale = !destinations.contains(&rules)
        && rules.is_file()
        && std::fs::read(&rules)
            .map(|bytes| String::from_utf8_lossy(&bytes).contains(KICAD_RULES_MARKER))
            .unwrap_or(false);
    let process = with_suffix(output, ".via-process.json");
    let generated_process = process.is_file() && is_generated_via_process(&process);
    if process.exists() && destinations.contains(&process) && !generated_process {
        return Err("refusing to overwrite an unrelated via-process sidecar".to_owned());
    }
    if stale {
        std::fs::remove_file(&rules).map_err(|e| os_call_error(&e, &rules))?;
    }
    if generated_process && !destinations.contains(&process) {
        std::fs::remove_file(&process).map_err(|e| os_call_error(&e, &process))?;
    }
    for (artifact, path) in manifest.artifacts.iter().zip(&destinations) {
        if let Some(directory) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            create_directories(directory)?;
        }
        let mut content = artifact.content.clone();
        if suffix(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) == ".kicad_pro" {
            let mut project = copper_json::parse(&content).map_err(|e| e.to_string())?;
            if let Json::Object(object) = &mut project
                && let Some(Json::Object(meta)) = object.get_mut("meta")
            {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                meta.insert("filename".to_owned(), Json::Str(name));
            }
            content = Dumps::new().indent(2).sort_keys(true).dumps(&project) + "\n";
        }
        std::fs::write(path, content.as_bytes()).map_err(|e| os_error(&e, path))?;
    }
    Ok(destinations)
}

/// `Path.mkdir(parents=True, exist_ok=True)`, reporting the failing directory.
fn create_directories(directory: &Path) -> WriteResult<()> {
    if directory.is_dir() {
        return Ok(());
    }
    let mut current = PathBuf::new();
    for component in directory.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) || current.is_dir() {
            continue;
        }
        if let Err(error) = std::fs::create_dir(&current)
            && !current.is_dir()
        {
            return Err(os_call_error(&error, &current));
        }
    }
    Ok(())
}

fn is_generated_via_process(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let Ok(Json::Object(document)) = copper_json::parse(&text) else { return false };
    document.get("schema").and_then(Json::as_str) == Some("copperscript-via-process/v0.1")
        && document.get("generated_by").and_then(Json::as_str) == Some("CopperScript")
}

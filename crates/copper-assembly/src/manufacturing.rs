//! Native KiCad manufacturing files, explicitly without independent CAM
//! signoff (`pcbir.manufacturing_files`).
//!
//! This consumes the final native board, not a reconstructed physical IR:
//! filled copper and native edits survive unchanged. `kicad-cli` refills,
//! checks and exports it in a staging directory that is published only when
//! every step and inventory check passed.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Dumps, Object, Value};
use copper_lang::paths;
use copper_pcb::project::with_suffix;
use copper_physical::kicad_mod::{self, Node};
use copper_units::Decimal;

use crate::csv::{self, Field};
use crate::files::{self, open_error, os_error, sha256_hex};
use crate::zip;

/// A refused export, worded like the Python exception the CLI prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `ManufacturingFilesError` and the other `ValueError`s.
    Files(String),
    /// A failed `kicad-cli` step (`RuntimeError`).
    Runtime(String),
    /// `OSError`, including `FileExistsError` for existing output.
    Os(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Files(message) | Error::Runtime(message) | Error::Os(message) => formatter.write_str(message),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn refuse<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Files(message.into()))
}

fn read(path: &Path, shown: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|failure| Error::Os(open_error(&failure, shown)))
}

/// `path.read_text(encoding=...)`: decoded, with universal newlines.
fn read_text(path: &Path, shown: &str, signature: bool) -> Result<String> {
    let bytes = read(path, shown)?;
    let bytes = if signature { bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes) } else { &bytes[..] };
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(failure) => refuse(files::decode_error(bytes, failure)),
    }
}

/// A CSV file read in text mode with `newline=""` (no newline translation).
fn read_csv(path: &Path, shown: &str) -> Result<csv::DictReader> {
    read_text(path, shown, true).map(|text| csv::dict_reader(&text))
}

/// `_number`: a finite decimal, formatted fixed point.
fn number(value: Option<&str>) -> Result<String> {
    let text = value.unwrap_or("");
    match Decimal::parse(text) {
        Ok(result) => Ok(result.to_fixed_string()),
        Err(_) => refuse(format!("invalid placement number: {}", copper_json::python_str_repr(text))),
    }
}

fn field<'a>(row: &'a indexmap::IndexMap<String, Option<String>>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(|value| value.as_deref())
}

/// `write_jlcpcb_cpl(native_csv, output, references=...)`: rename KiCad's
/// columns, never guess supplier rotation offsets, and require exact BOM/CPL
/// reference equality. `shown` is `str(native_csv)`.
pub fn write_jlcpcb_cpl(native_csv: &Path, shown: &str, output: &Path, references: &[String]) -> Result<()> {
    let reader = read_csv(native_csv, shown)?;
    let names = reader.fieldnames.clone().unwrap_or_default();
    if !["Ref", "PosX", "PosY", "Rot", "Side"].iter().all(|key| names.iter().any(|name| name == key)) {
        return refuse("unexpected KiCad position CSV columns");
    }
    let mut selected: Vec<[String; 5]> = Vec::new();
    let mut seen: Vec<Option<&str>> = Vec::new();
    for row in &reader.rows {
        let reference = field(row, "Ref");
        if seen.contains(&reference) {
            return refuse(format!("duplicate position reference: {}", reference.unwrap_or("None")));
        }
        seen.push(reference);
        let Some(reference) = reference.filter(|reference| references.iter().any(|r| r == reference)) else {
            continue; // Explicit non-populated parts do not belong in the assembly CPL.
        };
        let side = match field(row, "Side") {
            Some("top") => "Top",
            Some("bottom") => "Bottom",
            other => return refuse(format!("unknown placement side: {}", other.unwrap_or("None"))),
        };
        selected.push([
            reference.to_owned(),
            number(field(row, "PosX"))?,
            number(field(row, "PosY"))?,
            number(field(row, "Rot"))?,
            side.to_owned(),
        ]);
    }
    let mut missing: Vec<&String> =
        references.iter().filter(|reference| !seen.contains(&Some(reference.as_str()))).collect();
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        let names: Vec<&str> = missing.iter().map(|name| name.as_str()).collect();
        return refuse(format!("BOM references missing from position export: {}", names.join(", ")));
    }
    selected.sort();
    let mut content = String::new();
    csv::write_row(&mut content, &["Designator", "Mid X", "Mid Y", "Rotation", "Layer"].map(Field::from));
    for row in &selected {
        csv::write_row(&mut content, &row.each_ref().map(|value| Field::Text(value)));
    }
    std::fs::write(output, content.as_bytes())
        .map_err(|failure| Error::Os(open_error(&failure, &paths::display(output))))
}

/// `_bom_references`: the designators of a JLCPCB selection BOM.
fn bom_references(path: &Path, shown: &str) -> Result<Vec<String>> {
    let reader = read_csv(path, shown)?;
    let names = reader.fieldnames.clone().unwrap_or_default();
    let required = ["Comment", "Designator", "Footprint", "LCSC Part #"];
    if !required.iter().all(|key| names.iter().any(|name| name == key)) {
        return refuse("BOM is not a JLCPCB selection CSV");
    }
    let mut references: Vec<String> = Vec::new();
    for row in &reader.rows {
        if required.iter().any(|key| field(row, key).unwrap_or("").trim_matches(copper_json::python_isspace).is_empty())
        {
            return refuse("BOM contains an incomplete selection");
        }
        for reference in field(row, "Designator").unwrap_or("").split(',') {
            let reference = reference.trim_matches(copper_json::python_isspace);
            if reference.is_empty() || references.iter().any(|known| known == reference) {
                return refuse(format!("empty or duplicate BOM reference: {reference}"));
            }
            references.push(reference.to_owned());
        }
    }
    if references.is_empty() {
        return refuse("BOM contains no populated components");
    }
    Ok(references)
}

/// `PurePath` ordering: component by component (case-folded on Windows).
fn path_key(relative: &str) -> Vec<String> {
    relative.split('/').map(|part| if cfg!(windows) { part.to_lowercase() } else { part.to_owned() }).collect()
}

fn relative_posix(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    paths::as_posix(relative)
}

/// `_zip_files`: the files under `root`, sorted, deflated and dated 1980.
fn zip_files(output: &Path, files: &[PathBuf], root: &Path) -> Result<()> {
    let mut names: Vec<(Vec<String>, String, &PathBuf)> = files
        .iter()
        .map(|path| {
            let name = relative_posix(path, root);
            (path_key(&name), name, path)
        })
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entries = Vec::new();
    for (_, name, path) in names {
        entries.push((name, read(path, &paths::display(path))?));
    }
    std::fs::write(output, zip::archive(&entries))
        .map_err(|failure| Error::Os(open_error(&failure, &paths::display(output))))
}

/// The completed `subprocess.run(..., text=True, capture_output=True)`.
struct Completed {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(command: &[String], cwd: &Path) -> Result<Completed> {
    let output = Command::new(&command[0]).args(&command[1..]).current_dir(cwd).output().map_err(|failure| {
        // POSIX names the executable; Windows' CreateProcess error does not.
        let name = (!cfg!(windows)).then_some(command[0].as_str());
        Error::Os(copper_json::oserror::python_os_error(&failure, name, copper_json::oserror::Call::Os))
    })?;
    let text = |bytes: &[u8]| files::universal_newlines(&String::from_utf8_lossy(bytes));
    // Python reports a signal as its negative number.
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        output.status.code().unwrap_or_else(|| -output.status.signal().unwrap_or(1))
    };
    #[cfg(not(unix))]
    let code = output.status.code().unwrap_or(1);
    Ok(Completed { code, stdout: text(&output.stdout), stderr: text(&output.stderr) })
}

fn strip(text: &str) -> &str {
    text.trim_matches(copper_json::python_isspace)
}

/// `_run`: a step that must succeed.
fn require(command: &[String], cwd: &Path, action: &str) -> Result<()> {
    let result = run(command, cwd)?;
    if result.code != 0 {
        let detail = [strip(&result.stderr), strip(&result.stdout)]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or("no diagnostic output");
        return Err(Error::Runtime(format!("failed to {action}: {detail}")));
    }
    Ok(())
}

fn tag(node: &[Node]) -> Option<&str> {
    match node.first() {
        Some(Node::Atom(text)) => Some(text),
        _ => None,
    }
}

/// `tempfile.mkdtemp(prefix=prefix, dir=parent)`: a new directory with a
/// random eight-character suffix.
fn make_temporary(parent: &Path, prefix: &str) -> Result<PathBuf> {
    const CHARACTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let seed = format!("{:?}{}{}", std::time::SystemTime::now(), std::process::id(), paths::display(parent));
    for attempt in 0u32..100 {
        let digest = <sha2::Sha256 as sha2::Digest>::digest(format!("{seed}#{attempt}").as_bytes());
        let suffix: String =
            digest.iter().take(8).map(|byte| CHARACTERS[*byte as usize % CHARACTERS.len()] as char).collect();
        let path = parent.join(format!("{prefix}{suffix}"));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(failure) => return Err(Error::Os(os_error(&failure, &paths::display(&path)))),
        }
    }
    Err(Error::Os(format!("[Errno 17] No usable temporary directory name found: {}", paths::display(parent))))
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to).map(|_| ()).map_err(|failure| Error::Os(open_error(&failure, &paths::display(from))))
}

/// `shutil.copytree(source, destination)`.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination).map_err(|failure| Error::Os(os_error(&failure, &paths::display(destination))))?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(source)
        .map_err(|failure| Error::Os(os_error(&failure, &paths::display(source))))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("directory entry"));
        if entry.is_dir() {
            copy_tree(&entry, &target)?;
        } else {
            copy(&entry, &target)?;
        }
    }
    Ok(())
}

/// Every file under `root`, recursively (`rglob("*")` filtered by `is_file`).
fn walk(root: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(root).map_err(|failure| Error::Os(os_error(&failure, &paths::display(root))))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        if path.is_dir() && !is_link {
            walk(&path, found)?;
        } else if path.is_file() {
            found.push(path);
        }
    }
    Ok(())
}

/// Files directly in `directory` whose name ends with `suffix`.
fn glob(directory: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(suffix)))
        .collect()
}

/// The inputs of one export.
pub struct Export<'a> {
    pub pcb: &'a Path,
    pub output: &'a Path,
    /// `str(Path(--kicad-cli))`.
    pub kicad_cli: &'a str,
    /// The reviewed BOM and its `str()`.
    pub bom: Option<(&'a Path, &'a str)>,
    pub replace_existing: bool,
}

/// `export_manufacturing_files`: stage, native-refill and DRC, export,
/// check the inventory and publish atomically. Returns the published
/// directory.
pub fn export_manufacturing_files(request: &Export) -> Result<PathBuf> {
    let pcb = paths::resolve(request.pcb);
    if request.output.symlink_metadata().is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return refuse("output directory must not be a symlink");
    }
    let output = paths::resolve(request.output);
    if output.exists() {
        if !request.replace_existing {
            return Err(Error::Os(format!("manufacturing directory already exists: {}", paths::display(&output))));
        }
        let manifest = output.join("manifest.json");
        let replaceable = manifest.is_file() && {
            let text = read_text(&manifest, &paths::display(&manifest), false)?;
            let document = copper_json::parse(&text).map_err(|failure| Error::Files(failure.python_message(&text)))?;
            document.get("schema").and_then(Value::as_str) == Some("copperscript-manufacturing-files/v0.1")
        };
        if !replaceable {
            return refuse("replacement requires a CopperScript manufacturing-files directory");
        }
    }
    let bom_inside = request.bom.is_some_and(|(bom, _)| paths::is_relative_to(&paths::resolve(bom), &output));
    if paths::is_relative_to(&pcb, &output) || bom_inside {
        return refuse("input PCB/BOM must be outside the output directory");
    }
    let shown_pcb = paths::display(&pcb);
    let text = files::universal_newlines(&read_text(&pcb, &shown_pcb, false)?);
    let root = kicad_mod::parse_sexpr(&text, &shown_pcb).map_err(|failure| Error::Files(failure.to_string()))?;
    if tag(&root) != Some("kicad_pcb") {
        return refuse("expected a native .kicad_pcb board");
    }
    let copper: Vec<String> = kicad_mod::first(&root, "layers")
        .map(|layers| {
            layers[1..]
                .iter()
                .filter_map(|item| match item {
                    Node::List(item) => match item.get(1) {
                        Some(Node::Atom(name)) if name.ends_with(".Cu") => Some(name.clone()),
                        _ => None,
                    },
                    Node::Atom(_) => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let outlined = ["gr_circle", "gr_line", "gr_arc", "gr_poly", "gr_rect"].iter().any(|name| {
        kicad_mod::children(&root, name).iter().any(|item| {
            kicad_mod::first(item, "layer").and_then(|layer| kicad_mod::atom(layer, 1)) == Some("Edge.Cuts")
        })
    });
    if copper.is_empty() || !outlined {
        return refuse("board needs copper layers and an outline");
    }
    let mut layers = copper.clone();
    layers.extend(
        ["F.Mask", "B.Mask", "F.Silkscreen", "B.Silkscreen", "F.Paste", "B.Paste", "Edge.Cuts"].map(str::to_owned),
    );
    let references = match request.bom {
        Some((bom, shown)) => Some(bom_references(bom, shown)?),
        None => None,
    };
    let parent = files::parent(&output);
    files::mkdir_parents(&parent, true, true).map_err(Error::Os)?;
    let name = output.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let stage = make_temporary(&parent, &format!(".{name}-"))?;
    let result = stage_and_publish(request, &pcb, &output, &stage, &copper, &layers, references.as_deref());
    if stage.exists() {
        let _ = std::fs::remove_dir_all(&stage); // Only our exact newly-created staging directory.
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn stage_and_publish(
    request: &Export,
    pcb: &Path,
    output: &Path,
    stage: &Path,
    copper: &[String],
    layers: &[String],
    references: Option<&[String]>,
) -> Result<PathBuf> {
    let native = stage.join(pcb.file_name().expect("a board file name"));
    copy(pcb, &native)?;
    let project = with_suffix(pcb, ".kicad_pro");
    if !project.is_file() {
        return refuse("matching .kicad_pro required to retain design rules");
    }
    copy(&project, &with_suffix(&native, ".kicad_pro"))?;
    // Exported component-scoped custom rules belong to the same project.
    let rules = with_suffix(pcb, ".kicad_dru");
    if rules.is_file() {
        copy(&rules, &with_suffix(&native, ".kicad_dru"))?;
    }
    let directory = files::parent(pcb);
    for name in ["fp-lib-table", "CopperScript.pretty"] {
        let path = directory.join(name);
        if path.is_dir() {
            copy_tree(&path, &stage.join(name))?;
        } else if path.is_file() {
            copy(&path, &stage.join(name))?;
        }
    }
    let cli = request.kicad_cli.to_owned();
    let command = |args: &[&str]| -> Vec<String> {
        std::iter::once(cli.clone()).chain(args.iter().map(|arg| (*arg).to_owned())).collect()
    };
    let shown = |path: &Path| paths::display(path);
    let version = run(&command(&["version"]), stage)?;
    if version.code != 0 || strip(&version.stdout).is_empty() {
        return refuse("cannot query KiCad version");
    }
    let version = strip(&version.stdout).to_owned();
    let (native_text, drc) = (shown(&native), stage.join("drc.json"));
    require(
        &command(&[
            "pcb",
            "drc",
            "--refill-zones",
            "--save-board",
            "--format",
            "json",
            "--severity-all",
            "--exit-code-violations",
            "-o",
            &shown(&drc),
            &native_text,
        ]),
        stage,
        "run native DRC",
    )?;
    let report_text = read_text(&drc, &shown(&drc), false)?;
    let report =
        copper_json::parse(&report_text).map_err(|failure| Error::Files(failure.python_message(&report_text)))?;
    let clean = |key: &str| match &report {
        Value::Object(report) => matches!(report.get(key), Some(Value::Array(items)) if items.is_empty()),
        _ => false,
    };
    if !clean("violations") || !clean("unconnected_items") {
        return refuse("native DRC report must contain zero violations and opens");
    }
    let (gerbers, drills) = (stage.join("gerbers"), stage.join("drill"));
    for directory in [&gerbers, &drills] {
        std::fs::create_dir(directory).map_err(|failure| Error::Os(os_error(&failure, &shown(directory))))?;
    }
    let layer_list = layers.join(",");
    require(
        &command(&[
            "pcb",
            "export",
            "gerbers",
            "--layers",
            &layer_list,
            "--no-protel-ext",
            "--subtract-soldermask",
            "-o",
            &shown(&gerbers),
            &native_text,
        ]),
        stage,
        "export Gerbers",
    )?;
    require(
        &command(&[
            "pcb",
            "export",
            "drill",
            "--format",
            "excellon",
            "--excellon-units",
            "mm",
            "--drill-origin",
            "absolute",
            "--excellon-separate-th",
            "--generate-report",
            "-o",
            &shown(&drills),
            &native_text,
        ]),
        stage,
        "export drills",
    )?;
    let netlist = stage.join("board.d356");
    require(
        &command(&["pcb", "export", "ipcd356", "-o", &shown(&netlist), &native_text]),
        stage,
        "export manufacturing netlist",
    )?;
    let artwork = glob(&gerbers, ".gbr");
    let drill_files = glob(&drills, ".drl");
    if artwork.len() != layers.len() || drill_files.is_empty() || !netlist.is_file() {
        return refuse("missing manufacturing outputs");
    }
    let mut inventory: Vec<PathBuf> = artwork.iter().chain(&drill_files).cloned().collect();
    inventory.push(netlist.clone());
    for file in &inventory {
        let size = std::fs::metadata(file).map_err(|failure| Error::Os(os_error(&failure, &shown(file))))?.len();
        if size == 0 {
            return refuse("empty manufacturing output");
        }
    }
    let native_positions = stage.join("positions-kicad.csv");
    require(
        &command(&[
            "pcb",
            "export",
            "pos",
            "--format",
            "csv",
            "--units",
            "mm",
            "--side",
            "both",
            "--exclude-dnp",
            "-o",
            &shown(&native_positions),
            &native_text,
        ]),
        stage,
        "export positions",
    )?;
    if let (Some(references), Some((bom, _))) = (references, request.bom) {
        copy(bom, &stage.join("bom.csv"))?;
        write_jlcpcb_cpl(&native_positions, &shown(&native_positions), &stage.join("cpl.csv"), references)?;
    }
    let archived: Vec<PathBuf> = artwork.iter().chain(&drill_files).cloned().collect();
    zip_files(&stage.join("gerbers-drill.zip"), &archived, stage)?;
    let mut document = Object::new();
    let input_digest = sha256_hex(&read(pcb, &shown(pcb))?);
    let exported_digest = sha256_hex(&read(&native, &native_text)?);
    for (key, value) in [
        ("schema", Value::from("copperscript-manufacturing-files/v0.1")),
        ("kicad_version", Value::from(version.as_str())),
        ("input_pcb_sha256", Value::from(input_digest.as_str())),
        ("exported_pcb_sha256", Value::from(exported_digest.as_str())),
        ("native_drc", Value::from("passed")),
        ("independent_cam", Value::from("skipped_by_request")),
        ("qualified_release", Value::Bool(false)),
        ("supplier_availability", Value::from("not_checked")),
        ("assembly_bom_included", Value::Bool(request.bom.is_some())),
        ("copper_layers", Value::Array(copper.iter().map(|layer| Value::from(layer.as_str())).collect())),
        (
            "placement_convention",
            Value::from("KiCad mm, common origin, Y up, no bottom X mirror, native CCW rotations"),
        ),
        ("assembly_preview_review", Value::from("required; verify centroid, pin 1, polarity and supplier rotation")),
    ] {
        document.insert(key.to_owned(), value);
    }
    let manifest = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n";
    let manifest_path = stage.join("manifest.json");
    std::fs::write(&manifest_path, manifest.as_bytes())
        .map_err(|failure| Error::Os(open_error(&failure, &shown(&manifest_path))))?;
    let mut found = Vec::new();
    walk(stage, &mut found)?;
    let mut listed: Vec<(Vec<String>, String, PathBuf)> = found
        .into_iter()
        .map(|path| {
            let name = relative_posix(&path, stage);
            (path_key(&name), name, path)
        })
        .collect();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut sums = String::new();
    for (_, name, path) in &listed {
        sums.push_str(&format!("{}  {name}\n", sha256_hex(&read(path, &shown(path))?)));
    }
    let sums_path = stage.join("SHA256SUMS");
    std::fs::write(&sums_path, sums.as_bytes())
        .map_err(|failure| Error::Os(open_error(&failure, &shown(&sums_path))))?;
    let mut packaged: Vec<PathBuf> = listed.into_iter().map(|(_, _, path)| path).collect();
    packaged.push(sums_path);
    zip_files(&stage.join("manufacturing-package.zip"), &packaged, stage)?;
    let mut backup = None;
    if output.exists() {
        let parent = files::parent(output);
        let name = output.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let previous = make_temporary(&parent, &format!(".{name}-previous-"))?;
        // Our new empty directory; previous outputs are never deleted.
        std::fs::remove_dir(&previous).map_err(|failure| Error::Os(os_error(&failure, &shown(&previous))))?;
        std::fs::rename(output, &previous).map_err(|failure| Error::Os(rename_error(&failure, output, &previous)))?;
        backup = Some(previous);
    }
    if let Err(failure) = std::fs::rename(stage, output) {
        if let Some(backup) = &backup {
            let _ = std::fs::rename(backup, output);
        }
        return Err(Error::Os(rename_error(&failure, stage, output)));
    }
    Ok(output.to_path_buf())
}

/// `str(OSError)` of `Path.rename`, which names both paths.
fn rename_error(error: &std::io::Error, from: &Path, to: &Path) -> String {
    format!("{} -> {}", os_error(error, &paths::display(from)), copper_json::python_str_repr(&paths::display(to)))
}

//! `copper models {list,fetch} BOARD [--locked] [--offline]`: the 3D models a
//! board's parts name by `https://` URL. `list` shows each one's licence note
//! and whether it is cached; `fetch` downloads the missing ones (with curl)
//! into the project's `.copper-cache/models/`, refusing any file whose SHA-256
//! differs from the part's `model_sha256`. Builds never download; they read
//! only that cache.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::text;
use copper_lang::compiler::load_design;
use copper_lang::elaborate::elaborate;
use copper_lang::paths;
use copper_lang::remote_models::{RemoteModel, cache_directory, library_models};

use crate::option_parser::{self, flag};

const USAGE: &str = "usage: copper models [-h] [--locked] [--offline] {list,fetch} board";

const ABOUT: &str = "the 3D models a board's parts name by https:// URL (model, model_sha256, model_license,
model_redistributable). list: each model, its licence note and whether it is cached. fetch: download the
missing ones with curl into the project's .copper-cache/models/, checked against model_sha256. Builds read
only that cache, never the network. Models not marked model_redistributable = true are exported to
CopperScript-local.3dshapes/ and left out of manufacturing packages: fetching one is your own download
under its vendor's terms; do not commit or share it.";

fn describe(model: &RemoteModel) -> String {
    let licence = model.license.as_deref().unwrap_or("no licence note");
    let sharing = if model.redistributable { "redistributable" } else { "local only" };
    format!("{} [{}; {sharing}] for {}", model.url, licence, model.parts.join(", "))
}

/// Download `url` to a temporary file in `cache` with curl; the bytes.
fn download(url: &str, cache: &Path) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(cache).map_err(|failure| format!("cannot create {}: {failure}", paths::display(cache)))?;
    let temporary: PathBuf = cache.join(format!(".download-{}", std::process::id()));
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "300", "--output"])
        .arg(&temporary)
        .arg(url)
        .output()
        .map_err(|failure| format!("cannot run curl: {failure}"))?;
    let result = if output.status.success() {
        std::fs::read(&temporary).map_err(|failure| format!("cannot read the download: {failure}"))
    } else {
        Err(format!("download failed: {}", String::from_utf8_lossy(&output.stderr).trim()))
    };
    let _ = std::fs::remove_file(&temporary); // Only our own temporary file.
    result
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper models: error: {message}");
        2
    };
    let parsed = match option_parser::parse(arguments, &[flag("--locked"), flag("--offline")]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\n{ABOUT}");
        return 0;
    }
    let (action, board) = match parsed.positionals.as_slice() {
        [action, board] if action == "list" || action == "fetch" => (action.as_str(), board.clone()),
        [action, _] => return usage_error(err, &format!("invalid choice: '{action}' (choose from list, fetch)")),
        [_, _, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
        _ => return usage_error(err, "the following arguments are required: {list,fetch}, board"),
    };
    let fail = |out: &mut dyn Write, message: String| {
        let _ = text::print(out, &format!("MODELS ERROR: {message}"));
        2
    };
    let path = Path::new(&board);
    let design = match load_design(path, parsed.flag("--locked"), parsed.flag("--offline")) {
        Ok(design) => design,
        Err(failure) => return fail(out, format!("cannot compile {board}: {failure}")),
    };
    let flat = match elaborate(&design.electrical) {
        Ok(flat) => flat,
        Err(message) => return fail(out, message),
    };
    let models = library_models(&flat.library);
    let base = paths::resolve(path).parent().map(Path::to_path_buf).unwrap_or_default();
    let cache = cache_directory(&base);
    if models.is_empty() {
        let _ = text::print(out, "No part names a 3D model by URL.");
        return 0;
    }
    let mut failed = 0;
    for model in &models {
        let line = match model.cached(&cache) {
            Err(message) => {
                failed += 1;
                format!("MISMATCH {}: {message}", describe(model))
            }
            Ok(Some(file)) => format!("cached   {} -> {}", describe(model), paths::display(&file)),
            Ok(None) if action == "list" => format!("missing  {}", describe(model)),
            Ok(None) if parsed.flag("--offline") => {
                failed += 1;
                format!("missing  {} (offline: not fetched)", describe(model))
            }
            Ok(None) => match download(&model.url, &cache).and_then(|bytes| model.store(&cache, &bytes)) {
                Ok(file) => format!("fetched  {} -> {}", describe(model), paths::display(&file)),
                Err(message) => {
                    failed += 1;
                    format!("FAILED   {}: {message}", describe(model))
                }
            },
        };
        let _ = text::print(out, &line);
    }
    let local = models.iter().filter(|model| !model.redistributable).count();
    if local > 0 {
        let _ = text::print(
            out,
            &format!(
                "{local} model(s) are local only: exports keep them in CopperScript-local.3dshapes/, out of manufacturing packages. Do not commit or share them."
            ),
        );
    }
    if failed > 0 { 1 } else { 0 }
}

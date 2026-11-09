//! `copper plot PCB -o OUTPUT`: native Gerber X2, Excellon, IPC-D-356 and
//! position files from a filled `.kicad_pcb`, without KiCad.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::paths;

use crate::option_parser::{self, short_value};
use crate::pypath;

const USAGE: &str = "usage: copper plot [-h] -o OUTPUT pcb";

fn stage_files(output: &Path, files: &[(String, String)]) -> Result<(), String> {
    let parent = output.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|failure| format!("cannot create {}: {failure}", parent.display()))?;
    let name = output.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let stage: PathBuf = parent.join(format!(".{name}-plot-{}", std::process::id()));
    std::fs::create_dir(&stage).map_err(|failure| format!("cannot create {}: {failure}", stage.display()))?;
    let written = files.iter().try_for_each(|(file, contents)| {
        std::fs::write(stage.join(file), contents).map_err(|failure| format!("cannot write {file}: {failure}"))
    });
    let published = written.and_then(|()| {
        std::fs::rename(&stage, output).map_err(|failure| format!("cannot publish {}: {failure}", output.display()))
    });
    if published.is_err() {
        let _ = std::fs::remove_dir_all(&stage); // Only our own new staging directory.
    }
    published
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper plot: error: {message}");
        2
    };
    let parsed = match option_parser::parse(arguments, &[short_value("--output", "-o")]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nplot native Gerber X2, Excellon, IPC-D-356 and position files from a filled board, without KiCad;\nzone fills are taken from the board as saved, not refilled, and no DRC is run"
        );
        crate::help::options(out, "plot");
        return 0;
    }
    let (pcb, output) = match (parsed.positionals.as_slice(), parsed.value("--output")) {
        ([pcb], Some(output)) => (PathBuf::from(pypath::display(pcb)), PathBuf::from(pypath::display(output))),
        ([_, extra @ ..], _) if !extra.is_empty() => {
            return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" ")));
        }
        _ => return usage_error(err, "the following arguments are required: pcb, -o/--output"),
    };
    let fail = |out: &mut dyn Write, message: String| {
        let _ = text::print(out, &format!("PLOT ERROR: {message}"));
        2
    };
    if output.exists() {
        return fail(out, format!("output directory already exists: {}", paths::display(&output)));
    }
    let shown = paths::display(&pcb);
    let source = match std::fs::read(&pcb) {
        Ok(bytes) => bytes,
        Err(failure) => return fail(out, format!("cannot read {shown}: {failure}")),
    };
    let body = source.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&source);
    let Ok(source) = std::str::from_utf8(body) else {
        return fail(out, format!("{shown} is not UTF-8"));
    };
    let board = match copper_cam::board::parse_board(&source.replace("\r\n", "\n"), &shown) {
        Ok(board) => board,
        Err(failure) => return fail(out, failure),
    };
    let stem = pcb.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| "board".to_owned());
    let files = match copper_cam::plot::plot(&board, &stem, env!("CARGO_PKG_VERSION")) {
        Ok(files) => files,
        Err(failure) => return fail(out, failure),
    };
    if let Err(failure) = stage_files(&output, &files) {
        return fail(out, failure);
    }
    let _ = text::print(
        out,
        &format!(
            "Plotted {} files -> {} ({} copper layers, {} zone fill areas read from the board; no refill or DRC)",
            files.len(),
            paths::display(&output),
            board.copper_count(),
            board.fills.len()
        ),
    );
    0
}

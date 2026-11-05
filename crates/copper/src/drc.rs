//! `copper drc PCB`: check a filled board natively against its design
//! rules, without KiCad.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::{Dumps, text};
use copper_lang::paths;

use crate::option_parser::{self, value};
use crate::pypath;

const USAGE: &str = "usage: copper drc [-h] [--project PROJECT] [--rules RULES] [--report REPORT] [--cam CAM_DIR] pcb";

fn read(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|failure| format!("cannot read {}: {failure}", paths::display(path)))?;
    let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let text = String::from_utf8(body.to_vec()).map_err(|_| format!("{} is not UTF-8", paths::display(path)))?;
    Ok(text.replace("\r\n", "\n"))
}

fn walk(root: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(root).map_err(|failure| format!("cannot list {}: {failure}", paths::display(root)))?;
    let mut entries: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper drc: error: {message}");
        2
    };
    let specs = [value("--project"), value("--rules"), value("--report"), value("--cam")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\ncheck a filled .kicad_pcb natively against the design rules of its .kicad_pro and .kicad_dru\n(same stem by default): clearances, holes, board edge, sizes, keepouts, opens and dangling\ncopper; with --cam, also read back the Gerbers and drills under CAM_DIR for shorts and opens\nagainst the board netlist; exit 0 when clean, 1 with findings, 2 on error. No KiCad is run."
        );
        return 0;
    }
    let pcb = match parsed.positionals.as_slice() {
        [pcb] => PathBuf::from(pypath::display(pcb)),
        [] => return usage_error(err, "the following arguments are required: pcb"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let project = parsed
        .value("--project")
        .map_or_else(|| pcb.with_extension("kicad_pro"), |p| PathBuf::from(pypath::display(p)));
    let custom = match parsed.value("--rules") {
        Some(path) => Some(PathBuf::from(pypath::display(path))),
        None => Some(pcb.with_extension("kicad_dru")).filter(|path| path.is_file()),
    };
    let report_path = parsed.value("--report").map(|path| PathBuf::from(pypath::display(path)));
    let cam = parsed.value("--cam").map(|path| PathBuf::from(pypath::display(path)));
    let mut readback = None;
    let result = (|| -> Result<copper_cam::drc::Report, String> {
        let source = read(&pcb)?;
        let board = copper_cam::board::parse_board(&source, &paths::display(&pcb))?;
        let custom_text = custom.as_deref().map(read).transpose()?;
        let rules = copper_cam::rules::Rules::load(&read(&project)?, custom_text.as_deref())
            .map_err(|failure| format!("{}: {failure}", paths::display(&project)))?;
        let report = copper_cam::drc::check(&board, &rules)?;
        if let Some(directory) = &cam {
            let (mut gerbers, mut drills) = (Vec::new(), Vec::new());
            let mut found = Vec::new();
            walk(directory, &mut found)?;
            for path in found {
                let name = paths::display(&path);
                match path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).as_deref() {
                    Some("gbr") => gerbers.push((name, read(&path)?)),
                    Some("drl") => drills.push((name, read(&path)?)),
                    _ => {}
                }
            }
            let points = copper_cam::readback::test_points(&board);
            readback = Some(copper_cam::readback::check(&gerbers, &drills, &points, board.copper_count(), 10_000.0)?);
        }
        if let Some(path) = &report_path {
            let mut files = vec![paths::display(&project)];
            files.extend(custom.as_deref().map(paths::display));
            let json = report.to_json(&paths::display(&pcb), &files);
            let text = Dumps::new().indent(2).dumps(&json) + "\n";
            std::fs::write(path, text)
                .map_err(|failure| format!("cannot write {}: {failure}", paths::display(path)))?;
        }
        Ok(report)
    })();
    match result {
        Ok(report) => {
            let _ = text::print(out, &report.summary());
            if let Some(readback) = &readback {
                let _ = text::print(out, &readback.summary());
            }
            if report.passed() && readback.as_ref().is_none_or(|readback| readback.passed()) { 0 } else { 1 }
        }
        Err(failure) => {
            let _ = text::print(out, &format!("DRC ERROR: {failure}"));
            2
        }
    }
}

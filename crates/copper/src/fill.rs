//! `copper fill PCB -o OUTPUT`: fill the board's copper zones natively and
//! write the board with the new fills, without KiCad.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::paths;

use crate::option_parser::{self, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper fill [-h] [--project PROJECT] [--rules RULES] -o OUTPUT pcb";

fn read(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|failure| format!("cannot read {}: {failure}", paths::display(path)))?;
    let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let text = String::from_utf8(body.to_vec()).map_err(|_| format!("{} is not UTF-8", paths::display(path)))?;
    Ok(text.replace("\r\n", "\n"))
}

fn sibling(pcb: &Path, extension: &str) -> PathBuf {
    pcb.with_extension(extension)
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper fill: error: {message}");
        2
    };
    let specs = [value("--project"), value("--rules"), short_value("--output", "-o")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nfill the copper zones of a .kicad_pcb natively, with the design rules of its .kicad_pro and\n.kicad_dru (same stem by default), and write the filled board; no KiCad and no DRC"
        );
        crate::help::options(out, "fill");
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
        let _ = text::print(out, &format!("FILL ERROR: {message}"));
        2
    };
    if output.exists() {
        return fail(out, format!("output already exists: {}", paths::display(&output)));
    }
    let project =
        parsed.value("--project").map_or_else(|| sibling(&pcb, "kicad_pro"), |p| PathBuf::from(pypath::display(p)));
    let custom = match parsed.value("--rules") {
        Some(path) => Some(PathBuf::from(pypath::display(path))),
        None => Some(sibling(&pcb, "kicad_dru")).filter(|path| path.is_file()),
    };
    let result = (|| -> Result<String, String> {
        let source = read(&pcb)?;
        let board = copper_cam::board::parse_board(&source, &paths::display(&pcb))?;
        let custom_text = custom.as_deref().map(read).transpose()?;
        let rules = copper_cam::rules::Rules::load(&read(&project)?, custom_text.as_deref())
            .map_err(|failure| format!("{}: {failure}", paths::display(&project)))?;
        let fills = copper_cam::fill::fill_zones(&board, &rules)?;
        let written = copper_cam::pcb_write::write_fills(&source, &board, &fills)?;
        std::fs::write(&output, written)
            .map_err(|failure| format!("cannot write {}: {failure}", paths::display(&output)))?;
        let mut lines = Vec::new();
        for fill in &fills {
            let zone = &board.zones[fill.zone];
            let area: f64 = fill.shapes.iter().map(|shape| copper_cam::poly::shape_area(shape)).sum::<f64>() / 1e12;
            lines.push(format!(
                "  {} on {}: {} area(s), {:.2} mm2{}",
                zone.name,
                board.copper_layers[fill.layer],
                fill.shapes.len(),
                area,
                if fill.islands_removed > 0 {
                    format!(", {} island(s) removed", fill.islands_removed)
                } else {
                    String::new()
                }
            ));
        }
        Ok(format!(
            "Filled {} zone layer(s) -> {} (rules: {}{}; no DRC run)\n{}",
            fills.len(),
            paths::display(&output),
            paths::display(&project),
            custom.as_deref().map_or(String::new(), |path| format!(", {}", paths::display(path))),
            lines.join("\n")
        ))
    })();
    match result {
        Ok(summary) => {
            let _ = text::print(out, &summary);
            0
        }
        Err(failure) => fail(out, failure),
    }
}

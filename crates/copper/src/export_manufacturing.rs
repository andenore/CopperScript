//! `copper export-manufacturing PCB --skip-independent-cam -o OUTPUT
//! [--kicad-cli PATH] [--bom BOM] [--replace]`: native-DRC-clean KiCad
//! manufacturing files without independent CAM qualification.

use std::io::Write;
use std::path::PathBuf;

use copper_assembly::manufacturing::{Export, export_manufacturing_files};
use copper_json::text;
use copper_lang::paths;

use crate::option_parser::{self, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper export-manufacturing [-h] [--kicad-cli KICAD_CLI] --skip-independent-cam \
[--bom BOM] [--replace] -o OUTPUT pcb";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper export-manufacturing: error: {message}");
        2
    };
    let specs = [
        value("--kicad-cli"),
        flag("--skip-independent-cam"),
        value("--bom"),
        flag("--replace"),
        short_value("--output", "-o"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nexport native-DRC-clean KiCad manufacturing files without independent CAM qualification"
        );
        return 0;
    }
    let (pcb, output) = match (parsed.positionals.as_slice(), parsed.value("--output")) {
        ([pcb], Some(output)) if parsed.flag("--skip-independent-cam") => (pcb.clone(), output.to_owned()),
        ([_, extra @ ..], _) if !extra.is_empty() => {
            return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" ")));
        }
        _ => {
            return usage_error(err, "the following arguments are required: pcb, --skip-independent-cam, -o/--output");
        }
    };
    let kicad_cli = pypath::display(parsed.value("--kicad-cli").unwrap_or("kicad-cli"));
    let bom = parsed.value("--bom").map(|bom| {
        let shown = pypath::display(bom);
        (PathBuf::from(&shown), shown)
    });
    let request = Export {
        pcb: &PathBuf::from(pypath::display(&pcb)),
        output: &PathBuf::from(pypath::display(&output)),
        kicad_cli: &kicad_cli,
        bom: bom.as_ref().map(|(path, shown)| (path.as_path(), shown.as_str())),
        replace_existing: parsed.flag("--replace"),
    };
    match export_manufacturing_files(&request) {
        Ok(published) => {
            let _ = text::print(
                out,
                &format!(
                    "Manufacturing files -> {}; native DRC passed; independent CAM skipped; supplier preview review required",
                    paths::display(&published)
                ),
            );
            0
        }
        Err(failure) => {
            let _ = text::print(out, &format!("MANUFACTURING EXPORT ERROR: {failure}"));
            2
        }
    }
}

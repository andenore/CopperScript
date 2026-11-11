//! `copper export-manufacturing PCB --skip-independent-cam -o OUTPUT
//! [--kicad-cli PATH] [--drc native|kicad] [--plotter native|kicad]
//! [--expected-open-ends REPORT] [--bom BOM] [--replace]`: native-DRC-clean manufacturing files without
//! independent CAM qualification, refilled, checked and plotted by
//! CopperScript itself (the default) or by KiCad on request.

use std::io::Write;
use std::path::PathBuf;

use copper_assembly::manufacturing::{Checker, Export, Plotter, export_manufacturing_files};
use copper_json::text;
use copper_lang::paths;

use crate::option_parser::{self, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper export-manufacturing [-h] [--kicad-cli KICAD_CLI] --skip-independent-cam \
[--drc {native,kicad}] [--plotter {native,kicad}] [--expected-open-ends REPORT] [--bom BOM] [--replace] -o OUTPUT pcb";

/// The plotter and the checker: CopperScript's own unless `kicad` is named.
fn choices(plotter: Option<&str>, checker: Option<&str>) -> Result<(Plotter, Checker), String> {
    let invalid = |option: &str, other: &str| {
        format!("argument {option}: invalid choice: '{other}' (choose from 'native', 'kicad')")
    };
    let plotter = match plotter {
        None | Some("native") => Plotter::Native,
        Some("kicad") => Plotter::Kicad,
        Some(other) => return Err(invalid("--plotter", other)),
    };
    let checker = match checker {
        None | Some("native") => Checker::Native,
        Some("kicad") => Checker::Kicad,
        Some(other) => return Err(invalid("--drc", other)),
    };
    Ok((plotter, checker))
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper export-manufacturing: error: {message}");
        2
    };
    let specs = [
        value("--kicad-cli"),
        flag("--skip-independent-cam"),
        value("--plotter"),
        value("--drc"),
        value("--expected-open-ends"),
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
            "{USAGE}\n\nexport native-DRC-clean KiCad manufacturing files without independent CAM qualification;\nby default CopperScript refills and checks the board (and reads the plotted files back) and\nwrites the files with its own plotter, so KiCad is not needed; --drc kicad and --plotter kicad\nselect kicad-cli instead, and --kicad-cli names that program only when one of them is used"
        );
        crate::help::options(out, "export-manufacturing");
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
    let (plotter, checker) = match choices(parsed.value("--plotter"), parsed.value("--drc")) {
        Ok(choices) => choices,
        Err(message) => return usage_error(err, &message),
    };
    // KiCad's DRC has no such excuse, so the option is refused rather than ignored.
    let open_ends = match parsed.value("--expected-open-ends") {
        Some(_) if checker == Checker::Kicad => {
            return usage_error(err, "argument --expected-open-ends: not supported with --drc kicad");
        }
        Some(report) => match crate::drc::expected_open_ends(&PathBuf::from(pypath::display(report))) {
            Ok(ends) => ends,
            Err(message) => {
                let _ = text::print(out, &format!("MANUFACTURING EXPORT ERROR: {message}"));
                return 2;
            }
        },
        None => Vec::new(),
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
        plotter,
        checker,
        expected_open_ends: &open_ends,
    };
    match export_manufacturing_files(&request) {
        Ok(published) => {
            let _ = text::print(
                out,
                &format!(
                    "Manufacturing files -> {}; native DRC passed ({}); plotted by {}; independent CAM skipped; supplier preview review required",
                    paths::display(&published),
                    if checker == Checker::Native { "CopperScript, plotted files read back" } else { "KiCad" },
                    if plotter == Plotter::Native { "CopperScript" } else { "KiCad" }
                ),
            );
            // The full package also holds reports, the PCB and a nested zip; fab
            // upload parsers misread it (JLCPCB reported missing drill files).
            let mut upload = format!("Upload to the fab: {}", paths::display(&published.join("gerbers-drill.zip")));
            if request.bom.is_some() {
                upload.push_str(" (board); bom.csv and cpl.csv (assembly)");
            }
            let _ = text::print(out, &format!("{upload}. manufacturing-package.zip is the archive, not an upload."));
            0
        }
        Err(failure) => {
            let _ = text::print(out, &format!("MANUFACTURING EXPORT ERROR: {failure}"));
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copperscript_plots_and_checks_unless_kicad_is_named() {
        assert_eq!(choices(None, None), Ok((Plotter::Native, Checker::Native)));
        assert_eq!(choices(Some("kicad"), None), Ok((Plotter::Kicad, Checker::Native)));
        assert_eq!(choices(None, Some("kicad")), Ok((Plotter::Native, Checker::Kicad)));
        assert_eq!(choices(Some("kicad"), Some("kicad")), Ok((Plotter::Kicad, Checker::Kicad)));
        assert_eq!(choices(Some("native"), Some("native")), Ok((Plotter::Native, Checker::Native)));
        assert!(choices(Some("gerbv"), None).is_err() && choices(None, Some("x")).is_err());
    }

    #[test]
    fn expected_open_ends_are_refused_with_kicad_drc() {
        let args: Vec<String> =
            ["b.kicad_pcb", "--skip-independent-cam", "--drc", "kicad", "--expected-open-ends", "r.json", "-o", "out"]
                .iter()
                .map(|arg| arg.to_string())
                .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&args, &mut out, &mut err), 2);
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("argument --expected-open-ends: not supported with --drc kicad"), "{err}");
        assert!(out.is_empty());
    }

    #[test]
    fn plotter_choice_is_validated_before_any_export() {
        let args: Vec<String> = ["board.kicad_pcb", "--skip-independent-cam", "--plotter", "gerbv", "-o", "out"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&args, &mut out, &mut err), 2);
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("invalid choice: 'gerbv' (choose from 'native', 'kicad')"), "{err}");
        assert!(out.is_empty());
        let args: Vec<String> = ["board.kicad_pcb", "--skip-independent-cam", "--drc", "freerouting", "-o", "out"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&args, &mut out, &mut err), 2);
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("argument --drc: invalid choice: 'freerouting'"), "{err}");
    }
}

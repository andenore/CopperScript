//! `copper export-kicad-pcb BOARD [--locked] [--offline] [-o OUTPUT] [--no-check]
//! [--footprint-root DIR]... [--allow-proxy-footprints] [--layers {2,4,6}]
//! [--fab-profile PROFILE] [--width-mm MM] [--height-mm MM] [--hard-macro SCENE]...
//! [--placement-templates SCENE]`.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_pcb::{Error, hard_macros, kicad_pcb, project, templates};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::pypath;

const USAGE: &str = "usage: copper export-kicad-pcb [-h] [--locked] [--offline] [-o OUTPUT] [--no-check] \
[--footprint-root FOOTPRINT_ROOT] [--allow-proxy-footprints] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--width-mm WIDTH_MM] [--height-mm HEIGHT_MM] \
[--hard-macro HARD_MACRO] [--placement-templates PLACEMENT_TEMPLATES] board";

const FLAGS: [&str; 5] = ["--help", "--locked", "--offline", "--no-check", "--allow-proxy-footprints"];
const VALUED: [&str; 8] = [
    "--output",
    "--footprint-root",
    "--layers",
    "--fab-profile",
    "--width-mm",
    "--height-mm",
    "--hard-macro",
    "--placement-templates",
];

#[derive(Default)]
struct Arguments {
    flags: Vec<&'static str>,
    output: Option<String>,
    footprint_roots: Vec<String>,
    layers: Option<String>,
    fab_profile: Option<String>,
    width_mm: Option<String>,
    height_mm: Option<String>,
    hard_macros: Vec<String>,
    placement_templates: Option<String>,
    positionals: Vec<String>,
}

fn looks_negative(value: &str) -> bool {
    value.starts_with('-') && value[1..].parse::<f64>().is_ok()
}

/// argparse for this command: abbreviations, `--option=value` and `-oVALUE`.
fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut index = 0;
    let mut only_positionals = false;
    while index < arguments.len() {
        let arg = &arguments[index];
        index += 1;
        if only_positionals || !arg.starts_with('-') || arg == "-" || looks_negative(arg) {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (name, inline): (&str, Option<String>) = if arg == "-h" {
            ("--help", None)
        } else if let Some(value) = arg.strip_prefix("-o").filter(|_| !arg.starts_with("--")) {
            ("--output", (!value.is_empty()).then(|| value.to_owned()))
        } else {
            let (key, value) = match arg.split_once('=') {
                Some((key, value)) => (key, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            let options: Vec<&'static str> = FLAGS.iter().chain(VALUED.iter()).copied().collect();
            let exact = options.iter().copied().find(|o| *o == key);
            let matches: Vec<&'static str> = options.iter().copied().filter(|o| o.starts_with(key)).collect();
            let option = match (exact, matches.as_slice()) {
                (Some(option), _) => option,
                (None, [option]) => option,
                (None, []) => return Err(format!("unrecognized arguments: {arg}")),
                (None, _) => return Err(format!("ambiguous option: {key} could match {}", matches.join(", "))),
            };
            (option, value)
        };
        if let Some(flag) = FLAGS.iter().copied().find(|f| *f == name) {
            if inline.is_some() {
                return Err(format!("argument {flag}: ignored explicit argument"));
            }
            parsed.flags.push(flag);
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => match arguments.get(index) {
                Some(value) if !value.starts_with('-') || looks_negative(value) => {
                    index += 1;
                    value.clone()
                }
                _ => return Err(format!("argument {name}: expected one argument")),
            },
        };
        match name {
            "--output" => parsed.output = Some(value),
            "--footprint-root" => parsed.footprint_roots.push(value),
            "--layers" => parsed.layers = Some(value),
            "--fab-profile" => parsed.fab_profile = Some(value),
            "--width-mm" => parsed.width_mm = Some(value),
            "--height-mm" => parsed.height_mm = Some(value),
            "--hard-macro" => parsed.hard_macros.push(value),
            _ => parsed.placement_templates = Some(value),
        }
    }
    Ok(parsed)
}

fn float(value: Option<&String>, default: f64, name: &str) -> Result<f64, String> {
    let Some(text) = value else { return Ok(default) };
    text.trim().parse::<f64>().map_err(|_| format!("argument {name}: invalid float value: '{text}'"))
}

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: export-kicad-pcb ({what})");
    NOT_IMPLEMENTED_EXIT
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper export-kicad-pcb: error: {message}");
        2
    };
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\ngenerate a KiCad 8 PCB draft using resolved footprints");
        return 0;
    }
    let layers = match parsed.layers.as_deref().map(str::trim) {
        None => 2,
        Some(text) => match text.parse::<i64>() {
            Ok(value) if [2, 4, 6].contains(&value) => value,
            Ok(value) => {
                return usage_error(err, &format!("argument --layers: invalid choice: {value} (choose from 2, 4, 6)"));
            }
            Err(_) => return usage_error(err, &format!("argument --layers: invalid int value: '{text}'")),
        },
    };
    let profile = parsed.fab_profile.clone().unwrap_or_else(|| "generic".to_owned());
    if !["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"].contains(&profile.as_str()) {
        return usage_error(
            err,
            &format!(
                "argument --fab-profile: invalid choice: '{profile}' (choose from generic, jlcpcb-four-layer, jlcpcb-six-layer)"
            ),
        );
    }
    let (width, height) = match (
        float(parsed.width_mm.as_ref(), 100.0, "--width-mm"),
        float(parsed.height_mm.as_ref(), 80.0, "--height-mm"),
    ) {
        (Ok(width), Ok(height)) => (width, height),
        (Err(message), _) | (_, Err(message)) => return usage_error(err, &message),
    };
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let (locked, offline) = (parsed.flags.contains(&"--locked"), parsed.flags.contains(&"--offline"));
    let design = match load_design(Path::new(&board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            return not_implemented(err, &failure.message);
        }
        Err(failure) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let diagnostics = match copper_lang::erc::check(&design.electrical, false) {
        Ok(diagnostics) => diagnostics,
        Err(message) => {
            let _ = text::print(out, &format!("COMPILE ERROR: {message}"));
            return 2;
        }
    };
    if copper_lang::erc::has_errors(&diagnostics) && !parsed.flags.contains(&"--no-check") {
        for diagnostic in &diagnostics {
            let _ = text::print(out, &diagnostic.to_string());
        }
        let _ = text::print(out, "KiCad generation stopped because ERC reported errors.");
        return 1;
    }
    let generated = (|| -> Result<kicad_pcb::Manifest, Error> {
        let mechanical = design.mechanical.is_some();
        let options = PhysicalOptions::new(
            if mechanical { 100.0 } else { width },
            if mechanical { 80.0 } else { height },
            layers,
            &profile,
        )?;
        let mut board_physical = if parsed.flags.contains(&"--allow-proxy-footprints") {
            physicalize(&design, Footprints::Proxy, &options, locked, offline)?
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> = parsed.footprint_roots.iter().map(PathBuf::from).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(&design, Footprints::Resolved(&mut resolver), &options, locked, offline)?
        };
        if let Some(scene) = &parsed.placement_templates {
            board_physical = templates::apply_placement_templates(board_physical, Path::new(scene), locked, offline)?;
        }
        for scene in &parsed.hard_macros {
            board_physical = hard_macros::apply_hard_macro_scene(board_physical, Path::new(scene), locked, offline)?;
        }
        let board_physical = hard_macros::materialize_hard_macros(board_physical)?;
        kicad_pcb::generate(&board_physical)
    })();
    let manifest = match generated {
        Ok(manifest) => manifest,
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        Err(Error::Value(message)) => {
            let _ = text::print(out, &format!("BACKEND ERROR: {message}"));
            return 2;
        }
    };
    let output = parsed.output.clone().unwrap_or_else(|| manifest.artifacts[0].name.clone());
    if let Err(message) = project::write_kicad_project(&manifest, Path::new(&output)) {
        let _ = text::print(out, &format!("OUTPUT ERROR: {message}"));
        return 2;
    }
    for warning in &manifest.warnings {
        let _ = text::print(out, &format!("WARNING: {warning}"));
    }
    let _ = text::print(
        out,
        &format!(
            "Generated KiCad {} PCB {} -> {}",
            manifest.target_version,
            design.electrical.name,
            pypath::display(&output)
        ),
    );
    0
}

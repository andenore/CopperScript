//! `copper emi-check BOARD [--locked] [--offline] [--layers {2,4,6}]
//! [--fab-profile PROFILE] [--footprint-root DIR]... [--allow-proxy-footprints] [--json]`:
//! screen the placed switching cells for noise sources (hot loop, switch node,
//! proximity). Screening evidence only; findings never change the exit status.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_drc::emi::emi_check;
use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::paths;
use copper_pcb::Error;
use copper_pcb::physicalize::{Footprints, physicalize};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, value};
use crate::pypath;

const USAGE: &str = "usage: copper emi-check [-h] [--locked] [--offline] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--footprint-root FOOTPRINT_ROOT] \
[--allow-proxy-footprints] [--json] board";

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: emi-check ({what})");
    NOT_IMPLEMENTED_EXIT
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper emi-check: error: {message}");
        2
    };
    let specs = [
        flag("--locked"),
        flag("--offline"),
        value("--layers"),
        value("--fab-profile"),
        value("--footprint-root"),
        flag("--allow-proxy-footprints"),
        flag("--json"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\nscreen the placed switching cells for noise sources (hot-loop area and inductance, switch-node copper, \
             inductor and switch-node proximity); screening evidence, not EMC qualification"
        );
        crate::help::options(out, "emi-check");
        return 0;
    }
    let layers = match parsed.value("--layers").map(str::trim) {
        None => None,
        Some(text) => match text.parse::<i64>() {
            Ok(value) if [2, 4, 6].contains(&value) => Some(value),
            Ok(value) => {
                return usage_error(err, &format!("argument --layers: invalid choice: {value} (choose from 2, 4, 6)"));
            }
            Err(_) => return usage_error(err, &format!("argument --layers: invalid int value: '{text}'")),
        },
    };
    let profile = parsed.value("--fab-profile").unwrap_or("generic");
    if !["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"].contains(&profile) {
        return usage_error(err, &format!("argument --fab-profile: invalid choice: '{profile}'"));
    }
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let design = match load_design(Path::new(&board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            return not_implemented(err, &failure.message);
        }
        Err(failure) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return 2;
        }
    };
    let physical = (|| {
        let options =
            copper_pcb::configuration::physical_options(&design, (100.0, 80.0), layers, profile, locked, offline)?;
        if parsed.flag("--allow-proxy-footprints") {
            physicalize(&design, Footprints::Proxy, &options, locked, offline)
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> =
                parsed.values("--footprint-root").iter().map(|root| PathBuf::from(pypath::display(root))).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(&design, Footprints::Resolved(&mut resolver), &options, locked, offline)
        }
    })();
    let physical = match physical {
        Ok(physical) => physical,
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        Err(Error::Value(message)) => {
            let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
            return 2;
        }
    };
    let view = match copper_lang::elaborate::elaborate(&design.electrical) {
        Ok(view) => view,
        Err(message) => {
            let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
            return 2;
        }
    };
    let model = copper_lang::emi::analyze(&view);
    let report = emi_check(&physical, &view, &model);
    if parsed.flag("--json") {
        let _ = out.write_all(report.json().as_bytes());
    } else {
        for line in report.lines() {
            let _ = text::print(out, &line);
        }
    }
    // Screening findings never fail the command.
    0
}

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
[--allow-proxy-footprints] [--run RUN [--write]] [--json] board";

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
        value("--run"),
        flag("--write"),
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
    if parsed.flag("--write") && parsed.value("--run").is_none() {
        return usage_error(
            err,
            "argument --write: needs --run RUN (the run whose route-report.json receives the section)",
        );
    }
    let routed = match parsed.value("--run") {
        Some(run) => match routed_board(&pypath::display(run)) {
            Ok(board) => Some(board),
            Err(message) => {
                let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                return 2;
            }
        },
        None => None,
    };
    let physical = (|| {
        if let Some(routed) = routed {
            return Ok(routed);
        }
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
    if let Some(extra) = unknown_placements(&physical, &view) {
        let _ = text::print(
            out,
            &format!(
                "EMI CHECK ERROR: the routed board places components the design does not have ({extra});                  --run must hold the board of this design"
            ),
        );
        return 2;
    }
    let model = copper_lang::emi::analyze(&view);
    let report = emi_check(&physical, &view, &model);
    // `--write`: the run's report keeps one copy of the result (`emi` section) for `copper issues`.
    let written = match (parsed.flag("--write"), parsed.value("--run")) {
        (true, Some(run)) => match write_section(&pypath::display(run), report.to_value()) {
            Ok(message) => Some(message),
            Err(message) => {
                let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                return 2;
            }
        },
        _ => None,
    };
    if parsed.flag("--json") {
        let _ = out.write_all(report.json().as_bytes());
    } else {
        for line in report.lines() {
            let _ = text::print(out, &line);
        }
    }
    if let Some(message) = written {
        // The JSON document owns stdout.
        if parsed.flag("--json") {
            let _ = writeln!(err, "{message}");
        } else {
            let _ = text::print(out, &message);
        }
    }
    // Screening findings never fail the command.
    0
}

/// Embed `section` in the route report of `run` (a run directory, or a state file inside one) and
/// say where.
fn write_section(run: &str, section: copper_json::Value) -> Result<String, String> {
    let path = Path::new(run);
    let directory = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    let located = copper_report::issues::load_run(&pypath::display(&directory.to_string_lossy()))
        .map_err(|error| error.message.clone())?;
    let report = located.report_path.ok_or_else(|| {
        format!("{run}: no route-report.json beside the run's board; --write needs the report of a finished run")
    })?;
    let written = crate::emi_section::write_into_report(Path::new(&report), section)?;
    let summary = written.summary.as_ref().map_or_else(
        || "no summary beside it".to_owned(),
        |path| format!("report_sha256 updated in {}", path.display()),
    );
    Ok(format!("emi section written to {} ({summary}; sha256 {})", written.report.display(), written.digest))
}

/// The routed `PhysicalBoard` of a run: a `copperscript.state/v1` file holding one (as written by
/// `route-board --checkpoint-dir DIR` to `DIR/final-board.state.json`), or a directory holding
/// `final-board.state.json` itself or in `checkpoint/`.
fn routed_board(run: &str) -> Result<copper_pcb::board::PhysicalBoard, String> {
    let path = Path::new(run);
    let file = if path.is_file() {
        path.to_path_buf()
    } else {
        ["final-board.state.json", "checkpoint/final-board.state.json"]
            .iter()
            .map(|name| path.join(name))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                format!(
                    "{run}: no final-board.state.json (route-board --checkpoint-dir writes one); a run that holds only                      board.kicad_pcb cannot be loaded because no converter from a KiCad board to the physical board exists"
                )
            })?
    };
    let text = std::fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let document = copper_state::StateDocument::read(&text).map_err(|error| format!("{}: {error}", file.display()))?;
    copper_route::state_codec::physical_board(&document.value)
        .map_err(|error| format!("{}: not a PhysicalBoard state ({error})", file.display()))
}

/// Placement references of `board` that are no component of the design, joined; `None` when all match.
fn unknown_placements(
    board: &copper_pcb::board::PhysicalBoard,
    view: &copper_lang::model::FlatElectricalView,
) -> Option<String> {
    let unknown: Vec<&str> = board
        .placements
        .iter()
        .map(|placement| placement.reference.as_str())
        .filter(|reference| !view.components.iter().any(|component| component.reference == *reference))
        .collect();
    (!unknown.is_empty()).then(|| unknown.join(", "))
}

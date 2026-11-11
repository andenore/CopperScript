//! `copper emi-check BOARD [--locked] [--offline] [--layers {2,4,6}]
//! [--fab-profile PROFILE] [--footprint-root DIR]... [--allow-proxy-footprints] [--run RUN [--write]]
//! [--plot-spectrum OUT.svg] [--plot-field OUT.svg [--field-height MM]] [--json]`:
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
[--allow-proxy-footprints] [--run RUN [--write]] [--plot-spectrum PLOT_SPECTRUM] [--plot-field PLOT_FIELD] [--field-height FIELD_HEIGHT] [--json] board";

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
        value("--plot-spectrum"),
        value("--plot-field"),
        value("--field-height"),
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
    let height_mm = match parsed.value("--field-height") {
        None => copper_report::emi_field::DEFAULT_HEIGHT_MM,
        Some(_) if parsed.value("--plot-field").is_none() => {
            return usage_error(err, "argument --field-height: needs --plot-field OUT.svg");
        }
        Some(text) => match length_mm(text) {
            Some(height) => height,
            None => return usage_error(err, &format!("argument --field-height: invalid length: '{text}' (e.g. 1mm)")),
        },
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
    let mut plotted: Vec<String> = Vec::new();
    if parsed.value("--plot-spectrum").is_some() || parsed.value("--plot-field").is_some() {
        let document = report.to_value();
        if let Some(path) = parsed.value("--plot-spectrum") {
            match plot_spectrum(&document, &pypath::display(path)) {
                Ok(message) => plotted.push(message),
                Err(message) => {
                    let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                    return 2;
                }
            }
        }
        if let Some(path) = parsed.value("--plot-field") {
            let source = if parsed.value("--run").is_some() { "routed board" } else { "placed board" };
            match plot_field(&document, &physical, &pypath::display(path), height_mm, source) {
                Ok(message) => plotted.push(message),
                Err(message) => {
                    let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                    return 2;
                }
            }
        }
    }
    for message in plotted {
        if parsed.flag("--json") {
            let _ = writeln!(err, "{message}");
        } else {
            let _ = text::print(out, &message);
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

/// `1`, `1mm`, `0.5 mm` as millimetres; `None` unless positive.
fn length_mm(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text.strip_suffix("mm").unwrap_or(text).trim();
    number.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

fn write_plot(path: &str, svg: &str) -> Result<(), String> {
    if let Some(parent) = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    copper_json::text::write_text(Path::new(path), svg).map_err(|error| format!("{path}: {error}"))
}

/// `--plot-spectrum`: one panel per cell with an `EMI-CONDUCTED` finding.
fn plot_spectrum(document: &copper_json::Value, path: &str) -> Result<String, String> {
    let panels = copper_report::emi_plot::panels_from_report(document);
    if panels.is_empty() {
        return Err("--plot-spectrum: no EMI-CONDUCTED finding with harmonics to plot (see the EMI-CONDUCTED:<cell> coverage entries for why a cell has none)".to_owned());
    }
    let board = document.get("board").and_then(copper_json::Value::as_str).unwrap_or("board");
    write_plot(path, &copper_report::emi_plot::spectrum_svg(board, &panels))?;
    Ok(format!(
        "conducted spectrum of {} cell(s) written to {path} (screening estimate, not EMC qualification)",
        panels.len()
    ))
}

/// `--plot-field`: the near-field map of every hot loop, stacked in one SVG.
fn plot_field(
    document: &copper_json::Value,
    physical: &copper_pcb::board::PhysicalBoard,
    path: &str,
    height_mm: f64,
    source: &str,
) -> Result<String, String> {
    let state = copper_route::state_codec::encode_board(physical);
    let scene = copper_report::scene::scene_from_physical(&state).map_err(|error| error.message.clone())?;
    let findings: Vec<&copper_json::Value> = document
        .get("findings")
        .and_then(copper_json::Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("code").and_then(copper_json::Value::as_str) == Some("EMI-HOT-LOOP"))
        .collect();
    if findings.is_empty() {
        return Err("--plot-field: no EMI-HOT-LOOP finding (no hot loop could be built; see the EMI-HOT-LOOP:<cell> coverage entries)".to_owned());
    }
    let mut pictures = Vec::new();
    for finding in findings {
        let map = copper_report::emi_field::hot_loop_field(
            finding,
            document,
            &scene,
            height_mm,
            copper_report::emi_field::DEFAULT_STEP_MM,
        )?;
        pictures.push(copper_report::emi_field::field_picture(&map, source));
    }
    let board = document.get("board").and_then(copper_json::Value::as_str).unwrap_or("board");
    write_plot(path, &copper_report::emi_field::field_svg(board, &pictures))?;
    Ok(format!(
        "near-field map of {} hot loop(s) at {height_mm} mm written to {path} (screening estimate; the inductor's own field is not included)",
        pictures.len()
    ))
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

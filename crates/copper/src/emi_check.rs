//! `copper emi-check BOARD [--locked] [--offline] [--layers {2,4,6}]
//! [--fab-profile PROFILE] [--footprint-root DIR]... [--allow-proxy-footprints] [--run RUN [--write]]
//! [--plot-spectrum OUT.svg] [--plot-field OUT.svg [--field-height MM]] [--plot-impedance OUT.svg]
//! [--plot-current OUT.svg] [--plot-summary OUT.svg]
//! [--fullwave [--fullwave-fidelity {coarse,fine}] [--fullwave-budget SECONDS] [--fullwave-margin MM] [--fullwave-capacitor-height MM] [--fullwave-source-height MM] [--fullwave-zone-outline]]
//! [--extract [--extract-budget FILAMENTS] [--extract-margin MM] [--extract-cell MM] [--extract-width-cells N] [--extract-single-mesh]
//! [--extract-no-switch-bar]] [--verbose] [--json]`:
//! screen the placed switching cells for noise sources (hot loop, switch node,
//! proximity). Screening evidence only; findings never change the exit status.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_drc::emi::emi_check_with_return_path;
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
[--allow-proxy-footprints] [--run RUN [--write]] [--plot-spectrum PLOT_SPECTRUM] [--plot-field PLOT_FIELD] [--field-height FIELD_HEIGHT] [--plot-impedance PLOT_IMPEDANCE] [--plot-current PLOT_CURRENT] [--plot-summary PLOT_SUMMARY] \n[--fullwave] [--fullwave-fidelity {coarse,fine}] [--fullwave-budget SECONDS] [--fullwave-margin MM] [--fullwave-capacitor-height MM] [--fullwave-source-height MM] [--fullwave-single-mesh] [--fullwave-zone-outline] \n[--extract] [--extract-budget FILAMENTS] [--extract-margin MM] [--extract-cell MM] [--extract-width-cells N] [--extract-single-mesh] [--extract-no-switch-bar] [--extract-ideal-capacitors] [--extract-reference] [--verbose] [--json] board";

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
        value("--plot-impedance"),
        value("--plot-current"),
        value("--plot-summary"),
        flag("--fullwave"),
        value("--fullwave-fidelity"),
        value("--fullwave-budget"),
        value("--fullwave-capacitor-height"),
        value("--fullwave-source-height"),
        value("--fullwave-margin"),
        flag("--fullwave-single-mesh"),
        flag("--fullwave-zone-outline"),
        flag("--extract"),
        value("--extract-budget"),
        value("--extract-margin"),
        value("--extract-cell"),
        value("--extract-width-cells"),
        flag("--extract-single-mesh"),
        flag("--extract-ideal-capacitors"),
        flag("--extract-no-switch-bar"),
        flag("--extract-reference"),
        flag("--verbose"),
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
             inductor and switch-node proximity); screening evidence, not EMC qualification. The text report is compact (one line for each finding, at most 110 characters a line); --verbose prints the full text with every input and caveat. --fullwave adds, per buck cell, the \n             hot-loop inductance from a region full-wave (FDTD) model of the loop (two meshes, a standard one and one refined by 1.5 around the port and pads, when the budget allows: minutes; \n             --fullwave-fidelity coarse|fine, --fullwave-budget SECONDS refuses a model estimated to run longer and stops one that overruns it by half, \n             --fullwave-margin MM sets the window margin around the loop, --fullwave-single-mesh runs one mesh only, --fullwave-zone-outline takes the pours from the zone outlines minus clearances instead of the native zone fill; a ring-down fit that fails its withheld-tail, stability or passivity checks makes the value indicative). \n             --extract adds, per buck cell, the hot-loop interconnect impedance (L and R at 1, 10 and 100 MHz) from a quasi-static partial-element extraction of the real filled copper (zone fills natively, tracks, pads, vias; measures.extract, an EMI-EXTRACT:<cell> coverage entry), beside the closed-form and full-wave values, with the numerical sensitivity of a series of three meshes (the finest and two coarser by 1.5); the connection inductances of the other capacitors on the hot node and their couplings to the hot loop are extracted as a multi-port matrix and used in EMI-CONDUCTED in place of the estimates (tag extracted_connection_inductance, the estimate stays beside them; a capacitor that no copper joins to the hot node, whose port the extraction closed with a straight bar, keeps its estimate, and an extraction whose mesh does not have the copper's connectivity has status error and is not used); seconds to minutes, never part of the default run \n             (--extract-budget FILAMENTS bounds the dense system, default 9000, at most 32768; 32768 requires 8.6 GB for the inductance matrix alone, plus assembly/factorization workspace; --extract-margin MM, --extract-cell MM, --extract-width-cells N (4 to 16 requested cells across loop copper), --extract-single-mesh, --extract-no-switch-bar). For buck-boost, --extract assembles a joint input/output native-copper network with independent signed contact ports (at most 16), shared-return and mutual coupling, and no package shorts or assumed current sharing. Each loop reports its own port and current map; other port currents are zero. Selected capacitor copper fixtures must not cross native copper. --extract-ideal-capacitors explicitly uses ideal terminal shorts instead, excluding capacitor body copper, its magnetic field, ESR and ESL (PEEC only). Repeated pad numbers require matching occurrences and one continuous land. --fullwave attempts experimental external source-gap matrices with a shared per-controller budget and explicit geometry, passivity and convergence checks; its boundary differs from PEEC. --fullwave-capacitor-height MM explicitly replaces the external capacitor fixtures by raised PEC bridges and rectangular terminal posts, retaining native copper below them; the supplied height is an analysis assumption, fixture fields are included, and component ESR/ESL and dielectric remain excluded. --fullwave-source-height MM explicitly elevates facing controller source gaps on rectangular PEC terminal posts; the height and fixture fields are analysis assumptions, not a characterized package model. Intersecting source volumes or fixtures are refused. Inspect extract_scope/fullwave_scope and result status: executed does not mean usable. Passive matrices do not provide a switching waveform or enable conducted, radiation or ringing prediction. --extract-reference also runs an independent reference solver from the same copper (a raster of every layer, flood-fill topology, finite-difference DC conduction, the DC inductance from the field energy of the current distribution: it shares no code with the mesher) and puts its R and L matrices beside the extraction's at 1 kHz in measures.extract.reference, with the difference of every entry; a disagreement beyond the stated limit adds a status reason (flag reference_disagrees), a topology disagreement makes the status error; it is the DC limit and does not validate skin or proximity redistribution; seconds to minutes more, never part of the default run
             --plot-summary OUT.svg draws the one-page summary of each cell (a title, one line of facts, up to five stat tiles, the findings as chips, and the near field, the conducted spectrum, the input-network impedance and with --extract the current distribution as pictures), the picture to open first; with EMI-RETURN-PATH findings (--run) it adds a return-path map with numbered marks (gaps, reference changes without a return path, plane and board edge points) and a legend beside it, and on a board with no switching cell that map is the whole picture; one page then charts the mode conversion of the pairs (a bar per pair in dB, the worst first, with its channel of sections and parts beside it) and one page per cable connector charts the common-mode voltage of each driver in dBuV, in three groups (pairs, ground noise, ranking only); one footer line ends the document. --plot-spectrum also draws, for each cable connector whose finding publishes a voltage, the same driver chart (no limit line: voltages, not a field). For each cable connector the report has an EMI-CABLE-COMMON-MODE finding: the common-mode voltage (dBuV) each driver leaves at the connector and a ranking of the drivers, never a current, a field or a margin (differential pairs need --run)"
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
    let fullwave = match fullwave_options(&parsed) {
        Ok(options) => options,
        Err(message) => return usage_error(err, &message),
    };
    let extract = match extract_options(&parsed) {
        Ok(options) => options,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.value("--plot-current").is_some() && extract.is_none() {
        return usage_error(err, "argument --plot-current: needs --extract");
    }
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
    if parsed.value("--run").is_some()
        && let Some(differences) = net_mismatches(&physical, &view)
    {
        let _ = text::print(
            out,
            &format!(
                "EMI CHECK ERROR: the routed board connects pads differently from the design ({differences}); --run must hold the board of this design"
            ),
        );
        return 2;
    }
    let model = copper_lang::emi::analyze(&view);
    // The return-path check needs the routed copper (`--run`) and the native zone fills of its board.
    let return_path = crate::emi_section::ReturnPath::prepare(&physical, &model, parsed.value("--run").is_some());
    let mut report = emi_check_with_return_path(&physical, &view, &model, &return_path.input());
    if let Some(options) = &fullwave {
        crate::emi_fullwave::apply(&physical, &mut report, options, err);
    }
    let mut extracted: Vec<(String, copper_extract::hotloop::HotLoopExtraction)> = Vec::new();
    if let Some(options) = &extract {
        extracted = crate::emi_extract::apply(&physical, &view, &model, &mut report, options, err);
    }
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
        let lines = if parsed.flag("--verbose") {
            report.lines()
        } else {
            let required: Vec<String> =
                report.coverage.iter().filter(|entry| entry.required).map(|entry| entry.check.clone()).collect();
            copper_report::emi_text::compact_lines(&report.to_value(), &required)
        };
        for line in lines {
            let _ = text::print(out, &line);
        }
    }
    let mut plotted: Vec<String> = Vec::new();
    if let Some(path) = parsed.value("--plot-current") {
        match plot_current(&report.board, &extracted, &pypath::display(path)) {
            Ok(message) => plotted.push(message),
            Err(message) => {
                let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                return 2;
            }
        }
    }
    if let Some(path) = parsed.value("--plot-impedance") {
        match plot_impedance(&report, &pypath::display(path)) {
            Ok(message) => plotted.push(message),
            Err(message) => {
                let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                return 2;
            }
        }
    }
    if let Some(path) = parsed.value("--plot-summary") {
        let input = SummaryInput {
            report: &report,
            physical: &physical,
            extracted: &extracted,
            parsed: &parsed,
            height_mm,
            extract: extract.is_some(),
            fullwave: fullwave.is_some(),
        };
        match plot_summary(&input, &pypath::display(path)) {
            Ok(message) => plotted.push(message),
            Err(message) => {
                let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                return 2;
            }
        }
    }
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
            match plot_field(&document, &physical, &pypath::display(path), height_mm) {
                Ok(message) => plotted.push(message),
                Err(message) => {
                    let _ = text::print(out, &format!("EMI CHECK ERROR: {message}"));
                    return 2;
                }
            }
        }
    }
    for message in plotted.into_iter().chain(written) {
        // The JSON document owns stdout.
        if parsed.flag("--json") {
            let _ = writeln!(err, "{message}");
        } else if parsed.flag("--verbose") {
            let _ = text::print(out, &message);
        } else {
            for line in compact_message(&message) {
                let _ = text::print(out, &line);
            }
        }
    }
    // Screening findings never fail the command.
    0
}

/// A message about a file written (a plot, the `--write` section) for the compact report: without the caveat in
/// parentheses after the path, which `--verbose` prints, and wrapped to the width of the report.
fn compact_message(message: &str) -> Vec<String> {
    let caveat =
        ["(screening", "(report_sha256", "(no summary"].iter().filter_map(|a| message.find(&format!(" {a}"))).min();
    let short = match caveat {
        Some(start) => {
            let rest = &message[start..];
            let tail = rest.find(')').map_or("", |end| &rest[end + 1..]);
            format!("{}{tail}", &message[..start])
        }
        None => message.to_owned(),
    };
    copper_report::emi_text::wrapped(&short, copper_report::emi_text::MAX_WIDTH, 2)
}

/// The `--fullwave*` options: `None` without `--fullwave`; a usage error when a sub-option is given without it.
fn fullwave_options(parsed: &option_parser::Parsed) -> Result<Option<crate::emi_fullwave::Options>, String> {
    let (fidelity, budget, margin) =
        (parsed.value("--fullwave-fidelity"), parsed.value("--fullwave-budget"), parsed.value("--fullwave-margin"));
    if !parsed.flag("--fullwave") {
        for (name, given) in [
            ("--fullwave-fidelity", fidelity),
            ("--fullwave-budget", budget),
            ("--fullwave-margin", margin),
            ("--fullwave-capacitor-height", parsed.value("--fullwave-capacitor-height")),
            ("--fullwave-source-height", parsed.value("--fullwave-source-height")),
            ("--fullwave-single-mesh", parsed.flag("--fullwave-single-mesh").then_some("")),
            ("--fullwave-zone-outline", parsed.flag("--fullwave-zone-outline").then_some("")),
        ] {
            if given.is_some() {
                return Err(format!("argument {name}: needs --fullwave"));
            }
        }
        return Ok(None);
    }
    let fidelity = match fidelity.map(str::trim) {
        None | Some("coarse") => copper_fullwave::Fidelity::Coarse,
        Some("fine") => copper_fullwave::Fidelity::Fine,
        Some(other) => {
            return Err(format!("argument --fullwave-fidelity: invalid choice: '{other}' (choose from coarse, fine)"));
        }
    };
    let positive = |name: &str, text: Option<&str>| -> Result<Option<f64>, String> {
        match text.map(str::trim) {
            None => Ok(None),
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(Some)
                .ok_or_else(|| format!("argument {name}: invalid positive number: '{text}'")),
        }
    };
    Ok(Some(crate::emi_fullwave::Options {
        fidelity,
        budget_seconds: positive("--fullwave-budget", budget)?,
        margin_mm: positive("--fullwave-margin", margin)?,
        capacitor_height_mm: positive("--fullwave-capacitor-height", parsed.value("--fullwave-capacitor-height"))?,
        source_height_mm: positive("--fullwave-source-height", parsed.value("--fullwave-source-height"))?,
        single_mesh: parsed.flag("--fullwave-single-mesh"),
        zone_outline: parsed.flag("--fullwave-zone-outline"),
    }))
}

/// The `--extract*` options: `None` without `--extract`; a usage error when a sub-option is given without it.
fn extract_options(parsed: &option_parser::Parsed) -> Result<Option<crate::emi_extract::Options>, String> {
    let (budget, margin, cell) =
        (parsed.value("--extract-budget"), parsed.value("--extract-margin"), parsed.value("--extract-cell"));
    let width = parsed.value("--extract-width-cells");
    if !parsed.flag("--extract") {
        for (name, given) in [
            ("--extract-budget", budget),
            ("--extract-margin", margin),
            ("--extract-cell", cell),
            ("--extract-width-cells", width),
            ("--extract-single-mesh", parsed.flag("--extract-single-mesh").then_some("")),
            ("--extract-ideal-capacitors", parsed.flag("--extract-ideal-capacitors").then_some("")),
            ("--extract-no-switch-bar", parsed.flag("--extract-no-switch-bar").then_some("")),
            ("--extract-reference", parsed.flag("--extract-reference").then_some("")),
        ] {
            if given.is_some() {
                return Err(format!("argument {name}: needs --extract"));
            }
        }
        return Ok(None);
    }
    let positive = |name: &str, text: Option<&str>| -> Result<Option<f64>, String> {
        match text.map(str::trim) {
            None => Ok(None),
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(Some)
                .ok_or_else(|| format!("argument {name}: invalid positive number: '{text}'")),
        }
    };
    let budget_filaments = match budget.map(str::trim) {
        None => None,
        Some(text) => match text.parse::<usize>() {
            Ok(n) if (50..=32_768).contains(&n) => Some(n),
            Ok(n) => {
                return Err(format!(
                    "argument --extract-budget: {n} filaments is outside 50 to 32768 (the inductance matrix alone is n^2 doubles, plus assembly/factorization workspace)"
                ));
            }
            Err(_) => return Err(format!("argument --extract-budget: invalid int value: '{text}'")),
        },
    };
    let width_cells = width
        .map(|text| {
            text.trim().parse::<usize>().ok().filter(|n| (4..=16).contains(n)).ok_or_else(|| {
                format!("argument --extract-width-cells: expected an integer from 4 to 16, got '{text}'")
            })
        })
        .transpose()?;
    Ok(Some(crate::emi_extract::Options {
        budget_filaments,
        width_cells,
        margin_mm: positive("--extract-margin", margin)?,
        cell_mm: positive("--extract-cell", cell)?,
        single_mesh: parsed.flag("--extract-single-mesh"),
        no_switch_bar: parsed.flag("--extract-no-switch-bar"),
        ideal_capacitors: parsed.flag("--extract-ideal-capacitors"),
        reference: parsed.flag("--extract-reference"),
        current_map: parsed.value("--plot-current").is_some() || parsed.value("--plot-summary").is_some(),
    }))
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

/// `--plot-spectrum`: one panel per cell with an `EMI-CONDUCTED` finding, then one chart of the driver voltages per
/// cable connector that publishes one.
fn plot_spectrum(document: &copper_json::Value, path: &str) -> Result<String, String> {
    let panels = copper_report::emi_plot::panels_from_report(document);
    // The voltage spectra of the drivers at each cable connector that has a published voltage: no limit, no field.
    let connectors = copper_report::emi_cable_plot::connector_pictures(document);
    if panels.is_empty() && connectors.is_empty() {
        return Err("--plot-spectrum: no EMI-CONDUCTED finding with harmonics to plot (see the EMI-CONDUCTED:<cell> coverage entries for why a cell has none)".to_owned());
    }
    let board = document.get("board").and_then(copper_json::Value::as_str).unwrap_or("board");
    let (drivers, connector_count) = copper_report::emi_cable_plot::voltage_counts(document);
    let svg = if connectors.is_empty() {
        copper_report::emi_plot::spectrum_svg(board, &panels)
    } else {
        copper_report::emi_plot::spectrum_svg_with(board, &panels, &connectors)
    };
    write_plot(path, &svg)?;
    let mut parts: Vec<String> = Vec::new();
    if !panels.is_empty() {
        parts.push(format!(
            "conducted spectrum of {}",
            copper_report::emi_impedance::counted(panels.len(), "cell", "cells")
        ));
    }
    if !connectors.is_empty() {
        parts.push(format!(
            "common-mode voltages at the connector of {} on {} (no limit line: voltages, not a field)",
            copper_report::emi_impedance::counted(drivers, "driver", "drivers"),
            copper_report::emi_impedance::counted(connector_count, "connector", "connectors")
        ));
    }
    Ok(format!("{} written to {path} (screening estimate, not EMC qualification)", parts.join(" and ")))
}

/// The picture data of the input network of an `EMI-CONDUCTED` finding; `None` for one without its network.
fn impedance_panel(
    finding: &copper_drc::emi::EmiFinding,
) -> Option<Result<copper_report::emi_impedance::ImpedancePanel, String>> {
    use std::collections::BTreeMap;
    let (Some(input), Some(cell)) = (finding.conducted_input.as_deref(), finding.cell.as_deref()) else { return None };
    let sources: BTreeMap<String, String> = finding
        .measures
        .get("capacitor_branches")
        .and_then(copper_json::Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|b| {
            let component = b.get("component").and_then(copper_json::Value::as_str)?;
            let source = b.get("connection_source").and_then(copper_json::Value::as_str)?;
            Some((component.to_owned(), source.to_owned()))
        })
        .collect();
    let extraction = finding
        .measures
        .get("extracted_connections")
        .and_then(|e| e.get("status"))
        .and_then(copper_json::Value::as_str);
    let scenario = finding.tags.iter().find_map(|t| t.strip_prefix("frequency_scenario_"));
    Some(copper_report::emi_impedance::panel_from_input(cell, scenario, &finding.tags, input, &sources, extraction))
}

/// `--plot-impedance`: the input-network impedance of every `EMI-CONDUCTED` finding, stacked in one SVG.
fn plot_impedance(report: &copper_drc::emi::EmiCheckReport, path: &str) -> Result<String, String> {
    let mut panels = Vec::new();
    for finding in report.findings.iter().filter(|f| f.finding.code == "EMI-CONDUCTED") {
        if let Some(panel) = impedance_panel(finding) {
            panels.push(panel?);
        }
    }
    if panels.is_empty() {
        return Err("--plot-impedance: no EMI-CONDUCTED finding with a network to plot (see the EMI-CONDUCTED:<cell> coverage entries for why a cell has none)".to_owned());
    }
    write_plot(path, &copper_report::emi_impedance::impedance_svg(&report.board, &panels))?;
    Ok(format!(
        "input-network impedance of {} written to {path} (screening estimate, not EMC qualification)",
        copper_report::emi_impedance::counted(panels.len(), "cell", "cells")
    ))
}

/// `--plot-current`: the current distribution of every extracted hot loop, stacked in one SVG.
fn plot_current(
    board: &str,
    extracted: &[(String, copper_extract::hotloop::HotLoopExtraction)],
    path: &str,
) -> Result<String, String> {
    let pictures: Vec<copper_report::emi_field::FieldPicture> = extracted
        .iter()
        .filter_map(|(cell, result)| crate::emi_current::panel(cell, result))
        .map(|panel| copper_report::emi_current::current_picture(&panel))
        .collect();
    if pictures.is_empty() {
        return Err("--plot-current: no hot-loop extraction produced a current distribution (see the EMI-EXTRACT:<cell> coverage entries for why a cell has none)".to_owned());
    }
    write_plot(path, &copper_report::emi_current::current_svg(board, &pictures))?;
    Ok(format!(
        "current distribution of {} written to {path} (screening estimate from the quasi-static extraction)",
        copper_report::emi_impedance::counted(pictures.len(), "hot loop", "hot loops")
    ))
}

/// `--plot-field`: the near-field map of every hot loop, stacked in one SVG.
fn plot_field(
    document: &copper_json::Value,
    physical: &copper_pcb::board::PhysicalBoard,
    path: &str,
    height_mm: f64,
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
        pictures.push(copper_report::emi_field::field_picture(&map));
    }
    let board = document.get("board").and_then(copper_json::Value::as_str).unwrap_or("board");
    write_plot(path, &copper_report::emi_field::field_svg(board, &pictures))?;
    Ok(format!(
        "near-field map of {} at {height_mm} mm written to {path} (screening estimate; the inductor's own field is not included)",
        copper_report::emi_impedance::counted(pictures.len(), "hot loop", "hot loops")
    ))
}

/// What `--plot-summary` needs besides the report.
struct SummaryInput<'a> {
    report: &'a copper_drc::emi::EmiCheckReport,
    physical: &'a copper_pcb::board::PhysicalBoard,
    extracted: &'a [(String, copper_extract::hotloop::HotLoopExtraction)],
    parsed: &'a option_parser::Parsed,
    height_mm: f64,
    extract: bool,
    fullwave: bool,
}

/// `--plot-summary`: one page per cell, the key numbers, the findings and four thumbnails of the other pictures;
/// when the report has `EMI-RETURN-PATH` findings, the return-path map after them (and, on a board with no switching
/// cell, in place of them); then the one page of the mode conversion of the pairs and one page per cable connector,
/// and the one footer line of the document.
fn plot_summary(input: &SummaryInput, path: &str) -> Result<String, String> {
    use copper_report::emi_summary::{CellThumbnails, Context, Thumb};
    let document = input.report.to_value();
    let cells: Vec<String> = document
        .get("cells")
        .and_then(copper_json::Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.get("controller").and_then(copper_json::Value::as_str).map(str::to_owned))
        .collect();
    let return_paths = copper_report::emi_return_path::findings(&document).len();
    let signal_findings = copper_report::emi_signal::signal_findings(&document).len();
    let connector_findings = copper_report::emi_signal::connector_findings(&document).len();
    if cells.is_empty() && signal_findings == 0 && connector_findings == 0 {
        return Err(
            "--plot-summary: no switching cell and no return-path, mode-conversion or cable finding to summarise"
                .to_owned(),
        );
    }
    let state = copper_route::state_codec::encode_board(input.physical);
    let scene = copper_report::scene::scene_from_physical(&state).map_err(|error| error.message.clone());
    let spectrum = copper_report::emi_plot::panels_from_report(&document);
    let findings: &[copper_json::Value] =
        document.get("findings").and_then(copper_json::Value::as_array).unwrap_or_default();
    let impedance: Vec<Result<copper_report::emi_impedance::ImpedancePanel, String>> = input
        .report
        .findings
        .iter()
        .filter(|f| f.finding.code == "EMI-CONDUCTED")
        .filter_map(impedance_panel)
        .collect();
    let missing = |text: &str| Thumb::Missing(text.to_owned());
    let mut thumbnails = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let loop_finding = findings.iter().find(|f| {
            f.get("code").and_then(copper_json::Value::as_str) == Some("EMI-HOT-LOOP")
                && f.get("cell").and_then(copper_json::Value::as_str) == Some(cell.as_str())
        });
        let overlay = match (loop_finding, &scene) {
            (None, _) => missing("no near-field map: no EMI-HOT-LOOP finding (copper explain EMI-HOT-LOOP)"),
            (_, Err(why)) => Thumb::Missing(format!("no near-field map: {why}")),
            (Some(finding), Ok(scene)) => match copper_report::emi_field::hot_loop_field(
                finding,
                &document,
                scene,
                input.height_mm,
                copper_report::emi_field::DEFAULT_STEP_MM,
            ) {
                Ok(map) => Thumb::Picture(copper_report::emi_field::field_thumbnail(&map)),
                Err(why) => Thumb::Missing(format!("no near-field map: {why}")),
            },
        };
        let spectrum_thumb = match spectrum.iter().find(|p| p.cell == *cell) {
            Some(panel) => Thumb::Picture(copper_report::emi_plot::spectrum_thumbnail(panel, i)),
            None => missing("no conducted spectrum: no EMI-CONDUCTED finding with harmonics"),
        };
        let impedance_thumb = match impedance.iter().find(|p| p.as_ref().map_or(true, |p| p.cell == *cell)) {
            Some(Ok(panel)) => Thumb::Picture(copper_report::emi_impedance::impedance_thumbnail(panel, i)),
            Some(Err(why)) => Thumb::Missing(format!("no impedance plot: {why}")),
            None => missing("no impedance plot: no EMI-CONDUCTED finding with a network"),
        };
        let current_thumb = if !input.extract {
            // No extraction: no picture and no placeholder.
            missing("")
        } else {
            let drawn = input
                .extracted
                .iter()
                .find(|(name, _)| name == cell)
                .and_then(|(name, result)| crate::emi_current::panel(name, result));
            match drawn {
                Some(panel) => Thumb::Picture(copper_report::emi_current::current_thumbnail(&panel, i)),
                None => Thumb::Missing(format!("no current density: no result (copper explain EMI-EXTRACT:{cell})")),
            }
        };
        thumbnails.push(CellThumbnails {
            cell: cell.clone(),
            overlay,
            spectrum: spectrum_thumb,
            impedance: impedance_thumb,
            current: current_thumb,
        });
    }
    let context = Context { run: input.parsed.value("--run"), extract: input.extract, fullwave: input.fullwave };
    let mut pages = copper_report::emi_summary::summary_pages(&document, &context, &thumbnails);
    let cell_pages = pages.len();
    pages.extend(copper_report::emi_return_path::return_path_picture(&document, scene.as_ref().ok()));
    // After the cells and the map: the one page of the mode conversion of the pairs, then one page per cable connector,
    // and the one footer line of the document at its very end.
    let pairs_page = copper_report::emi_summary::pairs_page(&document);
    let connector_pages = copper_report::emi_summary::connector_pages(&document);
    let (pairs, connectors) = (
        pairs_page.as_ref().map_or(0, |_| copper_report::emi_signal::conversions(&document).len()),
        connector_pages.len(),
    );
    pages.extend(pairs_page);
    pages.extend(connector_pages);
    // The one footer line of the document, at its very end.
    pages.push(copper_report::emi_summary::footer_page(cell_pages > 0));
    write_plot(path, &copper_report::emi_summary::summary_svg(&input.report.board, &pages))?;
    let mut parts: Vec<String> = Vec::new();
    if cell_pages > 0 {
        parts.push(format!("summary of {}", copper_report::emi_impedance::counted(cell_pages, "cell", "cells")));
    }
    if return_paths > 0 {
        parts.push(format!(
            "{}return-path map of {}",
            if cell_pages > 0 { "the " } else { "" },
            copper_report::emi_impedance::counted(return_paths, "finding", "findings")
        ));
    }
    // The pages of signals, pairs and connectors follow the message of the cell pages and the map unchanged; with
    // neither, they are what was written.
    let mut extra: Vec<String> = Vec::new();
    if pairs > 0 {
        extra.push(format!(
            "the mode-conversion page of {}",
            copper_report::emi_impedance::counted(pairs, "pair", "pairs")
        ));
    }
    if connectors > 0 {
        extra.push(format!(
            "{} for cable connectors",
            copper_report::emi_impedance::counted(connectors, "page", "pages")
        ));
    }
    if parts.is_empty() {
        return Ok(format!("{} written to {path} (screening estimate, not EMC qualification)", extra.join(" and ")));
    }
    let also = if extra.is_empty() { String::new() } else { format!("; also {}", extra.join(" and ")) };
    Ok(format!("{} written to {path} (screening estimate, not EMC qualification){also}", parts.join(" and ")))
}

/// Embed `section` in the route report of `run` (a run directory, or a state file inside one) and
/// say where.
fn write_section(run: &str, section: copper_json::Value) -> Result<String, String> {
    let path = Path::new(run);
    let start = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    // A checkpoint file or directory sits inside the run directory that holds the report: look in
    // the starting directory and up to two levels above it, the way a run is resolved elsewhere.
    let mut located = None;
    for directory in start.ancestors().take(3).filter(|directory| !directory.as_os_str().is_empty()) {
        let found = copper_report::issues::load_run(&pypath::display(&directory.to_string_lossy()))
            .map_err(|error| error.message.clone())?;
        if found.report_path.is_some() {
            located = Some(found);
            break;
        }
    }
    let report = located.and_then(|run| run.report_path).ok_or_else(|| {
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

/// Pads of `board` whose net differs from the design's, as `component.pin: source net vs checkpoint
/// net` (the first few, then a count); `None` when every pad agrees.
///
/// The design's assignment is derived like `physicalize` does it: each net endpoint's package pad,
/// widened over the footprint's internal pad groups. Net names go through the design's alias table
/// on both sides. Pads the design leaves unconnected only differ when the checkpoint connects them,
/// and components the checkpoint does not place are skipped (`unknown_placements` owns those).
fn net_mismatches(
    board: &copper_pcb::board::PhysicalBoard,
    view: &copper_lang::model::FlatElectricalView,
) -> Option<String> {
    use std::collections::BTreeMap;
    let canonical = |name: &str| view.net_aliases.get(name).cloned().unwrap_or_else(|| name.to_owned());
    let components: BTreeMap<&str, &copper_lang::model::ComponentInstance> =
        view.components.iter().map(|c| (c.reference.as_str(), c)).collect();
    let placed: BTreeMap<&str, &copper_pcb::board::Placement> =
        board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    // (component, pad number) -> (source net, label of the pin that names it)
    let mut expected: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for net in &view.nets {
        for endpoint in &net.endpoints {
            let (Some(component), Some(placement)) =
                (components.get(endpoint.component.as_str()), placed.get(endpoint.component.as_str()))
            else {
                continue;
            };
            let Some(part) = view.library.get(&component.part) else { continue };
            let Some(number) =
                copper_pcb::physicalize::physical_pin_number(component, part, &view.devices, &endpoint.pin)
            else {
                continue;
            };
            let label = format!("{}.{}", endpoint.component, endpoint.pin);
            let mut pads = vec![number.clone()];
            if let Some(footprint) = board.footprints.get(&placement.footprint) {
                for group in &footprint.internal_pad_groups {
                    if group.numbers.contains(&number) {
                        pads.extend(group.numbers.iter().cloned());
                    }
                }
            }
            for pad in pads {
                expected
                    .entry((endpoint.component.clone(), pad))
                    .or_insert_with(|| (canonical(&net.name), label.clone()));
            }
        }
    }
    let actual: BTreeMap<(String, String), String> = board
        .nets
        .iter()
        .flat_map(|net| {
            net.pads.iter().map(move |pad| ((pad.component.clone(), pad.pad.clone()), canonical(&net.name)))
        })
        .filter(|((component, _), _)| components.contains_key(component.as_str()))
        .collect();
    let mut differences: Vec<String> = Vec::new();
    for (key, (source, label)) in &expected {
        let found = actual.get(key);
        if found != Some(source) {
            differences.push(format!("{label}: {source} vs {}", found.map_or("no net", String::as_str)));
        }
    }
    for (key, net) in &actual {
        if !expected.contains_key(key) {
            differences.push(format!("{}.{}: no net vs {net}", key.0, key.1));
        }
    }
    if differences.is_empty() {
        return None;
    }
    let total = differences.len();
    differences.truncate(4);
    let mut text = differences.join("; ");
    if total > differences.len() {
        text.push_str(&format!("; and {} more", total - differences.len()));
    }
    Some(format!("component.pin: source net vs checkpoint net: {text}"))
}

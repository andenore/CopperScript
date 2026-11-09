//! `copper synthesize-antenna BOARD [--antenna NAME] [--budget N] [--pixel-cell MM]
//! [--no-fine] [--verify] [--threads N] [--report PATH] [--locked] [--offline]
//! [--layers {2,4,6}] [--fab-profile PROFILE] [--footprint-root DIR]... [--allow-proxy-footprints]`:
//! tune one `constraint antenna` in a full-wave model of the board and print
//! the `geometry` and `tuned_for` lines to put in the declaration.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_antenna::model::Problem;
use copper_antenna::optimize::{Options, synthesize, verify};
use copper_antenna::report;
use copper_json::text;
use copper_lang::compiler::{Design, LoadError, load_design};
use copper_lang::elaborate::elaborate;
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::model::ConstraintKind;
use copper_lang::paths;
use copper_pcb::Error;
use copper_pcb::antenna::{chosen_geometry, describe, resolve_sites, site_digest};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, value};
use crate::pypath;

const USAGE: &str = "usage: copper synthesize-antenna [-h] [--antenna NAME] [--budget N] [--pixel-cell MM] \
[--no-fine] [--verify] [--threads N] [--report PATH] [--locked] [--offline] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--footprint-root FOOTPRINT_ROOT] \
[--allow-proxy-footprints] board";

const ABOUT: &str = "tune a constraint antenna in a full-wave (FDTD) model of the board: resonance tuning, \
Nelder-Mead polish (or a pixel genetic algorithm for topology = pixel), L-section matching, fine-mesh \
verification with radiation efficiency. Prints the geometry and tuned_for lines for the declaration. \
Simulation evidence only: bench tuning in the enclosure remains mandatory.";

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: synthesize-antenna ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// The design without antenna declarations: the board the sites are resolved on.
fn without_antennas(design: &Design) -> Design {
    let mut bare = design.clone();
    bare.electrical.constraints.retain(|c| c.kind != ConstraintKind::Antenna);
    for module in bare.electrical.module_definitions.values_mut() {
        module.constraints.retain(|c| c.kind != ConstraintKind::Antenna);
    }
    bare
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper synthesize-antenna: error: {message}");
        2
    };
    let specs = [
        value("--antenna"),
        value("--budget"),
        value("--pixel-cell"),
        flag("--no-fine"),
        flag("--verify"),
        value("--threads"),
        value("--report"),
        flag("--locked"),
        flag("--offline"),
        value("--layers"),
        value("--fab-profile"),
        value("--footprint-root"),
        flag("--allow-proxy-footprints"),
    ];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\n{ABOUT}");
        crate::help::options(out, "synthesize-antenna");
        return 0;
    }
    let integer = |name: &str, default: usize| -> Result<usize, String> {
        match parsed.value(name) {
            None => Ok(default),
            Some(text) => text.trim().parse().map_err(|_| format!("argument {name}: invalid int value: '{text}'")),
        }
    };
    // 0: one solver thread per physical core (`copper_antenna::model::auto_threads`).
    let (budget, threads) = match (integer("--budget", 12), integer("--threads", 0)) {
        (Ok(budget), Ok(threads)) => (budget, threads),
        (Err(message), _) | (_, Err(message)) => return usage_error(err, &message),
    };
    let pixel_cell = match parsed.value("--pixel-cell") {
        None => None,
        Some(text) => match text.trim().parse::<f64>() {
            Ok(mm) if mm > 0.0 => Some((mm * 1e6).round() as i64),
            _ => return usage_error(err, &format!("argument --pixel-cell: invalid millimetre value: '{text}'")),
        },
    };
    let layers = match parsed.value("--layers").map(str::trim) {
        None => 2,
        Some(text) => match text.parse::<i64>() {
            Ok(value) if [2, 4, 6].contains(&value) => value,
            _ => {
                return usage_error(err, &format!("argument --layers: invalid choice: '{text}' (choose from 2, 4, 6)"));
            }
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
        let options = PhysicalOptions::new(100.0, 80.0, layers, profile)?;
        let bare = without_antennas(&design);
        if parsed.flag("--allow-proxy-footprints") {
            physicalize(&bare, Footprints::Proxy, &options, locked, offline)
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> =
                parsed.values("--footprint-root").iter().map(|root| PathBuf::from(pypath::display(root))).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(&bare, Footprints::Resolved(&mut resolver), &options, locked, offline)
        }
    })();
    let sites = physical.and_then(|physical| {
        let flat = elaborate(&design.electrical).map_err(Error::Value)?;
        let sites = resolve_sites(&physical, &flat)?;
        Ok((physical, sites))
    });
    let (physical, sites) = match sites {
        Ok(result) => result,
        Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
        Err(Error::Value(message)) => {
            let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: {message}"));
            return 2;
        }
    };
    let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
    let site = match (parsed.value("--antenna"), sites.as_slice()) {
        (_, []) => {
            let _ = text::print(out, "ANTENNA SYNTHESIS ERROR: the board declares no constraint antenna");
            return 2;
        }
        (None, [site]) => site,
        (None, _) => return usage_error(err, &format!("--antenna is required; choose from {}", names.join(", "))),
        (Some(name), _) => match sites.iter().find(|s| s.name == name) {
            Some(site) => site,
            None => return usage_error(err, &format!("unknown antenna '{name}'; choose from {}", names.join(", "))),
        },
    };
    if site.topology == copper_pcb::antenna::Topology::Pixel
        && let Some(message) = copper_pcb::antenna::tile_zone_conflict(&physical, site)
    {
        let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: antenna '{}': {message}", site.name));
        return 2;
    }
    let digest = site_digest(&physical, site);
    let problem = match Problem::new(&physical, site) {
        Ok(problem) => problem,
        Err(message) => {
            let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: {message}"));
            return 2;
        }
    };
    let _ = text::print(out, &format!("SYNTHESIZE: {}", describe(site)));
    let _ = text::print(out, &format!("  site digest {digest}"));
    let fine = !parsed.flag("--no-fine");
    let outcome = if parsed.flag("--verify") {
        let geometry = match chosen_geometry(&physical, site) {
            Ok((geometry, _)) => geometry,
            Err(error) => {
                let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: {error}"));
                return 2;
            }
        };
        let _ = text::print(out, &format!("  verifying {}", geometry.to_source()));
        verify(&problem, &geometry, fine, threads).and_then(|(evaluation, score)| {
            let layout = copper_pcb::antenna::layout(site, &geometry).map_err(|e| e.to_string())?;
            Ok(copper_antenna::optimize::Outcome {
                geometry,
                layout,
                evaluation,
                score,
                trials: Vec::new(),
                method: "verify",
            })
        })
    } else {
        let options = Options { budget, fine, threads, pixel_cell };
        let mut log = |line: &str| {
            let _ = text::print(&mut *out, line);
        };
        synthesize(&problem, &options, &mut log)
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) => {
            let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: {message}"));
            return 2;
        }
    };
    let score = &outcome.score;
    let resonance =
        score.resonance_hz.map_or("none in the analysis window".to_owned(), |f| format!("{:.4} GHz", f / 1e9));
    let _ = text::print(
        out,
        &format!(
            "RESULT ({} fidelity{}): resonance {resonance}; worst return loss {:.1} dB bare, {:.1} dB matched (target {:.1} dB)",
            outcome.evaluation.fidelity,
            if outcome.evaluation.converged { "" } else { ", NOT CONVERGED" },
            score.raw_worst_db,
            score.worst_db,
            site.max_return_loss_db
        ),
    );
    for band in &score.bands {
        let verdict = if band.worst_db <= site.max_return_loss_db { "meets" } else { "MISSES" };
        let _ = text::print(
            out,
            &format!(
                "  {}: {:.1} dB bare, {:.1} dB matched ({verdict} target)",
                band.band, band.raw_worst_db, band.worst_db
            ),
        );
    }
    let _ = text::print(out, &format!("  matching network: {}", score.network.describe()));
    for (f, e) in &outcome.evaluation.efficiency {
        let _ = text::print(out, &format!("  radiation efficiency at {:.4} GHz: {:.0} %", f / 1e9, e * 100.0));
    }
    let document = report::document(&problem, site, &digest, &outcome);
    if let Some(path) = parsed.value("--report") {
        let path = PathBuf::from(pypath::display(path));
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, report::render(&document)) {
            let _ = text::print(out, &format!("ANTENNA SYNTHESIS ERROR: cannot write {}: {error}", path.display()));
            return 2;
        }
        let _ = text::print(out, &format!("Report -> {}", path.display()));
    }
    if outcome.method != "verify" {
        let _ = text::print(out, "Put these lines in the constraint antenna declaration:");
        let _ = text::print(out, &format!("    geometry = \"{}\";", outcome.geometry.to_source()));
        let _ = text::print(out, &format!("    tuned_for = \"{digest}\";"));
    }
    let _ = text::print(out, "Simulation evidence only: bench tuning in the final enclosure remains mandatory.");
    0
}

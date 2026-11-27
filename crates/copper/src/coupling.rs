//! `copper coupling BOARD (--from NET | --antenna NAME) --to NET [--to NET]... [--run RUN] [--band LOW:HIGH]
//! [--ground NET]... [--terminate END=OHMS]... [--fidelity {coarse,fine}] [--budget SECONDS] [--margin MM]
//! [--threads N] [--describe] [--json] [--locked] [--offline] [--layers {2,4,6}] [--fab-profile PROFILE]
//! [--footprint-root DIR]... [--allow-proxy-footprints]`:
//! the full-wave coupling from one net, or from a `constraint antenna`'s feed, to the ends of other nets.
//!
//! With `--from NET` the region model (`copper_fullwave::coupling`) holds the aggressor, the victims and the
//! ground nets around them, with a lumped port at every end of every one of them, and each aggressor end is
//! excited in turn. With `--antenna NAME` the antenna site model of `copper synthesize-antenna` holds the
//! declared antenna geometry and the victims (`copper_antenna::model::evaluate_coupled`), and the feed is
//! excited. Ends are terminated by `--terminate`, else by the driver's declared `output_impedance`, else 50 ohm,
//! and every end says which. `--run` takes the copper from a routed run; without it only pads are modelled.
//! Screening evidence: accuracy in `docs/coupling-query-plan.md`. Never fails on the size of the coupling.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use copper_antenna::model::{Fidelity as AntennaFidelity, Problem, Victim, evaluate_coupled};
use copper_fullwave::coupling::{CouplingRequest, NetEnd, Termination, Terminations, coupling_model, peak_in};
use copper_fullwave::{Budget, Complex64, Fidelity};
use copper_json::{Dumps, Object, Value, text};
use copper_lang::compiler::{Design, LoadError, load_design};
use copper_lang::footprints::FootprintResolver;
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::model::ConstraintKind;
use copper_lang::paths;
use copper_pcb::Error;
use copper_pcb::physicalize::{Footprints, physicalize};

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, flag, value};
use crate::pypath;

const USAGE: &str = "usage: copper coupling [-h] (--from NET | --antenna NAME) --to NET [--to NET ...] [--run RUN] \
[--band LOW:HIGH] [--ground NET] [--terminate END=OHMS] [--excite END] [--receiver END] [--stitch MM] [--fidelity {coarse,fine}] [--budget SECONDS] \
[--margin MM] [--threads N] [--describe] [--json] [--locked] [--offline] [--layers {2,4,6}] \
[--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}] [--footprint-root FOOTPRINT_ROOT] \
[--allow-proxy-footprints] board";

const SUMMARY: &str = "full-wave coupling from a net or an antenna's feed to the ends of other nets";

pub const SCHEMA: &str = "copperscript-coupling/v0.1";

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: coupling ({what})");
    NOT_IMPLEMENTED_EXIT
}

/// `100MHz`, `2.4GHz`, `150kHz`, `1e9` (Hz).
pub(crate) fn parse_hz(text: &str) -> Option<f64> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    let (number, scale) = [("ghz", 1e9), ("mhz", 1e6), ("khz", 1e3), ("hz", 1.0)]
        .iter()
        .find_map(|(suffix, scale)| lower.strip_suffix(suffix).map(|n| (n.trim().to_owned(), *scale)))
        .unwrap_or((lower.clone(), 1.0));
    number.parse::<f64>().ok().map(|v| v * scale).filter(|v| v.is_finite() && *v > 0.0)
}

/// One transmission in the report.
struct PathOut {
    from: String,
    to: String,
    to_net: String,
    termination: Termination,
    s: Vec<Complex64>,
}

struct RunOut {
    cells: usize,
    steps: u64,
    seconds: f64,
    converged: bool,
}

struct Report {
    mode: &'static str,
    board: String,
    from: String,
    victims: Vec<String>,
    /// The band the in-band figures cover.
    band: (f64, f64),
    freqs: Vec<f64>,
    paths: Vec<PathOut>,
    runs: Vec<RunOut>,
    notes: Vec<String>,
    /// `EMI-RF-DESENSE` findings: text lines and JSON entries.
    findings: Vec<(Vec<String>, Value)>,
}

fn db(s: &Complex64) -> f64 {
    20.0 * s.norm().max(1e-15).log10()
}

fn round(value: f64, digits: i32) -> f64 {
    let k = 10f64.powi(digits);
    (value * k).round() / k
}

fn put(object: &mut Object, key: &str, value: Value) {
    object.insert(key.to_owned(), value);
}

impl Report {
    fn lines(&self) -> Vec<String> {
        let mut lines =
            vec![format!("COUPLING: {} from {} to {} ({})", self.mode, self.from, self.victims.join(", "), self.board)];
        for run in &self.runs {
            lines.push(format!(
                "  run: {} cells, {} steps, {:.0} s{}",
                run.cells,
                run.steps,
                run.seconds,
                if run.converged { "" } else { ", energy not decayed (the record is truncated)" }
            ));
        }
        let mut seen = BTreeSet::new();
        for p in &self.paths {
            if seen.insert(p.to.clone()) {
                lines.push(format!(
                    "  end {} ({}): {} ohm, {}",
                    p.to, p.to_net, p.termination.ohms, p.termination.source
                ));
            }
        }
        let (lo, hi) = self.band;
        for p in &self.paths {
            let band = peak_in(&self.freqs, &p.s, lo, hi);
            let sweep = peak_in(&self.freqs, &p.s, f64::NEG_INFINITY, f64::INFINITY);
            let fmt =
                |x: Option<(f64, f64)>| x.map_or("-".to_owned(), |(f, d)| format!("{d:.1} dB at {:.3} GHz", f / 1e9));
            lines.push(format!(
                "  {} -> {} ({}): {} in {:.3}-{:.3} GHz; sweep peak {}",
                p.from,
                p.to,
                p.to_net,
                fmt(band),
                lo / 1e9,
                hi / 1e9,
                fmt(sweep)
            ));
        }
        for (text, _) in &self.findings {
            lines.extend(text.iter().cloned());
        }
        for note in &self.notes {
            lines.push(format!("  note: {note}"));
        }
        lines.push("  evidence: screening (full-wave FDTD; accuracy in docs/coupling-query-plan.md)".to_owned());
        lines
    }

    fn json(&self) -> String {
        let mut o = Object::new();
        put(&mut o, "schema", Value::from(SCHEMA));
        put(&mut o, "board", Value::from(self.board.as_str()));
        put(&mut o, "mode", Value::from(self.mode));
        put(&mut o, "from", Value::from(self.from.as_str()));
        put(&mut o, "victims", Value::Array(self.victims.iter().map(|v| Value::from(v.as_str())).collect()));
        put(&mut o, "band_hz", Value::Array(vec![Value::from(self.band.0), Value::from(self.band.1)]));
        put(&mut o, "frequencies_hz", Value::Array(self.freqs.iter().map(|f| Value::from(*f)).collect()));
        let paths = self
            .paths
            .iter()
            .map(|p| {
                let mut q = Object::new();
                put(&mut q, "from", Value::from(p.from.as_str()));
                put(&mut q, "to", Value::from(p.to.as_str()));
                put(&mut q, "to_net", Value::from(p.to_net.as_str()));
                let mut t = Object::new();
                put(&mut t, "ohms", Value::from(p.termination.ohms));
                put(&mut t, "source", Value::from(p.termination.source.as_str()));
                put(&mut q, "termination", Value::Object(t));
                put(&mut q, "s_db", Value::Array(p.s.iter().map(|s| Value::from(round(db(s), 3))).collect()));
                let peak = |range: (f64, f64)| {
                    peak_in(&self.freqs, &p.s, range.0, range.1).map_or(Value::Null, |(f, d)| {
                        let mut v = Object::new();
                        put(&mut v, "db", Value::from(round(d, 3)));
                        put(&mut v, "frequency_hz", Value::from(f));
                        Value::Object(v)
                    })
                };
                put(&mut q, "peak_in_band", peak(self.band));
                put(&mut q, "peak", peak((f64::NEG_INFINITY, f64::INFINITY)));
                Value::Object(q)
            })
            .collect();
        put(&mut o, "paths", Value::Array(paths));
        let runs = self
            .runs
            .iter()
            .map(|r| {
                let mut q = Object::new();
                put(&mut q, "cells", Value::from(r.cells as i64));
                put(&mut q, "steps", Value::from(r.steps as i64));
                put(&mut q, "seconds", Value::from(round(r.seconds, 1)));
                put(&mut q, "converged", Value::Bool(r.converged));
                Value::Object(q)
            })
            .collect();
        put(&mut o, "runs", Value::Array(runs));
        put(&mut o, "findings", Value::Array(self.findings.iter().map(|(_, v)| v.clone()).collect()));
        put(&mut o, "notes", Value::Array(self.notes.iter().map(|n| Value::from(n.as_str())).collect()));
        put(&mut o, "evidence_grade", Value::from("screening"));
        Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(o)) + "\n"
    }
}

/// The design without antenna declarations: the board antenna sites are resolved on.
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
        let _ = writeln!(err, "{USAGE}\ncopper coupling: error: {message}");
        2
    };
    let specs = [
        value("--from"),
        value("--antenna"),
        value("--to"),
        value("--run"),
        value("--band"),
        value("--ground"),
        value("--terminate"),
        value("--excite"),
        value("--receiver"),
        value("--stitch"),
        value("--fidelity"),
        value("--budget"),
        value("--margin"),
        value("--threads"),
        flag("--describe"),
        flag("--json"),
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
        let _ = writeln!(out, "{USAGE}\n\n{SUMMARY}");
        crate::help::options(out, "coupling");
        return 0;
    }
    let board = match parsed.positionals.as_slice() {
        [board] => board.clone(),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let (from, antenna) = (parsed.value("--from"), parsed.value("--antenna"));
    if from.is_some() == antenna.is_some() {
        return usage_error(err, "exactly one of --from NET and --antenna NAME is required");
    }
    let victims = parsed.owned_values("--to");
    if victims.is_empty() {
        return usage_error(err, "the following arguments are required: --to");
    }
    if let Some(from) = from
        && victims.iter().any(|v| v == from)
    {
        return usage_error(err, &format!("argument --to: {from} is the aggressor"));
    }
    for region_only in ["--band", "--budget", "--margin", "--ground", "--excite", "--receiver", "--stitch"] {
        if antenna.is_some() && !parsed.values(region_only).is_empty() {
            return usage_error(
                err,
                &format!(
                    "argument {region_only}: applies to --from NET (an antenna uses its declared bands and site model)"
                ),
            );
        }
    }
    let band = match parsed.value("--band") {
        None => (100e6, 3e9),
        Some(text) => match text.split_once(':').and_then(|(a, b)| Some((parse_hz(a)?, parse_hz(b)?))) {
            Some((lo, hi)) if hi > lo => (lo, hi),
            _ => {
                return usage_error(
                    err,
                    &format!("argument --band: expected LOW:HIGH such as 100MHz:3GHz, got '{text}'"),
                );
            }
        },
    };
    let fine = match parsed.value("--fidelity").unwrap_or("coarse") {
        "coarse" => false,
        "fine" => true,
        other => {
            return usage_error(
                err,
                &format!("argument --fidelity: invalid choice: '{other}' (choose from coarse, fine)"),
            );
        }
    };
    let number = |name: &str| -> Result<Option<f64>, String> {
        parsed
            .value(name)
            .map(|t| {
                t.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or(format!("argument {name}: invalid value: '{t}'"))
            })
            .transpose()
    };
    let (budget, margin, threads) = match (number("--budget"), number("--margin"), number("--threads")) {
        (Ok(b), Ok(m), Ok(t)) => (b, m, t.map_or(0, |t| t as usize)),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return usage_error(err, &e),
    };
    let mut terminations = Terminations::default();
    for item in parsed.values("--terminate") {
        match item.rsplit_once('=').and_then(|(end, ohms)| Some((end.trim(), ohms.trim().parse::<f64>().ok()?))) {
            Some((end, ohms)) if ohms > 0.0 && ohms.is_finite() && !end.is_empty() => {
                terminations.overrides.insert(end.to_owned(), ohms);
            }
            _ => {
                return usage_error(
                    err,
                    &format!("argument --terminate: expected END=OHMS such as U2.4=1000, got '{item}'"),
                );
            }
        }
    }
    let layers = match parsed.value("--layers").map(str::trim) {
        None => None,
        Some(text) => match text.parse::<i64>() {
            Ok(value) if [2, 4, 6].contains(&value) => Some(value),
            _ => {
                return usage_error(err, &format!("argument --layers: invalid choice: '{text}' (choose from 2, 4, 6)"));
            }
        },
    };
    let profile = parsed.value("--fab-profile").unwrap_or("generic");
    if !["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"].contains(&profile) {
        return usage_error(err, &format!("argument --fab-profile: invalid choice: '{profile}'"));
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
    let fail = |out: &mut dyn Write, message: &str| {
        let _ = text::print(out, &format!("COUPLING ERROR: {message}"));
        2
    };
    let routed = match parsed.value("--run") {
        Some(run) => match crate::emi_check::routed_board(&pypath::display(run)) {
            Ok(board) => Some(board),
            Err(message) => return fail(out, &message),
        },
        None => None,
    };
    let physicalized = |design: &Design| {
        let options =
            copper_pcb::configuration::physical_options(design, (100.0, 80.0), layers, profile, locked, offline)?;
        if parsed.flag("--allow-proxy-footprints") {
            physicalize(design, Footprints::Proxy, &options, locked, offline)
        } else {
            let base = paths::resolve(Path::new(&board)).parent().map(Path::to_path_buf).unwrap_or_default();
            let roots: Vec<PathBuf> =
                parsed.values("--footprint-root").iter().map(|root| PathBuf::from(pypath::display(root))).collect();
            let mut resolver = FootprintResolver::new(&base, &roots, false, locked, offline);
            physicalize(design, Footprints::Resolved(&mut resolver), &options, locked, offline)
        }
    };
    let view = match copper_lang::elaborate::elaborate(&design.electrical) {
        Ok(view) => view,
        Err(message) => return fail(out, &message),
    };
    let emi = copper_lang::emi::analyze(&view);
    let declared = Terminations::from_emi(&emi);
    terminations.drivers = declared.drivers;
    let mut notes = Vec::new();
    if routed.is_none() {
        notes.push("no --run: only pads and the design's own copper are modelled, no routed tracks".to_owned());
    }

    let report = if let Some(aggressor) = from {
        let physical = match routed {
            Some(board) => board,
            None => match physicalized(&design) {
                Ok(board) => board,
                Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
                Err(Error::Value(message)) => return fail(out, &message),
            },
        };
        for net in std::iter::once(aggressor).chain(victims.iter().map(String::as_str)) {
            if !physical.nets.iter().any(|n| n.name == net) && !physical.tracks.iter().any(|t| t.net == net) {
                return fail(out, &format!("net {net} is not on the board"));
            }
        }
        let mut request = CouplingRequest::new(aggressor, &victims);
        // Diagnostic: `COPPER_COUPLING_NO_RING_FIT` runs every record to decay or the time cap instead of fitting.
        request.ring_fit = std::env::var_os("COPPER_COUPLING_NO_RING_FIT").is_none();
        request.band_hz = band;
        request.fidelity = if fine { Fidelity::Fine } else { Fidelity::Coarse };
        request.threads = threads;
        if let Some(seconds) = budget {
            request.budget = Budget { max_seconds: seconds, ..Budget::default() };
        }
        if let Some(text) = parsed.value("--stitch") {
            match text.trim().parse::<f64>() {
                Ok(mm) if mm >= 0.5 && mm.is_finite() => request.stitch_pitch_nm = Some((mm * 1e6).round() as i64),
                _ => {
                    return usage_error(
                        err,
                        &format!("argument --stitch: expected a pitch of at least 0.5 (mm), got '{text}'"),
                    );
                }
            }
        }
        if let Some(mm) = margin {
            request.margin_nm = (mm * 1e6).round() as i64;
        }
        let grounds = parsed.owned_values("--ground");
        if !grounds.is_empty() {
            request.grounds = Some(grounds.into_iter().collect());
        }
        let model = match coupling_model(&physical, &request, &|end: &NetEnd| terminations.choose(end)) {
            Ok(model) => model,
            Err(e) => return fail(out, &e.to_string()),
        };
        let ends: Vec<NetEnd> = model.ports.iter().map(|p| p.end.clone()).collect();
        let mut model = model;
        let excite = parsed.owned_values("--excite");
        if !excite.is_empty() {
            let aggressor_ends: Vec<&str> = model.aggressor_ports.iter().map(|&k| ends[k].name.as_str()).collect();
            if let Some(bad) = excite.iter().find(|e| !aggressor_ends.contains(&e.as_str())) {
                return fail(
                    out,
                    &format!(
                        "--excite names no end of {aggressor}: {bad} (its ends are {})",
                        aggressor_ends.join(", ")
                    ),
                );
            }
            model.aggressor_ports.retain(|&k| excite.contains(&ends[k].name));
        }
        // An antenna whose feed net is a victim: the desense check from the aggressor's driver to the radio's pad.
        let has_antenna = design.electrical.constraints.iter().any(|c| c.kind == ConstraintKind::Antenna);
        let mut receivers: Vec<(copper_pcb::antenna::AntennaSite, Result<String, String>)> = Vec::new();
        if has_antenna {
            let sites = physicalized(&without_antennas(&design))
                .and_then(|bare| copper_pcb::antenna::resolve_sites(&bare, &view));
            match sites {
                Ok(sites) => {
                    for site in sites.into_iter().filter(|s| victims.contains(&s.feed_net)) {
                        let feed = format!("{}.{}", site.feed.component, site.feed.pad);
                        let candidates: Vec<String> = ends
                            .iter()
                            .filter(|e| e.net == site.feed_net && e.name != feed)
                            .map(|e| e.name.clone())
                            .collect();
                        let receiver = match (parsed.value("--receiver"), candidates.as_slice()) {
                            (Some(r), _) if candidates.iter().any(|c| c == r) => Ok(r.to_owned()),
                            (Some(r), _) => {
                                return fail(
                                    out,
                                    &format!(
                                        "--receiver {r} is no end of {} other than the feed land {feed} (they are {})",
                                        site.feed_net,
                                        candidates.join(", ")
                                    ),
                                );
                            }
                            (None, [one]) => Ok(one.clone()),
                            (None, []) => Err(format!("{} has no end but the feed land {feed}", site.feed_net)),
                            (None, _) => Err(format!(
                                "{} has several ends besides the feed land ({}): name the radio's with --receiver",
                                site.feed_net,
                                candidates.join(", ")
                            )),
                        };
                        receivers.push((site, receiver));
                    }
                }
                Err(e) => notes.push(format!("antenna sites not resolved, no desense check: {e:?}")),
            }
        }
        let unknown = terminations.unknown_overrides(&ends);
        if !unknown.is_empty() {
            let names: Vec<&str> = ends.iter().map(|e| e.name.as_str()).collect();
            return fail(
                out,
                &format!("--terminate names no end: {} (the ends are {})", unknown.join(", "), names.join(", ")),
            );
        }
        if parsed.flag("--describe") {
            let _ = text::print(out, &format!("COUPLING: region model from {aggressor} to {}", victims.join(", ")));
            for line in model.model.describe().lines() {
                let _ = text::print(out, &format!("  {line}"));
            }
            // Copper by net and layer (sheet area, mm^2), and how much of it lies in each antenna region.
            let mut areas: std::collections::BTreeMap<(String, i64), f64> = std::collections::BTreeMap::new();
            for sheet in &model.model.sheets {
                *areas.entry((sheet.net.clone(), (sheet.z * 1e6).round() as i64)).or_default() +=
                    copper_fullwave::geom::area(&sheet.ring);
            }
            for ((net, z), area) in &areas {
                let _ = text::print(out, &format!("  copper {net} at z {:.3} mm: {area:.1} mm^2", *z as f64 / 1e6));
            }
            for p in &model.ports {
                let _ = text::print(
                    out,
                    &format!(
                        "  end {} ({}): {} ohm, {}; returns to {}",
                        p.end.name,
                        p.end.net,
                        p.termination.ohms,
                        p.termination.source,
                        p.plane.value()
                    ),
                );
            }
            for note in &model.notes {
                let _ = text::print(out, &format!("  note: {note}"));
            }
            return 0;
        }
        let result = match model.solve() {
            Ok(result) => result,
            Err(e) => return fail(out, &e.to_string()),
        };
        let termination_of = |name: &str| {
            model
                .ports
                .iter()
                .find(|p| p.end.name == name)
                .map(|p| p.termination.clone())
                .unwrap_or_else(Termination::default_ohms)
        };
        let violations = result.passivity_violations(0.02);
        if let Some((end, f, sum)) = violations.iter().max_by(|a, b| a.2.total_cmp(&b.2)) {
            notes.push(format!(
                "NOT PASSIVE: exciting {end} sends out {sum:.2} times the incident power at {:.3} GHz ({} frequencies \
                 above 1.02): a numerical fault, the couplings of that run are not evidence",
                f / 1e9,
                violations.len()
            ));
        }
        notes.extend(model.notes.iter().cloned());
        notes.push(format!("ground nets: {}", model.grounds.iter().cloned().collect::<Vec<_>>().join(", ")));
        let findings = receivers
            .iter()
            .map(|(site, receiver)| {
                let bands: Vec<(f64, f64)> = site
                    .bands
                    .iter()
                    .map(|b| {
                        (copper_pcb::antenna::decimal_f64(&b.low_hz), copper_pcb::antenna::decimal_f64(&b.high_hz))
                    })
                    .collect();
                let site_band = (
                    bands.iter().map(|b| b.0).fold(f64::INFINITY, f64::min),
                    bands.iter().map(|b| b.1).fold(f64::NEG_INFINITY, f64::max),
                );
                let find = |driver: &str| {
                    let receiver = receiver.clone()?;
                    result
                        .paths
                        .iter()
                        .find(|p| p.from.starts_with(&format!("{driver}.")) && p.to == receiver)
                        .map(|p| DesensePath {
                            driver_end: p.from.clone(),
                            termination: termination_of(&p.from),
                            receiver_end: p.to.clone(),
                            s_db: p.db(),
                        })
                        .ok_or_else(|| {
                            format!("no solved path from the driver {driver} to {receiver} (was its end excited?)")
                        })
                };
                desense(aggressor, &emi, site, site_band, &result.freqs, &find)
            })
            .collect();
        Report {
            mode: "region",
            board: board.clone(),
            from: aggressor.to_owned(),
            victims: victims.clone(),
            band,
            freqs: result.freqs.clone(),
            paths: result
                .paths
                .iter()
                .map(|p| PathOut {
                    from: p.from.clone(),
                    to: p.to.clone(),
                    to_net: p.to_net.clone(),
                    termination: termination_of(&p.to),
                    s: p.s.clone(),
                })
                .collect(),
            runs: result
                .runs
                .iter()
                .map(|r| RunOut { cells: r.cells, steps: r.steps, seconds: r.seconds, converged: r.converged })
                .collect(),
            notes,
            findings,
        }
    } else {
        let name = antenna.expect("one of --from and --antenna");
        let bare = match physicalized(&without_antennas(&design)) {
            Ok(board) => board,
            Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
            Err(Error::Value(message)) => return fail(out, &message),
        };
        let sites = match copper_pcb::antenna::resolve_sites(&bare, &view) {
            Ok(sites) => sites,
            Err(Error::NotImplemented(what)) => return not_implemented(err, &what),
            Err(Error::Value(message)) => return fail(out, &message),
        };
        let Some(site) = sites.iter().find(|s| s.name == name) else {
            let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
            return usage_error(err, &format!("unknown antenna '{name}'; choose from {}", names.join(", ")));
        };
        let problem = match Problem::new(&bare, site) {
            Ok(problem) => problem,
            Err(message) => return fail(out, &message),
        };
        let geometry = match copper_pcb::antenna::chosen_geometry(&bare, site) {
            Ok((geometry, _)) => geometry,
            Err(e) => return fail(out, &e.to_string()),
        };
        let layout = match copper_pcb::antenna::layout(site, &geometry) {
            Ok(layout) => layout,
            Err(e) => return fail(out, &e.to_string()),
        };
        let victim_board = routed.as_ref().unwrap_or(&bare);
        let mut built = Vec::new();
        for net in &victims {
            if *net == site.feed_net || *net == site.ground {
                return usage_error(err, &format!("argument --to: {net} is the antenna's feed or ground net"));
            }
            match Victim::from_board(&problem, victim_board, net, &|end: &NetEnd| terminations.choose(end)) {
                Ok(victim) => built.push(victim),
                Err(message) => return fail(out, &message),
            }
        }
        let ends: Vec<NetEnd> = built.iter().flat_map(|v| v.geometry.ports.iter().map(|p| p.end.clone())).collect();
        let unknown = terminations.unknown_overrides(&ends);
        if !unknown.is_empty() {
            let names: Vec<&str> = ends.iter().map(|e| e.name.as_str()).collect();
            return fail(
                out,
                &format!("--terminate names no end: {} (the ends are {})", unknown.join(", "), names.join(", ")),
            );
        }
        let fidelity = if fine { AntennaFidelity::fine(site, threads) } else { AntennaFidelity::coarse(site, threads) };
        let feed = format!("antenna {name} feed ({})", problem.port_description());
        if parsed.flag("--describe") {
            let _ = text::print(out, &format!("COUPLING: antenna site model from {feed} to {}", victims.join(", ")));
            let _ = text::print(
                out,
                &format!("  geometry {}, {} fidelity (cell {} mm)", geometry.to_source(), fidelity.name, fidelity.cell),
            );
            for v in &built {
                for p in &v.geometry.ports {
                    let _ = text::print(
                        out,
                        &format!(
                            "  end {} ({}): {} ohm, {}; returns to {}",
                            p.end.name,
                            p.end.net,
                            p.termination.ohms,
                            p.termination.source,
                            p.plane.value()
                        ),
                    );
                }
                let _ = text::print(
                    out,
                    &format!(
                        "  victim {}: {} sheets, {} vias; ground cut back {:.3} mm around it",
                        v.net(),
                        v.geometry.sheets.len(),
                        v.geometry.wires.len(),
                        v.clearance
                    ),
                );
            }
            return 0;
        }
        let (evaluation, couplings) = match evaluate_coupled(&problem, &layout, &fidelity, &built) {
            Ok(result) => result,
            Err(message) => return fail(out, &message),
        };
        let bands: Vec<(f64, f64)> = site
            .bands
            .iter()
            .map(|b| (copper_pcb::antenna::decimal_f64(&b.low_hz), copper_pcb::antenna::decimal_f64(&b.high_hz)))
            .collect();
        let band = (
            bands.iter().map(|b| b.0).fold(f64::INFINITY, f64::min),
            bands.iter().map(|b| b.1).fold(f64::NEG_INFINITY, f64::max),
        );
        notes.push(format!(
            "the site model holds the radiator fed at its land, not the line from the radio to the land: for the \
             coupling to that line, ask --from {} --to {}",
            site.feed_net,
            victims.join(" --to ")
        ));
        notes.push(format!(
            "coupling is referenced to the wave incident at the 50 ohm feed; the bare antenna's worst S11 in band is {:.1} dB",
            evaluation
                .freqs
                .iter()
                .zip(&evaluation.s11_db)
                .filter(|(f, _)| **f >= band.0 && **f <= band.1)
                .map(|(_, s)| *s)
                .fold(f64::NEG_INFINITY, f64::max)
        ));
        let findings = victims
            .iter()
            .map(|net| {
                let find = |driver: &str| {
                    couplings
                        .iter()
                        .find(|c| c.net == *net && c.port.starts_with(&format!("{driver}.")))
                        .map(|c| DesensePath {
                            driver_end: c.port.clone(),
                            termination: c.termination.clone(),
                            receiver_end: "the feed".to_owned(),
                            s_db: c.s_db.clone(),
                        })
                        .ok_or_else(|| "no end of the net is a pad of its declared driver".to_owned())
                };
                desense(net, &emi, site, band, &evaluation.freqs, &find)
            })
            .collect();
        Report {
            mode: "antenna",
            board: board.clone(),
            from: format!("antenna {name}"),
            victims: victims.clone(),
            band,
            freqs: evaluation.freqs.clone(),
            paths: couplings
                .iter()
                .map(|c| PathOut {
                    from: format!("antenna {name}"),
                    to: c.port.clone(),
                    to_net: c.net.clone(),
                    termination: c.termination.clone(),
                    s: c.s.clone(),
                })
                .collect(),
            runs: vec![RunOut {
                cells: evaluation.cells,
                steps: evaluation.steps,
                seconds: evaluation.seconds,
                converged: evaluation.converged,
            }],
            notes,
            findings,
        }
    };
    if parsed.flag("--json") {
        let _ = out.write_all(report.json().as_bytes());
    } else {
        for line in report.lines() {
            let _ = text::print(out, &line);
        }
    }
    0
}

fn si(q: &copper_units::Quantity) -> f64 {
    q.base_value().decimal_text().parse().unwrap_or(f64::NAN)
}

/// `EMI-RF-DESENSE` for victim `net`: the spurs its declared signal puts into the antenna's band through the
/// coupling from its driver's end, held to the antenna's declared `sensitivity_dbm` and `desense_margin_db`.
/// The path of a desense finding: the signal's driver end, its termination, the receiver's end and `|S|` in dB
/// from one to the other at each analysis frequency.
struct DesensePath {
    driver_end: String,
    termination: Termination,
    receiver_end: String,
    s_db: Vec<f64>,
}

fn desense(
    net: &str,
    emi: &copper_lang::emi::EmiModel,
    site: &copper_pcb::antenna::AntennaSite,
    band: (f64, f64),
    freqs: &[f64],
    find_path: &dyn Fn(&str) -> Result<DesensePath, String>,
) -> (Vec<String>, Value) {
    use copper_emi::desense::{Source, received_spurs, verdict};
    use copper_emi::signal::{Line, clock_lines, continuous_bound};
    let mut o = Object::new();
    put(&mut o, "rule", Value::from("EMI-RF-DESENSE"));
    put(&mut o, "net", Value::from(net));
    let mut inputs: Vec<String> = Vec::new();
    let finish = |status: &str, text: String, mut o: Object, inputs: Vec<String>| {
        put(&mut o, "status", Value::from(status));
        put(&mut o, "inputs", Value::Array(inputs.iter().map(|i| Value::from(i.as_str())).collect()));
        (vec![format!("  EMI-RF-DESENSE {net}: {text}")], Value::Object(o))
    };
    let source = emi
        .signals
        .values()
        .find(|s| s.net == net || s.partner.as_deref() == Some(net) || s.members.iter().any(|m| m.net == net));
    let Some(source) = source else {
        return finish("not_applicable", "not a declared fast signal (no spectrum to couple)".to_owned(), o, inputs);
    };
    // The driver's end: a pad of the component whose pin declares the signal.
    let Some(driver) = source.pins.first().map(|p| p.component.clone()) else {
        return finish("not_applicable", "the signal names no driving pin".to_owned(), o, inputs);
    };
    let path = match find_path(&driver) {
        Ok(path) => path,
        Err(why) => return finish("not_applicable", why, o, inputs),
    };
    put(&mut o, "driver_end", Value::from(path.driver_end.as_str()));
    put(&mut o, "receiver_end", Value::from(path.receiver_end.as_str()));
    // The sweep must cover the band: a line with no coupling sample would otherwise drop out and read as a pass.
    let (lo, hi) = (freqs.first().copied().unwrap_or(f64::NAN), freqs.last().copied().unwrap_or(f64::NAN));
    if !(lo <= band.0 && hi >= band.1) {
        return finish(
            "not_applicable",
            format!(
                "the sweep ({:.3}-{:.3} GHz) does not cover the antenna's band ({:.3}-{:.3} GHz)",
                lo / 1e9,
                hi / 1e9,
                band.0 / 1e9,
                band.1 / 1e9
            ),
            o,
            inputs,
        );
    }
    let ohms = path.termination.ohms;
    let (swing, kind) = match (&source.open_circuit_swing, &source.swing) {
        (Some(v), _) => (si(&v.value), Source::Thevenin),
        (None, Some(v)) => (si(&v.value), Source::Loaded),
        (None, None) => {
            return finish(
                "not_applicable",
                "the driver declares no signal_swing or open_circuit_swing".to_owned(),
                o,
                inputs,
            );
        }
    };
    let rise = source.rise_time.as_ref().map(|n| si(&n.value));
    let fall = source.fall_time.as_ref().map(|n| si(&n.value)).or(rise);
    let (Some(rise), Some(fall)) = (rise, fall) else {
        return finish("not_applicable", "the driver declares no rise_time or fall_time".to_owned(), o, inputs);
    };
    inputs.push(format!(
        "source: {} {swing} V, {ohms} ohm ({}), edges {:.3} / {:.3} ns",
        if kind == Source::Thevenin { "open-circuit swing" } else { "loaded swing" },
        path.termination.source,
        rise * 1e9,
        fall * 1e9
    ));
    let (lines, spectrum): (Vec<Line>, &str) = if let Some(f) = &source.clock_frequency {
        let f = si(&f.value);
        inputs.push(format!("spectrum: clock lines of {:.3} MHz, 50 % duty", f / 1e6));
        (clock_lines(swing, f, 0.5, rise, fall, band.1), "clock_lines")
    } else if source.data_rate.is_some() {
        inputs.push(
            "spectrum: the continuous bound on any line of any two-level waveform of this swing and edge (an upper bound, loose by construction)"
                .to_owned(),
        );
        let edge = rise.min(fall);
        let bound = freqs
            .iter()
            .filter(|f| **f >= band.0 && **f <= band.1)
            .map(|f| Line { harmonic: 0, frequency: *f, amplitude: continuous_bound(swing, edge, *f) })
            .collect();
        (bound, "continuous_bound")
    } else {
        return finish("not_applicable", "the driver declares no clock_frequency or data_rate".to_owned(), o, inputs);
    };
    put(&mut o, "spectrum", Value::from(spectrum));
    let spurs = received_spurs(&lines, ohms, kind, band, freqs, &path.s_db);
    inputs.push(format!("path: {} to {}", path.driver_end, path.receiver_end));
    put(
        &mut o,
        "spurs",
        Value::Array(
            spurs
                .iter()
                .map(|s| {
                    let mut q = Object::new();
                    put(&mut q, "harmonic", Value::from(i64::from(s.harmonic)));
                    put(&mut q, "frequency_hz", Value::from(s.frequency));
                    put(&mut q, "source_dbm", Value::from(round(s.source_dbm, 2)));
                    put(&mut q, "coupling_db", Value::from(round(s.coupling_db, 2)));
                    put(&mut q, "received_dbm", Value::from(round(s.received_dbm, 2)));
                    Value::Object(q)
                })
                .collect(),
        ),
    );
    if spurs.is_empty() {
        return finish(
            "pass",
            format!("no line of the signal falls in {:.3}-{:.3} GHz", band.0 / 1e9, band.1 / 1e9),
            o,
            inputs,
        );
    }
    let worst = spurs.iter().copied().max_by(|a, b| a.received_dbm.total_cmp(&b.received_dbm)).expect("a spur");
    let describe = format!(
        "worst spur {} at {:.3} MHz: {:.1} dBm from the driver, {:.1} dB coupling, {:.1} dBm at {}",
        if worst.harmonic > 0 { format!("harmonic {}", worst.harmonic) } else { "bound".to_owned() },
        worst.frequency / 1e6,
        worst.source_dbm,
        worst.coupling_db,
        worst.received_dbm,
        path.receiver_end
    );
    let Some(sensitivity) = site.sensitivity_dbm else {
        return finish(
            "no_verdict",
            format!("{describe}; no verdict: the antenna declares no sensitivity_dbm"),
            o,
            inputs,
        );
    };
    let (_, margin) = verdict(&spurs, sensitivity, site.desense_margin_db).expect("a spur");
    let limit = sensitivity - site.desense_margin_db;
    put(&mut o, "limit_dbm", Value::from(round(limit, 2)));
    put(&mut o, "margin_db", Value::from(round(margin, 2)));
    let status = if margin >= 0.0 { "pass" } else { "fail" };
    finish(
        status,
        format!(
            "{describe}; limit {limit:.1} dBm (sensitivity {sensitivity:.1} dBm, margin {:.1} dB): {status} by {:.1} dB",
            site.desense_margin_db,
            margin.abs()
        ),
        o,
        inputs,
    )
}

#[cfg(test)]
mod tests {
    use super::parse_hz;

    #[test]
    fn frequencies_parse_with_and_without_units() {
        assert_eq!(parse_hz("100MHz"), Some(100e6));
        assert_eq!(parse_hz("2.4 GHz"), Some(2.4e9));
        assert_eq!(parse_hz("150kHz"), Some(150e3));
        assert_eq!(parse_hz("1e9"), Some(1e9));
        assert_eq!(parse_hz("-1GHz"), None);
        assert_eq!(parse_hz("fast"), None);
    }
}

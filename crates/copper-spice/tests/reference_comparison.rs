//! Reference comparison (plan phase I1 of `docs/simulation-improvement-plan.md`):
//! every deck in `tests/reference/decks/` run through the native solver and
//! compared with the ngspice results committed in `tests/reference/results/`.
//!
//! The test never runs ngspice; `tests/reference/generate.py` made the CSV
//! files once (see `tests/reference/README.md`). A deck is an ordinary
//! ngspice netlist: its elements are read here as the body of a subcircuit
//! `DECK` with no ports, instantiated as `X` (nodes `X#NODE`, elements
//! `X.NAME`), and its analysis cards are run with the solver's own analyses
//! on the reference's sweep points.
//!
//! # Tolerances
//!
//! Both solvers converge their Newton iterations to a relative 1e-6 of each
//! node voltage plus 1e-9 V (copper-sim's fixed `RELTOL` and `VNTOL`, the
//! decks' `.options reltol=1e-6 vntol=1e-9` for ngspice), so two
//! implementations of the same equations agree to a few 1e-6 at a bias
//! point. One systematic difference sits below that: ngspice-46 uses the
//! CODATA 2014 `k` and `q` (its `const.boltz` is 1.38064852e-23), copper-sim
//! the exact 2019 values, so `V_T` differs by 3.4e-7 of itself and a junction
//! current at a forced voltage `V` by `(V / n V_T) 3.4e-7`: 1e-5 at 0.75 V
//! and 27 degC, 1.7e-5 at 1 V and -40 degC. Each kind of quantity gets a
//! bound derived from these, and a difference beyond it is a finding: fixed in
//! the reader or solver, or listed in [`KNOWN`] with its cause (README).
//!
//! | Quantity | Bound | Why |
//! | --- | --- | --- |
//! | DC voltage (`.op`, `.dc`) | 1e-5 of the value (of `|V(a)| + |V(b)|` for a difference `v(a,b)`) + 1 uV | ten times the Newton tolerance of either solver on each node, and the absolute part for nodes near 0 V at the 1e-6 V level of a converged step |
//! | DC current | 3e-5 of the value + 1e-12 A (or 1e-5 of the largest current of the series when that is smaller) + 1e-7 of the larger neighbouring point's current | the `V_T` difference above at up to 1.2 V and -40 degC; 1e-12 A is the G_MIN leak of one junction at 1 V, which sets how well a tiny current is resolved; the last part is `VNTOL` on an internal node (1e-9 V) through a junction conductance `I / V_T` (4e-8 of `I`), for a current that is the small difference of larger ones (a transistor's collector current at `V_CE = 0`) |
//! | AC phasor | 2e-4 of the reference's magnitude + 1e-6 of the largest magnitude of the sweep, on the complex difference (so magnitude and phase together) | small-signal conductances are derivatives at the bias point: the bias differences above move `g_m = I / V_T` by up to 3e-5, and capacitances by less; 2e-4 leaves a factor of 7; the absolute part keeps deep notches from counting |
//! | Noise density | 2e-4 of the value + 1e-6 of the largest density of the sweep | densities are `|H|^2 S` with the same bias dependence as AC (in amplitude, `sqrt` halves it); the generators' constants differ by below 1e-6 |
//! | Transient | 2e-3 of the waveform's range + 1 uV, against copper's waveform within one reference output step of the reference time (a tube), copper at `reltol` 1e-5 | both are second-order integrators with their own step control; the integration floor rule of `copper_sim::spectrum` gives `0.02 sqrt(reltol)` = 6e-5 at 1e-5, and the reference (ngspice at `reltol` 1e-6) a third of that; edges placed by either step control may sit a step apart, hence the window; the reference's own integration error depends on its output step, and the `jfet` and `opamp_follower` steps were refined until a finer one no longer moved it by the bound (README) |

use copper_sim::{Circuit, Complex, InitialConditions, NoiseSpec, Sweep, TranOptions};
use copper_spice::{Dialect, Library};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The instance name of a deck in the circuit.
const INSTANCE: &str = "X";
/// The parameter a swept source's value is replaced by.
const SWEEP_PARAM: &str = "SWEEPVAL";
/// copper-sim's transient tolerance in the comparison (see the module docs).
const TRAN_RELTOL: f64 = 1e-5;

/// A difference listed in the README with its cause: deck, analysis kind and
/// quantity (at every temperature of the deck), and the largest error in
/// units of the bound it may show, about a tenth above what it shows now, so
/// that a growing difference fails and a fixed one is noticed in the table
/// (`known` with a small worst value). The cause names the README section.
struct Known {
    deck: &'static str,
    analysis: &'static str,
    quantity: &'static str,
    at_most: f64,
    cause: &'static str,
}

// The README's sections on the causes. Those marked "established" there are
// shown by a test of their own below or by a variant deck.
const BREAKDOWN: &str = "breakdown onset";
const SUBSTRATE: &str = "substrate G_MIN";
const BASE_RESISTANCE: &str = "base resistance linearisation";
const REVERSE_CUBIC: &str = "reverse junction current";
const MEYER_CHARGE: &str = "MOSFET gate charge in transient";
const MEYER_VBS: &str = "MOSFET gate charge: body-effect transcapacitance";
const MEYER_ETA: &str = "MOSFET gate charge: ETA transcapacitance";
const MEYER_PARTITION: &str = "MOSFET gate charge: linear-region partition";
const BADMOS3: &str = "level 3 without VMAX";
const LEVEL3_DERIVATIVES: &str = "level-3 derivatives";

const KNOWN: &[Known] = &[
    Known { deck: "bjt_base_resistance", analysis: "dc", quantity: "i(vc1)", at_most: 3.3, cause: SUBSTRATE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vb1)", at_most: 310.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vc1)", at_most: 310.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vb2)", at_most: 360.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vc2)", at_most: 210.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vb3)", at_most: 440.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vc3)", at_most: 440.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vb4)", at_most: 1200.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_base_resistance", analysis: "ac", quantity: "i(vc4)", at_most: 410.0, cause: BASE_RESISTANCE },
    Known { deck: "bjt_npn", analysis: "dc", quantity: "i(vc1)", at_most: 3.3, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "op", quantity: "i(vs1)", at_most: 28000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "op", quantity: "i(vs2)", at_most: 28000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "op", quantity: "i(vs3)", at_most: 28000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "op", quantity: "i(vs4)", at_most: 28000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "dc", quantity: "i(vs1)", at_most: 27000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "dc", quantity: "i(vs2)", at_most: 27000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "dc", quantity: "i(vs3)", at_most: 27000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "dc", quantity: "i(vs4)", at_most: 27000.0, cause: SUBSTRATE },
    Known { deck: "bjt_substrate", analysis: "dc", quantity: "i(vc1)", at_most: 1.2, cause: SUBSTRATE },
    Known { deck: "cmos_inverter", analysis: "tran", quantity: "i(vdd)", at_most: 18.0, cause: MEYER_CHARGE },
    Known { deck: "cmos_inverter", analysis: "tran", quantity: "v(out)", at_most: 1.9, cause: MEYER_CHARGE },
    Known { deck: "diode", analysis: "dc", quantity: "i(vd)", at_most: 27.0, cause: BREAKDOWN },
    Known { deck: "jfet", analysis: "op", quantity: "i(vg5)", at_most: 1.3, cause: REVERSE_CUBIC },
    Known { deck: "jfet", analysis: "dc", quantity: "i(vg5)", at_most: 1.3, cause: REVERSE_CUBIC },
    Known { deck: "jfet", analysis: "dc", quantity: "i(vg1)", at_most: 3.4, cause: REVERSE_CUBIC },
    Known { deck: "mos1", analysis: "ac", quantity: "i(vg1)", at_most: 11.0, cause: MEYER_VBS },
    Known { deck: "mos1", analysis: "ac", quantity: "i(vg4)", at_most: 77.0, cause: MEYER_VBS },
    Known { deck: "mos1", analysis: "ac", quantity: "i(vd4)", at_most: 2.2, cause: MEYER_VBS },
    Known { deck: "mos3", analysis: "op", quantity: "i(vd7)", at_most: 1000.0, cause: BADMOS3 },
    Known { deck: "mos3", analysis: "dc", quantity: "i(vd7)", at_most: 3800.0, cause: BADMOS3 },
    Known { deck: "mos3", analysis: "ac", quantity: "i(vd3)", at_most: 500.0, cause: LEVEL3_DERIVATIVES },
    Known { deck: "mos3", analysis: "ac", quantity: "i(vg4)", at_most: 1500.0, cause: MEYER_ETA },
    Known { deck: "mos3", analysis: "ac", quantity: "i(vd4)", at_most: 160.0, cause: LEVEL3_DERIVATIVES },
    Known { deck: "mos3", analysis: "tran", quantity: "i(vd7)", at_most: 60.0, cause: BADMOS3 },
    Known { deck: "mos3_badmos3", analysis: "ac", quantity: "i(vd2)", at_most: 60.0, cause: MEYER_PARTITION },
    Known { deck: "mos3_derivatives", analysis: "ac", quantity: "i(vd1)", at_most: 410.0, cause: LEVEL3_DERIVATIVES },
    Known { deck: "mos3_derivatives", analysis: "ac", quantity: "i(vd2)", at_most: 150.0, cause: LEVEL3_DERIVATIVES },
    Known { deck: "temperature_tnom", analysis: "op", quantity: "i(vgj)", at_most: 1.5, cause: REVERSE_CUBIC },
    Known { deck: "temperature_tnom", analysis: "dc", quantity: "i(vgj)", at_most: 9.2, cause: REVERSE_CUBIC },
];

/// The reference directory (`REFERENCE_ROOT` names another with the same
/// layout, for trying a deck before it is committed).
fn root() -> PathBuf {
    match std::env::var_os("REFERENCE_ROOT") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("reference"),
    }
}

#[derive(Debug, Clone)]
enum Card {
    Op,
    Dc { source: String },
    Ac,
    Noise { output: (String, Option<String>), input: String },
    Tran { step: f64, start: f64, max: Option<f64>, uic: bool },
}

impl Card {
    fn kind(&self) -> &'static str {
        match self {
            Card::Op => "op",
            Card::Dc { .. } => "dc",
            Card::Ac => "ac",
            Card::Noise { .. } => "noise",
            Card::Tran { .. } => "tran",
        }
    }
}

struct Deck {
    name: String,
    options: Vec<String>,
    body: Vec<String>,
    cards: Vec<Card>,
    probes: Vec<String>,
    /// `*@ tran-probe`: the probes of a transient (`probes` without it).
    tran_probes: Vec<String>,
    /// `*@ tran-reltol`: copper-sim's transient tolerance for this deck.
    tran_reltol: f64,
    /// `*@ dialect pspice`: the reader's dialect (a vendor deck's model; spice3 without it).
    dialect: Dialect,
}

fn number(text: &str) -> f64 {
    copper_spice::lexer::number(&text.to_ascii_uppercase()).unwrap_or_else(|| panic!("not a number: {text}"))
}

/// `v(a)`, `v(a,b)`: the node names.
fn probe_nodes(probe: &str) -> (String, Option<String>) {
    let inner = &probe[2..probe.len() - 1];
    let mut parts = inner.split(',').map(|s| s.trim().to_string());
    let a = parts.next().unwrap();
    (a, parts.next())
}

fn read_deck(path: &Path) -> Deck {
    let text = std::fs::read_to_string(path).unwrap();
    let name = path.file_stem().unwrap().to_string_lossy().to_string();
    let mut lines = text.lines();
    lines.next(); // the title
    let (mut options, mut body, mut cards, mut probes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut tran_probes = Vec::new();
    let mut tran_reltol = TRAN_RELTOL;
    let mut dialect = Dialect::Spice3;
    let mut last_card = false;
    let mut joined: Vec<String> = Vec::new();
    for line in lines {
        let s = line.trim();
        if let Some(rest) = s.strip_prefix("*@") {
            let mut words = rest.split_whitespace();
            match words.next() {
                Some(key @ ("probe" | "tran-probe")) => {
                    let probes = if key == "probe" { &mut probes } else { &mut tran_probes };
                    let rest: String = words.collect::<Vec<_>>().join(" ").to_ascii_lowercase();
                    let mut depth = 0;
                    let mut current = String::new();
                    for ch in rest.chars() {
                        match ch {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            ' ' if depth == 0 => {
                                if !current.is_empty() {
                                    probes.push(std::mem::take(&mut current));
                                }
                                continue;
                            }
                            _ => {}
                        }
                        current.push(ch);
                    }
                    if !current.is_empty() {
                        probes.push(current);
                    }
                }
                Some("tran-reltol") => tran_reltol = number(words.next().unwrap()),
                Some("dialect") => {
                    let name = words.next().unwrap_or("");
                    dialect = Dialect::parse(name).unwrap_or_else(|| panic!("{}: dialect {name}", path.display()));
                }
                _ => {}
            }
            continue;
        }
        if let Some(cont) = s.strip_prefix('+')
            && last_card
        {
            joined.last_mut().unwrap().push(' ');
            joined.last_mut().unwrap().push_str(cont.trim());
            continue;
        }
        let low = s.to_ascii_lowercase();
        let first = low.split_whitespace().next().unwrap_or("");
        last_card = matches!(first, ".op" | ".dc" | ".ac" | ".noise" | ".tran");
        if last_card {
            joined.push(low.clone());
        } else if first.starts_with(".option") {
            options.push(line.to_string());
        } else if first != ".end" {
            body.push(line.to_string());
        }
    }
    for card in joined {
        let w: Vec<&str> = card.split_whitespace().collect();
        cards.push(match w[0] {
            ".op" => Card::Op,
            ".dc" => Card::Dc { source: w[1].to_string() },
            ".ac" => Card::Ac,
            ".noise" => {
                // `.noise v(out[,ref]) SRC ...`, with the node pair possibly split by spaces.
                let close = card.find(')').unwrap();
                let open = card.find("v(").unwrap();
                let output = probe_nodes(&card[open..=close]);
                let input = card[close + 1..].split_whitespace().next().unwrap().to_string();
                Card::Noise { output, input }
            }
            ".tran" => {
                let uic = w.contains(&"uic");
                let nums: Vec<f64> = w[1..].iter().filter(|x| **x != "uic").map(|x| number(x)).collect();
                Card::Tran { step: nums[0], start: nums.get(2).copied().unwrap_or(0.0), max: nums.get(3).copied(), uic }
            }
            other => unreachable!("{other}"),
        });
    }
    if tran_probes.is_empty() {
        tran_probes = probes.clone();
    }
    Deck { name, options, body, cards, probes, tran_probes, tran_reltol, dialect }
}

/// The library text of `deck`, with source `swept` (if any) set to the sweep parameter.
fn library_text(deck: &Deck, swept: Option<&str>) -> String {
    let mut text = String::new();
    for o in &deck.options {
        writeln!(text, "{o}").unwrap();
    }
    writeln!(text, ".SUBCKT DECK PARAMS: {SWEEP_PARAM}=0").unwrap();
    for line in &deck.body {
        let first = line.split_whitespace().next().unwrap_or("");
        match swept {
            Some(s) if first.eq_ignore_ascii_case(s) => {
                // `NAME a b DC value ...`: the DC value becomes the parameter.
                let words: Vec<&str> = line.split_whitespace().collect();
                let dc = words.iter().position(|w| w.eq_ignore_ascii_case("dc")).expect("a swept source names DC");
                let mut out: Vec<String> = words.iter().map(|w| w.to_string()).collect();
                out[dc + 1] = format!("{{{SWEEP_PARAM}}}");
                writeln!(text, "{}", out.join(" ")).unwrap();
            }
            _ => writeln!(text, "{line}").unwrap(),
        }
    }
    writeln!(text, ".ENDS DECK").unwrap();
    text
}

fn circuit(lib: &Library, celsius: f64, sweep: f64) -> Circuit {
    let mut c = Circuit::new();
    lib.instantiate_at(&mut c, INSTANCE, "DECK", &[], &[(SWEEP_PARAM.to_string(), sweep)], celsius)
        .unwrap_or_else(|e| panic!("{e}"));
    c
}

/// A deck's node in the circuit; ngspice's `xu.vas` (node `vas` of instance
/// `xu`) is `X.XU#VAS`.
fn node(name: &str) -> String {
    let name = name.to_ascii_uppercase();
    match name.rsplit_once('.') {
        Some((path, inner)) => format!("{INSTANCE}.{path}#{inner}"),
        None => format!("{INSTANCE}#{name}"),
    }
}

/// A deck's element in the circuit (`xu.v1` is `X.XU.V1`).
fn element(name: &str) -> String {
    format!("{INSTANCE}.{}", name.to_ascii_uppercase())
}

/// Probe `p` (`v(a)`, `v(a,b)`, `i(vx)`) of an AC result.
fn ac_probe(r: &copper_sim::AcResult, p: &str) -> Vec<Complex> {
    if p.starts_with("v(") {
        let (a, b) = probe_nodes(p);
        match b {
            Some(b) => r.voltage_between(&node(&a), &node(&b)).unwrap(),
            None => r.voltage(&node(&a)).unwrap().to_vec(),
        }
    } else {
        r.current(&element(&p[2..p.len() - 1])).unwrap()
    }
}

/// A reference series: `(x, value)` in file order.
type Series = Vec<(f64, Complex)>;

/// `(analysis index, temperature, quantity)` to its series.
fn read_reference(dir: &Path, deck: &str) -> BTreeMap<(usize, String, String), Series> {
    let path = dir.join("results").join(format!("{deck}.csv"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out: BTreeMap<(usize, String, String), Series> = BTreeMap::new();
    for line in text.lines().skip(2) {
        // `analysis,temp,quantity,x,re,im`; a quantity with a comma (`v(a,b)`) is quoted.
        let f: Vec<&str> = line.split(',').collect();
        let n = f.len();
        if n < 6 {
            continue;
        }
        let index: usize = f[0].split(':').next().unwrap().parse().unwrap();
        let quantity = f[2..n - 3].join(",").trim_matches('"').to_string();
        let parse = |s: &str| s.parse::<f64>().unwrap();
        out.entry((index, f[1].to_string(), quantity))
            .or_default()
            .push((parse(f[n - 3]), Complex::new(parse(f[n - 2]), parse(f[n - 1]))));
    }
    out
}

/// What a quantity is, for its bound.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    DcVoltage,
    DcCurrent,
    Ac,
    Noise,
}

/// One compared series: the largest error over the bound, and where.
struct Outcome {
    deck: String,
    /// The analysis card's index in the deck.
    card: usize,
    analysis: &'static str,
    temp: String,
    quantity: String,
    points: usize,
    worst: f64,
    at: f64,
    got: Complex,
    want: Complex,
}

/// Errors of `got` against `want` in units of the bound of `kind`; the worst
/// and its index. `scale` is, per point, the magnitude a DC voltage
/// difference `v(a,b)` is the difference of (`|V(a)| + |V(b)|`): each node
/// carries the Newton tolerance of its own voltage.
fn worst(kind: Kind, want: &[Complex], got: &[Complex], scale: Option<&[f64]>) -> (f64, usize) {
    let largest = want.iter().map(|z| z.abs()).fold(0.0, f64::max);
    let neighbour = |k: usize| {
        let before = if k > 0 { want[k - 1].abs() } else { 0.0 };
        before.max(want.get(k + 1).map_or(0.0, |z| z.abs()))
    };
    let mut out = (0.0, 0);
    for (k, (w, g)) in want.iter().zip(got).enumerate() {
        let bound = match kind {
            Kind::DcVoltage => 1e-5 * w.abs().max(scale.map_or(0.0, |s| s[k])) + 1e-6,
            Kind::DcCurrent => 3e-5 * w.abs() + 1e-12f64.min(1e-5 * largest).max(1e-18) + 1e-7 * neighbour(k),
            Kind::Ac | Kind::Noise => 2e-4 * w.abs() + 1e-6 * largest,
        };
        let e = (*g - *w).abs() / bound;
        if e > out.0 || k == 0 {
            out = (e.max(out.0), if e >= out.0 { k } else { out.1 });
        }
    }
    out
}

/// The transient tube comparison: for each reference point, how far it lies
/// outside the range copper's waveform takes within one reference step of
/// it, in units of the bound.
fn tube(times: &[f64], values: &[f64], want: &Series, window: f64) -> (f64, usize) {
    let (lo, hi) = want.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), (_, z)| (l.min(z.re), h.max(z.re)));
    let bound = 2e-3 * (hi - lo) + 1e-6;
    let at = |t: f64| -> f64 {
        let k = times.partition_point(|x| *x < t).clamp(1, times.len() - 1);
        let (t0, t1) = (times[k - 1], times[k]);
        let s = if t1 > t0 { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 0.0 };
        values[k - 1] + s * (values[k] - values[k - 1])
    };
    let mut out = (0.0, 0);
    for (k, (t, z)) in want.iter().enumerate() {
        let (a, b) = (t - window, t + window);
        let mut range = (at(a.max(times[0])), at(a.max(times[0])));
        let end = at(b.min(*times.last().unwrap()));
        range = (range.0.min(end), range.1.max(end));
        let first = times.partition_point(|x| *x <= a);
        for i in first..times.len() {
            if times[i] >= b {
                break;
            }
            range = (range.0.min(values[i]), range.1.max(values[i]));
        }
        let outside = (z.re - range.1).max(range.0 - z.re).max(0.0);
        if outside / bound > out.0 {
            out = (outside / bound, k);
        }
    }
    out
}

fn run_deck(dir: &Path, deck: &Deck, outcomes: &mut Vec<Outcome>) {
    let reference = read_reference(dir, &deck.name);
    let lib = deck_library(deck, None);
    let temps = reference_temps(&reference);
    for (index, card) in deck.cards.iter().enumerate() {
        for temp in &temps {
            let celsius: f64 = temp.parse().unwrap();
            let series = |q: &str| reference.get(&(index, temp.clone(), q.to_string()));
            let mut record = |quantity: &str, kind: Kind, want: &Series, got: Vec<Complex>, scale: Option<&[f64]>| {
                let w: Vec<Complex> = want.iter().map(|p| p.1).collect();
                let (e, k) = worst(kind, &w, &got, scale);
                outcomes.push(Outcome {
                    deck: deck.name.clone(),
                    card: index,
                    analysis: card.kind(),
                    temp: temp.clone(),
                    quantity: quantity.to_string(),
                    points: w.len(),
                    worst: e,
                    at: want[k].0,
                    got: got[k],
                    want: w[k],
                });
            };
            match card {
                Card::Op | Card::Dc { .. } => {
                    let swept = match card {
                        Card::Dc { source } => Some(source.as_str()),
                        _ => None,
                    };
                    let first = deck
                        .probes
                        .first()
                        .and_then(|p| series(p))
                        .unwrap_or_else(|| panic!("{}: no reference for card {index} at {temp}", deck.name));
                    let xs: Vec<f64> = first.iter().map(|p| p.0).collect();
                    // Per voltage probe: the values and the magnitudes they are differences of.
                    let mut voltages: BTreeMap<String, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
                    let mut currents: BTreeMap<String, Vec<f64>> = BTreeMap::new();
                    match swept {
                        None => {
                            let c = circuit(&lib, celsius, 0.0);
                            let op = copper_sim::operating_point(&c).unwrap_or_else(|e| panic!("{}: {e}", deck.name));
                            for p in &deck.probes {
                                if p.starts_with("v(") {
                                    let (a, b) = probe_nodes(p);
                                    let va = op.voltage(&node(&a)).unwrap();
                                    let vb = b.map_or(0.0, |b| op.voltage(&node(&b)).unwrap());
                                    voltages.insert(p.clone(), (vec![va - vb], vec![va.abs() + vb.abs()]));
                                } else {
                                    let i = op.current(&element(&p[2..p.len() - 1])).unwrap();
                                    currents.insert(p.clone(), vec![i]);
                                }
                            }
                        }
                        Some(source) => {
                            let is_temp = source.eq_ignore_ascii_case("temp");
                            let swept_lib = if is_temp { lib.clone() } else { deck_library(deck, Some(source)) };
                            let circuits: Vec<Circuit> = xs
                                .iter()
                                .map(|x| {
                                    if is_temp {
                                        circuit(&swept_lib, *x, 0.0)
                                    } else {
                                        circuit(&swept_lib, celsius, *x)
                                    }
                                })
                                .collect();
                            let r =
                                copper_sim::dc_sweep(&circuits, &xs).unwrap_or_else(|e| panic!("{}: {e}", deck.name));
                            for p in &deck.probes {
                                if p.starts_with("v(") {
                                    let (a, b) = probe_nodes(p);
                                    let va = r.voltage(&node(&a)).unwrap().to_vec();
                                    let vb = b.map_or(vec![0.0; va.len()], |b| r.voltage(&node(&b)).unwrap().to_vec());
                                    let scale = va.iter().zip(&vb).map(|(x, y)| x.abs() + y.abs()).collect();
                                    voltages
                                        .insert(p.clone(), (va.iter().zip(&vb).map(|(x, y)| x - y).collect(), scale));
                                } else {
                                    currents.insert(p.clone(), r.current(&element(&p[2..p.len() - 1])).unwrap());
                                }
                            }
                        }
                    }
                    for p in &deck.probes {
                        let want = series(p).unwrap();
                        if let Some((v, scale)) = voltages.get(p) {
                            let got = v.iter().map(|x| Complex::new(*x, 0.0)).collect();
                            record(p, Kind::DcVoltage, want, got, Some(scale));
                        } else {
                            let got = currents[p].iter().map(|x| Complex::new(*x, 0.0)).collect();
                            record(p, Kind::DcCurrent, want, got, None);
                        }
                    }
                }
                Card::Ac => {
                    let c = circuit(&lib, celsius, 0.0);
                    let first = series(&deck.probes[0]).unwrap();
                    let fs: Vec<f64> = first.iter().map(|p| p.0).collect();
                    let r = copper_sim::ac(&c, &Sweep::List(fs)).unwrap_or_else(|e| panic!("{}: {e}", deck.name));
                    for p in &deck.probes {
                        record(p, Kind::Ac, series(p).unwrap(), ac_probe(&r, p), None);
                    }
                }
                Card::Noise { output, input } => {
                    let c = circuit(&lib, celsius, 0.0);
                    let want = series("onoise").unwrap();
                    let fs: Vec<f64> = want.iter().map(|p| p.0).collect();
                    let spec = NoiseSpec {
                        output: node(&output.0),
                        reference: output.1.as_ref().map(|b| node(b)),
                        input: Some(element(input)),
                        temperature: celsius + 273.15,
                        combination: None,
                    };
                    let r =
                        copper_sim::noise(&c, &Sweep::List(fs), &spec).unwrap_or_else(|e| panic!("{}: {e}", deck.name));
                    let on: Vec<Complex> = r.output_density().iter().map(|x| Complex::new(*x, 0.0)).collect();
                    record("onoise", Kind::Noise, want, on, None);
                    let inp: Vec<Complex> = r.input_density().unwrap().iter().map(|x| Complex::new(*x, 0.0)).collect();
                    record("inoise", Kind::Noise, series("inoise").unwrap(), inp, None);
                }
                Card::Tran { step, start, max, uic } => {
                    let c = circuit(&lib, celsius, 0.0);
                    let first = series(&deck.tran_probes[0]).unwrap();
                    let stop = first.last().unwrap().0;
                    // ngspice's default step limit: the smaller of the print step and a fiftieth of the span.
                    let limit = max.unwrap_or(step.min((stop - start) / 50.0));
                    let options = TranOptions {
                        start: *start,
                        max_step: Some(limit),
                        reltol: deck.tran_reltol,
                        initial: InitialConditions { uic: *uic, ..Default::default() },
                        ..TranOptions::new(stop)
                    };
                    let r = copper_sim::transient(&c, &options).unwrap_or_else(|e| panic!("{}: {e}", deck.name));
                    for p in &deck.tran_probes {
                        let got = if p.starts_with("v(") {
                            let (a, b) = probe_nodes(p);
                            match b {
                                Some(b) => r.voltage_between(&node(&a), &node(&b)).unwrap(),
                                None => r.voltage(&node(&a)).unwrap(),
                            }
                        } else {
                            r.current(&element(&p[2..p.len() - 1])).unwrap()
                        };
                        let want = series(p).unwrap();
                        let (e, k) = tube(r.times(), &got, want, *step);
                        let t = want[k].0;
                        let i = r.times().partition_point(|x| *x < t).min(got.len() - 1);
                        outcomes.push(Outcome {
                            deck: deck.name.clone(),
                            card: index,
                            analysis: "tran",
                            temp: temp.clone(),
                            quantity: p.clone(),
                            points: want.len(),
                            worst: e,
                            at: t,
                            got: Complex::new(got[i], 0.0),
                            want: want[k].1,
                        });
                    }
                }
            }
        }
    }
}

/// A value as printed: real, or `re+imj` for a phasor.
fn show(z: Complex) -> String {
    if z.im == 0.0 { format!("{:.7e}", z.re) } else { format!("{:.7e}{:+.7e}j", z.re, z.im) }
}

fn decks(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir.join("decks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "cir"))
        .collect();
    out.sort();
    out
}

/// The library of a deck. A deck that includes model files (the opt-in
/// vendor decks) is written to a temporary file and loaded with its
/// includes, which must then be absolute paths.
fn deck_library(deck: &Deck, swept: Option<&str>) -> Library {
    let text = library_text(deck, swept);
    let includes = deck.body.iter().any(|l| {
        let first = l.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        matches!(first.as_str(), ".include" | ".inc" | ".lib")
    });
    let lib = if includes {
        let path = std::env::temp_dir().join(format!("copper-reference-{}-{}.cir", deck.name, std::process::id()));
        std::fs::write(&path, &text).unwrap();
        let lib = Library::load(&path, deck.dialect, true);
        let _ = std::fs::remove_file(&path);
        lib
    } else {
        Library::parse(&text, &format!("{}.cir", deck.name), deck.dialect)
    };
    lib.unwrap_or_else(|e| panic!("{}: {e}", deck.name))
}

/// Run every deck of `dir` (`decks/`, `results/`), print the table and
/// return the differences beyond their bounds (`known` lists the established
/// ones).
fn compare_directory(dir: &Path, known: &[Known]) -> Vec<String> {
    let filter = std::env::var("REFERENCE_DECK").ok();
    let mut outcomes = Vec::new();
    for path in decks(dir) {
        let deck = read_deck(&path);
        if filter.as_ref().is_some_and(|f| !deck.name.contains(f.as_str())) {
            continue;
        }
        run_deck(dir, &deck, &mut outcomes);
    }
    let mut failures = Vec::new();
    let mut compared = 0;
    for o in &outcomes {
        compared += o.points;
        let listed = known.iter().find(|k| k.deck == o.deck && k.analysis == o.analysis && k.quantity == o.quantity);
        let mark = match listed {
            Some(k) if o.worst <= k.at_most => format!("known: {}", k.cause),
            Some(k) => {
                failures.push(format!(
                    "{} {} {} degC {}: {:.3} x the bound exceeds its listed {} ({})",
                    o.deck, o.analysis, o.temp, o.quantity, o.worst, k.at_most, k.cause
                ));
                "KNOWN, LARGER".to_string()
            }
            None if o.worst <= 1.0 => "ok".to_string(),
            None => {
                failures.push(format!(
                    "{} {} {} degC {}: {:.3} x the bound at x = {:.6e} (copper {}, ngspice {})",
                    o.deck,
                    o.analysis,
                    o.temp,
                    o.quantity,
                    o.worst,
                    o.at,
                    show(o.got),
                    show(o.want)
                ));
                "FAIL".to_string()
            }
        };
        println!(
            "{:<24} {:<6} {:>4} {:<16} {:>5} pts  worst {:>9.3} x bound at {:>12.5e}: copper {} ngspice {}  {mark}",
            o.deck,
            format!("{}{}", o.analysis, o.card),
            o.temp,
            o.quantity,
            o.points,
            o.worst,
            o.at,
            show(o.got),
            show(o.want)
        );
    }
    let listed = outcomes
        .iter()
        .filter(|o| known.iter().any(|k| k.deck == o.deck && k.analysis == o.analysis && k.quantity == o.quantity))
        .count();
    println!(
        "{} decks, {} series ({listed} listed as known), {compared} compared values",
        decks(dir).len(),
        outcomes.len()
    );
    failures
}

#[test]
fn every_deck_matches_its_ngspice_reference() {
    let failures = compare_directory(&root(), KNOWN);
    assert!(
        failures.is_empty(),
        "{} differences beyond their bounds:
{}",
        failures.len(),
        failures.join(
            "
"
        )
    );
}

/// Vendor models whose licence forbids committing them (README, "vendor
/// models"): decks in `tests/reference/vendor/decks/` that `.include` a model
/// from the model cache by its absolute path, with ngspice results made by
/// `generate.py` in `tests/reference/vendor/results/`. The directory is
/// gitignored; without it the test has nothing to do.
#[test]
#[ignore = "needs a local tests/reference/vendor directory (README, vendor models)"]
fn vendor_decks_match_their_local_references() {
    let dir = root().join("vendor");
    if !dir.join("decks").is_dir() {
        println!("no {}: nothing to compare", dir.join("decks").display());
        return;
    }
    let failures = compare_directory(&dir, &[]);
    assert!(
        failures.is_empty(),
        "{} differences beyond their bounds:
{}",
        failures.len(),
        failures.join(
            "
"
        )
    );
}

/// The temperatures of a reference, as written in its CSV, in increasing order.
fn reference_temps(reference: &BTreeMap<(usize, String, String), Series>) -> Vec<String> {
    let mut t: Vec<String> = reference.keys().map(|k| k.1.clone()).collect();
    t.sort_by(|a, b| a.parse::<f64>().unwrap().total_cmp(&b.parse::<f64>().unwrap()));
    t.dedup();
    t
}

/// The cause of the listed AC difference of `bjt_base_resistance` (README,
/// "base resistance linearisation"): ngspice stamps the bias-dependent base
/// resistance of a Gummel-Poon transistor (`RBM` with or without `IRB`) as
/// the plain conductance `1 / r_bb` of the bias point, without the
/// derivative of `r_bb` with respect to the junction voltages or the base
/// current, where copper-sim linearises its law exactly. With `r_bb` frozen
/// at copper-sim's own bias-point value (a constant `RB`), the AC results of
/// copper-sim equal ngspice's within the AC bound.
#[test]
fn base_resistance_difference_is_its_linearisation() {
    let deck = read_deck(&root().join("decks").join("bjt_base_resistance.cir"));
    let reference = read_reference(&root(), &deck.name);
    let lib = deck_library(&deck, None);
    let ac_index = deck.cards.iter().position(|c| matches!(c, Card::Ac)).unwrap();
    for temp in reference_temps(&reference) {
        let celsius: f64 = temp.parse().unwrap();
        let op = copper_sim::operating_point(&circuit(&lib, celsius, 0.0)).unwrap();
        // r_bb = (V(b) - V(b')) / I_B of the first transistor of each pair.
        let rbb = |q: &str, b: &str, source: &str| {
            let drop = op.voltage(&node(b)).unwrap() - op.voltage(&format!("{INSTANCE}.{q}#b")).unwrap();
            drop / -op.current(&element(source)).unwrap()
        };
        let (irb, rbm) = (rbb("Q1", "b1", "VB1"), rbb("Q3", "b3", "VB3"));
        let mut frozen = deck.body.clone();
        for line in &mut frozen {
            *line = line
                .replace("RB=120 RBM=15 IRB=0.2m", &format!("RB={irb:e}"))
                .replace("RB=200 RBM=20", &format!("RB={rbm:e}"));
        }
        let frozen_deck = Deck { body: frozen, ..read_deck(&root().join("decks").join("bjt_base_resistance.cir")) };
        let frozen_lib = deck_library(&frozen_deck, None);
        let first = &reference[&(ac_index, temp.clone(), "i(vb1)".to_string())];
        let fs: Vec<f64> = first.iter().map(|p| p.0).collect();
        let r = copper_sim::ac(&circuit(&frozen_lib, celsius, 0.0), &Sweep::List(fs)).unwrap();
        for p in ["i(vb1)", "i(vc1)", "i(vb2)", "i(vc2)", "i(vb3)", "i(vc3)", "i(vb4)", "i(vc4)"] {
            let want: Vec<Complex> = reference[&(ac_index, temp.clone(), p.to_string())].iter().map(|x| x.1).collect();
            let (e, k) = worst(Kind::Ac, &want, &ac_probe(&r, p), None);
            println!("{temp} degC {p}: r_bb {irb:.4} / {rbm:.4} ohm frozen, worst {e:.3} x the AC bound at point {k}");
            assert!(e <= 1.0, "{temp} degC {p}: {e} x the bound with r_bb frozen");
        }
    }
}

/// The cause of the listed DC differences of the substrate currents (README,
/// "substrate G_MIN"): ngspice-46 puts a G_MIN conductance across every
/// bipolar transistor's substrate junction (its substrate diode, `ISS` = 0
/// here); copper-sim's substrate junction has its depletion charge only. The
/// reference's substrate current is G_MIN times copper-sim's substrate
/// junction voltage.
#[test]
fn substrate_difference_is_the_gmin_leak() {
    let deck = read_deck(&root().join("decks").join("bjt_substrate.cir"));
    let reference = read_reference(&root(), &deck.name);
    let lib = deck_library(&deck, None);
    let op_index = deck.cards.iter().position(|c| matches!(c, Card::Op)).unwrap();
    // The substrate, and the internal node its junction ends at: the collector
    // of a vertical device, the base of the lateral PNP.
    let junctions =
        [("i(vs1)", "s1", "Q1#c"), ("i(vs2)", "s2", "Q2#b"), ("i(vs3)", "s3", "Q3#c"), ("i(vs4)", "s4", "Q4#c")];
    for temp in reference_temps(&reference) {
        let celsius: f64 = temp.parse().unwrap();
        let op = copper_sim::operating_point(&circuit(&lib, celsius, 0.0)).unwrap();
        for (probe, s, inner) in junctions {
            let v = op.voltage(&format!("{INSTANCE}.{inner}")).unwrap() - op.voltage(&node(s)).unwrap();
            let leak = copper_sim::devices::GMIN * v;
            let want = reference[&(op_index, temp.clone(), probe.to_string())][0].1.re;
            let got = op.current(&element(&probe[2..probe.len() - 1])).unwrap();
            println!("{temp} degC {probe}: ngspice {want:.6e} A, copper {got:.6e} A, G_MIN x {v:.4} V = {leak:.6e} A");
            assert!((want - got - leak).abs() < 1e-3 * leak.abs() + 1e-17, "{temp} {probe}");
        }
    }
}

/// The cause of the listed level-3 AC differences (README, "level-3
/// derivatives"): ngspice's `mos3load` writes the derivatives of the drain
/// current by hand, and with `VMAX` and `ETA` its `g_m` and `g_ds` are not
/// the derivatives of its own DC current. copper-sim's exact derivatives
/// equal a fourth-order central difference of the reference's DC sweeps.
#[test]
fn level3_difference_is_ngspice_derivatives() {
    let deck = read_deck(&root().join("decks").join("mos3_derivatives.cir"));
    let reference = read_reference(&root(), &deck.name);
    let lib = deck_library(&deck, None);
    let difference = |index: usize, probe: &str| {
        let s = &reference[&(index, "27".to_string(), probe.to_string())];
        let (h, f) = (s[1].0 - s[0].0, |k: usize| -s[k].1.re);
        (-f(4) + 8.0 * f(3) - 8.0 * f(1) + f(0)) / (12.0 * h)
    };
    let (gm_dc, gds_dc) = (difference(0, "i(vd1)"), difference(1, "i(vd2)"));
    let gm_ac = -reference[&(2, "27".to_string(), "i(vd1)".to_string())][0].1.re;
    let gds_ac = -reference[&(2, "27".to_string(), "i(vd2)".to_string())][0].1.re;
    let r = copper_sim::ac(&circuit(&lib, 27.0, 0.0), &Sweep::List(vec![1e3])).unwrap();
    let gm = -ac_probe(&r, "i(vd1)")[0].re;
    let gds = -ac_probe(&r, "i(vd2)")[0].re;
    println!("g_m: copper {gm:.6e}, ngspice DC difference {gm_dc:.6e}, ngspice AC {gm_ac:.6e}");
    println!("g_ds: copper {gds:.6e}, ngspice DC difference {gds_dc:.6e}, ngspice AC {gds_ac:.6e}");
    // The difference's own error: Newton noise of 1e-9 A over 0.12 V, below 1e-5 of g_m.
    assert!((gm / gm_dc - 1.0).abs() < 1e-4, "g_m");
    assert!((gds / gds_dc - 1.0).abs() < 1e-3, "g_ds");
    assert!((gm_ac / gm_dc - 1.0).abs() > 0.01, "ngspice's AC g_m differs from its DC slope");
}

/// The integration-floor rule of `copper_sim::spectrum` (the harmonics a
/// linear circuit shows in a transient run at `reltol` with the steps left to
/// the error control are `0.02 sqrt(reltol)` of the fundamental within a
/// factor of 2), derived on one RC low-pass, on three more linear circuits:
/// a series RLC band-pass, a Sallen-Key low-pass around a VCVS and a
/// fifth-order LC ladder, each in its periodic steady state at 1 kHz. A
/// linear circuit has no harmonics, so every one is integration error. The
/// fundamental is compared with ngspice's AC result at 1 kHz: trapezoidal
/// integration at steps `h` changes a sine's amplitude by about
/// `(w h)^2 / 12`, so the bound is twice that at the run's longest step.
#[test]
fn integration_floor_holds_on_more_linear_circuits() {
    use copper_sim::spectrum::coherent;
    for name in ["floor_rlc", "floor_sallen_key", "floor_ladder"] {
        let deck = read_deck(&root().join("decks").join(format!("{name}.cir")));
        let reference = read_reference(&root(), name);
        let lib = deck_library(&deck, None);
        let c = circuit(&lib, 27.0, 0.0);
        let probe = &deck.probes[0];
        let ac_index = deck.cards.iter().position(|k| matches!(k, Card::Ac)).unwrap();
        let gain = reference[&(ac_index, "27".to_string(), probe.clone())][0].1.abs();
        for reltol in [1e-3, 1e-4, 1e-5, 1e-6] {
            let options = TranOptions { steady: Some(1e-3), reltol, ..TranOptions::new(10e-3) };
            let r = copper_sim::transient(&c, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
            let (a, b) = probe_nodes(probe);
            let v = match b {
                Some(b) => r.voltage_between(&node(&a), &node(&b)).unwrap(),
                None => r.voltage(&node(&a)).unwrap(),
            };
            let s = coherent(r.times(), &v, 1e3).unwrap();
            let fundamental = s.harmonic(1).unwrap();
            let worst = (2..=20).map(|n| s.harmonic(n).unwrap()).fold(0.0, f64::max) / fundamental;
            let rule = 0.02 * reltol.sqrt();
            let amplitude = (fundamental / gain - 1.0).abs();
            let wh = 2.0 * std::f64::consts::PI * 1e3 * r.stats().largest_step;
            let trapezoid = wh * wh / 12.0;
            println!(
                "{name} reltol {reltol:.0e}: harmonics {worst:.2e} = {:.2} x 0.02 sqrt(reltol); fundamental {amplitude:.2e} off ngspice's AC gain ((w h)^2 / 12 = {trapezoid:.2e}); {} steps",
                worst / rule,
                r.stats().accepted_steps
            );
            assert!(worst <= 2.0 * rule, "{name} at {reltol}: {worst} above twice the rule {rule}");
            assert!(amplitude <= 2.0 * trapezoid + 1e-6, "{name} at {reltol}: the fundamental is {amplitude} off");
        }
    }
}

/// Transient noise with flicker noise (README, "transient noise"): a diode's
/// shot and 1/f noise in a 10 s run sampled every 50 us, its output's power
/// spectral density estimated by Welch's method (Hann windows of 4096
/// samples, half overlapping) and averaged over octave bands from 20 Hz to
/// 640 Hz, against ngspice's noise analysis of the same circuit times
/// `sinc^4(f step)` (the linear interpolation of the samples,
/// `copper_sim::transient_noise`). Each band's estimate is an average of
/// `N` periodogram bins (segments times bins in the band), whose relative
/// spread is `1 / sqrt(N)` (a factor of 1.4 more for the overlap); the
/// bound is four times that.
#[test]
fn transient_flicker_noise_matches_the_noise_analysis() {
    use copper_sim::spectrum::dft;
    let deck = read_deck(&root().join("decks").join("noise_transient.cir"));
    let reference = read_reference(&root(), &deck.name);
    let lib = deck_library(&deck, None);
    let c = circuit(&lib, 27.0, 0.0);
    let noise_index = deck.cards.iter().position(|k| matches!(k, Card::Noise { .. })).unwrap();
    let onoise = &reference[&(noise_index, "27".to_string(), "onoise".to_string())];
    let density = |f: f64| {
        // Log-log interpolation of ngspice's output density, V²/Hz.
        let k = onoise.partition_point(|p| p.0 < f).clamp(1, onoise.len() - 1);
        let (f0, f1) = (onoise[k - 1].0, onoise[k].0);
        let (s0, s1) = (onoise[k - 1].1.re.powi(2), onoise[k].1.re.powi(2));
        (s0.ln() + (f / f0).ln() / (f1 / f0).ln() * (s1.ln() - s0.ln())).exp()
    };
    let (step, stop) = (50e-6, 10.0);
    let options = TranOptions {
        max_step: Some(step),
        noise: Some(copper_sim::TranNoise { seed: 7, temperature: 300.15 }),
        ..TranOptions::new(stop)
    };
    let r = copper_sim::transient(&c, &options).unwrap();
    let (t, v) = (r.times(), r.voltage(&node("a")).unwrap());
    let samples = (stop / step).round() as usize;
    let uniform: Vec<f64> = (0..samples)
        .map(|k| {
            let x = k as f64 * step;
            let i = t.partition_point(|y| *y < x).clamp(1, t.len() - 1);
            let s = ((x - t[i - 1]) / (t[i] - t[i - 1])).clamp(0.0, 1.0);
            v[i - 1] + s * (v[i] - v[i - 1])
        })
        .collect();
    let n = 4096;
    let window: Vec<f64> =
        (0..n).map(|k| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / n as f64).cos()).collect();
    let power: f64 = window.iter().map(|w| w * w).sum();
    let mut psd = vec![0.0; n / 2];
    let mut segments = 0;
    let mut start = 0;
    while start + n <= samples {
        let seg = &uniform[start..start + n];
        let mean = seg.iter().sum::<f64>() / n as f64;
        let x: Vec<Complex> = seg.iter().zip(&window).map(|(s, w)| Complex::new((s - mean) * w, 0.0)).collect();
        for (p, z) in psd.iter_mut().zip(dft(&x)) {
            *p += 2.0 * step * z.abs().powi(2) / power;
        }
        segments += 1;
        start += n / 2;
    }
    let df = 1.0 / (n as f64 * step);
    let mut low = 20.0;
    while low < 640.0 {
        let bins: Vec<usize> = (1..n / 2).filter(|k| (*k as f64 * df) >= low && (*k as f64 * df) < 2.0 * low).collect();
        let got = bins.iter().map(|k| psd[*k] / segments as f64).sum::<f64>() / bins.len() as f64;
        let want = bins
            .iter()
            .map(|k| {
                let f = *k as f64 * df;
                let x = std::f64::consts::PI * f * step;
                density(f) * (x.sin() / x).powi(4)
            })
            .sum::<f64>()
            / bins.len() as f64;
        let spread = 1.4 / ((segments * bins.len()) as f64).sqrt();
        println!(
            "{low:.0} to {:.0} Hz: transient {:.4e} V²/Hz, noise analysis {want:.4e} V²/Hz ({:+.1} %, bound {:.1} %, {} bins x {segments} segments)",
            2.0 * low,
            got,
            (got / want - 1.0) * 100.0,
            4.0 * spread * 100.0,
            bins.len()
        );
        assert!((got / want - 1.0).abs() < 4.0 * spread, "{low} Hz band: {got} vs {want}");
        low *= 2.0;
    }
}

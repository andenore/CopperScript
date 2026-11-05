//! Synthesis drivers.
//!
//! Every candidate is scored in the full-wave model of the board
//! ([`crate::model`]) by its worst return loss over every declared band, with
//! the best L-section matching network synthesized from its simulated feed
//! impedance ([`crate::matching`]) unless it already meets the target bare.
//!
//! Parametric families share a short coordinate vector: `(height, length)`
//! for the monopole and meandered monopole (the run's height above the ground
//! edge and the conductor length, realized constructively as the fewest
//! meander turns that fit), plus the feed-to-leg spacing `short` for the
//! inverted-F. Search is multi-fidelity, in the spirit of space mapping:
//!
//! 1. **Resonance tuning** on the coarse mesh: the length is scaled by
//!    `f_resonance / f_centre` until the series resonance sits on the band
//!    centre (electrical length is inversely proportional to frequency).
//! 2. **Polish**: a bounded Nelder-Mead search over the coordinates on the
//!    coarse mesh, minimizing the matched worst-case return loss.
//! 3. **Fine verification**: the winner on a mesh twice as fine, with one
//!    more length correction if the resonance moved, and the radiation
//!    efficiency from a near-to-far-field transform.
//!
//! Monopoles, meanders and pixel grids are scored by the matched worst case:
//! their matching network is part of the design. An inverted-F is meant to
//! be self-matched, with the short spacing as its impedance knob, so its
//! search scores the bare worst case and the matching network is only
//! reported.
//!
//! Pixel topologies run a seeded genetic algorithm over the tiles of a grid in
//! the region (binary encoding, tournament selection, uniform crossover,
//! bit-flip mutation, elitism, connectivity repair to the feed tile), after
//! Mair and Baumgarten (Scientific Reports 14, 2024), then the same fine
//! verification.

use indexmap::IndexMap;

use copper_pcb::antenna::{AntennaSite, Geometry, Layout, Topology, layout, quarter_wave_nm, snap};

use crate::matching::{Network, l_sections, return_loss_db};
use crate::model::{Evaluation, Fidelity, Problem, evaluate};

/// One band's worst return loss, bare and matched.
#[derive(Clone, Debug)]
pub struct BandScore {
    pub band: String,
    pub raw_worst_db: f64,
    pub worst_db: f64,
}

/// What a search minimizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Objective {
    /// The worst return loss over every band with the best L-section.
    Matched,
    /// The worst return loss over every band without any network.
    Bare,
}

impl Score {
    /// The figure of merit under `objective` (dB, lower is better).
    pub fn objective_db(&self, objective: Objective) -> f64 {
        match objective {
            Objective::Matched => self.worst_db,
            Objective::Bare => self.raw_worst_db,
        }
    }
}

/// A candidate's figure of merit.
#[derive(Clone, Debug)]
pub struct Score {
    /// The worst return loss over every band with `network` (dB).
    pub worst_db: f64,
    /// The same without any matching network.
    pub raw_worst_db: f64,
    pub network: Network,
    pub resonance_hz: Option<f64>,
    pub bands: Vec<BandScore>,
}

/// One full-wave evaluation in the search history.
#[derive(Clone, Debug)]
pub struct Trial {
    pub stage: &'static str,
    pub geometry: String,
    pub fidelity: &'static str,
    pub resonance_hz: Option<f64>,
    pub raw_worst_db: f64,
    pub worst_db: f64,
    pub seconds: f64,
    pub cells: usize,
    pub converged: bool,
}

/// What a synthesis produced.
pub struct Outcome {
    pub geometry: Geometry,
    pub layout: Layout,
    /// The final evaluation (fine unless fine verification was skipped).
    pub evaluation: Evaluation,
    pub score: Score,
    pub trials: Vec<Trial>,
    pub method: &'static str,
}

fn interpolate(freqs: &[f64], values: &[ems::prelude::Complex64], f: f64) -> ems::prelude::Complex64 {
    let i = freqs.partition_point(|x| *x < f).clamp(1, freqs.len() - 1);
    let t = (f - freqs[i - 1]) / (freqs[i] - freqs[i - 1]);
    values[i - 1] * (1.0 - t) + values[i] * t
}

/// The series resonance (reactance crossing zero upward) nearest `f0`, or the
/// frequency of the deepest return loss when there is none.
pub fn resonance(evaluation: &Evaluation, f0: f64) -> Option<f64> {
    let (f, z) = (&evaluation.freqs, &evaluation.z);
    let crossings = (0..f.len().saturating_sub(1)).filter(|i| z[*i].im <= 0.0 && z[i + 1].im > 0.0).map(|i| {
        let t = -z[i].im / (z[i + 1].im - z[i].im);
        f[i] + t * (f[i + 1] - f[i])
    });
    crossings
        .min_by(|a, b| (a - f0).abs().total_cmp(&(b - f0).abs()))
        .or_else(|| evaluation.s11_db.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| f[i]))
}

/// Worst return loss over every band, bare and with the best L-section.
pub fn score(site: &AntennaSite, evaluation: &Evaluation) -> Score {
    let f0 = site.centre_hz();
    let points: Vec<Vec<f64>> = site
        .bands
        .iter()
        .map(|band| {
            let (low, high) =
                (copper_pcb::antenna::decimal_f64(&band.low_hz), copper_pcb::antenna::decimal_f64(&band.high_hz));
            let mut fs: Vec<f64> = evaluation.freqs.iter().copied().filter(|f| (low..=high).contains(f)).collect();
            fs.extend([low, high]);
            fs
        })
        .collect();
    let worst = |network: &Network| -> (f64, Vec<f64>) {
        let per_band: Vec<f64> = points
            .iter()
            .map(|fs| {
                fs.iter()
                    .map(|f| return_loss_db(network.apply(interpolate(&evaluation.freqs, &evaluation.z, *f), *f), 50.0))
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .collect();
        (per_band.iter().copied().fold(f64::NEG_INFINITY, f64::max), per_band)
    };
    let bare = Network { elements: Vec::new() };
    let (raw_worst_db, raw_bands) = worst(&bare);
    let mut best = (raw_worst_db, raw_bands.clone(), bare);
    if raw_worst_db > site.max_return_loss_db {
        for network in l_sections(interpolate(&evaluation.freqs, &evaluation.z, f0), f0, 50.0) {
            let (value, bands) = worst(&network);
            if value < best.0 {
                best = (value, bands, network);
            }
        }
    }
    let bands = site
        .bands
        .iter()
        .zip(raw_bands.iter().zip(&best.1))
        .map(|(band, (raw, matched))| BandScore { band: band.text.clone(), raw_worst_db: *raw, worst_db: *matched })
        .collect();
    Score { worst_db: best.0, raw_worst_db, network: best.2, resonance_hz: resonance(evaluation, f0), bands }
}

/// The parametric family of a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Monopole,
    Meander,
    InvertedF,
    Auto,
}

/// The meander leg pitch: at least the clearance between legs, and two track widths.
fn pitch(site: &AntennaSite) -> i64 {
    let raw = (site.width + site.clearance).max(2 * site.width);
    let grid = copper_pcb::antenna::LENGTH_GRID_NM;
    (raw + grid - 1) / grid * grid
}

/// The fewest-turn meander of conductor `length` with its run at `height`.
fn meander_for(site: &AntennaSite, height: i64, length: i64) -> Option<Geometry> {
    let p = pitch(site);
    let margin = site.margin();
    let room = site.frame.length() - margin - site.feed_u;
    let depth_max = height - margin;
    if depth_max <= 0 {
        return None;
    }
    for turns in 1..=200u32 {
        let t = i64::from(turns);
        let horizontal = p + 2 * t * p;
        if horizontal > room {
            return None;
        }
        let base = height + horizontal;
        if length < base {
            return None;
        }
        let depth = snap(((length - base) / (2 * t)).min(depth_max)).max(0);
        if depth <= 0 {
            continue;
        }
        let tail = length - base - 2 * t * depth;
        if tail < 0 || horizontal + tail > room {
            continue;
        }
        let geometry = Geometry::Meander { height, depth, pitch: p, turns, tail: snap(tail) };
        if layout(site, &geometry).is_ok() {
            return Some(geometry);
        }
    }
    None
}

/// The geometry of `family` at `coords` (`[height, length]`, plus `short`
/// for the inverted-F), snapped to the grid; `None` when it does not fit.
fn realize(site: &AntennaSite, family: Family, coords: &[i64]) -> Option<Geometry> {
    let (height, length) = (snap(coords[0]), snap(coords[1]));
    let monopole = Geometry::Monopole { height, length };
    match family {
        Family::Monopole => layout(site, &monopole).ok().map(|_| monopole),
        Family::Meander => meander_for(site, height, length),
        Family::InvertedF => {
            // The arm runs from the leg to the usable end of the region: clamp
            // the length to that room rather than failing, so a scaled or
            // reflected step past the end still yields the longest arm.
            let link = site.short.as_ref()?;
            let short = snap(*coords.get(2)?).clamp(link.short_min, link.short_max);
            let room = site.frame.length() - site.margin() - (site.feed_u - short);
            let length = length.clamp(snap(short + site.width + copper_pcb::antenna::LENGTH_GRID_NM), snap(room));
            let geometry = Geometry::InvertedF { height, length, short };
            layout(site, &geometry).ok().map(|_| geometry)
        }
        Family::Auto => {
            if layout(site, &monopole).is_ok() {
                Some(monopole)
            } else {
                meander_for(site, height, length)
            }
        }
    }
}

/// The coordinates of a parametric geometry (see [`realize`]).
fn coordinates(site: &AntennaSite, geometry: &Geometry) -> Option<Vec<i64>> {
    match geometry {
        Geometry::Monopole { height, length } => Some(vec![*height, *length]),
        Geometry::Meander { height, .. } => layout(site, geometry).ok().map(|l| vec![*height, l.length_nm]),
        Geometry::InvertedF { height, length, short } => Some(vec![*height, *length, *short]),
        Geometry::Pixel { .. } => None,
    }
}

/// Search settings.
pub struct Options {
    /// Coarse full-wave evaluations the search may spend.
    pub budget: usize,
    /// Run fine verification (and its correction) after the search.
    pub fine: bool,
    pub threads: usize,
    /// Pixel cell size, defaulting to the larger of the track width and 1 mm.
    pub pixel_cell: Option<i64>,
}

struct Search<'a, 'b> {
    problem: &'a Problem<'b>,
    coarse: Fidelity,
    fine: Fidelity,
    budget: usize,
    used: usize,
    trials: Vec<Trial>,
    cache: IndexMap<(String, &'static str), (Evaluation, Score)>,
    log: &'a mut dyn FnMut(&str),
}

impl Search<'_, '_> {
    fn run(
        &mut self,
        geometry: &Geometry,
        fine: bool,
        stage: &'static str,
    ) -> Result<Option<(Evaluation, Score)>, String> {
        let site = self.problem.site;
        let Ok(realized) = layout(site, geometry) else { return Ok(None) };
        let fidelity = if fine { self.fine.clone() } else { self.coarse.clone() };
        let key = (geometry.to_source(), fidelity.name);
        if let Some(hit) = self.cache.get(&key) {
            return Ok(Some(hit.clone()));
        }
        if !fine {
            if self.used >= self.budget {
                return Ok(None);
            }
            self.used += 1;
        }
        let evaluation = evaluate(self.problem, &realized, &fidelity)?;
        let score = score(site, &evaluation);
        (self.log)(&format!(
            "  {stage:<9} {:<6} {:<62} resonance {:>10}  bare {:6.1} dB  matched {:6.1} dB  {:.2} Mcells {} steps {:.2} ps {:5.1} s{}",
            fidelity.name,
            geometry.to_source(),
            score.resonance_hz.map_or("-".to_owned(), |f| format!("{:.4} GHz", f / 1e9)),
            score.raw_worst_db,
            score.worst_db,
            evaluation.cells as f64 / 1e6,
            evaluation.steps,
            evaluation.dt_ps,
            evaluation.seconds,
            if evaluation.converged { "" } else { "  (not converged)" }
        ));
        self.trials.push(Trial {
            stage,
            geometry: geometry.to_source(),
            fidelity: fidelity.name,
            resonance_hz: score.resonance_hz,
            raw_worst_db: score.raw_worst_db,
            worst_db: score.worst_db,
            seconds: evaluation.seconds,
            cells: evaluation.cells,
            converged: evaluation.converged,
        });
        self.cache.insert(key, (evaluation.clone(), score.clone()));
        Ok(Some((evaluation, score)))
    }

    fn exhausted(&self) -> bool {
        self.used >= self.budget
    }
}

/// The first-order starting point of a parametric family.
fn initial(site: &AntennaSite, family: Family) -> Result<Geometry, String> {
    let length = quarter_wave_nm(site.centre_hz(), site.eps_eff);
    let height = snap(site.frame.depth() - site.margin() - copper_pcb::antenna::LENGTH_GRID_NM / 2);
    let mut coords = vec![height, length];
    if let Some(link) = &site.short {
        coords.push((link.short_min + link.short_max) / 2);
    }
    realize(site, family, &coords).ok_or_else(|| {
        format!(
            "the region cannot hold a quarter wave ({:.2} mm at effective permittivity {:.3}) as {}; enlarge it \
             along the board edge or deepen it",
            length as f64 * 1e-6,
            site.eps_eff,
            match family {
                Family::Monopole => "a monopole",
                Family::Meander => "a meander",
                Family::InvertedF => "an inverted-F",
                Family::Auto => "a monopole or meander",
            }
        )
    })
}

/// A bounded Nelder-Mead minimization over `start.len()` variables. `f`
/// returns `None` for infeasible points (treated as +inf) and stops the
/// search when the budget is spent (`Err`).
fn nelder_mead(
    mut f: impl FnMut(&[f64]) -> Result<Option<f64>, String>,
    start: Vec<f64>,
    step: Vec<f64>,
    iterations: usize,
) -> Result<(Vec<f64>, f64), String> {
    let n = start.len();
    let mut value = |x: &[f64]| -> Result<f64, String> { Ok(f(x)?.unwrap_or(f64::INFINITY)) };
    let mut simplex: Vec<Vec<f64>> = vec![start.clone()];
    for i in 0..n {
        let mut vertex = start.clone();
        vertex[i] += step[i];
        simplex.push(vertex);
    }
    let mut values = Vec::new();
    for x in &simplex {
        values.push(value(x)?);
    }
    let last = n;
    for _ in 0..iterations {
        let mut order: Vec<usize> = (0..=n).collect();
        order.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
        simplex = order.iter().map(|i| simplex[*i].clone()).collect();
        values = order.iter().map(|i| values[*i]).collect();
        let centroid: Vec<f64> = (0..n).map(|k| simplex[..last].iter().map(|v| v[k]).sum::<f64>() / n as f64).collect();
        let along =
            |t: f64| -> Vec<f64> { (0..n).map(|k| centroid[k] + t * (simplex[last][k] - centroid[k])).collect() };
        let reflected = along(-1.0);
        let fr = value(&reflected)?;
        if fr < values[0] {
            let expanded = along(-2.0);
            let fe = value(&expanded)?;
            (simplex[last], values[last]) = if fe < fr { (expanded, fe) } else { (reflected, fr) };
        } else if fr < values[last - 1] {
            (simplex[last], values[last]) = (reflected, fr);
        } else {
            let contracted = along(0.5);
            let fc = value(&contracted)?;
            if fc < values[last] {
                (simplex[last], values[last]) = (contracted, fc);
            } else {
                for i in 1..=n {
                    simplex[i] = (0..n).map(|k| (simplex[0][k] + simplex[i][k]) / 2.0).collect();
                    values[i] = value(&simplex[i])?;
                }
            }
        }
        let spread: f64 =
            (0..n).map(|k| simplex[1..].iter().map(|v| (v[k] - simplex[0][k]).abs()).fold(0.0, f64::max)).sum();
        if spread < 20_000.0 {
            break;
        }
    }
    let best = (0..=n).min_by(|a, b| values[*a].total_cmp(&values[*b])).expect("a simplex");
    Ok((simplex[best].clone(), values[best]))
}

const BUDGET_SPENT: &str = "budget spent";

/// Synthesize a parametric (monopole or meander) antenna for `problem`.
pub fn synthesize(problem: &Problem, options: &Options, log: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    let site = problem.site;
    if site.topology == Topology::Pixel {
        return synthesize_pixel(problem, options, log);
    }
    let family = match (site.topology, site.short.is_some()) {
        (Topology::Monopole, _) => Family::Monopole,
        (Topology::Meander, _) => Family::Meander,
        (Topology::InvertedF, _) | (Topology::Auto, true) => Family::InvertedF,
        _ => Family::Auto,
    };
    let mut search = Search {
        problem,
        coarse: Fidelity::coarse(site, options.threads),
        fine: Fidelity::fine(site, options.threads),
        budget: options.budget.max(1),
        used: 0,
        trials: Vec::new(),
        cache: IndexMap::new(),
        log,
    };
    let f0 = site.centre_hz();
    let start = initial(site, family)?;
    let mut coords = coordinates(site, &start).expect("parametric");
    let mut best: Option<(Geometry, Score)> = None;
    let objective = if family == Family::InvertedF { Objective::Bare } else { Objective::Matched };
    let consider = |geometry: &Geometry, score: &Score, best: &mut Option<(Geometry, Score)>| {
        if best.as_ref().is_none_or(|(_, b)| score.objective_db(objective) < b.objective_db(objective)) {
            *best = Some((geometry.clone(), score.clone()));
        }
    };

    // 1. Resonance tuning.
    for _ in 0..5 {
        let Some(geometry) = realize(site, family, &coords) else { break };
        let Some((_, score)) = search.run(&geometry, false, "resonance")? else { break };
        consider(&geometry, &score, &mut best);
        let Some(fr) = score.resonance_hz else { break };
        if (fr / f0 - 1.0).abs() < 0.004 || search.exhausted() {
            break;
        }
        coords[1] = (coords[1] as f64 * fr / f0) as i64;
    }

    // 2. Polish over (height, length).
    if !search.exhausted()
        && let Some((geometry, _)) = &best
    {
        let start = coordinates(site, geometry).expect("parametric");
        let mut step =
            vec![((site.frame.depth() - 2 * site.margin()) as f64 / 4.0).max(200_000.0), start[1] as f64 * 0.03];
        if let Some(link) = &site.short {
            step.push(((link.short_max - link.short_min) as f64 / 4.0).max(100_000.0));
        }
        let iterations = search.budget.saturating_sub(search.used);
        let result = nelder_mead(
            |x| {
                if search.exhausted() {
                    return Err(BUDGET_SPENT.to_owned());
                }
                let coords: Vec<i64> = x.iter().map(|v| *v as i64).collect();
                let Some(geometry) = realize(site, family, &coords) else { return Ok(None) };
                match search.run(&geometry, false, "polish")? {
                    Some((_, score)) => {
                        consider(&geometry, &score, &mut best);
                        Ok(Some(score.objective_db(objective)))
                    }
                    None => Ok(None),
                }
            },
            start.iter().map(|v| *v as f64).collect(),
            step,
            iterations,
        );
        match result {
            Ok(_) => {}
            Err(message) if message == BUDGET_SPENT => {}
            Err(message) => return Err(message),
        }
    }
    let Some((mut geometry, mut score)) = best else {
        return Err("no candidate geometry could be evaluated; raise the budget or enlarge the region".to_owned());
    };
    coords = coordinates(site, &geometry).expect("parametric");
    let (mut evaluation, _) = search.cache[&(geometry.to_source(), "coarse")].clone();

    // 3. Fine verification, with one length correction if the resonance moved.
    if options.fine {
        let (fine_eval, fine_score) = search.run(&geometry, true, "verify")?.expect("a realizable geometry");
        (evaluation, score) = (fine_eval, fine_score);
        let mut corrected_coords = coords.clone();
        corrected_coords[1] = (coords[1] as f64 * score.resonance_hz.unwrap_or(f0) / f0) as i64;
        if let Some(fr) = score.resonance_hz
            && (fr / f0 - 1.0).abs() > 0.006
            && let Some(corrected) = realize(site, family, &corrected_coords)
            && let Some((corrected_eval, corrected_score)) = search.run(&corrected, true, "correct")?
            && corrected_score.objective_db(objective) < score.objective_db(objective)
        {
            (geometry, evaluation, score) = (corrected, corrected_eval, corrected_score);
        }
    }
    let realized = layout(site, &geometry).map_err(|e| e.to_string())?;
    Ok(Outcome { geometry, layout: realized, evaluation, score, trials: search.trials, method: "parametric" })
}

/// A deterministic xorshift generator seeded from the site.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The pixel grid of a site: `(cell, columns, rows, feed column)`.
fn pixel_grid(site: &AntennaSite, cell: i64) -> Option<(i64, u32, u32, i64)> {
    let clearance = site.clearance;
    let u0 = site.feed_u;
    let left = (u0 - cell / 2 - clearance) / cell;
    let first_u = u0 - cell / 2 - left * cell;
    let bottom = clearance.max(site.margin() - site.width / 2);
    let columns = (site.frame.length() - clearance - first_u) / cell;
    let rows = (site.frame.depth() - clearance - bottom) / cell;
    (columns > left && rows >= 1).then_some((cell, columns as u32, rows as u32, left))
}

/// Synthesize a pixel antenna with a genetic algorithm.
fn synthesize_pixel(problem: &Problem, options: &Options, log: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    let site = problem.site;
    let cell = snap(options.pixel_cell.unwrap_or(site.width.max(1_000_000)));
    let Some((cell, columns, rows, feed_column)) = pixel_grid(site, cell) else {
        return Err("the region is too small for a pixel grid of this cell size".to_owned());
    };
    let (c, r) = (columns as usize, rows as usize);
    let n = c * r;
    let feed_bit = feed_column as usize;
    let mut search = Search {
        problem,
        coarse: Fidelity::coarse(site, options.threads),
        fine: Fidelity::fine(site, options.threads),
        budget: options.budget.max(2),
        used: 0,
        trials: Vec::new(),
        cache: IndexMap::new(),
        log,
    };
    // Seed: the first-order monopole rasterized, up the feed column then
    // along the top row.
    let quarter = quarter_wave_nm(site.centre_hz(), site.eps_eff);
    let mut seed = vec![false; n];
    for row in 0..r {
        seed[row * c + feed_bit] = true;
    }
    let run_tiles = ((quarter - rows as i64 * cell).max(0) / cell) as usize;
    for column in feed_bit..(feed_bit + run_tiles + 1).min(c) {
        seed[(r - 1) * c + column] = true;
    }
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in site.name.bytes().chain(site.declaration.bytes()) {
        digest = (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let mut rng = Rng(digest | 1);
    let geometry = |bits: &[bool]| Geometry::Pixel { cell, columns, rows, bits: bits.to_vec() };
    let mutate = |bits: &mut Vec<bool>, rng: &mut Rng| {
        let flips = 1 + rng.below(3);
        for _ in 0..flips {
            let i = rng.below(n);
            bits[i] = !bits[i];
        }
        bits[feed_bit] = true;
    };
    let population_size = (search.budget / 4).clamp(4, 12);
    let mut population: Vec<(Vec<bool>, f64)> = Vec::new();
    let mut members = vec![seed.clone()];
    while members.len() < population_size {
        let mut child = seed.clone();
        mutate(&mut child, &mut rng);
        members.push(child);
    }
    for bits in members {
        match search.run(&geometry(&bits), false, "seed")? {
            Some((_, score)) => population.push((bits, score.worst_db)),
            None if search.exhausted() => break,
            None => {}
        }
    }
    if population.is_empty() {
        return Err("no pixel candidate could be evaluated".to_owned());
    }
    while !search.exhausted() {
        population.sort_by(|a, b| a.1.total_cmp(&b.1));
        let pick = |rng: &mut Rng, population: &[(Vec<bool>, f64)]| {
            let (a, b) = (rng.below(population.len()), rng.below(population.len()));
            population[a.min(b)].0.clone()
        };
        let (mother, father) = (pick(&mut rng, &population), pick(&mut rng, &population));
        let mut child: Vec<bool> =
            mother.iter().zip(&father).map(|(m, f)| if rng.next() & 1 == 0 { *m } else { *f }).collect();
        mutate(&mut child, &mut rng);
        if let Some((_, score)) = search.run(&geometry(&child), false, "evolve")? {
            population.push((child, score.worst_db));
            population.sort_by(|a, b| a.1.total_cmp(&b.1));
            population.truncate(population_size);
        }
    }
    population.sort_by(|a, b| a.1.total_cmp(&b.1));
    let best = geometry(&population[0].0);
    let (mut evaluation, mut score) = search.cache[&(best.to_source(), "coarse")].clone();
    if options.fine {
        (evaluation, score) = search.run(&best, true, "verify")?.expect("a realizable geometry");
    }
    let realized = layout(site, &best).map_err(|e| e.to_string())?;
    Ok(Outcome { geometry: best, layout: realized, evaluation, score, trials: search.trials, method: "pixel-ga" })
}

/// Evaluate a site's current geometry once at `fine` or coarse fidelity.
pub fn verify(
    problem: &Problem,
    geometry: &Geometry,
    fine: bool,
    threads: usize,
) -> Result<(Evaluation, Score), String> {
    let site = problem.site;
    let realized = layout(site, geometry).map_err(|e| e.to_string())?;
    let fidelity = if fine { Fidelity::fine(site, threads) } else { Fidelity::coarse(site, threads) };
    let evaluation = evaluate(problem, &realized, &fidelity)?;
    let score = score(site, &evaluation);
    Ok((evaluation, score))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nelder_mead_finds_a_bowl_minimum() {
        let (x, value) = nelder_mead(
            |x| Ok(Some((x[0] - 3_000_000.0).powi(2) + (x[1] - 18_000_000.0).powi(2) + (x[2] - 2_000_000.0).powi(2))),
            vec![1_000_000.0, 15_000_000.0, 1_000_000.0],
            vec![500_000.0, 500_000.0, 300_000.0],
            400,
        )
        .expect("a minimum");
        assert!(
            (x[0] - 3_000_000.0).abs() < 30_000.0
                && (x[1] - 18_000_000.0).abs() < 30_000.0
                && (x[2] - 2_000_000.0).abs() < 30_000.0,
            "{x:?} {value}"
        );
    }
}

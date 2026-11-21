//! The colour and dash of each step's curve, and the legend that explains them.
//!
//! Up to eight steps each get their own colour (and a dash as the second
//! encoding). Beyond eight, colours would repeat, so the steps are told apart
//! by their stepped params instead: the params are grouped into factors
//! (params that move together, as in a zipped step, are one factor), the
//! factor with the most levels up to eight sets the colour, and the
//! combinations of the other factors set the dash when there are at most four
//! of them (else the largest other factor with at most four levels does).
//! A factor with more than eight levels and nothing smaller is drawn on a
//! sequential ramp in its own order. Steps the encoding does not separate
//! share a style; the interactive page tells them apart in its cursor readout
//! and toggles them by factor level.

use crate::run::BenchRun;

/// Step colours in fixed order (blue, aqua, purple, yellow, pink, green, teal, ochre): checked for
/// lightness, chroma, colour-vision-deficiency and normal-vision separation of neighbours on white.
/// None is the warning colour, which marks failures only.
pub const STEP_COLORS: [&str; 8] =
    ["#1d5fa8", "#1baf7a", "#8e44ad", "#eda100", "#e87ba4", "#008300", "#2a9d8f", "#b7791f"];
/// Dash patterns, the second encoding of a step.
pub const DASHES: [&str; 4] = ["", "8 4", "2 3", "10 3 2 3"];
/// A sequential ramp (dark blue to olive) for a factor of more than eight
/// ordered levels; every stop has enough contrast on white for a 2 px line.
const RAMP: [(f64, f64, f64); 5] = [
    (0x2c as f64, 0x1b as f64, 0x5e as f64),
    (0x3b as f64, 0x52 as f64, 0x8b as f64),
    (0x21 as f64, 0x91 as f64, 0x8c as f64),
    (0x3f as f64, 0xa3 as f64, 0x56 as f64),
    (0xa8 as f64, 0x98 as f64, 0x00 as f64),
];

/// Params that move together over the steps, and their levels.
#[derive(Clone, Debug, PartialEq)]
pub struct Factor {
    /// The params, in step order (`F_DRIVE, T_STOP`).
    pub names: Vec<String>,
    /// The distinct value combinations in order of first appearance (`1 kHz, 2 ms`).
    pub levels: Vec<String>,
    /// Each step's level.
    pub of_step: Vec<usize>,
}

/// What a step's colour or dash encodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Encoding {
    /// One style per step (up to eight steps).
    Step,
    /// The levels of these factors (their combinations when several), with
    /// each step's index into `levels`.
    Factors { factors: Vec<usize>, levels: Vec<String>, of_step: Vec<usize> },
    /// Nothing: every step alike.
    None,
}

/// The styles of a bench's steps.
#[derive(Clone, Debug, PartialEq)]
pub struct StepStyles {
    pub colors: Vec<String>,
    pub dashes: Vec<&'static str>,
    pub factors: Vec<Factor>,
    pub color: Encoding,
    pub dash: Encoding,
    /// The colour factor is drawn on a sequential ramp.
    pub ramp: bool,
}

fn ramp_color(t: f64) -> String {
    let t = t.clamp(0.0, 1.0) * (RAMP.len() - 1) as f64;
    let i = (t.floor() as usize).min(RAMP.len() - 2);
    let f = t - i as f64;
    let mix = |a: f64, b: f64| (a + (b - a) * f).round() as u8;
    let (a, b) = (RAMP[i], RAMP[i + 1]);
    format!("#{:02x}{:02x}{:02x}", mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// The value texts of a step's stepped params.
fn values(run: &BenchRun, step: usize) -> Vec<String> {
    run.steps[step].stepped.iter().map(|(_, value)| value.to_string()).collect()
}

/// Group the stepped params into factors: two params are one factor when each one's value determines the other's.
pub fn factors(run: &BenchRun) -> Vec<Factor> {
    let Some(first) = run.steps.first() else { return Vec::new() };
    let names: Vec<String> = first.stepped.iter().map(|(name, _)| name.clone()).collect();
    let table: Vec<Vec<String>> = (0..run.steps.len()).map(|s| values(run, s)).collect();
    if table.iter().any(|row| row.len() != names.len()) {
        return Vec::new();
    }
    let determines = |a: usize, b: usize| {
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for row in &table {
            match seen.iter().find(|(x, _)| *x == row[a]) {
                Some((_, y)) if *y != row[b] => return false,
                Some(_) => {}
                None => seen.push((&row[a], &row[b])),
            }
        }
        true
    };
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..names.len() {
        match groups.iter_mut().find(|group| determines(group[0], i) && determines(i, group[0])) {
            Some(group) => group.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups
        .into_iter()
        .map(|group| {
            let mut levels: Vec<String> = Vec::new();
            let of_step = table
                .iter()
                .map(|row| {
                    let text = group.iter().map(|&i| row[i].as_str()).collect::<Vec<_>>().join(", ");
                    match levels.iter().position(|level| *level == text) {
                        Some(k) => k,
                        None => {
                            levels.push(text);
                            levels.len() - 1
                        }
                    }
                })
                .collect();
            Factor { names: group.iter().map(|&i| names[i].clone()).collect(), levels, of_step }
        })
        .collect()
}

/// The combined levels of several factors, per step.
fn combined(factors: &[Factor], chosen: &[usize], steps: usize) -> (Vec<String>, Vec<usize>) {
    let mut levels: Vec<String> = Vec::new();
    let of_step = (0..steps)
        .map(|s| {
            let text = chosen
                .iter()
                .map(|&f| factors[f].levels[factors[f].of_step[s]].as_str())
                .collect::<Vec<_>>()
                .join("; ");
            match levels.iter().position(|level| *level == text) {
                Some(k) => k,
                None => {
                    levels.push(text);
                    levels.len() - 1
                }
            }
        })
        .collect();
    (levels, of_step)
}

impl StepStyles {
    pub fn of(run: &BenchRun) -> StepStyles {
        let count = run.steps.len();
        let factors = factors(run);
        if count <= STEP_COLORS.len() || factors.is_empty() {
            return StepStyles {
                colors: (0..count).map(|k| STEP_COLORS[k % STEP_COLORS.len()].to_owned()).collect(),
                dashes: (0..count).map(|k| DASHES[(k + k / STEP_COLORS.len()) % DASHES.len()]).collect(),
                factors,
                color: Encoding::Step,
                dash: Encoding::Step,
                ramp: false,
            };
        }
        // Colour: the factor with the most levels up to eight, else the largest on a ramp.
        let fitting = (0..factors.len()).filter(|&f| factors[f].levels.len() <= STEP_COLORS.len());
        let by_levels = |f: &usize| factors[*f].levels.len();
        let (color_factor, ramp) = match fitting.clone().max_by_key(|f| (by_levels(f), std::cmp::Reverse(*f))) {
            Some(f) => (f, false),
            None => ((0..factors.len()).max_by_key(|f| (by_levels(f), std::cmp::Reverse(*f))).unwrap_or(0), true),
        };
        let levels = factors[color_factor].levels.len();
        let colors: Vec<String> = factors[color_factor]
            .of_step
            .iter()
            .map(|&level| {
                if ramp { ramp_color(level as f64 / (levels.max(2) - 1) as f64) } else { STEP_COLORS[level].to_owned() }
            })
            .collect();
        let color = Encoding::Factors {
            factors: vec![color_factor],
            levels: factors[color_factor].levels.clone(),
            of_step: factors[color_factor].of_step.clone(),
        };
        // Dash: the combinations of the other factors, or the largest of them that fits.
        let others: Vec<usize> = (0..factors.len()).filter(|&f| f != color_factor).collect();
        let (all_levels, all_steps) = combined(&factors, &others, count);
        let dash = if others.is_empty() {
            Encoding::None
        } else if all_levels.len() <= DASHES.len() {
            Encoding::Factors { factors: others.clone(), levels: all_levels, of_step: all_steps }
        } else {
            match others
                .iter()
                .copied()
                .filter(|&f| factors[f].levels.len() <= DASHES.len())
                .max_by_key(|f| (by_levels(f), std::cmp::Reverse(*f)))
            {
                Some(f) => Encoding::Factors {
                    factors: vec![f],
                    levels: factors[f].levels.clone(),
                    of_step: factors[f].of_step.clone(),
                },
                None => Encoding::None,
            }
        };
        let dashes = match &dash {
            Encoding::Factors { of_step, .. } => of_step.iter().map(|&level| DASHES[level]).collect(),
            _ => vec![DASHES[0]; count],
        };
        StepStyles { colors, dashes, factors, color, dash, ramp }
    }

    /// The colour and dash of step `k`.
    pub fn get(&self, k: usize) -> (&str, &'static str) {
        (self.colors.get(k).map(String::as_str).unwrap_or(STEP_COLORS[0]), self.dashes.get(k).copied().unwrap_or(""))
    }

    /// The names of the params an encoding shows (`F_DRIVE, T_STOP`).
    pub fn names(&self, encoding: &Encoding) -> String {
        match encoding {
            Encoding::Factors { factors, .. } => {
                factors.iter().map(|&f| self.factors[f].names.join(", ")).collect::<Vec<_>>().join("; ")
            }
            _ => String::new(),
        }
    }

    /// Whether each step has its own style (no legend by factor).
    pub fn per_step(&self) -> bool {
        self.color == Encoding::Step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_runs_from_its_first_to_its_last_stop() {
        assert_eq!(ramp_color(0.0), "#2c1b5e");
        assert_eq!(ramp_color(1.0), "#a89800");
        assert_eq!(ramp_color(0.5), "#21918c");
    }
}

//! Stability: the loop gain at a probe (Tian's method) and the poles and
//! zeros of the linearised circuit.
//!
//! # Loop gain
//!
//! [`loop_gain`] reads a loop at a probe the caller has inserted in it: a
//! 0 V voltage source in series with the loop, plus terminal `x`, minus
//! terminal `y`. The DC operating point does not change. At each frequency two
//! small-signal solves of the closed circuit, with the circuit's own AC
//! sources at zero, use the same factorisation:
//!
//! 1. a unit voltage at the probe (`V(x) - V(y) = 1`): `V1 = V(x)`, `I1` the
//!    probe's current from `x` through it to `y`;
//! 2. a unit current injected into `x` (the probe at 0 V): `V2`, `I2`.
//!
//! Tian's loop gain (M. Tian et al., "Striving for small-signal stability",
//! IEEE Circuits and Devices 17(1), 2001) is
//!
//! ```text
//! X = 2 (I1 V2 - V1 I2) + V1 + I2,   T = X / (1 - X)
//! ```
//!
//! In terms of the admittance parameters of the circuit opened at the probe
//! (port 1 at `x`, port 2 at `y`) it is `T = (Y12 + Y21) / (Y11 + Y22)`:
//! symmetric in the two ports, so the probe's orientation does not matter,
//! and the loading of both sides of the break is in it. Where the loop is
//! one-way at the break (`Y12 = 0` or `Y21 = 0`, as at the output of an
//! amplifier model) it equals Middlebrook's double-injection loop gain
//! `(Y21 - Y12) / (Y11 + Y22 + 2 Y12)`. `T` is positive at low frequency
//! for negative feedback; `1 + T = 0` marks oscillation. A circuit with
//! several loops through the probe gives their combined loop gain at that
//! point; loops that do not pass the probe stay closed.
//!
//! # Poles and zeros
//!
//! The small-signal MNA matrix at angular frequency `w` is exactly
//! `G + j w C` (capacitors, inductors, couplings and the charges of
//! nonlinear devices at their operating point make up `C`). [`pole_zero`]
//! takes `G` and `C` from one assembly at `w = 1` and finds the finite
//! generalised eigenvalues of the pencil `det(G + s C) = 0`
//! ([`crate::eigen::finite_eigenvalues`]: the infinite eigenvalues of the
//! algebraic unknowns deflated by orthogonal transformations, then the
//! eigenvalues of the reduced, nonsingular pencil by the shifted QR
//! iteration). The slow roots come from the inverted pencil instead, whose
//! largest eigenvalues they are ([`crate::eigen::with_slow_roots_inverted`]
//! on the reduced pencil, then on the whole one where its QR iteration
//! succeeds): in a stiff circuit the QR iteration's round-off is set by the
//! fastest roots and can put a root of a few rad/s on the wrong side of the
//! imaginary axis. Each eigenvalue is then refined by Newton's method on the
//! original complex pencil (the bordered system `[G + s C, C x; e_k^T, 0]`,
//! started from inverse iteration), which also gives its eigenvector: the
//! nodes whose small-signal voltage is largest in that mode are the nodes the
//! pole dominates. Where that refinement does not converge (a repeated
//! pole: identical sub-circuits) or runs to another eigenvalue, inverse
//! iteration at a fixed shift beside the QR value finds the eigenvalue
//! nearest it, independently of the QR iteration, and certifies it when its
//! backward error `||(G + s C) x|| / ((||G|| + |s| ||C||) ||x||)` is within
//! [`CERTIFIED`] ([`nearest_eigenvalue`]): a QR value of a stiff pencil with
//! no eigenvalue near it is replaced by the one that is there, and a real one
//! is confirmed. A pole neither method certifies keeps its QR value and is
//! marked unrefined; roots at the origin are not refined.
//!
//! The zeros of a transfer function `V(out) - V(ref)` per unit of an input
//! source are the finite eigenvalues of the bordered pencil
//! `[[G + s C, b], [e^T, 0]]` (`b` the input's column of the right-hand side,
//! `e` the output's row), whose determinant is `-det(G + s C) H(s)`. A mode
//! the input does not excite or the output does not see appears as a pole and
//! as a zero at the same place. Zeros keep their QR values (not refined):
//! where the gain is small the bordered system is too ill-conditioned for
//! Newton's method to improve on them; in the stiff circuit of the tests
//! they rebuild the transfer function within 5e-8 to 7e-4 of the AC analysis
//! (the poles alone are refined to round-off).
//!
//! A pole is in the right half-plane when its real part exceeds
//! [`RHP_TOLERANCE`] of its magnitude ([`UNREFINED_RHP`] unrefined). As many
//! roots as `G` has null directions, the smallest, are at the origin
//! ([`ORIGIN_RANK_TOLERANCE`]): a node with only capacitors to it (an
//! electrode in air) or an ideal integrator; such a root is reported, has no
//! damping, and does not make the circuit unstable.
//!
//! The analysis is dense, `O(n^3)` in the number of unknowns, and refuses
//! circuits of more than [`MAX_POLE_ZERO_UNKNOWNS`].

use crate::analysis::{Sweep, solve_point};
use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::complex::Complex;
use crate::diagnose::diagnose;
use crate::eigen::{EigenError, Mat, finite_eigenvalues};
use crate::error::{AnalysisPoint, SimError};
use crate::lu::{Lu, LuError};
use crate::mna::{Layout, Mode, Unknown, assemble};
use crate::sparse_lu::{SparseLu, SparseMatrix, factor_reusing};
use crate::timing::{self, Phase};
use std::f64::consts::PI;

/// The largest circuit (in unknowns) [`pole_zero`] analyses.
pub const MAX_POLE_ZERO_UNKNOWNS: usize = 1500;

/// A pole with a real part above this fraction of its magnitude is in the
/// right half-plane (refined poles; unrefined ones need [`UNREFINED_RHP`]).
pub const RHP_TOLERANCE: f64 = 1e-9;
/// The same for a pole whose refinement did not converge.
pub const UNREFINED_RHP: f64 = 1e-6;
/// Roots at the origin come from a singular `G`: a node without a DC path
/// (only capacitors to it) or an ideal integrator. Their number is the
/// nullity of `G` (pivoted QR, rows scaled to a unit largest entry, this rank
/// tolerance), and they are the roots of smallest magnitude, which round-off
/// leaves a hair to either side of zero (a double root by up to the square
/// root of round-off); a root counts only when its magnitude is also at most
/// [`ORIGIN_TOLERANCE`] of the norm of the QR iteration's matrix.
pub const ORIGIN_RANK_TOLERANCE: f64 = 1e-13;
/// See [`ORIGIN_RANK_TOLERANCE`].
pub const ORIGIN_TOLERANCE: f64 = 1e-6;

/// Result of [`loop_gain`].
#[derive(Debug, Clone, PartialEq)]
pub struct LoopGainResult {
    /// The probe element.
    pub probe: String,
    /// The frequencies, Hz.
    pub frequencies: Vec<f64>,
    /// Tian's loop gain at each frequency.
    pub gain: Vec<Complex>,
}

/// Solve `a x = b` for several right-hand sides, sparse first, dense when the sparse LU refuses.
fn solve_many(
    c: &Circuit,
    layout: &Layout,
    a: &[Complex],
    rhs: &[Vec<Complex>],
    at: AnalysisPoint,
    factors: &mut Option<SparseLu<Complex>>,
) -> Result<Vec<Vec<Complex>>, SimError> {
    let n = layout.size();
    let sparse = SparseMatrix::from_dense(a, n);
    let factored = {
        let _span = timing::span(Phase::Factor);
        factor_reusing(&sparse, factors.take())
    };
    let _span = timing::span(Phase::Solve);
    match factored {
        Some(lu) => {
            let out = rhs.iter().map(|b| lu.solve(b).ok_or(SimError::NonFinite(at))).collect();
            *factors = Some(lu);
            out
        }
        None => {
            let lu = Lu::factor(a.to_vec(), n).map_err(|e| match e {
                LuError::NonFinite => SimError::NonFinite(at),
                _ => SimError::Singular { at, causes: diagnose(c, layout, at.is_dc(), e) },
            })?;
            rhs.iter().map(|b| lu.solve(b).ok_or(SimError::NonFinite(at))).collect()
        }
    }
}

/// The loop gain at `probe`, a voltage source in series with the loop (see
/// the module docs), over `sweep`. A circuit with diodes, switches or
/// devices is linearised at its operating point first, as [`crate::ac`] does.
pub fn loop_gain(circuit: &Circuit, sweep: &Sweep, probe: &str) -> Result<LoopGainResult, SimError> {
    let frequencies = sweep.frequencies()?;
    let linearized;
    let circuit = if circuit.is_nonlinear() {
        linearized = crate::newton::linearized(circuit)?;
        &linearized
    } else {
        circuit
    };
    let id = circuit.find_element(probe).ok_or_else(|| SimError::UnknownElement(probe.to_string()))?;
    let ElementKind::VoltageSource { plus, .. } = *circuit.element(id).kind() else {
        return Err(SimError::BadReference {
            element: probe.to_string(),
            detail: "a loop-gain probe must be a voltage source in series with the loop".to_string(),
        });
    };
    let layout = Layout::of(circuit);
    let branch = layout.branch_row[id.index()].expect("a voltage source has a branch current");
    let Some(x) = (if plus.is_ground() { None } else { layout.node_row[plus.index()] }) else {
        return Err(SimError::BadReference {
            element: probe.to_string(),
            detail: "the probe's plus terminal must be a node of the loop, not the ground".to_string(),
        });
    };
    let n = layout.size();
    let mut gain = Vec::with_capacity(frequencies.len());
    let mut factors = None;
    for &f in &frequencies {
        let at = AnalysisPoint::Ac { frequency: f };
        let a = {
            let _span = timing::span(Phase::Assembly);
            assemble(circuit, &layout, Mode::Ac { omega: 2.0 * PI * f }).a
        };
        let mut voltage = vec![Complex::ZERO; n];
        voltage[branch] = Complex::ONE;
        let mut current = vec![Complex::ZERO; n];
        current[x] = Complex::ONE;
        let solved = solve_many(circuit, &layout, &a, &[voltage, current], at, &mut factors)?;
        let (v1, i1) = (solved[0][x], solved[0][branch]);
        let (v2, i2) = (solved[1][x], solved[1][branch]);
        let big_x = (i1 * v2 - v1 * i2).scale(2.0) + v1 + i2;
        let t = big_x / (Complex::ONE - big_x);
        if !t.is_finite() {
            return Err(SimError::NonFinite(at));
        }
        gain.push(t);
    }
    Ok(LoopGainResult { probe: probe.to_string(), frequencies, gain })
}

/// A transfer function for [`pole_zero`]: `V(output) - V(reference)` per unit of `input`.
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    /// An independent source element, or a `source` macro instance.
    pub input: String,
    /// The output node.
    pub output: String,
    /// The output's reference node (`None` is ground).
    pub reference: Option<String>,
}

/// A pole or a zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    /// The root `s`, rad/s.
    pub value: Complex,
    /// Whether Newton refinement on the circuit's pencil converged (always false for a zero).
    pub refined: bool,
    /// For a pole: the nodes with the largest small-signal voltage in its mode,
    /// with their magnitude relative to the largest (at least 0.5), largest first.
    pub nodes: Vec<(String, f64)>,
    /// The root is at the origin (see [`ORIGIN_TOLERANCE`]).
    pub origin: bool,
}

impl Root {
    /// `s / (2 pi)`: the root in hertz.
    pub fn hertz(&self) -> Complex {
        self.value.scale(0.5 / PI)
    }

    /// The damping ratio `-Re(s) / |s|`: 1 for a real pole in the left
    /// half-plane, between 0 and 1 for a decaying complex pair, negative in the
    /// right half-plane; `None` at the origin.
    pub fn damping(&self) -> Option<f64> {
        let m = self.value.abs();
        (m > 0.0 && !self.origin).then(|| -self.value.re / m)
    }

    /// Whether the root is in the right half-plane (see [`RHP_TOLERANCE`]);
    /// a root at the origin is not.
    pub fn right_half_plane(&self) -> bool {
        let tolerance = if self.refined { RHP_TOLERANCE } else { UNREFINED_RHP };
        !self.origin && self.value.re > tolerance * self.value.abs()
    }
}

/// Result of [`pole_zero`].
#[derive(Debug, Clone, PartialEq)]
pub struct PoleZeroResult {
    /// The finite poles, by magnitude; a complex pair is the upper one then its conjugate.
    pub poles: Vec<Root>,
    /// The finite zeros of the transfer function, when one was given.
    pub zeros: Vec<Root>,
    /// The transfer function the zeros belong to.
    pub transfer: Option<Transfer>,
    /// The number of unknowns of the circuit.
    pub unknowns: usize,
    /// The infinite eigenvalues: unknowns without dynamics of their own.
    pub infinite: usize,
}

impl PoleZeroResult {
    /// True when no pole is in the right half-plane (poles at the origin do not count).
    pub fn stable(&self) -> bool {
        !self.poles.iter().any(Root::right_half_plane)
    }

    /// The smallest damping ratio of the poles (negative with a pole in the
    /// right half-plane); `None` without poles off the origin.
    pub fn damping(&self) -> Option<f64> {
        self.poles.iter().filter_map(Root::damping).reduce(f64::min)
    }
}

/// The real pencil `G + s C` of the small-signal matrix (`A(j w) = G + j w C`).
fn pencil(c: &Circuit, layout: &Layout) -> (Mat, Mat) {
    let n = layout.size();
    let a = assemble(c, layout, Mode::Ac { omega: 1.0 }).a;
    (
        Mat { rows: n, cols: n, a: a.iter().map(|z| z.re).collect() },
        Mat { rows: n, cols: n, a: a.iter().map(|z| z.im).collect() },
    )
}

fn eigen_error(error: EigenError) -> SimError {
    SimError::PoleZero(match error {
        EigenError::NoConvergence => "the QR iteration did not converge".to_string(),
        EigenError::SingularPencil => {
            "the circuit's equations are singular at every frequency, so it has no defined poles".to_string()
        }
        EigenError::NonFinite => "a matrix entry is not finite".to_string(),
    })
}

/// The complex matrix `G + s C`, row-major.
fn at(g: &Mat, c: &Mat, s: Complex) -> Vec<Complex> {
    g.a.iter().zip(&c.a).map(|(g, c)| Complex::new(*g, 0.0) + s.scale(*c)).collect()
}

/// Factor a complex square matrix for the refinement, sparse first, dense
/// when the sparse LU refuses.
enum Factors {
    Sparse(Box<SparseLu<Complex>>),
    Dense(Lu),
}

impl Factors {
    fn of(a: &[Complex], n: usize) -> Option<Factors> {
        if let Ok(lu) = SparseLu::factor(&SparseMatrix::from_dense(a, n)) {
            return Some(Factors::Sparse(Box::new(lu)));
        }
        Lu::factor(a.to_vec(), n).ok().map(Factors::Dense)
    }

    fn solve(&self, b: &[Complex]) -> Option<Vec<Complex>> {
        match self {
            Factors::Sparse(lu) => lu.solve(b),
            Factors::Dense(lu) => lu.solve(b),
        }
    }
}

/// The index of the largest entry.
fn largest(x: &[Complex]) -> usize {
    x.iter().enumerate().fold((0, -1.0), |best, (k, z)| if z.abs() > best.1 { (k, z.abs()) } else { best }).0
}

/// Newton refinement of the eigenvalue `guess` of `G + s C`; `spectral` is
/// the largest eigenvalue's magnitude, the scale near the origin. Returns
/// the converged value (or `None`) and the eigenvector (inverse iteration's
/// when Newton does not converge).
fn refine(g: &Mat, c: &Mat, guess: Complex, spectral: f64) -> (Option<Complex>, Option<Vec<Complex>>) {
    let n = g.rows;
    let size = guess.abs().max(1e-9 * spectral).max(f64::MIN_POSITIVE);
    // Inverse iteration from just beside the guess (off the real axis, so a real guess is not hit exactly).
    let shift = guess + Complex::new(0.6, 0.8).scale(1e-6 * size);
    let Some(lu) = Factors::of(&at(g, c, shift), n) else { return (None, None) };
    let mut x: Vec<Complex> = (0..n).map(|k| Complex::new(1.0 + 0.1 * (k % 7) as f64, 0.0)).collect();
    for _ in 0..2 {
        let Some(y) = lu.solve(&x) else { return (None, None) };
        let k = largest(&y);
        let pivot = y[k];
        if pivot.abs() == 0.0 {
            return (None, None);
        }
        x = y.iter().map(|z| *z / pivot).collect();
    }
    let mut lambda = guess;
    let k = largest(&x);
    let initial = x.clone();
    let scale = guess.abs().max(1e-3 * spectral);
    let mut previous = f64::INFINITY;
    for _ in 0..20 {
        // [G + l C, C x; e_k^T, 0] [dx; dl] = -[(G + l C) x; 0]
        let a = at(g, c, lambda);
        let m = n + 1;
        let mut j = vec![Complex::ZERO; m * m];
        let mut residual = vec![Complex::ZERO; m];
        for r in 0..n {
            let mut cx = Complex::ZERO;
            let mut ax = Complex::ZERO;
            for col in 0..n {
                let v = a[r * n + col];
                j[r * m + col] = v;
                ax += v * x[col];
                cx += x[col].scale(c.a[r * n + col]);
            }
            j[r * m + n] = cx;
            residual[r] = -ax;
        }
        j[n * m + k] = Complex::ONE;
        let Some(step) = Factors::of(&j, m).and_then(|f| f.solve(&residual)) else { break };
        for (xi, d) in x.iter_mut().zip(&step) {
            *xi += *d;
        }
        let dl = step[n];
        lambda += dl;
        if !lambda.is_finite() {
            break;
        }
        // Converged: a step at round-off, or steps that stopped shrinking
        // once they are below 1e-10 of the root (round-off of a less well
        // conditioned root).
        let step_size = dl.abs();
        if step_size <= 1e-13 * scale || (step_size <= 1e-10 * scale && step_size >= 0.5 * previous) {
            return (Some(lambda), Some(x));
        }
        previous = step_size;
    }
    (None, Some(initial))
}

/// The backward error below which [`nearest_eigenvalue`] accepts an
/// eigenvalue: `||(G + s C) x|| <= CERTIFIED (||G|| + |s| ||C||) ||x||`
/// (maximum norms), `s` an exact eigenvalue of a pencil that close to this one.
pub const CERTIFIED: f64 = 1e-11;

/// Inverse iterations of [`nearest_eigenvalue`].
const INVERSE_ITERATIONS: usize = 60;

/// The eigenvalue of `G + s C` nearest `guess`, independently of the QR
/// iteration that produced the guess: inverse iteration at a fixed shift just
/// beside `guess`, which converges to the eigenvalue nearest the shift (the
/// infinite ones are never approached), then Newton's refinement from it when
/// that converges (a simple root), else the iterate's residual-minimising
/// value. Returned with its eigenvector when its backward error is within
/// [`CERTIFIED`]: a repeated root (identical sub-circuits) is certified where
/// Newton's bordered system is singular, and a QR value with no eigenvalue
/// near it (round-off of a stiff pencil) is replaced by the eigenvalue that
/// is there.
fn nearest_eigenvalue(g: &Mat, c: &Mat, guess: Complex, spectral: f64) -> Option<(Complex, Vec<Complex>)> {
    let n = g.rows;
    let size = guess.abs().max(1e-9 * spectral).max(f64::MIN_POSITIVE);
    let shift = guess + Complex::new(0.6, 0.8).scale(1e-7 * size);
    let lu = Factors::of(&at(g, c, shift), n)?;
    let apply_c = |x: &[Complex]| -> Vec<Complex> {
        (0..n).map(|r| (0..n).fold(Complex::ZERO, |acc, col| acc + x[col].scale(c.a[r * n + col]))).collect()
    };
    let mut x: Vec<Complex> = (0..n).map(|k| Complex::new(1.0 + 0.1 * (k % 7) as f64, 0.01 * (k % 3) as f64)).collect();
    let mut value = guess;
    for _ in 0..INVERSE_ITERATIONS {
        let y = lu.solve(&apply_c(&x))?;
        let pivot = y[largest(&y)];
        if pivot.abs() == 0.0 || !pivot.is_finite() {
            return None;
        }
        // y = x / (shift - s) for an eigenvector x: theta from the largest entry.
        let theta = pivot / x[largest(&y)];
        x = y.iter().map(|z| *z / pivot).collect();
        if theta.abs() == 0.0 || !theta.is_finite() {
            return None;
        }
        // Every iteration runs: the value settles long before the vector does.
        value = shift - Complex::ONE / theta;
    }
    // The value that minimises the residual of x: s = -(Cx)^H (Gx) / |Cx|^2.
    let cx = apply_c(&x);
    let gx: Vec<Complex> =
        (0..n).map(|r| (0..n).fold(Complex::ZERO, |acc, col| acc + x[col].scale(g.a[r * n + col]))).collect();
    let (num, den) = cx
        .iter()
        .zip(&gx)
        .fold((Complex::ZERO, 0.0), |(num, den), (cv, gv)| (num + cv.conj() * *gv, den + cv.abs() * cv.abs()));
    if den == 0.0 {
        return None;
    }
    let iterated = value;
    value = Complex::ZERO - num.scale(1.0 / den);
    let norm = |m: &Mat| (0..n).map(|r| (0..n).map(|col| m.a[r * n + col].abs()).sum::<f64>()).fold(0.0, f64::max);
    let (g_norm, c_norm) = (norm(g), norm(c));
    // The backward error of (s, x): ||(G + s C) x|| / ((||G|| + |s| ||C||) ||x||).
    let backward = |s: Complex, x: &[Complex]| -> f64 {
        let residual = (0..n)
            .map(|r| {
                (0..n).fold(Complex::ZERO, |acc, col| {
                    acc + x[col] * (Complex::new(g.a[r * n + col], 0.0) + s.scale(c.a[r * n + col]))
                })
            })
            .map(|z| z.abs())
            .fold(0.0, f64::max);
        let x_norm = x.iter().map(|z| z.abs()).fold(0.0, f64::max);
        residual / ((g_norm + s.abs() * c_norm) * x_norm)
    };
    let mut error = backward(value, &x);
    // The iteration's own value where it does better (round-off in the quotient).
    let iterated_error = backward(iterated, &x);
    if iterated_error < error {
        (value, error) = (iterated, iterated_error);
    }
    // Newton's refinement where it does better (a simple root); on a repeated
    // root its bordered system is singular and it does not.
    if let (Some(newton), Some(vector)) = refine(g, c, value, spectral)
        && (newton - value).abs() <= 1e-6 * size
    {
        let newton_error = backward(newton, &vector);
        if newton_error < error {
            (value, x, error) = (newton, vector, newton_error);
        }
    }
    (error <= CERTIFIED && value.is_finite()).then_some((value, x))
}

/// The nodes with the largest voltage in the mode `x` (at least half the largest).
fn dominant(c: &Circuit, layout: &Layout, x: &[Complex]) -> Vec<(String, f64)> {
    let mut nodes: Vec<(String, f64)> = layout
        .unknowns
        .iter()
        .enumerate()
        .filter_map(|(k, unknown)| match unknown {
            Unknown::Node(node) => x.get(k).map(|z| (c.node_name(*node).to_string(), z.abs())),
            Unknown::Branch(_) => None,
        })
        .collect();
    let big = nodes.iter().fold(0.0f64, |m, (_, v)| m.max(*v));
    if big == 0.0 || !big.is_finite() {
        return Vec::new();
    }
    nodes.retain(|(_, v)| *v >= 0.5 * big);
    nodes.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    nodes.truncate(4);
    nodes.into_iter().map(|(name, v)| (name, v / big)).collect()
}

/// Refine every root of `G + s C` and sort them by magnitude, a complex pair
/// upper first; with `layout`, record the nodes each mode dominates. `scale`
/// is the norm of the QR iteration's matrix, for [`ORIGIN_TOLERANCE`].
fn roots(g: &Mat, c: &Mat, values: Vec<Complex>, node_names: Option<(&Circuit, &Layout)>, scale: f64) -> Vec<Root> {
    // Zeros (no node names) keep their QR values: the bordered system of a
    // transfer function with a small gain is too ill-conditioned for Newton's
    // method to improve on them.
    let refining = node_names.is_some();
    let spectral = values.iter().fold(0.0f64, |m, z| m.max(z.abs()));
    let _span = timing::span(Phase::Solve);
    let mut out: Vec<Root> = Vec::with_capacity(values.len());
    let mut skip = vec![false; values.len()];
    for (i, guess) in values.iter().enumerate() {
        if skip[i] {
            continue;
        }
        // The QR iteration returns a complex pair as conjugates: refine the upper one.
        let pair =
            (guess.im != 0.0).then(|| (i + 1..values.len()).find(|&j| !skip[j] && values[j] == guess.conj())).flatten();
        if let Some(j) = pair {
            skip[j] = true;
        }
        let upper = if guess.im < 0.0 { guess.conj() } else { *guess };
        let (refined, vector) = if refining { refine(g, c, upper, spectral) } else { (None, None) };
        // A refinement counts when it stays nearest the eigenvalue it started
        // from (and a real one stays real): one that ran to another
        // eigenvalue is dropped.
        let refined = refined.filter(|r| {
            if guess.im == 0.0 && r.im.abs() > 1e-9 * r.abs() {
                return false;
            }
            let own = (*r - upper).abs().min((*r - upper.conj()).abs());
            values
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i && Some(*k) != pair)
                .all(|(_, other)| (*r - *other).abs() > own)
        });
        // Where Newton did not converge or ran to another root: the eigenvalue
        // inverse iteration finds nearest the QR value, if certified.
        let (refined, vector) = match refined {
            // Roots at the origin (floating nodes) are told apart below, by G's nullity.
            None if refining && upper.abs() > ORIGIN_TOLERANCE * scale => {
                match nearest_eigenvalue(g, c, upper, spectral) {
                    Some((value, x)) => (Some(value), Some(x)),
                    None => (None, vector),
                }
            }
            refined => (refined, vector),
        };
        let mut value = refined.unwrap_or(upper);
        if guess.im == 0.0 && value.im.abs() <= 1e-9 * value.abs() {
            value.im = 0.0;
        }
        if value.im.abs() <= 1e-9 * value.abs() {
            value.im = 0.0;
        }
        let nodes = match (node_names, &vector) {
            (Some((circuit, layout)), Some(x)) => dominant(circuit, layout, x),
            _ => Vec::new(),
        };
        let refined = refined.is_some();
        let origin = false;
        if pair.is_some() {
            value.im = value.im.abs();
            out.push(Root { value, refined, nodes: nodes.clone(), origin });
            out.push(Root { value: value.conj(), refined, nodes, origin });
        } else {
            out.push(Root { value, refined, nodes, origin });
        }
    }
    out.sort_by(|a, b| {
        a.value
            .abs()
            .total_cmp(&b.value.abs())
            .then(b.value.im.total_cmp(&a.value.im))
            .then(a.value.re.total_cmp(&b.value.re))
    });
    // The roots at the origin: as many of the smallest as G's nullity.
    let nullity = g_nullity(g);
    for root in out.iter_mut().take(nullity) {
        if root.value.abs() <= ORIGIN_TOLERANCE * scale {
            root.origin = true;
        }
    }
    out
}

/// The relative size, against the largest, from which an eigenvalue `mu` of
/// `-G^-1 C` gives a slow root `s = 1 / mu` more accurately than the QR
/// iteration on the pencil does ([`with_slow_roots_inverted`]).
pub const INVERTED_SLOW: f64 = crate::eigen::INVERTED_SLOW;

/// `direct`, the finite roots of `G + s C` from [`finite_eigenvalues`] (whose
/// own slow roots come from its deflated pencil inverted, accurate to about
/// 1e-4 there), with
/// its slow roots taken from the eigenvalues `mu` of `-G^-1 C` instead (`s =
/// 1 / mu`), when `G` is nonsingular. The QR iteration's absolute round-off is
/// a multiple of `1e-16` times the norm of its balanced matrix, which in a
/// stiff pencil (conductances from 1e-12 to 1e6 S beside microfarad couplings,
/// as in vendor op-amp models with 1 uohm and 1 Tohm resistors) is set by the
/// fastest roots, `1e15` rad/s and more: a root of a few rad/s can then come
/// out on the wrong side of the imaginary axis, and a repeated one (two equal
/// coupling networks) far off, where Newton's refinement does not converge.
/// Inverted, the slowest roots are the largest `mu`, which the QR iteration
/// gives to round-off relative to themselves. The `k` roots with `|mu|` at
/// least [`INVERTED_SLOW`] of the largest replace the `k` smallest direct
/// roots; the others stay. With a singular `G` (a node with only capacitors
/// to it) the roots at the origin keep their direct values and the pencil is
/// inverted at a shift `sigma` a hundredth of the slowest other root (`s =
/// sigma + 1 / mu`).
fn full_pencil_slow_roots(g: &Mat, c: &Mat, direct: Vec<Complex>) -> Vec<Complex> {
    let n = g.rows;
    let mut sorted = direct.clone();
    sorted.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
    // Roots at the origin (a singular G) keep their direct values; the shift
    // that makes G + sigma C invertible is then well below the slowest other root.
    let nullity = g_nullity(g);
    let sigma = match (nullity, sorted.get(nullity)) {
        (0, _) => 0.0,
        (_, Some(slowest)) if slowest.abs() > 0.0 => 1e-2 * slowest.abs(),
        _ => return direct,
    };
    let shifted: Vec<f64> = g.a.iter().zip(&c.a).map(|(g, c)| g + sigma * c).collect();
    let Ok(lu) = crate::real_lu::RealLu::factor(shifted, n) else { return direct };
    let mut m = Mat::zeros(n, n);
    for j in 0..n {
        let column: Vec<f64> = (0..n).map(|i| -c.a[i * n + j]).collect();
        let Some(x) = lu.solve(&column) else { return direct };
        for (i, v) in x.iter().enumerate() {
            m.a[i * n + j] = *v;
        }
    }
    let Ok(mu) = crate::eigen::eigenvalues(&m) else { return direct };
    // s = sigma + 1 / mu, the origin roots (the largest mu when sigma > 0) left out.
    let mut inverted: Vec<(f64, Complex)> = mu
        .iter()
        .filter(|z| z.abs() > 0.0 && z.is_finite())
        .map(|z| (z.abs(), Complex::new(sigma, 0.0) + Complex::ONE / *z))
        .collect();
    inverted.sort_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
    if inverted.len() < nullity {
        return direct;
    }
    let others = &inverted[nullity..];
    let largest = others.iter().fold(0.0f64, |a, (m, _)| a.max(*m));
    if largest == 0.0 || !largest.is_finite() {
        return direct;
    }
    let mut slow: Vec<Complex> =
        others.iter().filter(|(m, _)| *m >= INVERTED_SLOW * largest).map(|(_, s)| *s).collect();
    if slow.is_empty() || nullity + slow.len() > direct.len() {
        return direct;
    }
    for s in &mut slow {
        if s.im.abs() <= 1e-12 * s.abs() {
            s.im = 0.0;
        }
    }
    let mut out: Vec<Complex> = sorted[..nullity].to_vec();
    out.extend(slow.iter().copied());
    out.extend(sorted.into_iter().skip(nullity + slow.len()));
    out
}

/// The nullity of `G`: pivoted QR with rows scaled to a unit largest entry,
/// at [`ORIGIN_RANK_TOLERANCE`] (the number of roots at the origin).
fn g_nullity(g: &Mat) -> usize {
    let mut rows = g.clone();
    for i in 0..rows.rows {
        let big = (0..rows.cols).fold(0.0f64, |m, j| m.max(rows.a[i * rows.cols + j].abs()));
        if big > 0.0 {
            for j in 0..rows.cols {
                rows.a[i * rows.cols + j] /= big;
            }
        }
    }
    rows.rows - crate::eigen::qr_pivoted(&rows, ORIGIN_RANK_TOLERANCE, 0.0).rank
}

/// The poles of `circuit` linearised at its operating point, and the zeros of
/// `transfer` when given (see the module docs).
pub fn pole_zero(circuit: &Circuit, transfer: Option<&Transfer>) -> Result<PoleZeroResult, SimError> {
    if let Some(e) = circuit.elements().iter().find(|e| matches!(e.kind(), crate::circuit::ElementKind::Delay { .. })) {
        return Err(SimError::PoleZero(format!(
            "{} is an ideal delay (a transmission line, a LAPLACE exp(-s T) or a DELAY()), which has infinitely many poles; use analysis loop or ac",
            e.name()
        )));
    }
    let linearized;
    let circuit = if circuit.is_nonlinear() {
        linearized = crate::newton::linearized(circuit)?;
        &linearized
    } else {
        circuit
    };
    let layout = Layout::of(circuit);
    let n = layout.size();
    if n > MAX_POLE_ZERO_UNKNOWNS {
        return Err(SimError::PoleZero(format!(
            "the circuit has {n} unknowns; the dense pole-zero analysis takes at most {MAX_POLE_ZERO_UNKNOWNS}"
        )));
    }
    // A circuit that cannot be solved names its cause as the AC analysis does.
    let frequency = 0.5 / PI;
    solve_point(circuit, &layout, Mode::Ac { omega: 1.0 }, AnalysisPoint::Ac { frequency }, &mut None)?;
    let (g, c) = {
        let _span = timing::span(Phase::Assembly);
        pencil(circuit, &layout)
    };
    let spectrum = {
        let _span = timing::span(Phase::Factor);
        finite_eigenvalues(&g, &c).map_err(eigen_error)?
    };
    let (infinite, scale) = (spectrum.infinite, spectrum.norm);
    let values = full_pencil_slow_roots(&g, &c, spectrum.values);
    let poles = roots(&g, &c, values, Some((circuit, &layout)), scale);
    let zeros = match transfer {
        None => Vec::new(),
        Some(t) => {
            let row = |name: &str| -> Result<Option<usize>, SimError> {
                let node = circuit.find_node(name).ok_or_else(|| SimError::UnknownNode(name.to_string()))?;
                if node.is_ground() {
                    return Ok(None);
                }
                layout.node_row[node.index()].map(Some).ok_or_else(|| SimError::NotConnected(name.to_string()))
            };
            let (out, reference) = (row(&t.output)?, t.reference.as_deref().map(row).transpose()?.flatten());
            let (stamp, _) = crate::noise::input_stamp(circuit, &layout, &t.input)?;
            let mut b = vec![0.0; n];
            let node_row = |node: NodeId| if node.is_ground() { None } else { layout.node_row[node.index()] };
            match stamp {
                crate::noise::InputStamp::Branch(Some(k)) => b[k] = 1.0,
                crate::noise::InputStamp::Branch(None) => {}
                crate::noise::InputStamp::Current(p, q) => {
                    if let Some(r) = node_row(p) {
                        b[r] -= 1.0;
                    }
                    if let Some(r) = node_row(q) {
                        b[r] += 1.0;
                    }
                }
            }
            let m = n + 1;
            let mut gb = Mat::zeros(m, m);
            let mut cb = Mat::zeros(m, m);
            for (r, input) in b.iter().enumerate() {
                for col in 0..n {
                    gb.a[r * m + col] = g.a[r * n + col];
                    cb.a[r * m + col] = c.a[r * n + col];
                }
                gb.a[r * m + n] = *input;
            }
            if let Some(r) = out {
                gb.a[n * m + r] += 1.0;
            }
            if let Some(r) = reference {
                gb.a[n * m + r] -= 1.0;
            }
            if b.iter().all(|x| *x == 0.0) || gb.a[n * m..].iter().all(|x| *x == 0.0) {
                return Err(SimError::PoleZero(
                    "the transfer function is zero: its input or its output is the ground".to_string(),
                ));
            }
            let zeros = finite_eigenvalues(&gb, &cb).map_err(|error| match error {
                EigenError::SingularPencil => SimError::PoleZero(
                    "the transfer function is zero at every frequency: the output does not depend on the input"
                        .to_string(),
                ),
                other => eigen_error(other),
            })?;
            roots(&gb, &cb, zeros.values, None, zeros.norm)
        }
    };
    Ok(PoleZeroResult { poles, zeros, transfer: transfer.cloned(), unknowns: n, infinite })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::SourceValue;
    use crate::models::{OpampParams, OpampPins};

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * a.abs().max(b.abs())
    }

    /// Four identical RC sections from one source and two identical RLC
    /// tanks: a fourfold real root and a twofold complex pair.
    fn repeated_sections() -> Circuit {
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let vin = c.node("in");
        c.voltage_source("V1", vin, g, SourceValue::ac(1.0, 0.0)).unwrap();
        for k in 1..=4 {
            let node = c.node(&format!("rc{k}"));
            c.resistor(&format!("R{k}"), vin, node, 49.9).unwrap();
            c.capacitor(&format!("C{k}"), node, g, 470e-12).unwrap();
        }
        for k in 1..=2 {
            let (a, b) = (c.node(&format!("t{k}a")), c.node(&format!("t{k}b")));
            c.resistor(&format!("RT{k}"), vin, a, 10.0).unwrap();
            c.inductor(&format!("LT{k}"), a, b, 1e-6).unwrap();
            c.capacitor(&format!("CT{k}"), b, g, 1e-9).unwrap();
        }
        c
    }

    #[test]
    fn repeated_roots_are_certified_by_inverse_iteration() {
        let c = repeated_sections();
        let result = pole_zero(&c, None).unwrap();
        let rc = -1.0 / (49.9 * 470e-12);
        // s^2 L C + s R C + 1 = 0.
        let (l, cap, r): (f64, f64, f64) = (1e-6, 1e-9, 10.0);
        let re = -r / (2.0 * l);
        let im = (1.0 / (l * cap) - re * re).sqrt();
        assert_eq!(result.poles.len(), 8, "{:?}", result.poles);
        for pole in &result.poles {
            assert!(pole.refined, "every root certified: {pole:?}");
            let near_rc = (pole.value - Complex::new(rc, 0.0)).abs() <= 1e-9 * rc.abs();
            let near_tank = (pole.value.re - re).abs() <= 1e-9 * im && (pole.value.im.abs() - im).abs() <= 1e-9 * im;
            assert!(near_rc || near_tank, "{pole:?} against {rc} and {re} +/- {im}j");
        }
        assert_eq!(result.poles.iter().filter(|p| p.value.im == 0.0).count(), 4);
    }

    #[test]
    fn a_qr_value_with_no_eigenvalue_near_it_is_replaced() {
        let c = repeated_sections();
        let c = crate::newton::linearized(&c).unwrap();
        let layout = Layout::of(&c);
        let (g, cm) = pencil(&c, &layout);
        let rc = -1.0 / (49.9 * 470e-12);
        // A spurious "unstable" value beside the real root, as round-off of a
        // stiff pencil leaves one: inverse iteration finds the root that is there.
        let spurious = Complex::new(-0.9 * rc * 0.05, 0.9 * rc.abs());
        let spectral = 1e9;
        let (value, _) = nearest_eigenvalue(&g, &cm, spurious, spectral).expect("an eigenvalue is certified");
        let (l, cap, r): (f64, f64, f64) = (1e-6, 1e-9, 10.0);
        let re = -r / (2.0 * l);
        let im = (1.0 / (l * cap) - re * re).sqrt();
        let candidates = [Complex::new(rc, 0.0), Complex::new(re, im), Complex::new(re, -im)];
        let nearest = candidates.iter().fold(f64::INFINITY, |m, z| m.min((*z - spurious).abs()));
        let found = candidates.iter().find(|z| (**z - value).abs() <= 1e-9 * z.abs()).expect("a true root");
        assert!(((*found - spurious).abs() - nearest).abs() <= 1e-9 * nearest, "the nearest root: {value:?}");
        assert!(value.re < 0.0);
    }

    #[test]
    fn rc_and_rlc_poles() {
        // RC ladder of three equal sections from an ideal source: poles of the
        // tridiagonal [2 -1 0; -1 2 -1; 0 -1 1] / (R C): (2 - 2 cos((2k-1) pi / 7)) / (R C).
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let (r, cap) = (1e3, 1e-9);
        let vin = c.node("in");
        c.voltage_source("V1", vin, g, SourceValue::ac(1.0, 0.0)).unwrap();
        let mut prev = vin;
        for k in 1..=3 {
            let node = c.node(&format!("n{k}"));
            c.resistor(&format!("R{k}"), prev, node, r).unwrap();
            c.capacitor(&format!("C{k}"), node, g, cap).unwrap();
            prev = node;
        }
        let result = pole_zero(&c, None).unwrap();
        assert_eq!(result.poles.len(), 3);
        let mut want: Vec<f64> =
            (1..=3).map(|k| -(2.0 - 2.0 * ((2 * k - 1) as f64 * PI / 7.0).cos()) / (r * cap)).collect();
        want.sort_by(|a, b| b.total_cmp(a));
        for (pole, want) in result.poles.iter().zip(&want) {
            assert!(close(pole.value.re, *want, 1e-12) && pole.value.im == 0.0 && pole.refined, "{pole:?} {want}");
        }
        assert!(result.stable());
        assert_eq!(result.infinite, result.unknowns - 3);

        // Series RLC from a source: s^2 + (R/L) s + 1/(L C) = 0.
        let mut c = Circuit::new();
        let (a, b, o) = (c.node("a"), c.node("b"), c.node("o"));
        let (r, l, cap) = (10.0, 1e-3, 1e-6);
        c.voltage_source("V1", a, g, SourceValue::ac(1.0, 0.0)).unwrap();
        c.resistor("R1", a, b, r).unwrap();
        c.inductor("L1", b, o, l).unwrap();
        c.capacitor("C1", o, g, cap).unwrap();
        let transfer = Transfer { input: "V1".to_string(), output: "o".to_string(), reference: None };
        let result = pole_zero(&c, Some(&transfer)).unwrap();
        assert_eq!(result.poles.len(), 2);
        let w0 = 1.0 / (l * cap).sqrt();
        let alpha = r / (2.0 * l);
        let wd = (w0 * w0 - alpha * alpha).sqrt();
        assert!(close(result.poles[0].value.re, -alpha, 1e-12), "{:?}", result.poles);
        assert!(close(result.poles[0].value.im, wd, 1e-12), "{:?}", result.poles);
        assert_eq!(result.poles[1].value, result.poles[0].value.conj());
        assert!(close(result.damping().unwrap(), alpha / w0, 1e-12));
        assert!(result.poles[0].nodes.iter().any(|(node, _)| node == "o"), "{:?}", result.poles[0].nodes);
        // V(o) / V1 = 1 / (L C s^2 + R C s + 1): no finite zeros.
        assert!(result.zeros.is_empty(), "{:?}", result.zeros);
        // Across the resistor: V(a, b) / V1 = R C s / (...): one zero at the origin.
        let transfer = Transfer { input: "V1".to_string(), output: "a".to_string(), reference: Some("b".to_string()) };
        let result = pole_zero(&c, Some(&transfer)).unwrap();
        assert_eq!(result.zeros.len(), 1, "{:?}", result.zeros);
        assert!(result.zeros[0].value.abs() < 1e-6 * w0, "{:?}", result.zeros);
    }

    #[test]
    fn negative_resistance_and_positive_feedback_are_unstable() {
        // A parallel RLC tank with a negative conductance larger than 1/R: an
        // oscillator, a complex pair at (g - 1/R) / (2 C) in the right half-plane.
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let t = c.node("tank");
        let (r, l, cap, gneg) = (1e3, 1e-6, 1e-9, 2e-3);
        c.resistor("R1", t, g, r).unwrap();
        c.inductor("L1", t, g, l).unwrap();
        c.capacitor("C1", t, g, cap).unwrap();
        // Injects gneg * V(tank) into the tank: a conductance of -gneg.
        c.vccs("G1", g, t, t, g, gneg).unwrap();
        let result = pole_zero(&c, None).unwrap();
        assert_eq!(result.poles.len(), 2);
        let sigma = (gneg - 1.0 / r) / (2.0 * cap);
        assert!(close(result.poles[0].value.re, sigma, 1e-10), "{:?}", result.poles);
        assert!(!result.stable());
        assert!(result.poles[0].right_half_plane());
        assert_eq!(result.poles[0].nodes[0].0, "tank");
        assert!(result.damping().unwrap() < 0.0);
        // The same with the loss winning: stable.
        let mut c2 = Circuit::new();
        let t = c2.node("tank");
        c2.resistor("R1", t, g, r).unwrap();
        c2.inductor("L1", t, g, l).unwrap();
        c2.capacitor("C1", t, g, cap).unwrap();
        c2.vccs("G1", g, t, t, g, 0.5e-3).unwrap();
        assert!(pole_zero(&c2, None).unwrap().stable());

        // An op-amp with its feedback on the non-inverting input: a real pole
        // at w1 (A0 beta - 1) in the right half-plane.
        let mut c = Circuit::new();
        let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
        let (a0, gbw) = (1e5, 1e6);
        c.opamp("U1", OpampPins::new(inp, inn, out), &OpampParams::new(a0, gbw)).unwrap();
        c.resistor("RF", out, inp, 9e3).unwrap();
        c.resistor("RG", inp, g, 1e3).unwrap();
        c.voltage_source("VN", inn, g, SourceValue::dc(0.0)).unwrap();
        let result = pole_zero(&c, None).unwrap();
        assert!(!result.stable());
        let w1 = 2.0 * PI * gbw / a0;
        let pole = result.poles.iter().find(|p| p.right_half_plane()).unwrap();
        assert!(close(pole.value.re, w1 * (a0 * 0.1 - 1.0), 1e-9), "{pole:?}");
    }

    /// A non-inverting amplifier of gain `1 + rf / rg` around a one- or two-pole
    /// op-amp, with an optional output resistance and load; the probe in
    /// series with the output pin (plus at the pin when `forward`).
    fn amplifier(params: &OpampParams, rf: f64, rg: f64, load: Option<f64>, forward: bool) -> Circuit {
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let (inp, inn, pin, out) = (c.node("inp"), c.node("inn"), c.node("pin"), c.node("out"));
        c.voltage_source("VIN", inp, g, SourceValue::ac(1.0, 0.0)).unwrap();
        c.opamp("U1", OpampPins::new(inp, inn, pin), params).unwrap();
        let (plus, minus) = if forward { (pin, out) } else { (out, pin) };
        c.voltage_source("probe", plus, minus, SourceValue::dc(0.0)).unwrap();
        c.resistor("RF", out, inn, rf).unwrap();
        if rg > 0.0 {
            c.resistor("RG", inn, g, rg).unwrap();
        }
        if let Some(rl) = load {
            c.resistor("RL", out, g, rl).unwrap();
        }
        c
    }

    fn margins(result: &LoopGainResult) -> crate::measure::LoopMargins {
        crate::measure::loop_margins(&result.frequencies, &result.gain).unwrap()
    }

    #[test]
    fn single_pole_follower_and_gain_ten() {
        let (a0, gbw) = (1e5, 10e6);
        let params = OpampParams::new(a0, gbw);
        let sweep = Sweep::Decade { start: 1.0, stop: 1e9, points_per_decade: 50 };
        for (rf, rg, beta) in [(1e3, 0.0, 1.0), (9e3, 1e3, 0.1)] {
            let c = amplifier(&params, rf, rg, None, true);
            let result = loop_gain(&c, &sweep, "probe").unwrap();
            // T = A0 beta / (1 + s / w1) exactly.
            let w1 = 2.0 * PI * gbw / a0;
            for (f, t) in result.frequencies.iter().zip(&result.gain) {
                let want = Complex::real(a0 * beta) / Complex::new(1.0, 2.0 * PI * f / w1);
                assert!((*t - want).abs() <= 1e-9 * want.abs(), "{f} Hz: {t:?} {want:?}");
            }
            let m = margins(&result);
            let k = a0 * beta;
            let fc = w1 * (k * k - 1.0).sqrt() / (2.0 * PI);
            let pm = 180.0 - (k * k - 1.0).sqrt().atan().to_degrees();
            assert!(close(m.crossover.unwrap(), fc, 2e-4), "{:?} {fc}", m.crossover);
            assert!((m.phase_margin.unwrap() - pm).abs() < 0.01, "{:?} {pm}", m.phase_margin);
            assert_eq!(m.phase_crossover, None);
            assert_eq!(m.gain_margin, None);
        }
    }

    #[test]
    fn two_and_three_pole_loops() {
        let (a0, gbw, fp2) = (1e5, 10e6, 5e6);
        let mut params = OpampParams::new(a0, gbw);
        params.second_pole = Some(fp2);
        let sweep = Sweep::Decade { start: 1.0, stop: 1e10, points_per_decade: 100 };
        let c = amplifier(&params, 1e3, 0.0, None, true);
        let result = loop_gain(&c, &sweep, "probe").unwrap();
        let (w1, w2) = (2.0 * PI * gbw / a0, 2.0 * PI * fp2);
        // |T| = 1: (1 + x/w1^2)(1 + x/w2^2) = A0^2 with x = w^2.
        let (qa, qb, qc) = (1.0 / (w1 * w1 * w2 * w2), 1.0 / (w1 * w1) + 1.0 / (w2 * w2), 1.0 - a0 * a0);
        let x = (-qb + (qb * qb - 4.0 * qa * qc).sqrt()) / (2.0 * qa);
        let wc = x.sqrt();
        let pm = 180.0 - (wc / w1).atan().to_degrees() - (wc / w2).atan().to_degrees();
        let m = margins(&result);
        assert!(close(m.crossover.unwrap(), wc / (2.0 * PI), 2e-4), "{:?}", m.crossover);
        assert!((m.phase_margin.unwrap() - pm).abs() < 0.02, "{:?} {pm}", m.phase_margin);
        // The step overshoot of a two-pole loop follows from its damping:
        // the closed-loop poles are those of s^2 + (w1 + w2) s + w1 w2 (1 + A0).
        let closed = pole_zero(&c, None).unwrap();
        let zeta = (w1 + w2) / (2.0 * (w1 * w2 * (1.0 + a0)).sqrt());
        let found = closed.damping().unwrap();
        assert!(close(found, zeta, 1e-9), "{found} {zeta}");

        // A third pole from an RC in the feedback path, buffered by a VCVS:
        // the phase crosses -180 degrees where the three atans sum to 180.
        // With 10 MHz of gain-bandwidth the margins are positive and the
        // circuit stable; with 100 MHz both are negative and a pole pair is in
        // the right half-plane.
        for (gbw, stable) in [(10e6, true), (100e6, false)] {
            let mut params = OpampParams::new(a0, gbw);
            params.second_pole = Some(fp2);
            let mut c = Circuit::new();
            let g = NodeId::GROUND;
            let (inp, inn, pin, out, f) = (c.node("inp"), c.node("inn"), c.node("pin"), c.node("out"), c.node("f"));
            c.voltage_source("VIN", inp, g, SourceValue::ac(1.0, 0.0)).unwrap();
            c.opamp("U1", OpampPins::new(inp, inn, pin), &params).unwrap();
            c.voltage_source("probe", pin, out, SourceValue::dc(0.0)).unwrap();
            let (rr, cc) = (1e3, 1.0 / (2.0 * PI * 1e3 * 20e6));
            c.resistor("RR", out, f, rr).unwrap();
            c.capacitor("CR", f, g, cc).unwrap();
            c.vcvs("E1", inn, g, f, g, 1.0).unwrap();
            let result = loop_gain(&c, &sweep, "probe").unwrap();
            let (w1, w3) = (2.0 * PI * gbw / a0, 1.0 / (rr * cc));
            let phase = |w: f64| (w / w1).atan() + (w / w2).atan() + (w / w3).atan();
            let (mut lo, mut hi) = (w1, 1e3 * w3);
            for _ in 0..200 {
                let mid = (lo * hi).sqrt();
                if phase(mid) < PI { lo = mid } else { hi = mid }
            }
            let w180 = lo;
            let squares = (1.0 + (w180 / w1).powi(2)) * (1.0 + (w180 / w2).powi(2)) * (1.0 + (w180 / w3).powi(2));
            let gm = 20.0 * (squares.sqrt() / a0).log10();
            let m = margins(&result);
            assert!(close(m.phase_crossover.unwrap(), w180 / (2.0 * PI), 2e-4), "{:?}", m.phase_crossover);
            assert!((m.gain_margin.unwrap() - gm).abs() < 0.01, "{:?} {gm}", m.gain_margin);
            assert_eq!(m.gain_margin.unwrap() > 0.0, stable, "{m:?}");
            assert_eq!(m.phase_margin.unwrap() > 0.0, stable, "{m:?}");
            assert_eq!(pole_zero(&c, None).unwrap().stable(), stable);
        }
    }

    #[test]
    fn tian_is_independent_of_the_probe_direction_and_counts_the_loading() {
        let mut params = OpampParams::new(3e4, 50e6);
        params.output_resistance = 50.0;
        params.input_capacitance_common = 5e-12;
        params.second_pole = Some(80e6);
        let sweep = Sweep::Decade { start: 1e3, stop: 1e9, points_per_decade: 20 };
        let (rf, rg, rl) = (2e3, 1e3, 100.0);
        let forward = loop_gain(&amplifier(&params, rf, rg, Some(rl), true), &sweep, "probe").unwrap();
        let reverse = loop_gain(&amplifier(&params, rf, rg, Some(rl), false), &sweep, "probe").unwrap();
        for (a, b) in forward.gain.iter().zip(&reverse.gain) {
            assert!((*a - *b).abs() <= 1e-9 * a.abs(), "{a:?} {b:?}");
        }
        // The loop broken by hand at the amplifier's output, with the loading
        // replicated: the network driven by a test source through a copy of
        // the 50 ohm output resistance; the amplifier's pin loaded by a copy of
        // the network and the load. T = -V(U1#o) / V(test).
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let (inp, inn, pin, out, test) = (c.node("inp"), c.node("inn"), c.node("pin"), c.node("out"), c.node("test"));
        c.voltage_source("VIN", inp, g, SourceValue::dc(0.0)).unwrap();
        c.opamp("U1", OpampPins::new(inp, inn, pin), &params).unwrap();
        c.voltage_source("VT", test, g, SourceValue::ac(1.0, 0.0)).unwrap();
        c.resistor("RO", test, out, 50.0).unwrap();
        c.resistor("RF", out, inn, rf).unwrap();
        c.resistor("RG", inn, g, rg).unwrap();
        c.resistor("RL", out, g, rl).unwrap();
        // The copy of the network on the pin, its tap loaded like the real input.
        let tap = c.node("tap");
        c.resistor("RF2", pin, tap, rf).unwrap();
        c.resistor("RG2", tap, g, rg).unwrap();
        c.capacitor("CIN2", tap, g, 5e-12).unwrap();
        c.resistor("RL2", pin, g, rl).unwrap();
        let hand = crate::ac(&c, &sweep).unwrap();
        let internal = hand.voltage("U1#o").unwrap();
        let mut worst = 0.0f64;
        for (t, vo) in forward.gain.iter().zip(internal) {
            let want = -*vo;
            worst = worst.max((*t - want).abs() / want.abs());
        }
        assert!(worst < 1e-9, "worst relative difference {worst}");
        // The naive voltage ratio across the probe is not the loop gain here.
        let unloaded = Complex::real(3e4 * rg / (rf + rg));
        assert!((forward.gain[0] - unloaded).abs() > 1e-3 * unloaded.abs());
    }

    #[test]
    fn middlebrook_equals_tian_at_a_one_way_break() {
        // At the amplifier output Y21 = 0, so (Y21 - Y12)/(Y11 + Y22 + 2 Y12) and
        // (Y12 + Y21)/(Y11 + Y22) agree up to the sign of the orientation.
        let mut params = OpampParams::new(1e4, 20e6);
        params.output_resistance = 20.0;
        let c = amplifier(&params, 1e3, 1e3, Some(200.0), true);
        let sweep = Sweep::Decade { start: 1e3, stop: 1e8, points_per_decade: 5 };
        let tian = loop_gain(&c, &sweep, "probe").unwrap();
        // Y parameters of the opened loop: port 1 the pin, port 2 the net.
        let ports = |vx: f64, vy: f64, f: f64| {
            let mut o = Circuit::new();
            let g = NodeId::GROUND;
            let (inp, inn, pin, out) = (o.node("inp"), o.node("inn"), o.node("pin"), o.node("out"));
            o.voltage_source("VIN", inp, g, SourceValue::dc(0.0)).unwrap();
            o.opamp("U1", OpampPins::new(inp, inn, pin), &params).unwrap();
            o.voltage_source("VX", pin, g, SourceValue::ac(vx, 0.0)).unwrap();
            o.voltage_source("VY", out, g, SourceValue::ac(vy, 0.0)).unwrap();
            o.resistor("RF", out, inn, 1e3).unwrap();
            o.resistor("RG", inn, g, 1e3).unwrap();
            o.resistor("RL", out, g, 200.0).unwrap();
            let r = crate::ac(&o, &Sweep::List(vec![f])).unwrap();
            // Current into the network at each port: minus the source current.
            (-r.current("VX").unwrap()[0], -r.current("VY").unwrap()[0])
        };
        for (f, t) in tian.frequencies.iter().zip(&tian.gain) {
            let (y11, y21) = ports(1.0, 0.0, *f);
            let (y12, y22) = ports(0.0, 1.0, *f);
            let from_y = (y12 + y21) / (y11 + y22);
            assert!((*t - from_y).abs() <= 1e-9 * t.abs(), "{f}: {t:?} {from_y:?}");
            assert!(y21.abs() <= 1e-12 * y12.abs(), "one-way break");
            // Middlebrook with the loop signal from port 2 to port 1.
            let middlebrook = (y12 - y21) / (y11 + y22 + y21.scale(2.0));
            assert!((*t - middlebrook).abs() <= 1e-9 * t.abs());
        }
    }

    /// `H(s) / H(s0)` from the poles and zeros against the AC analysis of the
    /// input source (AC 1): the largest relative error over `sweep`.
    fn reconstruction_error(c: &Circuit, transfer: &Transfer, sweep: &Sweep) -> (f64, PoleZeroResult) {
        let result = pole_zero(c, Some(transfer)).unwrap();
        let ac = crate::ac(c, sweep).unwrap();
        let h: Vec<Complex> = match &transfer.reference {
            Some(r) => ac.voltage_between(&transfer.output, r).unwrap(),
            None => ac.voltage(&transfer.output).unwrap().to_vec(),
        };
        let rational = |f: f64| {
            let s = Complex::new(0.0, 2.0 * PI * f);
            let num = result.zeros.iter().fold(Complex::ONE, |acc, z| acc * (s - z.value));
            let den = result.poles.iter().fold(Complex::ONE, |acc, p| acc * (s - p.value));
            num / den
        };
        let fs = ac.frequencies();
        let mid = fs.len() / 2;
        let k = h[mid] / rational(fs[mid]);
        // Where |H| is below 1e-9 of its largest value the AC solution itself
        // carries round-off of that order (a node behind 1 Gohm and 10 uF).
        let big = h.iter().fold(0.0f64, |m, z| m.max(z.abs()));
        let worst = fs
            .iter()
            .zip(&h)
            .filter(|(_, h)| h.abs() > 1e-9 * big)
            .map(|(f, h)| ((k * rational(*f)) - *h).abs() / h.abs())
            .fold(0.0, f64::max);
        (worst, result)
    }

    #[test]
    fn stiff_circuits_reconstruct_their_transfer_function() {
        let g = NodeId::GROUND;
        let sweep = Sweep::Decade { start: 1e-2, stop: 1e10, points_per_decade: 10 };
        // A follower with input capacitances and output resistance driving an
        // RC network whose values span 1 mohm to 1 Gohm and 1 fF to 10 uF, a
        // capacitor straight across the source (no pole of its own), a loop of
        // three capacitors (one of them no pole), and coupled inductors.
        let mut c = Circuit::new();
        let (vin, a, inn, pin, b, m, n2, l1, l2) = (
            c.node("in"),
            c.node("a"),
            c.node("inn"),
            c.node("pin"),
            c.node("b"),
            c.node("m"),
            c.node("n2"),
            c.node("l1"),
            c.node("l2"),
        );
        c.voltage_source("V1", vin, g, SourceValue::ac(1.0, 0.0)).unwrap();
        c.capacitor("CS", vin, g, 1e-6).unwrap();
        c.resistor("R1", vin, a, 1e3).unwrap();
        c.capacitor("C1", a, g, 1e-15).unwrap();
        let mut params = OpampParams::new(1e5, 50e6);
        params.output_resistance = 30.0;
        params.input_capacitance_common = 2e-12;
        params.input_capacitance_differential = 1e-12;
        params.second_pole = Some(200e6);
        c.opamp("U1", OpampPins::new(a, inn, pin), &params).unwrap();
        c.resistor("RF", pin, inn, 1e-3).unwrap();
        c.resistor("R2", pin, b, 1e9).unwrap();
        c.capacitor("C2", b, g, 10e-6).unwrap();
        c.resistor("R3", pin, m, 100.0).unwrap();
        c.capacitor("CA", m, g, 1e-9).unwrap();
        c.capacitor("CB", m, n2, 2e-9).unwrap();
        c.capacitor("CC", n2, g, 3e-9).unwrap();
        c.resistor("R4", n2, l1, 10.0).unwrap();
        let la = c.inductor("LA", l1, g, 1e-6).unwrap();
        let lb = c.inductor("LB", l2, g, 4e-6).unwrap();
        c.coupling("K1", la, lb, 0.9).unwrap();
        c.resistor("RL", l2, g, 50.0).unwrap();
        for output in ["pin", "b", "n2", "l2"] {
            let transfer = Transfer { input: "V1".to_string(), output: output.to_string(), reference: None };
            let (worst, result) = reconstruction_error(&c, &transfer, &sweep);
            // The zeros are QR values (not refined): near pole-zero pairs
            // that almost cancel they limit the rebuilt response to 1e-4 to 7e-4.
            eprintln!("stiff circuit, output {output}: transfer function rebuilt within {worst:e}");
            assert!(worst < 1e-3, "{output}: worst {worst}, poles {:?}, zeros {:?}", result.poles, result.zeros);
            assert!(result.poles.iter().all(|p| p.refined), "{:?}", result.poles);
            assert!(result.stable());
        }
    }

    #[test]
    fn random_rc_networks_reconstruct() {
        let g = NodeId::GROUND;
        let mut rng = crate::rng::Rng::new(11);
        let sweep = Sweep::Decade { start: 1.0, stop: 1e9, points_per_decade: 8 };
        for size in [6usize, 25, 60] {
            let mut c = Circuit::new();
            let nodes: Vec<NodeId> = (0..size).map(|k| c.node(&format!("n{k}"))).collect();
            c.voltage_source("V1", nodes[0], g, SourceValue::ac(1.0, 0.0)).unwrap();
            for k in 1..size {
                let other = nodes[(rng.uniform() * k as f64) as usize];
                let r = 10f64.powf(1.0 + 5.0 * rng.uniform());
                c.resistor(&format!("R{k}"), other, nodes[k], r).unwrap();
                let cap = 10f64.powf(-12.0 + 4.0 * rng.uniform());
                let to = if rng.uniform() < 0.3 { nodes[(rng.uniform() * k as f64) as usize] } else { g };
                if to != nodes[k] {
                    c.capacitor(&format!("C{k}"), nodes[k], to, cap).unwrap();
                }
            }
            let transfer = Transfer { input: "V1".to_string(), output: format!("n{}", size - 1), reference: None };
            let (worst, result) = reconstruction_error(&c, &transfer, &sweep);
            assert!(worst < 1e-6, "{size} nodes: worst {worst}");
            // An RC network has real poles in the left half-plane.
            assert!(result.poles.iter().all(|p| p.value.im == 0.0 && p.value.re < 0.0), "{:?}", result.poles);
        }
    }

    #[test]
    fn floating_nodes_give_poles_at_the_origin() {
        // Two nodes joined to the rest only by capacitors: a pole at s = 0, which
        // round-off must not make unstable; the RC pole beside it is found as usual.
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let (a, f1, f2) = (c.node("a"), c.node("f1"), c.node("f2"));
        c.voltage_source("V1", a, g, SourceValue::ac(1.0, 0.0)).unwrap();
        c.capacitor("C1", a, f1, 1e-12).unwrap();
        c.capacitor("C2", f1, f2, 3e-12).unwrap();
        c.capacitor("C3", f2, g, 1e-12).unwrap();
        let b = c.node("b");
        c.resistor("R1", a, b, 1e3).unwrap();
        c.capacitor("C4", b, g, 1e-9).unwrap();
        let result = pole_zero(&c, None).unwrap();
        assert!(result.stable(), "{:?}", result.poles);
        assert!(result.poles.iter().any(|p| p.origin), "{:?}", result.poles);
        let rc = result.poles.iter().find(|p| !p.origin).unwrap();
        assert!(close(rc.value.re, -1e6, 1e-9), "{rc:?}");
        assert_eq!(result.damping(), Some(1.0));
    }

    #[test]
    fn tian_matches_the_admittance_form_at_a_two_way_break() {
        // A probe in a passive branch that two loops share: an amplifier whose
        // feedback runs through the probe and also around it through RB. Then Y12
        // and Y21 are both nonzero, and Tian's T must still be
        // (Y12 + Y21) / (Y11 + Y22), the same for either orientation.
        let mut params = OpampParams::new(1e4, 20e6);
        params.output_resistance = 100.0;
        params.input_capacitance_common = 3e-12;
        let build = |probe: Option<bool>, vx: f64, vy: f64| {
            let mut c = Circuit::new();
            let g = NodeId::GROUND;
            let (inp, inn, out, x, y) = (c.node("inp"), c.node("inn"), c.node("out"), c.node("x"), c.node("y"));
            c.voltage_source("VIN", inp, g, SourceValue::dc(0.0)).unwrap();
            c.opamp("U1", OpampPins::new(inp, inn, out), &params).unwrap();
            c.resistor("RF", out, x, 2e3).unwrap();
            c.resistor("RG", y, inn, 1e3).unwrap();
            c.resistor("RB", out, y, 5e3).unwrap();
            c.capacitor("CX", x, g, 2e-12).unwrap();
            c.resistor("RY", y, g, 4e3).unwrap();
            match probe {
                Some(true) => {
                    c.voltage_source("probe", x, y, SourceValue::dc(0.0)).unwrap();
                }
                Some(false) => {
                    c.voltage_source("probe", y, x, SourceValue::dc(0.0)).unwrap();
                }
                None => {
                    c.voltage_source("VX", x, g, SourceValue::ac(vx, 0.0)).unwrap();
                    c.voltage_source("VY", y, g, SourceValue::ac(vy, 0.0)).unwrap();
                }
            }
            c
        };
        let sweep = Sweep::Decade { start: 1e3, stop: 1e9, points_per_decade: 4 };
        let forward = loop_gain(&build(Some(true), 0.0, 0.0), &sweep, "probe").unwrap();
        let reverse = loop_gain(&build(Some(false), 0.0, 0.0), &sweep, "probe").unwrap();
        let currents = |vx: f64, vy: f64| {
            let r = crate::ac(&build(None, vx, vy), &sweep).unwrap();
            let ix: Vec<Complex> = r.current("VX").unwrap().iter().map(|z| -*z).collect();
            let iy: Vec<Complex> = r.current("VY").unwrap().iter().map(|z| -*z).collect();
            (ix, iy)
        };
        let ((y11, y21), (y12, y22)) = (currents(1.0, 0.0), currents(0.0, 1.0));
        for k in 0..forward.gain.len() {
            let want = (y12[k] + y21[k]) / (y11[k] + y22[k]);
            assert!((forward.gain[k] - want).abs() <= 1e-9 * want.abs(), "{k}: {:?} {want:?}", forward.gain[k]);
            assert!((reverse.gain[k] - want).abs() <= 1e-9 * want.abs());
            assert!(y21[k].abs() > 1e-6 * y12[k].abs(), "two-way");
        }
        // The test tells the two apart: Middlebrook's form, which assumes a
        // one-way break, differs from Tian's here by far more than the tolerance.
        let differs = (0..forward.gain.len()).any(|k| {
            let middlebrook = (y12[k] - y21[k]) / (y11[k] + y22[k] + y21[k].scale(2.0));
            (forward.gain[k] - middlebrook).abs() > 1e-6 * forward.gain[k].abs()
        });
        assert!(differs);
    }

    /// A stiff self-written circuit (1 uohm, 1 mohm into 1 pF, 1 Tohm taps)
    /// with two pairs of equal slow couplings: 47 kohm into 1 uF (-21.28 rad/s,
    /// twice) and 33 Mohm into 4.7 nF (-6.447 rad/s, twice).
    fn stiff_couplings(floating: bool) -> Circuit {
        let mut c = Circuit::new();
        let g = NodeId::GROUND;
        let vd = c.node("vd");
        c.voltage_source("VD", vd, g, SourceValue::ac(1.0, 0.0)).unwrap();
        let vp = c.node("vp");
        c.voltage_source("VP", vp, g, SourceValue::dc(5.0)).unwrap();
        let (vpi, mid, reference) = (c.node("vpi"), c.node("mid"), c.node("ref"));
        c.resistor("RZ", vp, vpi, 1e-6).unwrap();
        c.resistor("RM1", vpi, mid, 1e7).unwrap();
        c.resistor("RM2", mid, g, 1e7).unwrap();
        c.vcvs("EREF", reference, g, mid, g, 1.0).unwrap();
        let mut outputs = Vec::new();
        for (k, source) in [vd, vd].into_iter().enumerate() {
            let (i, t, o, p) = (
                c.node(&format!("i{k}")),
                c.node(&format!("t{k}")),
                c.node(&format!("o{k}")),
                c.node(&format!("p{k}")),
            );
            c.capacitor(&format!("C{k}"), source, i, 1e-6).unwrap();
            c.resistor(&format!("R{k}"), i, reference, 47e3).unwrap();
            c.resistor(&format!("RTA{k}"), i, t, 1e12).unwrap();
            c.resistor(&format!("RTB{k}"), t, reference, 1e12).unwrap();
            c.vcvs(&format!("E{k}"), o, reference, i, reference, 1.0).unwrap();
            c.resistor(&format!("RX{k}"), o, p, 1e-3).unwrap();
            c.capacitor(&format!("CP{k}"), p, g, 1e-12).unwrap();
            c.resistor(&format!("RL{k}"), p, reference, 1e3).unwrap();
            outputs.push(p);
        }
        for (k, source) in outputs.into_iter().enumerate() {
            let (s, o) = (c.node(&format!("s{k}")), c.node(&format!("so{k}")));
            c.capacitor(&format!("CS{k}"), source, s, 4.7e-9).unwrap();
            c.resistor(&format!("RS{k}"), s, g, 33e6).unwrap();
            c.vcvs(&format!("ES{k}"), o, g, s, g, 1.0).unwrap();
            c.resistor(&format!("RO{k}"), o, g, 1e3).unwrap();
        }
        if floating {
            // A node with only capacitors to it (an electrode in air): a root at the origin.
            let air = c.node("air");
            let p0 = c.find_node("p0").unwrap();
            c.capacitor("CAIR1", p0, air, 1e-12).unwrap();
            c.capacitor("CAIR2", air, g, 1e-12).unwrap();
        }
        c
    }

    #[test]
    fn slow_roots_of_a_stiff_pencil_are_exact() {
        // Before the slow roots came from the inverted pencil
        // (`eigen::with_slow_roots_inverted`) they were 1.25e-3 and 7.6e-3 off
        // here: the QR iteration's round-off is set by the 1e15 rad/s roots.
        for floating in [false, true] {
            let c = stiff_couplings(floating);
            let result = pole_zero(&c, None).unwrap();
            let values: Vec<Complex> = result.poles.iter().map(|p| p.value).collect();
            let error = |want: f64| {
                let mut e: Vec<f64> =
                    values.iter().map(|z| (*z - Complex::new(want, 0.0)).abs() / want.abs()).collect();
                e.sort_by(f64::total_cmp);
                e[1] // both of the pair
            };
            // 47 kohm beside the two 1 Tohm taps; 33 Mohm behind the 1 mohm source.
            let (a, b) = (-1.0 / (1e-6 / (1.0 / 47e3 + 1.0 / 2e12)), -1.0 / (33e6 * 4.7e-9 + 1e-3 * 4.7e-9));
            let (ea, eb) = (error(a), error(b));
            assert!(ea < 1e-8 && eb < 1e-8, "floating {floating}: {ea} {eb}");
            assert!(result.stable(), "{:?}", result.poles);
            assert_eq!(result.poles.iter().filter(|p| p.origin).count(), usize::from(floating));
        }
    }
}

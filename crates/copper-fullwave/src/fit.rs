//! Impedance of a loop that rings: the record is extended by a sum of damped exponentials fitted to its tail.
//!
//! After the excitation has ended the port voltage and current of a one-port are free oscillations,
//! `x[n] = sum_k c_k exp(s_k n dt)`, the natural frequencies of the loop with the port termination.
//! A loop that resonates in or near the band (CopperOrgan: 348 MHz) does not decay within the run, and the
//! transform of the cut record is wrong (a rectangular cut changes L by 2 to 9 % on loops that do decay in
//! 80 % of their record, and a window makes it worse). The tail is therefore fitted, the poles by linear
//! prediction on a decimated copy (least squares by Householder QR, roots by Durand-Kerner), the amplitudes
//! of the current and of the voltage by complex least squares on the same poles, and the transform of the
//! unrecorded future is added analytically (a geometric series per pole). The result is the transform of the
//! full ring-down, which is what a run to decay would give. The fit is tried at several model orders and the
//! median taken; the spread of the orders is the fit's numerical sensitivity, not an error bound.

use ems::Complex64;

use crate::solve::Record;

/// What the fit did.
#[derive(Clone, Debug, PartialEq)]
pub struct FitInfo {
    /// Model orders tried and the inductance each gave (band mean), henry; `None` where the fit failed.
    pub orders: Vec<(usize, Option<f64>)>,
    /// Median inductance over the orders that succeeded, henry.
    pub inductance: f64,
    /// `(max - min) / median` over the orders that succeeded.
    pub spread: f64,
    /// Lowest frequency among the fitted poles that carry amplitude, hertz (the ring), and its Q.
    pub ring_hz: Option<(f64, f64)>,
    /// Seconds of tail the fit used.
    pub tail_s: f64,
    /// Poles of the chosen order that carry amplitude and grow over the fitted tail by more than
    /// [`GROWTH_LIMIT`] (the fit reflects them into the unit circle; they are counted before that).
    pub unstable_poles: usize,
    /// Largest growth factor over the fitted tail among the poles that carry amplitude (1 when none grows).
    pub max_growth: f64,
}

/// Linear least squares `min |A x - b|` by Householder QR with a small ridge; `a` is row-major.
#[allow(clippy::needless_range_loop)] // dense elimination reads best with indices
fn lstsq(a: &[Vec<f64>], b: &[f64], ridge: f64) -> Option<Vec<f64>> {
    let (m, n) = (a.len(), a.first()?.len());
    let mut rows: Vec<Vec<f64>> = a.to_vec();
    let mut rhs: Vec<f64> = b.to_vec();
    let scale = rows.iter().flatten().fold(0.0_f64, |s, v| s.max(v.abs())).max(1e-300);
    for j in 0..n {
        let mut r = vec![0.0; n];
        r[j] = ridge * scale;
        rows.push(r);
        rhs.push(0.0);
    }
    let m = m + n;
    for k in 0..n {
        let norm = (k..m).map(|i| rows[i][k] * rows[i][k]).sum::<f64>().sqrt();
        if norm < 1e-300 {
            return None;
        }
        let alpha = if rows[k][k] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..m).map(|i| rows[i][k]).collect();
        v[0] -= alpha;
        let vn = v.iter().map(|x| x * x).sum::<f64>();
        if vn < 1e-300 {
            continue;
        }
        for j in k..n {
            let dot: f64 = (k..m).map(|i| v[i - k] * rows[i][j]).sum();
            let f = 2.0 * dot / vn;
            for i in k..m {
                rows[i][j] -= f * v[i - k];
            }
        }
        let dot: f64 = (k..m).map(|i| v[i - k] * rhs[i]).sum();
        let f = 2.0 * dot / vn;
        for i in k..m {
            rhs[i] -= f * v[i - k];
        }
    }
    let mut x = vec![0.0; n];
    for k in (0..n).rev() {
        let s: f64 = (k + 1..n).map(|j| rows[k][j] * x[j]).sum();
        if rows[k][k].abs() < 1e-300 {
            return None;
        }
        x[k] = (rhs[k] - s) / rows[k][k];
    }
    Some(x)
}

/// Solve the complex system `a x = b` (Gaussian elimination with partial pivoting).
#[allow(clippy::needless_range_loop)] // dense elimination reads best with indices
fn solve_complex(mut a: Vec<Vec<Complex64>>, mut b: Vec<Complex64>) -> Option<Vec<Complex64>> {
    let n = b.len();
    for k in 0..n {
        let p = (k..n).max_by(|&i, &j| a[i][k].norm().total_cmp(&a[j][k].norm()))?;
        if a[p][k].norm() < 1e-300 {
            return None;
        }
        a.swap(k, p);
        b.swap(k, p);
        for i in k + 1..n {
            let f = a[i][k] / a[k][k];
            for j in k..n {
                let t = a[k][j];
                a[i][j] -= f * t;
            }
            let t = b[k];
            b[i] -= f * t;
        }
    }
    let mut x = vec![Complex64::ZERO; n];
    for k in (0..n).rev() {
        let mut s = b[k];
        for j in k + 1..n {
            s -= a[k][j] * x[j];
        }
        x[k] = s / a[k][k];
    }
    Some(x)
}

/// Roots of the monic polynomial `z^p + c[0] z^(p-1) + ... + c[p-1]` (Durand-Kerner).
fn roots(c: &[f64]) -> Vec<Complex64> {
    let p = c.len();
    let eval = |z: Complex64| {
        let mut v = Complex64::ONE;
        for &ck in c {
            v = v * z + Complex64::new(ck, 0.0);
        }
        v
    };
    let radius = 1.0 + c.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let mut z: Vec<Complex64> = (0..p)
        .map(|k| Complex64::from_polar(0.9 * radius.min(1.2), 0.4 + 2.0 * std::f64::consts::PI * k as f64 / p as f64))
        .collect();
    for _ in 0..2000 {
        let mut worst = 0.0_f64;
        for k in 0..p {
            let mut den = Complex64::ONE;
            for j in 0..p {
                if j != k {
                    den *= z[k] - z[j];
                }
            }
            if den.norm() < 1e-300 {
                continue;
            }
            let step = eval(z[k]) / den;
            z[k] -= step;
            worst = worst.max(step.norm());
        }
        if worst < 1e-13 {
            break;
        }
    }
    z
}

/// Poles `z_k` (per decimated sample) of order `p` fitted to `y` by forward linear prediction; those outside
/// the unit circle are reflected in it, those that are real-negative or too close to zero are kept as they come.
/// The second vector holds `ln |z_k|` of each pole before the reflection (positive: the fit found it growing).
fn poles_of(y: &[f64], p: usize) -> Option<(Vec<Complex64>, Vec<f64>)> {
    let l = y.len();
    if l < 3 * p + 2 {
        return None;
    }
    // y[m] = -sum_j a[j] y[m-1-j] for m = p..l.
    let a: Vec<Vec<f64>> = (p..l).map(|m| (0..p).map(|j| y[m - 1 - j]).collect()).collect();
    let b: Vec<f64> = (p..l).map(|m| -y[m]).collect();
    let coef = lstsq(&a, &b, 1e-10)?;
    let mut z = roots(&coef);
    let growth: Vec<f64> = z.iter().map(|r| r.norm().max(1e-300).ln()).collect();
    for r in &mut z {
        if r.norm() > 1.0 {
            *r = 1.0 / r.conj();
        }
        if r.norm() > 1.0 - 1e-9 {
            *r *= 1.0 - 1e-9 / r.norm().max(1e-300);
        }
    }
    Some((z, growth))
}

/// Amplitudes `A_k` with `y[m] ~ sum_k A_k z_k^m` (complex least squares through the normal equations).
fn amplitudes(y: &[f64], z: &[Complex64]) -> Option<Vec<Complex64>> {
    let p = z.len();
    let cols: Vec<Vec<Complex64>> = z
        .iter()
        .map(|zk| {
            let mut v = Vec::with_capacity(y.len());
            let mut cur = Complex64::ONE;
            for _ in 0..y.len() {
                v.push(cur);
                cur *= *zk;
            }
            v
        })
        .collect();
    let mut g = vec![vec![Complex64::ZERO; p]; p];
    let mut r = vec![Complex64::ZERO; p];
    for i in 0..p {
        for j in 0..p {
            g[i][j] = cols[i].iter().zip(&cols[j]).map(|(a, b)| a.conj() * *b).sum();
        }
        r[i] = cols[i].iter().zip(y).map(|(a, y)| a.conj() * *y).sum();
    }
    let ridge = (0..p).map(|i| g[i][i].re).fold(0.0_f64, f64::max) * 1e-12;
    for (i, row) in g.iter_mut().enumerate() {
        row[i] += Complex64::new(ridge, 0.0);
    }
    solve_complex(g, r)
}

/// One signal's tail as decimated block means: `y[m]` = mean of `d` samples starting at `first + m d`.
fn blocks(x: &[f64], first: usize, d: usize) -> Vec<f64> {
    let n = x.len().saturating_sub(first) / d;
    (0..n).map(|m| x[first + m * d..first + (m + 1) * d].iter().sum::<f64>() / d as f64).collect()
}

/// A fitted signal: the amplitudes at the first sample of the fit and the exponents `s_k` (1/s).
struct Model {
    s: Vec<Complex64>,
    c: Vec<Complex64>,
    /// Time of the first fitted sample, seconds.
    t_first: f64,
}

/// Sum of the unrecorded future of a signal whose last recorded sample is at `t_last`,
/// `sum_{m>=1} x(t_last + m dt) exp(-j w (t_last + m dt)) dt`.
fn future(model: &Model, t_last: f64, dt: f64, freqs: &[f64]) -> Vec<Complex64> {
    freqs
        .iter()
        .map(|f| {
            let w = 2.0 * std::f64::consts::PI * f;
            let mut total = Complex64::ZERO;
            for (s, c) in model.s.iter().zip(&model.c) {
                if s.re >= 0.0 {
                    continue;
                }
                // Amplitude at t_last + dt, then a geometric series.
                let at = *c * (*s * (t_last + dt - model.t_first)).exp();
                let ratio = (*s * dt).exp() * Complex64::cis(-w * dt);
                total += at * Complex64::cis(-w * (t_last + dt)) * dt / (Complex64::ONE - ratio);
            }
            total
        })
        .collect()
}

/// The amplitudes of `x` (tail from sample `first`, block length `d`) on the poles `s` (1/s) and the
/// model of the signal they give, with the relative residual of the block means (`None` when the
/// least squares fails).
fn model_on_poles(x: &[f64], first: usize, t0: f64, dt: f64, d: usize, s: &[Complex64]) -> Option<(Model, f64)> {
    let block_dt = d as f64 * dt;
    let z: Vec<Complex64> = s.iter().map(|sk| (*sk * block_dt).exp()).collect();
    let y = blocks(x, first, d);
    let a = amplitudes(&y, &z)?;
    let (mut err, mut norm) = (0.0_f64, 0.0_f64);
    let mut powers = vec![Complex64::ONE; z.len()];
    for sample in &y {
        let fitted: Complex64 = a.iter().zip(&powers).map(|(a, p)| *a * *p).sum();
        err += (Complex64::new(*sample, 0.0) - fitted).norm_sqr();
        norm += sample * sample;
        for (p, zk) in powers.iter_mut().zip(&z) {
            *p *= *zk;
        }
    }
    // The block mean of exp(s t) over d samples is exp(s t_first_of_block) * gain.
    let c = a
        .iter()
        .zip(s)
        .map(|(a, s)| {
            let gain: Complex64 = (0..d).map(|j| (*s * (j as f64 * dt)).exp()).sum::<Complex64>() / d as f64;
            *a / gain
        })
        .collect();
    let residual = (err / norm.max(1e-300)).sqrt();
    (c_is_finite(&a) && residual.is_finite())
        .then(|| (Model { s: s.to_vec(), c, t_first: t0 + first as f64 * dt }, residual))
}

fn c_is_finite(values: &[Complex64]) -> bool {
    values.iter().all(|v| v.re.is_finite() && v.im.is_finite())
}

/// The transforms of a port's voltage and current records completed by the fitted ring-down, and
/// what completed them (so that the other ports of the run can be completed with the same poles).
#[derive(Clone, Debug)]
pub struct Completed {
    pub v: Vec<Complex64>,
    pub i: Vec<Complex64>,
    /// The natural frequencies `s_k` of the chosen order, 1/s.
    pub poles: Vec<Complex64>,
    /// Samples per block of the decimated tail the poles were fitted on.
    pub decimation: usize,
}

impl Completed {
    /// `V / I`.
    pub fn z(&self) -> Vec<Complex64> {
        self.v.iter().zip(&self.i).map(|(v, i)| *v / *i).collect()
    }
}

/// Largest relative residual of a passive port's tail on the excited port's poles for which its
/// completion is used.
pub const PASSIVE_RESIDUAL_LIMIT: f64 = 0.2;

impl Record {
    /// This record's transforms completed with the poles of another port's fit (`poles`, tail blocks
    /// of `decimation` samples from `tail_start`): the passive ports of a run ring at the natural
    /// frequencies of the loop, so the same exponents describe their tail. `None` when the amplitudes
    /// cannot be fitted or the tail is not described by those poles (relative residual above
    /// [`PASSIVE_RESIDUAL_LIMIT`]): the caller falls back and says so.
    pub fn complete_with(
        &self,
        freqs: &[f64],
        tail_start: f64,
        poles: &[Complex64],
        decimation: usize,
    ) -> Option<(Vec<Complex64>, Vec<Complex64>)> {
        if self.i.len() < 8 || self.v.len() < 8 || poles.is_empty() {
            return None;
        }
        let first_i = ((tail_start - self.t0_i) / self.dt).ceil().max(0.0) as usize;
        let first_v = ((tail_start - self.t0_v) / self.dt).ceil().max(0.0) as usize;
        let (mi, ri) = model_on_poles(&self.i, first_i, self.t0_i, self.dt, decimation, poles)?;
        let (mv, rv) = model_on_poles(&self.v, first_v, self.t0_v, self.dt, decimation, poles)?;
        if ri > PASSIVE_RESIDUAL_LIMIT || rv > PASSIVE_RESIDUAL_LIMIT {
            return None;
        }
        let t_last_v = self.t0_v + (self.v.len() - 1) as f64 * self.dt;
        let t_last_i = self.t0_i + (self.i.len() - 1) as f64 * self.dt;
        let fv = ems::post::dft(self.t0_v, self.dt, &self.v, freqs);
        let fi = ems::post::dft(self.t0_i, self.dt, &self.i, freqs);
        let tv = future(&mv, t_last_v, self.dt, freqs);
        let ti = future(&mi, t_last_i, self.dt, freqs);
        Some((fv.iter().zip(&tv).map(|(a, b)| *a + *b).collect(), fi.iter().zip(&ti).map(|(a, b)| *a + *b).collect()))
    }
}

/// A pole whose amplitude over the fitted tail grows by more than this factor counts as unstable.
pub const GROWTH_LIMIT: f64 = 1.05;

/// Everything one model order gave.
struct OrderFit {
    inductance: f64,
    done: Completed,
    ring: Vec<Complex64>,
    mi: Model,
    mv: Model,
    unstable: usize,
    max_growth: f64,
}

/// The fits of all orders to one (possibly cut) record and the choice among them.
struct Fits {
    orders: Vec<(usize, Option<f64>)>,
    fits: Vec<OrderFit>,
    chosen: usize,
    median: f64,
    spread: f64,
    tail_s: f64,
}

impl Fits {
    fn info(&self) -> FitInfo {
        let c = &self.fits[self.chosen];
        FitInfo {
            orders: self.orders.clone(),
            inductance: self.median,
            spread: self.spread,
            ring_hz: c.ring.first().map(|c| (c.re, c.im)),
            tail_s: self.tail_s,
            unstable_poles: c.unstable,
            max_growth: c.max_growth,
        }
    }
}

impl Record {
    /// `Z(f)` from the recorded samples before `t_cut` (all if `None`) completed by the exponentials
    /// fitted to the tail from `tail_start` seconds on; `band` is the range over which the orders are
    /// compared. `None` when no order fits (the tail is too short for the lowest order).
    pub fn z_fitted(
        &self,
        freqs: &[f64],
        t_cut: Option<f64>,
        tail_start: f64,
        band: (f64, f64),
    ) -> Option<(Vec<Complex64>, FitInfo)> {
        self.completed(freqs, t_cut, tail_start, band).map(|(done, info)| (done.z(), info))
    }

    /// Like [`z_fitted`](Self::z_fitted), but the completed voltage and current transforms and the poles
    /// of the order that gave the median inductance, not only their quotient.
    pub fn completed(
        &self,
        freqs: &[f64],
        t_cut: Option<f64>,
        tail_start: f64,
        band: (f64, f64),
    ) -> Option<(Completed, FitInfo)> {
        let fits = self.fit_orders(freqs, t_cut, tail_start, band)?;
        let info = fits.info();
        let chosen = fits.fits.into_iter().nth(fits.chosen)?;
        Some((chosen.done, info))
    }

    fn fit_orders(&self, freqs: &[f64], t_cut: Option<f64>, tail_start: f64, band: (f64, f64)) -> Option<Fits> {
        let t_cut = t_cut.unwrap_or(f64::INFINITY);
        let n_i = self.i.iter().enumerate().take_while(|(k, _)| self.t0_i + *k as f64 * self.dt <= t_cut).count();
        let n_v = self.v.iter().enumerate().take_while(|(k, _)| self.t0_v + *k as f64 * self.dt <= t_cut).count();
        let (vi, ii) = (&self.v[..n_v], &self.i[..n_i]);
        if ii.len() < 8 || vi.len() < 8 {
            return None;
        }
        let first_i = ((tail_start - self.t0_i) / self.dt).ceil().max(0.0) as usize;
        let first_v = ((tail_start - self.t0_v) / self.dt).ceil().max(0.0) as usize;
        let tail = (ii.len().saturating_sub(first_i)) as f64 * self.dt;
        // About 240 decimated samples, at least 8 raw samples per block.
        let d = ((ii.len().saturating_sub(first_i)) / 240).max(1);
        let yi = blocks(ii, first_i, d);
        let fv = ems::post::dft(self.t0_v, self.dt, vi, freqs);
        let fi = ems::post::dft(self.t0_i, self.dt, ii, freqs);
        let t_last_v = self.t0_v + (vi.len() - 1) as f64 * self.dt;
        let t_last_i = self.t0_i + (ii.len() - 1) as f64 * self.dt;
        let block_dt = d as f64 * self.dt;
        let mut orders = Vec::new();
        let mut fits: Vec<OrderFit> = Vec::new();
        let max_order = (yi.len() / 4).min(16);
        for p in (2..=max_order).step_by(2) {
            let result = (|| {
                let (z, growth) = poles_of(&yi, p)?;
                let s: Vec<Complex64> = z.iter().map(|zk| zk.ln() / block_dt).collect();
                let mi = model_on_poles(ii, first_i, self.t0_i, self.dt, d, &s)?.0;
                let mv = model_on_poles(vi, first_v, self.t0_v, self.dt, d, &s)?.0;
                let tv = future(&mv, t_last_v, self.dt, freqs);
                let ti = future(&mi, t_last_i, self.dt, freqs);
                let done = Completed {
                    v: fv.iter().zip(&tv).map(|(a, b)| *a + *b).collect(),
                    i: fi.iter().zip(&ti).map(|(a, b)| *a + *b).collect(),
                    poles: s.clone(),
                    decimation: d,
                };
                let (l, _) = crate::solve::inductance_of(freqs, &done.z(), band.0, band.1)?;
                // Growth of the poles that carry amplitude, over the blocks of the fitted tail.
                let big = mi.c.iter().map(|c| c.norm()).fold(0.0_f64, f64::max);
                let blocks_in_tail = yi.len() as f64;
                let growths: Vec<f64> = growth
                    .iter()
                    .zip(&mi.c)
                    .filter(|(_, c)| c.norm() >= 1e-3 * big)
                    .map(|(g, _)| (g.max(0.0) * blocks_in_tail).exp())
                    .collect();
                let max_growth = growths.iter().copied().fold(1.0_f64, f64::max);
                let unstable = growths.iter().filter(|g| **g > GROWTH_LIMIT).count();
                let ring = ring_of(&s, &mi.c, 1e-3);
                Some(OrderFit { inductance: l, done, ring, mi, mv, unstable, max_growth })
            })();
            match result {
                Some(fit) if fit.inductance.is_finite() => {
                    orders.push((p, Some(fit.inductance)));
                    fits.push(fit);
                }
                _ => orders.push((p, None)),
            }
        }
        if fits.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = fits.iter().map(|b| b.inductance).collect();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let spread = (sorted[sorted.len() - 1] - sorted[0]) / median;
        // The order whose inductance is the median; its lowest pole of significant amplitude is the ring.
        let chosen = (0..fits.len())
            .min_by(|a, b| (fits[*a].inductance - median).abs().total_cmp(&(fits[*b].inductance - median).abs()))?;
        Some(Fits { orders, fits, chosen, median, spread, tail_s: tail })
    }
}

/// What the withheld-tail test saw.
#[derive(Clone, Debug, PartialEq)]
pub struct TailPrediction {
    /// Fraction of the fitted tail that was withheld, and its length in seconds.
    pub fraction: f64,
    pub held_s: f64,
    /// Relative rms error of the current and of the voltage predicted over the withheld tail by the fit of the
    /// first part (`|predicted - recorded| / |recorded|`, both rms over the withheld samples).
    pub error_i: f64,
    pub error_v: f64,
    /// Spread of the orders on the head alone.
    pub head_spread: f64,
    /// Frequency of the lowest ring pole of the head fit that carries amplitude, hertz.
    pub ring_hz: Option<f64>,
}

impl TailPrediction {
    /// The larger of the two errors.
    pub fn error(&self) -> f64 {
        self.error_i.max(self.error_v)
    }
}

/// The signal of `model` at time `t`.
fn evaluate(model: &Model, t: f64) -> f64 {
    model.s.iter().zip(&model.c).map(|(s, c)| (*c * (*s * (t - model.t_first)).exp()).re).sum()
}

impl Record {
    /// Fit the first `1 - fraction` of the tail (from `tail_start` to the last sample) and predict the rest: the
    /// relative error of the predicted voltage and current over the withheld samples. `None` when the head is too
    /// short to fit or there is nothing to withhold.
    pub fn tail_prediction(
        &self,
        freqs: &[f64],
        tail_start: f64,
        band: (f64, f64),
        fraction: f64,
    ) -> Option<TailPrediction> {
        let t_end = (self.t0_i + (self.i.len().checked_sub(1)?) as f64 * self.dt)
            .min(self.t0_v + (self.v.len().checked_sub(1)?) as f64 * self.dt);
        let split = tail_start + (1.0 - fraction) * (t_end - tail_start);
        if t_end <= tail_start || split <= tail_start {
            return None;
        }
        let fits = self.fit_orders(freqs, Some(split), tail_start, band)?;
        let chosen = &fits.fits[fits.chosen];
        let error = |x: &[f64], t0: f64, model: &Model| -> Option<f64> {
            let (mut err, mut norm) = (0.0_f64, 0.0_f64);
            for (k, v) in x.iter().enumerate() {
                let t = t0 + k as f64 * self.dt;
                if t > split {
                    let d = evaluate(model, t) - v;
                    err += d * d;
                    norm += v * v;
                }
            }
            (norm > 0.0).then(|| (err / norm).sqrt())
        };
        Some(TailPrediction {
            fraction,
            held_s: t_end - split,
            error_i: error(&self.i, self.t0_i, &chosen.mi)?,
            error_v: error(&self.v, self.t0_v, &chosen.mv)?,
            head_spread: fits.spread,
            ring_hz: fits.info().ring_hz.map(|(f, _)| f),
        })
    }
}

/// Largest relative rms error of the withheld tail for which a fit is accepted. Chosen from the validation
/// cases (`docs/emi-noise-analysis.md`, section 11, "Acceptance of the ring-down fit").
pub const HOLDOUT_ERROR_LIMIT: f64 = 0.10;
/// Fraction of the fitted tail withheld from the fit and predicted.
pub const HOLDOUT_FRACTION: f64 = 0.3;
/// Largest spread of the model orders (`(max - min) / median` of the inductance) for which a fit is accepted.
pub const SPREAD_LIMIT: f64 = 0.05;
/// How far below zero `Re Z / |Z|` of the completed response may go anywhere in the band: the loop is lossless
/// in the model, so `Re Z` is small and of either sign by numerical noise, and only a clearly negative resistance
/// is a violation.
pub const PASSIVITY_LIMIT: f64 = 0.05;

/// The evidence for or against a ring-down fit, and the violations found. A fit is used only when `violations`
/// is empty; otherwise the plain transform of the truncated record stays and the result is indicative.
#[derive(Clone, Debug, PartialEq)]
pub struct FitCheck {
    pub holdout: Option<TailPrediction>,
    pub spread: f64,
    pub unstable_poles: usize,
    pub max_growth: f64,
    /// Smallest `Re Z / |Z|` of the completed response over the whole analysis band.
    pub min_re_z_ratio: f64,
    pub violations: Vec<String>,
}

impl FitCheck {
    /// Whether the fit passed every check.
    pub fn accepted(&self) -> bool {
        self.violations.is_empty()
    }

    /// One line for the report.
    pub fn summary(&self) -> String {
        let hold = self.holdout.as_ref().map_or("no withheld-tail test".to_owned(), |h| {
            format!(
                "withheld {:.0} % of the tail ({:.1} ns) predicted to {:.1} % (current) and {:.1} % (voltage)",
                100.0 * h.fraction,
                h.held_s * 1e9,
                100.0 * h.error_i,
                100.0 * h.error_v
            )
        });
        format!(
            "{hold}; orders agree to {:.2} %; {} unstable pole(s) (largest growth {:.3}); min Re Z / |Z| {:.3}",
            100.0 * self.spread,
            self.unstable_poles,
            self.max_growth,
            self.min_re_z_ratio
        )
    }
}

/// Check a completed fit: the withheld-tail prediction, the spread of the orders, the stability of the poles
/// and the passivity of the completed port response (`Re Z >= 0` up to [`PASSIVITY_LIMIT`]).
pub fn check_fit(info: &FitInfo, holdout: Option<TailPrediction>, z: &[Complex64]) -> FitCheck {
    let min_re_z_ratio = z.iter().map(|v| v.re / v.norm().max(1e-300)).fold(f64::INFINITY, f64::min);
    let mut violations = Vec::new();
    match &holdout {
        None => violations
            .push("no withheld-tail test could be made (the tail is too short to fit and predict)".to_owned()),
        Some(h) if h.error() > HOLDOUT_ERROR_LIMIT => violations.push(format!(
            "the fit of the first {:.0} % of the tail predicts the withheld {:.1} ns to {:.1} %, above the {:.0} % limit",
            100.0 * (1.0 - h.fraction),
            h.held_s * 1e9,
            100.0 * h.error(),
            100.0 * HOLDOUT_ERROR_LIMIT
        )),
        Some(h) => {
            // A prediction over less than one period of the ring says little about the ring.
            let needed = h.ring_hz.map_or(1.0e-9, |f| 1.0 / f);
            if h.held_s < needed {
                violations.push(format!(
                    "the withheld tail ({:.2} ns) is shorter than one period of the ring ({:.2} ns): too short to test the fit",
                    h.held_s * 1e9,
                    needed * 1e9
                ));
            }
        }
    }
    if info.spread >= SPREAD_LIMIT {
        violations.push(format!(
            "the model orders disagree by {:.1} %, above the {:.0} % limit",
            100.0 * info.spread,
            100.0 * SPREAD_LIMIT
        ));
    }
    if info.unstable_poles > 0 {
        violations.push(format!(
            "{} fitted pole(s) grow over the tail (largest factor {:.3}, limit {GROWTH_LIMIT}): the fit reflected them into the unit circle",
            info.unstable_poles, info.max_growth
        ));
    }
    if min_re_z_ratio.is_nan() || min_re_z_ratio < -PASSIVITY_LIMIT {
        violations.push(format!(
            "the completed port response is not passive: Re Z / |Z| reaches {min_re_z_ratio:.3} (limit -{PASSIVITY_LIMIT})"
        ));
    }
    FitCheck {
        holdout,
        spread: info.spread,
        unstable_poles: info.unstable_poles,
        max_growth: info.max_growth,
        min_re_z_ratio,
        violations,
    }
}

impl Record {
    /// The fit of this record's tail and the evidence for it: the completed spectra, the fit and the
    /// [`FitCheck`] (also when the check fails: the caller decides what to do with a rejected fit).
    pub fn checked_fit(
        &self,
        freqs: &[f64],
        tail_start: f64,
        band: (f64, f64),
    ) -> Option<(Completed, FitInfo, FitCheck)> {
        let (done, info) = self.completed(freqs, None, tail_start, band)?;
        let holdout = self.tail_prediction(freqs, tail_start, band, HOLDOUT_FRACTION);
        let check = check_fit(&info, holdout, &done.z());
        Some((done, info, check))
    }
}

/// `(frequency, Q)` of the lowest-frequency pole whose amplitude is at least `rel` of the largest, as a
/// one-element vector (empty when none): Q = |s| / (2 |Re s|).
fn ring_of(s: &[Complex64], c: &[Complex64], rel: f64) -> Vec<Complex64> {
    let big = c.iter().map(|c| c.norm()).fold(0.0_f64, f64::max);
    let mut found: Option<(f64, f64)> = None;
    for (s, c) in s.iter().zip(c) {
        if c.norm() < rel * big || s.im <= 0.0 {
            continue;
        }
        let f = s.im / (2.0 * std::f64::consts::PI);
        let q = s.norm() / (2.0 * s.re.abs().max(1e-300));
        if found.is_none_or(|(g, _)| f < g) {
            found = Some((f, q));
        }
    }
    found.map(|(f, q)| vec![Complex64::new(f, q)]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-port: a source resistance r0 driving an L in parallel with C: V and I after the pulse are a
    /// damped sinusoid; the fit of a truncated record must give the transform of the full one.
    #[test]
    fn a_truncated_ring_down_is_completed() {
        let dt = 2e-12;
        let f0 = 400e6;
        let q = 25.0;
        let alpha = std::f64::consts::PI * f0 / q;
        let w = 2.0 * std::f64::consts::PI * f0;
        let n_full = 200_000;
        // Current: a decaying start then the ring; the voltage is -50 I plus a source pulse that ends before 1 ns.
        let i: Vec<f64> = (0..n_full)
            .map(|n| {
                let t = n as f64 * dt;
                1e-3 * (-alpha * t).exp() * (w * t).cos() + 4e-4 * (-t / 0.4e-9).exp()
            })
            .collect();
        let v: Vec<f64> = i
            .iter()
            .enumerate()
            .map(|(n, x)| {
                let t = n as f64 * dt;
                -50.0 * x + 0.1 * (-((t - 0.4e-9) / 0.1e-9).powi(2)).exp()
            })
            .collect();
        let rec = Record { dt, t0_v: 0.0, t0_i: 0.0, v: v.clone(), i: i.clone() };
        let freqs: Vec<f64> = (1..=40).map(|k| k as f64 * 25e6).collect();
        let full = rec.z_at(&freqs, None, crate::solve::Window::Rectangular);
        // The full record is 400 ns long (Q 25 at 400 MHz: 20 e-foldings of the amplitude); cut at 10 ns.
        let cut = Record { dt, t0_v: 0.0, t0_i: 0.0, v: v[..5000].to_vec(), i: i[..5000].to_vec() };
        let plain = cut.z_at(&freqs, None, crate::solve::Window::Rectangular);
        let (fitted, info) = cut.z_fitted(&freqs, None, 1.0e-9, (50e6, 150e6)).expect("fit");
        let err =
            |a: &[Complex64]| a.iter().zip(&full).map(|(x, y)| (*x - *y).norm() / y.norm()).fold(0.0_f64, f64::max);
        assert!(err(&fitted) < 0.01, "fitted error {} (plain {}), {info:?}", err(&fitted), err(&plain));
        assert!(err(&plain) > 5.0 * err(&fitted));
        let (f, qq) = info.ring_hz.expect("ring");
        assert!((f / f0 - 1.0).abs() < 0.01 && (qq / q - 1.0).abs() < 0.1, "{f} {qq}");
    }

    /// A passive one-port: `V = R I + L dI/dt` for a current that is a decaying start and a ring of `f0` and `q`
    /// (`n` samples of 2 ps): `Z = R + j w L` exactly, so a transform of the truncated record is wrong and the
    /// completed one can be compared with the exact response. `late` adds a mode `(frequency, start time, amplitude)`
    /// that the first part of a record does not contain.
    fn passive_record(n: usize, r: f64, l: f64, late: Option<(f64, f64, f64)>, growth: f64) -> Record {
        let dt = 2e-12;
        let (f0, q) = (400e6, 25.0);
        let alpha = std::f64::consts::PI * f0 / q;
        let w = 2.0 * std::f64::consts::PI * f0;
        let (t1, t2) = (0.4e-9, 0.1e-9);
        let current = |t: f64| {
            // Zero at t = 0, so that V = R I + L dI/dt has no impulse there and Z = R + j w L exactly.
            let mut i = 1e-3 * (-alpha * t).exp() * (w * t).sin() + 4e-4 * ((-t / t1).exp() - (-t / t2).exp());
            let mut di = 1e-3 * (-alpha * t).exp() * (w * (w * t).cos() - alpha * (w * t).sin())
                + 4e-4 * (-(-t / t1).exp() / t1 + (-t / t2).exp() / t2);
            if let Some((f, t0, amp)) = late
                && t >= t0
            {
                let wl = 2.0 * std::f64::consts::PI * f;
                i += amp * (wl * (t - t0)).sin() * (-(t - t0) / 30e-9).exp();
                di += amp * (wl * (wl * (t - t0)).cos() - (wl * (t - t0)).sin() / 30e-9) * (-(t - t0) / 30e-9).exp();
            }
            if growth > 0.0 {
                i += 2e-4 * ((t * growth).exp() - (-t / 0.2e-9).exp());
                di += 2e-4 * (growth * (t * growth).exp() + (-t / 0.2e-9).exp() / 0.2e-9);
            }
            (i, di)
        };
        let (mut i, mut v) = (Vec::new(), Vec::new());
        for k in 0..n {
            let (x, dx) = current(k as f64 * dt);
            i.push(x);
            v.push(r * x + l * dx);
        }
        Record { dt, t0_v: 0.0, t0_i: 0.0, v, i }
    }

    fn cut(r: &Record, n: usize) -> Record {
        Record { v: r.v[..n].to_vec(), i: r.i[..n].to_vec(), ..r.clone() }
    }

    /// Three ports of one run: the excited one rings, the second rings on the same poles (a transfer
    /// term), the third carries a tail the poles do not describe. Cut at 10 ns out of 400 ns.
    fn three_port_records(n: usize) -> Vec<Record> {
        let dt = 2e-12;
        let (f0, q) = (400e6, 25.0);
        let alpha = std::f64::consts::PI * f0 / q;
        let w = 2.0 * std::f64::consts::PI * f0;
        let record = |i_of: &dyn Fn(f64) -> f64| {
            let i: Vec<f64> = (0..n).map(|k| i_of(k as f64 * dt)).collect();
            let v: Vec<f64> = i.iter().map(|x| -50.0 * x).collect();
            Record { dt, t0_v: 0.0, t0_i: 0.0, v, i }
        };
        vec![
            passive_record(n, 50.0, 5e-9, None, 0.0),
            record(&|t| 5e-4 * (-alpha * t).exp() * (w * t + 0.7).cos() + 1e-4 * (-t / 0.5e-9).exp()),
            record(&|t| 1e-3 * (-t / 60e-9).exp() * (2.0 * std::f64::consts::PI * 233e6 * t).cos()),
        ]
    }

    const BAND: (f64, f64) = (50e6, 150e6);

    fn grid() -> Vec<f64> {
        (1..=40).map(|k| k as f64 * 25e6).collect()
    }

    #[test]
    fn a_clean_ring_is_accepted_with_a_small_withheld_tail_error_and_a_passive_response() {
        let full = passive_record(200_000, 50.0, 5e-9, None, 0.0);
        let rec = cut(&full, 5000);
        let (done, _, check) = rec.checked_fit(&grid(), 1.0e-9, BAND).expect("fit");
        let hold = check.holdout.as_ref().expect("withheld-tail test");
        assert!(check.accepted(), "{:?}", check.violations);
        assert!(hold.error() < 0.01, "withheld-tail error {:.4}", hold.error());
        assert!(check.unstable_poles == 0 && check.min_re_z_ratio > 0.5, "{}", check.summary());
        // The completed response is the exact one, R + j w L, over the whole band.
        for (f, z) in grid().iter().zip(done.z()) {
            let want = Complex64::new(50.0, 2.0 * std::f64::consts::PI * f * 5e-9);
            assert!((z - want).norm() / want.norm() < 0.01, "{f}: {z} vs {want}");
        }
    }

    #[test]
    fn a_mode_that_the_fitted_part_does_not_contain_fails_the_withheld_tail_test() {
        // A second ring starts at 7.5 ns, inside the last 30 % of the tail of a record cut at 10 ns.
        let rec = cut(&passive_record(200_000, 50.0, 5e-9, Some((233e6, 7.5e-9, 1e-3)), 0.0), 5000);
        let (_, _, check) = rec.checked_fit(&grid(), 1.0e-9, BAND).expect("fit");
        let hold = check.holdout.as_ref().expect("withheld-tail test");
        assert!(hold.error() > HOLDOUT_ERROR_LIMIT, "withheld-tail error {:.3}", hold.error());
        assert!(!check.accepted() && check.violations.iter().any(|v| v.contains("withheld")), "{:?}", check.violations);
    }

    /// How the withheld-tail error maps to the error of the completion, on records whose exact response is known:
    /// a second ring of growing amplitude starts inside the withheld part of the tail. The table is the basis of
    /// [`HOLDOUT_ERROR_LIMIT`] (docs section 11): the limit sits where the completion's inductance, read over the
    /// band, is still within a few percent of the exact one.
    #[test]
    fn the_withheld_tail_error_bounds_the_error_of_the_completion() {
        let l_exact = 5e-9;
        let mut rows = Vec::new();
        for amp in [1e-6, 1e-5, 1e-4, 3e-4, 1e-3, 3e-3] {
            let rec = cut(&passive_record(200_000, 50.0, l_exact, Some((233e6, 7.5e-9, amp)), 0.0), 5000);
            let Some((done, info, check)) = rec.checked_fit(&grid(), 1.0e-9, BAND) else { continue };
            let z = done.z();
            let (l, _) = crate::solve::inductance_of(&grid(), &z, BAND.0, BAND.1).expect("band");
            let max = grid()
                .iter()
                .zip(&z)
                .map(|(f, z)| (*z - Complex64::new(50.0, 2.0 * std::f64::consts::PI * f * l_exact)).norm() / z.norm())
                .fold(0.0_f64, f64::max);
            let hold = check.holdout.as_ref().map_or(f64::NAN, |h| h.error());
            println!(
                "late mode {amp:.0e} A: withheld error {:6.2} %, completion L {:+7.2} %, response max {:6.2} %, spread {:.2} %, {}",
                100.0 * hold,
                100.0 * (l / l_exact - 1.0),
                100.0 * max,
                100.0 * info.spread,
                if check.accepted() { "accepted" } else { "rejected" }
            );
            rows.push((hold, (l / l_exact - 1.0).abs(), check.accepted()));
        }
        // An accepted fit has a completion within 3 % in L; the withheld error rises with the late mode.
        for (hold, l_err, accepted) in &rows {
            if *accepted {
                assert!(*hold <= HOLDOUT_ERROR_LIMIT && *l_err < 0.03, "{hold} {l_err}");
            }
        }
        assert!(
            rows.windows(2).all(|w| w[1].0 >= w[0].0 * 0.9),
            "the withheld error must rise with the late mode: {rows:?}"
        );
        assert!(rows.iter().any(|r| r.2) && rows.iter().any(|r| !r.2));
    }

    #[test]
    fn a_growing_pole_is_counted_and_rejected() {
        // A mode growing by e^(t / 4 ns): 9 ns of tail is a factor 9.
        let rec = cut(&passive_record(200_000, 50.0, 5e-9, None, 1.0 / 4e-9), 5000);
        let (_, info, check) = rec.checked_fit(&grid(), 1.0e-9, BAND).expect("fit");
        assert!(info.unstable_poles >= 1 && info.max_growth > GROWTH_LIMIT, "{info:?}");
        assert!(!check.accepted() && check.violations.iter().any(|v| v.contains("grow")), "{:?}", check.violations);
    }

    #[test]
    fn a_negative_resistance_is_a_passivity_violation() {
        let rec = cut(&passive_record(200_000, -50.0, 1e-9, None, 0.0), 5000);
        let (_, _, check) = rec.checked_fit(&grid(), 1.0e-9, BAND).expect("fit");
        assert!(check.min_re_z_ratio < -PASSIVITY_LIMIT, "{}", check.summary());
        assert!(!check.accepted() && check.violations.iter().any(|v| v.contains("passive")), "{:?}", check.violations);
        // The same record with a positive resistance is accepted, so the violation is the sign and nothing else.
        let ok = cut(&passive_record(200_000, 50.0, 1e-9, None, 0.0), 5000);
        assert!(ok.checked_fit(&grid(), 1.0e-9, BAND).expect("fit").2.accepted());
    }

    #[test]
    fn a_tail_too_short_to_withhold_part_of_it_is_rejected_not_accepted_unchecked() {
        let rec = cut(&passive_record(200_000, 50.0, 5e-9, None, 0.0), 700);
        match rec.checked_fit(&grid(), 1.0e-9, BAND) {
            None => {}
            Some((_, _, check)) => assert!(!check.accepted(), "{}", check.summary()),
        }
    }

    #[test]
    fn a_rejected_fit_leaves_the_plain_transform_and_says_why() {
        use crate::solve::port_responses;
        let late: Vec<Record> = vec![cut(&passive_record(200_000, 50.0, 5e-9, Some((233e6, 7.5e-9, 1e-3)), 0.0), 5000)];
        let plain = port_responses(&late, &[50.0], 0, &grid(), None).expect("plain");
        let done = port_responses(&late, &[50.0], 0, &grid(), Some((1.0e-9, BAND))).expect("responses");
        assert!(done.fit.is_none(), "a rejected fit must not be used");
        let check = done.fit_check.expect("the evidence is kept");
        assert!(!check.accepted());
        for (a, b) in done.z_in.iter().zip(&plain.z_in) {
            assert!((*a - *b).norm() < 1e-12 * b.norm().max(1.0));
        }
    }

    #[test]
    fn completed_responses_give_z_and_s_from_the_same_spectra() {
        use crate::solve::port_responses;
        let freqs: Vec<f64> = (1..=40).map(|k| k as f64 * 25e6).collect();
        let z_refs = [50.0, 50.0, 50.0];
        let completion = Some((1.0e-9, (50e6, 150e6)));
        let full = port_responses(&three_port_records(200_000), &z_refs, 0, &freqs, None).expect("full");
        let cut_records: Vec<Record> = three_port_records(200_000)
            .into_iter()
            .map(|r| Record { v: r.v[..5000].to_vec(), i: r.i[..5000].to_vec(), ..r })
            .collect();
        let plain = port_responses(&cut_records, &z_refs, 0, &freqs, None).expect("plain");
        let done = port_responses(&cut_records, &z_refs, 0, &freqs, completion).expect("completed");
        assert!(done.fit.is_some());
        // S11 is the reflection of the returned Z.
        for (z, s) in done.z_in.iter().zip(&done.s[0]) {
            let want = (*z - 50.0) / (*z + 50.0);
            assert!((*s - want).norm() < 1e-9, "{s} vs {want}");
        }
        // The plain transform does not satisfy it once Z alone is corrected: the old mixed result.
        let mixed = done
            .z_in
            .iter()
            .zip(&plain.s[0])
            .map(|(z, s)| (*s - (*z - 50.0) / (*z + 50.0)).norm())
            .fold(0.0_f64, f64::max);
        assert!(mixed > 1e-3, "{mixed}");
        // The transfer term of the passive port is completed too.
        let err = |a: &[Complex64], b: &[Complex64]| {
            a.iter().zip(b).map(|(x, y)| (*x - *y).norm() / y.norm().max(1e-12)).fold(0.0_f64, f64::max)
        };
        assert!(err(&done.s[1], &full.s[1]) < 0.02, "completed {}", err(&done.s[1], &full.s[1]));
        assert!(err(&plain.s[1], &full.s[1]) > 5.0 * err(&done.s[1], &full.s[1]));
        // The third port's tail is not on those poles: plain transform, flagged, and not close to the full record's.
        assert_eq!(done.uncorrected, vec![2]);
        assert!(err(&done.s[2], &full.s[2]) > 0.05, "{}", err(&done.s[2], &full.s[2]));
    }

    #[test]
    fn without_a_usable_fit_nothing_is_completed() {
        use crate::solve::port_responses;
        let freqs: Vec<f64> = (1..=40).map(|k| k as f64 * 25e6).collect();
        let cut: Vec<Record> = three_port_records(200_000)
            .into_iter()
            .map(|r| Record { v: r.v[..300].to_vec(), i: r.i[..300].to_vec(), ..r })
            .collect();
        let done = port_responses(&cut, &[50.0; 3], 0, &freqs, Some((1.0e-9, (50e6, 150e6)))).expect("responses");
        assert!(done.fit.is_none() && done.uncorrected.is_empty());
        for (z, s) in done.z_in.iter().zip(&done.s[0]) {
            assert!((*s - (*z - 50.0) / (*z + 50.0)).norm() < 1e-9);
        }
    }
}

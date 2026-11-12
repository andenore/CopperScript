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
//! median taken; the spread of the orders is the fit's uncertainty.

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
fn poles_of(y: &[f64], p: usize) -> Option<Vec<Complex64>> {
    let l = y.len();
    if l < 3 * p + 2 {
        return None;
    }
    // y[m] = -sum_j a[j] y[m-1-j] for m = p..l.
    let a: Vec<Vec<f64>> = (p..l).map(|m| (0..p).map(|j| y[m - 1 - j]).collect()).collect();
    let b: Vec<f64> = (p..l).map(|m| -y[m]).collect();
    let coef = lstsq(&a, &b, 1e-10)?;
    let mut z = roots(&coef);
    for r in &mut z {
        if r.norm() > 1.0 {
            *r = 1.0 / r.conj();
        }
        if r.norm() > 1.0 - 1e-9 {
            *r *= 1.0 - 1e-9 / r.norm().max(1e-300);
        }
    }
    Some(z)
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
        let mut best: Vec<(usize, f64, Completed, Vec<Complex64>)> = Vec::new();
        let max_order = (yi.len() / 4).min(16);
        for p in (2..=max_order).step_by(2) {
            let result = (|| {
                let z = poles_of(&yi, p)?;
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
                Some((l, done, s, mi.c))
            })();
            match result {
                Some((l, done, s, c)) if l.is_finite() => {
                    orders.push((p, Some(l)));
                    best.push((p, l, done, ring_of(&s, &c, 1e-3)));
                }
                _ => orders.push((p, None)),
            }
        }
        if best.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = best.iter().map(|b| b.1).collect();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let spread = (sorted[sorted.len() - 1] - sorted[0]) / median;
        // The order whose inductance is the median; its lowest pole of significant amplitude is the ring.
        let chosen = best.iter().min_by(|a, b| (a.1 - median).abs().total_cmp(&(b.1 - median).abs()))?;
        let ring = chosen.3.first().map(|c| (c.re, c.im));
        Some((chosen.2.clone(), FitInfo { orders, inductance: median, spread, ring_hz: ring, tail_s: tail }))
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

    /// Three ports of one run: the excited one rings, the second rings on the same poles (a transfer
    /// term), the third carries a tail the poles do not describe. Cut at 10 ns out of 400 ns.
    fn three_port_records(n: usize) -> Vec<Record> {
        let dt = 2e-12;
        let (f0, q) = (400e6, 25.0);
        let alpha = std::f64::consts::PI * f0 / q;
        let w = 2.0 * std::f64::consts::PI * f0;
        let record = |i_of: &dyn Fn(f64) -> f64, v_extra: &dyn Fn(f64) -> f64| {
            let i: Vec<f64> = (0..n).map(|k| i_of(k as f64 * dt)).collect();
            let v: Vec<f64> = i.iter().enumerate().map(|(k, x)| -50.0 * x + v_extra(k as f64 * dt)).collect();
            Record { dt, t0_v: 0.0, t0_i: 0.0, v, i }
        };
        let pulse = |t: f64| 0.1 * (-((t - 0.4e-9) / 0.1e-9).powi(2)).exp();
        vec![
            record(&|t| 1e-3 * (-alpha * t).exp() * (w * t).cos() + 4e-4 * (-t / 0.4e-9).exp(), &pulse),
            record(&|t| 5e-4 * (-alpha * t).exp() * (w * t + 0.7).cos() + 1e-4 * (-t / 0.5e-9).exp(), &|_| 0.0),
            record(&|t| 1e-3 * (-t / 60e-9).exp() * (2.0 * std::f64::consts::PI * 233e6 * t).cos(), &|_| 0.0),
        ]
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
        // The third port's tail is not on those poles: plain transform, flagged.
        assert_eq!(done.uncorrected, vec![2]);
        assert!(err(&done.s[2], &plain.s[2]) < 1e-12);
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

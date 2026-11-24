//! Dense real linear algebra for the pole-zero analysis ([`crate::stability`]).
//!
//! | Routine | Method |
//! | --- | --- |
//! | [`qr_pivoted`] | Householder QR with column pivoting: `A P = Q R`, the rank where a pivot falls to `tol` of the first |
//! | [`eigenvalues`] | balancing by powers of two, Householder reduction to Hessenberg form, Francis double-shift QR (the EISPACK `balanc`, `orthes` and `hqr` sequence) |
//! | [`finite_eigenvalues`] | the finite eigenvalues of a real pencil `G + s C` with singular `C`: the infinite ones deflated by orthogonal transformations first (see the function), the slow ones from the inverted reduced pencil ([`with_slow_roots_inverted`]) |
//!
//! Everything is dense and `O(n^3)`; no dependency.

use crate::complex::Complex;
use crate::real_lu::RealLu;

/// A row-major dense real matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Mat {
    /// Number of rows.
    pub rows: usize,
    /// Number of columns.
    pub cols: usize,
    /// Entries, row-major.
    pub a: Vec<f64>,
}

impl Mat {
    /// A zero matrix.
    pub fn zeros(rows: usize, cols: usize) -> Mat {
        Mat { rows, cols, a: vec![0.0; rows * cols] }
    }

    /// The identity.
    pub fn identity(n: usize) -> Mat {
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            m.a[i * n + i] = 1.0;
        }
        m
    }

    /// Entry `(r, c)`.
    pub fn get(&self, r: usize, c: usize) -> f64 {
        self.a[r * self.cols + c]
    }

    fn set(&mut self, r: usize, c: usize, v: f64) {
        self.a[r * self.cols + c] = v;
    }

    /// The product `self * other`.
    pub fn mul(&self, other: &Mat) -> Mat {
        assert_eq!(self.cols, other.rows, "matrix product sizes");
        let mut out = Mat::zeros(self.rows, other.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let x = self.a[i * self.cols + k];
                if x == 0.0 {
                    continue;
                }
                let row = &other.a[k * other.cols..(k + 1) * other.cols];
                for (o, y) in out.a[i * other.cols..(i + 1) * other.cols].iter_mut().zip(row) {
                    *o += x * y;
                }
            }
        }
        out
    }

    /// The transpose.
    pub fn transpose(&self) -> Mat {
        let mut out = Mat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                out.a[j * self.rows + i] = self.a[i * self.cols + j];
            }
        }
        out
    }

    /// Rows `from..to`.
    pub fn row_block(&self, from: usize, to: usize) -> Mat {
        Mat { rows: to - from, cols: self.cols, a: self.a[from * self.cols..to * self.cols].to_vec() }
    }

    /// Columns `from..to`.
    pub fn col_block(&self, from: usize, to: usize) -> Mat {
        let mut out = Mat::zeros(self.rows, to - from);
        for i in 0..self.rows {
            for j in from..to {
                out.a[i * (to - from) + j - from] = self.a[i * self.cols + j];
            }
        }
        out
    }

    /// The rows of `self` followed by those of `other`.
    pub fn stack(&self, other: &Mat) -> Mat {
        assert_eq!(self.cols, other.cols, "stacked matrices need the same columns");
        let mut a = self.a.clone();
        a.extend_from_slice(&other.a);
        Mat { rows: self.rows + other.rows, cols: self.cols, a }
    }

    /// The largest absolute entry.
    pub fn max_abs(&self) -> f64 {
        self.a.iter().fold(0.0, |m, x| m.max(x.abs()))
    }
}

/// The result of [`qr_pivoted`]: `A P = Q R`.
#[derive(Debug, Clone)]
pub struct PivotedQr {
    /// Orthogonal, `rows x rows`.
    pub q: Mat,
    /// Upper trapezoidal, `rows x cols`; rows from `rank` on are set to zero.
    pub r: Mat,
    /// `perm[k]` is the column of `A` in position `k` of `R`.
    pub perm: Vec<usize>,
    /// The number of pivots above the tolerance.
    pub rank: usize,
}

/// Householder QR of `a` with column pivoting (the column of largest
/// remaining norm first). Elimination stops when the largest remaining
/// column norm is at most `tol` times the first pivot, or at most `floor`
/// (or zero); that count is the rank, and the rows of `R` below it are set
/// to zero.
pub fn qr_pivoted(a: &Mat, tol: f64, floor: f64) -> PivotedQr {
    let (m, n) = (a.rows, a.cols);
    let mut r = a.clone();
    let mut q = Mat::identity(m);
    let mut perm: Vec<usize> = (0..n).collect();
    let mut rank = 0;
    let mut first = 0.0f64;
    for k in 0..m.min(n) {
        // The column of largest remaining norm (recomputed: no downdating error).
        let mut best = (k, -1.0f64);
        for j in k..n {
            let s: f64 = (k..m).map(|i| r.get(i, j) * r.get(i, j)).sum();
            if s > best.1 {
                best = (j, s);
            }
        }
        let norm = best.1.max(0.0).sqrt();
        if k == 0 {
            first = norm;
        }
        if norm == 0.0 || norm <= tol * first || norm <= floor {
            break;
        }
        if best.0 != k {
            for i in 0..m {
                r.a.swap(i * n + k, i * n + best.0);
            }
            perm.swap(k, best.0);
        }
        let x0 = r.get(k, k);
        let alpha = if x0 >= 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..m).map(|i| r.get(i, k)).collect();
        v[0] -= alpha;
        let vn2: f64 = v.iter().map(|x| x * x).sum();
        if vn2 > 0.0 {
            // R <- H R on rows k.., columns k..
            for j in k..n {
                let s: f64 = v.iter().enumerate().map(|(l, vl)| vl * r.get(k + l, j)).sum();
                let f = 2.0 * s / vn2;
                for (l, vl) in v.iter().enumerate() {
                    let x = r.get(k + l, j) - f * vl;
                    r.set(k + l, j, x);
                }
            }
            // Q <- Q H on columns k..
            for i in 0..m {
                let s: f64 = v.iter().enumerate().map(|(l, vl)| vl * q.get(i, k + l)).sum();
                let f = 2.0 * s / vn2;
                for (l, vl) in v.iter().enumerate() {
                    let x = q.get(i, k + l) - f * vl;
                    q.set(i, k + l, x);
                }
            }
        }
        r.set(k, k, alpha);
        for i in k + 1..m {
            r.set(i, k, 0.0);
        }
        rank = k + 1;
    }
    for i in rank..m {
        for j in 0..n {
            r.set(i, j, 0.0);
        }
    }
    PivotedQr { q, r, perm, rank }
}

/// Why an eigenvalue computation failed.
#[derive(Debug, Clone, PartialEq)]
pub enum EigenError {
    /// The QR iteration did not converge for an eigenvalue.
    NoConvergence,
    /// The pencil is singular: `det(G + s C)` vanishes for every `s`.
    SingularPencil,
    /// A matrix entry is not finite.
    NonFinite,
}

/// Balance `h` (n x n, row-major) by a diagonal similarity of powers of two.
fn balance(h: &mut [f64], n: usize) {
    const RADIX: f64 = 2.0;
    let sqrdx = RADIX * RADIX;
    for _sweep in 0..200 {
        let mut done = true;
        for i in 0..n {
            let mut c = 0.0;
            let mut r = 0.0;
            for j in 0..n {
                if j != i {
                    c += h[j * n + i].abs();
                    r += h[i * n + j].abs();
                }
            }
            if c == 0.0 || r == 0.0 {
                continue;
            }
            let mut g = r / RADIX;
            let mut f = 1.0;
            let s = c + r;
            while c < g {
                f *= RADIX;
                c *= sqrdx;
            }
            g = r * RADIX;
            while c > g {
                f /= RADIX;
                c /= sqrdx;
            }
            if (c + r) / f < 0.95 * s {
                done = false;
                let g = 1.0 / f;
                for j in 0..n {
                    h[i * n + j] *= g;
                }
                for j in 0..n {
                    h[j * n + i] *= f;
                }
            }
        }
        if done {
            break;
        }
    }
}

/// Reduce `h` to upper Hessenberg form by Householder similarities.
fn hessenberg(h: &mut [f64], n: usize) {
    if n < 3 {
        return;
    }
    for k in 0..n - 2 {
        let norm: f64 = (k + 1..n).map(|i| h[i * n + k] * h[i * n + k]).sum::<f64>().sqrt();
        if norm == 0.0 {
            continue;
        }
        let x0 = h[(k + 1) * n + k];
        let alpha = if x0 >= 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k + 1..n).map(|i| h[i * n + k]).collect();
        v[0] -= alpha;
        let vn2: f64 = v.iter().map(|x| x * x).sum();
        if vn2 == 0.0 {
            continue;
        }
        // h <- H h (rows k+1..)
        for j in 0..n {
            let s: f64 = v.iter().enumerate().map(|(l, vl)| vl * h[(k + 1 + l) * n + j]).sum();
            let f = 2.0 * s / vn2;
            for (l, vl) in v.iter().enumerate() {
                h[(k + 1 + l) * n + j] -= f * vl;
            }
        }
        // h <- h H (columns k+1..)
        for i in 0..n {
            let s: f64 = v.iter().enumerate().map(|(l, vl)| vl * h[i * n + k + 1 + l]).sum();
            let f = 2.0 * s / vn2;
            for (l, vl) in v.iter().enumerate() {
                h[i * n + k + 1 + l] -= f * vl;
            }
        }
        h[(k + 1) * n + k] = alpha;
        for i in k + 2..n {
            h[i * n + k] = 0.0;
        }
    }
}

/// The eigenvalues of an upper Hessenberg matrix by the Francis double-shift
/// QR iteration (EISPACK `hqr`, eigenvalues only).
fn hqr(hm: &[f64], n: usize) -> Result<Vec<Complex>, EigenError> {
    // 1-based copy, as the classic formulation indexes it.
    let w = n + 1;
    let mut a = vec![0.0; w * w];
    for i in 0..n {
        for j in 0..n {
            a[(i + 1) * w + j + 1] = hm[i * n + j];
        }
    }
    let ix = |i: usize, j: usize| i * w + j;
    let mut wr = vec![0.0; n + 1];
    let mut wi = vec![0.0; n + 1];
    let mut anorm = 0.0;
    for i in 1..=n {
        for j in i.saturating_sub(1).max(1)..=n {
            anorm += a[ix(i, j)].abs();
        }
    }
    let sign = |a: f64, b: f64| if b >= 0.0 { a.abs() } else { -a.abs() };
    let mut nn = n;
    let mut t = 0.0;
    while nn >= 1 {
        let mut its = 0;
        loop {
            let mut l = nn;
            while l >= 2 {
                let mut s = a[ix(l - 1, l - 1)].abs() + a[ix(l, l)].abs();
                if s == 0.0 {
                    s = anorm;
                }
                if a[ix(l, l - 1)].abs() + s == s {
                    a[ix(l, l - 1)] = 0.0;
                    break;
                }
                l -= 1;
            }
            let mut x = a[ix(nn, nn)];
            if l == nn {
                wr[nn] = x + t;
                wi[nn] = 0.0;
                nn -= 1;
                break;
            }
            let mut y = a[ix(nn - 1, nn - 1)];
            let mut wv = a[ix(nn, nn - 1)] * a[ix(nn - 1, nn)];
            if l == nn - 1 {
                let p = 0.5 * (y - x);
                let q = p * p + wv;
                let mut z = q.abs().sqrt();
                x += t;
                if q >= 0.0 {
                    z = p + sign(z, p);
                    wr[nn - 1] = x + z;
                    wr[nn] = x + z;
                    if z != 0.0 {
                        wr[nn] = x - wv / z;
                    }
                    wi[nn - 1] = 0.0;
                    wi[nn] = 0.0;
                } else {
                    wr[nn - 1] = x + p;
                    wr[nn] = x + p;
                    wi[nn - 1] = -z;
                    wi[nn] = z;
                }
                nn = nn.saturating_sub(2);
                break;
            }
            if its == 60 {
                return Err(EigenError::NoConvergence);
            }
            if its % 10 == 0 && its > 0 {
                // An exceptional shift.
                t += x;
                for i in 1..=nn {
                    a[ix(i, i)] -= x;
                }
                let s = a[ix(nn, nn - 1)].abs() + a[ix(nn - 1, nn - 2)].abs();
                x = 0.75 * s;
                y = x;
                wv = -0.4375 * s * s;
            }
            its += 1;
            let mut m = nn - 2;
            let (mut p, mut q, mut r);
            loop {
                let z = a[ix(m, m)];
                let r0 = x - z;
                let s0 = y - z;
                p = (r0 * s0 - wv) / a[ix(m + 1, m)] + a[ix(m, m + 1)];
                q = a[ix(m + 1, m + 1)] - z - r0 - s0;
                r = a[ix(m + 2, m + 1)];
                let s = p.abs() + q.abs() + r.abs();
                p /= s;
                q /= s;
                r /= s;
                if m == l {
                    break;
                }
                let u = a[ix(m, m - 1)].abs() * (q.abs() + r.abs());
                let v = p.abs() * (a[ix(m - 1, m - 1)].abs() + z.abs() + a[ix(m + 1, m + 1)].abs());
                if u + v == v {
                    break;
                }
                m -= 1;
            }
            for i in m + 2..=nn {
                a[ix(i, i - 2)] = 0.0;
                if i != m + 2 {
                    a[ix(i, i - 3)] = 0.0;
                }
            }
            let mut k = m;
            while k < nn {
                if k != m {
                    p = a[ix(k, k - 1)];
                    q = a[ix(k + 1, k - 1)];
                    r = 0.0;
                    if k != nn - 1 {
                        r = a[ix(k + 2, k - 1)];
                    }
                    x = p.abs() + q.abs() + r.abs();
                    if x != 0.0 {
                        p /= x;
                        q /= x;
                        r /= x;
                    }
                }
                let s = sign((p * p + q * q + r * r).sqrt(), p);
                if s != 0.0 {
                    if k == m {
                        if l != m {
                            a[ix(k, k - 1)] = -a[ix(k, k - 1)];
                        }
                    } else {
                        a[ix(k, k - 1)] = -s * x;
                    }
                    p += s;
                    x = p / s;
                    y = q / s;
                    let z = r / s;
                    q /= p;
                    r /= p;
                    for j in k..=nn {
                        let mut pp = a[ix(k, j)] + q * a[ix(k + 1, j)];
                        if k != nn - 1 {
                            pp += r * a[ix(k + 2, j)];
                            a[ix(k + 2, j)] -= pp * z;
                        }
                        a[ix(k + 1, j)] -= pp * y;
                        a[ix(k, j)] -= pp * x;
                    }
                    let mmin = if nn < k + 3 { nn } else { k + 3 };
                    for i in l..=mmin {
                        let mut pp = x * a[ix(i, k)] + y * a[ix(i, k + 1)];
                        if k != nn - 1 {
                            pp += z * a[ix(i, k + 2)];
                            a[ix(i, k + 2)] -= pp * r;
                        }
                        a[ix(i, k + 1)] -= pp * q;
                        a[ix(i, k)] -= pp;
                    }
                }
                k += 1;
            }
            if l >= nn - 1 {
                break;
            }
        }
    }
    Ok((1..=n).map(|i| Complex::new(wr[i], wi[i])).collect())
}

/// The eigenvalues of the real square matrix `m` (balanced, reduced to
/// Hessenberg form, then the shifted QR iteration).
pub fn eigenvalues(m: &Mat) -> Result<Vec<Complex>, EigenError> {
    Ok(balanced_eigenvalues(m)?.0)
}

/// [`eigenvalues`] and the Frobenius norm of the balanced matrix, the scale
/// of the eigenvalues' absolute round-off.
fn balanced_eigenvalues(m: &Mat) -> Result<(Vec<Complex>, f64), EigenError> {
    assert_eq!(m.rows, m.cols, "eigenvalues of a square matrix");
    let n = m.rows;
    if m.a.iter().any(|x| !x.is_finite()) {
        return Err(EigenError::NonFinite);
    }
    if n == 0 {
        return Ok((Vec::new(), 0.0));
    }
    let mut h = m.a.clone();
    balance(&mut h, n);
    let norm = h.iter().map(|x| x * x).sum::<f64>().sqrt();
    hessenberg(&mut h, n);
    Ok((hqr(&h, n)?, norm))
}

/// The finite eigenvalues of a pencil ([`finite_eigenvalues`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    /// The finite eigenvalues.
    pub values: Vec<Complex>,
    /// The number of infinite eigenvalues deflated.
    pub infinite: usize,
    /// The Frobenius norm of the balanced matrix whose eigenvalues they are:
    /// their absolute round-off is a small multiple of `1e-16` times it.
    pub norm: f64,
}

/// Rank tolerance of the capacitance-like matrix of a pencil, relative to its
/// largest pivot and to its largest entry after the row scaling (each row
/// that has one scaled to a unit largest entry): a reduced pencil's `C` is
/// judged against the original scale.
pub const DYNAMIC_RANK_TOLERANCE: f64 = 1e-11;
/// Rank tolerance of the algebraic rows of a pencil.
pub const ALGEBRAIC_RANK_TOLERANCE: f64 = 1e-13;

/// The power of two nearest `x` in log scale.
fn power_of_two(x: f64) -> f64 {
    2f64.powi(x.log2().round().clamp(-1000.0, 1000.0) as i32)
}

/// Scale every row to a unit largest entry by a power of two.
fn normalize_rows(g: &mut Mat) {
    for i in 0..g.rows {
        let big = (0..g.cols).fold(0.0f64, |m, j| m.max(g.get(i, j).abs()));
        if big > 0.0 {
            let f = power_of_two(1.0 / big);
            for j in 0..g.cols {
                let x = g.get(i, j) * f;
                g.set(i, j, x);
            }
        }
    }
}

/// The finite eigenvalues `s` of the regular pencil `det(G + s C) = 0`
/// (`G`, `C` square of one size), and the number of infinite ones.
///
/// Rows are first scaled by powers of two (each row that `C` touches to a
/// unit largest `C` entry, the others to a unit largest `G` entry), which
/// leaves the eigenvalues alone; columns are not scaled, so the null-space
/// bases below do not shrink the dynamic unknowns. Then, while `C` is
/// singular: an orthogonal row transformation from the pivoted QR of `C`'s
/// nonzero rows splits the pencil into `r` dynamic rows `G1 + s C1` and
/// algebraic rows `G2` (no `s`; with the rows `C` does not touch, which are
/// never mixed); every solution has `G2 x = 0`, so with `Z` an orthonormal
/// basis of the null space of `G2` the pencil becomes the `r x r` pencil
/// `G1 Z + s C1 Z`, with the same finite eigenvalues. `G2` without full row
/// rank means `det(G + s C)` vanishes for every `s` (a singular pencil). When
/// `C` is nonsingular the eigenvalues are those of `-C^-1 G`.
pub fn finite_eigenvalues(g: &Mat, c: &Mat) -> Result<Spectrum, EigenError> {
    let n = g.rows;
    assert!(g.cols == n && c.rows == n && c.cols == n, "pencil sizes");
    if g.a.iter().chain(&c.a).any(|x| !x.is_finite()) {
        return Err(EigenError::NonFinite);
    }
    let (mut g, mut c) = (g.clone(), c.clone());
    // Row scaling.
    for i in 0..n {
        let big_c = (0..n).fold(0.0f64, |m, j| m.max(c.get(i, j).abs()));
        let big = if big_c > 0.0 { big_c } else { (0..n).fold(0.0f64, |m, j| m.max(g.get(i, j).abs())) };
        if big > 0.0 {
            let f = power_of_two(1.0 / big);
            for j in 0..n {
                g.set(i, j, g.get(i, j) * f);
                c.set(i, j, c.get(i, j) * f);
            }
        }
    }
    let floor = DYNAMIC_RANK_TOLERANCE * c.max_abs();
    reduce(&g, &c, floor)
}

/// The relative size, against the largest, from which an eigenvalue `mu` of
/// `-G^-1 C` gives a slow root `s = 1 / mu` of a regular pencil more
/// accurately than the eigenvalues of `-C^-1 G` do ([`with_slow_roots_inverted`]).
pub const INVERTED_SLOW: f64 = 1e-4;

/// `direct`, the eigenvalues of `-C^-1 G` of a pencil with nonsingular `C`
/// (the infinite eigenvalues already deflated), with its slow roots taken from
/// the eigenvalues `mu` of `-G^-1 C` instead (`s = 1 / mu`) when `G` is
/// nonsingular too. The QR iteration's absolute round-off is a multiple of
/// `1e-16` times the norm of its balanced matrix, which in a stiff pencil
/// (conductances from 1e-12 to 1e6 S beside microfarad couplings, as in vendor
/// op-amp models with 1 uohm and 1 Tohm resistors) is set by the fastest
/// roots, 1e15 rad/s and more: a root of a few rad/s can then come out on the
/// wrong side of the imaginary axis, and a repeated one (two equal coupling
/// networks) far off. Inverted, the slowest roots are the largest `mu`, which
/// the QR iteration gives to round-off relative to themselves. The `k` roots
/// with `|mu|` at least [`INVERTED_SLOW`] of the largest replace the `k`
/// smallest direct roots. With roots at the origin (`G` singular) the
/// pencil is inverted at a shift `sigma`, a hundredth of the slowest other
/// root, and `s = sigma + 1 / mu`.
pub(crate) fn with_slow_roots_inverted(g: &Mat, c: &Mat, direct: Vec<Complex>) -> Vec<Complex> {
    let n = g.rows;
    // With roots at the origin (a node with only capacitors to it) G is
    // singular: invert at a shift a hundredth of the slowest other root.
    let fastest = direct.iter().fold(0.0f64, |a, z| a.max(z.abs()));
    let near_origin = |z: &Complex| z.abs() <= 1e-9 * fastest;
    let sigma = if direct.iter().any(near_origin) {
        match direct.iter().filter(|z| !near_origin(z)).map(|z| z.abs()).min_by(f64::total_cmp) {
            Some(slowest) => 1e-2 * slowest,
            None => return direct,
        }
    } else {
        0.0
    };
    let shifted: Vec<f64> = g.a.iter().zip(&c.a).map(|(g, c)| g + sigma * c).collect();
    let Ok(lu) = RealLu::factor(shifted, n) else { return direct };
    let mut m = Mat::zeros(n, n);
    for j in 0..n {
        let column: Vec<f64> = (0..n).map(|i| -c.get(i, j)).collect();
        let Some(x) = lu.solve(&column) else { return direct };
        for (i, v) in x.iter().enumerate() {
            m.set(i, j, *v);
        }
    }
    let Ok((mu, _)) = balanced_eigenvalues(&m) else { return direct };
    let largest = mu.iter().fold(0.0f64, |a, z| a.max(z.abs()));
    if largest == 0.0 || !largest.is_finite() || mu.len() != direct.len() {
        return direct;
    }
    let mut slow: Vec<Complex> = mu
        .iter()
        .filter(|z| z.abs() >= INVERTED_SLOW * largest)
        .map(|z| Complex::new(sigma, 0.0) + Complex::new(1.0, 0.0) / *z)
        .collect();
    for s in &mut slow {
        if s.im.abs() <= 1e-12 * s.abs() {
            s.im = 0.0;
        }
    }
    let mut fast = direct;
    fast.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
    let k = slow.len();
    slow.extend(fast.into_iter().skip(k));
    slow
}

/// One deflation step of [`finite_eigenvalues`] on a scaled pencil, then the
/// reduced pencil (orthogonal transformations keep its scaling).
fn reduce(g: &Mat, c: &Mat, floor: f64) -> Result<Spectrum, EigenError> {
    let n = g.rows;
    if n == 0 {
        return Ok(Spectrum { values: Vec::new(), infinite: 0, norm: 0.0 });
    }
    // Rows that C touches; the others are algebraic as they are and never mixed.
    let dynamic: Vec<usize> = (0..n).filter(|&i| (0..n).any(|j| c.get(i, j) != 0.0)).collect();
    let algebraic: Vec<usize> = (0..n).filter(|i| !dynamic.contains(i)).collect();
    let pick = |m: &Mat, rows: &[usize]| {
        let mut out = Mat::zeros(rows.len(), m.cols);
        for (k, &i) in rows.iter().enumerate() {
            out.a[k * m.cols..(k + 1) * m.cols].copy_from_slice(&m.a[i * m.cols..(i + 1) * m.cols]);
        }
        out
    };
    let (gd, cd, ga) = (pick(g, &dynamic), pick(c, &dynamic), pick(g, &algebraic));
    let qr = qr_pivoted(&cd, DYNAMIC_RANK_TOLERANCE, floor);
    let r = qr.rank;
    if r == n {
        // C nonsingular: the eigenvalues of -C^-1 G.
        let lu = RealLu::factor(c.a.clone(), n).map_err(|_| EigenError::SingularPencil)?;
        let mut m = Mat::zeros(n, n);
        for j in 0..n {
            let column: Vec<f64> = (0..n).map(|i| -g.get(i, j)).collect();
            let x = lu.solve(&column).ok_or(EigenError::NonFinite)?;
            for (i, v) in x.iter().enumerate() {
                m.set(i, j, *v);
            }
        }
        let (values, norm) = balanced_eigenvalues(&m)?;
        return Ok(Spectrum { values: with_slow_roots_inverted(g, c, values), infinite: 0, norm });
    }
    let qt = qr.q.transpose();
    let (qg, qc) = (qt.mul(&gd), qt.mul(&cd));
    let (g1, c1) = (qg.row_block(0, r), qc.row_block(0, r));
    let mut g2 = qg.row_block(r, dynamic.len()).stack(&ga);
    normalize_rows(&mut g2);
    let m2 = g2.rows;
    let null = qr_pivoted(&g2.transpose(), ALGEBRAIC_RANK_TOLERANCE, 0.0);
    if null.rank < m2 {
        return Err(EigenError::SingularPencil);
    }
    let z = null.q.col_block(m2, n);
    let reduced = reduce(&g1.mul(&z), &c1.mul(&z), floor)?;
    Ok(Spectrum { infinite: reduced.infinite + (n - r), ..reduced })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat(rows: usize, cols: usize, a: &[f64]) -> Mat {
        Mat { rows, cols, a: a.to_vec() }
    }

    fn sorted(mut v: Vec<Complex>) -> Vec<Complex> {
        v.sort_by(|a, b| a.re.total_cmp(&b.re).then(a.im.total_cmp(&b.im)));
        v
    }

    #[test]
    fn eigenvalues_of_known_matrices() {
        // Upper triangular: the diagonal.
        let e = sorted(eigenvalues(&mat(3, 3, &[1.0, 2.0, 3.0, 0.0, -4.0, 5.0, 0.0, 0.0, 7.0])).unwrap());
        assert_eq!(e.len(), 3);
        for (got, want) in e.iter().zip([-4.0, 1.0, 7.0]) {
            assert!((got.re - want).abs() < 1e-12 && got.im == 0.0, "{got:?}");
        }
        // A rotation-like block: +-2j around -1.
        let e = sorted(eigenvalues(&mat(2, 2, &[-1.0, 2.0, -2.0, -1.0])).unwrap());
        assert!((e[0].re + 1.0).abs() < 1e-14 && (e[0].im + 2.0).abs() < 1e-14);
        assert!((e[1].re + 1.0).abs() < 1e-14 && (e[1].im - 2.0).abs() < 1e-14);
        // A companion matrix of (x-1)(x-2)(x-3)(x-4)(x-5): x^5 - 15x^4 + 85x^3 - 225x^2 + 274x - 120.
        let coeffs = [-15.0, 85.0, -225.0, 274.0, -120.0];
        let mut c = Mat::zeros(5, 5);
        for (j, k) in coeffs.iter().enumerate() {
            c.a[j] = -k;
        }
        for i in 1..5 {
            c.a[i * 5 + i - 1] = 1.0;
        }
        let e = sorted(eigenvalues(&c).unwrap());
        for (got, want) in e.iter().zip(1..=5) {
            assert!((got.re - want as f64).abs() < 1e-9 && got.im.abs() < 1e-9, "{got:?}");
        }
    }

    #[test]
    fn random_matrix_eigenvalues_satisfy_trace_and_determinant() {
        let mut rng = crate::rng::Rng::new(7);
        for n in [4usize, 9, 17, 40] {
            let a: Vec<f64> = (0..n * n).map(|_| rng.uniform() * 2.0 - 1.0).collect();
            let m = Mat { rows: n, cols: n, a: a.clone() };
            let e = eigenvalues(&m).unwrap();
            let trace: f64 = (0..n).map(|i| a[i * n + i]).sum();
            let sum = e.iter().fold(Complex::ZERO, |s, z| s + *z);
            assert!((sum.re - trace).abs() < 1e-10 * n as f64 && sum.im.abs() < 1e-10, "n {n}");
            // Each eigenvalue makes A - lambda I singular: a pivoted QR finds rank n - 1 (real ones).
            for z in e.iter().filter(|z| z.im == 0.0) {
                let mut shifted = m.clone();
                for i in 0..n {
                    shifted.a[i * n + i] -= z.re;
                }
                let qr = qr_pivoted(&shifted, 1e-9, 0.0);
                assert!(qr.rank < n, "n {n}: {z:?} is no eigenvalue");
            }
        }
    }

    #[test]
    fn qr_reveals_rank() {
        // Rank 2: the third column is the sum of the first two.
        let a = mat(3, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 9.0, 7.0, 8.0, 15.0]);
        let qr = qr_pivoted(&a, 1e-12, 0.0);
        assert_eq!(qr.rank, 2);
        // Q is orthogonal and Q R = A P.
        let qtq = qr.q.transpose().mul(&qr.q);
        for i in 0..3 {
            for j in 0..3 {
                assert!((qtq.get(i, j) - if i == j { 1.0 } else { 0.0 }).abs() < 1e-14);
            }
        }
        let qrm = qr.q.mul(&qr.r);
        for i in 0..3 {
            for (k, &j) in qr.perm.iter().enumerate() {
                assert!((qrm.get(i, k) - a.get(i, j)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn pencil_with_algebraic_unknowns() {
        // RC: V source 1 -> node a, R a-b, C b-ground. Unknowns va, vb, iV.
        // KCL a: (va - vb)/R + iV = 0; KCL b: (vb - va)/R + s C vb = 0; branch: va = E.
        let (r, cap) = (1e3, 1e-6);
        let g = mat(3, 3, &[1.0 / r, -1.0 / r, 1.0, -1.0 / r, 1.0 / r, 0.0, 1.0, 0.0, 0.0]);
        let c = mat(3, 3, &[0.0, 0.0, 0.0, 0.0, cap, 0.0, 0.0, 0.0, 0.0]);
        let Spectrum { values: e, infinite, .. } = finite_eigenvalues(&g, &c).unwrap();
        assert_eq!((e.len(), infinite), (1, 2));
        assert!((e[0].re + 1.0 / (r * cap)).abs() < 1e-9 * 1e3 && e[0].im == 0.0, "{e:?}");
        // A floating capacitor between a and b (both held by resistors): one pole (R1 + R2) ... C.
        let (r1, r2) = (1e3, 4e3);
        let g = mat(2, 2, &[1.0 / r1, 0.0, 0.0, 1.0 / r2]);
        let c = mat(2, 2, &[cap, -cap, -cap, cap]);
        let Spectrum { values: e, infinite, .. } = finite_eigenvalues(&g, &c).unwrap();
        assert_eq!((e.len(), infinite), (1, 1));
        let want = -1.0 / ((r1 + r2) * cap);
        assert!((e[0].re / want - 1.0).abs() < 1e-12, "{e:?}");
        // Singular pencil: a node with nothing at all.
        let g = mat(2, 2, &[1.0, 0.0, 0.0, 0.0]);
        let c = Mat::zeros(2, 2);
        assert_eq!(finite_eigenvalues(&g, &c).unwrap_err(), EigenError::SingularPencil);
    }
}

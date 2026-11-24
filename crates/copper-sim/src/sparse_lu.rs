//! Sparse LU factorisation for the circuit solves, real and complex.
//!
//! A circuit matrix has a few entries per row, so a dense factorisation
//! (cubic in the number of unknowns) wastes almost all of its work. This one
//! keeps the method of [`crate::lu`] where it matters for the results (rows,
//! then columns, then rows equilibrated by powers of two, which adds no
//! rounding; one step of iterative refinement against the original matrix)
//! and changes the elimination order:
//!
//! - **First factorisation** ([`SparseLu::factor`]): Markowitz ordering with
//!   threshold partial pivoting. Each step takes, among the entries at least
//!   [`PIVOT_THRESHOLD`] times the largest of their column in the remaining
//!   matrix, the one with the fewest fill-ins `(r - 1)(c - 1)` (`r`, `c` the
//!   entries left in its row and column), ties to the larger relative size,
//!   then to the lower column and row. The pivot order and the pattern of the
//!   factors, fill-ins included, are kept.
//! - **Refactorisation** ([`SparseLu::refactor`]): new values on the same
//!   pattern (the next Newton iteration, the next time step) reuse the pivot
//!   order and the pattern: a left-looking numeric pass with no search, about
//!   as many operations as the factors have entries. Each pivot is checked
//!   against the rest of its column; one smaller than [`REFACTOR_THRESHOLD`]
//!   times it (the values have moved too far) makes the caller factor afresh.
//!
//! Either step refuses (`Err` or `false`) a pivot at or below
//! [`PIVOT_FLOOR`], an empty row or column and a non-finite entry. The
//! callers then factor the same matrix with the dense [`crate::lu`] or
//! [`crate::real_lu`], so a singular or nearly singular circuit gets the
//! dense factorisation's verdict and its diagnostics, unchanged.
//!
//! [`SparseMatrix`] is the compressed-row matrix the assembly stamps into:
//! its pattern only grows (an entry stamped once stays, possibly zero), so
//! the factors' pattern stays valid across the solves of an analysis.

use crate::complex::Complex;
use crate::lu::LuError;
use crate::timing::{self, Counter};
use std::fmt::Debug;
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

/// A pivot must be at least this fraction of the largest entry of its column
/// in the remaining matrix when the order is chosen.
pub const PIVOT_THRESHOLD: f64 = 0.1;
/// A refactorisation keeps the pivot order while each pivot is at least this
/// fraction of the largest entry of its column.
pub const REFACTOR_THRESHOLD: f64 = 1e-3;
/// Pivots at or below this magnitude (equilibrated matrix) are left to the
/// dense factorisation, whose tolerance it is: the dense factorisation
/// decides whether the matrix is singular.
pub const PIVOT_FLOOR: f64 = crate::lu::PIVOT_TOLERANCE;
/// Factors with a pivot below this magnitude (equilibrated matrix: the last
/// pivots of a nearly singular one, nearly floating nodes held by 1 Tohm)
/// are weak: their solves are refined until the correction stops shrinking.
pub const WEAK_PIVOT: f64 = 1e-10;
/// Refinement steps of a solve with weak factors at most.
const WEAK_REFINEMENTS: usize = 4;

/// The arithmetic the factorisation needs: `f64` and [`Complex`].
pub trait Scalar:
    Copy
    + Debug
    + PartialEq
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + AddAssign
    + SubAssign
{
    /// Zero.
    const ZERO: Self;
    /// The magnitude.
    fn magnitude(self) -> f64;
    /// Multiplied by a real factor.
    fn scaled(self, s: f64) -> Self;
    /// Both parts finite.
    fn finite(self) -> bool;
}

impl Scalar for f64 {
    const ZERO: f64 = 0.0;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn scaled(self, s: f64) -> f64 {
        self * s
    }
    fn finite(self) -> bool {
        self.is_finite()
    }
}

impl Scalar for Complex {
    const ZERO: Complex = Complex::ZERO;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn scaled(self, s: f64) -> Complex {
        self.scale(s)
    }
    fn finite(self) -> bool {
        self.is_finite()
    }
}

/// A square matrix in compressed rows (columns sorted within each row) whose
/// pattern grows as entries are stamped.
#[derive(Debug, Clone, PartialEq)]
pub struct SparseMatrix<T> {
    n: usize,
    row_start: Vec<usize>,
    cols: Vec<usize>,
    values: Vec<T>,
    /// Entries stamped outside the pattern since [`SparseMatrix::clear`], in stamping order.
    extra: Vec<(usize, usize, T)>,
}

impl<T: Scalar> SparseMatrix<T> {
    /// An `n x n` matrix with no entries.
    pub fn new(n: usize) -> Self {
        SparseMatrix { n, row_start: vec![0; n + 1], cols: Vec::new(), values: Vec::new(), extra: Vec::new() }
    }

    /// The non-zero entries of the row-major `n x n` matrix `a`.
    pub fn from_dense(a: &[T], n: usize) -> Self {
        assert_eq!(a.len(), n * n, "matrix size");
        let mut m = SparseMatrix::new(n);
        for r in 0..n {
            for c in 0..n {
                let v = a[r * n + c];
                if v != T::ZERO {
                    m.cols.push(c);
                    m.values.push(v);
                }
            }
            m.row_start[r + 1] = m.cols.len();
        }
        m
    }

    /// The transpose (same entries, rows and columns exchanged).
    pub fn transposed(&self) -> Self {
        let mut t = SparseMatrix::new(self.n);
        let mut count = vec![0usize; self.n + 1];
        for &c in &self.cols {
            count[c + 1] += 1;
        }
        for c in 0..self.n {
            count[c + 1] += count[c];
        }
        t.row_start.clone_from(&count);
        t.cols = vec![0; self.cols.len()];
        t.values = vec![T::ZERO; self.cols.len()];
        for r in 0..self.n {
            for e in self.row_start[r]..self.row_start[r + 1] {
                let c = self.cols[e];
                let slot = count[c];
                count[c] += 1;
                t.cols[slot] = r;
                t.values[slot] = self.values[e];
            }
        }
        t
    }

    /// Number of rows (and columns).
    pub fn size(&self) -> usize {
        self.n
    }

    /// Number of entries in the pattern.
    pub fn entries(&self) -> usize {
        self.cols.len()
    }

    /// Every value to zero; the pattern stays.
    pub fn clear(&mut self) {
        self.values.iter_mut().for_each(|v| *v = T::ZERO);
        self.extra.clear();
    }

    /// The entries of `base` and nothing else (`base` the same size; the
    /// pattern grows to hold them).
    pub fn load(&mut self, base: &SparseMatrix<T>) {
        assert_eq!(base.n, self.n, "matrix size");
        self.extra.clear();
        if self.same_pattern(&base.row_start, &base.cols) {
            self.values.copy_from_slice(&base.values);
            return;
        }
        self.clear();
        for r in 0..base.n {
            for e in base.row_start[r]..base.row_start[r + 1] {
                self.add(r, base.cols[e], base.values[e]);
            }
        }
        for &(r, c, v) in &base.extra {
            self.add(r, c, v);
        }
    }

    /// Add `v` to entry `(r, c)`.
    pub fn add(&mut self, r: usize, c: usize, v: T) {
        let row = &self.cols[self.row_start[r]..self.row_start[r + 1]];
        match row.binary_search(&c) {
            Ok(k) => self.values[self.row_start[r] + k] += v,
            Err(_) => self.extra.push((r, c, v)),
        }
    }

    /// Merge the entries stamped outside the pattern into it; true when the pattern grew.
    pub fn finish(&mut self) -> bool {
        if self.extra.is_empty() {
            return false;
        }
        let extra = std::mem::take(&mut self.extra);
        let mut rows: Vec<Vec<(usize, T)>> = (0..self.n)
            .map(|r| (self.row_start[r]..self.row_start[r + 1]).map(|e| (self.cols[e], self.values[e])).collect())
            .collect();
        for (r, c, v) in extra {
            match rows[r].binary_search_by_key(&c, |&(col, _)| col) {
                Ok(k) => rows[r][k].1 += v,
                Err(k) => rows[r].insert(k, (c, T::ZERO + v)),
            }
        }
        self.cols.clear();
        self.values.clear();
        for (r, row) in rows.into_iter().enumerate() {
            for (c, v) in row {
                self.cols.push(c);
                self.values.push(v);
            }
            self.row_start[r + 1] = self.cols.len();
        }
        true
    }

    /// The matrix, row-major.
    pub fn to_dense(&self) -> Vec<T> {
        let n = self.n;
        let mut a = vec![T::ZERO; n * n];
        for r in 0..n {
            for e in self.row_start[r]..self.row_start[r + 1] {
                a[r * n + self.cols[e]] += self.values[e];
            }
        }
        for &(r, c, v) in &self.extra {
            a[r * n + c] += v;
        }
        a
    }

    /// The entries of row `r`: `(column, value)`.
    pub fn row(&self, r: usize) -> impl Iterator<Item = (usize, T)> + '_ {
        (self.row_start[r]..self.row_start[r + 1]).map(move |e| (self.cols[e], self.values[e]))
    }

    fn same_pattern(&self, row_start: &[usize], cols: &[usize]) -> bool {
        self.row_start == row_start && self.cols == cols
    }
}

/// The factors of `a`: `previous` refactored when its pattern fits and its
/// order still serves, else a fresh factorisation; `None` when the matrix is
/// left to the dense factorisation (see the module docs).
pub fn factor_reusing<T: Scalar>(a: &SparseMatrix<T>, previous: Option<SparseLu<T>>) -> Option<SparseLu<T>> {
    if let Some(mut lu) = previous
        && lu.fits(a)
        && (lu.factors(a) || lu.refactor(a))
    {
        timing::count(Counter::Refactored);
        return Some(lu);
    }
    let lu = SparseLu::factor(a).ok();
    timing::count(if lu.is_some() { Counter::Ordered } else { Counter::Dense });
    lu
}

/// The power of two nearest to `x` in log scale, within `2^-1000 ..= 2^1000`
/// (as [`crate::lu`]'s, from the bits: `2^e` or `2^(e+1)` for `x = m 2^e`,
/// the upper one when `m >= sqrt 2`).
fn power_of_two_near(x: f64) -> f64 {
    if !(x.is_normal() && x > 0.0) {
        let e = if x.is_finite() { x.log2().round().clamp(-1000.0, 1000.0) } else { 1000.0 };
        return 2f64.powi(e as i32);
    }
    const MANTISSA: u64 = (1 << 52) - 1;
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let up = (bits & MANTISSA) >= (std::f64::consts::SQRT_2.to_bits() & MANTISSA);
    let e = (exponent + i32::from(up)).clamp(-1000, 1000);
    f64::from_bits(((e + 1023) as u64) << 52)
}

/// The equilibration of [`crate::lu`]: rows, columns, rows, by powers of two.
fn equilibrate<T: Scalar>(a: &SparseMatrix<T>) -> Result<(Vec<f64>, Vec<f64>), LuError> {
    let n = a.n;
    if a.values.iter().any(|v| !v.finite()) {
        return Err(LuError::NonFinite);
    }
    let mut row_scale = vec![1.0; n];
    let mut col_scale = vec![1.0; n];
    let row_pass = |row_scale: &mut [f64], col_scale: &[f64]| -> Result<(), LuError> {
        for (r, rs) in row_scale.iter_mut().enumerate() {
            let big = (a.row_start[r]..a.row_start[r + 1])
                .map(|e| a.values[e].magnitude() * *rs * col_scale[a.cols[e]])
                .fold(0.0, f64::max);
            if big == 0.0 {
                return Err(LuError::EmptyRow(r));
            }
            *rs *= power_of_two_near(1.0 / big);
        }
        Ok(())
    };
    row_pass(&mut row_scale, &col_scale)?;
    let mut big = vec![0.0f64; n];
    for (r, rs) in row_scale.iter().enumerate() {
        for e in a.row_start[r]..a.row_start[r + 1] {
            let c = a.cols[e];
            big[c] = big[c].max(a.values[e].magnitude() * rs);
        }
    }
    for (c, cs) in col_scale.iter_mut().enumerate() {
        if big[c] == 0.0 {
            return Err(LuError::Singular(c));
        }
        *cs = power_of_two_near(1.0 / big[c]);
    }
    row_pass(&mut row_scale, &col_scale)?;
    Ok((row_scale, col_scale))
}

/// The LU factors of a sparse square matrix, with the pivot order and
/// pattern a refactorisation reuses.
#[derive(Debug, Clone)]
pub struct SparseLu<T> {
    n: usize,
    /// The pattern and values of the factored matrix.
    a_row_start: Vec<usize>,
    a_cols: Vec<usize>,
    a_values: Vec<T>,
    row_scale: Vec<f64>,
    col_scale: Vec<f64>,
    /// Pivot position to original row, and to original column.
    row_perm: Vec<usize>,
    col_perm: Vec<usize>,
    /// Per pivot column `j`: the matrix entries in its original column, `(entry, pivot row)`.
    scatter_start: Vec<usize>,
    scatter: Vec<(usize, usize)>,
    /// `L` by columns, below the (unit) diagonal: pivot rows and values.
    l_start: Vec<usize>,
    l_rows: Vec<usize>,
    l_values: Vec<T>,
    /// `U` by columns, above the diagonal, pivot rows increasing.
    u_start: Vec<usize>,
    u_rows: Vec<usize>,
    u_values: Vec<T>,
    diag: Vec<T>,
    /// Whether a pivot is below [`WEAK_PIVOT`].
    weak: bool,
}

impl<T: Scalar> SparseLu<T> {
    /// Factor `a` (the entries stamped outside its pattern must have been
    /// merged with [`SparseMatrix::finish`]), choosing the pivot order.
    pub fn factor(a: &SparseMatrix<T>) -> Result<SparseLu<T>, LuError> {
        assert!(a.extra.is_empty(), "finish the matrix before factoring it");
        let n = a.n;
        let (row_scale, col_scale) = equilibrate(a)?;
        // Working copy, dense for constant-time access, with the structure
        // of the remaining matrix in per-row and per-column lists.
        let mut w = vec![T::ZERO; n * n];
        let mut present = vec![false; n * n];
        let mut row_list: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut col_list: Vec<Vec<usize>> = vec![Vec::new(); n];
        for r in 0..n {
            for e in a.row_start[r]..a.row_start[r + 1] {
                let c = a.cols[e];
                w[r * n + c] = a.values[e].scaled(row_scale[r] * col_scale[c]);
                present[r * n + c] = true;
                row_list[r].push(c);
                col_list[c].push(r);
            }
        }
        let mut row_count: Vec<usize> = row_list.iter().map(Vec::len).collect();
        let mut col_count: Vec<usize> = col_list.iter().map(Vec::len).collect();
        let mut row_pos = vec![usize::MAX; n];
        let mut col_pos = vec![usize::MAX; n];
        let mut row_perm = Vec::with_capacity(n);
        let mut col_perm = Vec::with_capacity(n);
        for step in 0..n {
            // (cost, ratio, row, column) of the best candidate so far.
            let mut best: Option<(usize, f64, usize, usize)> = None;
            'columns: for c in 0..n {
                if col_pos[c] != usize::MAX {
                    continue;
                }
                let column_max = col_list[c]
                    .iter()
                    .filter(|&&r| row_pos[r] == usize::MAX)
                    .map(|&r| w[r * n + c].magnitude())
                    .fold(0.0, f64::max);
                if column_max <= PIVOT_FLOOR {
                    return Err(LuError::Singular(step));
                }
                for &r in &col_list[c] {
                    if row_pos[r] != usize::MAX {
                        continue;
                    }
                    let size = w[r * n + c].magnitude();
                    if size < PIVOT_THRESHOLD * column_max || size <= PIVOT_FLOOR {
                        continue;
                    }
                    let cost = (row_count[r] - 1) * (col_count[c] - 1);
                    let ratio = size / column_max;
                    let better = match best {
                        None => true,
                        Some((bc, br, _, _)) => cost < bc || (cost == bc && ratio > br),
                    };
                    if better {
                        best = Some((cost, ratio, r, c));
                        if cost == 0 && ratio == 1.0 {
                            break 'columns;
                        }
                    }
                }
            }
            let Some((_, _, p, q)) = best else { return Err(LuError::Singular(step)) };
            row_pos[p] = step;
            col_pos[q] = step;
            row_perm.push(p);
            col_perm.push(q);
            let pivot = w[p * n + q];
            for &c in &row_list[p] {
                if col_pos[c] == usize::MAX {
                    col_count[c] -= 1;
                }
            }
            let rows: Vec<usize> = col_list[q].iter().copied().filter(|&r| row_pos[r] == usize::MAX).collect();
            let pivot_row: Vec<usize> = row_list[p].iter().copied().filter(|&c| col_pos[c] == usize::MAX).collect();
            for r in rows {
                row_count[r] -= 1;
                let l = w[r * n + q] / pivot;
                w[r * n + q] = l;
                for &c in &pivot_row {
                    let k = r * n + c;
                    if !present[k] {
                        present[k] = true;
                        row_list[r].push(c);
                        col_list[c].push(r);
                        row_count[r] += 1;
                        col_count[c] += 1;
                    }
                    let u = w[p * n + c];
                    w[k] -= l * u;
                }
            }
        }
        // The factors' pattern and values, by pivot positions.
        let mut l_cols: Vec<Vec<(usize, T)>> = vec![Vec::new(); n];
        let mut u_cols: Vec<Vec<(usize, T)>> = vec![Vec::new(); n];
        let mut diag = vec![T::ZERO; n];
        for (r, cols) in row_list.iter().enumerate() {
            let i = row_pos[r];
            for &c in cols {
                let j = col_pos[c];
                let v = w[r * n + c];
                match i.cmp(&j) {
                    std::cmp::Ordering::Greater => l_cols[j].push((i, v)),
                    std::cmp::Ordering::Less => u_cols[j].push((i, v)),
                    std::cmp::Ordering::Equal => diag[j] = v,
                }
            }
        }
        let flatten = |cols: Vec<Vec<(usize, T)>>| {
            let mut start = Vec::with_capacity(n + 1);
            let mut rows = Vec::new();
            let mut values = Vec::new();
            start.push(0);
            for mut col in cols {
                col.sort_by_key(|&(i, _)| i);
                for (i, v) in col {
                    rows.push(i);
                    values.push(v);
                }
                start.push(rows.len());
            }
            (start, rows, values)
        };
        let (l_start, l_rows, l_values) = flatten(l_cols);
        let (u_start, u_rows, u_values) = flatten(u_cols);
        let mut buckets: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
        for (r, &i) in row_pos.iter().enumerate() {
            for e in a.row_start[r]..a.row_start[r + 1] {
                buckets[col_pos[a.cols[e]]].push((e, i));
            }
        }
        let mut scatter_start = Vec::with_capacity(n + 1);
        let mut scatter = Vec::with_capacity(a.cols.len());
        scatter_start.push(0);
        for bucket in buckets {
            scatter.extend(bucket);
            scatter_start.push(scatter.len());
        }
        Ok(SparseLu {
            n,
            a_row_start: a.row_start.clone(),
            a_cols: a.cols.clone(),
            a_values: a.values.clone(),
            row_scale,
            col_scale,
            row_perm,
            col_perm,
            scatter_start,
            scatter,
            l_start,
            l_rows,
            l_values,
            u_start,
            u_rows,
            u_values,
            weak: diag.iter().any(|d| d.magnitude() < WEAK_PIVOT),
            diag,
        })
    }

    /// Whether these are the factors of `a` (same pattern, same values).
    pub fn factors(&self, a: &SparseMatrix<T>) -> bool {
        a.extra.is_empty() && a.same_pattern(&self.a_row_start, &self.a_cols) && a.values == self.a_values
    }

    /// Whether `a` has the pattern these factors were built for.
    pub fn fits(&self, a: &SparseMatrix<T>) -> bool {
        a.extra.is_empty() && a.same_pattern(&self.a_row_start, &self.a_cols)
    }

    /// Factor the new values of `a` (same pattern, [`SparseLu::fits`]) in
    /// the same pivot order. False when a pivot is too small for that order
    /// (or `a` cannot be factored at all); the factors are then unusable and
    /// the caller factors afresh.
    pub fn refactor(&mut self, a: &SparseMatrix<T>) -> bool {
        debug_assert!(self.fits(a));
        let Ok((row_scale, col_scale)) = equilibrate(a) else { return false };
        let n = self.n;
        let mut x = vec![T::ZERO; n];
        for j in 0..n {
            let cs = col_scale[self.col_perm[j]];
            for &(e, i) in &self.scatter[self.scatter_start[j]..self.scatter_start[j + 1]] {
                x[i] += a.values[e].scaled(row_scale[self.row_perm[i]] * cs);
            }
            for idx in self.u_start[j]..self.u_start[j + 1] {
                let k = self.u_rows[idx];
                let u = x[k];
                x[k] = T::ZERO;
                self.u_values[idx] = u;
                if u == T::ZERO {
                    continue;
                }
                for lidx in self.l_start[k]..self.l_start[k + 1] {
                    let i = self.l_rows[lidx];
                    x[i] -= self.l_values[lidx] * u;
                }
            }
            let d = x[j];
            x[j] = T::ZERO;
            let below = self.l_start[j]..self.l_start[j + 1];
            let column_max = below.clone().map(|lidx| x[self.l_rows[lidx]].magnitude()).fold(0.0, f64::max);
            let size = d.magnitude();
            if !(size > PIVOT_FLOOR && size >= REFACTOR_THRESHOLD * column_max) {
                return false;
            }
            for lidx in below {
                let i = self.l_rows[lidx];
                self.l_values[lidx] = x[i] / d;
                x[i] = T::ZERO;
            }
            self.diag[j] = d;
        }
        self.a_values.clone_from(&a.values);
        self.weak = self.diag.iter().any(|d| d.magnitude() < WEAK_PIVOT);
        self.row_scale = row_scale;
        self.col_scale = col_scale;
        true
    }

    fn substitute(&self, b: &[T]) -> Vec<T> {
        let n = self.n;
        let mut y: Vec<T> = self.row_perm.iter().map(|&r| b[r].scaled(self.row_scale[r])).collect();
        for j in 0..n {
            let yj = y[j];
            if yj == T::ZERO {
                continue;
            }
            for idx in self.l_start[j]..self.l_start[j + 1] {
                y[self.l_rows[idx]] -= self.l_values[idx] * yj;
            }
        }
        for j in (0..n).rev() {
            let yj = y[j] / self.diag[j];
            y[j] = yj;
            if yj == T::ZERO {
                continue;
            }
            for idx in self.u_start[j]..self.u_start[j + 1] {
                y[self.u_rows[idx]] -= self.u_values[idx] * yj;
            }
        }
        let mut x = vec![T::ZERO; n];
        for (j, &c) in self.col_perm.iter().enumerate() {
            x[c] = y[j].scaled(self.col_scale[c]);
        }
        x
    }

    /// Solve `A x = b` with one step of iterative refinement; `None` if not finite.
    pub fn solve(&self, b: &[T]) -> Option<Vec<T>> {
        let n = self.n;
        assert_eq!(b.len(), n, "right-hand side size");
        let mut x = self.substitute(b);
        if x.iter().any(|z| !z.finite()) {
            return None;
        }
        // One step of refinement; weak factors take more, while the
        // correction (in the columns' own scale) keeps shrinking.
        let mut last = f64::INFINITY;
        for _ in 0..if self.weak { WEAK_REFINEMENTS } else { 1 } {
            let residual: Vec<T> = (0..n)
                .map(|r| {
                    let mut s = b[r];
                    for e in self.a_row_start[r]..self.a_row_start[r + 1] {
                        s -= self.a_values[e] * x[self.a_cols[e]];
                    }
                    s
                })
                .collect();
            let d = self.substitute(&residual);
            if !d.iter().all(|z| z.finite()) {
                break;
            }
            let size = d.iter().zip(&self.col_scale).map(|(z, s)| z.magnitude() / s).fold(0.0, f64::max);
            if size >= last {
                break;
            }
            for (xi, di) in x.iter_mut().zip(&d) {
                *xi += *di;
            }
            last = size;
        }
        x.iter().all(|z| z.finite()).then_some(x)
    }

    /// Entries of `L` and `U` (the diagonal included): the work of a solve.
    pub fn factor_entries(&self) -> usize {
        self.l_rows.len() + self.u_rows.len() + self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lu::Lu;
    use crate::real_lu::RealLu;
    use crate::rng::Rng;

    /// A random `n x n` circuit-like matrix: a few entries per row over a
    /// diagonal, values spanning several decades, some zero diagonals.
    fn random_dense(rng: &mut Rng, n: usize) -> Vec<f64> {
        let mut a = vec![0.0; n * n];
        for r in 0..n {
            if r % 5 != 3 {
                a[r * n + r] = (1.0 + rng.uniform()) * 10f64.powi((rng.uniform() * 8.0) as i32 - 4);
            }
            for _ in 0..3 {
                let c = (rng.next_u64() % n as u64) as usize;
                a[r * n + c] += rng.symmetric() * 10f64.powi((rng.uniform() * 6.0) as i32 - 3);
                a[c * n + r] += rng.symmetric();
            }
        }
        a
    }

    fn relative_error(x: &[f64], y: &[f64]) -> f64 {
        let scale = y.iter().map(|v| v.abs()).fold(0.0, f64::max);
        x.iter().zip(y).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max) / scale
    }

    #[test]
    fn random_sparse_systems_match_the_dense_factorisation() {
        let mut rng = Rng::new(7);
        let mut solved = 0;
        for trial in 0..60 {
            let n = 5 + trial * 3;
            let dense = random_dense(&mut rng, n);
            let b: Vec<f64> = (0..n).map(|_| rng.symmetric()).collect();
            let Ok(reference) = RealLu::factor(dense.clone(), n) else { continue };
            let Some(want) = reference.solve(&b) else { continue };
            let a = SparseMatrix::from_dense(&dense, n);
            let lu = SparseLu::factor(&a).expect("a regular random matrix factors");
            let got = lu.solve(&b).expect("finite");
            assert!(relative_error(&got, &want) < 1e-9, "n = {n}: {}", relative_error(&got, &want));
            solved += 1;
        }
        assert!(solved > 50, "{solved} of 60 random systems were regular");
    }

    #[test]
    fn a_ladder_factors_without_fill() {
        // An RC ladder driven by a voltage source: tridiagonal with one branch row and column.
        let n = 200;
        let mut a: SparseMatrix<f64> = SparseMatrix::new(n + 1);
        for k in 0..n {
            a.add(k, k, 2.0 + 1e-3 * k as f64);
            if k + 1 < n {
                a.add(k, k + 1, -1.0);
                a.add(k + 1, k, -1.0);
            }
        }
        a.add(0, n, 1.0);
        a.add(n, 0, 1.0);
        a.finish();
        let lu = SparseLu::factor(&a).unwrap();
        assert_eq!(lu.factor_entries(), a.entries());
        let b: Vec<f64> = (0..=n).map(|k| if k == n { 1.0 } else { 0.0 }).collect();
        let want = RealLu::factor(a.to_dense(), n + 1).unwrap().solve(&b).unwrap();
        assert!(relative_error(&lu.solve(&b).unwrap(), &want) < 1e-12);
    }

    #[test]
    fn complex_systems_match_the_dense_factorisation() {
        let mut rng = Rng::new(11);
        for n in [3, 10, 40] {
            let re = random_dense(&mut rng, n);
            let dense: Vec<Complex> = re
                .iter()
                .map(|&v| if v == 0.0 { Complex::ZERO } else { Complex::new(v, rng.symmetric() * v) })
                .collect();
            let b: Vec<Complex> = (0..n).map(|_| Complex::new(rng.symmetric(), rng.symmetric())).collect();
            let want = Lu::factor(dense.clone(), n).unwrap().solve(&b).unwrap();
            let got = SparseLu::factor(&SparseMatrix::from_dense(&dense, n)).unwrap().solve(&b).unwrap();
            let scale = want.iter().map(|z| z.abs()).fold(0.0, f64::max);
            let error = got.iter().zip(&want).map(|(a, b)| (*a - *b).abs()).fold(0.0, f64::max);
            assert!(error < 1e-9 * scale, "n = {n}: {error}");
        }
    }

    #[test]
    fn refactoring_reuses_the_order_until_a_pivot_is_too_small() {
        let mut rng = Rng::new(3);
        let n = 30;
        let dense = random_dense(&mut rng, n);
        let a = SparseMatrix::from_dense(&dense, n);
        let mut lu = SparseLu::factor(&a).unwrap();
        assert!(lu.factors(&a) && lu.fits(&a));
        // New values on the same pattern: the same order serves.
        let mut moved = a.clone();
        for v in &mut moved.values {
            *v *= 1.0 + 0.01 * rng.symmetric();
        }
        assert!(!lu.factors(&moved) && lu.fits(&moved));
        assert!(lu.refactor(&moved));
        assert!(lu.factors(&moved));
        let b: Vec<f64> = (0..n).map(|_| rng.symmetric()).collect();
        let want = RealLu::factor(moved.to_dense(), n).unwrap().solve(&b).unwrap();
        assert!(relative_error(&lu.solve(&b).unwrap(), &want) < 1e-9);
        // A first pivot that vanishes: the order no longer serves, a fresh factorisation does.
        let mut two = SparseMatrix::from_dense(&[4.0, 1.0, 1.0, 3.0], 2);
        let mut lu = SparseLu::factor(&two).unwrap();
        two.values = vec![1e-9, 1.0, 1.0, 3.0];
        assert!(!lu.refactor(&two));
        let x = SparseLu::factor(&two).unwrap().solve(&[1.0, 2.0]).unwrap();
        let want = RealLu::factor(two.to_dense(), 2).unwrap().solve(&[1.0, 2.0]).unwrap();
        assert!(relative_error(&x, &want) < 1e-12);
    }

    #[test]
    fn singular_and_nearly_singular_matrices_are_left_to_the_dense_factorisation() {
        // Each is refused, and the dense factorisation gives its verdict.
        let cases: [(Vec<f64>, LuError); 4] = [
            (vec![1.0, -1.0, -1.0, 1.0], LuError::Singular(1)),
            (vec![1.0, 0.0, 0.0, 0.0], LuError::EmptyRow(1)),
            (vec![1.0, 1.0, 1.0, 1.0 + 1e-14], LuError::Singular(1)),
            (vec![1.0, f64::NAN, 0.0, 1.0], LuError::NonFinite),
        ];
        for (dense, verdict) in cases {
            let a = SparseMatrix::from_dense(&dense, 2);
            assert!(SparseLu::factor(&a).is_err(), "{dense:?}");
            assert_eq!(RealLu::factor(a.to_dense(), 2).unwrap_err(), verdict, "{dense:?}");
        }
        // A zero column (the unknown appears in no equation).
        let a = SparseMatrix::from_dense(&[1.0, 0.0, 2.0, 0.0], 2);
        assert!(SparseLu::factor(&a).is_err());
        // Nearly singular but within the dense tolerance: factored here as
        // weak factors, whose refined solve is the dense solve's.
        let a = SparseMatrix::from_dense(&[1.0, 1.0, 1.0, 1.0 + 1e-11], 2);
        let lu = SparseLu::factor(&a).expect("a pivot within the dense tolerance");
        assert!(lu.weak);
        let want = RealLu::factor(a.to_dense(), 2).unwrap().solve(&[1.0, 2.0]).unwrap();
        let got = lu.solve(&[1.0, 2.0]).unwrap();
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() <= 1e-4 * w.abs(), "{got:?} against {want:?}");
        }
        // Refactoring into a singular matrix fails rather than dividing by zero.
        let mut a = SparseMatrix::from_dense(&[2.0, 1.0, 1.0, 2.0], 2);
        let mut lu = SparseLu::factor(&a).unwrap();
        a.values = vec![1.0, 1.0, 1.0, 1.0];
        assert!(!lu.refactor(&a));
    }

    #[test]
    fn powers_of_two_from_the_bits() {
        for x in [1.0, 1.4, 1.5, 0.7, 3.0, 1e-300, 1e300, 123.456, 2f64.powi(-1010), f64::MIN_POSITIVE / 4.0] {
            let want = 2f64.powi(x.log2().round().clamp(-1000.0, 1000.0) as i32);
            assert_eq!(power_of_two_near(x), want, "{x}");
        }
        assert_eq!(power_of_two_near(f64::INFINITY), 2f64.powi(1000));
    }

    #[test]
    fn the_pattern_grows_and_loads() {
        let mut a: SparseMatrix<f64> = SparseMatrix::new(3);
        a.add(0, 0, 2.0);
        a.add(2, 1, -1.0);
        a.add(0, 0, 1.0);
        assert!(a.finish());
        assert_eq!(a.entries(), 2);
        assert_eq!(a.to_dense(), vec![3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0]);
        a.clear();
        a.add(2, 1, 5.0);
        assert!(!a.finish());
        assert_eq!(a.row(2).collect::<Vec<_>>(), vec![(1, 5.0)]);
        // An entry stamped zero stays in the pattern.
        a.add(1, 2, 0.0);
        assert!(a.finish() && a.entries() == 3);
        let t = a.transposed();
        assert_eq!(t.row(1).collect::<Vec<_>>(), vec![(2, 5.0)]);
        // Loading a matrix of another pattern keeps only its entries.
        let mut b: SparseMatrix<f64> = SparseMatrix::new(3);
        b.add(1, 1, 4.0);
        b.finish();
        a.load(&b);
        a.finish();
        assert_eq!(a.to_dense(), vec![0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 0.0]);
        let mut c = a.clone();
        c.load(&a);
        assert_eq!(c, a);
    }
}

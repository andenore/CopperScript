//! Dense complex LU factorisation with row equilibration and partial pivoting.
//!
//! Sizes are tens to a few hundred unknowns, so a dense row-major matrix is
//! simplest and fast enough. The matrix is first equilibrated: rows, then
//! columns, then rows again are scaled so their largest entry has magnitude
//! about 1 (KCL rows are in siemens, branch rows are dimensionless, node
//! voltages and branch currents differ in scale, and conductances span many
//! decades). The scale factors are powers of two, so scaling adds no
//! rounding. Elimination then picks the largest remaining entry of each
//! column as the pivot. A pivot at or below [`PIVOT_TOLERANCE`] (relative to
//! the unit scale) reports the column as singular, and an all-zero row or
//! column reports that row or column. The factors are
//! kept, so one factorisation serves several right-hand sides (the noise
//! analysis of a later phase solves the transposed system with the same
//! factors). [`Lu::solve`] does one step of iterative refinement against the
//! original matrix and returns an error instead of a non-finite solution.

use crate::complex::Complex;

/// Pivots at or below this magnitude (rows scaled to a unit largest entry)
/// mean a singular matrix.
pub const PIVOT_TOLERANCE: f64 = 1e-13;

/// Why a factorisation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuError {
    /// Row `0` has no non-zero entry (an equation with no terms).
    EmptyRow(usize),
    /// No usable pivot in column `0` (the unknown of that column is not
    /// determined by the equations).
    Singular(usize),
    /// The matrix has a non-finite entry.
    NonFinite,
}

/// The power of two nearest to `x` (in log scale), within `2^-1000 ..= 2^1000`.
fn power_of_two_near(x: f64) -> f64 {
    let e = if x.is_finite() { x.log2().round().clamp(-1000.0, 1000.0) } else { 1000.0 };
    2f64.powi(e as i32)
}

/// The LU factors of a square matrix.
#[derive(Debug, Clone)]
pub struct Lu {
    n: usize,
    /// The original matrix, row-major, for iterative refinement.
    a: Vec<Complex>,
    /// Row scale factors applied before factoring.
    row_scale: Vec<f64>,
    /// Column scale factors: the factored matrix is `R A C`.
    col_scale: Vec<f64>,
    /// Unit-lower `L` (below the diagonal) and `U` (on and above), row-major,
    /// rows in pivot order.
    lu: Vec<Complex>,
    /// `perm[i]` is the original row in position `i`.
    perm: Vec<usize>,
}

impl Lu {
    /// Factor the row-major `n x n` matrix `a`.
    pub fn factor(a: Vec<Complex>, n: usize) -> Result<Lu, LuError> {
        assert_eq!(a.len(), n * n, "matrix size");
        if a.iter().any(|z| !z.is_finite()) {
            return Err(LuError::NonFinite);
        }
        let mut lu = a.clone();
        let mut row_scale = vec![1.0; n];
        let mut col_scale = vec![1.0; n];
        for pass in 0..3 {
            if pass == 1 {
                for (c, cs) in col_scale.iter_mut().enumerate() {
                    let big = (0..n).map(|r| lu[r * n + c].abs()).fold(0.0, f64::max);
                    if big == 0.0 {
                        return Err(LuError::Singular(c));
                    }
                    let f = power_of_two_near(1.0 / big);
                    *cs *= f;
                    for r in 0..n {
                        lu[r * n + c] = lu[r * n + c].scale(f);
                    }
                }
                continue;
            }
            for (r, rs) in row_scale.iter_mut().enumerate() {
                let row = &mut lu[r * n..(r + 1) * n];
                let big = row.iter().map(|z| z.abs()).fold(0.0, f64::max);
                if big == 0.0 {
                    return Err(LuError::EmptyRow(r));
                }
                let f = power_of_two_near(1.0 / big);
                *rs *= f;
                for z in row.iter_mut() {
                    *z = z.scale(f);
                }
            }
        }
        let mut perm: Vec<usize> = (0..n).collect();
        let mut nonzero: Vec<(usize, Complex)> = Vec::with_capacity(n);
        for col in 0..n {
            let mut piv = col;
            let mut best = lu[col * n + col].abs();
            for r in col + 1..n {
                let m = lu[r * n + col].abs();
                if m > best {
                    piv = r;
                    best = m;
                }
            }
            if best.is_nan() || best <= PIVOT_TOLERANCE {
                return Err(LuError::Singular(col));
            }
            if piv != col {
                for c in 0..n {
                    lu.swap(col * n + c, piv * n + c);
                }
                perm.swap(col, piv);
            }
            let d = lu[col * n + col];
            // The pivot row's nonzero entries only: an update by a zero entry
            // leaves the element as it is (but for the sign of a zero).
            nonzero.clear();
            nonzero.extend((col + 1..n).filter(|&c| lu[col * n + c] != Complex::ZERO).map(|c| (c, lu[col * n + c])));
            for r in col + 1..n {
                let f = lu[r * n + col] / d;
                lu[r * n + col] = f;
                if f == Complex::ZERO {
                    continue;
                }
                for &(c, u) in &nonzero {
                    let t = f * u;
                    lu[r * n + c] -= t;
                }
            }
        }
        Ok(Lu { n, a, row_scale, col_scale, lu, perm })
    }

    /// Number of unknowns.
    pub fn size(&self) -> usize {
        self.n
    }

    fn substitute(&self, b: &[Complex]) -> Vec<Complex> {
        let n = self.n;
        // Apply row scaling and the pivot permutation.
        let mut y: Vec<Complex> = self.perm.iter().map(|&r| b[r].scale(self.row_scale[r])).collect();
        for r in 0..n {
            let row = &self.lu[r * n..r * n + r];
            let s = row.iter().zip(&y[..r]).fold(y[r], |s, (l, yc)| s - *l * *yc);
            y[r] = s;
        }
        for r in (0..n).rev() {
            let row = &self.lu[r * n + r + 1..(r + 1) * n];
            let s = row.iter().zip(&y[r + 1..]).fold(y[r], |s, (u, yc)| s - *u * *yc);
            y[r] = s / self.lu[r * n + r];
        }
        // Undo the column scaling: x = C y.
        for (yi, cs) in y.iter_mut().zip(&self.col_scale) {
            *yi = yi.scale(*cs);
        }
        y
    }

    /// Solve `A x = b`, with one step of iterative refinement. `None` if the
    /// solution is not finite.
    pub fn solve(&self, b: &[Complex]) -> Option<Vec<Complex>> {
        let n = self.n;
        assert_eq!(b.len(), n, "right-hand side size");
        let mut x = self.substitute(b);
        if x.iter().any(|z| !z.is_finite()) {
            return None;
        }
        let residual: Vec<Complex> = (0..n)
            .map(|r| {
                let mut s = b[r];
                for (c, xc) in x.iter().enumerate() {
                    s -= self.a[r * n + c] * *xc;
                }
                s
            })
            .collect();
        let d = self.substitute(&residual);
        if d.iter().all(|z| z.is_finite()) {
            for (xi, di) in x.iter_mut().zip(&d) {
                *xi += *di;
            }
        }
        x.iter().all(|z| z.is_finite()).then_some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> Complex {
        Complex::new(re, im)
    }

    #[test]
    fn solves_a_complex_system_needing_pivoting() {
        // Zero leading entry forces a row swap.
        let a = vec![
            c(0.0, 0.0),
            c(2.0, 1.0),
            c(1.0, 0.0),
            c(1.0, -1.0),
            c(0.0, 0.0),
            c(3.0, 0.0),
            c(4.0, 0.0),
            c(1.0, 0.0),
            c(0.0, 2.0),
        ];
        let x_true = [c(1.0, 2.0), c(-0.5, 0.25), c(3.0, -1.0)];
        let b: Vec<Complex> = (0..3).map(|r| (0..3).fold(Complex::ZERO, |s, k| s + a[r * 3 + k] * x_true[k])).collect();
        let lu = Lu::factor(a, 3).unwrap();
        let x = lu.solve(&b).unwrap();
        for (got, want) in x.iter().zip(&x_true) {
            assert!((*got - *want).abs() < 1e-14, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn badly_scaled_rows_are_equilibrated() {
        // Conductances of 1e-12 S next to a unit branch row.
        let a = vec![c(1e-12, 0.0), c(-1e-12, 0.0), c(1.0, 0.0), c(1.0, 0.0)];
        let lu = Lu::factor(a, 2).unwrap();
        let x = lu.solve(&[c(1e-12, 0.0), c(3.0, 0.0)]).unwrap();
        assert!((x[0] - c(2.0, 0.0)).abs() < 1e-12 && (x[1] - c(1.0, 0.0)).abs() < 1e-12, "{x:?}");
    }

    #[test]
    fn column_scaling_handles_a_source_into_a_huge_admittance() {
        // KCL row [Y, 1], branch row [1, 0] with Y = 1e300: rows alone leave a
        // Schur complement of 1e-300; scaling the branch column fixes it.
        let a = vec![c(0.0, 1e300), c(1.0, 0.0), c(1.0, 0.0), c(0.0, 0.0)];
        let lu = Lu::factor(a, 2).unwrap();
        let x = lu.solve(&[Complex::ZERO, c(2.0, 0.0)]).unwrap();
        assert!((x[0] - c(2.0, 0.0)).abs() < 1e-15, "{x:?}");
        assert!((x[1] - c(0.0, -2e300)).abs() < 1e285, "{x:?}");
    }

    #[test]
    fn singular_and_empty() {
        let a = vec![c(1.0, 0.0), c(-1.0, 0.0), c(-1.0, 0.0), c(1.0, 0.0)];
        assert_eq!(Lu::factor(a, 2).unwrap_err(), LuError::Singular(1));
        let a = vec![c(1.0, 0.0), c(0.0, 0.0), c(0.0, 0.0), c(0.0, 0.0)];
        assert_eq!(Lu::factor(a, 2).unwrap_err(), LuError::EmptyRow(1));
        let a = vec![c(f64::NAN, 0.0)];
        assert_eq!(Lu::factor(a, 1).unwrap_err(), LuError::NonFinite);
    }
}

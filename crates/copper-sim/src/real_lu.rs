//! Dense real LU factorisation for the Newton and transient solves.
//!
//! The same method as the complex [`crate::lu`]: rows, then columns, then
//! rows equilibrated by powers of two, partial pivoting, the same
//! [`PIVOT_TOLERANCE`], one step of iterative refinement. Real arithmetic is
//! about four times cheaper, which matters when a transient run factors the
//! matrix thousands of times.

use crate::lu::{LuError, PIVOT_TOLERANCE};

fn power_of_two_near(x: f64) -> f64 {
    let e = if x.is_finite() { x.log2().round().clamp(-1000.0, 1000.0) } else { 1000.0 };
    2f64.powi(e as i32)
}

/// The LU factors of a real square matrix.
#[derive(Debug, Clone)]
pub struct RealLu {
    n: usize,
    a: Vec<f64>,
    row_scale: Vec<f64>,
    col_scale: Vec<f64>,
    lu: Vec<f64>,
    perm: Vec<usize>,
}

impl RealLu {
    /// Factor the row-major `n x n` matrix `a`.
    pub fn factor(a: Vec<f64>, n: usize) -> Result<RealLu, LuError> {
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
                        lu[r * n + c] *= f;
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
                    *z *= f;
                }
            }
        }
        let mut perm: Vec<usize> = (0..n).collect();
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
            let (upper, lower) = lu.split_at_mut((col + 1) * n);
            let pivot_row = &upper[col * n + col + 1..col * n + n];
            for r in 0..n - col - 1 {
                let row = &mut lower[r * n..(r + 1) * n];
                let f = row[col] / d;
                row[col] = f;
                if f == 0.0 {
                    continue;
                }
                for (x, u) in row[col + 1..].iter_mut().zip(pivot_row) {
                    *x -= f * u;
                }
            }
        }
        Ok(RealLu { n, a, row_scale, col_scale, lu, perm })
    }

    /// The matrix that was factored, row-major.
    pub fn matrix(&self) -> &[f64] {
        &self.a
    }

    fn substitute(&self, b: &[f64]) -> Vec<f64> {
        let n = self.n;
        let mut y: Vec<f64> = self.perm.iter().map(|&r| b[r] * self.row_scale[r]).collect();
        for r in 0..n {
            let row = &self.lu[r * n..r * n + r];
            y[r] -= row.iter().zip(&y[..r]).map(|(l, yc)| l * yc).sum::<f64>();
        }
        for r in (0..n).rev() {
            let row = &self.lu[r * n + r + 1..(r + 1) * n];
            let s = y[r] - row.iter().zip(&y[r + 1..]).map(|(u, yc)| u * yc).sum::<f64>();
            y[r] = s / self.lu[r * n + r];
        }
        for (yi, cs) in y.iter_mut().zip(&self.col_scale) {
            *yi *= cs;
        }
        y
    }

    /// Solve `A x = b` with one step of iterative refinement; `None` if not finite.
    pub fn solve(&self, b: &[f64]) -> Option<Vec<f64>> {
        let n = self.n;
        assert_eq!(b.len(), n, "right-hand side size");
        let mut x = self.substitute(b);
        if x.iter().any(|z| !z.is_finite()) {
            return None;
        }
        let residual: Vec<f64> =
            (0..n).map(|r| b[r] - self.a[r * n..(r + 1) * n].iter().zip(&x).map(|(a, x)| a * x).sum::<f64>()).collect();
        let d = self.substitute(&residual);
        if d.iter().all(|z| z.is_finite()) {
            for (xi, di) in x.iter_mut().zip(&d) {
                *xi += di;
            }
        }
        x.iter().all(|z| z.is_finite()).then_some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_with_pivoting_and_reports_singular() {
        let a = vec![0.0, 2.0, 1.0, 1.0, 0.0, 3.0, 4.0, 1.0, 2.0];
        let x_true = [1.0, -0.5, 3.0];
        let b: Vec<f64> = (0..3).map(|r| (0..3).map(|k| a[r * 3 + k] * x_true[k]).sum()).collect();
        let x = RealLu::factor(a, 3).unwrap().solve(&b).unwrap();
        for (got, want) in x.iter().zip(&x_true) {
            assert!((got - want).abs() < 1e-14);
        }
        assert_eq!(RealLu::factor(vec![1.0, -1.0, -1.0, 1.0], 2).unwrap_err(), LuError::Singular(1));
        assert_eq!(RealLu::factor(vec![1.0, 0.0, 0.0, 0.0], 2).unwrap_err(), LuError::EmptyRow(1));
    }
}

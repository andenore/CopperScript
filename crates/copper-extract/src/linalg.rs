//! Dense complex linear algebra for the loop equations: Gaussian elimination
//! with partial pivoting on a matrix stored as separate real and imaginary
//! planes (so the inner loops are plain `f64` slices), with several right-hand
//! sides solved together.

use copper_emi::spectrum::Complex;

/// The matrix is singular to working precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Singular;

/// A square complex matrix, row major, real and imaginary parts apart.
#[derive(Clone, Debug)]
pub struct CMatrix {
    n: usize,
    re: Vec<f64>,
    im: Vec<f64>,
}

impl CMatrix {
    /// `re + j * scale * im` for two real `n x n` row-major matrices.
    pub fn from_planes(n: usize, re: &[f64], im: &[f64], scale: f64) -> CMatrix {
        assert_eq!(re.len(), n * n);
        assert_eq!(im.len(), n * n);
        CMatrix { n, re: re.to_vec(), im: im.iter().map(|v| v * scale).collect() }
    }

    pub fn dim(&self) -> usize {
        self.n
    }

    /// Solve `A X = B` in place. `rhs` holds one vector of length `n` per column of `B`; on return it holds the
    /// solutions. The matrix is destroyed.
    pub fn solve(mut self, rhs: &mut [Vec<Complex>]) -> Result<(), Singular> {
        let n = self.n;
        let nc = rhs.len();
        if n == 0 {
            return Ok(());
        }
        // Right-hand sides as planes, row major n x nc.
        let mut br = vec![0.0; n * nc];
        let mut bi = vec![0.0; n * nc];
        for (c, column) in rhs.iter().enumerate() {
            for (r, v) in column.iter().enumerate() {
                br[r * nc + c] = v.re;
                bi[r * nc + c] = v.im;
            }
        }
        let scale = self.re.iter().chain(self.im.iter()).fold(0.0_f64, |m, v| m.max(v.abs()));
        if scale.is_nan() || scale <= 0.0 || !scale.is_finite() {
            return Err(Singular);
        }
        for k in 0..n {
            // Partial pivoting on the column below the diagonal.
            let mut best = k;
            let mut best_mag = -1.0;
            for i in k..n {
                let (a, b) = (self.re[i * n + k], self.im[i * n + k]);
                let mag = a * a + b * b;
                if mag > best_mag {
                    best_mag = mag;
                    best = i;
                }
            }
            if best_mag.sqrt() < 1e-14 * scale {
                return Err(Singular);
            }
            if best != k {
                for j in 0..n {
                    self.re.swap(k * n + j, best * n + j);
                    self.im.swap(k * n + j, best * n + j);
                }
                for c in 0..nc {
                    br.swap(k * nc + c, best * nc + c);
                    bi.swap(k * nc + c, best * nc + c);
                }
            }
            let (pr, pi) = (self.re[k * n + k], self.im[k * n + k]);
            let denom = pr * pr + pi * pi;
            let (ir, ii) = (pr / denom, -pi / denom);
            let (re_head, re_tail) = self.re.split_at_mut((k + 1) * n);
            let (im_head, im_tail) = self.im.split_at_mut((k + 1) * n);
            let row_re = &re_head[k * n + k + 1..k * n + n];
            let row_im = &im_head[k * n + k + 1..k * n + n];
            let (b_head_r, b_tail_r) = br.split_at_mut((k + 1) * nc);
            let (b_head_i, b_tail_i) = bi.split_at_mut((k + 1) * nc);
            let krow_r = &b_head_r[k * nc..];
            let krow_i = &b_head_i[k * nc..];
            for i in 0..(n - k - 1) {
                let base = i * n;
                let (ar, ai) = (re_tail[base + k], im_tail[base + k]);
                if ar == 0.0 && ai == 0.0 {
                    continue;
                }
                // Multiplier f = a_ik / a_kk.
                let (fr, fi) = (ar * ir - ai * ii, ar * ii + ai * ir);
                let dst_r = &mut re_tail[base + k + 1..base + n];
                let dst_i = &mut im_tail[base + k + 1..base + n];
                for j in 0..dst_r.len() {
                    dst_r[j] -= fr * row_re[j] - fi * row_im[j];
                    dst_i[j] -= fr * row_im[j] + fi * row_re[j];
                }
                for c in 0..nc {
                    let (xr, xi) = (krow_r[c], krow_i[c]);
                    b_tail_r[i * nc + c] -= fr * xr - fi * xi;
                    b_tail_i[i * nc + c] -= fr * xi + fi * xr;
                }
            }
        }
        // Back substitution.
        for k in (0..n).rev() {
            let (pr, pi) = (self.re[k * n + k], self.im[k * n + k]);
            let denom = pr * pr + pi * pi;
            let (ir, ii) = (pr / denom, -pi / denom);
            for c in 0..nc {
                let (mut sr, mut si) = (br[k * nc + c], bi[k * nc + c]);
                for j in (k + 1)..n {
                    let (ar, ai) = (self.re[k * n + j], self.im[k * n + j]);
                    let (xr, xi) = (br[j * nc + c], bi[j * nc + c]);
                    sr -= ar * xr - ai * xi;
                    si -= ar * xi + ai * xr;
                }
                br[k * nc + c] = sr * ir - si * ii;
                bi[k * nc + c] = sr * ii + si * ir;
            }
        }
        for (c, column) in rhs.iter_mut().enumerate() {
            for (r, v) in column.iter_mut().enumerate() {
                *v = Complex::new(br[r * nc + c], bi[r * nc + c]);
            }
        }
        if rhs.iter().flatten().any(|v| !(v.re.is_finite() && v.im.is_finite())) {
            return Err(Singular);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_small_complex_system_and_detects_singularity() {
        // (1+j) x + 2 y = 3 ; x - j y = 1 + j  ->  check by substitution.
        let re = vec![1.0, 2.0, 1.0, 0.0];
        let im = vec![1.0, 0.0, 0.0, -1.0];
        let m = CMatrix::from_planes(2, &re, &im, 1.0);
        let mut rhs = vec![vec![Complex::new(3.0, 0.0), Complex::new(1.0, 1.0)]];
        m.solve(&mut rhs).expect("regular");
        let (x, y) = (rhs[0][0], rhs[0][1]);
        let r1 = Complex::new(1.0, 1.0) * x + y.scale(2.0);
        let r2 = x - Complex::J * y;
        assert!((r1 - Complex::new(3.0, 0.0)).abs() < 1e-12 && (r2 - Complex::new(1.0, 1.0)).abs() < 1e-12);
        let singular = CMatrix::from_planes(2, &[1.0, 2.0, 2.0, 4.0], &[0.0; 4], 1.0);
        assert_eq!(singular.solve(&mut [vec![Complex::ONE, Complex::ONE]]), Err(Singular));
    }
}

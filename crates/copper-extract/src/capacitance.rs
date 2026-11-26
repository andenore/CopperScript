//! Capacitance per unit length of conductors in a layered dielectric: a 2-D electrostatic field solution of one
//! cross-section.
//!
//! **Method.** Finite volumes (the box-integration form of finite differences) on a tensor grid of the
//! cross-section `(x, z)`: lines at every conductor edge and dielectric interface, graded geometrically away from
//! them (finest next to conductor edges, where the charge density is singular), coarse far away. Each grid node
//! carries a potential; a conductor's nodes are held at its potential, the outer box's nodes at 0 V (a Dirichlet
//! wall) or left free (a Neumann wall: no flux crosses it). The flux between two neighbouring nodes is the
//! permittivity of the two cells beside their edge, weighted by the cells' widths, times the potential difference
//! over the edge length. The equations of the free nodes are symmetric positive definite and banded (the nodes
//! numbered across the shorter grid dimension first), and are solved directly by banded Cholesky: deterministic and
//! without iteration tolerances. A conductor's charge is the flux leaving its nodes, which is Gauss's law on the
//! discrete grid, so the charges of a solution sum to zero with the box's and the capacitance matrix is symmetric to
//! rounding.
//!
//! **What it is valid for.** Quasi-static (TEM) fields: the cross-section is uniform along the third axis, so a
//! result is per metre of length, and end, corner and crossing effects of real 3-D copper are not in it (the board
//! assembly of [`crate::parasitics`] says how it uses it). Dielectrics are horizontal slabs with one isotropic
//! relative permittivity each; air elsewhere. Copper is a perfect conductor of rectangular cross-section, any
//! thickness including zero (a line of nodes). Solder mask, dielectric dispersion and conductor roughness are not
//! modelled.
//!
//! **Accuracy** (tests in `tests/capacitance.rs`, against closed forms): see [`SectionMesh::default`] and the
//! module documentation of those tests.

use std::f64::consts::PI;

/// Vacuum permittivity, F/m (CODATA 2018).
pub const EPSILON_0: f64 = 8.854_187_812_8e-12;
/// Vacuum permeability, H/m (`1 / (epsilon_0 c^2)`).
pub const MU_0: f64 = 1.0 / (EPSILON_0 * SPEED_OF_LIGHT * SPEED_OF_LIGHT);
/// Speed of light in vacuum, m/s.
pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;

/// A horizontal dielectric slab, `bottom < top`, metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slab {
    pub bottom: f64,
    pub top: f64,
    pub er: f64,
}

/// A conductor's cross-section: the rectangle `[x0, x1] x [z0, z1]` (metres, `z0 == z1` for zero thickness) of
/// conductor `group`. Rectangles of one group are one conductor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub x1: f64,
    pub z0: f64,
    pub z1: f64,
    pub group: usize,
}

/// What a wall of the box does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wall {
    /// Held at 0 V; its charge is reported as the box's.
    Grounded,
    /// No flux crosses it (a mirror plane).
    Open,
}

/// The grid's resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionMesh {
    /// The finest cell, as a fraction of the smallest conductor width, gap or dielectric thickness.
    pub fine_fraction: f64,
    /// Bounds on the finest cell, metres.
    pub fine_min: f64,
    pub fine_max: f64,
    /// Geometric growth of cells away from an edge.
    pub growth: f64,
    /// The coarsest cell, as a fraction of the box's extent along the axis.
    pub coarse_fraction: f64,
    /// Solve twice, with the finest cell as given and halved, and extrapolate `2 Q(h/2) - Q(h)`: the error of the
    /// edge cells is first order in their size (measured: a factor 4 finer leaves 0.26 of it), so this removes it
    /// and leaves the error of the grading.
    pub extrapolate: bool,
}

impl Default for SectionMesh {
    /// The mesh the board extraction uses: the finest cell 1/20 of the smallest feature (between 0.5 um and
    /// 20 um), growth 1.15, the coarsest cell 1/20 of the box, extrapolated from that cell and its half. The errors
    /// it leaves on the closed forms are in `tests/capacitance.rs`.
    fn default() -> Self {
        SectionMesh {
            fine_fraction: 1.0 / 20.0,
            fine_min: 0.5e-6,
            fine_max: 20e-6,
            growth: 1.15,
            coarse_fraction: 0.05,
            extrapolate: true,
        }
    }
}

/// A cross-section to solve.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub conductors: Vec<Rect>,
    pub slabs: Vec<Slab>,
    /// The box `[x0, z0, x1, z1]`, metres; copper outside it is cut off.
    pub bounds: [f64; 4],
    /// The left and right walls.
    pub sides: Wall,
    /// The bottom and top walls.
    pub ends: Wall,
    pub mesh: SectionMesh,
}

/// Failures of a solve.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionError {
    /// Nothing to solve: no free node, or a box without a grounded wall and no conductor held at 0 V.
    Degenerate(String),
    /// The matrix is not positive definite (a floating region with no path to a held potential).
    Singular,
}

impl std::fmt::Display for SectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SectionError::Degenerate(why) => write!(f, "the cross-section cannot be solved: {why}"),
            SectionError::Singular => f.write_str("the cross-section's equations are singular"),
        }
    }
}

impl std::error::Error for SectionError {}

/// The charges of one solution, C/m.
#[derive(Clone, Debug, PartialEq)]
pub struct Charges {
    /// Per conductor group.
    pub groups: Vec<f64>,
    /// On the grounded walls.
    pub walls: f64,
}

/// The tensor grid of a section.
struct Grid {
    x: Vec<f64>,
    z: Vec<f64>,
}

/// Lines over `[lo, hi]`: every key, cells of `fine` at each key growing by `growth` up to `coarse` between them.
fn graded(lo: f64, hi: f64, keys: &[(f64, f64)], growth: f64, coarse: f64) -> Vec<f64> {
    let mut points: Vec<(f64, f64)> = keys.iter().copied().filter(|(x, _)| *x >= lo && *x <= hi).collect();
    points.push((lo, coarse));
    points.push((hi, coarse));
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Merge keys closer than a hundredth of the finer cell, keeping the finer size.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (x, h) in points {
        match merged.last_mut() {
            Some(last) if x - last.0 <= 1e-3 * h.min(last.1) => last.1 = last.1.min(h),
            _ => merged.push((x, h)),
        }
    }
    let mut lines = vec![merged[0].0];
    for pair in merged.windows(2) {
        let ((a, ha), (b, hb)) = (pair[0], pair[1]);
        let span = b - a;
        // Grow from both ends until the two fronts meet; the middle cells are then spread evenly.
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (mut xa, mut xb) = (a, b);
        let (mut sa, mut sb) = (ha.min(span), hb.min(span));
        loop {
            let gap = xb - xa;
            if gap <= sa.max(sb) * 1.5 {
                break;
            }
            if sa <= sb {
                xa += sa;
                left.push(xa);
                sa = (sa * growth).min(coarse);
            } else {
                xb -= sb;
                right.push(xb);
                sb = (sb * growth).min(coarse);
            }
        }
        lines.extend(left);
        lines.extend(right.into_iter().rev());
        lines.push(b);
    }
    lines.dedup_by(|a, b| (*a - *b).abs() < 1e-15);
    lines
}

impl Section {
    /// The smallest feature across (conductor width, gap between conductors on one level, slab thickness), and the
    /// thinnest conductor (`INFINITY` when all are of zero thickness).
    fn smallest_features(&self) -> (f64, f64) {
        let (mut across, mut thinnest) = (f64::INFINITY, f64::INFINITY);
        for c in &self.conductors {
            across = across.min(c.x1 - c.x0);
            if c.z1 > c.z0 {
                thinnest = thinnest.min(c.z1 - c.z0);
            }
        }
        for (i, a) in self.conductors.iter().enumerate() {
            for b in &self.conductors[i + 1..] {
                let overlap_z = a.z0 <= b.z1 && b.z0 <= a.z1;
                if overlap_z {
                    let gap = if a.x1 <= b.x0 { b.x0 - a.x1 } else { a.x0 - b.x1 };
                    if gap > 0.0 {
                        across = across.min(gap);
                    }
                }
            }
        }
        for s in &self.slabs {
            across = across.min(s.top - s.bottom);
        }
        (across, thinnest)
    }

    /// The grid; `refine` scales the finest cells (0.5 for the extrapolation's second grid).
    fn grid(&self, refine: f64) -> Grid {
        let [x0, z0, x1, z1] = self.bounds;
        let m = &self.mesh;
        let (across, thinnest) = self.smallest_features();
        let fine = (across * m.fine_fraction).clamp(m.fine_min, m.fine_max) * refine;
        // Through a conductor's thickness: at least four cells.
        let fine_z = fine.min((thinnest / 4.0).max(m.fine_min * refine));
        let mut xs: Vec<(f64, f64)> = Vec::new();
        let mut zs: Vec<(f64, f64)> = Vec::new();
        for c in &self.conductors {
            xs.push((c.x0, fine));
            xs.push((c.x1, fine));
            zs.push((c.z0, fine_z));
            zs.push((c.z1, fine_z));
        }
        for s in &self.slabs {
            // An interface needs cells of a fraction of the slab, not of the finest feature.
            let h = ((s.top - s.bottom) / 8.0).max(fine);
            zs.push((s.bottom, h));
            zs.push((s.top, h));
        }
        Grid {
            x: graded(x0, x1, &xs, m.growth, (x1 - x0) * m.coarse_fraction),
            z: graded(z0, z1, &zs, m.growth, (z1 - z0) * m.coarse_fraction),
        }
    }

    /// Relative permittivity at height `z`.
    fn er(&self, z: f64) -> f64 {
        self.slabs.iter().find(|s| z > s.bottom && z < s.top).map_or(1.0, |s| s.er)
    }

    /// The number of conductor groups (the largest group index plus one).
    pub fn group_count(&self) -> usize {
        self.conductors.iter().map(|c| c.group + 1).max().unwrap_or(0)
    }

    /// The charges per metre when group `g` is held at `potentials[g]` volts (the walls at 0 V).
    pub fn solve(&self, potentials: &[f64]) -> Result<Charges, SectionError> {
        let coarse = self.solve_on(&self.grid(1.0), potentials)?;
        if !self.mesh.extrapolate {
            return Ok(coarse);
        }
        let fine = self.solve_on(&self.grid(0.5), potentials)?;
        Ok(Charges {
            groups: fine.groups.iter().zip(&coarse.groups).map(|(f, c)| 2.0 * f - c).collect(),
            walls: 2.0 * fine.walls - coarse.walls,
        })
    }

    fn solve_on(&self, grid: &Grid, potentials: &[f64]) -> Result<Charges, SectionError> {
        let (nx, nz) = (grid.x.len(), grid.z.len());
        // Node kinds: usize::MAX free, usize::MAX - 1 wall, else group.
        const FREE: usize = usize::MAX;
        const WALL: usize = usize::MAX - 1;
        let tol = |a: f64, b: f64| (a - b).abs() <= 1e-12 * (1.0 + a.abs().max(b.abs()));
        let mut kind = vec![FREE; nx * nz];
        let at = |i: usize, j: usize| i * nz + j;
        for i in 0..nx {
            for j in 0..nz {
                let wall = (self.sides == Wall::Grounded && (i == 0 || i == nx - 1))
                    || (self.ends == Wall::Grounded && (j == 0 || j == nz - 1));
                if wall {
                    kind[at(i, j)] = WALL;
                }
            }
        }
        for c in &self.conductors {
            let i0 = grid.x.iter().position(|&x| x >= c.x0 || tol(x, c.x0));
            let j0 = grid.z.iter().position(|&z| z >= c.z0 || tol(z, c.z0));
            let (Some(i0), Some(j0)) = (i0, j0) else { continue };
            for i in i0..nx {
                if grid.x[i] > c.x1 && !tol(grid.x[i], c.x1) {
                    break;
                }
                for j in j0..nz {
                    if grid.z[j] > c.z1 && !tol(grid.z[j], c.z1) {
                        break;
                    }
                    if kind[at(i, j)] != WALL {
                        kind[at(i, j)] = c.group;
                    }
                }
            }
        }
        let potential = |k: usize| -> f64 {
            match kind[k] {
                FREE | WALL => 0.0,
                g => potentials.get(g).copied().unwrap_or(0.0),
            }
        };
        // Cell permittivities (cells between lines i, i+1 and j, j+1).
        let cell_eps: Vec<f64> =
            (0..nz.saturating_sub(1)).map(|j| self.er(0.5 * (grid.z[j] + grid.z[j + 1])) * EPSILON_0).collect();
        let dx: Vec<f64> = grid.x.windows(2).map(|w| w[1] - w[0]).collect();
        let dz: Vec<f64> = grid.z.windows(2).map(|w| w[1] - w[0]).collect();
        // Edge conductances: horizontal edge (i,j)-(i+1,j), vertical edge (i,j)-(i,j+1).
        let gh = |i: usize, j: usize| -> f64 {
            let below = if j > 0 { cell_eps[j - 1] * dz[j - 1] / 2.0 } else { 0.0 };
            let above = if j + 1 < nz { cell_eps[j] * dz[j] / 2.0 } else { 0.0 };
            (below + above) / dx[i]
        };
        let gv = |i: usize, j: usize| -> f64 {
            let left = if i > 0 { cell_eps[j] * dx[i - 1] / 2.0 } else { 0.0 };
            let right = if i + 1 < nx { cell_eps[j] * dx[i] / 2.0 } else { 0.0 };
            (left + right) / dz[j]
        };
        // Number the free nodes across the shorter dimension first.
        let across_x = nx < nz;
        let order: Vec<(usize, usize)> = if across_x {
            (0..nz).flat_map(|j| (0..nx).map(move |i| (i, j))).collect()
        } else {
            (0..nx).flat_map(|i| (0..nz).map(move |j| (i, j))).collect()
        };
        let mut index = vec![usize::MAX; nx * nz];
        let mut free: Vec<(usize, usize)> = Vec::new();
        for &(i, j) in &order {
            if kind[at(i, j)] == FREE {
                index[at(i, j)] = free.len();
                free.push((i, j));
            }
        }
        let n = free.len();
        if n == 0 {
            return Err(SectionError::Degenerate("no free node".to_owned()));
        }
        let anchored = self.sides == Wall::Grounded || self.ends == Wall::Grounded || !self.conductors.is_empty();
        if !anchored {
            return Err(SectionError::Degenerate("nothing holds a potential".to_owned()));
        }
        // Bandwidth from the actual neighbours.
        let mut band = 0usize;
        let neighbours = |i: usize, j: usize| -> [Option<(usize, usize, f64)>; 4] {
            [
                (i > 0).then(|| (i - 1, j, gh(i - 1, j))),
                (i + 1 < nx).then(|| (i + 1, j, gh(i, j))),
                (j > 0).then(|| (i, j - 1, gv(i, j - 1))),
                (j + 1 < nz).then(|| (i, j + 1, gv(i, j))),
            ]
        };
        for (k, &(i, j)) in free.iter().enumerate() {
            for (a, b, _) in neighbours(i, j).into_iter().flatten() {
                let m = index[at(a, b)];
                if m != usize::MAX && m < k {
                    band = band.max(k - m);
                }
            }
        }
        let w = band + 1;
        // Lower band: row k holds A[k][k - band ..= k] at [k * w + (col - (k - band))].
        let mut l = vec![0.0f64; n * w];
        let mut rhs = vec![0.0f64; n];
        for (k, &(i, j)) in free.iter().enumerate() {
            let mut diagonal = 0.0;
            for (a, b, g) in neighbours(i, j).into_iter().flatten() {
                if g == 0.0 {
                    continue;
                }
                diagonal += g;
                let m = index[at(a, b)];
                if m == usize::MAX {
                    rhs[k] += g * potential(at(a, b));
                } else if m < k {
                    l[k * w + (m + band - k)] -= g;
                }
            }
            l[k * w + band] = diagonal;
        }
        // Banded Cholesky in place.
        for k in 0..n {
            let first = k.saturating_sub(band);
            for col in first..=k {
                let mut sum = l[k * w + (col + band - k)];
                let start = first.max(col.saturating_sub(band));
                for p in start..col {
                    sum -= l[k * w + (p + band - k)] * l[col * w + (p + band - col)];
                }
                if col == k {
                    if sum.is_nan() || sum <= 0.0 {
                        return Err(SectionError::Singular);
                    }
                    l[k * w + band] = sum.sqrt();
                } else {
                    l[k * w + (col + band - k)] = sum / l[col * w + band];
                }
            }
        }
        // Forward and back substitution.
        let mut y = rhs;
        for k in 0..n {
            let first = k.saturating_sub(band);
            let mut sum = y[k];
            for p in first..k {
                sum -= l[k * w + (p + band - k)] * y[p];
            }
            y[k] = sum / l[k * w + band];
        }
        for k in (0..n).rev() {
            let mut sum = y[k];
            for row in k + 1..(k + w).min(n) {
                sum -= l[row * w + (k + band - row)] * y[row];
            }
            y[k] = sum / l[k * w + band];
        }
        let v = |i: usize, j: usize| -> f64 {
            let k = at(i, j);
            if index[k] != usize::MAX { y[index[k]] } else { potential(k) }
        };
        // Charges: the flux out of each held node into nodes of another kind.
        let groups = self.group_count();
        let mut charges = Charges { groups: vec![0.0; groups], walls: 0.0 };
        for i in 0..nx {
            for j in 0..nz {
                let own = kind[at(i, j)];
                if own == FREE {
                    continue;
                }
                for (a, b, g) in neighbours(i, j).into_iter().flatten() {
                    if kind[at(a, b)] == own {
                        continue;
                    }
                    let flux = g * (v(i, j) - v(a, b));
                    if own == WALL {
                        charges.walls += flux;
                    } else {
                        charges.groups[own] += flux;
                    }
                }
            }
        }
        Ok(charges)
    }

    /// The Maxwell capacitance matrix, F/m: entry `[i][j]` is the charge on group `i` with group `j` at 1 V and every
    /// other group at 0 V (negative off the diagonal). Symmetric to rounding.
    pub fn capacitance_matrix(&self) -> Result<Vec<Vec<f64>>, SectionError> {
        let n = self.group_count();
        let mut matrix = vec![vec![0.0; n]; n];
        for j in 0..n {
            let mut potentials = vec![0.0; n];
            potentials[j] = 1.0;
            let charges = self.solve(&potentials)?;
            for (row, charge) in matrix.iter_mut().zip(&charges.groups) {
                row[j] = *charge;
            }
        }
        Ok(matrix)
    }

    /// The same section with every dielectric replaced by vacuum (for the inductance of TEM lines,
    /// `L = mu_0 epsilon_0 C_0^-1`).
    pub fn in_vacuum(&self) -> Section {
        Section { slabs: Vec::new(), ..self.clone() }
    }

    /// The number of grid lines `(x, z)` the solve uses (for tests and timing).
    pub fn grid_size(&self) -> (usize, usize) {
        let grid = self.grid(1.0);
        (grid.x.len(), grid.z.len())
    }
}

/// Closed forms the solver is checked against (zero-thickness strips).
pub mod closed_form {
    use super::{EPSILON_0, PI, SPEED_OF_LIGHT};

    /// The complete elliptic integral of the first kind `K(k)`, given `k` and its complement `k' = sqrt(1 - k^2)`
    /// (passed separately so that a modulus near 1 keeps its precision), by the arithmetic-geometric mean.
    pub fn elliptic_k(_k: f64, k_complement: f64) -> f64 {
        let (mut a, mut b) = (1.0f64, k_complement);
        for _ in 0..64 {
            let (next_a, next_b) = ((a + b) / 2.0, (a * b).sqrt());
            if (next_a - next_b).abs() <= 1e-16 * next_a {
                a = next_a;
                break;
            }
            a = next_a;
            b = next_b;
        }
        PI / (2.0 * a)
    }

    /// `K(k') / K(k)`.
    pub fn k_ratio(k: f64, k_complement: f64) -> f64 {
        elliptic_k(k_complement, k) / elliptic_k(k, k_complement)
    }

    /// Hammerstad and Jensen (1980): the impedance of a zero-thickness strip of width `u h` at height `h` over a
    /// ground plane in vacuum, ohm. Stated accuracy 0.01 % for `u <= 1`, 0.03 % for `u <= 1000`.
    pub fn microstrip_z_vacuum(u: f64) -> f64 {
        let eta0 = 1.0 / (EPSILON_0 * SPEED_OF_LIGHT);
        let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
        eta0 / (2.0 * PI) * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln()
    }

    /// Hammerstad and Jensen (1980): the effective permittivity of a zero-thickness microstrip of width `u h` on a
    /// substrate of `er` (air above). Stated accuracy 0.2 % for `er <= 128` and `0.01 <= u <= 100`.
    pub fn microstrip_effective_er(u: f64, er: f64) -> f64 {
        let a = 1.0
            + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0
            + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
        let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
        (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b)
    }

    /// Capacitance per metre of a zero-thickness microstrip of width `u h` on `er` (air above), F/m:
    /// `er_eff / (c Z_vacuum)`.
    pub fn microstrip_capacitance(u: f64, er: f64) -> f64 {
        microstrip_effective_er(u, er) / (SPEED_OF_LIGHT * microstrip_z_vacuum(u))
    }

    /// Capacitance per metre of a zero-thickness strip of width `u h` at height `h` over a plane in a homogeneous
    /// dielectric `er`, F/m (Hammerstad's vacuum impedance; accuracy as [`microstrip_z_vacuum`]).
    pub fn strip_over_plane_capacitance(u: f64, er: f64) -> f64 {
        er / (SPEED_OF_LIGHT * microstrip_z_vacuum(u))
    }

    /// Cohn (1955), exact: two zero-thickness strips of width `w`, edge gap `s`, centred between two ground planes
    /// `b` apart in a homogeneous `er`. Returns `(C_even, C_odd)` per strip, F/m: the charge per metre on one strip
    /// with both at 1 V, and with them at +1 V and -1 V.
    pub fn coupled_stripline(w: f64, s: f64, b: f64, er: f64) -> (f64, f64) {
        let eps = EPSILON_0 * er;
        let t1 = (PI * w / (2.0 * b)).tanh();
        let x2 = PI * (w + s) / (2.0 * b);
        let (t2, c2) = (x2.tanh(), 1.0 / x2.tanh());
        // k_e = tanh(a) tanh(b), k_o = tanh(a) coth(b); their complements from sech for precision.
        let ke = t1 * t2;
        let ko = t1 * c2;
        let complement = |k: f64| ((1.0 - k) * (1.0 + k)).sqrt();
        let c_even = 4.0 * eps / super::closed_form::k_ratio(ke, complement(ke));
        let c_odd = 4.0 * eps / super::closed_form::k_ratio(ko, complement(ko));
        (c_even, c_odd)
    }

    /// Exact (conformal mapping): the capacitance per metre between two zero-thickness coplanar strips of width `w`
    /// with an edge gap `s` in a homogeneous `er` and no ground, F/m: `eps K(k') / K(k)`, `k = s / (s + 2 w)`.
    pub fn coplanar_strips(w: f64, s: f64, er: f64) -> f64 {
        let k = s / (s + 2.0 * w);
        let kc = ((1.0 - k) * (1.0 + k)).sqrt();
        EPSILON_0 * er * super::closed_form::k_ratio(k, kc)
    }
}

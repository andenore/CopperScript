//! A small independent circuit solver for the cable fixtures: modified nodal analysis with complex impedances at one
//! frequency, dense Gaussian elimination with partial pivoting.
//!
//! It shares no code with the estimators of `copper_emi` (it does not call `mode_conversion`, `estimate`, `circuit` or
//! any closed form); it uses only the crate's [`Complex`] number type.
//!
//! Node 0 is the reference. Every impedance, source, coupled group and line adds *branch currents* as unknowns, so a
//! zero impedance (a short) and an ideal voltage source need no special case. A branch current is positive when it
//! leaves the first node of the branch through the element.

use copper_emi::spectrum::Complex;

pub struct Circuit {
    nodes: usize,
    omega: f64,
    branches: usize,
    matrix: Vec<(usize, usize, Complex)>,
    rhs: Vec<(usize, Complex)>,
}

pub struct Solution {
    voltages: Vec<Complex>,
    currents: Vec<Complex>,
}

impl Solution {
    /// The voltage of `node` against the reference (node 0 is zero).
    pub fn v(&self, node: usize) -> Complex {
        if node == 0 { Complex::ZERO } else { self.voltages[node - 1] }
    }

    /// The current of branch `id`.
    pub fn i(&self, id: usize) -> Complex {
        self.currents[id]
    }
}

impl Circuit {
    /// A circuit with `nodes` nodes besides the reference (numbered `1..=nodes`) at angular frequency `omega`.
    pub fn new(nodes: usize, omega: f64) -> Self {
        Circuit { nodes, omega, branches: 0, matrix: Vec::new(), rhs: Vec::new() }
    }

    fn voltage_term(&mut self, row: usize, node: usize, coefficient: Complex) {
        if node != 0 {
            self.matrix.push((row, node - 1, coefficient));
        }
    }

    /// A new branch, its current leaving `from` and entering `to` through the element.
    fn branch(&mut self, from: usize, to: usize) -> usize {
        let id = self.branches;
        self.branches += 1;
        let column = self.nodes + id;
        if from != 0 {
            self.matrix.push((from - 1, column, Complex::ONE));
        }
        if to != 0 {
            self.matrix.push((to - 1, column, -Complex::ONE));
        }
        id
    }

    /// The branch row: `V_a - V_b - sum_m z_m i_m = value`, `terms` the `(branch, z)` pairs.
    fn branch_equation(&mut self, a: usize, b: usize, id: usize, terms: &[(usize, Complex)], value: Complex) {
        let row = self.nodes + id;
        self.voltage_term(row, a, Complex::ONE);
        self.voltage_term(row, b, -Complex::ONE);
        for (branch, z) in terms {
            self.matrix.push((row, self.nodes + branch, -*z));
        }
        if value != Complex::ZERO {
            self.rhs.push((row, value));
        }
    }

    /// An impedance from `a` to `b`; returns the branch id (current from `a` to `b`).
    pub fn impedance(&mut self, a: usize, b: usize, z: Complex) -> usize {
        let id = self.branch(a, b);
        self.branch_equation(a, b, id, &[(id, z)], Complex::ZERO);
        id
    }

    /// An ideal voltage source, `V_plus - V_minus = v`; the branch current leaves `plus` through the source, so it is
    /// negative while the source delivers current into the circuit.
    pub fn voltage_source(&mut self, plus: usize, minus: usize, v: Complex) -> usize {
        let id = self.branch(plus, minus);
        self.branch_equation(plus, minus, id, &[], v);
        id
    }

    /// An ideal current source driving `i` out of `from` through the source into `to`.
    pub fn current_source(&mut self, from: usize, to: usize, i: Complex) {
        if from != 0 {
            self.rhs.push((from - 1, -i));
        }
        if to != 0 {
            self.rhs.push((to - 1, i));
        }
    }

    /// Coupled series branches: `V_a_j - V_b_j = sum_k z[j][k] i_k` for each port `j` (a mutual inductance is
    /// `z = j omega M`). Returns the id of the first branch; the others follow.
    pub fn coupled(&mut self, ports: &[(usize, usize)], z: &[Vec<Complex>]) -> usize {
        let first = self.branches;
        for (a, b) in ports {
            self.branch(*a, *b);
        }
        for (j, (a, b)) in ports.iter().enumerate() {
            let terms: Vec<(usize, Complex)> = z[j].iter().enumerate().map(|(k, zk)| (first + k, *zk)).collect();
            self.branch_equation(*a, *b, first + j, &terms, Complex::ZERO);
        }
        first
    }

    /// A lossless line of impedance `z0` and delay `delay` between `node1` and `node2` (both against the reference),
    /// by its chain matrix: `V1 = cos V2 - j z0 sin i2'`, `i1 = j sin V2 / z0 - cos i2'` with `i2'` leaving port 2.
    /// Returns the id of the branch of port 1; port 2 follows.
    pub fn line(&mut self, node1: usize, node2: usize, z0: f64, delay: f64) -> usize {
        let theta = self.omega * delay;
        let (cos, sin) = (theta.cos(), theta.sin());
        let first = self.branch(node1, 0);
        let second = self.branch(node2, 0);
        let (row1, row2) = (self.nodes + first, self.nodes + second);
        // Row 1: V1 - cos V2 + j z0 sin i2 = 0, with i2 the current entering the line at port 2.
        self.voltage_term(row1, node1, Complex::ONE);
        self.voltage_term(row1, node2, Complex::new(-cos, 0.0));
        self.matrix.push((row1, self.nodes + second, Complex::new(0.0, z0 * sin)));
        // Row 2: i1 - j sin V2 / z0 + cos i2 = 0.
        self.matrix.push((row2, self.nodes + first, Complex::ONE));
        self.voltage_term(row2, node2, Complex::new(0.0, -sin / z0));
        self.matrix.push((row2, self.nodes + second, Complex::new(cos, 0.0)));
        first
    }

    pub fn solve(self) -> Solution {
        let n = self.nodes + self.branches;
        let mut a = vec![vec![Complex::ZERO; n + 1]; n];
        for (row, column, value) in &self.matrix {
            a[*row][*column] = a[*row][*column] + *value;
        }
        for (row, value) in &self.rhs {
            a[*row][n] = a[*row][n] + *value;
        }
        for column in 0..n {
            let pivot = (column..n).max_by(|x, y| a[*x][column].abs().total_cmp(&a[*y][column].abs())).unwrap();
            assert!(a[pivot][column].abs() > 1e-300, "singular network at unknown {column}");
            a.swap(column, pivot);
            let pivot_row = a[column].clone();
            for (row, values) in a.iter_mut().enumerate() {
                if row != column {
                    let factor = values[column] / pivot_row[column];
                    if factor != Complex::ZERO {
                        for (value, pivot_value) in values.iter_mut().zip(&pivot_row).skip(column) {
                            *value = *value - factor * *pivot_value;
                        }
                    }
                }
            }
        }
        let x: Vec<Complex> = (0..n).map(|k| a[k][n] / a[k][k]).collect();
        Solution { voltages: x[..self.nodes].to_vec(), currents: x[self.nodes..].to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64) -> Complex {
        Complex::new(x, 0.0)
    }

    fn close(a: Complex, b: Complex, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn a_resistive_divider_gives_the_hand_voltage_and_current() {
        // 10 V, 1 kohm from node 1 to node 2, 3 kohm from node 2 to the reference: V2 = 10 * 3 / 4 = 7.5 V,
        // I = 10 / 4000 = 2.5 mA.
        let mut c = Circuit::new(2, 0.0);
        let source = c.voltage_source(1, 0, r(10.0));
        let upper = c.impedance(1, 2, r(1000.0));
        c.impedance(2, 0, r(3000.0));
        let s = c.solve();
        assert!(close(s.v(2), r(7.5), 1e-12) && close(s.v(1), r(10.0), 1e-12));
        assert!(close(s.i(upper), r(2.5e-3), 1e-15));
        // The source current leaves its plus node through the source: negative while it delivers.
        assert!(close(s.i(source), r(-2.5e-3), 1e-15));
    }

    #[test]
    fn a_series_rlc_at_resonance_is_its_resistance_with_the_capacitor_at_q_times_the_source() {
        // R = 10 ohm, L = 1 uH, C = 1 nF: f0 = 1 / (2 pi sqrt(LC)) = 5.032921 MHz, omega L = 1 / (omega C) =
        // sqrt(L / C) = 31.622777 ohm, so Z = R, I = 1 V / 10 ohm = 0.1 A and V_C = -j I sqrt(L / C) = -j 3.162278 V
        // (Q = 3.162278).
        let (l, cap): (f64, f64) = (1.0e-6, 1.0e-9);
        let omega = 1.0 / (l * cap).sqrt();
        let mut c = Circuit::new(3, omega);
        c.voltage_source(1, 0, r(1.0));
        let resistor = c.impedance(1, 2, r(10.0));
        c.impedance(2, 3, Complex::new(0.0, omega * l));
        c.impedance(3, 0, Complex::new(0.0, -1.0 / (omega * cap)));
        let s = c.solve();
        assert!((omega / (2.0 * std::f64::consts::PI) - 5.032_921e6).abs() < 10.0);
        assert!(close(s.i(resistor), r(0.1), 1e-12));
        assert!(close(s.v(3), Complex::new(0.0, -3.162_277_66), 1e-7));
    }

    #[test]
    fn coupled_branches_give_the_mutual_voltage() {
        // Two branches with j omega M between them, the second open: its voltage is j omega M i1.
        // omega = 2 pi 100 MHz, M = 1 nH, i1 = 1 mA: 6.283185e8 * 1e-9 * 1e-3 = 628.3185 uV.
        let omega = 2.0 * std::f64::consts::PI * 1.0e8;
        let jm = Complex::new(0.0, omega * 1.0e-9);
        let mut c = Circuit::new(2, omega);
        c.coupled(&[(1, 0), (2, 0)], &[vec![Complex::ZERO, jm], vec![jm, Complex::ZERO]]);
        c.current_source(0, 1, r(1.0e-3));
        let s = c.solve();
        assert!(close(s.v(2), Complex::new(0.0, 628.318_5e-6), 1e-10));
    }

    #[test]
    fn a_quarter_wave_line_inverts_the_load_and_a_matched_line_only_delays() {
        // Z0 = 50 ohm, a quarter wave (1 GHz, 0.25 ns) into 100 ohm presents Z0^2 / 100 = 25 ohm: a 1 V source
        // behind 25 ohm sees half of it at the line input.
        let omega = 2.0 * std::f64::consts::PI * 1.0e9;
        let mut c = Circuit::new(3, omega);
        c.voltage_source(1, 0, r(1.0));
        c.impedance(1, 2, r(25.0));
        c.line(2, 3, 50.0, 0.25e-9);
        c.impedance(3, 0, r(100.0));
        assert!(close(c.solve().v(2), r(0.5), 1e-12));
        // Matched at both ends, 2 V behind 50 ohm: the load sees (2 / 2) e^{-j theta} with theta = pi / 4 (0.125 ns).
        let mut c = Circuit::new(3, omega);
        c.voltage_source(1, 0, r(2.0));
        c.impedance(1, 2, r(50.0));
        c.line(2, 3, 50.0, 0.125e-9);
        c.impedance(3, 0, r(50.0));
        let hand = Complex::new(std::f64::consts::FRAC_1_SQRT_2, -std::f64::consts::FRAC_1_SQRT_2);
        assert!(close(c.solve().v(3), hand, 1e-12));
    }
}

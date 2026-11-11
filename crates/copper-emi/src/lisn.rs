//! Line impedance stabilisation network (LISN / V-network) model.
//!
//! One line of the CISPR 16-1-2 50 uH || 50 ohm V-network, as implemented here
//! (single line to reference ground, supply impedance neglected):
//!
//! ```text
//!              L = 50 uH
//!   EUT T o-----UUUUU------+--- mains side
//!         |                |
//!         |               C_m = 1 uF (+ r_s)
//!         |                |
//!         |               GND
//!         |
//!         +--- C_c = 0.1 uF ---+--- P (measurement port)
//!                              |
//!                      R_b = 1 kohm || R_m = 50 ohm (receiver)
//!                              |
//!                             GND
//! ```
//!
//! The device (EUT) terminal `T` sees the inductor branch (50 uH in series with
//! the mains-side decoupling capacitor) in parallel with the measurement
//! branch (coupling capacitor in series with the 1 kohm bleed resistor in
//! parallel with the 50 ohm receiver input). At high frequency the inductor
//! branch opens and the EUT sees `R_b || R_m` = 47.6 ohm, within the standard's
//! impedance tolerance of the nominal 50 ohm.
//!
//! The component values and the topology follow the commonly drawn schematic
//! of the 50 uH V-network; the exact figure (component values, the position of
//! the 1 kohm resistor, the 5 ohm variant) MUST be verified against CISPR
//! 16-1-2 before the model is used to qualify anything. The standard also
//! specifies a tolerance mask for `|Z|` and its phase that this model is not
//! checked against.

use crate::spectrum::Complex;
use std::f64::consts::PI;

/// Parameters of one line of a V-network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VNetwork {
    /// Series inductor toward the mains, H.
    pub inductance: f64,
    /// Mains-side decoupling capacitor, F.
    pub mains_capacitance: f64,
    /// Series resistance of the mains-side capacitor, ohm.
    pub mains_series_resistance: f64,
    /// Coupling capacitor to the measurement port, F.
    pub coupling_capacitance: f64,
    /// Bleed resistor at the measurement port, ohm.
    pub bleed_resistance: f64,
    /// Receiver input resistance, ohm.
    pub measurement_resistance: f64,
}

impl VNetwork {
    /// The 50 uH || 50 ohm V-network as drawn in the module docs.
    pub const fn cispr_50uh() -> Self {
        Self {
            inductance: 50e-6,
            mains_capacitance: 1e-6,
            mains_series_resistance: 0.0,
            coupling_capacitance: 0.1e-6,
            bleed_resistance: 1000.0,
            measurement_resistance: 50.0,
        }
    }

    fn jw(frequency: f64) -> Complex {
        Complex::new(0.0, 2.0 * PI * frequency)
    }

    /// Parallel combination of the bleed and receiver resistances.
    fn port_resistance(&self) -> f64 {
        self.bleed_resistance * self.measurement_resistance / (self.bleed_resistance + self.measurement_resistance)
    }

    fn inductor_branch(&self, f: f64) -> Complex {
        let jw = Self::jw(f);
        jw.scale(self.inductance)
            + Complex::new(self.mains_series_resistance, 0.0)
            + (jw.scale(self.mains_capacitance)).recip()
    }

    fn measurement_branch(&self, f: f64) -> Complex {
        let jw = Self::jw(f);
        (jw.scale(self.coupling_capacitance)).recip() + Complex::new(self.port_resistance(), 0.0)
    }

    /// Impedance seen by the device under test at its terminal, ohm, at
    /// `frequency` (Hz, `> 0`).
    pub fn eut_impedance(&self, frequency: f64) -> Complex {
        let (a, b) = (self.inductor_branch(frequency), self.measurement_branch(frequency));
        (a * b) / (a + b)
    }

    /// Transfer impedance from a noise current source `I_n` injected at the
    /// device terminal (ideal Norton source, high source impedance) to the
    /// voltage across the receiver input: `V_meas = Z_t * I_n`, ohm.
    ///
    /// `Z_t = Z_eut * (R_b || R_m) / Z_meas_branch`.
    pub fn transfer_impedance(&self, frequency: f64) -> Complex {
        self.eut_impedance(frequency).scale(self.port_resistance()) / self.measurement_branch(frequency)
    }

    /// Measured voltage in dB(uV) for a noise-current amplitude `current`
    /// (A, same peak or RMS convention as the result) at `frequency`.
    pub fn measured_dbuv(&self, frequency: f64, current: f64) -> f64 {
        20.0 * (self.transfer_impedance(frequency).abs() * current / 1e-6).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent solution by nodal analysis: unknown voltages at T and P for
    /// a 1 A source at T.
    fn nodal(net: &VNetwork, f: f64) -> (Complex, Complex) {
        let jw = Complex::new(0.0, 2.0 * PI * f);
        let y_l = (jw.scale(net.inductance)
            + Complex::new(net.mains_series_resistance, 0.0)
            + jw.scale(net.mains_capacitance).recip())
        .recip();
        let y_c = jw.scale(net.coupling_capacitance);
        let y_p = Complex::new(1.0 / net.bleed_resistance + 1.0 / net.measurement_resistance, 0.0);
        // [ y_l + y_c, -y_c ] [vt]   [1]
        // [ -y_c, y_c + y_p ] [vp] = [0]
        let (a, b, c, d) = (y_l + y_c, -y_c, -y_c, y_c + y_p);
        let det = a * d - b * c;
        let vt = d / det;
        let vp = (-c) / det;
        (vt, vp)
    }

    #[test]
    fn matches_independent_nodal_solution() {
        let net = VNetwork::cispr_50uh();
        for f in [9e3, 150e3, 500e3, 2e6, 30e6, 300e6] {
            let (vt, vp) = nodal(&net, f);
            let z = net.eut_impedance(f);
            let zt = net.transfer_impedance(f);
            assert!((z - vt).abs() < 1e-9 * vt.abs(), "Z at {f}");
            assert!((zt - vp).abs() < 1e-9 * vp.abs().max(1e-12), "Zt at {f}");
        }
    }

    #[test]
    fn asymptotes() {
        let net = VNetwork::cispr_50uh();
        let rp = 50.0 * 1000.0 / 1050.0;
        for f in [30e6, 100e6, 1e9] {
            let z = net.eut_impedance(f);
            assert!((z.abs() - rp).abs() < 0.01 * rp, "|Z| at {f} = {}", z.abs());
            assert!((net.transfer_impedance(f).abs() - rp).abs() < 0.01 * rp);
        }
        // Mid band stays within +-20 % of the nominal 50 ohm.
        for f in [1e6, 3e6, 10e6, 30e6] {
            let z = net.eut_impedance(f).abs();
            assert!((40.0..60.0).contains(&z), "{f}: {z}");
        }
        // 150 kHz hand value (inductor branch j46 ohm, measurement branch
        // 47.6 - j10.6 ohm): Z = 28.6 + j24.7, |Z| = 37.8 ohm. This model is
        // about 24 % below the nominal 50 ohm at the bottom of the band.
        let z150 = net.eut_impedance(150e3);
        assert!((z150.abs() - 37.8).abs() < 0.3, "{}", z150.abs());
    }

    #[test]
    fn measured_voltage_in_dbuv() {
        let net = VNetwork::cispr_50uh();
        // 1 mA at 10 MHz: |Zt| ~ 47 ohm -> ~ 93.4 dBuV.
        let db = net.measured_dbuv(10e6, 1e-3);
        let expect = 20.0 * (net.transfer_impedance(10e6).abs() * 1e-3 / 1e-6).log10();
        assert!((db - expect).abs() < 1e-12);
        assert!((db - 93.4).abs() < 0.5, "{db}");
    }
}

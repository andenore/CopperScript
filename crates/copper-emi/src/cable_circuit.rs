//! The cable's circuit: small closed forms of the definitions in section 17 of `docs/emi-noise-analysis.md`
//! (`docs/emi-signal-cable-plan.md` P4a).
//!
//! Definition and test fixtures only. Nothing here is called by `emi-check` and nothing is reported from it; every
//! function is compared with an independent nodal solution of the network it stands for in
//! `tests/cable_circuit_fixtures.rs`. None of it is correlated with a measurement.
//!
//! * **Source and load** (fixture 3b, 2): a Thevenin source `V_oc` with `Z_source` into a series impedance and a load.
//! * **Incident wave to connector voltage** (fixtures 3a, 3c, 6): the cascade's `T = 2 Z_ref / (2 Z_ref + Z_cmc)` is
//!   [`crate::mode_conversion::common_mode_transfer`]; the voltage at the far end of a cable is given here both as the
//!   forward-only first arrival and as the steady state of the series circuit, and they agree only for a matched
//!   cable.
//! * **Driven loop current** (fixture 8): the current of loop A from a loaded pin voltage or from a Thevenin source,
//!   never a mixture.
//! * **Ground-noise transfer** (fixture 8): `V_B = sum over k of Z_Bk I_k`, each conductor coupled to loop B with its
//!   own signed transfer before the currents are summed.

use crate::mode_conversion::common_mode_transfer;
use crate::spectrum::Complex;

/// What a cable presents at the connector in the common mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CableLoad {
    /// A common-mode impedance `Z_cable` (ohms, complex); `Z_cable = Z_ref` is the matched cable.
    Impedance(Complex),
    /// No current path at the far end.
    Open,
}

/// The current of a Thevenin source `V_oc` with `Z_source` through `Z_series` into `Z_load`:
/// `I = V_oc / (Z_source + Z_series + Z_load)` (P4a fixture 3b).
pub fn thevenin_current(v_oc: Complex, z_source: Complex, z_series: Complex, z_load: Complex) -> Complex {
    v_oc / (z_source + z_series + z_load)
}

/// The load voltage of the same series circuit: `V_oc Z_load / (Z_source + Z_series + Z_load)` (P4a fixtures 2 and 3b).
pub fn thevenin_load_voltage(v_oc: Complex, z_source: Complex, z_series: Complex, z_load: Complex) -> Complex {
    v_oc * z_load / (z_source + z_series + z_load)
}

/// The first-arrival voltage at the far end of a cable behind a series common-mode impedance, for the incident
/// common-mode wave `v_incident`: the cascade's transmission `T = 2 Z_ref / (2 Z_ref + Z_cmc)` times the forward-only
/// load multiplier `2 Z_cable / (Z_ref + Z_cable)` (2 for an open cable).
///
/// This is the first wavefront before any reflection returns. It is **not** a steady-state voltage and equals one only
/// for the matched cable (`Z_cable = Z_ref`, multiplier 1); see [`steady_state_voltage`] and P4a fixture 6.
pub fn first_arrival_voltage(v_incident: Complex, z_cmc: Complex, z_ref: f64, load: CableLoad) -> Complex {
    let multiplier = match load {
        CableLoad::Impedance(z) => Complex::new(2.0, 0.0) * z / (Complex::new(z_ref, 0.0) + z),
        CableLoad::Open => Complex::new(2.0, 0.0),
    };
    v_incident * common_mode_transfer(z_cmc, z_ref) * multiplier
}

/// The steady-state voltage across the cable load of the series circuit: a source matched to `Z_ref` whose incident
/// wave is `v_incident` (open-circuit voltage `2 v_incident`, source impedance `Z_ref`), the series `Z_cmc` and the
/// load. For an open cable it is the open-circuit voltage `2 v_incident`, whatever the choke.
pub fn steady_state_voltage(v_incident: Complex, z_cmc: Complex, z_ref: f64, load: CableLoad) -> Complex {
    let v_oc = v_incident.scale(2.0);
    match load {
        CableLoad::Impedance(z) => thevenin_load_voltage(v_oc, Complex::new(z_ref, 0.0), z_cmc, z),
        CableLoad::Open => v_oc,
    }
}

/// The current of driven loop A from a **loaded pin voltage**: `I_A = V_pin / Z_A`. The source impedance does not
/// appear, its loading is already in `V_pin` (P4a, driven loop A).
pub fn loaded_pin_current(v_pin: Complex, z_a: Complex) -> Complex {
    v_pin / z_a
}

/// The pin voltage of an explicit **Thevenin source** into loop A: `V_pin = V_oc Z_A / (Z_source + Z_A)`.
pub fn thevenin_pin_voltage(v_oc: Complex, z_source: Complex, z_a: Complex) -> Complex {
    thevenin_load_voltage(v_oc, z_source, Complex::ZERO, z_a)
}

/// The current of driven loop A from an explicit Thevenin source: `I_A = V_oc / (Z_source + Z_A)`.
pub fn thevenin_loop_current(v_oc: Complex, z_source: Complex, z_a: Complex) -> Complex {
    thevenin_current(v_oc, z_source, Complex::ZERO, z_a)
}

/// The signed transfer of driven conductor `k` to observation loop B: `Z_Bk = j omega M_Bk + R_shared`, with `M_Bk` the
/// mutual inductance of conductor `k`'s loop and loop B (sign included) and `R_shared` the resistance of the copper the
/// two loops share (P4a, transfer per conductor).
pub fn transfer_impedance(omega: f64, mutual: f64, r_shared: f64) -> Complex {
    Complex::new(r_shared, omega * mutual)
}

/// The ground-noise voltage `V_B = sum over k of Z_Bk I_k`: each conductor's current is coupled with its own transfer
/// and only then summed (`(Z_Bk, I_k)` per conductor). The currents are never added before the coupling.
pub fn ground_noise_voltage(contributions: &[(Complex, Complex)]) -> Complex {
    contributions.iter().fold(Complex::ZERO, |sum, (z, i)| sum + *z * *i)
}

/// The same voltage of a pair in modal terms,
/// `V_B = (Z_Bp + Z_Bn) I_cm + (Z_Bp - Z_Bn) I_dm`, with `I_cm = (I_p + I_n) / 2` and `I_dm = (I_p - I_n) / 2`.
pub fn ground_noise_voltage_modal(z_bp: Complex, z_bn: Complex, i_cm: Complex, i_dm: Complex) -> Complex {
    (z_bp + z_bn) * i_cm + (z_bp - z_bn) * i_dm
}

/// The pair's modal currents from its leg currents: `(I_cm, I_dm) = ((I_p + I_n) / 2, (I_p - I_n) / 2)`.
pub fn modal_currents(i_p: Complex, i_n: Complex) -> (Complex, Complex) {
    ((i_p + i_n).scale(0.5), (i_p - i_n).scale(0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64) -> Complex {
        Complex::new(x, 0.0)
    }

    #[test]
    fn the_series_circuit_divides_once() {
        // 2 V through 50 + 900 + 50 ohm: I = 2 / 1000 = 2 mA, and the 50 ohm load sees 0.1 V.
        let i = thevenin_current(r(2.0), r(50.0), r(900.0), r(50.0));
        assert!((i.re - 2.0e-3).abs() < 1e-15);
        assert!((thevenin_load_voltage(r(2.0), r(50.0), r(900.0), r(50.0)).re - 0.1).abs() < 1e-15);
    }

    #[test]
    fn a_loaded_pin_voltage_and_the_source_it_implies_give_one_current() {
        // 1 V on a 50 ohm loop is 20 mA; 2 V open circuit behind 50 ohm implies the same 1 V pin and 20 mA.
        assert!((loaded_pin_current(r(1.0), r(50.0)).re - 0.02).abs() < 1e-15);
        assert!((thevenin_pin_voltage(r(2.0), r(50.0), r(50.0)).re - 1.0).abs() < 1e-15);
        assert!((thevenin_loop_current(r(2.0), r(50.0), r(50.0)).re - 0.02).abs() < 1e-15);
    }

    #[test]
    fn the_first_arrival_and_the_steady_state_of_a_matched_cable_agree() {
        // 1 V incident in 50 ohm, 900 ohm choke, matched 50 ohm cable: 2 * 50 / 1000 = 0.1 V either way.
        let load = CableLoad::Impedance(r(50.0));
        assert!((first_arrival_voltage(r(1.0), r(900.0), 50.0, load).re - 0.1).abs() < 1e-15);
        assert!((steady_state_voltage(r(1.0), r(900.0), 50.0, load).re - 0.1).abs() < 1e-15);
    }

    #[test]
    fn the_ground_noise_voltage_couples_each_conductor_before_summing() {
        // omega = 1 rad/s keeps the numbers plain: M_Bp = 2, M_Bn = 1 (in H), +1 and -1 (in A): V_B = j (2 - 1) = j.
        let zp = transfer_impedance(1.0, 2.0, 0.0);
        let zn = transfer_impedance(1.0, 1.0, 0.0);
        let v = ground_noise_voltage(&[(zp, r(1.0)), (zn, r(-1.0))]);
        assert_eq!(v, Complex::new(0.0, 1.0));
        let (cm, dm) = modal_currents(r(1.0), r(-1.0));
        assert_eq!(ground_noise_voltage_modal(zp, zn, cm, dm), v);
    }
}

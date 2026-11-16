//! The fixtures of `docs/emi-signal-cable-plan.md` P4a (section 17 of `docs/emi-noise-analysis.md`): the cable's circuit
//! solved by an independent nodal solver (`support/nodal.rs`, sharing no code with the estimators) and compared with the
//! hand calculation of the plan and, where the plan asks for it, with the closed form of
//! `copper_emi::cable_circuit` or `copper_emi::mode_conversion`.
//!
//! Definition and test fixtures only: nothing here is reported by `emi-check`, and none of it is correlated with a
//! measurement. The numbering is the plan's: 1 return-conductor cancellation, 2 source and load, 3 choke traversal in
//! both representations, 4 simultaneous imbalance, 5 choke in the signal lines only, 6 cable termination, 7 choke
//! position, 8 ground-noise transfer.

#[path = "support/nodal.rs"]
mod nodal;

use std::f64::consts::PI;

use copper_emi::cable_circuit::{
    CableLoad, first_arrival_voltage, ground_noise_voltage, ground_noise_voltage_modal, loaded_pin_current,
    modal_currents, steady_state_voltage, thevenin_current, thevenin_load_voltage, thevenin_loop_current,
    thevenin_pin_voltage, transfer_impedance,
};
use copper_emi::inductance::mutual_filaments;
use copper_emi::mode_conversion::{Element, Series, combined_ratio, common_mode_transfer, skew_ratio_vs_output, trace};
use copper_emi::spectrum::Complex;
use nodal::Circuit;

fn r(x: f64) -> Complex {
    Complex::new(x, 0.0)
}

fn close(a: Complex, b: Complex, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

fn near(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

fn omega(frequency: f64) -> f64 {
    2.0 * PI * frequency
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 1: return-conductor cancellation.
// ---------------------------------------------------------------------------------------------------------------------

/// The pair (p, n) with a common-mode drive, an optional return conductor g inside the same cable and an optional
/// path from the cable's far end to the board reference through the environment.
///
/// Network (node 0 is the board reference): 1 V source (node 1) with `Z_source = 10 ohm` to the connector node 2, the
/// two pair conductors p and n of 50 ohm each from node 2 to the far end, node 3; the return conductor g of 5 ohm from
/// node 3 back to node 0 (when present); `Z_env = 500 ohm` from node 3 to node 0 (when present). Returns the currents
/// `(I_p, I_n, I_g)` into the cable at the connector (`I_g` is the current *returning* on g, so it enters the cable
/// as `-I_g`) and the net current of the bundle `I_p + I_n - I_g`.
fn bundle(with_return: bool, with_environment: bool) -> (Complex, Complex, Complex, Complex) {
    let mut c = Circuit::new(3, omega(100.0e6));
    c.voltage_source(1, 0, r(1.0));
    c.impedance(1, 2, r(10.0));
    let p = c.impedance(2, 3, r(50.0));
    let n = c.impedance(2, 3, r(50.0));
    let g = if with_return { Some(c.impedance(3, 0, r(5.0))) } else { None };
    if with_environment {
        c.impedance(3, 0, r(500.0));
    }
    let s = c.solve();
    let (ip, i_n) = (s.i(p), s.i(n));
    let ig = g.map_or(Complex::ZERO, |id| s.i(id));
    (ip, i_n, ig, ip + i_n - ig)
}

#[test]
fn fixture_1_a_return_conductor_in_the_cable_cancels_the_bundle_current() {
    // Hand calculation. The pair is two 50 ohm conductors in parallel: 25 ohm. Common-mode loop with g and no
    // environment: 10 + 25 + 5 = 40 ohm, I = 1 / 40 = 25 mA, 12.5 mA on each of p and n, 25 mA back on g.
    // Net current of the bundle at the connector: 12.5 + 12.5 - 25 = 0 (the pair's common-mode current is I_p + I_n =
    // 25 mA; in modal terms I_cm = 12.5 mA per leg).
    let (ip, i_n, ig, net) = bundle(true, false);
    assert!(close(ip, r(12.5e-3), 1e-15) && close(i_n, r(12.5e-3), 1e-15) && close(ig, r(25.0e-3), 1e-15));
    assert!(net.abs() < 1e-15, "{net:?}");
    // The pair's common-mode current is 25 mA, so the net current is zero against a pair current that is not.
    assert!(close(ip + i_n, r(25.0e-3), 1e-15));
}

#[test]
fn fixture_1_without_the_return_conductor_the_bundle_current_is_the_pairs_common_mode_current() {
    // Hand calculation. g removed, the pair's current returns through the environment, Z_env = 500 ohm:
    // 10 + 25 + 500 = 535 ohm, I = 1 / 535 = 1.869159 mA = I_p + I_n; the net current of the bundle is that current.
    let (ip, i_n, ig, net) = bundle(false, true);
    assert!(close(ig, Complex::ZERO, 0.0));
    assert!(close(net, ip + i_n, 1e-18));
    assert!(close(net, r(1.0 / 535.0), 1e-15));
    assert!(near(net.re, 1.869_159e-3, 1e-9));
}

#[test]
fn fixture_1_with_both_the_net_current_is_what_returns_through_the_environment() {
    // Hand calculation. g (5 ohm) and the environment (500 ohm) in parallel: 5 * 500 / 505 = 4.950495 ohm; the loop
    // is 10 + 25 + 4.950495 = 39.950495 ohm, I_p + I_n = 25.030979 mA. The current divides in inverse proportion to
    // the impedances: g takes 500 / 505 of it (24.783147 mA), the environment 5 / 505 (0.247831 mA). The net current
    // of the bundle at the connector is the environment's share, not the pair's common-mode current.
    let (ip, i_n, ig, net) = bundle(true, true);
    let pair = ip + i_n;
    assert!(near(pair.re, 25.030_979e-3, 1e-9));
    assert!(near(ig.re, 24.783_147e-3, 1e-9));
    assert!(near(net.re, 0.247_831_5e-3, 1e-10));
    assert!(close(net, pair.scale(5.0 / 505.0), 1e-15));
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 2: source and load impedance.
// ---------------------------------------------------------------------------------------------------------------------

/// A Thevenin source into a series impedance (a resistor and a coupling capacitor) and a load (a resistor in parallel
/// with a capacitor), solved at `frequency`; returns the load voltage and the series current.
fn divider(
    frequency: f64,
    v_oc: f64,
    z_source: f64,
    series_r: f64,
    series_c: f64,
    load_r: f64,
    load_c: f64,
) -> (Complex, Complex) {
    let w = omega(frequency);
    // Nodes: 1 source, 2 after the source impedance, 3 after the series resistor, 4 the load.
    let mut c = Circuit::new(4, w);
    c.voltage_source(1, 0, r(v_oc));
    c.impedance(1, 2, r(z_source));
    c.impedance(2, 3, r(series_r));
    let cap = c.impedance(3, 4, Complex::new(0.0, -1.0 / (w * series_c)));
    c.impedance(4, 0, r(load_r));
    c.impedance(4, 0, Complex::new(0.0, -1.0 / (w * load_c)));
    let s = c.solve();
    (s.v(4), s.i(cap))
}

#[test]
fn fixture_2_resistive_hand_numbers() {
    // 3.3 V, 22 ohm source, 10 ohm series, 50 ohm load: V = 3.3 * 50 / 82 = 2.012195 V, I = 3.3 / 82 = 40.2439 mA.
    let mut c = Circuit::new(3, 0.0);
    c.voltage_source(1, 0, r(3.3));
    c.impedance(1, 2, r(22.0));
    let series = c.impedance(2, 3, r(10.0));
    c.impedance(3, 0, r(50.0));
    let s = c.solve();
    assert!(near(s.v(3).re, 2.012_195_1, 1e-7) && near(s.i(series).re, 40.243_90e-3, 1e-8));
    let (z_source, z_series, z_load) = (r(22.0), r(10.0), r(50.0));
    assert!(close(thevenin_load_voltage(r(3.3), z_source, z_series, z_load), s.v(3), 1e-12));
    assert!(close(thevenin_current(r(3.3), z_source, z_series, z_load), s.i(series), 1e-15));
}

#[test]
fn fixture_2_complex_hand_number() {
    // 3.3 V, 20 ohm source, a series inductor of +j 100 ohm, 50 ohm load: Z = 70 + j 100, |Z| = 122.0656 ohm,
    // |V_load| = 3.3 * 50 / 122.0656 = 1.351733 V; the phase is -atan(100 / 70) = -55.0 degrees.
    let mut c = Circuit::new(3, 0.0);
    c.voltage_source(1, 0, r(3.3));
    c.impedance(1, 2, r(20.0));
    c.impedance(2, 3, Complex::new(0.0, 100.0));
    c.impedance(3, 0, r(50.0));
    let v = c.solve().v(3);
    assert!(near(v.abs(), 1.351_732_7, 1e-7) && near(v.arg(), -(100.0f64 / 70.0).atan(), 1e-12));
    let closed = thevenin_load_voltage(r(3.3), r(20.0), Complex::new(0.0, 100.0), r(50.0));
    assert!(close(closed, v, 1e-12));
}

#[test]
fn fixture_2_across_frequency_the_solver_and_the_closed_form_agree() {
    // 3.3 V, 22 ohm source, 10 ohm and 100 nF in series, a 50 ohm load in parallel with 5 pF.
    for frequency in [1.0e5, 1.0e6, 1.0e7, 1.0e8, 1.0e9] {
        let w = omega(frequency);
        let z_series = Complex::new(10.0, -1.0 / (w * 100.0e-9));
        let z_load = Complex::new(1.0 / 50.0, w * 5.0e-12).recip();
        let (v, i) = divider(frequency, 3.3, 22.0, 10.0, 100.0e-9, 50.0, 5.0e-12);
        let closed = thevenin_load_voltage(r(3.3), r(22.0), z_series, z_load);
        assert!(close(v, closed, 1e-12 * closed.abs().max(1e-3)), "{frequency}: {v:?} {closed:?}");
        let current = thevenin_current(r(3.3), r(22.0), z_series, z_load);
        assert!(close(i, current, 1e-12 * current.abs()), "{frequency}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 3: choke traversal in both representations.
// ---------------------------------------------------------------------------------------------------------------------

const Z_REF: f64 = 50.0;

/// A matched Thevenin source (`Z_source = Z_ref`, open-circuit voltage `v_oc`) through `n_zero` zero-impedance
/// elements and the series impedance `z_cmc` into a load `Z_ref`; returns the load voltage and the current.
fn matched_chain(v_oc: Complex, z_cmc: Option<Complex>, n_zero: usize) -> (Complex, Complex) {
    let nodes = 3 + n_zero;
    let mut c = Circuit::new(nodes, 0.0);
    c.voltage_source(1, 0, v_oc);
    c.impedance(1, 2, r(Z_REF));
    let mut node = 2;
    for k in 0..n_zero {
        // Alternate the two kinds of zero-impedance element: an impedance of zero and an ideal 0 V source.
        if k % 2 == 0 {
            c.impedance(node, node + 1, Complex::ZERO);
        } else {
            c.voltage_source(node, node + 1, Complex::ZERO);
        }
        node += 1;
    }
    let series = match z_cmc {
        Some(z) => c.impedance(node, node + 1, z),
        None => c.voltage_source(node, node + 1, Complex::ZERO),
    };
    c.impedance(node + 1, 0, r(Z_REF));
    let s = c.solve();
    (s.v(node + 1), s.i(series))
}

#[test]
fn fixture_3a_incident_wave_cases() {
    // T = 2 Z_ref / (2 Z_ref + Z_cmc), Z_ref = 50. A matched source of open-circuit voltage 2 V launches an incident
    // wave of 1 V; the load voltage of the network divided by that 1 V is the transmission.
    let v_oc = r(2.0);
    // No choke: 100 / 100 = 1 exactly; the network gives 2 * 50 / (50 + 50) = 1 V.
    let (v, _) = matched_chain(v_oc, None, 0);
    assert!(close(v, Complex::ONE, 1e-15));
    assert_eq!(common_mode_transfer(Complex::ZERO, Z_REF), Complex::ONE);
    // Z_cmc = 2 Z_ref = 100 ohm: 100 / 200 = 1/2; the network gives 2 * 50 / (50 + 100 + 50) = 0.5 V.
    let (v, _) = matched_chain(v_oc, Some(r(100.0)), 0);
    assert!(close(v, r(0.5), 1e-15) && close(common_mode_transfer(r(100.0), Z_REF), r(0.5), 1e-15));
    // A choke far larger than the reference: Z_cmc = 1e9 ohm: 100 / (100 + 1e9) = 1e-7, and the network gives
    // 2 * 50 / (1e9 + 100) = 1e-7 V: T tends to zero.
    let (v, _) = matched_chain(v_oc, Some(r(1.0e9)), 0);
    assert!(near(v.re, 1.0e-7, 1e-12) && common_mode_transfer(r(1.0e9), Z_REF).abs() < 1.0e-6);
    // A complex choke, Z_cmc = 30 + j 40 (|50 ... |): T = 100 / (130 + j 40), |T| = 100 / sqrt(130^2 + 40^2) =
    // 100 / 136.0147 = 0.735215.
    let z = Complex::new(30.0, 40.0);
    let (v, _) = matched_chain(v_oc, Some(z), 0);
    assert!(near(v.abs(), 0.735_215, 1e-6) && close(v, common_mode_transfer(z, Z_REF), 1e-15));
}

#[test]
fn fixture_3b_thevenin_source_current_and_load_voltage() {
    // V_oc = 3 V, Z_source = 30 ohm, Z_cmc = 20 + j 40 ohm, Z_load = 70 - j 10 ohm: the sum is 120 + j 30 ohm,
    // |sum| = 123.7437 ohm, I = 3 / 123.7437 = 24.2536 mA; |V_load| = 24.2536 mA * |70 - j 10| (70.7107 ohm) =
    // 1.71499 V.
    let (v_oc, z_source, z_cmc, z_load) = (r(3.0), r(30.0), Complex::new(20.0, 40.0), Complex::new(70.0, -10.0));
    let mut c = Circuit::new(3, 0.0);
    c.voltage_source(1, 0, v_oc);
    c.impedance(1, 2, z_source);
    let series = c.impedance(2, 3, z_cmc);
    c.impedance(3, 0, z_load);
    let s = c.solve();
    assert!(near(s.i(series).abs(), 24.2536e-3, 1e-7) && near(s.v(3).abs(), 1.714_99, 1e-5));
    assert!(close(thevenin_current(v_oc, z_source, z_cmc, z_load), s.i(series), 1e-15));
    assert!(close(thevenin_load_voltage(v_oc, z_source, z_cmc, z_load), s.v(3), 1e-12));
}

#[test]
fn fixture_3c_the_two_representations_agree_for_every_choke_including_zero() {
    // For Z_source = Z_load = Z_ref the load voltage of the series circuit is (V_oc / 2) T for every Z_cmc.
    let v_oc = Complex::new(2.0, 0.5);
    for z in [Complex::ZERO, r(100.0), r(900.0), Complex::new(0.0, 250.0), Complex::new(30.0, 40.0), r(1.0e6)] {
        let (v, _) = matched_chain(v_oc, Some(z), 0);
        let wave = v_oc.scale(0.5) * common_mode_transfer(z, Z_REF);
        assert!(close(v, wave, 1e-12 * wave.abs().max(1e-6)), "{z:?}");
        let divider = thevenin_load_voltage(v_oc, r(Z_REF), z, r(Z_REF));
        assert!(close(divider, wave, 1e-12 * wave.abs().max(1e-6)), "{z:?}");
    }
}

#[test]
fn fixture_3d_zero_impedance_elements_change_nothing_in_both_representations() {
    let v_oc = r(2.0);
    for z in [None, Some(r(100.0)), Some(Complex::new(30.0, 40.0))] {
        let (v0, i0) = matched_chain(v_oc, z, 0);
        for n in [1usize, 2, 5] {
            let (v, i) = matched_chain(v_oc, z, n);
            assert!(close(v, v0, 1e-15) && close(i, i0, 1e-15), "{z:?} {n}");
        }
    }
    // The closed forms: a series sum with any number of zeros is the sum without, and the incident transmission of a
    // zero impedance is exactly 1, however many are cascaded and wherever they sit around a real choke.
    let choke = Complex::new(30.0, 40.0);
    let t = common_mode_transfer(choke, Z_REF);
    let ones = (0..7).fold(Complex::ONE, |acc, _| acc * common_mode_transfer(Complex::ZERO, Z_REF));
    assert_eq!(ones, Complex::ONE);
    assert_eq!(ones * t * ones, t);
    let sum_zeros = (0..7).fold(choke, |acc, _| acc + Complex::ZERO);
    assert_eq!(
        thevenin_load_voltage(v_oc, r(Z_REF), sum_zeros, r(Z_REF)),
        thevenin_load_voltage(v_oc, r(Z_REF), choke, r(Z_REF))
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 4: simultaneous imbalance.
// ---------------------------------------------------------------------------------------------------------------------

/// The two-leg network: each leg a matched source (`Z = 50 ohm`) into a lossless 50 ohm line of the leg's delay into a
/// matched 50 ohm load. The sources are `+(1 + a)` on the leg p and `-(1 - a)` on the leg n (open-circuit volts), so the
/// incident leg waves are `(1 + a) / 2` and `-(1 - a) / 2` for `V_diff,in = 1` and the gain of the section is carried
/// by the sources. Returns `(V_p,out, V_n,out)` at the loads.
fn two_legs(frequency: f64, delay_p: f64, delay_n: f64, a: f64) -> (Complex, Complex) {
    let mut c = Circuit::new(6, omega(frequency));
    c.voltage_source(1, 0, r(1.0 + a));
    c.impedance(1, 2, r(50.0));
    c.line(2, 3, 50.0, delay_p);
    c.impedance(3, 0, r(50.0));
    c.voltage_source(4, 0, r(-(1.0 - a)));
    c.impedance(4, 5, r(50.0));
    c.line(5, 6, 50.0, delay_n);
    c.impedance(6, 0, r(50.0));
    let s = c.solve();
    (s.v(3), s.v(6))
}

#[test]
fn fixture_4_skew_and_imbalance_together_against_the_closed_form() {
    // Pure skew, f dt = 0.25 (1 GHz, 0.25 ns): (1/2) sin(pi/4) = 0.353553 against V_diff,in, and (1/2) tan(pi/4) = 0.5
    // against V_diff,out.
    let (f, dt) = (1.0e9, 0.25e-9);
    let (p, n) = two_legs(f, 0.0, dt, 0.0);
    let cm = (p + n).scale(0.5);
    assert!(near(cm.abs(), 0.353_553_39, 1e-7) && near(cm.abs() / (p - n).abs(), 0.5, 1e-9));
    assert!(near(skew_ratio_vs_output(f, dt), 0.5, 1e-12));
    // Imbalance alone: a / 2 = 0.05 whatever the frequency.
    for frequency in [1.0e6, 1.0e8, 3.0e9] {
        let (p, n) = two_legs(frequency, 0.0, 0.0, 0.1);
        assert!(near((p + n).scale(0.5).abs(), 0.05, 1e-12), "{frequency}");
    }
    // Both together, a = sin(pi f dt) = 0.1: (1/2) sqrt(0.01 + 0.01 * 0.99) = (1/2) sqrt(0.0199) = 0.0705337, where
    // the larger of the two terms (0.05) would read 2.99 dB low.
    let skew = 0.1f64.asin() / (PI * f);
    let (p, n) = two_legs(f, 0.0, skew, 0.1);
    let cm = (p + n).scale(0.5).abs();
    assert!(near(cm, 0.070_533_68, 1e-7), "{cm}");
    assert!(near(cm, combined_ratio(f, skew, 0.1), 1e-12));
    assert!(near(20.0 * (cm / 0.05).log10(), 2.99, 0.02));
    // A sweep of cases against the closed form and against the cascade's own leg waves.
    for (frequency, dt, a) in
        [(1.0e9, 0.25e-9, 0.0), (5.0e8, 0.6e-9, 0.05), (1.0e8, 0.2e-9, 0.1), (2.5e9, 0.13e-9, 0.2)]
    {
        let (p, n) = two_legs(frequency, 0.0, dt, a);
        let ratio = (p + n).scale(0.5).abs();
        assert!(near(ratio, combined_ratio(frequency, dt, a), 1e-12), "{frequency} {dt} {a}");
        let element = Element::Section { delay_p: 0.0, delay_n: dt, imbalance: a };
        let state = trace(&[element], frequency)[1];
        assert!(close(p, state.p, 1e-12) && close(n, state.n, 1e-12));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 5: choke in the signal lines only.
// ---------------------------------------------------------------------------------------------------------------------

/// One network with the two drivers side by side: the pair's common mode (the pair taken as one common-mode conductor
/// against the board reference, common-mode reference `Z_c,ref = 25 ohm`: a source of open-circuit voltage 2 V and
/// `Z_source = 25 ohm`, so an incident wave of 1 V, through the series choke into the cable's 25 ohm) and the ground
/// conductor (the ground-noise voltage `V_B = 1 mV` as an ideal source between the board reference and the connector's
/// ground pin, the ground conductor of 1 ohm to the far end, a 10 ohm load there). The signal-line choke sits in the
/// pair's path only; `z_ground_choke` is the wrong case with a choke in the ground conductor as well.
/// Returns the pair's common-mode voltage at the connector and the voltage at the ground conductor's far end.
fn choke_in_signal_lines(z_signal_choke: f64, z_ground_choke: f64) -> (Complex, Complex) {
    let mut c = Circuit::new(6, omega(100.0e6));
    c.voltage_source(1, 0, r(2.0));
    c.impedance(1, 2, r(25.0));
    c.impedance(2, 3, r(z_signal_choke));
    c.impedance(3, 0, r(25.0));
    c.voltage_source(4, 0, r(1.0e-3));
    c.impedance(4, 5, r(1.0));
    c.impedance(5, 6, r(z_ground_choke));
    c.impedance(6, 0, r(10.0));
    let s = c.solve();
    (s.v(3), s.v(6))
}

#[test]
fn fixture_5_a_choke_in_the_signal_lines_attenuates_the_pair_and_not_the_ground_conductor() {
    // Hand calculation. Pair: Z_c,ref = 25 ohm, Z_cmc = 18 Z_c,ref = 450 ohm, T = 50 / 500 = 0.1. Without the choke
    // V = 2 * 25 / (25 + 25) = 1 V; with it 2 * 25 / (25 + 450 + 25) = 0.1 V, 20 dB down.
    // Ground conductor: 1 mV through 1 + 10 ohm: V = 1 mV * 10 / 11 = 0.909091 mV, with or without that choke.
    let (pair_free, ground_free) = choke_in_signal_lines(0.0, 0.0);
    let (pair_choked, ground_choked) = choke_in_signal_lines(450.0, 0.0);
    assert!(close(pair_free, r(1.0), 1e-15) && close(pair_choked, r(0.1), 1e-15));
    assert!(near(20.0 * (pair_choked.abs() / pair_free.abs()).log10(), -20.0, 1e-12));
    assert!(close(pair_choked, thevenin_load_voltage(r(2.0), r(25.0), r(450.0), r(25.0)), 1e-15));
    assert!(close(pair_choked, common_mode_transfer(r(450.0), 25.0), 1e-15));
    assert!(near(ground_free.re, 0.909_091e-3, 1e-9) && close(ground_choked, ground_free, 1e-15));
    let closed = thevenin_load_voltage(r(1.0e-3), r(0.0), r(1.0), r(10.0));
    assert!(close(ground_choked, closed, 1e-15));
    // The contrast: had the choke carried the ground conductor too, the ground-noise voltage would fall to
    // 1 mV * 10 / (1 + 450 + 10) = 21.692 uV.
    let (_, ground_wrong) = choke_in_signal_lines(450.0, 450.0);
    assert!(near(ground_wrong.re, 21.692e-6, 1e-9));
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 6: cable termination.
// ---------------------------------------------------------------------------------------------------------------------

/// 1 V incident in 50 ohm (a matched source of 2 V open circuit), a 900 ohm choke, and the cable load `z_cable`
/// (`None`: open). Returns the steady-state voltage at the cable.
fn terminated(z_cmc: f64, z_cable: Option<f64>) -> Complex {
    let mut c = Circuit::new(3, 0.0);
    c.voltage_source(1, 0, r(2.0));
    c.impedance(1, 2, r(50.0));
    c.impedance(2, 3, r(z_cmc));
    if let Some(z) = z_cable {
        c.impedance(3, 0, r(z));
    }
    c.solve().v(3)
}

#[test]
fn fixture_6_matched_cable_the_cascade_and_the_circuit_agree() {
    // 50 / 900 / 50 ohm: T = 100 / 1000 = 0.1, the load multiplier 2 * 50 / 100 = 1, so the cascade says 0.1 V; the
    // circuit gives 2 * 50 / (50 + 900 + 50) = 0.1 V.
    let load = CableLoad::Impedance(r(50.0));
    let circuit = terminated(900.0, Some(50.0));
    let cascade = first_arrival_voltage(Complex::ONE, r(900.0), 50.0, load);
    assert!(close(circuit, r(0.1), 1e-15) && close(cascade, r(0.1), 1e-15));
    assert!(close(steady_state_voltage(Complex::ONE, r(900.0), 50.0, load), circuit, 1e-15));
}

#[test]
fn fixture_6_high_impedance_cable_the_forward_only_value_is_14_2_db_low() {
    // 50 / 900 / 900 ohm: T = 100 / 1000 = 0.1, multiplier 2 * 900 / 950 = 1.894737, cascade 0.189474 V; circuit
    // 2 * 900 / (50 + 900 + 900) = 1800 / 1850 = 0.972973 V; 20 log10(0.972973 / 0.189474) = 14.21 dB.
    let load = CableLoad::Impedance(r(900.0));
    let circuit = terminated(900.0, Some(900.0));
    let cascade = first_arrival_voltage(Complex::ONE, r(900.0), 50.0, load);
    assert!(near(cascade.re, 0.189_474, 1e-6) && near(circuit.re, 0.972_973, 1e-6));
    let low = 20.0 * (circuit.abs() / cascade.abs()).log10();
    assert!(near(low, 14.21, 0.005), "{low}");
    // The forward-only figure is not a steady-state voltage: this assertion fails if it is ever relabelled as one.
    assert!(circuit.abs() > 5.0 * cascade.abs());
    assert!(close(steady_state_voltage(Complex::ONE, r(900.0), 50.0, load), circuit, 1e-15));
}

#[test]
fn fixture_6_open_cable_the_circuit_gives_twice_the_incident_wave_whatever_the_choke() {
    // Open: no current, so the cable sees the whole open-circuit voltage, 2 V = twice the 1 V incident wave, for every
    // choke; the cascade gives T * 2 = 0.2 V for the 900 ohm choke (20 dB low).
    for choke in [0.0, 100.0, 900.0, 1.0e6] {
        let circuit = terminated(choke, None);
        assert!(close(circuit, r(2.0), 1e-12), "{choke}");
        assert!(close(steady_state_voltage(Complex::ONE, r(choke), 50.0, CableLoad::Open), r(2.0), 1e-15));
    }
    let cascade = first_arrival_voltage(Complex::ONE, r(900.0), 50.0, CableLoad::Open);
    assert!(close(cascade, r(0.2), 1e-15));
    assert!(near(20.0 * (2.0 / cascade.abs()).log10(), 20.0, 1e-12));
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 7: choke position.
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Part {
    /// A skewed section: the leg n delayed by this many seconds (50 ohm lines).
    Skew(f64),
    /// A series common-mode impedance (real, ohms), `V_drop = Z_cmc (I_p + I_n)`, the same drop in both legs.
    Choke(f64),
}

/// The two-leg network of the plan: two matched 50 ohm legs (`+1` and `-1` V open circuit, so `V_diff,in = 1`), the
/// parts in order from the driver, each leg ending in a 50 ohm load. The choke is a coupled pair of series branches
/// `[[Z, Z], [Z, Z]]`: a common-mode current `I_p = I_n` drops `2 Z I` in each leg, a differential current none.
/// Returns `(V_p,out, V_n,out)`.
fn channel(parts: &[Part], frequency: f64) -> (Complex, Complex) {
    let nodes = 4 + 2 * parts.len();
    let mut c = Circuit::new(nodes, omega(frequency));
    c.voltage_source(1, 0, r(1.0));
    c.impedance(1, 2, r(50.0));
    c.voltage_source(3, 0, r(-1.0));
    c.impedance(3, 4, r(50.0));
    let (mut np, mut nn) = (2, 4);
    let mut next = 5;
    for part in parts {
        let (out_p, out_n) = (next, next + 1);
        next += 2;
        match *part {
            Part::Skew(dt) => {
                c.line(np, out_p, 50.0, 0.0);
                c.line(nn, out_n, 50.0, dt);
            }
            Part::Choke(z) => {
                let z = r(z);
                c.coupled(&[(np, out_p), (nn, out_n)], &[vec![z, z], vec![z, z]]);
            }
        }
        (np, nn) = (out_p, out_n);
    }
    c.impedance(np, 0, r(50.0));
    c.impedance(nn, 0, r(50.0));
    let s = c.solve();
    (s.v(np), s.v(nn))
}

#[test]
fn fixture_7_the_choke_position_against_the_cascade() {
    // f dt = 0.25 in total (1 GHz, 0.25 ns); T_cc = 0.1 is Z_cmc = 18 Z_c,ref = 450 ohm with Z_c,ref = 25 ohm (half of
    // a 50 ohm leg).
    let (f, dt, z) = (1.0e9, 0.25e-9, 450.0);
    let section = |t: f64| Element::Section { delay_p: 0.0, delay_n: t, imbalance: 0.0 };
    let cmc = Element::CommonMode { z: Series::Resistor(z), z_ref: 25.0 };
    let cm = |parts: &[Part]| {
        let (p, n) = channel(parts, f);
        (p, n, (p + n).scale(0.5).abs())
    };
    let cascade = |elements: &[Element]| *trace(elements, f).last().unwrap();

    // Without a choke: the unfiltered conversion, (1/2) sin(pi/4) = 0.353553.
    let (_, _, unfiltered) = cm(&[Part::Skew(dt)]);
    assert!(near(unfiltered, 0.353_553_39, 1e-7));

    // Choke after the skewed section: the made common mode goes through T = 0.1: 0.0353553, 20 dB down.
    let (p, n, after) = cm(&[Part::Skew(dt), Part::Choke(z)]);
    assert!(near(after, 0.035_355_34, 1e-8) && near(20.0 * (after / unfiltered).log10(), -20.0, 1e-9));
    let state = cascade(&[section(dt), cmc]);
    assert!(close(p, state.p, 1e-9) && close(n, state.n, 1e-9));

    // Choke before: it sees a purely differential wave and does nothing, the conversion is the unfiltered one.
    let (p, n, before) = cm(&[Part::Choke(z), Part::Skew(dt)]);
    assert!(near(before, unfiltered, 1e-9));
    let state = cascade(&[cmc, section(dt)]);
    assert!(close(p, state.p, 1e-9) && close(n, state.n, 1e-9));

    // The skew split evenly on both sides: by hand V_cm = 0.1375 + j 0.1375 (magnitude 0.19445, -5.19 dB).
    let (p, n, split) = cm(&[Part::Skew(dt / 2.0), Part::Choke(z), Part::Skew(dt / 2.0)]);
    assert!(near(split, 0.194_45, 1e-4), "{split}");
    assert!(close((p + n).scale(0.5), Complex::new(0.1375, 0.1375), 1e-4));
    assert!(near(20.0 * (split / unfiltered).log10(), -5.19, 0.02));
    let state = cascade(&[section(dt / 2.0), cmc, section(dt / 2.0)]);
    assert!(close(p, state.p, 1e-9) && close(n, state.n, 1e-9));
    assert!(after < split && split < before);

    // Identity: a zero-impedance choke and zero-length sections anywhere change nothing.
    let base = channel(&[Part::Skew(dt), Part::Choke(z)], f);
    let padded = channel(
        &[Part::Choke(0.0), Part::Skew(0.0), Part::Skew(dt), Part::Choke(0.0), Part::Choke(z), Part::Skew(0.0)],
        f,
    );
    assert!(close(base.0, padded.0, 1e-12) && close(base.1, padded.1, 1e-12));
}

// ---------------------------------------------------------------------------------------------------------------------
// Fixture 8: ground-noise transfer.
// ---------------------------------------------------------------------------------------------------------------------

type Point = [f64; 3];

/// The mutual inductance of two closed polylines by a midpoint-rule Neumann double integral,
/// `M = (mu0 / 4 pi) sum (dl1 . dl2) / r`, `mu0 / 4 pi = 1e-7`; each edge is cut into `n` pieces. Written here and
/// independent of `copper_emi::inductance` and of the extraction crate.
fn neumann(loop_a: &[Point], loop_b: &[Point], n: usize) -> f64 {
    let pieces = |polyline: &[Point]| -> Vec<(Point, Point)> {
        let mut out = Vec::new();
        for k in 0..polyline.len() {
            let (p, q) = (polyline[k], polyline[(k + 1) % polyline.len()]);
            for j in 0..n {
                let t = (j as f64 + 0.5) / n as f64;
                let mid = [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1]), p[2] + t * (q[2] - p[2])];
                let dl = [(q[0] - p[0]) / n as f64, (q[1] - p[1]) / n as f64, (q[2] - p[2]) / n as f64];
                out.push((mid, dl));
            }
        }
        out
    };
    let (a, b) = (pieces(loop_a), pieces(loop_b));
    let mut sum = 0.0;
    for (ma, da) in &a {
        for (mb, db) in &b {
            let dot = da[0] * db[0] + da[1] * db[1] + da[2] * db[2];
            let d = ((ma[0] - mb[0]).powi(2) + (ma[1] - mb[1]).powi(2) + (ma[2] - mb[2]).powi(2)).sqrt();
            sum += dot / d;
        }
    }
    1.0e-7 * sum
}

/// A rectangular loop in a plane of constant `y`: `x` from 0 to `length`, `z` from `z0` to `z1`, counter-clockwise
/// seen from `-y`.
fn loop_xz(length: f64, z0: f64, z1: f64, y: f64) -> Vec<Point> {
    vec![[0.0, y, z0], [length, y, z0], [length, y, z1], [0.0, y, z1]]
}

/// The same mutual inductance from the finite-filament closed form of the crate, edge by edge.
fn closed_form(loop_a: &[Point], loop_b: &[Point]) -> f64 {
    let mut sum = 0.0;
    for i in 0..loop_a.len() {
        for j in 0..loop_b.len() {
            sum += mutual_filaments(loop_a[i], loop_a[(i + 1) % 4], loop_b[j], loop_b[(j + 1) % 4]);
        }
    }
    sum
}

const LENGTH: f64 = 20.0e-3;
const TRACE_HEIGHT: f64 = 0.2e-3;
const SENSE_HEIGHT: f64 = 5.0e-3;

/// Loop B (the sense path at the stated height over the reference plane, in the plane `y = 0`) and a driven loop A at
/// the lateral offset `y`.
fn loop_b(height: f64) -> Vec<Point> {
    loop_xz(LENGTH, 0.0, height, 0.0)
}

fn loop_a(y: f64) -> Vec<Point> {
    loop_xz(LENGTH, 0.0, TRACE_HEIGHT, y)
}

fn mutual(y: f64, height: f64) -> f64 {
    neumann(&loop_a(y), &loop_b(height), 200)
}

/// The network: driven loops A_k (branches of 50 ohm, driven by the given currents), the observation loop B (an open
/// branch of 10 nH), the mutuals `j omega M_k` between each A_k and B. Returns the open-circuit voltage of B.
fn ground_noise_network(frequency: f64, mutuals: &[f64], currents: &[Complex]) -> Complex {
    let w = omega(frequency);
    let m = mutuals.len();
    let mut c = Circuit::new(m + 1, w);
    let ports: Vec<(usize, usize)> = (1..=m + 1).map(|node| (node, 0)).collect();
    let mut z = vec![vec![Complex::ZERO; m + 1]; m + 1];
    for k in 0..m {
        z[k][k] = r(50.0);
        z[k][m] = Complex::new(0.0, w * mutuals[k]);
        z[m][k] = z[k][m];
    }
    z[m][m] = Complex::new(0.0, w * 10.0e-9);
    c.coupled(&ports, &z);
    for (k, current) in currents.iter().enumerate() {
        c.current_source(0, k + 1, *current);
    }
    c.solve().v(m + 1)
}

#[test]
fn fixture_8_the_numerical_neumann_integral_against_hand_and_closed_form() {
    // Two parallel straight filaments, 20 mm long, 2 mm apart: M = (mu0 / 2 pi) [ l ln(l / d + sqrt(1 + l^2 / d^2))
    // - sqrt(l^2 + d^2) + d ] = 2e-7 [ 0.02 * 2.99823 - 0.0201 + 0.002 ] = 8.3729 nH. A closed polyline of two
    // antiparallel pieces is not a loop, so the open integral is built from the same pieces.
    let n = 400usize;
    let mut sum = 0.0;
    for a in 0..n {
        for b in 0..n {
            let (xa, xb) = ((a as f64 + 0.5) / n as f64 * LENGTH, (b as f64 + 0.5) / n as f64 * LENGTH);
            let d = ((xa - xb).powi(2) + (2.0e-3f64).powi(2)).sqrt();
            sum += (LENGTH / n as f64) * (LENGTH / n as f64) / d;
        }
    }
    let numeric = 1.0e-7 * sum;
    assert!(near(numeric, 8.372_94e-9, 2.0e-12), "{numeric}");
    let closed = mutual_filaments([0.0, 0.0, 0.0], [LENGTH, 0.0, 0.0], [0.0, 2.0e-3, 0.0], [LENGTH, 2.0e-3, 0.0]);
    assert!(near(closed, 8.372_94e-9, 2.0e-12));
}

#[test]
fn fixture_8_loop_mutuals_by_neumann_agree_with_the_closed_form_and_fall_with_distance() {
    let beside = [2.0e-3, 3.0e-3, 6.0e-3, 12.0e-3];
    let mut previous = f64::INFINITY;
    for y in beside {
        let numeric = mutual(y, SENSE_HEIGHT);
        let closed = closed_form(&loop_a(y), &loop_b(SENSE_HEIGHT));
        assert!(numeric > 0.0 && near(numeric, closed, 2.0e-3 * closed), "{y}: {numeric} {closed}");
        assert!(numeric < previous, "A moved away: the transfer falls ({y})");
        previous = numeric;
    }
}

#[test]
fn fixture_8_a_single_loop_beside_b_and_the_same_loop_moved_away() {
    // V_B = j omega M I with I = 1 mA at 100 MHz: the solver, the closed form and j omega M I written out.
    let w = omega(100.0e6);
    let current = r(1.0e-3);
    let mut previous = f64::INFINITY;
    for y in [2.0e-3, 6.0e-3] {
        let m = mutual(y, SENSE_HEIGHT);
        let solved = ground_noise_network(100.0e6, &[m], &[current]);
        let closed = ground_noise_voltage(&[(transfer_impedance(w, m, 0.0), current)]);
        assert!(close(solved, closed, 1e-12 * closed.abs()));
        assert!(close(solved, Complex::new(0.0, w * m * 1.0e-3), 1e-12 * closed.abs()));
        assert!(solved.abs() < previous);
        previous = solved.abs();
    }
}

#[test]
fn fixture_8_symmetric_cancellation_gives_zero() {
    // A pair whose legs are mirror images about loop B (y = +2 mm and y = -2 mm) has equal mutuals; equal and opposite
    // currents give V_B = j omega (M - M) I = 0, while the same leg alone gives a voltage.
    let w = omega(100.0e6);
    let (mp, mn) = (mutual(2.0e-3, SENSE_HEIGHT), mutual(-2.0e-3, SENSE_HEIGHT));
    assert!(near(mp, mn, 1e-9 * mp), "{mp} {mn}");
    let (ip, i_n) = (r(1.0e-3), r(-1.0e-3));
    let solved = ground_noise_network(100.0e6, &[mp, mn], &[ip, i_n]);
    let single = ground_noise_network(100.0e6, &[mp], &[ip]);
    assert!(solved.abs() < 1e-8 * single.abs(), "{solved:?} {single:?}");
    // The summed current is zero as well, but that is not why: with unequal mutuals the voltage stays.
    let closed = ground_noise_voltage(&[(transfer_impedance(w, mp, 0.0), ip), (transfer_impedance(w, mn, 0.0), i_n)]);
    assert!(closed.abs() < 1e-8 * single.abs());
}

#[test]
fn fixture_8_asymmetric_residual_with_the_plans_mutuals() {
    // M_Bp = 2 nH, M_Bn = 1 nH, +1 mA and -1 mA at 100 MHz: V_B = j omega (2 - 1) nH * 1 mA = j 6.283185e8 * 1e-9 *
    // 1e-3 = j 628.3185 uV, with a summed current of zero.
    let w = omega(100.0e6);
    let (mp, mn) = (2.0e-9, 1.0e-9);
    let (ip, i_n) = (r(1.0e-3), r(-1.0e-3));
    let solved = ground_noise_network(100.0e6, &[mp, mn], &[ip, i_n]);
    assert!(close(solved, Complex::new(0.0, 628.318_5e-6), 1e-10), "{solved:?}");
    let zp = transfer_impedance(w, mp, 0.0);
    let zn = transfer_impedance(w, mn, 0.0);
    assert!(close(ground_noise_voltage(&[(zp, ip), (zn, i_n)]), solved, 1e-15));
    assert!(close(ip + i_n, Complex::ZERO, 0.0));
    // Adding the currents first (zero) and coupling the sum would read zero, which is the error this definition excludes.
    assert_eq!(ground_noise_voltage(&[(zp + zn, ip + i_n)]), Complex::ZERO);
    // The same asymmetry from geometry: legs at 2 mm and 3 mm from loop B, the residual follows j omega (M_p - M_n) I.
    let (gp, gn) = (mutual(2.0e-3, SENSE_HEIGHT), mutual(3.0e-3, SENSE_HEIGHT));
    let solved = ground_noise_network(100.0e6, &[gp, gn], &[ip, i_n]);
    assert!(solved.abs() > 0.0 && close(solved, Complex::new(0.0, w * (gp - gn) * 1.0e-3), 1e-12 * solved.abs()));
}

#[test]
fn fixture_8_a_common_mode_current_follows_the_sum_of_the_mutuals() {
    // I_dm = 1 mA, I_cm = 0.5 mA: I_p = 1.5 mA, I_n = -0.5 mA. V_B = j omega [(M_p + M_n) I_cm + (M_p - M_n) I_dm]
    // = j 6.283185e8 [3e-9 * 0.5e-3 + 1e-9 * 1e-3] = j 6.283185e8 * 2.5e-12 = j 1.570796 mV.
    let w = omega(100.0e6);
    let (mp, mn) = (2.0e-9, 1.0e-9);
    let (ip, i_n) = (r(1.5e-3), r(-0.5e-3));
    let solved = ground_noise_network(100.0e6, &[mp, mn], &[ip, i_n]);
    assert!(close(solved, Complex::new(0.0, 1.570_796e-3), 1e-9), "{solved:?}");
    let (cm, dm) = modal_currents(ip, i_n);
    assert!(close(cm, r(0.5e-3), 1e-18) && close(dm, r(1.0e-3), 1e-18));
    let (zp, zn) = (transfer_impedance(w, mp, 0.0), transfer_impedance(w, mn, 0.0));
    assert!(close(ground_noise_voltage_modal(zp, zn, cm, dm), solved, 1e-12));
    assert!(close(ground_noise_voltage(&[(zp, ip), (zn, i_n)]), solved, 1e-12));
    // Shared copper resistance adds to each transfer and is carried the same way: R = 1 mohm on both.
    let (zp, zn) = (transfer_impedance(w, mp, 1.0e-3), transfer_impedance(w, mn, 1.0e-3));
    assert!(close(ground_noise_voltage_modal(zp, zn, cm, dm), ground_noise_voltage(&[(zp, ip), (zn, i_n)]), 1e-12));
}

#[test]
fn fixture_8_a_source_declared_both_ways_gives_one_pin_voltage_current_and_noise() {
    // Matched 50 ohm line, Z_A = 50 ohm. (i) Loaded pin voltage 1 V. (ii) Thevenin: V_oc = 2 V, Z_source = 50 ohm,
    // which implies V_pin = 2 * 50 / 100 = 1 V and I_A = 2 / 100 = 20 mA. Both: I_A = 1 / 50 = 20 mA, not 10 mA.
    // The second pair is V_oc = 1.5 V behind 25 ohm: V_pin = 1.5 * 50 / 75 = 1 V, I_A = 1.5 / 75 = 20 mA.
    let w = omega(100.0e6);
    let m = 1.5e-9;
    for (v_oc, z_source) in [(2.0, 50.0), (1.5, 25.0)] {
        // Network (i): an ideal 1 V at the pin (node 1) driving loop A (node 1 to the reference, 50 ohm, mutual to B).
        let build = |thevenin: bool| {
            let mut c = Circuit::new(if thevenin { 3 } else { 2 }, w);
            if thevenin {
                c.voltage_source(3, 0, r(v_oc));
                c.impedance(3, 1, r(z_source));
            } else {
                c.voltage_source(1, 0, r(1.0));
            }
            let jm = Complex::new(0.0, w * m);
            let a = c.coupled(&[(1, 0), (2, 0)], &[vec![r(50.0), jm], vec![jm, Complex::new(0.0, w * 10.0e-9)]]);
            let s = c.solve();
            (s.v(1), s.i(a), s.v(2))
        };
        let (pin_a, i_a, noise_a) = build(false);
        let (pin_b, i_b, noise_b) = build(true);
        assert!(close(pin_a, r(1.0), 1e-12) && close(pin_b, r(1.0), 1e-12));
        assert!(close(i_a, r(0.02), 1e-15) && close(i_b, r(0.02), 1e-15));
        assert!(close(noise_a, noise_b, 1e-15) && noise_a.abs() > 0.0);
        // The closed forms of both representations agree with the networks and with each other.
        let (v_oc, z_source) = (r(v_oc), r(z_source));
        assert!(close(thevenin_pin_voltage(v_oc, z_source, r(50.0)), r(1.0), 1e-12));
        assert!(close(thevenin_loop_current(v_oc, z_source, r(50.0)), loaded_pin_current(r(1.0), r(50.0)), 1e-15));
        let from_loaded = ground_noise_voltage(&[(transfer_impedance(w, m, 0.0), loaded_pin_current(r(1.0), r(50.0)))]);
        assert!(close(from_loaded, noise_a, 1e-15));
        // The error the definition excludes: dividing the loaded pin voltage by Z_source + Z_A applies the loading
        // twice. For the matched example it is 10 mA instead of 20 mA, 6.02 dB low.
        let twice = loaded_pin_current(r(1.0), z_source + r(50.0));
        if z_source.re == 50.0 {
            assert!(close(twice, r(0.01), 1e-15) && near(20.0 * (twice.abs() / 0.02).log10(), -6.02, 0.01));
        }
        assert!((twice - r(0.02)).abs() > 1e-3);
    }
}

#[test]
fn fixture_8_the_sense_height_band() {
    // The sense path stands for the cable leaving the board; its height is a scenario parameter. V_B of one driven
    // leg for sense heights across a band, from the Neumann integral. It falls as the height grows (the leg's loop is
    // thin and low, so the flux through it comes mostly from the sense loop's far side, which moves away): 136.7,
    // 115.0 and 95.35 uV for 3, 5 and 8 mm, a spread of 3.13 dB. That spread is what the fallback of the plan compares
    // with 6 dB.
    let w = omega(100.0e6);
    let heights = [3.0e-3, 5.0e-3, 8.0e-3];
    let v: Vec<f64> =
        heights.iter().map(|h| ground_noise_network(100.0e6, &[mutual(2.0e-3, *h)], &[r(1.0e-3)]).abs()).collect();
    assert!(v[0] > v[1] && v[1] > v[2], "{v:?}");
    let closed: Vec<f64> = heights
        .iter()
        .map(|h| {
            ground_noise_voltage(&[(transfer_impedance(w, closed_form(&loop_a(2.0e-3), &loop_b(*h)), 0.0), r(1.0e-3))])
                .abs()
        })
        .collect();
    for (a, b) in v.iter().zip(&closed) {
        assert!(near(*a, *b, 2.0e-3 * b));
    }
    let spread = 20.0 * (v[0] / v[2]).log10();
    assert!(near(spread, 3.13, 0.01) && near(spread, 20.0 * (closed[0] / closed[2]).log10(), 0.05), "{spread}");
}

//! Noise analysis and Monte Carlo sampling against closed-form results.
//!
//! Every expected value comes from the circuit equations or the statistics
//! written out in the comments, not from another run of the simulator.

use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE};
use copper_sim::rng::Rng;
use copper_sim::{
    Circuit, NodeId, NoiseSourceKind, NoiseSpec, OpampParams, OpampPins, SimError, SourceValue, Sweep, integrate_psd,
    noise, operating_point,
};
use std::f64::consts::PI;

const G: NodeId = NodeId::GROUND;
const T: f64 = DEFAULT_TEMPERATURE;

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    ((a - b) / b).abs() <= tolerance
}

/// An op-amp close enough to ideal at audio frequencies: `A0 = 1e9`,
/// `GBW = 1e15 Hz`, so the loop-gain error of a gain-of-ten stage at 1 kHz is
/// below 1e-8.
fn ideal_opamp() -> OpampParams {
    OpampParams::new(1e9, 1e15)
}

#[test]
fn johnson_noise_of_a_resistor() {
    // A lone resistor to ground: e_n = sqrt(4 k T R), white.
    let r = 10e3;
    let mut c = Circuit::new();
    let o = c.node("out");
    c.resistor("R1", o, G, r).unwrap();
    let sweep = Sweep::Decade { start: 10.0, stop: 100e3, points_per_decade: 5 };
    let res = noise(&c, &sweep, &NoiseSpec::new("out")).unwrap();
    let expected = (4.0 * BOLTZMANN * T * r).sqrt(); // 12.87 nV/rtHz at 27 C
    assert!((expected - 12.87e-9).abs() < 0.01e-9);
    for d in res.output_density() {
        assert!(close(d, expected, 1e-12), "{d} vs {expected}");
    }
    // A hotter resistor: density as sqrt(T).
    let hot = NoiseSpec { temperature: 400.0, ..NoiseSpec::new("out") };
    let res = noise(&c, &sweep, &hot).unwrap();
    assert!(close(res.output_density()[0], (4.0 * BOLTZMANN * 400.0 * r).sqrt(), 1e-12));
    // Integrated over 10 Hz to 100 kHz: e_n sqrt(99990 Hz).
    let total = integrate_psd(&res.frequencies, &res.output_psd, 10.0, 100e3).unwrap().sqrt();
    assert!(close(total, (4.0 * BOLTZMANN * 400.0 * r * 99_990.0).sqrt(), 1e-12));
}

#[test]
fn divider_density_shares_and_input_referred() {
    // V1 (ideal, a short for noise) -> R1 -> out -> R2 -> ground.
    // Output PSD 4kT (R1 || R2); R1's share R2/(R1+R2), R2's share R1/(R1+R2).
    // Input-referred to V1: the output density divided by R2/(R1+R2).
    let (r1, r2) = (30e3, 10e3);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("V1", i, G, SourceValue::ac(1.0, 0.0)).unwrap();
    c.resistor("R1", i, o, r1).unwrap();
    c.resistor("R2", o, G, r2).unwrap();
    let spec = NoiseSpec { input: Some("V1".into()), ..NoiseSpec::new("out") };
    let res = noise(&c, &Sweep::List(vec![1e3]), &spec).unwrap();
    let parallel = r1 * r2 / (r1 + r2);
    let total = 4.0 * BOLTZMANN * T * parallel;
    assert!(close(res.output_psd[0], total, 1e-12));
    let share = |name: &str| res.output_psd_of(|c| c.element == name)[0] / res.output_psd[0];
    assert!(close(share("R1"), r2 / (r1 + r2), 1e-12));
    assert!(close(share("R2"), r1 / (r1 + r2), 1e-12));
    assert!(res.contributors.iter().all(|c| matches!(c.kind, NoiseSourceKind::Thermal { .. })));
    let input = res.input.as_ref().unwrap();
    assert!(!input.current);
    assert!((input.gain[0].re - r2 / (r1 + r2)).abs() < 1e-12 && input.gain[0].im.abs() < 1e-15);
    let referred = res.input_density().unwrap()[0];
    assert!(close(referred, total.sqrt() * (r1 + r2) / r2, 1e-12));
    // The differential output V(in, out) is the same noise (V(in) is noiseless).
    let spec = NoiseSpec { reference: Some("in".into()), ..NoiseSpec::new("out") };
    let res = noise(&c, &Sweep::List(vec![1e3]), &spec).unwrap();
    assert!(close(res.output_psd[0], total, 1e-12));
}

#[test]
fn rc_lowpass_integrates_to_kt_over_c() {
    // The noise of R filtered by C integrates over all frequencies to kT/C,
    // whatever R is. The band 1 Hz to 1e4 fc misses 4kTR * 1 Hz below
    // (4RC = 4e-6 of the total) and (2/pi)(fc / f_top) = 6.4e-5 above, so
    // the RMS is low by about 3.4e-5.
    let (r, cap) = (1e3, 1e-9);
    let fc = 1.0 / (2.0 * PI * r * cap);
    let mut c = Circuit::new();
    let o = c.node("out");
    c.resistor("R1", o, G, r).unwrap();
    c.capacitor("C1", o, G, cap).unwrap();
    let sweep = Sweep::Decade { start: 1.0, stop: 1e4 * fc, points_per_decade: 50 };
    let res = noise(&c, &sweep, &NoiseSpec::new("out")).unwrap();
    let fs = &res.frequencies;
    // Density: sqrt(4kTR) / |1 + j f/fc|.
    for (k, &f) in fs.iter().enumerate() {
        let exact = (4.0 * BOLTZMANN * T * r / (1.0 + (f / fc).powi(2))).sqrt();
        assert!(close(res.output_density()[k], exact, 1e-9), "{f}");
    }
    let rms = integrate_psd(fs, &res.output_psd, fs[0], fs[fs.len() - 1]).unwrap().sqrt();
    let exact = (BOLTZMANN * T / cap).sqrt(); // 2.035 uV for 1 nF
    assert!(close(rms, exact, 1e-4), "{rms} vs {exact}");
    assert!(rms < exact);
}

#[test]
fn inverting_amplifier_noise_gain() {
    // Inverting stage: source V1 -> RG -> inn, RF from inn to out, RP from
    // inp to ground. Noise gain G = 1 + RF/RG. With an ideal op-amp the
    // output PSD is the sum of
    //   en^2 G^2                    voltage noise
    //   in^2 RF^2                   current noise at inn (flows through RF)
    //   in^2 RP^2 G^2               current noise at inp (across RP)
    //   4kT RF                      RF thermal
    //   4kT RG (RF/RG)^2            RG thermal
    //   4kT RP G^2                  RP thermal
    let (rg, rf, rp) = (1e3, 10e3, 909.0);
    let (en, i_n) = (5e-9, 2e-12);
    let mut c = Circuit::new();
    let (src, inn, inp, out) = (c.node("src"), c.node("inn"), c.node("inp"), c.node("out"));
    c.voltage_source("V1", src, G, SourceValue::ac(1.0, 0.0)).unwrap();
    c.resistor("RG", src, inn, rg).unwrap();
    c.resistor("RF", inn, out, rf).unwrap();
    c.resistor("RP", inp, G, rp).unwrap();
    let mut p = ideal_opamp();
    p.voltage_noise = Some(en);
    p.current_noise = Some(i_n);
    p.output_resistance = 50.0; // inside the loop: noiseless and without effect at this loop gain
    c.opamp("U1", OpampPins::new(inp, inn, out), &p).unwrap();
    let spec = NoiseSpec { input: Some("V1".into()), ..NoiseSpec::new("out") };
    let res = noise(&c, &Sweep::List(vec![1e3]), &spec).unwrap();
    let gain = 1.0 + rf / rg;
    let kt4 = 4.0 * BOLTZMANN * T;
    let terms = [
        ("voltage", en * en * gain * gain),
        ("inn", i_n * i_n * rf * rf),
        ("inp", i_n * i_n * rp * rp * gain * gain),
        ("RF", kt4 * rf),
        ("RG", kt4 * rg * (rf / rg).powi(2)),
        ("RP", kt4 * rp * gain * gain),
    ];
    let total: f64 = terms.iter().map(|(_, x)| x).sum();
    assert!(close(res.output_psd[0], total, 1e-7), "{} vs {total}", res.output_psd[0]);
    for (name, expected) in terms {
        let got = match name {
            "voltage" => res.output_psd_of(|c| matches!(c.kind, NoiseSourceKind::VoltageNoise { .. }))[0],
            "inn" | "inp" => res.output_psd_of(
                |c| matches!(c.kind, NoiseSourceKind::CurrentNoise { terminal, .. } if terminal == name),
            )[0],
            resistor => res.output_psd_of(|c| c.element == resistor)[0],
        };
        assert!(close(got, expected, 1e-7), "{name}: {got} vs {expected}");
    }
    // Only the three resistors and the op-amp's three generators: its internal
    // resistors (gain, pole, output) are noiseless.
    assert_eq!(res.contributors.len(), 6);
    // Input-referred to V1: the signal gain is -RF/RG.
    let g = res.input.as_ref().unwrap().gain[0];
    assert!(close(g.re, -rf / rg, 1e-7) && g.im.abs() < 1e-6);
    assert!(close(res.input_density().unwrap()[0], total.sqrt() * rg / rf, 1e-7));
}

#[test]
fn one_over_f_corner_integration() {
    // Unity follower with en = 10 nV/rtHz and a 100 Hz corner: the output
    // PSD is en^2 (1 + fc/f), whose integral from f1 to f2 is
    // en^2 ((f2 - f1) + fc ln(f2/f1)).
    let (en, fc) = (10e-9, 100.0);
    let mut c = Circuit::new();
    let out = c.node("out");
    let mut p = ideal_opamp();
    p.voltage_noise = Some(en);
    p.voltage_noise_corner = Some(fc);
    c.opamp("U1", OpampPins::new(G, out, out), &p).unwrap();
    // The sum of a white and a 1/f density is no single power law, so the
    // power-law segments of the integration are off near the corner: by
    // 9e-5 at 10 points per decade, below 2e-5 at 50 (the error falls as the
    // square of the step).
    let sweep = Sweep::Decade { start: 0.1, stop: 100e3, points_per_decade: 50 };
    let res = noise(&c, &sweep, &NoiseSpec::new("out")).unwrap();
    let fs = &res.frequencies;
    for (f1, f2) in [(1.0f64, 10e3f64), (0.1, 100e3), (3.0, 70.0), (250.0, 99e3)] {
        let exact = (en * en * ((f2 - f1) + fc * (f2 / f1).ln())).sqrt();
        let got = integrate_psd(fs, &res.output_psd, f1, f2).unwrap().sqrt();
        assert!(close(got, exact, 2e-5), "{f1}..{f2}: {got} vs {exact}");
    }
    let coarse = Sweep::Decade { start: 0.1, stop: 100e3, points_per_decade: 10 };
    let res10 = noise(&c, &coarse, &NoiseSpec::new("out")).unwrap();
    let got = integrate_psd(&res10.frequencies, &res10.output_psd, 1.0, 10e3).unwrap().sqrt();
    assert!(close(got, (en * en * (9999.0 + fc * 1e4f64.ln())).sqrt(), 1.5e-4), "{got}");
    // At the corner the density is sqrt(2) en.
    let k = fs.iter().position(|f| (*f - fc).abs() < 1e-9).unwrap();
    assert!(close(res.output_density()[k], en * 2f64.sqrt(), 1e-7));

    // Current noise with its own corner across a source resistor at inp:
    // PSD = in^2 (1 + fc/f) R^2 + 4kTR, with no voltage noise.
    let (i_n, fci, r) = (1e-12, 1e3, 100e3);
    let mut c = Circuit::new();
    let (inp, out) = (c.node("inp"), c.node("out"));
    c.resistor("RS", inp, G, r).unwrap();
    let mut p = ideal_opamp();
    p.current_noise = Some(i_n);
    p.current_noise_corner = Some(fci);
    c.opamp("U1", OpampPins::new(inp, out, out), &p).unwrap();
    let res = noise(&c, &sweep, &NoiseSpec::new("out")).unwrap();
    let (f1, f2) = (10.0f64, 10e3f64);
    let exact = i_n * i_n * r * r * ((f2 - f1) + fci * (f2 / f1).ln()) + 4.0 * BOLTZMANN * T * r * (f2 - f1);
    let got = integrate_psd(&res.frequencies, &res.output_psd, f1, f2).unwrap();
    // A power (not RMS) comparison, with the corner inside the band: 2.4e-5 at 50 points per decade.
    assert!(close(got, exact, 5e-5), "{got} vs {exact}");
}

#[test]
fn noise_errors_name_the_problem() {
    let mut c = Circuit::new();
    let (a, b) = (c.node("a"), c.node("b"));
    c.resistor("R1", a, G, 1e3).unwrap();
    let _ = b;
    let sweep = Sweep::List(vec![1e3]);
    assert!(matches!(noise(&c, &sweep, &NoiseSpec::new("zz")), Err(SimError::UnknownNode(_))));
    assert!(matches!(noise(&c, &sweep, &NoiseSpec::new("b")), Err(SimError::NotConnected(_))));
    let spec = NoiseSpec { input: Some("R1".into()), ..NoiseSpec::new("a") };
    assert!(matches!(noise(&c, &sweep, &spec), Err(SimError::BadReference { .. })));
    let spec = NoiseSpec { temperature: 0.0, ..NoiseSpec::new("a") };
    assert!(matches!(noise(&c, &sweep, &spec), Err(SimError::InvalidValue { .. })));
    // A source with no path to the output has no input-referred noise.
    let mut c = Circuit::new();
    let (a, s) = (c.node("a"), c.node("s"));
    c.resistor("R1", a, G, 1e3).unwrap();
    c.voltage_source("V1", s, G, SourceValue::ac(1.0, 0.0)).unwrap();
    c.resistor("R2", s, G, 1e3).unwrap();
    let spec = NoiseSpec { input: Some("V1".into()), ..NoiseSpec::new("a") };
    assert!(matches!(noise(&c, &sweep, &spec), Err(SimError::BadReference { .. })));
}

/// Sample mean and standard deviation.
fn statistics(x: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    (mean, var.sqrt())
}

#[test]
fn sampling_statistics_and_determinism() {
    // Uniform on [-1, 1): mean 0, standard deviation 1/sqrt(3). Truncated
    // normal of sigma 1/3 at +-1 (3 sigma): standard deviation 0.98657 / 3
    // (the variance of a standard normal truncated at +-3 is 0.97334).
    let n = 20_000;
    let mut r = Rng::new(42);
    let u: Vec<f64> = (0..n).map(|_| r.symmetric()).collect();
    let (mean, sd) = statistics(&u);
    // Standard errors: 0.577/sqrt(n) = 0.0041 for the mean; 4 standard errors allowed.
    assert!(mean.abs() < 0.017, "{mean}");
    assert!(close(sd, 1.0 / 3f64.sqrt(), 0.02), "{sd}");
    let g: Vec<f64> = (0..n).map(|_| r.truncated_gaussian(1.0 / 3.0, 1.0)).collect();
    let (mean, sd) = statistics(&g);
    assert!(mean.abs() < 0.01, "{mean}");
    assert!(close(sd, 0.97334f64.sqrt() / 3.0, 0.02), "{sd}");
    assert!(g.iter().all(|x| x.abs() <= 1.0));
    // The same seed gives the same draws.
    let mut a = Rng::stream(5, 9);
    let mut b = Rng::stream(5, 9);
    for _ in 0..100 {
        assert_eq!(a.gaussian().to_bits(), b.gaussian().to_bits());
    }
}

#[test]
fn monte_carlo_divider_ratio() {
    // R1 = R2 = 10 kohm, each uniform within +-1 %: the ratio
    // R2 / (R1 + R2) = 0.5 + (d2 - d1) / 4 to first order, with mean 0.5 and
    // standard deviation sqrt(2) (0.01 / sqrt 3) / 4 = 2.041e-3. The corners
    // bound it: R1 low and R2 high give 1.01 / (0.99 + 1.01) = 0.505 exactly,
    // the reverse 0.495.
    let runs = 4000;
    let ratio = |d1: f64, d2: f64| {
        let mut c = Circuit::new();
        let (i, o) = (c.node("in"), c.node("out"));
        c.voltage_source("V1", i, G, SourceValue::dc(1.0)).unwrap();
        c.resistor("R1", i, o, 10e3 * (1.0 + d1)).unwrap();
        c.resistor("R2", o, G, 10e3 * (1.0 + d2)).unwrap();
        operating_point(&c).unwrap().voltage("out").unwrap()
    };
    let sample = |seed: u64| -> Vec<f64> {
        (0..runs)
            .map(|k| {
                let mut r = Rng::stream(seed, k);
                let (d1, d2) = (0.01 * r.symmetric(), 0.01 * r.symmetric());
                ratio(d1, d2)
            })
            .collect()
    };
    let x = sample(1);
    let (mean, sd) = statistics(&x);
    let sigma = 2f64.sqrt() * 0.01 / 3f64.sqrt() / 4.0;
    // Mean within 4 standard errors (sigma / sqrt(runs) = 3.2e-5).
    assert!((mean - 0.5).abs() < 4.0 * sigma / (runs as f64).sqrt(), "{mean}");
    assert!(close(sd, sigma, 0.05), "{sd} vs {sigma}");
    // The extreme corners bound every run: R2 high and R1 low, and the reverse.
    let (high, low) = (ratio(-0.01, 0.01), ratio(0.01, -0.01));
    assert!((high - 1.01 / 2.0).abs() < 1e-12 && (low - 0.99 / 2.0).abs() < 1e-12);
    assert!(x.iter().all(|v| *v <= high && *v >= low));
    // Determinism: the same seed gives bit-identical runs; another seed does not.
    assert!(sample(1).iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert!(sample(2).iter().zip(&x).any(|(a, b)| a.to_bits() != b.to_bits()));
}

#[test]
fn a_weighted_output_is_the_weighted_sum_of_node_voltages() {
    // The divider of `divider_density_shares_and_input_referred` with its two
    // generators, R1 (30 kohm) and R2 (10 kohm), both reaching `out`: the
    // output w V(out) has the density |w| sqrt(4 k T (R1 || R2)); w V(out) -
    // w V(out) has none; and w (V(out) - V(in)) is the reference form.
    use copper_sim::{Complex, NoiseTerm};
    let (r1, r2) = (30e3, 10e3);
    let mut c = Circuit::new();
    let (i, o) = (c.node("in"), c.node("out"));
    c.voltage_source("V1", i, G, SourceValue::ac(1.0, 0.0)).unwrap();
    c.resistor("R1", i, o, r1).unwrap();
    c.resistor("R2", o, G, r2).unwrap();
    let sweep = Sweep::List(vec![1e3, 1e4]);
    let w = Complex::new(0.6, -0.8); // |w| = 1
    let weighted = |terms: Vec<NoiseTerm>, input: bool| {
        let input = input.then(|| "V1".to_owned());
        let spec = NoiseSpec { combination: Some(terms), input, ..NoiseSpec::new("") };
        noise(&c, &sweep, &spec).unwrap()
    };
    let term = |node: &str, reference: Option<&str>, weight: Complex| NoiseTerm {
        node: node.into(),
        reference: reference.map(str::to_owned),
        weights: vec![weight; 2],
    };
    let total = 4.0 * BOLTZMANN * T * r1 * r2 / (r1 + r2);
    let res = weighted(vec![term("out", None, w.scale(3.0))], true);
    for psd in &res.output_psd {
        assert!(close(*psd, 9.0 * total, 1e-12), "{psd}");
    }
    // The input's transfer is weighted too: input-referred noise does not change.
    let plain = noise(&c, &sweep, &NoiseSpec { input: Some("V1".into()), ..NoiseSpec::new("out") }).unwrap();
    assert!(close(res.input_density().unwrap()[0], plain.input_density().unwrap()[0], 1e-12));
    let res = weighted(vec![term("out", None, w), term("out", None, -w)], false);
    assert!(res.output_psd.iter().all(|psd| *psd == 0.0));
    // Such an output has no gain from the input: input-referred noise is refused.
    let spec = NoiseSpec {
        combination: Some(vec![term("out", None, w), term("out", None, -w)]),
        input: Some("V1".into()),
        ..NoiseSpec::new("")
    };
    assert!(matches!(noise(&c, &sweep, &spec), Err(SimError::BadReference { .. })));
    let res = weighted(vec![term("out", Some("in"), Complex::ONE)], false);
    let reference = noise(&c, &sweep, &NoiseSpec { reference: Some("in".into()), ..NoiseSpec::new("out") }).unwrap();
    assert_eq!(res.output_psd, reference.output_psd);
    // One weight per frequency.
    let short = NoiseTerm { weights: vec![Complex::ONE], ..term("out", None, Complex::ONE) };
    let spec = NoiseSpec { combination: Some(vec![short]), ..NoiseSpec::new("") };
    assert!(matches!(noise(&c, &sweep, &spec), Err(SimError::BadSweep(_))));
}

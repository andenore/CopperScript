//! Compatibility checks against real vendor op-amp models.
//!
//! Vendor model files are copyrighted and never part of the repository, so
//! these tests are `#[ignore]`d and read the files from the directory named
//! by `COPPER_SPICE_VENDOR_DIR` (each file at the relative path listed
//! below, as its vendor's archive unpacks). Run them with
//!
//! ```console
//! COPPER_SPICE_VENDOR_DIR=/path/to/models cargo test -p copper-spice --test vendor_models -- --ignored --nocapture
//! ```
//!
//! Each test loads the model, solves the operating point of a follower,
//! measures the open-loop gain (an L/C loop break: DC feedback, AC open),
//! the unity-gain frequency, the input-referred noise at 1 kHz and 10 kHz
//! and the slew rate of a follower step, and prints them. Missing files
//! skip the model with a message.

use std::path::PathBuf;

use copper_sim::{Circuit, NodeId, NoiseSpec, SourceValue, Sweep, TranOptions, Waveform};
use copper_spice::{Dialect, Library};

struct Part {
    name: &'static str,
    file: &'static str,
    subckt: &'static str,
    dialect: Dialect,
    /// The roles of the subcircuit's ports, in order: `+`, `-`, `out`, `vp`, `vn`.
    order: [&'static str; 5],
    supply: f64,
    step: f64,
    /// How long the follower's step response is run, s.
    stop: f64,
}

const PARTS: &[Part] = &[
    Part {
        name: "OPA810",
        file: "x-SBOMB73/opa810_a.lib",
        subckt: "OPA810",
        dialect: Dialect::Pspice,
        order: ["+", "-", "out", "vp", "vn"],
        supply: 5.0,
        step: 2.0,
        stop: 15e-6,
    },
    Part {
        name: "OPA1612",
        file: "x-SBOM396/OPA161x.LIB",
        subckt: "OPA161x",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "OPA2134",
        file: "x-SBOM042/OPAx134.LIB",
        subckt: "OPAx134",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "TL072",
        file: "x-SLOJ067/TL072.301",
        subckt: "TL072",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "LM358",
        file: "x-SNOM268/lmx58_lm2904.lib",
        subckt: "LMX58_LM2904",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 60e-6,
    },
];

fn vendor_dir() -> Option<PathBuf> {
    std::env::var_os("COPPER_SPICE_VENDOR_DIR").map(PathBuf::from)
}

/// The amplifier as `U1` between nodes `inp`, `inn`, `out`, `vp`, `vn` with ±supply.
fn amplifier(lib: &Library, part: &Part, c: &mut Circuit) -> Result<(), String> {
    let nodes: Vec<NodeId> = part
        .order
        .iter()
        .map(|role| match *role {
            "+" => c.node("inp"),
            "-" => c.node("inn"),
            "out" => c.node("out"),
            "vp" => c.node("vp"),
            _ => c.node("vn"),
        })
        .collect();
    let (vp, vn) = (c.node("vp"), c.node("vn"));
    c.voltage_source("VP", vp, NodeId::GROUND, SourceValue::dc(part.supply)).map_err(|e| e.to_string())?;
    c.voltage_source("VN", vn, NodeId::GROUND, SourceValue::dc(-part.supply)).map_err(|e| e.to_string())?;
    lib.instantiate(c, "U1", part.subckt, &nodes, &[]).map(|_| ()).map_err(|e| e.to_string())
}

fn check(part: &Part) {
    let Some(dir) = vendor_dir() else {
        println!("{}: COPPER_SPICE_VENDOR_DIR is not set; skipped", part.name);
        return;
    };
    let path = dir.join(part.file);
    if !path.exists() {
        println!("{}: {} not found; skipped", part.name, path.display());
        return;
    }
    let lib = match Library::load(&path, part.dialect, false) {
        Ok(lib) => lib,
        Err(e) => {
            println!("{}: LOAD FAILED: {e}", part.name);
            return;
        }
    };
    // Open loop: L from out to inn (DC feedback), C from inn to ground (AC ground).
    let mut c = Circuit::with_ground("0");
    if let Err(e) = amplifier(&lib, part, &mut c) {
        println!("{}: INSTANTIATE FAILED: {e}", part.name);
        return;
    }
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    c.voltage_source("VIN", inp, NodeId::GROUND, SourceValue::new(0.0, 1.0, 0.0)).unwrap();
    c.inductor("LFB", out, inn, 1e9).unwrap();
    c.capacitor("CFB", inn, NodeId::GROUND, 1e9).unwrap();
    match copper_sim::operating_point(&c) {
        Ok(op) => println!(
            "{}: op: V(out) = {:.4e} V, I(VP) = {:.4e} A, V(inn) = {:.4e} V",
            part.name,
            op.voltage("out").unwrap(),
            op.current("VP").unwrap(),
            op.voltage("inn").unwrap()
        ),
        Err(e) => {
            println!("{}: OP FAILED: {e}", part.name);
            return;
        }
    }
    let sweep = Sweep::Decade { start: 1.0, stop: 1e9, points_per_decade: 20 };
    match copper_sim::ac(&c, &sweep) {
        Ok(ac) => {
            let v = ac.voltage("out").unwrap();
            let f = ac.frequencies();
            let db = |z: copper_sim::Complex| 20.0 * z.abs().log10();
            let ugf = f.iter().zip(v).find(|(_, z)| z.abs() < 1.0).map(|(f, _)| *f);
            let at = |target: f64| f.iter().zip(v).find(|(f, _)| **f >= target).map(|(_, z)| db(*z));
            println!(
                "{}: ac: A_OL(10 Hz) = {:.1} dB, A_OL(1 kHz) = {:.1} dB, A_OL(100 kHz) = {:.1} dB, unity gain near {:.3e} Hz",
                part.name,
                at(10.0).unwrap_or(f64::NAN),
                at(1e3).unwrap_or(f64::NAN),
                at(1e5).unwrap_or(f64::NAN),
                ugf.unwrap_or(f64::NAN)
            );
        }
        Err(e) => println!("{}: AC FAILED: {e}", part.name),
    }
    // Follower: noise and slew.
    let mut c = Circuit::with_ground("0");
    amplifier(&lib, part, &mut c).unwrap();
    let (inp, inn, out) = (c.node("inp"), c.node("inn"), c.node("out"));
    let vin = c.voltage_source("VIN", inp, NodeId::GROUND, SourceValue::new(0.0, 1.0, 0.0)).unwrap();
    c.resistor("RFB", out, inn, 1e-3).unwrap();
    c.resistor("RL", out, NodeId::GROUND, 10e3).unwrap();
    let noise = copper_sim::noise(
        &c,
        &Sweep::List(vec![10.0, 1e3, 1e4, 1e5]),
        &NoiseSpec { input: Some("VIN".into()), ..NoiseSpec::new("out") },
    );
    match noise {
        Ok(n) => {
            let d = n.input_density().unwrap();
            println!(
                "{}: noise: e_n(10 Hz) = {:.3e}, e_n(1 kHz) = {:.3e}, e_n(10 kHz) = {:.3e}, e_n(100 kHz) = {:.3e} V/rtHz",
                part.name, d[0], d[1], d[2], d[3]
            );
        }
        Err(e) => println!("{}: NOISE FAILED: {e}", part.name),
    }
    let step = part.step;
    c.set_waveform(
        vin,
        Waveform::Pulse {
            initial: -step / 2.0,
            pulsed: step / 2.0,
            delay: 1e-6,
            rise: 1e-9,
            fall: 1e-9,
            width: 20e-6,
            period: None,
            cycles: None,
        },
    )
    .unwrap();
    match copper_sim::transient(&c, &TranOptions { max_step: Some(part.stop / 750.0), ..TranOptions::new(part.stop) }) {
        Ok(t) => {
            let v = t.voltage("out").unwrap();
            let times = t.times();
            let (lo, hi) = (-step / 2.0, step / 2.0);
            let cross =
                |level: f64| times.iter().zip(&v).find(|(tt, x)| **tt > 1e-6 && **x >= level).map(|(tt, _)| *tt);
            let (a, b) = (cross(lo + 0.2 * step), cross(lo + 0.8 * step));
            let sr = match (a, b) {
                (Some(a), Some(b)) if b > a => 0.6 * step / (b - a),
                _ => f64::NAN,
            };
            let _ = hi;
            println!("{}: tran: 20-80 % slew = {:.3e} V/s ({} steps)", part.name, sr, t.stats().accepted_steps);
        }
        Err(e) => println!("{}: TRAN FAILED: {e}", part.name),
    }
}

#[test]
#[ignore = "needs vendor model files in COPPER_SPICE_VENDOR_DIR"]
fn vendor_op_amps() {
    for part in PARTS {
        check(part);
    }
}

/// The operating points of the test circuits TI ships with two models, for
/// comparison with the bias points PSpice wrote into the same archives
/// (decoded outside the test): every node voltage, `NODE value` per line.
#[test]
#[ignore = "needs vendor model files in COPPER_SPICE_VENDOR_DIR"]
fn bias_points_for_pspice_comparison() {
    let Some(dir) = vendor_dir() else { return };
    let cases: [(&str, &str, &str, f64, bool); 2] = [
        ("x-SBOMB73/opa810_a.lib", "OPA810", "opa810", 12.0, false),
        ("x-SNOM268/lmx58_lm2904.lib", "LMX58_LM2904", "lm358", 18.0, true),
    ];
    for (file, subckt, tag, supply, follower) in cases {
        let path = dir.join(file);
        if !path.exists() {
            continue;
        }
        let lib = Library::load(&path, Dialect::Pspice, false).unwrap();
        let mut c = Circuit::with_ground("0");
        let (vcc, vee) = (c.node("VCC"), c.node("VEE"));
        c.voltage_source("V_V1", vcc, NodeId::GROUND, SourceValue::dc(supply)).unwrap();
        c.voltage_source("V_V2", vee, NodeId::GROUND, SourceValue::dc(-supply)).unwrap();
        if follower {
            // X_U1 VIN VOUT VCC VEE VOUT; VIN = 0 at the bias point.
            let (vin, vout) = (c.node("VIN"), c.node("VOUT"));
            c.voltage_source("V_V3", vin, NodeId::GROUND, SourceValue::dc(0.0)).unwrap();
            lib.instantiate(&mut c, "U1", subckt, &[vin, vout, vcc, vee, vout], &[]).unwrap();
        } else {
            // Gain 10: R_R2 0 N01830 100, R_R1 N01830 OUT 900, X_U1 IN+ N01830 OUT VCC VEE.
            let (inp, fb, out) = (c.node("IN+"), c.node("N01830"), c.node("OUT"));
            c.voltage_source("V_V3", inp, NodeId::GROUND, SourceValue::dc(0.0)).unwrap();
            c.resistor("R_R2", NodeId::GROUND, fb, 100.0).unwrap();
            c.resistor("R_R1", fb, out, 900.0).unwrap();
            lib.instantiate(&mut c, "U1", subckt, &[inp, fb, out, vcc, vee], &[]).unwrap();
        }
        let op = copper_sim::operating_point(&c).unwrap();
        for name in c.node_names().iter().skip(1) {
            if let Ok(v) = op.voltage(name) {
                println!("{tag} {name} {v:e}");
            }
        }
    }
}

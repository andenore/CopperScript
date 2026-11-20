//! Compatibility checks against real vendor models.
//!
//! Vendor model files are copyrighted and never part of the repository, so
//! these tests are `#[ignore]`d and read the files from the directory named
//! by `COPPER_SPICE_VENDOR_DIR`: each file at the relative path listed
//! below, as its vendor's archive unpacks, or anywhere below that directory
//! under its file name or as `member-NAME` (the name `copper sim` extracts
//! an archive member under, so the directory may be a `.copper-cache/spice`
//! that `copper models fetch` filled). Run them with
//!
//! ```console
//! COPPER_SPICE_VENDOR_DIR=/path/to/models [COPPER_SPICE_VENDOR_TRAN=1] cargo test -p copper-spice --test vendor_models -- --ignored --nocapture
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
        name: "OPA2277",
        file: "x-SBOM063/OPAx277.LIB",
        subckt: "OPAx277",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 60e-6,
    },
    Part {
        name: "OPA350",
        file: "x-SBOM071/OPAx350.LIB",
        subckt: "OPAx350",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 2.5,
        step: 2.0,
        stop: 15e-6,
    },
    Part {
        name: "OPA1641",
        file: "x-SBOM627/OPA164x.LIB",
        subckt: "OPA164x",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "OPA828",
        file: "x-SBOMAJ0/OPAx828.LIB",
        subckt: "OPAx828",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "OPA1656",
        file: "x-SBOMAW6/OPA1656.LIB",
        subckt: "OPA1656",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 15.0,
        step: 5.0,
        stop: 15e-6,
    },
    Part {
        name: "TLV9002",
        file: "x-SBOMAL2/tlv9002.lib",
        subckt: "TLV9002",
        dialect: Dialect::Pspice,
        order: ["+", "-", "vp", "vn", "out"],
        supply: 2.5,
        step: 2.0,
        stop: 60e-6,
    },
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

/// `file` below `dir`: at its relative path, or by its file name (also as
/// `member-NAME`) anywhere below.
fn find(dir: &std::path::Path, file: &str) -> Option<PathBuf> {
    let direct = dir.join(file);
    if direct.exists() {
        return Some(direct);
    }
    let leaf = file.rsplit('/').next()?.to_ascii_lowercase();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name == leaf || name == format!("member-{leaf}") {
                return Some(path);
            }
        }
    }
    None
}

/// The unity-gain frequency of `v` (log-interpolated between the points around |v| = 1).
fn unity_gain(f: &[f64], v: &[copper_sim::Complex]) -> Option<f64> {
    let k = v.iter().position(|z| z.abs() < 1.0)?;
    if k == 0 {
        return None;
    }
    let (a, b) = (v[k - 1].abs().ln(), v[k].abs().ln());
    let t = a / (a - b);
    Some((f[k - 1].ln() + t * (f[k].ln() - f[k - 1].ln())).exp())
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
    let Some(path) = find(&dir, part.file) else {
        println!("{}: {} not found; skipped", part.name, part.file);
        return;
    };
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
    let sweep = Sweep::Decade { start: 1.0, stop: 1e9, points_per_decade: 200 };
    match copper_sim::ac(&c, &sweep) {
        Ok(ac) => {
            let v = ac.voltage("out").unwrap();
            let f = ac.frequencies();
            let db = |z: copper_sim::Complex| 20.0 * z.abs().log10();
            let ugf = unity_gain(f, v);
            let at = |target: f64| f.iter().zip(v).find(|(f, _)| **f >= target).map(|(_, z)| db(*z));
            // The gain-bandwidth product as data sheets state it at G = 100: 100 times
            // the frequency where |A_OL| falls through 100.
            let hundred: Vec<copper_sim::Complex> = v.iter().map(|z| z.scale(0.01)).collect();
            let gbw100 = unity_gain(f, &hundred).map(|f| 100.0 * f);
            println!("{}: ac: gain-bandwidth at G = 100: {:.4e} Hz", part.name, gbw100.unwrap_or(f64::NAN));
            for target in [10.0, 1e3, 1e5, 1e6, 1e7, 4e7, 6.3e7, 1e8] {
                if let Some((fk, z)) = f.iter().zip(v).find(|(f, _)| **f >= target * (1.0 - 1e-9)) {
                    println!(
                        "{}: ac: A_OL({fk:.4e} Hz) = {:.4e} at {:.2} deg",
                        part.name,
                        z.abs(),
                        z.im.atan2(z.re).to_degrees()
                    );
                }
            }
            let phase =
                ugf.and_then(|u| f.iter().zip(v).find(|(f, _)| **f >= u)).map(|(_, z)| z.im.atan2(z.re).to_degrees());
            println!("{}: ac: phase at unity gain {:.1} deg", part.name, phase.unwrap_or(f64::NAN));
            println!(
                "{}: ac: A_OL(10 Hz) = {:.1} dB, A_OL(1 kHz) = {:.1} dB, A_OL(100 kHz) = {:.1} dB, unity gain at {:.4e} Hz",
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
    // The follower's small-signal -3 dB bandwidth (a data sheet's "bandwidth, G = +1").
    if let Ok(ac) = copper_sim::ac(&c, &Sweep::Decade { start: 1e3, stop: 1e9, points_per_decade: 200 }) {
        let v = ac.voltage("out").unwrap();
        let low = v[0].abs();
        let scaled: Vec<copper_sim::Complex> = v.iter().map(|z| z.scale(std::f64::consts::SQRT_2 / low)).collect();
        let bw = unity_gain(ac.frequencies(), &scaled);
        let peak = v.iter().map(|z| z.abs() / low).fold(0.0, f64::max);
        println!(
            "{}: follower: -3 dB at {:.4e} Hz, peaking {:.2} dB",
            part.name,
            bw.unwrap_or(f64::NAN),
            20.0 * peak.log10()
        );
    }
    if std::env::var_os("COPPER_SPICE_VENDOR_TRAN").is_none() {
        println!("{}: tran: skipped (set COPPER_SPICE_VENDOR_TRAN=1 for the slew test)", part.name);
        return;
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

/// The TL431 shunt reference (reference and cathode tied, 10 mA in; data
/// sheet 2.495 V, dynamic impedance 0.2 ohm typical) and the CSD18563Q5A
/// threshold (gate on drain, 250 uA; data sheet 2 V typical).
#[test]
#[ignore = "needs vendor model files in COPPER_SPICE_VENDOR_DIR"]
fn vendor_references_and_transistors() {
    let Some(dir) = vendor_dir() else { return };
    if let Some(path) = find(&dir, "x-SLVM071/TL431.lib") {
        match Library::load(&path, Dialect::Pspice, false) {
            Err(e) => println!("TL431: LOAD FAILED: {e}"),
            Ok(lib) => {
                // Ports 7 6 11: cathode, anode, reference (the netlist senses V(11, 6)
                // against 2.495 V and the cathode current through 7).
                let mut c = Circuit::with_ground("0");
                let k = c.node("k");
                let ik = c.current_source("IK", NodeId::GROUND, k, SourceValue::new(10e-3, 1.0, 0.0)).unwrap();
                match lib.instantiate(&mut c, "U1", "TL431", &[k, NodeId::GROUND, k], &[]) {
                    Err(e) => println!("TL431: INSTANTIATE FAILED: {e}"),
                    Ok(_) => {
                        match copper_sim::operating_point(&c) {
                            Ok(op) => {
                                let z = copper_sim::ac(&c, &Sweep::List(vec![1e3]))
                                    .map(|ac| ac.voltage("k").unwrap()[0].abs())
                                    .unwrap_or(f64::NAN);
                                println!(
                                    "TL431: op: V(KA) = {:.4} V at 10 mA, |z_KA(1 kHz)| = {z:.4} ohm",
                                    op.voltage("k").unwrap()
                                );
                            }
                            Err(e) => println!("TL431: OP FAILED: {e}"),
                        }
                        // The same current ramped up from zero in a transient run.
                        c.set_waveform(ik, Waveform::Pwl(vec![(0.0, 0.0), (1e-3, 10e-3), (2e-3, 10e-3)])).unwrap();
                        match copper_sim::transient(&c, &TranOptions { max_step: Some(5e-6), ..TranOptions::new(2e-3) })
                        {
                            Ok(t) => {
                                let v = t.voltage("k").unwrap();
                                println!("TL431: tran: V(KA) = {:.4} V after a ramp to 10 mA", v.last().unwrap());
                            }
                            Err(e) => println!("TL431: TRAN FAILED: {e}"),
                        }
                    }
                }
            }
        }
    }
    if let Some(path) = find(&dir, "x-SLPM082/CSD18563Q5A.lib") {
        match Library::load(&path, Dialect::Pspice, false) {
            Err(e) => println!("CSD18563Q5A: LOAD FAILED: {e}"),
            Ok(lib) => {
                let mut c = Circuit::with_ground("0");
                let d = c.node("d");
                c.current_source("ID", NodeId::GROUND, d, SourceValue::dc(250e-6)).unwrap();
                match lib.instantiate(&mut c, "Q1", "CSD18563Q5A", &[d, d, NodeId::GROUND], &[]) {
                    Err(e) => println!("CSD18563Q5A: INSTANTIATE FAILED: {e}"),
                    Ok(_) => match copper_sim::operating_point(&c) {
                        Ok(op) => println!("CSD18563Q5A: V_GS(th) = {:.4} V at 250 uA", op.voltage("d").unwrap()),
                        Err(e) => println!("CSD18563Q5A: OP FAILED: {e}"),
                    },
                }
            }
        }
    }
}

#[test]
#[ignore = "needs vendor model files in COPPER_SPICE_VENDOR_DIR"]
fn vendor_op_amps() {
    // One thread per part; each line is prefixed with its part.
    std::thread::scope(|scope| {
        for part in PARTS {
            scope.spawn(move || check(part));
        }
    });
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
        let Some(path) = find(&dir, file) else { continue };
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

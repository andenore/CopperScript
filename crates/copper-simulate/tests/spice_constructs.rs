//! The netlist constructs and device models of plan phase F7 through the
//! whole `sim spice` chain: a board, a part whose `sim spice` block names a
//! self-written subcircuit, a bench and its measures, each against a value
//! worked out here by hand.

use std::fs;
use std::path::PathBuf;

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

const BOARD: &str = "board B {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.Dut;
    component J_IN: p.Header;
    component R_LOAD: RESISTOR { value = 1Mohm; }
    net IN { J_IN.1; U1.IN; }
    net OUT { U1.OUT; R_LOAD.1; }
    net GND { J_IN.2; R_LOAD.2; }
    supply G { voltage = 0V; net = GND; external = true; }
}
";

const HEADER: &str = "part Header {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}
";

/// A scratch project: the board above, a part `Dut` (pins `IN`, `OUT`) whose
/// `sim spice` block reads `files[0]` (subcircuit `DUT IN OUT`) with `extra`
/// properties, the other model files beside it, and one bench.
struct Project {
    directory: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Project {
        let directory = std::env::temp_dir().join(format!("copper-spice-constructs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("parts")).unwrap();
        fs::create_dir_all(directory.join("benches")).unwrap();
        fs::write(directory.join("board.copper"), BOARD).unwrap();
        fs::write(directory.join("parts").join("header.copper"), HEADER).unwrap();
        Project { directory }
    }

    fn run(&self, files: &[(&str, &str)], extra: &str, bench: &str) -> BenchRun {
        let part = format!(
            "part Dut {{
    category = \"amplifier.opamp\";
    pin IN {{ number = \"1\"; domains = \"analog\"; directions = \"input\"; }}
    pin OUT {{ number = \"2\"; domains = \"analog\"; directions = \"output\"; }}
    sim spice {{ file = \"{}\"; subckt = \"DUT\"; dialect = pspice; terminals = IN, OUT; {extra} }}
}}
",
            files[0].0
        );
        fs::write(self.directory.join("parts").join("dut.copper"), part).unwrap();
        for (name, text) in files {
            fs::write(self.directory.join("parts").join(name), text).unwrap();
        }
        fs::write(self.directory.join("benches").join("bench.copper"), bench).unwrap();
        let board = self.directory.join("board.copper");
        let design = compile_design_file(&board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
        simulate(&board, &design, None).unwrap_or_else(|failure| panic!("{failure}")).remove(0)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn measure(run: &BenchRun, name: &str) -> f64 {
    assert_eq!(run.error, None, "{}", run.name);
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[0].measures[k].value
}

fn model(run: &BenchRun) -> String {
    run.models.iter().find(|m| m.name == "U1").map(|m| m.model.clone()).unwrap_or_default()
}

const OP: &str = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 1V; ac = 1V; }
    analysis op;
    measure out = V(OUT);
}
";

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-30)
}

#[test]
fn params_from_the_sim_spice_block_override_the_defaults() {
    let p = Project::new("params");
    let lib = ".SUBCKT DUT IN OUT PARAMS: GAIN=1 ROUT=1\nE1 X 0 IN 0 {GAIN}\nR1 X OUT {ROUT}\n.ENDS\n";
    let run = p.run(&[("dut.lib", lib)], "params = values { GAIN = 7; rout = 1kohm; };", OP);
    // 7 V behind 1 kohm into the 1 Mohm load.
    assert!(rel(measure(&run, "out"), 7.0 * 1e6 / (1e6 + 1e3)) < 1e-12, "{}", measure(&run, "out"));
    assert!(model(&run).contains("GAIN = 7, ROUT = 1000"), "{}", model(&run));
    let run = p.run(&[("dut.lib", lib)], "", OP);
    assert!(rel(measure(&run, "out"), 1e6 / (1e6 + 1.0)) < 1e-12);
    // A name the subcircuit does not declare is the reader's error.
    let run = p.run(&[("dut.lib", lib)], "params = values { GAINS = 7; };", OP);
    let error = run.error.unwrap_or_default();
    assert!(error.contains("SIM078") && error.contains("GAINS is not a parameter of .SUBCKT DUT"), "{error}");
}

#[test]
fn options_are_noted_and_library_sections_selected() {
    let p = Project::new("sections");
    let top = ".OPTIONS RELTOL=1E-4 ITL4=50 NOPAGE\n.LIB \"corners.lib\" FAST\n.SUBCKT DUT IN OUT\nE1 OUT 0 IN 0 {G}\n.ENDS\n";
    let corners = "* Two corners.\n.LIB SLOW\n.PARAM G=2\n.ENDL SLOW\n.LIB FAST\n.PARAM G=5\n.ENDL FAST\n";
    let run = p.run(&[("dut.lib", top), ("corners.lib", corners)], "", OP);
    assert!(rel(measure(&run, "out"), 5.0) < 1e-12, "{}", measure(&run, "out"));
    let description = model(&run);
    assert!(description.contains(".OPTIONS RELTOL ITL4 NOPAGE ignored"), "{description}");
    // A missing section names itself; an option that changes the circuit is refused.
    let top_missing = top.replace("FAST", "TYPICAL");
    let run = p.run(&[("dut.lib", top_missing.as_str()), ("corners.lib", corners)], "", OP);
    assert!(run.error.unwrap_or_default().contains("the library section TYPICAL"));
    let scaled = top.replace("NOPAGE", "SCALE=1U");
    let run = p.run(&[("dut.lib", scaled.as_str()), ("corners.lib", corners)], "", OP);
    assert!(run.error.unwrap_or_default().contains(".OPTIONS SCALE is not supported"));
}

#[test]
fn ddt_is_an_exact_time_derivative() {
    let p = Project::new("ddt");
    let lib = ".SUBCKT DUT IN OUT\nE1 OUT 0 VALUE = {1U * DDT(V(IN)) + 0.5 * V(IN)}\n.ENDS\n";
    let ac = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 1V; ac = 1V; }
    analysis ac { from = 1kHz; to = 100kHz; points_per_decade = 10; }
    measure gain = mag(at(V(OUT) / V(IN), 10kHz));
    measure phase = phase(at(V(OUT) / V(IN), 10kHz));
}
";
    // 0.5 + j w 1e-6 at 10 kHz.
    let run = p.run(&[("dut.lib", lib)], "", ac);
    let w = 2.0 * std::f64::consts::PI * 1e4;
    assert!(rel(measure(&run, "gain"), (0.25 + (w * 1e-6f64).powi(2)).sqrt()) < 1e-9, "{}", measure(&run, "gain"));
    assert!(rel(measure(&run, "phase"), (w * 1e-6 / 0.5f64).atan().to_degrees()) < 1e-9, "{}", measure(&run, "phase"));
    // A 1 V ramp over 10 us: 1e-6 * 1e5 V/s = 0.1 V on top of half the input.
    let tran = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; waveform = pwl { points = 0s, 0V, 10us, 0V, 20us, 1V, 40us, 1V; }; }
    analysis tran { stop = 40us; max_step = 0.1us; }
    measure dc = at(V(OUT), 5us);
    measure ramp = at(V(OUT), 15us);
    measure after = at(V(OUT), 30us);
}
";
    let run = p.run(&[("dut.lib", lib)], "", tran);
    assert!(measure(&run, "dc").abs() < 1e-12, "{}", measure(&run, "dc"));
    assert!((measure(&run, "ramp") - (0.1 + 0.25)).abs() < 1e-6, "{}", measure(&run, "ramp"));
    assert!((measure(&run, "after") - 0.5).abs() < 1e-6, "{}", measure(&run, "after"));
}

#[test]
fn switches_start_in_their_flagged_state() {
    // In the hysteresis band (0.5 V to 1.5 V) at 1 V, the flag decides.
    let p = Project::new("switch");
    let lib = |flag: &str| {
        format!(
            ".SUBCKT DUT IN OUT\nVCC N1 0 1\nR1 N1 OUT 1K\nS1 OUT 0 IN 0 SMOD {flag}\n.MODEL SMOD SW(VT=1 VH=0.5 RON=10 ROFF=1G)\n.ENDS\n"
        )
    };
    let on = p.run(&[("dut.lib", lib("ON").as_str())], "", OP);
    let load = |r: f64| r * 1e6 / (r + 1e6);
    assert!(rel(measure(&on, "out"), load(10.0) / (1e3 + load(10.0))) < 1e-9, "{}", measure(&on, "out"));
    let off = p.run(&[("dut.lib", lib("OFF").as_str())], "", OP);
    assert!(rel(measure(&off, "out"), load(1e9) / (1e3 + load(1e9))) < 1e-9, "{}", measure(&off, "out"));
    let plain = p.run(&[("dut.lib", lib("").as_str())], "", OP);
    assert_eq!(measure(&plain, "out"), measure(&off, "out"));
}

#[test]
fn mosfet_gate_capacitance_through_sim_spice() {
    // An N-channel device in saturation (drain at 5 V inside the model), its
    // gate driven by the bench: the input admittance is j w (2/3 C_ox + C_gso W).
    let p = Project::new("meyer");
    let lib = |level: u32| {
        format!(
            ".SUBCKT DUT IN OUT\nVD D 0 5\nM1 D IN 0 0 NM W=100U L=4U\nR1 D OUT 1K\n.MODEL NM NMOS(LEVEL={level} VTO=0.8 KP=60U TOX=25N CGSO=0.3N)\n.ENDS\n"
        )
    };
    let ac = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 2V; ac = 1V; }
    analysis ac { from = 100kHz; to = 10MHz; points_per_decade = 10; }
    measure c_in = mag(at(I(U1.IN), 1MHz)) / (2 * 3.14159265358979 * 1MHz);
}
";
    let cox = 3.9 * 8.854214871e-12 / 25e-9 * 100e-6 * 4e-6;
    for level in [1, 3] {
        let run = p.run(&[("dut.lib", lib(level).as_str())], "", ac);
        assert_eq!(run.status(), Status::Pass, "{:?}", run.error);
        let expect = 2.0 / 3.0 * cox + 0.3e-9 * 100e-6;
        assert!(rel(measure(&run, "c_in"), expect) < 1e-6, "level {level}: {} vs {expect}", measure(&run, "c_in"));
    }
}

#[test]
fn lossless_transmission_line_reflects_and_delays() {
    // 150 ohm source, 50 ohm line of 10 ns, the 1 Mohm load nearly open:
    // reflection coefficients 0.5 at the source and (1M - 50) / (1M + 50) at the load.
    let p = Project::new("tline");
    let lib = ".SUBCKT DUT IN OUT\nRS IN A 150\nT1 A 0 OUT 0 Z0=50 TD=10N\n.ENDS\n";
    let (gs, gl) = (0.5, (1e6 - 50.0) / (1e6 + 50.0));
    let a = 50.0 / 200.0;
    let tran = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; waveform = step { initial = 0V; final = 1V; at = 1ns; rise = 0.1ns; }; }
    analysis tran { stop = 60ns; max_step = 0.2ns; }
    measure before = at(V(OUT), 10ns);
    measure first = at(V(OUT), 16ns);
    measure second = at(V(OUT), 41ns);
}
";
    let run = p.run(&[("dut.lib", lib)], "", tran);
    assert!(measure(&run, "before").abs() < 1e-9, "{}", measure(&run, "before"));
    assert!(rel(measure(&run, "first"), a * (1.0 + gl)) < 1e-6, "{}", measure(&run, "first"));
    assert!(rel(measure(&run, "second"), a * (1.0 + gl) * (1.0 + gl * gs)) < 1e-6, "{}", measure(&run, "second"));
    let ac = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 0V; ac = 1V; }
    analysis ac { from = 1MHz; to = 100MHz; points_per_decade = 10; }
    measure gain = mag(at(V(OUT) / V(IN), 10MHz));
}
";
    let run = p.run(&[("dut.lib", lib)], "", ac);
    let wt = 2.0 * std::f64::consts::PI * 1e7 * 1e-8;
    let den = (1.0 - gs * gl * (2.0 * wt).cos(), gs * gl * (2.0 * wt).sin());
    let expect = a * (1.0 + gl) / (den.0 * den.0 + den.1 * den.1).sqrt();
    assert!(rel(measure(&run, "gain"), expect) < 1e-9, "{} vs {expect}", measure(&run, "gain"));
}

#[test]
fn laplace_delay_and_delay_function() {
    let p = Project::new("delays");
    let laplace = ".SUBCKT DUT IN OUT\nE1 OUT 0 LAPLACE {V(IN)} = {EXP(-S*2U)/(1+S*1U)}\n.ENDS\n";
    let function = ".SUBCKT DUT IN OUT\nB1 OUT 0 V={2*DELAY(V(IN), 3U)}\n.ENDS\n";
    let tran = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; waveform = step { initial = 0V; final = 1V; at = 1us; rise = 1ns; }; }
    analysis tran { stop = 8us; max_step = 10ns; }
    measure early = at(V(OUT), 2.9us);
    measure later = at(V(OUT), 5.001us);
}
";
    let ac = "bench T for B {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 0V; ac = 1V; }
    analysis ac { from = 10kHz; to = 1MHz; points_per_decade = 10; }
    measure gain = mag(at(V(OUT) / V(IN), 100kHz));
    measure phase = phase(at(V(OUT) / V(IN), 100kHz));
}
";
    // exp(-2 us s) / (1 + 1 us s): 0 until 3 us, then 1 - exp(-(t - 3 us) / 1 us).
    let run = p.run(&[("dut.lib", laplace)], "", tran);
    assert!(measure(&run, "early").abs() < 1e-9, "{}", measure(&run, "early"));
    let expect = 1.0 - (-(5.001e-6 - 3.001e-6 + 0.5e-9) / 1e-6f64).exp();
    assert!((measure(&run, "later") - expect).abs() < 2e-3, "{} vs {expect}", measure(&run, "later"));
    let run = p.run(&[("dut.lib", laplace)], "", ac);
    let w = 2.0 * std::f64::consts::PI * 1e5;
    assert!(rel(measure(&run, "gain"), 1.0 / (1.0 + (w * 1e-6f64).powi(2)).sqrt()) < 1e-9);
    let phase = (-(w * 1e-6f64).atan() - w * 2e-6).to_degrees();
    assert!((measure(&run, "phase") - phase).abs() < 1e-6, "{} vs {phase}", measure(&run, "phase"));
    // 2 DELAY(V(IN), 3 us): twice the input, 3 us late.
    let run = p.run(&[("dut.lib", function)], "", tran);
    assert!(measure(&run, "early").abs() < 1e-12);
    assert!((measure(&run, "later") - 2.0).abs() < 1e-9, "{}", measure(&run, "later"));
    let run = p.run(&[("dut.lib", function)], "", ac);
    assert!(rel(measure(&run, "gain"), 2.0) < 1e-12);
    let phase = (-w * 3e-6).to_degrees();
    assert!((measure(&run, "phase") - phase).abs() < 1e-6, "{} vs {phase}", measure(&run, "phase"));
}

#[test]
fn value_without_equals_and_nested_braces() {
    // PSpice writes VALUE {expr} without the equals sign, and some vendor files nest braces.
    let p = Project::new("value");
    let lib = ".SUBCKT DUT IN OUT\nE1 OUT 0 VALUE { {3*V(IN)} }\nG1 0 X VALUE {1M*V(IN)}\nR1 X 0 1K\n.ENDS\n";
    let run = p.run(&[("dut.lib", lib)], "", OP);
    assert!(rel(measure(&run, "out"), 3.0) < 1e-12, "{}", measure(&run, "out"));
}

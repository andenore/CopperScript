//! `use layout;` with given parasitics (no extraction): where the elements go in the circuit, against the closed
//! form of the same circuit drawn by hand; the bench without `use layout;` untouched by them; the refusals.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_lang::compiler::{Design, compile_design_file};
use copper_simulate::layout::{LayoutBranch, LayoutCapacitor, LayoutInput, LayoutNet, LayoutParasitics};
use copper_simulate::{ModelChoice, Status, simulate_with_layout};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-layout-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

fn compile(board: &Path) -> Design {
    compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"))
}

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

/// R1 from VIN to VOUT, C1 = 100 nF from VOUT to ground; J1 is the input connector.
const RC: &str = "board RC {
    use library \"tiny\";
    import p \"./parts\";
    component R1: RESISTOR { value = 1kohm; }
    component C1: CAPACITOR { value = 100nF; }
    component J1: p.HEADER;
    net VIN { J1.1; R1.1; }
    net VOUT { R1.2; C1.1; }
    net GND { J1.2; C1.2; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

const WITH: &str = "bench Corner for RC {
    use layout;
    replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
    analysis ac { from = 1kHz; to = 10MHz; points_per_decade = 10; }
    measure at_1khz = mag(at(V(VOUT), 1kHz));
    measure at_1mhz = mag(at(V(VOUT), 1MHz));
    measure at_10mhz = mag(at(V(VOUT), 10MHz));
    measure r1_pin = mag(at(V(R1.2), 1MHz));
}";

const WITHOUT: &str = "bench Plain for RC {
    replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
    analysis ac { from = 1kHz; to = 10MHz; points_per_decade = 10; }
    measure at_1mhz = mag(at(V(VOUT), 1MHz));
}";

/// VOUT as two nodes, R1.2's pad (named, node 0) and C1.1's (node 1), joined by 0.5 ohm and 20 nH; 3 pF from
/// R1.2's pad to ground and 1 pF from VIN to VOUT at C1.1's pad.
fn parasitics() -> LayoutParasitics {
    let single = |net: &str| LayoutNet {
        net: net.to_owned(),
        nodes: vec![net.to_owned()],
        named: 0,
        pins: Vec::new(),
        branches: Vec::new(),
        track_length_m: 0.0,
        unattributed: 0.0,
    };
    LayoutParasitics {
        source: "hand-written".to_owned(),
        nets: vec![
            single("GND"),
            single("VIN"),
            LayoutNet {
                net: "VOUT".to_owned(),
                nodes: vec!["pad R1.2".to_owned(), "pad C1.1".to_owned()],
                named: 0,
                pins: vec![("R1".to_owned(), "2".to_owned(), 0), ("C1".to_owned(), "1".to_owned(), 1)],
                branches: vec![LayoutBranch { a: 0, b: 1, resistance: 0.5, inductance: 20e-9, length_m: 0.02 }],
                track_length_m: 0.02,
                unattributed: 0.0,
            },
        ],
        capacitors: vec![
            LayoutCapacitor { a: (0, 0), b: (2, 0), farad: 3e-12 },
            LayoutCapacitor { a: (1, 0), b: (2, 1), farad: 1e-12 },
        ],
        flags: Vec::new(),
        notes: Vec::new(),
    }
}

/// `V(VOUT)` (R1.2's pad) and `V(C1.1)` by hand, at `f`.
fn closed_form(f: f64) -> (f64, f64) {
    #[derive(Clone, Copy)]
    struct C(f64, f64);
    let add = |a: C, b: C| C(a.0 + b.0, a.1 + b.1);
    let sub = |a: C, b: C| C(a.0 - b.0, a.1 - b.1);
    let mul = |a: C, b: C| C(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    let inv = |a: C| {
        let d = a.0 * a.0 + a.1 * a.1;
        C(a.0 / d, -a.1 / d)
    };
    let w = 2.0 * std::f64::consts::PI * f;
    // Nodal equations of nodes a (R1.2's pad) and b (C1.1's pad), VIN = 1.
    let g1 = C(1e-3, 0.0);
    let y_ag = C(0.0, w * 3e-12);
    let y_ab = inv(C(0.5, w * 20e-9));
    let y_bg = C(0.0, w * 100e-9);
    let y_vb = C(0.0, w * 1e-12);
    // [g1 + y_ag + y_ab, -y_ab; -y_ab, y_ab + y_bg + y_vb] [a; b] = [g1; y_vb]
    let (m00, m01, m11) = (add(add(g1, y_ag), y_ab), C(-y_ab.0, -y_ab.1), add(add(y_ab, y_bg), y_vb));
    let det = sub(mul(m00, m11), mul(m01, m01));
    let a = mul(sub(mul(g1, m11), mul(m01, y_vb)), inv(det));
    let b = mul(sub(mul(m00, y_vb), mul(m01, g1)), inv(det));
    ((a.0 * a.0 + a.1 * a.1).sqrt(), (b.0 * b.0 + b.1 * b.1).sqrt())
}

fn value(run: &copper_simulate::BenchRun, name: &str) -> f64 {
    let k = run.measures.iter().position(|m| m.name == name).unwrap();
    run.steps[0].measures[k].value
}

#[test]
fn the_layout_goes_between_the_pins_and_matches_the_circuit_drawn_by_hand() {
    let board = project(
        "hand",
        &[
            ("board.copper", RC),
            ("parts/header.copper", HEADER),
            ("benches/corner.copper", WITH),
            ("benches/plain.copper", WITHOUT),
        ],
    );
    let design = compile(&board);
    let input = LayoutInput::Ready(Arc::new(parasitics()));
    let runs = simulate_with_layout(&board, &design, None, ModelChoice::Default, &input).unwrap();
    let corner = runs.iter().find(|r| r.name == "Corner").unwrap();
    assert!(corner.error.is_none(), "{:?}", corner.error);
    for (name, f) in [("at_1khz", 1e3), ("at_1mhz", 1e6), ("at_10mhz", 1e7)] {
        let (expected, _) = closed_form(f);
        let got = value(corner, name);
        assert!((got / expected - 1.0).abs() < 1e-9, "{name}: {got} against {expected}");
    }
    // V(R1.2) is the pin's own pad, the named node: the same as V(VOUT).
    assert!((value(corner, "r1_pin") / value(corner, "at_1mhz") - 1.0).abs() < 1e-12);
    // The schematic alone is the plain RC.
    let layout = corner.layout.as_ref().expect("the layout run");
    let k = corner.measures.iter().position(|m| m.name == "at_1mhz").unwrap();
    let schematic = layout.schematic[0][k].value;
    let rc = 1.0 / (1.0 + (2.0 * std::f64::consts::PI * 1e6 * 1e3 * 100e-9_f64).powi(2)).sqrt();
    assert!((schematic / rc - 1.0).abs() < 1e-9, "{schematic} against {rc}");
    assert_eq!(layout.elements, (1, 1, 2));
    assert_eq!(layout.contributors[0].nets, ["GND", "VOUT"]);
    // A bench without use layout is exactly the run without any layout.
    let plain = runs.iter().find(|r| r.name == "Plain").unwrap();
    let alone =
        simulate_with_layout(&board, &design, Some("Plain"), ModelChoice::Default, &LayoutInput::Absent).unwrap();
    assert_eq!(plain, &alone[0]);
    assert!(plain.layout.is_none());
    // The text and the JSON.
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("layout at_1mhz: ") && text.contains(" schematic, "), "{text}");
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert_eq!(json.matches("\"layout\": {").count(), 1, "{json}");
}

#[test]
fn a_bench_that_uses_layout_without_one_is_an_error_not_a_schematic_run() {
    let board =
        project("absent", &[("board.copper", RC), ("parts/header.copper", HEADER), ("benches/corner.copper", WITH)]);
    let design = compile(&board);
    let runs = simulate_with_layout(&board, &design, None, ModelChoice::Default, &LayoutInput::Absent).unwrap();
    assert_eq!(runs[0].status(), Status::Error);
    let error = runs[0].error.as_deref().unwrap();
    assert!(error.starts_with("SIM118: bench Corner says use layout, but no routed board was given"), "{error}");
    assert!(runs[0].steps.is_empty());
    let refused = LayoutInput::Refused("SIM119: stale".to_owned());
    let runs = simulate_with_layout(&board, &design, None, ModelChoice::Default, &refused).unwrap();
    assert_eq!(runs[0].error.as_deref(), Some("SIM119: stale"));
    assert!(copper_simulate::uses_layout(&board, &design, None));
}

#[test]
fn the_spelling_and_the_target_of_use_layout_are_checked() {
    let misspelt = "bench Corner for RC { use layot; analysis op; measure v = V(VOUT); }";
    let board = project(
        "spelling",
        &[("board.copper", RC), ("parts/header.copper", HEADER), ("benches/corner.copper", misspelt)],
    );
    let failure = copper_simulate::simulate(&board, &compile(&board), None).expect_err("PAR011");
    assert!(
        failure.to_string().contains("PAR011") && failure.to_string().contains("'library' or 'layout'"),
        "{failure}"
    );
    // A module on its own has no routed board.
    let module = "module RcStage {
        use library \"tiny\";
        port IN: input;
        port GND: power_in;
        component R1: RESISTOR { value = 1kohm; }
        net VIN { port.IN; R1.1; }
        net GND { port.GND; R1.2; }
    }";
    let top = "board Top {
        use library \"tiny\";
        import p \"./parts\";
        module F: p.RcStage;
        net A { F.IN; }
        net GND { F.GND; }
        supply GROUND { voltage = 0V; net = GND; }
    }";
    let bench = "bench Alone for RcStage {
        use layout;
        supply GROUND { voltage = 0V; net = GND; }
        model SRC: source { out = IN; ref = GND; ac = 1V; }
        analysis op;
        measure v = V(VIN);
    }";
    let board =
        project("module", &[("board.copper", top), ("parts/rc.copper", module), ("benches/alone.copper", bench)]);
    let failure = copper_simulate::simulate(&board, &compile(&board), None).expect_err("SIM120");
    assert!(failure.to_string().contains("SIM120"), "{failure}");
}

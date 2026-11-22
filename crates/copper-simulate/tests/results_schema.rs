//! The shape of the `copperscript-sim/v1` results document is frozen: every
//! field, its place and its JSON types, over benches of every analysis kind
//! (a golden structure, not golden numbers). Each field must also be
//! described in `docs/sim-results-schema.md`.
//!
//! When the shape changes on purpose, the schema version changes with it:
//! set `COPPER_PRINT_SIM_SHAPE=1` to print the new shape.

use std::collections::BTreeMap;
use std::path::PathBuf;

use copper_json::Value;
use copper_lang::compiler::compile_design_file;

fn project(files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

const OPAMP: &str = "part OPAMP {
    category = \"amplifier.opamp\";
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; absolute_min = \"VN-0.3V\"; absolute_max = \"VP+0.3V\"; }
    pin INP { number = \"3\"; domains = \"analog\"; directions = \"input\"; absolute_min = \"VN-0.3V\"; absolute_max = \"VP+0.3V\"; }
    pin VN  { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"6\"; domains = \"analog\"; directions = \"output\"; }
    pin VP  { number = \"7\"; domains = \"power\"; directions = \"input\"; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB;
        output_swing_from_rail = 0.5V; voltage_noise = 10nV/rtHz; }
}";

/// A non-inverting amplifier of gain 1 + R_F / R_G on +-5 V, driven from J2.
const BOARD: &str = "board Amp {
    use library \"tiny\";
    import p \"./parts\";
    param R_F = 10kohm;
    component J2: p.HEADER;
    component U1: p.OPAMP;
    component RF: RESISTOR { value = R_F; tolerance = 1%; }
    component RG: RESISTOR { value = 10kohm; tolerance = 1%; }
    component CL: p.CAP_RATED { value = 100pF; }
    net IN { J2.1; U1.INP; }
    net OUT { U1.OUT; RF.1; CL.1; }
    net FB { U1.INN; RF.2; RG.1; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { J2.2; RG.2; CL.2; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

/// A capacitor part with a voltage rating: its stress is recorded (`steps[].stress`).
const CAP_RATED: &str = "part CAP_RATED {
    category = \"passive.capacitor\";
    rated_voltage = 50V;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}";

const AC: &str = "bench Gain for Amp {
    replace J2 with source { out = J2.1; ref = GND; ac = 1V; }
    step R_F in 10kohm, 30kohm;
    analysis ac { from = 10Hz; to = 1MHz; points_per_decade = 5; }
    let h = V(OUT) / V(IN);
    measure gain_1khz = db(at(h, 1kHz)) { min = 5dB; max = 13dB; }
    measure peak = max(db(h)) { max = 15dB; where R_F <= 10kohm; }
    measure bandwidth = upto(db(h) > 0dB);
    measure never_40db = upto(db(h) > 40dB);
    plot db(h);
    plot phase(h);
    montecarlo { runs = 4; seed = 3; }
}";

const SWEEP: &str = "bench Sweep for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
    analysis dc { sweep = J2; from = -3V; to = 3V; step = 0.5V; }
    measure high = max(V(OUT));
    plot V(OUT);
}";

const TRAN: &str = "bench Sine for Amp {
    replace J2 with source { out = J2.1; ref = GND; waveform = sine { amplitude = 1V; frequency = 1kHz; }; }
    param T_STOP = 2ms;
    step R_F, T_STOP in (10kohm, 2ms), (20kohm, 3ms);
    analysis tran { stop = T_STOP; max_step = 20us; steady = 1kHz; }
    measure distortion = thd(V(OUT), 1kHz) { max = 1%; }
    plot V(OUT);
    plot spectrum(V(OUT), 1kHz);
}";

const NOISE: &str = "bench Noise for Amp {
    replace J2 with source { out = J2.1; ref = GND; ac = 1V; }
    analysis noise { from = 10Hz; to = 100kHz; points_per_decade = 5; output = V(OUT); input = J2; }
    measure density = at(noise(V(OUT)), 1kHz);
    plot noise(V(OUT));
}";

const STABILITY: &str = "bench Stability for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
    analysis amp: loop { from = 100Hz; to = 10MHz; points_per_decade = 5; probe = U1.OUT; }
    measure margin = phase_margin(loop_gain()) { min = 30deg; }
    analysis poles: pz { input = J2; output = V(OUT); }
    measure damping = damping();
}";

const CORNERS: &str = "bench Corners for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 1V; }
    analysis op;
    worstcase;
    measure out = V(OUT) { min = 1.9V; max = 2.1V; }
}";

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Objects whose keys are names (params, variables, stepped fields), not fields.
fn named_keys(path: &str) -> bool {
    [".stepped", ".params", "steps[].analysis", ".x", ".sensitivity"].iter().any(|suffix| path.ends_with(suffix))
        && !path.ends_with("benches[].analysis.stepped")
}

fn walk(value: &Value, path: &str, shape: &mut BTreeMap<String, Vec<&'static str>>) {
    let types = shape.entry(path.to_owned()).or_default();
    let name = type_name(value);
    if !types.contains(&name) {
        types.push(name);
        types.sort_unstable();
    }
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter() {
                let key = if named_keys(path) { "*" } else { key.as_str() };
                walk(child, &format!("{path}.{key}"), shape);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, &format!("{path}[]"), shape);
            }
        }
        _ => {}
    }
}

/// Every path of the document with its JSON types, one per line.
fn shape_of(document: &Value) -> String {
    let mut shape = BTreeMap::new();
    walk(document, "", &mut shape);
    shape
        .iter()
        .filter(|(path, _)| !path.is_empty())
        .map(|(path, types)| format!("{} {}", &path[1..], types.join("|")))
        .collect::<Vec<_>>()
        .join("\n")
}

const SHAPE: &str = include_str!("results_schema.shape");

#[test]
fn the_results_document_has_the_frozen_v1_shape() {
    copper_simulate::timing::enable(true);
    let board = project(&[
        ("board.copper", BOARD),
        ("parts/header.copper", HEADER),
        ("parts/opamp.copper", OPAMP),
        ("parts/cap.copper", CAP_RATED),
        ("benches/ac.copper", AC),
        ("benches/sweep.copper", SWEEP),
        ("benches/tran.copper", TRAN),
        ("benches/noise.copper", NOISE),
        ("benches/stability.copper", STABILITY),
        ("benches/corners.copper", CORNERS),
    ]);
    let design = compile_design_file(&board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    let runs = copper_simulate::simulate(&board, &design, None).unwrap_or_else(|failure| panic!("{failure}"));
    for run in &runs {
        assert_eq!(run.error, None, "{}", run.name);
    }
    // With the source identity `copper sim` writes (`source`, added within v1).
    let source = copper_simulate::evidence::SourceIdentity::of(&board, &design);
    let json = copper_simulate::results::results_json_from(&runs, "Amp", Some(&source));
    let document = copper_json::parse(&json).unwrap();
    assert_eq!(document.get("schema").and_then(|v| v.as_str()), Some("copperscript-sim/v1"));
    let shape = shape_of(&document);
    if std::env::var("COPPER_PRINT_SIM_SHAPE").is_ok() {
        println!("{shape}");
    }
    // The frozen shape: every path with the JSON types it may take. Each one must
    // appear in this document, and with types among those declared.
    let expected: BTreeMap<&str, Vec<&str>> = SHAPE
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (path, types) = line.split_once(' ').unwrap_or((line, ""));
            (path, types.split('|').collect())
        })
        .collect();
    let mut problems = Vec::new();
    let observed: BTreeMap<&str, Vec<&str>> = shape
        .lines()
        .map(|line| {
            let (path, types) = line.split_once(' ').unwrap();
            (path, types.split('|').collect())
        })
        .collect();
    for (path, types) in &observed {
        match expected.get(path) {
            None => problems.push(format!("new field {path} ({})", types.join("|"))),
            Some(allowed) => {
                for t in types {
                    if !allowed.contains(t) {
                        problems.push(format!("{path} is {t}, declared {}", allowed.join("|")));
                    }
                }
            }
        }
    }
    for path in expected.keys() {
        if !observed.contains_key(path) {
            problems.push(format!("missing field {path}"));
        }
    }
    assert!(
        problems.is_empty(),
        "the results document changed shape:
{}",
        problems.join(
            "
"
        )
    );
    // Every field is described in the schema document.
    let doc =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/sim-results-schema.md"))
            .expect("docs/sim-results-schema.md");
    let undocumented: Vec<&&str> = expected.keys().filter(|path| !doc.contains(&format!("`{path}`"))).collect();
    assert!(undocumented.is_empty(), "fields missing from docs/sim-results-schema.md: {undocumented:#?}");
}

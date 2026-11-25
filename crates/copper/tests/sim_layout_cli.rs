//! `copper sim --layout RUN` on a routed copy of `examples/simulation/howland_pump`.
//!
//! The example has no layout (the language has no tracks) and does not route in every checkout (its real
//! footprints come from the KiCad footprint package), so this file builds one: the example's source and benches
//! copied, with proxy footprints, a declared four-layer stack-up (0.2 mm of er 4.3 from `F.Cu` to a ground plane on
//! `In1.Cu`), fixed placements and hand-placed tracks of 0.25 mm for its six signal nets (supply nets left as pads:
//! they are one node each anyway), written as a route-board run's `final-board.state.json`. The copy adds
//! `use layout;` to the output impedance and stability benches.
//!
//! Set `COPPER_SIM_LAYOUT_KEEP=DIR` to keep the project, the run and the outputs in `DIR`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const STACKUP: &str = r#"    mechanical {
        outline rectangle { width = 50mm; height = 40mm; }
        stackup {
            copper F.Cu { thickness = 0.035mm; }
            dielectric P1 { thickness = 0.2mm; er = 4.3; loss_tangent = 0.02; material = "prepreg"; type = prepreg; }
            copper In1.Cu { thickness = 0.035mm; }
            dielectric C1 { thickness = 1.06mm; er = 4.6; loss_tangent = 0.02; material = "FR4"; type = core; }
            copper In2.Cu { thickness = 0.035mm; }
            dielectric P2 { thickness = 0.2mm; er = 4.3; loss_tangent = 0.02; material = "prepreg"; type = prepreg; }
            copper B.Cu { thickness = 0.035mm; }
        }
    }
"#;

/// `(reference, x, y, rotation)`, mm and degrees.
const PLACEMENTS: &[(&str, f64, f64, i32)] = &[
    ("U1", 20.0, 20.0, 0),
    ("U2", 40.0, 20.0, 0),
    ("J_IN", 6.0, 24.0, 180),
    ("R1", 12.0, 24.0, 0),
    ("R2", 30.0, 26.0, 0),
    ("R3", 12.0, 15.0, 180),
    ("R4", 20.0, 15.0, 180),
    ("R_SENSE", 30.0, 22.0, 0),
    ("J_OUT", 36.0, 26.0, 0),
    ("J_PWR", 6.0, 32.0, 0),
    ("C_U1N", 12.0, 32.0, 0),
    ("C_U1P", 18.0, 32.0, 0),
    ("C_U2N", 24.0, 32.0, 0),
    ("C_U2P", 30.0, 32.0, 0),
];

/// The example's source with proxy footprints, the stack-up, the plane and the placements. Its decoupling
/// annotations are dropped: the proxy footprints do not satisfy their land checks, and they only steer placement.
fn source() -> String {
    let original = std::fs::read_to_string(workspace().join("examples/simulation/howland_pump/board.copper")).unwrap();
    let mut out = String::new();
    for line in original.lines() {
        let mut line = line.to_owned();
        if let Some(start) = line.find(" role = decoupling;") {
            let end = line[start..].find(" }").map(|k| start + k).unwrap();
            line = format!("{}{}", &line[..start], &line[end..]);
        }
        for (reference, part) in [
            ("U1", "p.OPAMP_ADA4817_LIKE"),
            ("U2", "p.OPAMP_ADA4817_LIKE"),
            ("J_IN", "p.HEADER_2"),
            ("J_OUT", "p.HEADER_2"),
            ("J_PWR", "p.HEADER_3"),
        ] {
            let plain = format!("component {reference}: {part};");
            if line.contains(&plain) {
                line = line.replace(&plain, &format!("component {reference}: {part} {{ footprint = \"PROXY\"; }}"));
            }
        }
        out.push_str(&line);
        out.push('\n');
        if line.trim_start().starts_with("import p ") {
            out.push_str(STACKUP);
        }
    }
    let mut extra = String::from(
        "    constraint copper_zone(GND) { id = \"plane\"; layers = \"In1.Cu\"; polygon_mm = \"1,1; 49,1; 49,39; 1,39\"; island_policy = keep_all; }\n",
    );
    for (reference, x, y, rotation) in PLACEMENTS {
        extra.push_str(&format!(
            "    constraint fixed_placement({reference}) {{ x = {x}mm; y = {y}mm; rotation = {rotation}; }}\n"
        ));
    }
    let end = out.rfind('}').unwrap();
    format!("{}{extra}}}\n", &out[..end])
}

fn mm(x: f64, y: f64) -> Point {
    Point::new((x * 1e6).round() as i64, (y * 1e6).round() as i64)
}

/// The hand-placed copper: every signal net, `F.Cu` over the plane except BUF, which crosses LOAD on `B.Cu`.
fn route(board: &mut PhysicalBoard) {
    let mut path = |net: &str, layer: CopperLayer, points: &[(f64, f64)]| {
        for pair in points.windows(2) {
            board.tracks.push(TrackSegment {
                net: net.to_owned(),
                start: mm(pair[0].0, pair[0].1),
                end: mm(pair[1].0, pair[1].1),
                width_nm: 250_000,
                layer,
            });
        }
    };
    let (front, back) = (CopperLayer::Front, CopperLayer::Back);
    path("VIN", front, &[(7.0, 24.0), (11.0, 24.0)]);
    path("PFB", front, &[(13.0, 24.0), (15.0, 24.0), (15.0, 20.0), (17.5, 20.0)]);
    path("PFB", front, &[(15.0, 24.0), (15.0, 26.0), (29.0, 26.0)]);
    path("NFB", front, &[(19.0, 15.0), (16.5, 15.0), (13.0, 15.0)]);
    path("NFB", front, &[(16.5, 15.0), (16.5, 18.73), (17.5, 18.73)]);
    path("DRIVE", front, &[(22.5, 18.73), (24.0, 18.73), (24.0, 15.0), (21.0, 15.0)]);
    path("DRIVE", front, &[(24.0, 18.73), (24.0, 22.0), (29.0, 22.0)]);
    path("LOAD", front, &[(31.0, 22.0), (34.0, 22.0), (34.0, 20.0), (37.5, 20.0)]);
    path("LOAD", front, &[(34.0, 22.0), (34.0, 26.0), (35.0, 26.0)]);
    path("BUF", front, &[(31.0, 26.0), (32.5, 26.0)]);
    path("BUF", back, &[(32.5, 26.0), (32.5, 17.5), (36.5, 17.5)]);
    path("BUF", front, &[(36.5, 17.5), (36.5, 18.73), (37.5, 18.73), (42.5, 18.73)]);
    let via = |net: &str, at: (f64, f64)| Via {
        net: net.to_owned(),
        position: mm(at.0, at.1),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    };
    board.vias.push(via("BUF", (32.5, 26.0)));
    board.vias.push(via("BUF", (36.5, 17.5)));
    // The ground pads reach the plane through a via in the pad.
    for at in [(5.0, 24.0), (37.0, 26.0), (11.0, 15.0), (13.0, 32.0), (19.0, 32.0), (25.0, 32.0), (31.0, 32.0)] {
        board.vias.push(via("GND", at));
    }
}

/// The copied project in `dir` and its routed run in `dir/run`; returns the board file.
fn project(dir: &Path, routed: bool) -> PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    let example = workspace().join("examples/simulation/howland_pump");
    for sub in ["parts", "benches"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        for entry in std::fs::read_dir(example.join(sub)).unwrap() {
            let entry = entry.unwrap();
            let mut text = std::fs::read_to_string(entry.path()).unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if sub == "benches" && (name == "output_impedance.copper" || name == "stability.copper") {
                let open = text.find(" {\n").unwrap() + 3;
                text.insert_str(open, "    use layout;\n");
            }
            std::fs::write(dir.join(sub).join(&name), text).unwrap();
        }
    }
    let board_file = dir.join("board.copper");
    std::fs::write(&board_file, source()).unwrap();
    let design = compile_design_file(&board_file, false, true).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (50.0, 40.0), None, "generic", false, true)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, true).unwrap_or_else(|e| panic!("{e:?}"));
    // The hand-placed copper starts and ends on the pads it is drawn for.
    let model = copper_extract::board::board_model(&board).expect("fill");
    let pad = |reference: &str, number: &str| {
        let (_, p) = model.cam.pads().find(|(f, p)| f.reference == reference && p.number == number).unwrap();
        ((p.center.x as f64 * 1e-6 * 100.0).round() / 100.0, (p.center.y as f64 * 1e-6 * 100.0).round() / 100.0)
    };
    assert_eq!(pad("J_IN", "1"), (7.0, 24.0));
    assert_eq!(pad("R4", "1"), (21.0, 15.0));
    assert_eq!(pad("U1", "2"), (17.5, 18.73));
    assert_eq!(pad("U1", "6"), (22.5, 18.73));
    assert_eq!(pad("U2", "3"), (37.5, 20.0));
    if routed {
        route(&mut board);
    }
    let state = copper_state::StateDocument::new(
        "pcbir.physical:PhysicalBoard",
        copper_json::Object::from([(
            "note".to_owned(),
            Value::from(
                "Hand-placed copper of a copy of examples/simulation/howland_pump (crates/copper/tests/sim_layout_cli.rs)",
            ),
        )]),
        copper_route::state_codec::encode_board(&board),
    );
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::write(dir.join("run").join("final-board.state.json"), state.to_text()).unwrap();
    board_file
}

fn scratch(name: &str) -> (PathBuf, bool) {
    match std::env::var_os("COPPER_SIM_LAYOUT_KEEP") {
        Some(dir) => (PathBuf::from(dir).join(name), true),
        None => (std::env::temp_dir().join(format!("copper-sim-layout-{name}-{}", std::process::id())), false),
    }
}

/// `value.key` for each key of `path`; an array is indexed by a key that is a number.
fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(value, |v, key| match key.parse::<usize>() {
        Ok(index) => &v.as_array().unwrap_or_else(|| panic!("{key} of {v:?}"))[index],
        Err(_) => v.get(key).unwrap_or_else(|| panic!("no {key}")),
    })
}

/// A JSON number as `f64`.
fn number(value: &Value) -> f64 {
    match value {
        Value::Float(x) => *x,
        Value::Int(i) => i.to_string().parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

/// The element of an array whose `name` is `name`.
fn named<'a>(array: &'a Value, name: &str) -> &'a Value {
    array
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn a_bench_that_uses_layout_runs_with_and_without_the_routed_board() {
    let (dir, keep) = scratch("routed");
    let board = project(&dir, true);
    let (board_text, run, out) = (board.to_str().unwrap(), dir.join("run"), dir.join("out"));
    let output = copper(&[
        "sim",
        board_text,
        "--bench",
        "OutputImpedance",
        "--layout",
        run.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    // The schematic meets the 100 kohm at 1 MHz; with this layout (the plane 0.2 mm under every signal track) it
    // does not, so the bench fails on that measure alone.
    assert_eq!(output.status.code(), Some(1), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("FAIL: 1 of 2 limited measures out of limits"), "{stdout}");
    assert!(stdout.contains("layout: ") && stdout.contains("capacitors"), "{stdout}");
    assert!(stdout.contains("layout z_out_1mhz:") && stdout.contains("schematic"), "{stdout}");
    assert!(stdout.contains("layout largest: "), "{stdout}");
    let results = copper_json::parse(&std::fs::read_to_string(out.join("sim-results.json")).unwrap()).unwrap();
    let bench = at(&results, &["benches", "0"]);
    let layout = at(bench, &["layout"]);
    assert!(number(at(layout, &["elements", "capacitors"])) > 10.0, "{layout:?}");
    assert!(number(at(layout, &["elements", "resistors"])) >= 6.0, "{layout:?}");
    let value = |measure: &Value| number(at(measure, &["steps", "0", "value"]));
    let with = value(named(at(bench, &["measures"]), "z_out_1mhz"));
    let without = value(named(at(layout, &["schematic", "measures"]), "z_out_1mhz"));
    // The load node gains the capacitance of its tracks and pads over the plane: its impedance at 1 MHz falls.
    assert!(with < without, "{with} against {without}");
    let schematic_pass = at(named(at(layout, &["schematic", "measures"]), "z_out_1mhz"), &["steps", "0", "pass"]);
    assert_eq!(schematic_pass, &Value::Bool(true));
    let load_gnd = at(layout, &["contributors"])
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            at(c, &["kind"]).as_str() == Some("capacitance")
                && at(c, &["nets", "0"]).as_str() == Some("GND")
                && at(c, &["nets", "1"]).as_str() == Some("LOAD")
        })
        .map(|c| number(at(c, &["value"])));
    if let Some(c) = load_gnd {
        // The extra load capacitance the impedance implies at 1 MHz, against the extracted LOAD to GND.
        let implied = (1.0 / with - 1.0 / without) / (2.0 * std::f64::consts::PI * 1e6);
        assert!(implied > 0.5 * c && implied < 2.0 * c, "implied {implied} against extracted {c}");
    }
    let page = std::fs::read_to_string(out.join("OutputImpedance.html")).unwrap();
    assert!(page.contains("<h2>Layout</h2>"), "the page has the layout section");
    if !keep {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_bench_that_uses_layout_is_refused_without_a_routed_board_and_the_others_run() {
    let (dir, keep) = scratch("missing");
    let board = project(&dir, true);
    let out = dir.join("out");
    let output =
        copper(&["sim", board.to_str().unwrap(), "--bench", "OutputImpedance", "--out", out.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}{}", text(&output.stderr));
    assert!(
        stdout.contains("ERROR: SIM118: bench OutputImpedance says use layout, but no routed board was given"),
        "{stdout}"
    );
    // A bench without use layout is not affected.
    let output =
        copper(&["sim", board.to_str().unwrap(), "--bench", "CurrentFlatness", "--out", out.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stdout));
    if !keep {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_unrouted_or_foreign_board_is_refused() {
    let (dir, keep) = scratch("unrouted");
    let board = project(&dir, false);
    let out = dir.join("out");
    let run = dir.join("run");
    let output = copper(&[
        "sim",
        board.to_str().unwrap(),
        "--bench",
        "OutputImpedance",
        "--layout",
        run.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("ERROR: SIM119: the routed board") && stdout.contains("not fully routed"), "{stdout}");
    // The routed board of another design.
    let foreign = workspace().join("examples/emi_return_path/routed");
    let output = copper(&[
        "sim",
        board.to_str().unwrap(),
        "--bench",
        "OutputImpedance",
        "--layout",
        foreign.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("ERROR: SIM119") && stdout.contains("is not this design's"), "{stdout}");
    // No routed board in the directory.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let output = copper(&[
        "sim",
        board.to_str().unwrap(),
        "--bench",
        "OutputImpedance",
        "--layout",
        empty.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(text(&output.stdout).contains("ERROR: SIM118: --layout"), "{}", text(&output.stdout));
    if !keep {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

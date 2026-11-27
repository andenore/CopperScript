//! Plan F6: a part with both a built-in (macro) block and a `sim spice`
//! (vendor) block per unit, the model choice, and vendor files registered by
//! hand (`sha256` with a `download` note).

use std::fs;

use copper_lang::compiler::{Design, compile_design_file, load_design};
use copper_lang::serializer::design_json;
use copper_lang::simulation::{
    ModelChoice, SimKind, SpiceSource, has_alternatives, select_models, units_without_macro,
};

const PINS: &str = "    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
    pin NC { number = \"6\"; connection = do_not_connect; }
";

const MACRO: &str = "sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; unmodelled = NC; gbw = 10MHz; open_loop_gain = 100dB; }";

fn directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-model-library-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
    directory
}

fn compile(name: &str, sim: &str) -> Result<Design, copper_lang::CopperScriptError> {
    let directory = directory(name);
    fs::write(directory.join("parts").join("amp.copper"), format!("part Amp {{\n{PINS}{sim}\n}}\n")).unwrap();
    let board = "board A {\n    import p \"./parts\";\n    component U1: p.Amp;\n}\n";
    fs::write(directory.join("board.copper"), board).unwrap();
    compile_design_file(&directory.join("board.copper"), false, true)
}

fn error_of(result: Result<Design, copper_lang::CopperScriptError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(failure) => failure.to_string(),
    }
}

#[test]
fn a_part_carries_a_macro_and_a_vendor_model_for_one_unit() {
    let spice =
        "sim spice { file = \"amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; }";
    let design = compile("pair", &format!("{MACRO}\n{spice}")).unwrap();
    let part = &design.electrical.library["p.Amp"];
    assert_eq!(part.sim.len(), 2);
    assert!(has_alternatives(&part.sim));
    let kinds = |choice| select_models(&part.sim, choice).iter().map(|m| m.spec.kind).collect::<Vec<_>>();
    assert_eq!(kinds(ModelChoice::Default), [SimKind::Opamp]);
    assert_eq!(kinds(ModelChoice::Macro), [SimKind::Opamp]);
    assert_eq!(kinds(ModelChoice::Vendor), [SimKind::Spice]);
    assert!(units_without_macro(&part.sim).is_empty());
    // Each block keeps its own unmodelled list.
    assert_eq!(part.sim[1].unmodelled, ["NC"]);
    // The part JSON lists both blocks.
    let json = design_json(&design);
    assert!(json.contains("\"kind\": \"opamp\"") && json.contains("\"kind\": \"spice\""), "{json}");
}

#[test]
fn units_pair_their_models_by_name() {
    let pins = "    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin INP2 { number = \"4\"; domains = \"analog\"; directions = \"input\"; }
    pin INN2 { number = \"5\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT2 { number = \"6\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"7\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"8\"; domains = \"power\"; directions = \"input\"; }
";
    let blocks =
        "sim opamp A { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; }
    sim opamp B { inp = INP2; inn = INN2; out = OUT2; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; }
    sim spice A { file = \"amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; }";
    let directory = directory("units");
    fs::write(directory.join("parts").join("amp.copper"), format!("part Dual {{\n{pins}{blocks}\n}}\n")).unwrap();
    fs::write(directory.join("board.copper"), "board A {\n    import p \"./parts\";\n    component U1: p.Dual;\n}\n")
        .unwrap();
    let design = compile_design_file(&directory.join("board.copper"), false, true).unwrap();
    let part = &design.electrical.library["p.Dual"];
    let chosen = |choice| {
        select_models(&part.sim, choice).iter().map(|m| (m.unit.clone().unwrap(), m.spec.kind)).collect::<Vec<_>>()
    };
    assert_eq!(chosen(ModelChoice::Default), [("A".to_owned(), SimKind::Opamp), ("B".to_owned(), SimKind::Opamp)]);
    // Unit A runs its spice model, unit B (no vendor model) its macro model.
    assert_eq!(chosen(ModelChoice::Vendor), [("B".to_owned(), SimKind::Opamp), ("A".to_owned(), SimKind::Spice)]);
}

#[test]
fn a_unit_has_at_most_one_block_of_each_sort() {
    let message = error_of(compile("two-macros", &format!("{MACRO}\n{MACRO}")));
    assert!(message.contains("SIM017") && message.contains("name the unit"), "{message}");
    let spice =
        "sim spice { file = \"amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; }";
    let message = error_of(compile("three", &format!("{MACRO}\n{spice}\n{spice}")));
    assert!(message.contains("SIM017"), "{message}");
    let named =
        "sim spice A { file = \"amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; }";
    let message =
        error_of(compile("named-twice", &format!("{}\n{named}\n{named}", MACRO.replace("sim opamp", "sim opamp A"))));
    assert!(message.contains("SIM017") && message.contains("duplicate sim unit 'A'"), "{message}");
}

#[test]
fn the_vendor_model_must_cover_the_pins_on_its_own() {
    // The spice block leaves NC neither a terminal nor unmodelled: only the vendor choice fails.
    let spice = "sim spice { file = \"amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; }";
    let message = error_of(compile("vendor-pins", &format!("{MACRO}\n{spice}")));
    assert!(message.contains("SIM106"), "{message}");
    assert!(message.contains("--models vendor") && message.contains("'NC'"), "{message}");
}

#[test]
fn a_vendor_file_registered_by_hand_needs_its_hash_and_a_download_note() {
    let hash = "b".repeat(64);
    let block = |body: &str| {
        format!("sim spice {{ {body} subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; }}")
    };
    let design = compile(
        "registered",
        &block(&format!(
            "sha256 = \"{hash}\"; download = \"the AMP macro model from its product page\"; member = \"amp.cir\";"
        )),
    )
    .unwrap();
    let spice = design.electrical.library["p.Amp"].sim[0].spec.spice.clone().unwrap();
    assert_eq!(
        spice.source,
        SpiceSource::Registered {
            sha256: hash.clone(),
            download: "the AMP macro model from its product page".into(),
            member: Some("amp.cir".into())
        }
    );
    assert!(design_json(&design).contains("\"download\": \"the AMP macro model from its product page\""));
    let message = error_of(compile("download-no-hash", &block("download = \"somewhere\";")));
    assert!(message.contains("SIM076") && message.contains("download needs the sha256"), "{message}");
    let message = error_of(compile(
        "both",
        &block(&format!("source = \"https://example.com/a.zip\"; sha256 = \"{hash}\"; download = \"x\";")),
    ));
    assert!(message.contains("SIM076") && message.contains("not both"), "{message}");
    let message = error_of(compile("hash-alone", &block(&format!("sha256 = \"{hash}\";"))));
    assert!(message.contains("SIM076") && message.contains("download"), "{message}");
}

#[test]
fn a_vendor_model_carries_a_note_on_where_it_differs() {
    let hash = "d".repeat(64);
    let sim = format!(
        "sim spice {{ sha256 = \"{hash}\"; download = \"x\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; note = \"its slew rate is the model's own\"; }}"
    );
    let design = compile("note", &sim).unwrap();
    let spice = design.electrical.library["p.Amp"].sim[0].spec.spice.clone().unwrap();
    assert_eq!(spice.note.as_deref(), Some("its slew rate is the model's own"));
    assert!(design_json(&design).contains("\"note\": \"its slew rate is the model's own\""));
    let message = error_of(compile("note-number", &sim.replace("\"its slew rate is the model's own\"", "3")));
    assert!(message.contains("SIM076") && message.contains("note"), "{message}");
}

#[test]
fn a_loaded_board_binds_vendor_models_to_its_project_cache() {
    let hash = "c".repeat(64);
    let directory = directory("bind");
    fs::write(directory.join("copper.mod"), "module example.invalid/bind\n").unwrap();
    let sim = format!(
        "sim spice {{ sha256 = \"{hash}\"; download = \"x\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; unmodelled = NC; }}"
    );
    fs::write(directory.join("parts").join("amp.copper"), format!("part Amp {{\n{PINS}{sim}\n}}\n")).unwrap();
    fs::create_dir_all(directory.join("boards")).unwrap();
    fs::write(
        directory.join("boards").join("board.copper"),
        "board A {\n    import p \"../parts\";\n    component U1: p.Amp;\n}\n",
    )
    .unwrap();
    let design = load_design(&directory.join("boards").join("board.copper"), false, true).unwrap();
    let spice = design.electrical.library["p.Amp"].sim[0].spec.spice.clone().unwrap();
    assert!(spice.project.as_deref().is_some_and(|p| p.ends_with("boards")), "{:?}", spice.project);
    let message = copper_lang::spice_models::resolve(&spice).unwrap_err();
    assert!(message.starts_with("SIM108"), "{message}");
    assert!(message.contains("copper models add") && message.contains("download it yourself: x"), "{message}");
    // The cache is the project's (the directory of the nearest copper.mod above the board), named as the loader
    // resolves it: on Windows the temporary directory is a short 8.3 path (RUNNER~1) the resolved one expands.
    let cache = copper_lang::paths::resolve(&directory).join(".copper-cache").join("spice").join(&hash);
    assert!(message.contains(&copper_lang::paths::display(&cache)), "{message}");
}

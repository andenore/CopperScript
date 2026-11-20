//! `sim spice` part blocks: syntax, checks and the compiled JSON.

use std::fs;

use copper_lang::compiler::{Design, compile_design_file};
use copper_lang::serializer::design_json;
use copper_lang::simulation::{SimKind, SpiceSource};

const PINS: &str = "    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
";

fn compile(name: &str, sim: &str) -> Result<Design, copper_lang::CopperScriptError> {
    let directory = std::env::temp_dir().join(format!("copper-spice-block-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
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
fn local_and_pinned_vendor_models_compile() {
    let design = compile(
        "local",
        "sim spice { file = \"models/amp.lib\"; subckt = \"AMP\"; dialect = pspice; terminals = INP, INN, VP, VN, OUT; }",
    )
    .unwrap();
    let part = &design.electrical.library["p.Amp"];
    let model = &part.sim[0];
    assert_eq!(model.spec.kind, SimKind::Spice);
    assert_eq!(model.pins.keys().collect::<Vec<_>>(), ["1", "2", "3", "4", "5"]);
    assert_eq!(model.pins["3"], "VP");
    let spice = model.spec.spice.as_ref().unwrap();
    assert_eq!(spice.source, SpiceSource::File("models/amp.lib".into()));
    assert_eq!((spice.subckt.as_str(), spice.dialect.as_str()), ("AMP", "pspice"));
    assert!(spice.directory.ends_with("parts"), "{}", spice.directory);
    let json = design_json(&design);
    assert!(json.contains("\"kind\": \"spice\""), "{json}");
    assert!(json.contains("\"file\": \"models/amp.lib\""), "{json}");
    let hash = "a".repeat(64);
    let design = compile(
        "url",
        &format!("sim spice {{ source = \"https://www.ti.com/lit/zip/x\"; sha256 = \"{hash}\"; subckt = \"X\"; terminals = INP, INN, OUT, VP, VN; }}"),
    )
    .unwrap();
    let spice = design.electrical.library["p.Amp"].sim[0].spec.spice.clone().unwrap();
    assert_eq!(
        spice.source,
        SpiceSource::Url { url: "https://www.ti.com/lit/zip/x".into(), sha256: hash.clone(), member: None }
    );
    assert_eq!(spice.dialect, "spice3", "the default dialect");
    let design = compile(
        "member",
        &format!("sim spice {{ source = \"https://www.ti.com/lit/zip/x\"; sha256 = \"{hash}\"; member = \"amp_a.lib\"; subckt = \"X\"; terminals = INP, INN, OUT, VP, VN; }}"),
    )
    .unwrap();
    let spice = design.electrical.library["p.Amp"].sim[0].spec.spice.clone().unwrap();
    assert!(matches!(spice.source, SpiceSource::Url { member: Some(m), .. } if m == "amp_a.lib"));
    // A pin may be on several subcircuit nodes; pins left out are listed as unmodelled.
    assert!(
        compile(
            "shared",
            "sim spice { file = \"a.cir\"; subckt = \"A\"; terminals = INP, INN, OUT, VP, VP; unmodelled = VN; }"
        )
        .is_ok()
    );
}

#[test]
fn sim_spice_checks_name_the_problem() {
    let cases = [
        (
            "nofile",
            "sim spice { subckt = \"A\"; terminals = INP, INN, OUT, VP, VN; }",
            "SIM076: sim spice needs a model file",
        ),
        (
            "nosub",
            "sim spice { file = \"a.cir\"; terminals = INP, INN, OUT, VP, VN; }",
            "SIM076: sim spice needs the subcircuit",
        ),
        ("noterm", "sim spice { file = \"a.cir\"; subckt = \"A\"; }", "SIM076: sim spice needs terminals"),
        (
            "http",
            "sim spice { source = \"http://x/a.cir\"; sha256 = \"00\"; subckt = \"A\"; terminals = INP; }",
            "must be an https:// URL",
        ),
        (
            "hash",
            &format!(
                "sim spice {{ source = \"https://x/a.cir\"; sha256 = \"{}\"; subckt = \"A\"; terminals = INP; }}",
                "z".repeat(64)
            ),
            "64 hexadecimal digits",
        ),
        (
            "both",
            &format!(
                "sim spice {{ file = \"a\"; source = \"https://x/a.cir\"; sha256 = \"{}\"; subckt = \"A\"; terminals = INP; }}",
                "0".repeat(64)
            ),
            "not both",
        ),
        (
            "dialect",
            "sim spice { file = \"a.cir\"; subckt = \"A\"; dialect = pspise; terminals = INP, INN, OUT, VP, VN; }",
            "did you mean 'pspice'?",
        ),
        ("key", "sim spice { file = \"a.cir\"; subckt = \"A\"; termnals = INP; }", "did you mean 'terminals'?"),
        ("pin", "sim spice { file = \"a.cir\"; subckt = \"A\"; terminals = INP, INN, OUTT, VP, VN; }", "SIM014"),
        ("uncovered", "sim spice { file = \"a.cir\"; subckt = \"A\"; terminals = INP, INN, OUT; }", "SIM016: pin 'VP'"),
        (
            "member",
            "sim spice { file = \"a.cir\"; member = \"a.lib\"; subckt = \"A\"; terminals = INP, INN, OUT, VP, VN; }",
            "inside a ZIP archive",
        ),
        (
            "twice",
            "sim spice { file = \"a.cir\"; file = \"b.cir\"; subckt = \"A\"; terminals = INP; }",
            "duplicate property",
        ),
    ];
    for (name, sim, expected) in cases {
        let message = error_of(compile(name, sim));
        assert!(message.contains(expected), "{name}: {message}");
    }
}

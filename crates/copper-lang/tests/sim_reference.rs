//! The editor's table of the simulation language (`copper_lang::sim_reference`)
//! against the checker: every analysis kind and every option it lists is
//! accepted by the bench checker, and an option it does not list is refused
//! with the table's names as the hint.

use copper_lang::bench::compile_bench_source;
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::sim_reference::ANALYSES;

const RC_BOARD: &str = "board RC {
    use library \"tiny\";
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    net VIN { R1.1; }
    net VOUT { R1.2; C1.1; }
    net GND { C1.2; }
    supply INPUT { voltage = 1V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn rc() -> Design {
    compile_design_source(RC_BOARD, "rc.copper", false, true).unwrap()
}

/// The table's example values in this board's names.
fn local(text: &str) -> String {
    text.replace("V(OUT)", "V(VOUT)").replace("U1.OUT", "R1.2").replace("J1", "INPUT").replace("0V", "0.5V")
}

fn compile(analysis: &str) -> Result<(), String> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; dc = 1V; ac = 1V; }}\n {analysis}\n measure m = V(VOUT);\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc()).map(|_| ()).map_err(|failure| failure.to_string())
}

#[test]
fn every_analysis_option_in_the_table_is_accepted() {
    for kind in ANALYSES {
        // The example statement compiles, apart from what this small board cannot give.
        let example = local(kind.example);
        if let Err(failure) = compile(&example) {
            assert!(!failure.contains("unknown analysis"), "{}: {failure}", kind.name);
        }
        let base = example.trim_end_matches(';').trim_end_matches('}').trim().to_owned();
        for option in kind.options {
            let statement = if base.ends_with('{') || kind.example.contains('{') {
                format!("{base} {} = {}; }}", option.name, local(option.example))
            } else {
                format!("analysis {} {{ {} = {}; }}", kind.name, option.name, local(option.example))
            };
            if let Err(failure) = compile(&statement) {
                assert!(!failure.contains("unknown analysis"), "{} {}: {statement}: {failure}", kind.name, option.name);
            }
        }
    }
}

#[test]
fn an_option_the_table_does_not_list_is_refused_with_its_names() {
    for kind in ANALYSES.iter().filter(|kind| kind.name != "op") {
        let failure = compile(&format!("analysis {} {{ fromm = 1; }}", kind.name)).unwrap_err();
        assert!(failure.contains("unknown analysis") && failure.contains("property"), "{}: {failure}", kind.name);
    }
}

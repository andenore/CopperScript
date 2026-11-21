//! The bench language of phase F9 of `docs/integrated-simulation-plan.md`:
//! `analysis tran` with `steady`, `noise`, `seed` and `window`, the spectrum
//! functions, and stepped ranges. Nothing here simulates.

use copper_lang::bench::{Analysis, Bench, bench_json, compile_bench_source};
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::expression::{Function, Units};
use copper_lang::transient::SpectrumWindow;
use copper_units::Dimension;

fn error_of<T: std::fmt::Debug>(result: Result<T, copper_lang::CopperScriptError>) -> String {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(failure) => failure.to_string(),
    }
}

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

/// A bench for `RC` driven by a 1 kHz sine, with `analysis`, then `body`.
fn bench(analysis: &str, body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; waveform = sine {{ amplitude = 1V; frequency = 1kHz; }}; }}\n \
         {analysis}\n{body}\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc())
}

#[test]
fn steady_noise_and_window_properties() {
    let b = bench(
        "analysis tran { stop = 2ms; max_step = 5us; steady = 1kHz; window = hann; }",
        " measure h1 = harmonic(V(VOUT), 1kHz, 1);",
    )
    .unwrap();
    let Analysis::Tran(tran) = &b.analysis else { panic!("tran") };
    let steady = tran.steady.as_ref().unwrap();
    assert_eq!((steady.value.dimension(), steady.value.to_string().as_str()), (Dimension::Frequency, "1 kHz"));
    assert_eq!(tran.window, Some(SpectrumWindow::Hann));
    assert!(!tran.noise);
    let json = bench_json(&b);
    assert!(json.contains("\"steady\"") && json.contains("\"window\": \"hann\""), "{json}");
    // A period works too, and a param of either.
    let b = bench("analysis tran { stop = 2ms; steady = 1ms; }", "").unwrap();
    let Analysis::Tran(tran) = &b.analysis else { panic!("tran") };
    assert_eq!(tran.steady.as_ref().unwrap().value.dimension(), Dimension::Time);
    let b = bench("param F0 = 1kHz;\n analysis tran { stop = 2ms; steady = F0; }", "").unwrap();
    let Analysis::Tran(tran) = &b.analysis else { panic!("tran") };
    assert_eq!(tran.steady.as_ref().unwrap().param.as_deref(), Some("F0"));

    let b = bench("analysis tran { stop = 1ms; max_step = 1us; noise = true; seed = 42; }", "").unwrap();
    let Analysis::Tran(tran) = &b.analysis else { panic!("tran") };
    assert!(tran.noise && tran.seed == Some(42));
    assert!(bench_json(&b).contains("\"seed\": 42"));

    for (analysis, code, words) in [
        ("analysis tran { stop = 2ms; steady = 1V; }", "SIM112", "period"),
        ("analysis tran { stop = 2ms; steady = 0s; }", "SIM112", "positive"),
        ("analysis tran { stop = 2ms; steady = 1ms; uic = true; }", "SIM112", "uic"),
        ("analysis tran { stop = 2ms; max_step = 1us; steady = 1ms; noise = true; }", "SIM112", "noise"),
        ("analysis tran { stop = 2ms; noise = true; }", "SIM114", "max_step"),
        ("analysis tran { stop = 2ms; noise = yes; }", "SIM114", "true"),
        ("analysis tran { stop = 2ms; max_step = 1us; noise = true; seed = -1; }", "SIM114", "seed"),
        ("analysis tran { stop = 2ms; seed = 3; }", "SIM114", "noise = true"),
        ("analysis tran { stop = 2ms; window = han; }", "SIM113", "hann"),
    ] {
        let message = error_of(bench(analysis, ""));
        assert!(message.contains(code) && message.contains(words), "{analysis}: {message}");
    }
    // A steady state finds its own start: no ic.
    let message = error_of(bench("analysis tran { stop = 2ms; steady = 1ms; }", " ic C1 = 1V;"));
    assert!(message.contains("SIM112") && message.contains("ic"), "{message}");
}

#[test]
fn spectrum_functions_are_typed_and_spectrum_is_a_plot_only() {
    let b = bench(
        "analysis tran { stop = 10ms; start = 2ms; max_step = 5us; }",
        " measure spur = spur(V(VOUT), 1kHz);\n measure sfdr = sfdr(V(VOUT), 1kHz, 0Hz, 20kHz);\n \
         measure sinad = sinad(V(VOUT), 1kHz);\n measure snr = snr(V(VOUT), 1kHz, 10Hz, 100kHz);\n \
         plot V(VOUT), spectrum(V(VOUT), 1kHz);\n plot spectrum(V(VOUT));",
    )
    .unwrap();
    let units: Vec<Units> = b.measures.iter().map(|m| m.expression.ty.units).collect();
    assert_eq!(units[0], Units::of(Dimension::Voltage));
    assert!(units[1..].iter().all(|u| *u == Units::of(Dimension::Gain)), "{units:?}");
    let spectra: Vec<bool> = b
        .plots
        .iter()
        .flat_map(|p| &p.expressions)
        .map(|e| copper_lang::expression::calls(e, Function::Spectrum))
        .collect();
    assert_eq!(spectra, [false, true, true]);
    for (body, words) in [
        (" measure m = max(spectrum(V(VOUT), 1kHz));", "spectrum(...) is plotted on its own frequency axis"),
        (" let s = spectrum(V(VOUT), 1kHz);", "not in a let"),
        (" plot 2 * spectrum(V(VOUT), 1kHz);", "whole plot expression"),
        (" measure m = spur(V(VOUT), 1V);", "the fundamental, a frequency"),
        (" measure m = sfdr(V(VOUT), 1kHz, 1kHz);", "2 arguments (x, f0), or 4"),
        (" measure m = snr(V(VOUT), 1kHz, 1ms, 2ms);", "band edge"),
        (" plot spectrum(V(VOUT), 1kHz, 2kHz);", "1 argument (x, windowed), or 2"),
    ] {
        let message = error_of(bench("analysis tran { stop = 10ms; }", body));
        assert!(message.contains(words), "{body}: {message}");
    }
    // In time only.
    let message = error_of(bench(
        "analysis ac { from = 1Hz; to = 1kHz; points_per_decade = 5; }",
        " measure m = sinad(mag(V(VOUT)), 1kHz);",
    ));
    assert!(message.contains("SIM113") && message.contains("analysis tran"), "{message}");
}

#[test]
fn stepped_ranges() {
    let values = |step: &str| -> Vec<String> {
        let b = bench("analysis tran { stop = 1ms; }", step).unwrap_or_else(|e| panic!("{step}: {e}"));
        b.steps[0].values.iter().map(|q| q.to_string()).collect()
    };
    assert_eq!(
        values(" step R_VALUE from 1kohm to 2kohm by 250ohm;"),
        ["1 kohm", "1.25 kohm", "1.5 kohm", "1.75 kohm", "2 kohm"]
    );
    // The last value is the end itself when the steps do not land on it, and a range may run down.
    assert_eq!(values(" step R_VALUE from 1kohm to 2kohm by 400ohm;"), ["1 kohm", "1.4 kohm", "1.8 kohm", "2 kohm"]);
    assert_eq!(values(" step R_VALUE from 3kohm to 1kohm by 1kohm;"), ["3 kohm", "2 kohm", "1 kohm"]);
    assert_eq!(values(" step R_VALUE from 1kohm to 2kohm points 3;"), ["1 kohm", "1.5 kohm", "2 kohm"]);
    for (step, words) in [
        (" step R_VALUE from 1kohm to 2kohm;", "by s; or step P from a to b points n"),
        (" step R_VALUE from 1kohm to 1kohm by 1ohm;", "two different ends"),
        (" step R_VALUE from 1kohm to 2kohm by -1ohm;", "positive"),
        (" step R_VALUE from 0ohm to 2kohm by 1ohm;", "more than 1000 values"),
        (" step R_VALUE from 1kohm to 2kohm points 1;", "from 2 to 1000"),
        (" step R_VALUE from 1V to 2V by 1V;", "SIM"),
    ] {
        let message = error_of(bench("analysis tran { stop = 1ms; }", step));
        assert!(message.contains(words), "{step}: {message}");
    }
}

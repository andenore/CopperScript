//! Physical commands agree on source construction when --layers is omitted.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str = r#"board SourceStack {
    use library "tiny";
    component R1: RESISTOR { value = 1kohm; footprint = "0402"; }
    component R2: RESISTOR { value = 1kohm; footprint = "0402"; }
    net N { R1.1; R2.1; }
    no_connect R1.2, R2.2 { reason = "configuration fixture"; }
    constraint fixed_placement(R1) { x = 10mm; y = 15mm; }
    constraint fixed_placement(R2) { x = 20mm; y = 15mm; }
    mechanical {
        outline rectangle { width = 40mm; height = 30mm; }
        stackup {
            copper F.Cu { thickness = 0.035mm; }
            dielectric top { thickness = 0.2mm; er = 4.1; }
            copper In1.Cu { thickness = 0.035mm; }
            dielectric core { thickness = 1mm; er = 4.1; }
            copper In2.Cu { thickness = 0.035mm; }
            dielectric bottom { thickness = 0.2mm; er = 4.1; }
            copper B.Cu { thickness = 0.035mm; }
        }
    }
}"#;

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("source-configuration").join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("board.copper"), SOURCE).unwrap();
    path
}

fn run(dir: &Path, command: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .current_dir(dir)
        .args([command, "board.copper", "--locked", "--offline", "--allow-proxy-footprints"])
        .args(args)
        .output()
        .unwrap()
}

fn success(result: &Output) {
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn inferred_and_explicit_stacks_export_and_route_identical_boards() {
    for command in ["export-kicad-pcb", "plan-layout", "route-board"] {
        let dir = scratch(command);
        let mut args = vec!["-o", "auto.kicad_pcb"];
        if command != "export-kicad-pcb" {
            args.extend(["--candidates", "1"]);
        }
        success(&run(&dir, command, &args));
        args[1] = "explicit.kicad_pcb";
        args.extend(["--layers", "4"]);
        success(&run(&dir, command, &args));
        let auto = std::fs::read_to_string(dir.join("auto.kicad_pcb")).unwrap();
        assert!(auto.contains("In1.Cu") && auto.contains("In2.Cu"));
        assert_eq!(auto, std::fs::read_to_string(dir.join("explicit.kicad_pcb")).unwrap(), "{command}");
    }
}

#[test]
fn global_routing_si_and_editor_use_the_same_stack() {
    for (command, args) in [
        ("route-global", vec!["-o", "auto.json"]),
        ("si-check", vec!["--json"]),
        ("edit-mechanical", vec!["--scene-output", "auto.json"]),
    ] {
        let dir = scratch(command);
        let automatic = run(&dir, command, &args);
        success(&automatic);
        let saved =
            if command == "si-check" { automatic.stdout } else { std::fs::read(dir.join("auto.json")).unwrap() };
        let mut explicit_args = args;
        if command != "si-check" {
            explicit_args[1] = "explicit.json";
        }
        explicit_args.extend(["--layers", "4"]);
        let explicit = run(&dir, command, &explicit_args);
        success(&explicit);
        let explicit_saved =
            if command == "si-check" { explicit.stdout } else { std::fs::read(dir.join("explicit.json")).unwrap() };
        assert_eq!(saved, explicit_saved, "{command}");
    }
}

#[test]
fn an_explicit_layer_count_cannot_replace_the_source_stackup() {
    let dir = scratch("conflict");
    for command in [
        "export-kicad-pcb",
        "plan-layout",
        "route-global",
        "route-board",
        "si-check",
        "synthesize-antenna",
        "edit-mechanical",
    ] {
        let result = run(&dir, command, &["--layers", "2", "--help"]);
        success(&result);
        let result = run(&dir, command, &["--layers", "2"]);
        assert_eq!(result.status.code(), Some(2), "{command}");
        let message = format!("{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        assert!(
            message.contains("stackup declares 4") && message.contains("--layers selects 2"),
            "{command}: {message}"
        );
    }
}

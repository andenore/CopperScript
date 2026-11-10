//! `copper init [DIRECTORY] [--name NAME] [--force]`: a new project that
//! checks offline as written. It has a `copper.mod` with no dependencies, a
//! `board.copper` with a small supplied and decoupled circuit on the built-in
//! `tiny` library, and a `.gitignore` for the package cache and build output.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::help;
use crate::option_parser::{self, flag, value};

const USAGE: &str = "usage: copper init [-h] [--name NAME] [--force] [directory]";

/// `my-sensor_board` → `MySensorBoard`; a leading digit gets a `Board` prefix.
fn board_name(raw: &str) -> String {
    let mut name = String::new();
    for part in raw.split(|c: char| !c.is_ascii_alphanumeric()).filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            name.push(first.to_ascii_uppercase());
            name.extend(chars);
        }
    }
    if name.is_empty() {
        return "Board".to_owned();
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert_str(0, "Board");
    }
    name
}

/// `MySensorBoard` → `my-sensor-board`, for the module path.
fn module_name(board: &str) -> String {
    let mut module = String::new();
    for (index, c) in board.chars().enumerate() {
        if c.is_ascii_uppercase() && index > 0 {
            module.push('-');
        }
        module.push(c.to_ascii_lowercase());
    }
    module
}

fn manifest(board: &str) -> String {
    format!(
        "module local/{module}\n\
         \n\
         // Pin libraries here. A KiCad footprint library, for example:\n\
         // require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd\n\
         // footprint-library Resistor_SMD github.com/KiCad/kicad-footprints/Resistor_SMD.pretty\n\
         // Then run `copper lock board.copper` and commit copper.lock.\n",
        module = module_name(board)
    )
}

fn source(board: &str) -> String {
    format!(
        "// A starting point: a 5 V input, a 3.3 V regulator and an RC filter on the\n\
         // built-in `tiny` library. Replace the parts with library definitions\n\
         // (`copper library search QUERY --project .`) and real footprints.\n\
         board {board} {{\n\
         \x20   use library \"tiny\";\n\
         \n\
         \x20   component PS1: VOLTAGE_SOURCE;\n\
         \x20   component U1: REGULATOR_3V3 {{ footprint = \"SOT-23-3\"; }}\n\
         \x20   component C1: CAPACITOR {{ value = 1uF; footprint = \"0603\"; }}\n\
         \x20   component R1: RESISTOR {{ value = 1kohm; footprint = \"0402\"; }}\n\
         \x20   component C2: CAPACITOR {{ value = 100nF; footprint = \"0402\"; }}\n\
         \n\
         \x20   net VBUS {{ PS1.OUT; U1.IN; }}\n\
         \x20   net V3V3 {{ U1.OUT; C1.1; R1.1; }}\n\
         \x20   net FILTERED {{ R1.2; C2.1; }}\n\
         \x20   net GND {{ PS1.GND; U1.GND; C1.2; C2.2; }}\n\
         \n\
         \x20   supply VBUS {{ voltage = 5V; source = PS1.OUT; }}\n\
         \x20   supply V3V3 {{ voltage = 3.3V; source = U1.OUT; }}\n\
         \x20   supply GND {{ voltage = 0V; external = true; }}\n\
         \n\
         \x20   mechanical {{\n\
         \x20       outline rectangle {{ width = 30mm; height = 20mm; }}\n\
         \x20   }}\n\
         }}\n"
    )
}

const GITIGNORE: &str =
    "# The package cache (`copper check --locked` refills it) and build output.\n.copper-cache/\nbuild/\n";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper init: error: {message}");
        2
    };
    let parsed = match option_parser::parse(arguments, &[value("--name"), flag("--force")]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.help() {
        let _ = writeln!(out, "{USAGE}\n\ncreate a new project: copper.mod, board.copper and .gitignore");
        help::options(out, "init");
        return 0;
    }
    let directory = match parsed.positionals.as_slice() {
        [] => PathBuf::from("."),
        [directory] => PathBuf::from(directory),
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let raw_name = match parsed.value("--name") {
        Some(name) => name.to_owned(),
        None => {
            let absolute = std::path::absolute(&directory).unwrap_or_else(|_| directory.clone());
            absolute.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
        }
    };
    let board = board_name(&raw_name);
    let files: [(&str, String); 3] =
        [("copper.mod", manifest(&board)), ("board.copper", source(&board)), (".gitignore", GITIGNORE.to_owned())];
    let force = parsed.flag("--force");
    let existing: Vec<&str> =
        files.iter().map(|(name, _)| *name).filter(|name| directory.join(name).exists()).collect();
    if !force && !existing.is_empty() {
        let _ = writeln!(
            err,
            "copper init: error: {} already exist{} in {}; pass --force to overwrite",
            existing.join(", "),
            if existing.len() == 1 { "s" } else { "" },
            display(&directory)
        );
        return 2;
    }
    if let Err(failure) = std::fs::create_dir_all(&directory) {
        let _ = writeln!(err, "copper init: error: cannot create {}: {failure}", display(&directory));
        return 2;
    }
    for (name, text) in &files {
        let path = directory.join(name);
        if let Err(failure) = std::fs::write(&path, text) {
            let _ = writeln!(err, "copper init: error: cannot write {}: {failure}", display(&path));
            return 2;
        }
    }
    let board_path = display(&directory.join("board.copper"));
    let _ = writeln!(out, "Created {board} in {}: copper.mod, board.copper, .gitignore", display(&directory));
    let _ = writeln!(out, "Next: copper check {board_path}");
    let _ =
        writeln!(out, "      copper plan-layout {board_path} --allow-proxy-footprints -o build/placement.kicad_pcb");
    0
}

fn display(path: &Path) -> String {
    copper_lang::paths::display(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_identifiers_and_modules_are_kebab_case() {
        assert_eq!(board_name("my-sensor_board"), "MySensorBoard");
        assert_eq!(board_name("2layer"), "Board2layer");
        assert_eq!(board_name("..."), "Board");
        assert_eq!(module_name("MySensorBoard"), "my-sensor-board");
    }

    #[test]
    fn the_new_project_passes_erc_offline_and_is_not_overwritten() {
        let directory = std::env::temp_dir().join(format!("copper-init-{}", std::process::id())).join("demo-board");
        let _ = std::fs::remove_dir_all(&directory);
        let call = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(&args, &mut out, &mut err);
            (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
        };
        let path = directory.to_string_lossy().into_owned();
        let (code, out, _) = call(&[&path]);
        assert_eq!(code, 0, "{out}");
        assert!(out.starts_with("Created DemoBoard"), "{out}");
        let design = copper_lang::compiler::load_design(&directory.join("board.copper"), true, true)
            .unwrap_or_else(|failure| panic!("{failure}"));
        assert_eq!(design.electrical.name, "DemoBoard");
        let diagnostics = copper_lang::erc::check(&design.electrical, true).unwrap();
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>());
        let (code, _, err) = call(&[&path]);
        assert_eq!(code, 2);
        assert!(err.contains("pass --force"), "{err}");
        assert_eq!(call(&[&path, "--force", "--name", "other"]).0, 0);
        let source = std::fs::read_to_string(directory.join("board.copper")).unwrap();
        assert!(source.contains("board Other {"), "{source}");
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }
}

//! `copper compile-macro SOURCE -o OUTPUT [--locked] [--offline]`.

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_lang::macros::{canonical_macro_bytes, compile_macro_file};
use copper_lang::paths;

use crate::option_parser::{self, flag, short_value};
use crate::pypath;

const USAGE: &str = "usage: copper compile-macro [-h] -o OUTPUT [--locked] [--offline] source";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let parsed =
        match option_parser::parse(arguments, &[short_value("--output", "-o"), flag("--locked"), flag("--offline")]) {
            Ok(parsed) => parsed,
            Err(message) => {
                let _ = writeln!(err, "{USAGE}\ncopper compile-macro: error: {message}");
                return 2;
            }
        };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\nlower a Copper macro to a verified JSON asset");
        crate::help::options(out, "compile-macro");
        return 0;
    }
    let output = parsed.value("--output").map(str::to_owned);
    let ([source], Some(output)) = (parsed.positionals.as_slice(), output) else {
        let _ = writeln!(
            err,
            "{USAGE}\ncopper compile-macro: error: the following arguments are required: source, -o/--output"
        );
        return 2;
    };
    let output_path = paths::resolve(Path::new(&output));
    let result = (|| -> Result<usize, String> {
        if output_path == paths::resolve(Path::new(source)) {
            return Err("macro output must differ from source".to_owned());
        }
        let (data, inputs) = compile_macro_file(Path::new(source), parsed.flag("--locked"), parsed.flag("--offline"))
            .map_err(|error| error.to_string())?;
        if inputs.keys().any(|input| Path::new(input) == output_path) {
            return Err("macro output cannot overwrite a source or footprint input".to_owned());
        }
        let display = pypath::display(&output);
        let os = |failure: std::io::Error| paths::python_os_error(&failure, Some(&display));
        if let Some(parent) = Path::new(&output).parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|failure| paths::python_os_call_error(&failure, Some(&display)))?;
        }
        std::fs::write(&output, canonical_macro_bytes(&data)).map_err(os)?;
        Ok(inputs.len())
    })();
    match result {
        Ok(count) => {
            let _ = text::print(
                out,
                &format!(
                    "Compiled macro {} -> {} ({count} verified inputs; unqualified)",
                    pypath::display(source),
                    pypath::display(&output)
                ),
            );
            0
        }
        Err(message) => {
            let _ = text::print(out, &format!("MACRO ERROR: {message}"));
            2
        }
    }
}

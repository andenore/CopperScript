//! `copper compile-macro SOURCE -o OUTPUT [--locked] [--offline]`.

use std::io::Write;
use std::path::Path;

use copper_json::text;
use copper_lang::macros::{canonical_macro_bytes, compile_macro_file};
use copper_lang::paths;

use crate::{args, pypath};

const USAGE: &str = "usage: copper compile-macro [-h] -o OUTPUT [--locked] [--offline] source";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut rest: Vec<String> = Vec::new();
    let mut output: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        let arg = &arguments[index];
        if arg == "-o" || (arg.len() > 3 && "--output".starts_with(arg.as_str()) && arg.starts_with("--o")) {
            let Some(value) = arguments.get(index + 1) else {
                let _ =
                    writeln!(err, "{USAGE}\ncopper compile-macro: error: argument -o/--output: expected one argument");
                return 2;
            };
            output = Some(value.clone());
            index += 2;
            continue;
        }
        match arg.strip_prefix("--output=").or_else(|| arg.strip_prefix("-o").filter(|v| !v.is_empty())) {
            Some(value) => output = Some(value.to_owned()),
            None => rest.push(arg.clone()),
        }
        index += 1;
    }
    let parsed = match args::parse(&rest, &["--help", "--locked", "--offline"]) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "{USAGE}\ncopper compile-macro: error: {message}");
            return 2;
        }
    };
    if parsed.flags.contains(&"--help") {
        let _ = writeln!(out, "{USAGE}\n\nlower a Copper macro to a verified JSON asset");
        return 0;
    }
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
        let (data, inputs) = compile_macro_file(
            Path::new(source),
            parsed.flags.contains(&"--locked"),
            parsed.flags.contains(&"--offline"),
        )
        .map_err(|error| error.to_string())?;
        if inputs.keys().any(|input| Path::new(input) == output_path) {
            return Err("macro output cannot overwrite a source or footprint input".to_owned());
        }
        let display = pypath::display(&output);
        let os = |failure: std::io::Error| paths::python_os_error(&failure, Some(&display));
        if let Some(parent) = Path::new(&output).parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(os)?;
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

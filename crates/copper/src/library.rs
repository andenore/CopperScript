//! `copper library {index,search,inspect,diagnostics}`: compact library discovery.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_json::text;
use copper_lang::library_context::{
    KINDS, PageOptions, SECTIONS, diagnostics, index_library, inspect, project_catalog, search, to_json, write_index,
};
use copper_lang::packages::{find_manifest, read_manifest};
use copper_lang::paths;

use crate::option_parser::{self, Spec, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper library [-h] {index,search,inspect,diagnostics} ...";

/// Parsed options of one `library` action.
type Options = option_parser::Parsed;

/// One action's options, parsed by the shared parser (`-o` is `--output`).
fn parse(arguments: &[String], valued: &[&'static str], flags: &[&'static str]) -> Result<Options, String> {
    let mut specs: Vec<Spec> = flags.iter().map(|name| flag(name)).collect();
    for name in valued {
        specs.push(if *name == "--output" { short_value(name, "-o") } else { value(name) });
    }
    option_parser::parse(arguments, &specs)
}

fn integer(options: &Options, name: &str, default: i64) -> Result<i64, String> {
    match options.value(name) {
        None => Ok(default),
        Some(text) => text.trim().parse().map_err(|_| format!("argument {name}: invalid int value: '{text}'")),
    }
}

fn choice<'a>(options: &'a Options, name: &str, choices: &[&str]) -> Result<Option<&'a str>, String> {
    match options.value(name) {
        Some(value) if !choices.contains(&value) => Err(format!(
            "argument {name}: invalid choice: '{value}' (choose from {})",
            choices.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(", ")
        )),
        other => Ok(other),
    }
}

fn library_error(out: &mut dyn Write, message: &str) -> u8 {
    let _ = text::print(out, &format!("LIBRARY ERROR: {}", message.chars().take(800).collect::<String>()));
    2
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(action) = arguments.first().map(String::as_str) else {
        let _ = writeln!(err, "{USAGE}\ncopper library: error: the following arguments are required: library_action");
        return 2;
    };
    if action == "-h" || action == "--help" {
        let _ = writeln!(out, "{USAGE}\n\ncompact discovery and inspection across declared part libraries");
        crate::help::options(out, "library");
        return 0;
    }
    let rest = &arguments[1..];
    let (valued, flags): (Vec<&'static str>, Vec<&'static str>) = match action {
        "index" => (vec!["--output"], vec!["--help"]),
        "search" | "inspect" | "diagnostics" => {
            let mut valued = vec!["--project", "--limit", "--offset", "--max-chars"];
            match action {
                "search" => valued.extend(["--provider", "--kind"]),
                "inspect" => valued.extend(["--kind", "--section", "--expect-digest", "--member"]),
                _ => {}
            }
            (valued, vec!["--help", "--fetch", "--update-lock", "--json"])
        }
        other => {
            let _ = writeln!(
                err,
                "{USAGE}\ncopper library: error: argument library_action: invalid choice: '{other}' (choose from 'index', 'search', 'inspect', 'diagnostics')"
            );
            return 2;
        }
    };
    let usage = format!("usage: copper library {action} [-h] ...");
    let options = match parse(rest, &valued, &flags) {
        Ok(options) => options,
        Err(message) => {
            let _ = writeln!(err, "{usage}\ncopper library {action}: error: {message}");
            return 2;
        }
    };
    if options.flag("--help") {
        let _ = writeln!(out, "{usage}");
        crate::help::action_options(out, "library", action);
        return 0;
    }
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{usage}\ncopper library {action}: error: {message}");
        2
    };
    if action == "index" {
        let ([root], Some(output)) = (options.positionals.as_slice(), options.value("--output")) else {
            return usage_error(err, "the following arguments are required: root, -o/--output");
        };
        let (root, output) = (PathBuf::from(root), PathBuf::from(output));
        if paths::is_relative_to(&paths::resolve(&output), &paths::resolve(&root)) {
            return library_error(out, "index output must stay outside the indexed library root");
        }
        let result = index_library(&root).and_then(|catalog| write_index(&catalog, &output).map(|()| catalog));
        return match result {
            Ok(catalog) => {
                let _ = text::print(
                    out,
                    &format!(
                        "Library index: {} definitions; {} diagnostics -> {}",
                        catalog.entries.len(),
                        catalog.diagnostics.len(),
                        pypath::display(&output.to_string_lossy())
                    ),
                );
                u8::from(!catalog.complete)
            }
            Err(message) => library_error(out, &message),
        };
    }
    let positional_limit = match action {
        "search" => 1,
        "inspect" => 1,
        _ => 0,
    };
    if options.positionals.len() > positional_limit {
        return usage_error(
            err,
            &format!("unrecognized arguments: {}", options.positionals[positional_limit..].join(" ")),
        );
    }
    if action == "inspect" && options.positionals.is_empty() {
        return usage_error(err, "the following arguments are required: selector");
    }
    let parsed = (|| -> Result<(PageOptions, Option<&str>, &str), String> {
        let default_limit = if action == "search" { 5 } else { 20 };
        let page = PageOptions {
            offset: integer(&options, "--offset", 0)?,
            limit: integer(&options, "--limit", default_limit)?,
            max_chars: integer(&options, "--max-chars", 8000)?,
        };
        let kind = choice(&options, "--kind", &KINDS)?;
        let section = choice(&options, "--section", &SECTIONS)?.unwrap_or("overview");
        Ok((page, kind, section))
    })();
    let (page, kind, section) = match parsed {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    let current = std::env::current_dir().unwrap_or_default();
    let project = options.value("--project").map(PathBuf::from).unwrap_or(current);
    let result = (|| -> Result<(String, bool), String> {
        let mut providers = options.owned_values("--provider");
        if action == "inspect" {
            let selector = &options.positionals[0];
            let Some(manifest_path) = find_manifest(&paths::resolve(&project)) else {
                return Err("library discovery requires a project copper.mod".to_owned());
            };
            let manifest = read_manifest(&manifest_path).map_err(|e| e.to_string())?;
            let package = selector.split("::").next().unwrap_or_default();
            let mut best: Option<&String> = None;
            for module in manifest.requirements.keys() {
                if (package == module || package.starts_with(&format!("{module}/")))
                    && best.is_none_or(|b| module.len() > b.len())
                {
                    best = Some(module);
                }
            }
            let Some(best) = best else {
                return Err("selected package has no requirement in the project copper.mod".to_owned());
            };
            providers = vec![best.clone()];
        }
        let locked = !options.flag("--update-lock");
        let offline = !options.flag("--fetch");
        let mut catalog = project_catalog(Path::new(&project), locked, offline, &providers)?;
        let page = match action {
            "search" => search(&catalog, options.positionals.first().map_or("", String::as_str), kind, page)?,
            "inspect" => {
                let members = options.owned_values("--member");
                let selector = options.positionals[0].clone();
                inspect(&mut catalog, &selector, kind, section, page, options.value("--expect-digest"), &members)?
            }
            _ => diagnostics(&catalog, page)?,
        };
        Ok((to_json(&page), catalog.complete))
    })();
    match result {
        Ok((rendered, complete)) => {
            let _ = text::print(out, &rendered);
            u8::from(!complete)
        }
        Err(message) => library_error(out, &message),
    }
}

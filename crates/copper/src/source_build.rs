//! Build settings from the board source, as command line options.
//!
//! A board's `build` block holds defaults for options of the physical
//! commands. A command reads the block from its entry source, keeps the
//! options it accepts and evaluates them before its own arguments, so the
//! command line overrides the source by the usual last-one-wins rule.
//!
//! Only the entry file's syntax is read here: the block can only be in the
//! top-level board file, so no import is resolved. A file that cannot be read
//! or does not parse yields no options; the command's own load reports it.

use std::path::Path;

use copper_lang::build_settings::{BuildSettings, ROUTER_OPTION, lower_build};
use copper_lang::compiler::read_source;
use copper_lang::{CopperScriptError, close_match, parse, paths};
use copper_route::bench::UNIFIED_OPTION_FIELDS;

use crate::option_parser::{Parsed, Spec};

/// The first `router` setting of `settings` that is not a unified router
/// field, as an error at its key.
pub fn router_key_error(settings: &BuildSettings) -> Option<CopperScriptError> {
    let unknown = settings.router().find(|setting| !UNIFIED_OPTION_FIELDS.contains(&setting.key.as_str()))?;
    let mut error = CopperScriptError::new(
        "BLD003",
        format!(
            "unknown router setting {}; copper route-board --list-unified-options lists them",
            copper_json::python_str_repr(&unknown.key)
        ),
        unknown.location.clone(),
    );
    let suggestion = close_match(&unknown.key, &UNIFIED_OPTION_FIELDS);
    error.hint = suggestion.as_ref().map(|name| format!("did you mean {}?", copper_json::python_str_repr(name)));
    error.replacement = suggestion;
    Some(error)
}

/// The checked `build` block of the board file `path`, read by syntax alone.
/// `None` when the file cannot be read, does not parse, declares no block or
/// its block is invalid: the command's own load reports those.
fn source_settings(path: &Path) -> Option<BuildSettings> {
    let resolved = paths::resolve(path);
    let source = read_source(&resolved).ok()?;
    let document = parse(&source, &paths::display(&resolved)).ok()?;
    lower_build(&document).ok()?
}

/// The option tokens the `build` block of the board `path` gives a command
/// with the options `specs`: the settings whose option the command accepts,
/// in source order. An unknown `router` key is an error for a command that
/// takes `--unified-set`.
pub fn source_options(path: &Path, specs: &[Spec]) -> Result<Vec<String>, CopperScriptError> {
    let Some(settings) = source_settings(path) else { return Ok(Vec::new()) };
    let accepts = |option: &str| specs.iter().any(|spec| spec.long == option);
    if accepts(ROUTER_OPTION)
        && let Some(error) = router_key_error(&settings)
    {
        return Err(error);
    }
    Ok(settings
        .options()
        .into_iter()
        .filter(|option| accepts(&option.option))
        .flat_map(|option| option.tokens)
        .collect())
}

/// A command line with the source's options in front.
pub struct Merged {
    /// The source options, then the arguments as given.
    pub arguments: Vec<String>,
    /// The options taken from the source; empty when there are none.
    pub source: Vec<String>,
}

/// `arguments` with the options of the board's `build` block in front.
/// `parse` is the command's own parser; `None` from it (a usage error, or
/// help) leaves the arguments as they are, as does anything but exactly one
/// positional argument, the board.
pub fn merged(
    arguments: &[String],
    specs: &[Spec],
    parse: impl Fn(&[String], &[Spec]) -> Option<Parsed>,
) -> Result<Merged, CopperScriptError> {
    let unchanged = || Merged { arguments: arguments.to_vec(), source: Vec::new() };
    let Some(parsed) = parse(arguments, specs) else { return Ok(unchanged()) };
    let [board] = parsed.positionals.as_slice() else { return Ok(unchanged()) };
    if parsed.help() {
        return Ok(unchanged());
    }
    let source = source_options(Path::new(&crate::pypath::display(board)), specs)?;
    let mut all = source.clone();
    all.extend(arguments.iter().cloned());
    Ok(Merged { arguments: all, source })
}

/// [`merged`] for a command without `--unified-set`, which has no source
/// error of its own to report: the arguments with the source options in front.
pub fn with_source_options(
    arguments: &[String],
    specs: &[Spec],
    parse: impl Fn(&[String], &[Spec]) -> Option<Parsed>,
) -> Vec<String> {
    merged(arguments, specs, parse).map_or_else(|_| arguments.to_vec(), |merged| merged.arguments)
}

/// The line a command prints when it used source options.
pub fn notice(board: &str, source: &[String]) -> String {
    let name =
        Path::new(board).file_name().map_or_else(|| board.to_owned(), |name| name.to_string_lossy().into_owned());
    format!("Build settings from {name}: {}", source.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::option_parser::{self, flag, value};

    fn board(name: &str, body: &str) -> String {
        let directory = std::env::temp_dir().join(format!("copper-source-build-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("board.copper");
        std::fs::write(&path, format!("board B {{\n{body}\n}}\n")).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn exact(arguments: &[String], specs: &[Spec]) -> Option<Parsed> {
        option_parser::parse_exact(arguments, specs).ok()
    }

    const BLOCK: &str = "build {\n fab_profile = \"jlcpcb-four-layer\";\n placement { candidates = 2; }\n \
                         routing { fanout = false; }\n router { direct = true; }\n}";

    #[test]
    fn a_command_takes_the_options_it_accepts() {
        let path = board("accepts", BLOCK);
        let narrow = [value("--fab-profile"), flag("--json")];
        assert_eq!(source_options(Path::new(&path), &narrow).unwrap(), ["--fab-profile", "jlcpcb-four-layer"]);
        let wide = [value("--fab-profile"), value("--candidates"), flag("--no-fanout"), value("--unified-set")];
        assert_eq!(
            source_options(Path::new(&path), &wide).unwrap().join(" "),
            "--fab-profile jlcpcb-four-layer --candidates 2 --no-fanout --unified-set direct=true"
        );
    }

    #[test]
    fn source_options_come_before_the_command_line() {
        let path = board("merged", BLOCK);
        let specs = [value("--fab-profile"), value("--candidates")];
        let arguments = vec![path.clone(), "--candidates".to_owned(), "5".to_owned()];
        let merged = merged(&arguments, &specs, exact).unwrap();
        assert_eq!(merged.source, ["--fab-profile", "jlcpcb-four-layer", "--candidates", "2"]);
        let parsed = option_parser::parse_exact(&merged.arguments, &specs).unwrap();
        assert_eq!(parsed.value("--candidates"), Some("5"));
        assert_eq!(parsed.value("--fab-profile"), Some("jlcpcb-four-layer"));
        assert_eq!(
            notice(&path, &merged.source),
            "Build settings from board.copper: --fab-profile jlcpcb-four-layer --candidates 2"
        );
    }

    #[test]
    fn unreadable_invalid_and_absent_blocks_give_no_options() {
        let specs = [value("--fab-profile"), value("--unified-set")];
        assert!(source_options(Path::new("no-such-board.copper"), &specs).unwrap().is_empty());
        let broken = board("broken", "build { fab_profile = ");
        assert!(source_options(Path::new(&broken), &specs).unwrap().is_empty());
        let invalid = board("invalid", "build { fab_profile = \"nobody\"; }");
        assert!(source_options(Path::new(&invalid), &specs).unwrap().is_empty());
        let none = board("none", "");
        let arguments = vec![none, "--help".to_owned()];
        assert_eq!(merged(&arguments, &specs, exact).unwrap().arguments, arguments);
    }

    #[test]
    fn an_unknown_router_key_is_located() {
        let path = board("router", "build {\n router {\n  maximum_expansion = 5;\n }\n}");
        let error = source_options(Path::new(&path), &[value("--unified-set")]).unwrap_err();
        assert_eq!((error.code.as_str(), error.location.line, error.location.column), ("BLD003", 4, 3));
        assert_eq!(
            error.message,
            "unknown router setting 'maximum_expansion'; copper route-board --list-unified-options lists them"
        );
        assert_eq!(error.hint.as_deref(), Some("did you mean 'maximum_expansions'?"));
        // A command without --unified-set does not look at the router group.
        assert!(source_options(Path::new(&path), &[value("--fab-profile")]).unwrap().is_empty());
    }
}

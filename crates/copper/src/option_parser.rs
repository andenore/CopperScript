//! The command line parser every command uses.
//!
//! The conventions follow argparse, which the commands were first written
//! against: long options match any unambiguous prefix (unless a command parses
//! exactly), take values as `--name VALUE` or `--name=VALUE` (short ones also
//! as `-xVALUE` and `-x=VALUE`), may repeat, and mix freely with positionals;
//! `--` ends the options. An argument that looks like a negative number
//! (`-3`, `-.5`) is a value, not an option. Errors are usage errors (exit
//! status 2). `--help`/`-h` is always accepted.

/// One option of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The long name, `--output`.
    pub long: &'static str,
    /// An optional short alias, `-o`.
    pub short: Option<&'static str>,
    /// Whether the option takes a value.
    pub takes_value: bool,
}

pub const fn flag(long: &'static str) -> Spec {
    Spec { long, short: None, takes_value: false }
}

pub const fn value(long: &'static str) -> Spec {
    Spec { long, short: None, takes_value: true }
}

pub const fn short_value(long: &'static str, short: &'static str) -> Spec {
    Spec { long, short: Some(short), takes_value: true }
}

/// The parsed command line: options in order of appearance, then positionals.
#[derive(Default, Debug)]
pub struct Parsed {
    pub options: Vec<(&'static str, Option<String>)>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn flag(&self, long: &str) -> bool {
        self.options.iter().any(|(name, _)| *name == long)
    }

    /// Whether help was asked for.
    pub fn help(&self) -> bool {
        self.flag("--help")
    }

    /// The last value given for `long`.
    pub fn value(&self, long: &str) -> Option<&str> {
        self.options.iter().rev().find(|(name, _)| *name == long).and_then(|(_, value)| value.as_deref())
    }

    /// Every value given for a repeatable option, in order.
    pub fn values(&self, long: &str) -> Vec<&str> {
        self.options.iter().filter(|(name, _)| *name == long).filter_map(|(_, value)| value.as_deref()).collect()
    }

    /// `values` as owned strings.
    pub fn owned_values(&self, long: &str) -> Vec<String> {
        self.values(long).into_iter().map(str::to_owned).collect()
    }
}

/// argparse's `^-\d+$|^-\d*\.\d+$`: such an argument is a value, not an option.
pub fn looks_negative(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix('-') else { return false };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits(rest)
        || rest
            .split_once('.')
            .is_some_and(|(whole, fraction)| whole.bytes().all(|b| b.is_ascii_digit()) && digits(fraction))
}

/// How a command's parser treats abbreviations and unknown arguments.
#[derive(Clone, Copy)]
struct Config {
    /// Long options match an unambiguous prefix (argparse's `allow_abbrev`).
    abbreviations: bool,
    /// An unknown dashed argument containing a space is a positional, as
    /// argparse does (the report commands take free-text selectors).
    spaced_unknown_is_positional: bool,
}

/// Parse `arguments` against `specs`; long options may be abbreviated.
pub fn parse(arguments: &[String], specs: &[Spec]) -> Result<Parsed, String> {
    parse_with(arguments, specs, Config { abbreviations: true, spaced_unknown_is_positional: false })
}

/// `parse` without abbreviations: long options match exactly.
pub fn parse_exact(arguments: &[String], specs: &[Spec]) -> Result<Parsed, String> {
    parse_with(arguments, specs, Config { abbreviations: false, spaced_unknown_is_positional: false })
}

/// `parse` for the report commands (`explain`, `issues`, `view`, `summarize`,
/// the editors): an unknown dashed argument with a space is a positional, and
/// `Ok(None)` means help was asked for.
pub fn parse_report(arguments: &[String], specs: &[Spec]) -> Result<Option<Parsed>, String> {
    let parsed = parse_with(arguments, specs, Config { abbreviations: true, spaced_unknown_is_positional: true })?;
    Ok((!parsed.help()).then_some(parsed))
}

const HELP: Spec = flag("--help");

enum Unresolved {
    Unknown,
    Message(String),
}

/// The option `arg` names, and its inline value.
fn resolve(all: &[Spec], arg: &str, config: Config) -> Result<(Spec, Option<String>), Unresolved> {
    if arg == "-h" {
        return Ok((HELP, None));
    }
    if arg.starts_with("--") {
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key, Some(value.to_owned())),
            None => (arg, None),
        };
        if let Some(spec) = all.iter().find(|spec| spec.long == key) {
            return Ok((*spec, inline));
        }
        let matches: Vec<&Spec> =
            all.iter().filter(|spec| config.abbreviations && spec.long.starts_with(key)).collect();
        return match matches.as_slice() {
            [spec] => Ok((**spec, inline)),
            [] => Err(Unresolved::Unknown),
            _ => {
                let names: Vec<&str> = matches.iter().map(|spec| spec.long).collect();
                Err(Unresolved::Message(format!("ambiguous option: {key} could match {}", names.join(", "))))
            }
        };
    }
    let short = &arg[..arg.char_indices().nth(2).map_or(arg.len(), |(index, _)| index)];
    let spec = all.iter().find(|spec| spec.short == Some(short)).ok_or(Unresolved::Unknown)?;
    let rest = &arg[short.len()..];
    Ok((*spec, (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest).to_owned())))
}

fn parse_with(arguments: &[String], specs: &[Spec], config: Config) -> Result<Parsed, String> {
    let mut all: Vec<Spec> = vec![HELP];
    all.extend(specs.iter().filter(|spec| spec.long != HELP.long));
    let is_option = |arg: &str| arg.starts_with('-') && arg != "-" && !looks_negative(arg);
    let mut parsed = Parsed::default();
    let mut index = 0;
    let mut only_positionals = false;
    while index < arguments.len() {
        let arg = &arguments[index];
        index += 1;
        if only_positionals || !is_option(arg) {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (spec, inline) = match resolve(&all, arg, config) {
            Ok(found) => found,
            Err(Unresolved::Unknown) if config.spaced_unknown_is_positional && arg.contains(' ') => {
                parsed.positionals.push(arg.clone());
                continue;
            }
            Err(Unresolved::Unknown) => return Err(format!("unrecognized arguments: {arg}")),
            Err(Unresolved::Message(message)) => return Err(message),
        };
        if !spec.takes_value {
            if inline.is_some() {
                return Err(format!("argument {}: ignored explicit argument", spec.long));
            }
            parsed.options.push((spec.long, None));
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => match arguments.get(index) {
                Some(value) if !is_option(value) => {
                    index += 1;
                    value.clone()
                }
                _ => return Err(format!("argument {}: expected one argument", spec.long)),
            },
        };
        parsed.options.push((spec.long, Some(value)));
    }
    Ok(parsed)
}

/// Python's `int(text)` for decimal text: surrounding whitespace, a sign and
/// single underscores between digits.
pub fn python_int(text: &str) -> Option<i64> {
    let body = text.trim_matches(copper_json::python_isspace);
    let (negative, digits) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Python ints are unbounded; this saturates, which no option here can tell apart.
    let magnitude: i64 = clean.parse().unwrap_or(i64::MAX);
    Some(if negative { -magnitude } else { magnitude })
}

/// Python's `float(text)`: whitespace, `inf`/`nan` spellings and underscores
/// between digits.
pub fn python_float(text: &str) -> Option<f64> {
    let body = text.trim_matches(copper_json::python_isspace);
    let bytes = body.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_' {
            let digit = |at: Option<&u8>| at.is_some_and(u8::is_ascii_digit);
            if index == 0 || !digit(bytes.get(index - 1)) || !digit(bytes.get(index + 1)) {
                return None;
            }
        }
    }
    let clean: String = body.chars().filter(|c| *c != '_').collect();
    let unsigned = clean.trim_start_matches(['+', '-']);
    if clean.len() - unsigned.len() > 1 {
        return None;
    }
    let lower = unsigned.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        return clean.parse().ok();
    }
    if !unsigned.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    clean.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn prefixes_inline_values_and_positionals() {
        let specs = [flag("--locked"), short_value("--output", "-o"), value("--layers")];
        let parsed = parse(&strings(&["b.copper", "--lock", "-ox", "--lay=4", "--", "-z"]), &specs).unwrap();
        assert!(parsed.flag("--locked"));
        assert_eq!(parsed.value("--output"), Some("x"));
        assert_eq!(parsed.value("--layers"), Some("4"));
        assert_eq!(parsed.positionals, ["b.copper", "-z"]);
        assert!(parse(&strings(&["--output"]), &specs).is_err());
        assert!(parse(&strings(&["--x"]), &specs).is_err());
    }

    #[test]
    fn exact_parsing_rejects_prefixes() {
        let specs = [flag("--locked")];
        assert!(parse_exact(&strings(&["--lock"]), &specs).is_err());
        assert!(parse_exact(&strings(&["--locked"]), &specs).unwrap().flag("--locked"));
    }

    #[test]
    fn report_parsing_takes_spaced_unknowns_as_positionals_and_signals_help() {
        let specs = [flag("--json"), value("--net"), value("--limit"), short_value("--output", "-o")];
        let parsed =
            parse_report(&strings(&["run", "--js", "--ne=A", "--limit", "-1", "-oout.png", "--", "--json"]), &specs)
                .unwrap()
                .unwrap();
        assert!(parsed.flag("--json"));
        assert_eq!(parsed.value("--net"), Some("A"));
        assert_eq!(parsed.value("--limit"), Some("-1"));
        assert_eq!(parsed.value("--output"), Some("out.png"));
        assert_eq!(parsed.positionals, ["run", "--json"]);
        let spaced = parse_report(&strings(&["-x y"]), &specs).unwrap().unwrap();
        assert_eq!(spaced.positionals, ["-x y"]);
        assert!(parse(&strings(&["-x y"]), &specs).is_err());
        assert!(parse_report(&strings(&["-h"]), &specs).unwrap().is_none());
    }

    #[test]
    fn negative_numbers_are_values_and_flags_take_no_value() {
        let specs = [value("--cost"), flag("--json")];
        assert_eq!(parse(&strings(&["--cost", "-.5"]), &specs).unwrap().value("--cost"), Some("-.5"));
        assert_eq!(parse(&strings(&["-3"]), &specs).unwrap().positionals, ["-3"]);
        assert!(parse(&strings(&["--json=1"]), &specs).is_err());
        let ambiguous = parse(&strings(&["--"]), &[]).unwrap();
        assert!(ambiguous.positionals.is_empty());
        let both = [flag("--json"), flag("--jobs")];
        assert_eq!(parse(&strings(&["--j"]), &both).unwrap_err(), "ambiguous option: --j could match --json, --jobs");
    }

    #[test]
    fn numbers_parse_like_python() {
        assert_eq!(python_int(" +1_5 "), Some(15));
        assert_eq!(python_int("1__5"), None);
        assert_eq!(python_int("1.0"), None);
        assert_eq!(python_float("1_0.5"), Some(10.5));
        assert_eq!(python_float(" -Infinity "), Some(f64::NEG_INFINITY));
        assert_eq!(python_float("1e"), None);
        assert_eq!(python_float("."), None);
    }
}

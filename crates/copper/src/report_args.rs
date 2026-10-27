//! argparse's option conventions for the M5 report commands (`explain`,
//! `issues`, `view`, `summarize`).
//!
//! Long options may be abbreviated to any unambiguous prefix and take their
//! value as `--option VALUE` or `--option=VALUE`; a short option takes
//! `-o VALUE`, `-oVALUE` or `-o=VALUE`. `--` ends the options, and an argument
//! that looks like a negative number is a value. Usage errors exit 2; their
//! wording is not argparse's.

/// One option of a command: its long name, an optional short alias, and
/// whether it takes a value.
pub struct Opt {
    pub long: &'static str,
    pub short: Option<&'static str>,
    pub valued: bool,
}

pub const fn flag(long: &'static str) -> Opt {
    Opt { long, short: None, valued: false }
}

pub const fn valued(long: &'static str) -> Opt {
    Opt { long, short: None, valued: true }
}

/// The options given, in command-line order, and the positionals.
#[derive(Default)]
pub struct Parsed {
    pub options: Vec<(&'static str, Option<String>)>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn has(&self, long: &str) -> bool {
        self.options.iter().any(|(name, _)| *name == long)
    }

    /// The last value given for `long` (argparse's `store`).
    pub fn last(&self, long: &str) -> Option<&str> {
        self.options.iter().rev().find(|(name, _)| *name == long).and_then(|(_, value)| value.as_deref())
    }

    /// Every value given for `long`, in order (argparse's `append`).
    pub fn all(&self, long: &str) -> Vec<String> {
        self.options.iter().filter(|(name, _)| *name == long).filter_map(|(_, value)| value.clone()).collect()
    }
}

/// argparse's `^-\d+$|^-\d*\.\d+$`: such an argument is a value, not an option.
fn looks_negative(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix('-') else { return false };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits(rest)
        || rest
            .split_once('.')
            .is_some_and(|(whole, fraction)| whole.bytes().all(|b| b.is_ascii_digit()) && digits(fraction))
}

const HELP: Opt = flag("--help");

enum Unresolved {
    Unknown,
    Message(String),
}

/// The option `arg` names, and its inline value.
fn resolve<'a>(options: &'a [Opt], arg: &str) -> Result<(&'a Opt, Option<String>), Unresolved> {
    let all = || std::iter::once(&HELP).chain(options.iter());
    if arg == "-h" {
        return Ok((&HELP, None));
    }
    if let Some(body) = arg.strip_prefix("--") {
        let (key, inline) = match body.split_once('=') {
            Some((key, value)) => (key, Some(value.to_owned())),
            None => (body, None),
        };
        if let Some(option) = all().find(|option| option.long[2..] == *key) {
            return Ok((option, inline));
        }
        let matches: Vec<&Opt> = all().filter(|option| option.long[2..].starts_with(key)).collect();
        return match matches.as_slice() {
            [option] => Ok((option, inline)),
            [] => Err(Unresolved::Unknown),
            _ => Err(Unresolved::Message(format!("ambiguous option: --{key}"))),
        };
    }
    let short = &arg[..arg.char_indices().nth(2).map_or(arg.len(), |(index, _)| index)];
    let option = options.iter().find(|option| option.short == Some(short)).ok_or(Unresolved::Unknown)?;
    let rest = &arg[short.len()..];
    Ok((option, (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest).to_owned())))
}

/// Parse `args`; `Ok(None)` means help was asked for.
pub fn parse(args: &[String], options: &[Opt]) -> Result<Option<Parsed>, String> {
    let mut parsed = Parsed::default();
    let mut index = 0;
    let mut only_positionals = false;
    let is_option = |arg: &str| arg.starts_with('-') && arg != "-" && !looks_negative(arg);
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if only_positionals || !is_option(arg) {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (option, inline) = match resolve(options, arg) {
            Ok(found) => found,
            // argparse takes an unknown dashed argument with a space for a positional.
            Err(Unresolved::Unknown) if arg.contains(' ') => {
                parsed.positionals.push(arg.clone());
                continue;
            }
            Err(Unresolved::Unknown) => return Err(format!("unrecognized arguments: {arg}")),
            Err(Unresolved::Message(message)) => return Err(message),
        };
        if option.long == "--help" {
            return Ok(None);
        }
        if !option.valued {
            if inline.is_some() {
                return Err(format!("argument {}: ignored explicit argument", option.long));
            }
            parsed.options.push((option.long, None));
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => match args.get(index) {
                Some(value) if !is_option(value) => {
                    index += 1;
                    value.clone()
                }
                _ => return Err(format!("argument {}: expected one argument", option.long)),
            },
        };
        parsed.options.push((option.long, Some(value)));
    }
    Ok(Some(parsed))
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

    const OPTIONS: &[Opt] = &[
        flag("--json"),
        valued("--net"),
        valued("--limit"),
        Opt { long: "--output", short: Some("-o"), valued: true },
    ];

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn abbreviations_values_and_separator() {
        let parsed = parse(&strings(&["run", "--js", "--ne=A", "--limit", "-1", "-oout.png", "--", "--json"]), OPTIONS)
            .unwrap()
            .unwrap();
        assert!(parsed.has("--json"));
        assert_eq!(parsed.last("--net"), Some("A"));
        assert_eq!(parsed.last("--limit"), Some("-1"));
        assert_eq!(parsed.last("--output"), Some("out.png"));
        assert_eq!(parsed.positionals, ["run", "--json"]);
        assert!(parse(&strings(&["--net"]), OPTIONS).is_err());
        assert!(parse(&strings(&["--x"]), OPTIONS).is_err());
        assert!(parse(&strings(&["-h"]), OPTIONS).unwrap().is_none());
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

//! argparse's command-line conventions for one subcommand.
//!
//! Long options match any unambiguous prefix, take values as `--name VALUE`
//! or `--name=VALUE` (short ones also as `-xVALUE`), may repeat, and mix
//! freely with positionals; `--` ends the options. A value may not start
//! with `-` unless it looks like a negative number. Errors are usage errors
//! (exit status 2); their wording is not argparse's (plan, section 9).

/// One option of a subcommand.
#[derive(Clone, Copy)]
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
#[derive(Default)]
pub struct Parsed {
    pub options: Vec<(&'static str, Option<String>)>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn flag(&self, long: &str) -> bool {
        self.options.iter().any(|(name, _)| *name == long)
    }

    /// The last value given for `long`.
    pub fn value(&self, long: &str) -> Option<&str> {
        self.options.iter().rev().find(|(name, _)| *name == long).and_then(|(_, value)| value.as_deref())
    }

    /// Every value given for a repeatable option.
    pub fn values(&self, long: &str) -> Vec<&str> {
        self.options.iter().filter(|(name, _)| *name == long).filter_map(|(_, value)| value.as_deref()).collect()
    }
}

fn looks_negative(text: &str) -> bool {
    text.starts_with('-') && text[1..].parse::<f64>().is_ok()
}

/// Parse `arguments` against `specs` (`--help`/`-h` is always accepted).
pub fn parse(arguments: &[String], specs: &[Spec]) -> Result<Parsed, String> {
    let help = flag("--help");
    let mut all: Vec<Spec> = specs.to_vec();
    all.push(help);
    let mut parsed = Parsed::default();
    let mut index = 0;
    let mut only_positionals = false;
    while index < arguments.len() {
        let arg = &arguments[index];
        index += 1;
        if only_positionals || !arg.starts_with('-') || arg == "-" || looks_negative(arg) {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (spec, inline) = if arg.starts_with("--") {
            let (key, inline) = match arg.split_once('=') {
                Some((key, value)) => (key, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            let exact = all.iter().find(|spec| spec.long == key);
            let matches: Vec<&Spec> = all.iter().filter(|spec| spec.long.starts_with(key)).collect();
            let spec = match (exact, matches.as_slice()) {
                (Some(spec), _) => *spec,
                (None, [spec]) => **spec,
                (None, []) => return Err(format!("unrecognized arguments: {arg}")),
                (None, _) => {
                    let names: Vec<&str> = matches.iter().map(|spec| spec.long).collect();
                    return Err(format!("ambiguous option: {key} could match {}", names.join(", ")));
                }
            };
            (spec, inline)
        } else if arg == "-h" {
            (help, None)
        } else {
            let short = &arg[..2.min(arg.len())];
            let Some(spec) = all.iter().find(|spec| spec.short == Some(short)) else {
                return Err(format!("unrecognized arguments: {arg}"));
            };
            let rest = &arg[short.len()..];
            let inline = (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest).to_owned());
            (*spec, inline)
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
                Some(value) if !value.starts_with('-') || looks_negative(value) || value == "-" => {
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
}

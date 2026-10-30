//! Just enough of argparse's conventions for the M0 commands.
//!
//! Long options may be abbreviated to any unambiguous prefix, `--` ends the
//! options, and errors exit with status 2. Usage and help text are not
//! argparse's.

/// Split `args` into recognised long options and positionals.
pub struct Parsed {
    pub flags: Vec<&'static str>,
    pub positionals: Vec<String>,
}

pub fn parse(args: &[String], options: &[&'static str]) -> Result<Parsed, String> {
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut only_positionals = false;
    for arg in args {
        if only_positionals || !arg.starts_with('-') || arg == "-" {
            positionals.push(arg.clone());
        } else if arg == "--" {
            only_positionals = true;
        } else if arg == "-h" {
            flags.push("--help");
        } else {
            let matches: Vec<&'static str> =
                options.iter().copied().filter(|option| option.starts_with(arg.as_str())).collect();
            match matches.as_slice() {
                [option] => flags.push(option),
                _ if options.contains(&arg.as_str()) => flags.push(options.iter().copied().find(|o| o == arg).unwrap()),
                [] => return Err(format!("unrecognized arguments: {arg}")),
                _ => return Err(format!("ambiguous option: {arg} could match {}", matches.join(", "))),
            }
        }
    }
    Ok(Parsed { flags, positionals })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_separator() {
        let args: Vec<String> = ["--js", "syntax", "--", "--json"].iter().map(|a| a.to_string()).collect();
        let parsed = parse(&args, &["--help", "--json"]).unwrap();
        assert_eq!(parsed.flags, ["--json"]);
        assert_eq!(parsed.positionals, ["syntax", "--json"]);
        assert!(parse(&["--x".to_owned()], &["--help", "--json"]).is_err());
    }
}

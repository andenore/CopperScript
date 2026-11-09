//! Help text for every command: the grouped top-level list, each command's
//! one-line summary and its options.
//!
//! A command's own `--help` prints its usage line and summary as before, then
//! calls [`options`] for the option list kept here. A test checks that every
//! option a command's source accepts is documented in its entry, so help and
//! behaviour cannot drift apart.

use std::io::Write;

mod build;
mod manufacture;
mod review;
mod route;

/// One option: how it is spelled (with its metavariable) and what it does.
pub struct Opt {
    /// `-o, --output OUTPUT`, `--layers {2,4,6}`, `--locked`.
    pub flag: &'static str,
    /// One or two sentences; defaults as `(default: 5)`.
    pub help: &'static str,
}

/// A subcommand of a command with actions (`library search`, `assembly bom`).
pub struct Action {
    pub name: &'static str,
    pub summary: &'static str,
    pub options: &'static [Opt],
}

/// Where a command sits in the top-level help.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Start,
    Source,
    Layout,
    Review,
    Manufacture,
    Benchmark,
}

impl Group {
    pub const ALL: [Group; 6] =
        [Group::Start, Group::Source, Group::Layout, Group::Review, Group::Manufacture, Group::Benchmark];

    pub fn title(self) -> &'static str {
        match self {
            Group::Start => "Start here",
            Group::Source => "Write and check source",
            Group::Layout => "Place, route and edit",
            Group::Review => "Review a run",
            Group::Manufacture => "Export and manufacture",
            Group::Benchmark => "Benchmarks",
        }
    }
}

/// One command's help.
pub struct Command {
    pub name: &'static str,
    pub group: Group,
    pub summary: &'static str,
    pub options: &'static [Opt],
    pub actions: &'static [Action],
}

/// The option every command accepts.
pub const HELP_OPTION: Opt = Opt { flag: "-h, --help", help: "show this help message and exit" };

/// Every command's help, in no particular order.
pub fn all() -> impl Iterator<Item = &'static Command> {
    build::COMMANDS.iter().chain(route::COMMANDS).chain(review::COMMANDS).chain(manufacture::COMMANDS)
}

pub fn command(name: &str) -> Option<&'static Command> {
    all().find(|command| command.name == name)
}

/// Help lines wrap at this many columns.
const LINE_WIDTH: usize = 100;

/// `text` split into lines of at most `width` characters, at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines
}

fn write_options(out: &mut dyn Write, options: &[Opt]) {
    let longest = options.iter().map(|option| option.flag.len()).chain([HELP_OPTION.flag.len()]).max();
    let width = longest.unwrap_or(0).min(30);
    let text_width = LINE_WIDTH.saturating_sub(width + 4).max(30);
    let _ = writeln!(out, "\noptions:");
    for option in std::iter::once(&HELP_OPTION).chain(options) {
        let lines = wrap(option.help, text_width);
        let mut lines = lines.iter();
        if option.flag.len() > width {
            let _ = writeln!(out, "  {}", option.flag);
        } else {
            let first = lines.next().map_or("", String::as_str);
            let _ = writeln!(out, "  {:width$}  {first}", option.flag);
        }
        for line in lines {
            let _ = writeln!(out, "  {:width$}  {line}", "");
        }
    }
}

/// Print `name`'s option list (after its usage line and summary).
pub fn options(out: &mut dyn Write, name: &str) {
    let Some(command) = command(name) else { return };
    if !command.actions.is_empty() {
        let width = command.actions.iter().map(|action| action.name.len()).max().unwrap_or(0);
        let _ = writeln!(out, "\nactions:");
        for action in command.actions {
            let _ = writeln!(out, "  {:width$}  {}", action.name, action.summary);
        }
        let _ = writeln!(out, "\nRun `copper {name} ACTION --help` for an action's options.");
        if command.options.is_empty() {
            return;
        }
    }
    write_options(out, command.options);
}

/// Print the option list of `name`'s `action`.
pub fn action_options(out: &mut dyn Write, name: &str, action: &str) {
    let found = command(name).and_then(|command| command.actions.iter().find(|a| a.name == action));
    if let Some(found) = found {
        write_options(out, found.options);
    }
}

/// The top-level `copper --help`.
pub fn top_level(out: &mut dyn Write, usage: &str) {
    let _ = writeln!(out, "{usage}\n\nCopperScript: PCB design as code.");
    let width = all().map(|command| command.name.len()).max().unwrap_or(0);
    for group in Group::ALL {
        let _ = writeln!(out, "\n{}:", group.title());
        for command in crate::commands::COMMANDS.iter().filter_map(|name| self::command(name)) {
            if command.group == group {
                let _ = writeln!(out, "  {:width$}  {}", command.name, command.summary);
            }
        }
    }
    let _ = writeln!(
        out,
        "\nRun `copper COMMAND --help` for a command's options and `copper --version` for the build.\n\
         `copper completions bash|zsh|fish` prints a shell completion script."
    );
}

/// The option names a flag spelling declares: `-o, --output OUTPUT` gives `-o` and `--output`.
pub fn names(flag: &str) -> Vec<&str> {
    flag.split([',', ' ', '='])
        .map(|part| part.trim().trim_start_matches('[').trim_end_matches(']'))
        .filter(|part| part.starts_with('-') && part.len() > 1 && !part.starts_with("--{"))
        .collect()
}

#[cfg(test)]
mod tests;

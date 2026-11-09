//! `copper completions {bash,zsh,fish}`: a shell completion script built from
//! the command list and each command's documented options.

use std::io::Write;

use crate::commands::COMMANDS;
use crate::help;

const USAGE: &str = "usage: copper completions [-h] {bash,zsh,fish}";

/// A command's completable words: its options, or its actions then their options.
fn words(name: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let Some(command) = help::command(name) else { return (Vec::new(), Vec::new()) };
    let mut options: Vec<&'static str> = vec!["--help"];
    let documented = command.options.iter().chain(command.actions.iter().flat_map(|action| action.options));
    for option in documented {
        for spelling in help::names(option.flag) {
            if spelling.starts_with("--") && !options.contains(&spelling) {
                options.push(spelling);
            }
        }
    }
    (command.actions.iter().map(|action| action.name).collect(), options)
}

fn bash() -> String {
    let mut cases = String::new();
    for name in COMMANDS {
        let (actions, options) = words(name);
        let mut list = actions;
        list.extend(options);
        cases.push_str(&format!("        {name}) words=\"{}\" ;;\n", list.join(" ")));
    }
    format!(
        "# bash completion for copper: eval \"$(copper completions bash)\"\n\
         _copper() {{\n\
         \x20   local current=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
         \x20   if [ \"$COMP_CWORD\" -eq 1 ]; then\n\
         \x20       COMPREPLY=($(compgen -W \"{commands} --help --version\" -- \"$current\"))\n\
         \x20       return\n\
         \x20   fi\n\
         \x20   local words=\"\"\n\
         \x20   case \"${{COMP_WORDS[1]}}\" in\n\
         {cases}\
         \x20   esac\n\
         \x20   if [[ \"$current\" == -* ]] || [ \"$COMP_CWORD\" -eq 2 ]; then\n\
         \x20       COMPREPLY=($(compgen -W \"$words\" -- \"$current\"))\n\
         \x20   fi\n\
         \x20   [ ${{#COMPREPLY[@]}} -eq 0 ] && COMPREPLY=($(compgen -f -- \"$current\"))\n\
         }}\n\
         complete -o filenames -F _copper copper\n",
        commands = COMMANDS.join(" "),
    )
}

fn zsh() -> String {
    let mut cases = String::new();
    for name in COMMANDS {
        let (actions, options) = words(name);
        let mut list = actions;
        list.extend(options);
        cases.push_str(&format!("        {name}) words=({}) ;;\n", list.join(" ")));
    }
    format!(
        "#compdef copper\n\
         # zsh completion for copper: copper completions zsh > \"${{fpath[1]}}/_copper\"\n\
         _copper() {{\n\
         \x20   if (( CURRENT == 2 )); then\n\
         \x20       compadd -- {commands} --help --version\n\
         \x20       return\n\
         \x20   fi\n\
         \x20   local -a words\n\
         \x20   case $words[2] in\n\
         {cases}\
         \x20   esac\n\
         \x20   compadd -- $words\n\
         \x20   _files\n\
         }}\n\
         compdef _copper copper\n",
        commands = COMMANDS.join(" "),
    )
}

fn fish() -> String {
    let mut script = String::from(
        "# fish completion for copper: copper completions fish > ~/.config/fish/completions/copper.fish\n\
         complete -c copper -f -n __fish_use_subcommand -l version -d 'print the version'\n",
    );
    for name in COMMANDS {
        let summary = help::command(name).map_or("", |command| command.summary).replace('\'', "\\'");
        script.push_str(&format!("complete -c copper -n __fish_use_subcommand -a {name} -d '{summary}'\n"));
        let (actions, options) = words(name);
        for action in actions {
            script.push_str(&format!("complete -c copper -n '__fish_seen_subcommand_from {name}' -a {action}\n"));
        }
        for option in options {
            script.push_str(&format!(
                "complete -c copper -n '__fish_seen_subcommand_from {name}' -l {}\n",
                option.trim_start_matches("--")
            ));
        }
    }
    script
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let script = match arguments {
        [shell] if shell == "-h" || shell == "--help" => {
            let _ = writeln!(out, "{USAGE}\n\nprint a shell completion script (bash, zsh or fish)");
            help::options(out, "completions");
            return 0;
        }
        [shell] if shell == "bash" => bash(),
        [shell] if shell == "zsh" => zsh(),
        [shell] if shell == "fish" => fish(),
        [] => {
            let _ = writeln!(err, "{USAGE}\ncopper completions: error: the following arguments are required: shell");
            return 2;
        }
        _ => {
            let _ = writeln!(
                err,
                "{USAGE}\ncopper completions: error: argument shell: invalid choice: '{}' (choose from bash, zsh, fish)",
                arguments.join(" ")
            );
            return 2;
        }
    };
    let _ = out.write_all(script.as_bytes());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &[&str]) -> (u8, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn every_shell_lists_commands_and_their_options() {
        for shell in ["bash", "zsh", "fish"] {
            let (code, script) = call(&[shell]);
            assert_eq!(code, 0);
            assert!(script.contains("route-board"), "{shell}");
            assert!(script.contains("unified-set"), "{shell}");
            assert!(script.contains("search"), "{shell}: library actions");
        }
        assert_eq!(call(&["tcsh"]).0, 2);
        assert_eq!(call(&[]).0, 2);
    }
}

//! Command dispatch and the list of commands.

use std::io::Write;

use crate::{
    agent_guide, assembly, audit_footprints, benchmark_routing, check, check_footprint, compare_cam, compile,
    compile_macro, completions, drc, edit_mechanical, editor_overlay, explain, export_kicad, export_kicad_pcb,
    export_manufacturing, fill, help, init, issues, library, lock, lsp, models, plan_layout, plot, power_check, query,
    render, replay_critical, replay_unified, route_board, route_global, si_check, state, summarize, synthesize_antenna,
    view,
};

/// Exit status of behaviour a command does not implement.
pub const NOT_IMPLEMENTED_EXIT: u8 = 3;
/// The stderr marker of behaviour a command does not implement.
pub const NOT_IMPLEMENTED_MARKER: &str = "not implemented";

/// Every command, in the order the help lists them within each group.
pub const COMMANDS: &[&str] = &[
    "agent-guide",
    "init",
    "explain",
    "completions",
    "check",
    "compile",
    "query",
    "power-check",
    "lock",
    "library",
    "audit-footprints",
    "check-footprint",
    "compile-macro",
    "lsp",
    "plan-layout",
    "route-global",
    "route-board",
    "edit-mechanical",
    "edit",
    "editor-overlay",
    "synthesize-antenna",
    "si-check",
    "issues",
    "view",
    "summarize",
    "state",
    "replay-unified",
    "replay-critical",
    "export-kicad",
    "export-kicad-pcb",
    "fill",
    "plot",
    "drc",
    "render",
    "compare-cam",
    "export-manufacturing",
    "assembly",
    "panelize",
    "models",
    "bench",
    "benchmark-routing",
];

/// `copper --version`: the crate version and the first 12 hex digits of the
/// digest of the workspace's Rust sources (the full digest is in placement
/// checkpoints).
pub fn version() -> String {
    let digest = env!("COPPER_SOURCE_DIGEST");
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    format!("copper {} (sources {})", env!("CARGO_PKG_VERSION"), &hex[..hex.len().min(12)])
}

const USAGE: &str = "usage: copper COMMAND [ARGS]";

pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    // Placement's decoupling-access pass runs the decoupler router and
    // via-in-pad arrays check their sites against the routing clearance
    // index (both in copper-route); plan-layout and the editor reach them
    // through copper-place alone.
    copper_route::install_hooks();
    let Some(command) = args.first() else {
        let _ = writeln!(err, "{USAGE}\ncopper: error: the following arguments are required: command");
        return 2;
    };
    let rest = &args[1..];
    match command.as_str() {
        "-h" | "--help" => {
            help::top_level(out, USAGE);
            0
        }
        "-V" | "--version" => {
            let _ = writeln!(out, "{}", version());
            0
        }
        "init" => init::run(rest, out, err),
        "completions" => completions::run(rest, out, err),
        "agent-guide" => agent_guide::run(rest, out, err),
        "state" => state::run(rest, out, err),
        "compile" => compile::run(rest, out, err),
        "check" => check::run(rest, out, err),
        "query" => query::run(rest, out, err),
        "library" => library::run(rest, out, err),
        "lock" => lock::run(rest, out, err),
        "compile-macro" => compile_macro::run(rest, out, err),
        "audit-footprints" => audit_footprints::run(rest, out, err),
        "check-footprint" => check_footprint::run(rest, out, err),
        "power-check" => power_check::run(rest, out, err),
        "export-kicad" => export_kicad::run(rest, out, err),
        "export-kicad-pcb" => export_kicad_pcb::run(rest, out, err),
        "synthesize-antenna" => synthesize_antenna::run(rest, out, err),
        "panelize" => crate::panelize::run(rest, out, err),
        "lsp" => lsp::run(rest, out, err),
        "explain" => explain::run(rest, out, err),
        "issues" => issues::run(rest, out, err),
        "view" => view::run(rest, out, err),
        "summarize" => summarize::run(rest, out, err),
        "assembly" => assembly::run(rest, out, err),
        "si-check" => si_check::run(rest, out, err),
        "route-board" => route_board::run(rest, out, err),
        "replay-critical" => replay_critical::run(rest, out, err),
        "export-manufacturing" => export_manufacturing::run(rest, out, err),
        "plot" => plot::run(rest, out, err),
        "render" => render::run(rest, out, err),
        "models" => models::run(rest, out, err),
        "fill" => fill::run(rest, out, err),
        "drc" => drc::run(rest, out, err),
        "compare-cam" => compare_cam::run(rest, out, err),
        "plan-layout" => plan_layout::run(rest, out, err),
        "bench" => copper_route::ladder::run(rest, out, err),
        "benchmark-routing" => benchmark_routing::run(rest, out, err),
        "route-global" => route_global::run(rest, out, err),
        "replay-unified" => replay_unified::run(rest, out, err),
        "editor-overlay" => editor_overlay::run(rest, out, err),
        "edit-mechanical" | "edit" => edit_mechanical::run(rest, out, err),
        _ => {
            let _ = writeln!(err, "{USAGE}\ncopper: error: invalid command '{command}'");
            match suggestion(command) {
                Some(close) => {
                    let _ = writeln!(err, "did you mean `copper {close}`? (`copper --help` lists every command)");
                }
                None => {
                    let _ = writeln!(err, "`copper --help` lists every command");
                }
            }
            2
        }
    }
}

/// The command closest to a mistyped one, when it is close enough to be a typo.
fn suggestion(typed: &str) -> Option<&'static str> {
    let distance = |a: &str, b: &str| {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut previous = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let next = (row[j + 1] + 1).min(row[j] + 1).min(previous + usize::from(ca != *cb));
                previous = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    };
    let (best, score) = COMMANDS.iter().map(|name| (*name, distance(typed, name))).min_by_key(|(_, score)| *score)?;
    let prefix = COMMANDS.iter().find(|name| typed.len() >= 3 && name.starts_with(typed));
    prefix.copied().or((score <= 2 || score * 3 <= best.len()).then_some(best))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn help_lists_every_command_with_its_summary() {
        let (code, out, _) = call(&["--help"]);
        assert_eq!(code, 0);
        for name in COMMANDS {
            let summary = help::command(name).unwrap_or_else(|| panic!("{name} has no help entry")).summary;
            assert!(
                out.lines().any(|line| line.trim_start().starts_with(&format!("{name} ")) && line.ends_with(summary)),
                "{name}: {out}"
            );
        }
        assert!(!out.contains("Python"), "{out}");
    }

    #[test]
    fn version_names_the_build() {
        let (code, out, _) = call(&["--version"]);
        assert_eq!(code, 0);
        assert!(out.starts_with(&format!("copper {} (sources ", env!("CARGO_PKG_VERSION"))), "{out}");
        assert_eq!(call(&["-V"]).1, out);
    }

    #[test]
    fn compile_errors_go_to_stderr() {
        for command in [&["check"][..], &["compile"], &["query", "nets"], &["lock"], &["power-check"]] {
            let mut args = vec![command[0], "does-not-exist.copper"];
            args.extend(&command[1..]);
            args.push("--offline");
            let (code, out, err) = call(&args);
            let command = command[0];
            assert_eq!(code, 2, "{command}");
            assert!(out.is_empty(), "{command}: {out}");
            assert!(err.starts_with("COMPILE ERROR: "), "{command}: {err}");
        }
    }

    #[test]
    fn mistyped_commands_get_a_suggestion() {
        let (code, _, err) = call(&["rout-board"]);
        assert_eq!(code, 2);
        assert!(err.contains("did you mean `copper route-board`?"), "{err}");
        assert!(call(&["chek"]).2.contains("`copper check`"));
        assert!(!call(&["frobnicate"]).2.contains("did you mean"));
    }

    #[test]
    fn unknown_missing_and_dropped_commands_are_usage_errors() {
        assert_eq!(call(&["frobnicate"]).0, 2);
        assert_eq!(call(&["audit-cam"]).0, 2);
        assert_eq!(call(&[]).0, 2);
    }
}

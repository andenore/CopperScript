//! Help and behaviour cannot drift apart: every option literal a command's
//! source accepts is documented in its help entry, and every documented
//! option is one the source names.

use super::*;
use crate::commands::COMMANDS;

/// Each command's parsing source.
fn source(name: &str) -> &'static str {
    match name {
        "agent-guide" => include_str!("../agent_guide.rs"),
        "assembly" => include_str!("../assembly.rs"),
        "audit-footprints" => include_str!("../audit_footprints.rs"),
        "bench" => include_str!("../../../copper-route/src/ladder.rs"),
        "build" => include_str!("../build.rs"),
        "benchmark-routing" => include_str!("../benchmark_routing.rs"),
        "check" => include_str!("../check.rs"),
        "check-footprint" => include_str!("../check_footprint.rs"),
        "compare-cam" => include_str!("../compare_cam.rs"),
        "compile" => include_str!("../compile.rs"),
        "compile-macro" => include_str!("../compile_macro.rs"),
        "completions" => include_str!("../completions.rs"),
        "drc" => include_str!("../drc.rs"),
        "edit" | "edit-mechanical" => include_str!("../edit_mechanical.rs"),
        "editor-overlay" => include_str!("../editor_overlay.rs"),
        "explain" => include_str!("../explain.rs"),
        "export-kicad" => include_str!("../export_kicad.rs"),
        "export-kicad-pcb" => include_str!("../export_kicad_pcb.rs"),
        "export-manufacturing" => include_str!("../export_manufacturing.rs"),
        "fill" => include_str!("../fill.rs"),
        "init" => include_str!("../init.rs"),
        "issues" => include_str!("../issues.rs"),
        "library" => include_str!("../library.rs"),
        "lock" => include_str!("../lock.rs"),
        "lsp" => include_str!("../lsp.rs"),
        "models" => include_str!("../models.rs"),
        "panelize" => include_str!("../panelize.rs"),
        "plan-layout" => include_str!("../plan_layout.rs"),
        "plot" => include_str!("../plot.rs"),
        "power-check" => include_str!("../power_check.rs"),
        "query" => include_str!("../query.rs"),
        "render" => include_str!("../render.rs"),
        "replay-critical" => include_str!("../replay_critical.rs"),
        "replay-unified" => include_str!("../replay_unified.rs"),
        "route-board" => include_str!("../route_board.rs"),
        "route-global" => include_str!("../route_global.rs"),
        "si-check" => include_str!("../si_check.rs"),
        "state" => include_str!("../state.rs"),
        "summarize" => include_str!("../summarize.rs"),
        "synthesize-antenna" => include_str!("../synthesize_antenna.rs"),
        "view" => include_str!("../view.rs"),
        other => panic!("no source registered for {other}"),
    }
}

/// Literals in a command's source that are not its own options: removed
/// options that only produce a migration error, arguments passed to other
/// programs, and names shared through another module.
fn not_options(name: &str) -> &'static [&'static str] {
    match name {
        "route-board" => &[
            "--unified-direct",
            "--unified-no-decouplers",
            "--unified-retain-escape-copper",
            "--unified-iterations",
            "--unified-search-states",
            "--unified-pair-states",
            "--fine-regions",
            "--power-islands",
            "--no-route-smoothing",
            "--stitch-zone-pads",
            "--detailed-feedback-trials",
            "--critical-feedback-trials",
            "--pitch-mm",
            "--debug",
            "--support-pattern-trials",
            "--package-support-trials",
            "--placement-seeds",
            "--power-domain-weight",
            "--critical-pin-access-step-mm",
            "--layer-preference-cost",
            "--direction-preference-cost",
            "--layer-assignment-passes",
            "--local-demand-cost",
            "--local-demand-pitch-mm",
            "--fanout-step-mm",
            "--fanout-geometry-candidates",
            "--plane-escape-width-mm",
            "--maximum-ripup-blockers",
            "--zone-dependency-expansions",
            "--filled-copper-search-states",
        ],
        // `--copper` only produces an error (the build runs in-process); the rest
        // are arguments `build` passes to `route-board`, `fill`, `drc`, kicad-cli
        // and git, or refuses after `--`.
        "build" => &[
            "--copper",
            "--footprint-root",
            "--verify-plane-fill",
            "--report",
            "-o",
            "--output",
            "--macro-inputs",
            "--hard-macro",
            "--placement-templates",
            "--expected-open-ends",
            "--progress",
            "--refill-zones",
            "--save-board",
            "--severity-all",
            "--format",
            "--porcelain",
            "--no-ext-diff",
            "-C",
        ],
        // Arguments `bench` passes to `route-board` and to git.
        "bench" => &["--reuse-placement", "--checkpoint-dir", "--report", "-o", "--verify-plane-fill", "--short"],
        // Arguments `models fetch` passes to curl.
        "models" => {
            &["--fail", "--silent", "--show-error", "--location", "--proto", "--proto-redir", "--max-time", "--output"]
        }
        _ => &[],
    }
}

/// Documented options a command accepts through a shared parser rather than
/// a literal in its own file.
fn shared(name: &str) -> &'static [&'static str] {
    match name {
        // `plan_layout::parse_options` gives every command `-o` for `--output`.
        "route-global" | "plan-layout" | "export-kicad-pcb" => &["-o"],
        // `library` actions get `-o` from the same rule.
        "library" => &["-o"],
        _ => &[],
    }
}

/// `text` without its `#[cfg(test)]` items (brace-matched, string-aware).
fn without_tests(text: &str) -> String {
    let mut kept = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("#[cfg(test)]") {
        kept.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(open) = after.find('{') else { return kept };
        let (mut depth, mut in_string, mut escaped, mut end) = (0usize, false, false, after.len());
        for (index, c) in after[open..].char_indices() {
            if in_string {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + index + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &after[end..];
    }
    kept.push_str(rest);
    kept
}

/// Every `"--name"` and `"-x"` literal outside tests and comments.
fn option_literals(text: &str) -> Vec<String> {
    let code: String =
        without_tests(text).lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let mut found = Vec::new();
    let bytes = code.as_bytes();
    let mut index = 0;
    while let Some(offset) = code[index..].find("\"-") {
        let start = index + offset + 1;
        let mut end = start + 1;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
            end += 1;
        }
        index = end;
        if bytes.get(end) != Some(&b'"') {
            continue;
        }
        let literal = &code[start..end];
        let long = literal.starts_with("--") && literal.len() > 2;
        let short = !literal.starts_with("--") && literal.len() == 2 && bytes[start + 1].is_ascii_alphabetic();
        if (long || short) && !matches!(literal, "--help" | "-h") && !found.iter().any(|f| f == literal) {
            found.push(literal.to_owned());
        }
    }
    found
}

fn documented(command: &Command) -> Vec<&'static str> {
    let actions = command.actions.iter().flat_map(|action| action.options);
    command.options.iter().chain(actions).flat_map(|option| names(option.flag)).collect()
}

#[test]
fn every_command_has_help_and_every_entry_is_a_command() {
    for name in COMMANDS {
        assert!(command(name).is_some(), "{name} has no help entry");
    }
    for entry in all() {
        assert!(COMMANDS.contains(&entry.name), "help entry {} is not a command", entry.name);
        assert!(!entry.summary.is_empty() && !entry.summary.contains('\n'), "{}: one-line summary", entry.name);
    }
}

#[test]
fn every_accepted_option_is_documented() {
    let mut missing = Vec::new();
    for name in COMMANDS {
        let entry = command(name).expect("checked above");
        let names = documented(entry);
        for literal in option_literals(source(name)) {
            if !names.contains(&literal.as_str()) && !not_options(name).contains(&literal.as_str()) {
                missing.push(format!("{name}: {literal}"));
            }
        }
    }
    assert!(missing.is_empty(), "options missing from help:\n{}", missing.join("\n"));
}

#[test]
fn every_documented_option_exists() {
    let mut invented = Vec::new();
    for name in COMMANDS {
        let literals = option_literals(source(name));
        for documented in documented(command(name).expect("checked above")) {
            let known = literals.iter().any(|literal| literal == documented) || shared(name).contains(&documented);
            if !known {
                invented.push(format!("{name}: {documented}"));
            }
        }
    }
    assert!(invented.is_empty(), "documented options the source does not name:\n{}", invented.join("\n"));
}

#[test]
fn removed_options_are_not_documented() {
    for name in COMMANDS {
        let documented = documented(command(name).expect("checked above"));
        for removed in not_options(name) {
            assert!(!documented.contains(removed), "{name} still documents {removed}");
        }
    }
}

#[test]
fn option_names_parse_from_flag_spellings() {
    assert_eq!(names("-o, --output OUTPUT"), ["-o", "--output"]);
    assert_eq!(names("--fanout, --no-fanout"), ["--fanout", "--no-fanout"]);
    assert_eq!(names("--layers {2,4,6}"), ["--layers"]);
    assert_eq!(names("--unified-set KEY=VALUE"), ["--unified-set"]);
}

#[test]
fn test_items_and_comments_are_not_scanned() {
    let text = "const A: &str = \"--real\";\n// \"--comment\"\n#[cfg(test)]\nmod tests { fn f() { let _ = \"--test-only\"; let _ = \"}\"; } }\nconst B: &str = \"-o\";";
    assert_eq!(option_literals(text), ["--real", "-o"]);
}

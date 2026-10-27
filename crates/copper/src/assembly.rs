//! `copper assembly {snapshot,check,bom,candidate-bom} BOARD [--locked] [--offline] ...`:
//! pin and check explicit assembly selections offline.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_assembly::assembly::{self, AssemblyError, Lock};
use copper_json::text;
use copper_json::{Dumps, Value};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::model::Board;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, Spec, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper assembly [-h] {snapshot,check,bom,candidate-bom} ...";

fn specs(action: &str) -> Vec<Spec> {
    let mut specs = vec![flag("--locked"), flag("--offline")];
    match action {
        "snapshot" => specs.push(short_value("--output", "-o")),
        "check" => specs.extend([value("--lock"), value("--report")]),
        "bom" => specs.extend([
            value("--lock"),
            short_value("--output", "-o"),
            value("--hand-assembly"),
            value("--ordering"),
        ]),
        _ => specs.extend([value("--lock"), short_value("--output", "-o"), value("--positions")]),
    }
    specs
}

/// A path argument as Python holds it: `Path(arg)`, shown as `str()`.
struct PathArg {
    path: PathBuf,
    shown: String,
}

impl PathArg {
    fn new(arg: &str) -> PathArg {
        let shown = pypath::display(arg);
        PathArg { path: PathBuf::from(&shown), shown }
    }
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper assembly: error: {message}");
        2
    };
    let Some(action) = arguments.first() else {
        return usage_error(err, "the following arguments are required: assembly_command");
    };
    if action == "-h" || action == "--help" {
        let _ = writeln!(out, "{USAGE}\n\npin and check explicit assembly selections offline");
        return 0;
    }
    if !["snapshot", "check", "bom", "candidate-bom"].contains(&action.as_str()) {
        return usage_error(
            err,
            &format!(
                "argument assembly_command: invalid choice: '{action}' (choose from snapshot, check, bom, candidate-bom)"
            ),
        );
    }
    let parsed = match option_parser::parse(&arguments[1..], &specs(action)) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "usage: copper assembly {action} [-h] [--locked] [--offline] ... board");
        return 0;
    }
    let required: &[&str] = match action.as_str() {
        "snapshot" => &["--output"],
        "check" => &["--lock"],
        _ => &["--lock", "--output"],
    };
    let missing: Vec<&str> = required.iter().copied().filter(|name| parsed.value(name).is_none()).collect();
    let board = match parsed.positionals.as_slice() {
        [board] if missing.is_empty() => board.clone(),
        [_, extra @ ..] if !extra.is_empty() => {
            return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" ")));
        }
        _ => return usage_error(err, "the following arguments are required: board and options"),
    };
    let (locked, offline) = (parsed.flag("--locked"), parsed.flag("--offline"));
    let design = match load_design(Path::new(&board), locked, offline) {
        Ok(design) => design,
        Err(LoadError::Compile(failure)) if failure.code == NOT_IMPLEMENTED => {
            let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: assembly ({})", failure.message);
            return NOT_IMPLEMENTED_EXIT;
        }
        Err(failure) => {
            let _ = text::print(out, &format!("ASSEMBLY ERROR: {failure}"));
            return 2;
        }
    };
    match execute(action, &parsed, &design.electrical, out) {
        Ok(code) => code,
        Err(AssemblyError(message)) => {
            let _ = text::print(out, &format!("ASSEMBLY ERROR: {message}"));
            2
        }
    }
}

fn execute(
    action: &str,
    parsed: &option_parser::Parsed,
    board: &Board,
    out: &mut dyn Write,
) -> Result<u8, AssemblyError> {
    let diagnostics = copper_lang::erc::check(board, false).map_err(AssemblyError)?;
    if copper_lang::erc::has_errors(&diagnostics) {
        for diagnostic in &diagnostics {
            let _ = text::print(out, &diagnostic.to_string());
        }
        let _ = text::print(out, "Assembly stopped because ERC reported errors.");
        return Ok(1);
    }
    let argument = |name: &str| parsed.value(name).map(PathArg::new);
    if action == "snapshot" {
        let output = argument("--output").expect("required option");
        let content = assembly::lock_to_json(&assembly::snapshot(board)?);
        copper_assembly::files::mkdir_parents(&copper_assembly::files::parent(&output.path), true, true)
            .map_err(AssemblyError)?;
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output.path)
            .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &output.shown)))?;
        stream
            .write_all(content.as_bytes())
            .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &output.shown)))?;
        let _ = text::print(out, &format!("Created unreviewed assembly snapshot -> {}", output.shown));
        return Ok(0);
    }
    let lock_path = argument("--lock").expect("required option");
    let lock: Lock = assembly::load_lock(&lock_path.path, &lock_path.shown)?;
    let report = assembly::check_assembly(board, &lock)?;
    if action == "candidate-bom" {
        let output = argument("--output").expect("required option");
        let positions = argument("--positions");
        assembly::write_candidate_bom(
            board,
            &lock,
            &output.path,
            positions.as_ref().map(|positions| (positions.path.as_path(), positions.shown.as_str())),
        )?;
        let _ = text::print(
            out,
            &format!("Created UNAPPROVED assembly review candidate -> {}; not an ordering certificate", output.shown),
        );
        return Ok(0);
    }
    let count = |key: &str| report.get(key).map(|value| Dumps::compact().dumps(value)).unwrap_or_default();
    let names = |key: &str| -> Vec<String> {
        report
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    if action == "check" {
        if let Some(path) = argument("--report") {
            copper_assembly::files::mkdir_parents(&copper_assembly::files::parent(&path.path), true, true)
                .map_err(AssemblyError)?;
            let document = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(report.clone())) + "\n";
            text::write_text(&path.path, &document)
                .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &path.shown)))?;
        }
        let _ = text::print(
            out,
            &format!(
                "Assembly selections: {}/{} exact; {} reviewed; availability not checked",
                count("exact_selection_count"),
                count("component_count"),
                count("reviewed_count")
            ),
        );
        // Never let an exclusion from the supplier BOM/CPL pass silently.
        for (label, key) in [("consigned", "consigned"), ("not fitted", "not_fitted")] {
            let names = names(key);
            if !names.is_empty() {
                let mut shown = names.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
                if names.len() > 12 {
                    shown.push_str(&format!(", +{} more", names.len() - 12));
                }
                let _ = text::print(out, &format!("Outside the supplier BOM/CPL, {label}: {} ({shown})", names.len()));
            }
        }
        for issue in report.get("issues").and_then(Value::as_array).unwrap_or(&[]) {
            let field = |key: &str| issue.get(key).and_then(Value::as_str).unwrap_or("");
            let reference = issue.get("reference").and_then(Value::as_str).filter(|r| !r.is_empty());
            let _ = text::print(
                out,
                &format!("{}: {}: {}", field("code"), reference.unwrap_or(&board.name), field("message")),
            );
        }
        return Ok(if assembly::passed(&report) { 0 } else { 1 });
    }
    let output = argument("--output").expect("required option");
    assembly::write_jlcpcb_bom(board, &lock, &output.path)?;
    let (consigned, not_fitted) = (count("consigned_count"), count("not_fitted_count"));
    let left_out = names("consigned").len() + names("not_fitted").len() > 0;
    let _ = text::print(
        out,
        &format!(
            "Generated selection BOM -> {}; {} supplier-assembled{}; stock and manufacturing signoff not checked",
            output.shown,
            count("supplier_assembled_count"),
            if left_out {
                format!(", {consigned} consigned and {not_fitted} not fitted left out")
            } else {
                String::new()
            }
        ),
    );
    if let Some(path) = argument("--hand-assembly") {
        let total = assembly::write_hand_assembly_list(board, &lock, &path.path)?;
        let _ = text::print(
            out,
            &format!(
                "Generated hand-assembly list -> {}; {total} component(s) to fit or leave empty by hand",
                path.shown
            ),
        );
    }
    if let Some(path) = argument("--ordering") {
        let total = assembly::write_ordering_list(board, &lock, &path.path)?;
        let _ =
            text::print(out, &format!("Generated ordering list -> {}; {total} component(s) with sourcing", path.shown));
    }
    Ok(0)
}

//! `copper assembly {snapshot,update,check,bom,candidate-bom} BOARD [--locked] [--offline] ...`:
//! pin and check explicit assembly selections offline. `copper assembly offers`
//! is the one networked action: a timestamped live availability report.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_assembly::assembly::{self, AssemblyError, Lock};
use copper_assembly::offers;
use copper_json::text;
use copper_json::{Dumps, Value};
use copper_lang::compiler::{LoadError, load_design};
use copper_lang::mechanical::NOT_IMPLEMENTED;
use copper_lang::model::Board;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::option_parser::{self, Spec, flag, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper assembly [-h] {snapshot,update,check,bom,candidate-bom,offers} ...";
const ACTIONS: [&str; 6] = ["snapshot", "update", "check", "bom", "candidate-bom", "offers"];
const OFFERS_USAGE: &str = "usage: copper assembly offers [-h] [--lock LOCK] [-o OUTPUT] [--distributors NAMES] \
[--provider NAMES] [--quantity BOARDS] [--min-stock N] [--within-weeks W] [--country CC] [--currency CUR] \
[--write-alternates] board

Look up live distributor offers for every fitted manufacturer part in the lock
from each configured provider: digikey (DIGIKEY_CLIENT_ID, DIGIKEY_CLIENT_SECRET),
mouser (MOUSER_API_KEY), farnell (FARNELL_API_KEY, optional FARNELL_STORE) and
nexar/Octopart (NEXAR_CLIENT_ID, NEXAR_CLIENT_SECRET). --provider limits the
lookup to the named ones. Writes a timestamped report, by default under the
board's build/assembly-offers/. --write-alternates adds offers for the same
manufacturer part from distributors not yet listed on a selection; nothing
else in the lock changes.";

fn specs(action: &str) -> Vec<Spec> {
    let mut specs = vec![flag("--locked"), flag("--offline")];
    match action {
        "snapshot" => specs.push(short_value("--output", "-o")),
        "update" => specs.push(value("--lock")),
        "check" => specs.extend([value("--lock"), value("--report")]),
        "bom" => specs.extend([
            value("--lock"),
            short_value("--output", "-o"),
            value("--hand-assembly"),
            value("--ordering"),
            value("--assembly-sheet"),
        ]),
        "offers" => specs.extend([
            value("--lock"),
            short_value("--output", "-o"),
            value("--distributors"),
            value("--provider"),
            value("--quantity"),
            value("--min-stock"),
            value("--within-weeks"),
            value("--country"),
            value("--currency"),
            flag("--write-alternates"),
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
    if !ACTIONS.contains(&action.as_str()) {
        return usage_error(
            err,
            &format!(
                "argument assembly_command: invalid choice: '{action}' \
                 (choose from {})",
                ACTIONS.join(", ")
            ),
        );
    }
    let parsed = match option_parser::parse(&arguments[1..], &specs(action)) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        if action == "offers" {
            let _ = writeln!(out, "{OFFERS_USAGE}");
        } else {
            let _ = writeln!(out, "usage: copper assembly {action} [-h] [--locked] [--offline] ... board");
        }
        return 0;
    }
    if action == "offers" {
        let board = match parsed.positionals.as_slice() {
            [board] => board.clone(),
            [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
            [] => return usage_error(err, "the following arguments are required: board"),
        };
        if parsed.flag("--offline") {
            return usage_error(err, "offers queries the Nexar API and cannot run --offline");
        }
        return match run_offers(&parsed, &board, out) {
            Ok(code) => code,
            Err(AssemblyError(message)) => {
                let _ = text::print(out, &format!("ASSEMBLY ERROR: {message}"));
                2
            }
        };
    }
    let required: &[&str] = match action.as_str() {
        "snapshot" => &["--output"],
        "update" | "check" => &["--lock"],
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
    if action == "update" {
        let (updated, changes) = assembly::update(board, &lock)?;
        // Replace the lock only once the new one is fully written.
        let staged = lock_path.path.with_extension("lock.update");
        let failed =
            |failure: std::io::Error| AssemblyError(copper_assembly::files::open_error(&failure, &lock_path.shown));
        std::fs::write(&staged, assembly::lock_to_json(&updated)).map_err(failed)?;
        std::fs::rename(&staged, &lock_path.path).map_err(failed)?;
        let reset = changes.changed.iter().filter(|(_, _, reviewed)| *reviewed).count();
        let _ = text::print(
            out,
            &format!(
                "Updated assembly lock -> {}: {} unchanged, {} changed ({reset} review(s) reset), {} added, {} removed",
                lock_path.shown,
                changes.unchanged,
                changes.changed.len(),
                changes.added.len(),
                changes.removed.len()
            ),
        );
        let mut lines: Vec<String> = changes
            .changed
            .iter()
            .map(|(reference, what, reviewed)| {
                format!("CHANGED: {reference}: {what}{}", if *reviewed { " (review reset)" } else { "" })
            })
            .collect();
        lines.extend(changes.added.iter().map(|reference| format!("ADDED: {reference}: unreviewed, identity to fill")));
        lines.extend(changes.removed.iter().map(|reference| format!("REMOVED: {reference}: no longer assembled")));
        for line in lines.iter().take(40) {
            let _ = text::print(out, line);
        }
        if lines.len() > 40 {
            let _ = text::print(out, &format!("... and {} more", lines.len() - 40));
        }
        return Ok(0);
    }
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
    if count("supplier_assembled_count") == "0" && assembly::passed(&report) {
        // A board fitted entirely by hand has no supplier BOM; remove a stale
        // one so nothing downstream exports it, and still write the lists.
        if output.path.is_file() {
            std::fs::remove_file(&output.path)
                .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &output.shown)))?;
        }
        let _ = text::print(
            out,
            &format!("No supplier-assembled components; no selection BOM written to {}", output.shown),
        );
    } else {
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
    }
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
    if let Some(path) = argument("--assembly-sheet") {
        let bytes = std::fs::read(&lock_path.path)
            .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &lock_path.shown)))?;
        let digest = copper_assembly::files::sha256_hex(&bytes);
        let total = assembly::write_assembly_sheet(board, &lock, &digest, &path.path)?;
        let _ = text::print(
            out,
            &format!(
                "Generated assembly sheet -> {}; {total} component(s) to track; open it with the board in CopperReview",
                path.shown
            ),
        );
    }
    Ok(0)
}

fn count_option(parsed: &option_parser::Parsed, name: &str) -> Result<Option<u64>, AssemblyError> {
    parsed
        .value(name)
        .map(|text| text.parse::<u64>().map_err(|_| AssemblyError(format!("{name} must be a whole number"))))
        .transpose()
}

/// `assembly offers`: never compiles the board and never runs from any other
/// action; the lock alone names what to look up.
fn run_offers(parsed: &option_parser::Parsed, board: &str, out: &mut dyn Write) -> Result<u8, AssemblyError> {
    let board_path = PathArg::new(board);
    let directory = copper_assembly::files::parent(&board_path.path);
    let in_directory = |name: &str| {
        let path = directory.join(name);
        PathArg { shown: path.display().to_string(), path }
    };
    let lock_path = parsed.value("--lock").map(PathArg::new).unwrap_or_else(|| in_directory("assembly.lock"));
    let quantity = count_option(parsed, "--quantity")?.unwrap_or(1);
    if quantity == 0 {
        return Err(AssemblyError("--quantity must be at least 1".into()));
    }
    let criteria = offers::Criteria {
        quantity,
        min_stock: count_option(parsed, "--min-stock")?,
        within_weeks: count_option(parsed, "--within-weeks")?,
    };
    let locale = offers::Locale {
        country: parsed.value("--country").unwrap_or("US").to_owned(),
        currency: parsed.value("--currency").unwrap_or("USD").to_owned(),
    };
    let list = |name: &str| -> Option<Vec<String>> {
        parsed
            .value(name)
            .map(|names| names.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned).collect())
    };
    let lock = assembly::load_lock(&lock_path.path, &lock_path.shown)?;
    let write_alternates = parsed.flag("--write-alternates");
    if write_alternates {
        // Refuse before any network call, and never reformat a hand-edited lock.
        let current = std::fs::read_to_string(&lock_path.path)
            .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &lock_path.shown)))?;
        if lock.electrical_sha256.is_some()
            || copper_assembly::files::universal_newlines(&current) != assembly::lock_to_json(&lock)
        {
            return Err(AssemblyError(format!(
                "--write-alternates rewrites only a canonical {} lock; run `copper assembly update` first",
                assembly::SCHEMA
            )));
        }
    }
    let distributors: Vec<String> = list("--distributors").unwrap_or_else(|| offers::lock_distributors(&lock));
    if distributors.is_empty() {
        return Err(AssemblyError("no distributors to look up; pass --distributors".into()));
    }
    let (lines, unidentified) = offers::lines(&lock);
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let generated_at = offers::utc_timestamp(seconds);
    let output = parsed.value("--output").map(PathArg::new).unwrap_or_else(|| {
        let stamp: String = generated_at.chars().filter(|c| *c != '-' && *c != ':').collect();
        in_directory(&format!("build/assembly-offers/offers-{stamp}.json"))
    });

    let providers = offers::enabled_providers(list("--provider").as_deref(), &locale)?;
    let (results, used, provider_errors) = offers::fetch_all(&offers::Curl, &providers, &lines, &distributors)?;

    let request = offers::Request {
        board: &lock.board,
        lock: &lock_path.shown,
        generated_at: &generated_at,
        distributors: &distributors,
        providers: &used,
        provider_errors: &provider_errors,
        locale: &locale,
        criteria,
    };
    copper_assembly::files::mkdir_parents(&copper_assembly::files::parent(&output.path), true, true)
        .map_err(AssemblyError)?;
    text::write_text(&output.path, &offers::report(&request, &results, &unidentified))
        .map_err(|failure| AssemblyError(copper_assembly::files::open_error(&failure, &output.shown)))?;
    let short = offers::shortfalls(&results, &criteria);
    let _ = text::print(
        out,
        &format!(
            "Offers for {} part(s) from {} via {} at {generated_at} -> {}; {} without an adequate offer for \
             {quantity} board(s); live data, not a reservation",
            results.len(),
            distributors.join(", "),
            used.join(", "),
            output.shown,
            short.len()
        ),
    );
    for (provider, message) in &provider_errors {
        let _ = text::print(out, &format!("PROVIDER ERROR: {provider}: {message}; its offers are missing"));
    }
    for result in short.iter().take(40) {
        let _ = text::print(
            out,
            &format!(
                "SHORT: {} {} ({}): {}",
                result.line.manufacturer,
                result.line.mpn,
                result.line.references.join(", "),
                offers::shortfall_reason(result)
            ),
        );
    }
    if short.len() > 40 {
        let _ = text::print(out, &format!("... and {} more", short.len() - 40));
    }
    if !unidentified.is_empty() {
        let _ = text::print(out, &format!("UNIDENTIFIED (no manufacturer and MPN): {}", unidentified.join(", ")));
    }
    if write_alternates {
        let (updated, added) = offers::add_alternates(&lock, &results, &distributors, &criteria)?;
        if !added.is_empty() {
            // Replace the lock only once the new one is fully written.
            let staged = lock_path.path.with_extension("lock.offers");
            let failed =
                |failure: std::io::Error| AssemblyError(copper_assembly::files::open_error(&failure, &lock_path.shown));
            std::fs::write(&staged, assembly::lock_to_json(&updated)).map_err(failed)?;
            std::fs::rename(&staged, &lock_path.path).map_err(failed)?;
        }
        let _ = text::print(
            out,
            &format!(
                "Added {} alternate offer(s) -> {}; selections and reviews unchanged",
                added.len(),
                lock_path.shown
            ),
        );
        for (reference, supplier, code) in added.iter().take(40) {
            let _ = text::print(out, &format!("ADDED: {reference}: {supplier} {code}"));
        }
        if added.len() > 40 {
            let _ = text::print(out, &format!("... and {} more", added.len() - 40));
        }
    }
    let gated = criteria.min_stock.is_some() || criteria.within_weeks.is_some();
    Ok(if gated && !short.is_empty() { 1 } else { 0 })
}

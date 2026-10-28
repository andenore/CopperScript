//! Benchmark ladder (`pcbir.ladder`, the `copper bench` command): route a
//! fixed set of boards and compare completion and time.
//!
//! The ladder file lists boards with their project directory, source file and
//! `route-board` options. Project paths are relative to the ladder file, and
//! `${VAR}` expands from the environment. A board whose project directory is
//! missing is reported as `skipped`. Each run writes `<out>/<board>/route-report.json`,
//! `board.kicad_pcb` and `route.log`; the aggregate goes to `<out>/results.json`
//! and `<out>/results.md`. Numbers come from the report's `summary` block.
//!
//! The Python runner starts `python -m copperscript route-board`; this one
//! starts the binary it is part of (`copper route-board`) unless `--python`
//! names an interpreter, which then runs the Python implementation as Python's
//! runner does. The record's `command` drops the program as Python's drops
//! the interpreter: it starts at `route-board` on the native path and at
//! `copperscript` with `--python`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use copper_json::{Dumps, Object, Value};
use copper_lang::paths;

pub const SCHEMA: &str = "copperscript-bench/v1";
/// A candidate regresses when it loses completion, gains errors or is this much slower.
pub const SLOWDOWN_LIMIT: f64 = 1.25;
pub const SLOWDOWN_FLOOR_SECONDS: f64 = 30.0;

/// One ladder entry after `load_ladder`.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub name: String,
    pub project: PathBuf,
    pub source: String,
    pub args: Vec<String>,
    pub timeout_s: f64,
    pub native: bool,
    pub tags: Vec<String>,
}

/// `os.path.expandvars`: `$NAME` and `${NAME}` from the environment, unknown names kept.
pub fn expandvars(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c != '$' || index + 1 >= chars.len() {
            result.push(c);
            index += 1;
            continue;
        }
        if chars[index + 1] == '{' {
            match chars[index + 2..].iter().position(|c| *c == '}') {
                Some(end) => {
                    let name: String = chars[index + 2..index + 2 + end].iter().collect();
                    match std::env::var(&name) {
                        Ok(value) => result.push_str(&value),
                        Err(_) => {
                            result.push_str("${");
                            result.push_str(&name);
                            result.push('}');
                        }
                    }
                    index += end + 3;
                }
                None => {
                    result.extend(&chars[index..]);
                    break;
                }
            }
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        if end == index + 1 {
            result.push('$');
            index += 1;
            continue;
        }
        let name: String = chars[index + 1..end].iter().collect();
        match std::env::var(&name) {
            Ok(value) => result.push_str(&value),
            Err(_) => {
                result.push('$');
                result.push_str(&name);
            }
        }
        index = end;
    }
    result
}

fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => copper_json::python_float_repr(*number),
        Value::Str(text) => text.clone(),
        other => Dumps::new().dumps(other),
    }
}

/// `load_ladder(path)`: the boards, or the `ValueError` message.
pub fn load_ladder(path: &Path) -> Result<Vec<Board>, String> {
    let shown = paths::display(path);
    let text = std::fs::read_to_string(path).map_err(|e| paths::python_os_error(&e, Some(&shown)))?;
    let data = copper_json::parse(&text).map_err(|e| e.to_string())?;
    let boards = match data.get("boards") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        _ => return Err(format!("{shown}: expected a non-empty 'boards' list")),
    };
    let mut names: Vec<String> = Vec::new();
    let mut result = Vec::new();
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    for entry in boards {
        let name = match entry.get("name") {
            Some(Value::Str(name)) if !name.is_empty() && !names.contains(name) => name.clone(),
            _ => return Err(format!("{shown}: every board needs a unique name")),
        };
        names.push(name.clone());
        let project_text = match entry.get("project") {
            Some(value) => python_str(value),
            None => ".".to_owned(),
        };
        let mut project = PathBuf::from(expandvars(&project_text));
        if !project.is_absolute() {
            project = paths::resolve(&parent.join(project));
        }
        let source = match entry.get("source") {
            Some(value) => python_str(value),
            None => "board.copper".to_owned(),
        };
        let args = match entry.get("args") {
            Some(Value::Array(items)) => items.iter().map(|item| expandvars(&python_str(item))).collect(),
            Some(Value::Str(text)) => text.chars().map(|c| expandvars(&c.to_string())).collect(),
            _ => Vec::new(),
        };
        let timeout_s = match entry.get("timeout_s") {
            Some(Value::Int(number)) => number.to_string().parse().unwrap_or(1800.0),
            Some(Value::Float(number)) => *number,
            Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
            Some(Value::Str(text)) => text.trim().parse().map_err(|_| format!("could not convert string to float: '{text}'"))?,
            _ => 1800.0,
        };
        let native = match entry.get("native") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Null) | None => false,
            Some(Value::Int(number)) => number.to_string() != "0",
            Some(Value::Float(number)) => *number != 0.0,
            Some(Value::Str(text)) => !text.is_empty(),
            Some(Value::Array(items)) => !items.is_empty(),
            Some(Value::Object(object)) => !object.is_empty(),
        };
        let tags = match entry.get("tags") {
            Some(Value::Array(items)) => items.iter().map(python_str).collect(),
            _ => Vec::new(),
        };
        result.push(Board { name, project, source, args, timeout_s, native, tags });
    }
    Ok(result)
}

/// `placement_args`: reuse the board's saved placement, or save it on this first run.
pub fn placement_args(board: &Board, placements: Option<&Path>) -> Vec<String> {
    let Some(placements) = placements else { return Vec::new() };
    let directory = paths::resolve(placements).join(&board.name);
    let saved = directory.join("placement.state.json");
    if saved.is_file() {
        vec!["--reuse-placement".to_owned(), paths::display(&saved)]
    } else {
        vec!["--checkpoint-dir".to_owned(), paths::display(&directory)]
    }
}

/// How `run_board` starts `route-board`.
#[derive(Clone, Debug)]
pub enum Runner {
    /// This binary: `copper route-board ...`.
    Native(PathBuf),
    /// `PYTHON -m copperscript route-board ...`, as the Python runner does.
    Python(String),
}

impl Runner {
    fn command(&self, args: &[String]) -> (Vec<String>, usize) {
        match self {
            Runner::Native(exe) => {
                let mut command = vec![paths::display(exe), "route-board".to_owned()];
                command.extend(args.iter().cloned());
                (command, 1)
            }
            Runner::Python(python) => {
                let mut command = vec![python.clone(), "-m".to_owned(), "copperscript".to_owned(), "route-board".to_owned()];
                command.extend(args.iter().cloned());
                (command, 2)
            }
        }
    }
}

/// Settings of one ladder run shared by every board.
pub struct RunOptions<'a> {
    pub kicad_cli: Option<&'a str>,
    pub runner: &'a Runner,
    pub extra_args: &'a [String],
    pub placements: Option<&'a Path>,
}

fn record_base(board: &Board) -> Object {
    let mut record = Object::new();
    record.insert("name".to_owned(), Value::from(board.name.as_str()));
    record.insert("project".to_owned(), Value::from(paths::display(&board.project)));
    record.insert("tags".to_owned(), Value::Array(board.tags.iter().map(|tag| Value::from(tag.as_str())).collect()));
    record
}

/// `round(x, 1)`.
fn round1(seconds: f64) -> f64 {
    format!("{seconds:.1}").parse().unwrap_or(seconds)
}

/// The final line of the traceback an uncaught `OSError` leaves: its class and message.
fn crash_line(error: &std::io::Error, filename: Option<&str>) -> String {
    let class = match error.kind() {
        std::io::ErrorKind::AlreadyExists => "FileExistsError",
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    };
    format!("{class}: {}", paths::python_os_call_error(error, filename))
}

/// `run_board`: route one board under `out`, returning its result record.
///
/// Errors are the uncaught `OSError`s of the Python runner (an existing
/// board directory, an unwritable log).
pub fn run_board(board: &Board, out: &Path, options: &RunOptions) -> Result<Object, String> {
    // route-board runs in the project directory: report paths must not be relative.
    let directory = paths::resolve(out).join(&board.name);
    let mut record = record_base(board);
    let source = board.project.join(&board.source);
    if !source.is_file() {
        record.insert("status".to_owned(), Value::from("skipped"));
        record.insert("reason".to_owned(), Value::from(format!("missing {}", paths::display(&source))));
        return Ok(record);
    }
    // `mkdir(parents=True, exist_ok=False)`: the parents may exist, the directory itself may not.
    if let Some(parent) = directory.parent() {
        std::fs::create_dir_all(parent).map_err(|e| paths::python_os_call_error(&e, Some(&paths::display(parent))))?;
    }
    std::fs::create_dir(&directory).map_err(|e| crash_line(&e, Some(&paths::display(&directory))))?;
    let report = directory.join("route-report.json"); // what `copper issues RUN_DIR` reads
    let mut args = vec![board.source.clone()];
    args.extend(board.args.iter().cloned());
    args.extend(options.extra_args.iter().cloned());
    args.extend(placement_args(board, options.placements));
    args.extend([
        "--report".to_owned(),
        paths::display(&report),
        "-o".to_owned(),
        paths::display(&directory.join("board.kicad_pcb")),
    ]);
    if let Some(kicad_cli) = options.kicad_cli.filter(|_| board.native) {
        args.extend(["--verify-plane-fill".to_owned(), kicad_cli.to_owned()]);
    }
    let (command, skip) = options.runner.command(&args);
    let started = Instant::now();
    let log_path = directory.join("route.log");
    let mut log = std::fs::File::create(&log_path)
        .map_err(|e| paths::python_os_error(&e, Some(&paths::display(&log_path))))?;
    let _ = writeln!(log, "{}", command.join(" "));
    let _ = log.flush();
    let outcome = run_logged(&command, &board.project, &log, Duration::from_secs_f64(board.timeout_s.max(0.0)));
    let elapsed = round1(started.elapsed().as_secs_f64());
    let (code, status) = match outcome {
        Outcome::Exited(code) => (Some(code), "completed"),
        Outcome::TimedOut => (None, "timeout"),
        Outcome::Failed(message) => return Err(message),
    };
    record.insert("exit_code".to_owned(), code.map_or(Value::Null, |code| Value::from(i64::from(code))));
    record.insert("elapsed_s".to_owned(), Value::Float(elapsed));
    record.insert("command".to_owned(), Value::Array(command[skip..].iter().map(|c| Value::from(c.as_str())).collect()));
    if status == "timeout" {
        record.insert("status".to_owned(), Value::from(status));
        return Ok(record);
    }
    if !matches!(code, Some(0 | 1)) || !report.is_file() {
        record.insert("status".to_owned(), Value::from("error"));
        record.insert("reason".to_owned(), Value::from(last_line(&log_path)));
        return Ok(record);
    }
    let text = std::fs::read_to_string(&report).map_err(|e| paths::python_os_error(&e, Some(&paths::display(&report))))?;
    let document = copper_json::parse(&text).map_err(|e| e.to_string())?;
    let Some(summary) = document.get("summary").filter(|v| !matches!(v, Value::Null)) else {
        record.insert("status".to_owned(), Value::from("error"));
        record.insert("reason".to_owned(), Value::from("report has no summary block"));
        return Ok(record);
    };
    record.insert("status".to_owned(), Value::from(status));
    record.insert("route_status".to_owned(), document.get("status").cloned().unwrap_or(Value::Null));
    record.insert("summary".to_owned(), summary.clone());
    record.insert("phases".to_owned(), Value::Object(phase_seconds(&document)));
    Ok(record)
}

enum Outcome {
    Exited(i32),
    TimedOut,
    Failed(String),
}

/// `subprocess.run(command, cwd, stdout=log, stderr=STDOUT, timeout)`.
fn run_logged(command: &[String], cwd: &Path, log: &std::fs::File, timeout: Duration) -> Outcome {
    let err_log = match log.try_clone() {
        Ok(file) => file,
        Err(error) => return Outcome::Failed(paths::python_os_error(&error, None)),
    };
    let out_log = match log.try_clone() {
        Ok(file) => file,
        Err(error) => return Outcome::Failed(paths::python_os_error(&error, None)),
    };
    let mut child = match Command::new(&command[0])
        .args(&command[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_log))
        .stderr(Stdio::from(err_log))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Outcome::Failed(paths::python_os_error(&error, Some(&command[0]))),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Outcome::Exited(exit_code(status)),
            Ok(None) => {}
            Err(error) => return Outcome::Failed(paths::python_os_error(&error, None)),
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Outcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    // subprocess reports a signal as its negative number.
    status.code().or_else(|| status.signal().map(|signal| -signal)).unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// `_phase_seconds`: the numeric entries of the report's `timings`.
fn phase_seconds(document: &Value) -> Object {
    let mut result = Object::new();
    if let Some(Value::Object(timings)) = document.get("timings") {
        for (key, value) in timings {
            if matches!(value, Value::Int(_) | Value::Float(_) | Value::Bool(_)) {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    result
}

/// `_last_line`: the last non-blank line of the log, at most 300 characters.
fn last_line(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else { return String::new() };
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .map(|line| line.trim_matches(copper_json::python_isspace))
        .filter(|line| !line.is_empty())
        .last()
        .map(|line| line.chars().take(300).collect())
        .unwrap_or_default()
}

/// `_row(result)`: the summary numbers the table and the comparison read.
pub fn row(result: &Value) -> Object {
    let empty = Value::Object(Object::new());
    let summary = match result.get("summary") {
        Some(Value::Null) | None => &empty,
        Some(value) => value,
    };
    let get = |value: &Value, key: &str| value.get(key).cloned().unwrap_or(Value::Null);
    let section = |key: &str| summary.get(key).cloned().unwrap_or_else(|| empty.clone());
    let signal = section("signal");
    let plane = section("plane");
    let mut row = Object::new();
    row.insert("status".to_owned(), get(result, "status"));
    row.insert("completion".to_owned(), get(&signal, "completion"));
    row.insert("opens".to_owned(), get(&signal, "opens"));
    row.insert("connections".to_owned(), get(&signal, "connections"));
    let plane_opens = match plane.get("estimated_opens") {
        Some(value) => value.clone(),
        None => get(&plane, "explicit_opens"),
    };
    row.insert("plane_opens".to_owned(), plane_opens);
    row.insert("drc_errors".to_owned(), get(&section("drc"), "errors"));
    row.insert("native_unconnected".to_owned(), get(&section("native"), "unconnected"));
    row.insert("vias".to_owned(), get(&section("copper"), "vias"));
    row.insert("length_mm".to_owned(), get(&section("copper"), "track_length_mm"));
    row.insert("elapsed_s".to_owned(), get(result, "elapsed_s"));
    row
}

/// `value or 0` as a number.
fn or_zero(value: &Value) -> f64 {
    match value {
        Value::Int(number) => number.to_string().parse().unwrap_or(0.0),
        Value::Float(number) => *number,
        Value::Bool(true) => 1.0,
        _ => 0.0,
    }
}

fn is_none(value: &Value) -> bool {
    matches!(value, Value::Null)
}

/// `compare(baseline, candidate)`: human-readable regressions of the candidate.
pub fn compare(baseline: &Value, candidate: &Value) -> Vec<String> {
    let results = |document: &Value| -> Vec<Value> {
        match document.get("results") {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        }
    };
    let mut old: Object = Object::new();
    for item in results(baseline) {
        old.insert(python_str(&item.get("name").cloned().unwrap_or(Value::Null)), Value::Object(row(&item)));
    }
    let mut problems = Vec::new();
    for item in results(candidate) {
        let name = python_str(&item.get("name").cloned().unwrap_or(Value::Null));
        let new = row(&item);
        let Some(before) = old.get(&name).and_then(Value::as_object) else { continue };
        let status = |row: &Object| python_str(&row["status"]);
        if status(before) == "skipped" || status(&new) == "skipped" {
            continue;
        }
        if status(&new) != "completed" {
            if status(before) == "completed" {
                problems.push(format!("{name}: run {} (baseline completed)", status(&new)));
            }
            continue;
        }
        if status(before) != "completed" {
            continue;
        }
        if or_zero(&new["completion"]) < or_zero(&before["completion"]) {
            problems.push(format!(
                "{name}: completion {} -> {}",
                python_str(&before["completion"]),
                python_str(&new["completion"])
            ));
        }
        if or_zero(&new["drc_errors"]) > or_zero(&before["drc_errors"]) {
            problems.push(format!(
                "{name}: exact DRC errors {} -> {}",
                python_str(&before["drc_errors"]),
                python_str(&new["drc_errors"])
            ));
        }
        if !is_none(&before["native_unconnected"])
            && !is_none(&new["native_unconnected"])
            && or_zero(&new["native_unconnected"]) > or_zero(&before["native_unconnected"])
        {
            problems.push(format!(
                "{name}: native unconnected {} -> {}",
                python_str(&before["native_unconnected"]),
                python_str(&new["native_unconnected"])
            ));
        }
        let slow = or_zero(&before["elapsed_s"]);
        if or_zero(&new["elapsed_s"]) > (slow * SLOWDOWN_LIMIT).max(slow + SLOWDOWN_FLOOR_SECONDS) {
            problems.push(format!(
                "{name}: time {}s -> {}s",
                python_str(&before["elapsed_s"]),
                python_str(&new["elapsed_s"])
            ));
        }
    }
    problems
}

/// `markdown(document, problems)`: the results table and the regressions.
pub fn markdown(document: &Value, problems: Option<&[String]>) -> String {
    let field = |key: &str| document.get(key).cloned().unwrap_or(Value::Null);
    let commit = match field("copperscript_commit") {
        Value::Null | Value::Bool(false) => "unknown".to_owned(),
        Value::Str(text) if text.is_empty() => "unknown".to_owned(),
        other => python_str(&other),
    };
    let mut lines = vec![
        format!("# Benchmark ladder: {}", python_str(&field("label"))),
        String::new(),
        format!("CopperScript `{commit}`, {}", python_str(&field("started_utc"))),
        String::new(),
        "| Board | Status | Completion | Opens / connections | Plane opens (est.) | DRC errors | Native unconnected | Vias | Length mm | Time s |".to_owned(),
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |".to_owned(),
    ];
    let cell = |value: &Value| if is_none(value) { "–".to_owned() } else { python_str(value) };
    if let Some(Value::Array(results)) = document.get("results") {
        for item in results {
            let row = row(item);
            let opens = if is_none(&row["opens"]) {
                "–".to_owned()
            } else {
                format!("{} / {}", python_str(&row["opens"]), python_str(&row["connections"]))
            };
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                python_str(&item.get("name").cloned().unwrap_or(Value::Null)),
                python_str(&row["status"]),
                cell(&row["completion"]),
                opens,
                cell(&row["plane_opens"]),
                cell(&row["drc_errors"]),
                cell(&row["native_unconnected"]),
                cell(&row["vias"]),
                cell(&row["length_mm"]),
                cell(&row["elapsed_s"]),
            ));
        }
    }
    if let Some(problems) = problems {
        lines.extend([String::new(), "## Regressions against baseline".to_owned(), String::new()]);
        if problems.is_empty() {
            lines.push("None.".to_owned());
        } else {
            lines.extend(problems.iter().map(|item| format!("- {item}")));
        }
    }
    lines.join("\n") + "\n"
}

/// `_commit()`: the short head of the checkout this binary was built from, or None.
fn commit() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn utc_now_iso() -> String {
    let seconds = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

const USAGE: &str = "usage: copper bench [-h] --out OUT [--only ONLY] [--tags TAGS] [--label LABEL] \
[--baseline BASELINE] [--kicad-cli KICAD_CLI] [--python PYTHON] [--extra-arg EXTRA_ARGS] \
[--placements PLACEMENTS] [--jobs JOBS] ladder";

const DESCRIPTION: &str = "Benchmark ladder: route a fixed set of boards and compare completion and time.";

const VALUED: [&str; 10] = [
    "--out",
    "--only",
    "--tags",
    "--label",
    "--baseline",
    "--kicad-cli",
    "--python",
    "--extra-arg",
    "--placements",
    "--jobs",
];

/// Parsed `copper bench` arguments.
#[derive(Default)]
struct Arguments {
    help: bool,
    values: Vec<(&'static str, String)>,
    positionals: Vec<String>,
}

impl Arguments {
    fn last(&self, name: &str) -> Option<&String> {
        self.values.iter().rev().find(|(key, _)| *key == name).map(|(_, value)| value)
    }
}

fn looks_negative(value: &str) -> bool {
    value.starts_with('-') && value[1..].parse::<f64>().is_ok()
}

/// argparse with prefix matching for this command.
fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
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
        if arg == "-h" || "--help".starts_with(arg.as_str()) && arg.len() > 2 {
            parsed.help = true;
            continue;
        }
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key, Some(value.to_owned())),
            None => (arg.as_str(), None),
        };
        let matches: Vec<&'static str> = VALUED.iter().copied().filter(|o| o.starts_with(key)).collect();
        let option = match (VALUED.iter().copied().find(|o| *o == key), matches.as_slice()) {
            (Some(option), _) => option,
            (None, [option]) => option,
            (None, []) => return Err(format!("unrecognized arguments: {arg}")),
            (None, _) => return Err(format!("ambiguous option: {key} could match {}", matches.join(", "))),
        };
        let value = match inline {
            Some(value) => value,
            None => match arguments.get(index) {
                Some(value) if !value.starts_with('-') || looks_negative(value) => {
                    index += 1;
                    value.clone()
                }
                _ => return Err(format!("argument {option}: expected one argument")),
            },
        };
        parsed.values.push((option, value));
    }
    Ok(parsed)
}

/// Python's `int(text)` for `--jobs`.
fn python_int(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if !plain.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: i64 = plain.parse().ok()?;
    Some(if negative { -value } else { value })
}

/// The `copper bench` command (`pcbir.ladder.main`).
pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper bench: error: {message}");
        2
    };
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.help {
        let _ = writeln!(out, "{USAGE}\n\n{DESCRIPTION}");
        return 0;
    }
    let ladder = match parsed.positionals.as_slice() {
        [ladder] => PathBuf::from(crate::pypath_display(ladder)),
        [] => {
            let missing = if parsed.last("--out").is_none() { "ladder, --out" } else { "ladder" };
            return usage_error(err, &format!("the following arguments are required: {missing}"));
        }
        [_, extra @ ..] => return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" "))),
    };
    let Some(out_dir) = parsed.last("--out").map(|text| PathBuf::from(crate::pypath_display(text))) else {
        return usage_error(err, "the following arguments are required: --out");
    };
    let jobs = match parsed.last("--jobs") {
        None => 1,
        Some(text) => match python_int(text) {
            Some(jobs) => jobs,
            None => return usage_error(err, &format!("argument --jobs: invalid int value: '{text}'")),
        },
    };
    let mut boards = match load_ladder(&ladder) {
        Ok(boards) => boards,
        Err(message) => {
            // Python lets the ValueError escape as a traceback (exit status 1).
            let _ = writeln!(err, "ValueError: {message}");
            return 1;
        }
    };
    if let Some(only) = parsed.last("--only") {
        let wanted: Vec<&str> = only.split(',').collect();
        let mut unknown: Vec<&str> =
            wanted.iter().copied().filter(|name| !boards.iter().any(|board| board.name == *name)).collect();
        unknown.sort_unstable();
        unknown.dedup();
        if !unknown.is_empty() {
            return usage_error(err, &format!("unknown boards: {}", unknown.join(", ")));
        }
        boards.retain(|board| wanted.contains(&board.name.as_str()));
    }
    if let Some(tags) = parsed.last("--tags") {
        let tags: Vec<&str> = tags.split(',').collect();
        boards.retain(|board| board.tags.iter().any(|tag| tags.contains(&tag.as_str())));
    }
    if let Err(error) = std::fs::create_dir_all(&out_dir) {
        let _ = writeln!(err, "{}", paths::python_os_call_error(&error, Some(&paths::display(&out_dir))));
        return 1;
    }
    let label = parsed.last("--label").cloned().unwrap_or_else(|| {
        out_dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let mut document = Object::new();
    document.insert("schema".to_owned(), Value::from(SCHEMA));
    document.insert("label".to_owned(), Value::from(label));
    document.insert("started_utc".to_owned(), Value::from(utc_now_iso()));
    document.insert("copperscript_commit".to_owned(), commit().map_or(Value::Null, Value::from));
    document.insert("ladder".to_owned(), Value::from(paths::display(&ladder)));
    document.insert("results".to_owned(), Value::Array(Vec::new()));
    if jobs < 1 {
        return usage_error(err, "--jobs must be at least 1");
    }
    let runner = match parsed.last("--python") {
        Some(python) => Runner::Python(python.clone()),
        None => match std::env::current_exe() {
            Ok(exe) => Runner::Native(exe),
            Err(error) => {
                let _ = writeln!(err, "{}", paths::python_os_error(&error, None));
                return 1;
            }
        },
    };
    let extra_args: Vec<String> = parsed.values.iter().filter(|(k, _)| *k == "--extra-arg").map(|(_, v)| v.clone()).collect();
    let placements = parsed.last("--placements").map(|text| PathBuf::from(crate::pypath_display(text)));
    let options = RunOptions {
        kicad_cli: parsed.last("--kicad-cli").map(String::as_str),
        runner: &runner,
        extra_args: &extra_args,
        placements: placements.as_deref(),
    };
    let results_path = out_dir.join("results.json");
    let write_results = |document: &Object| -> Result<(), String> {
        let text = Dumps::new().indent(2).dumps(&Value::Object(document.clone())) + "\n";
        copper_json::text::write_text(&results_path, &text)
            .map_err(|e| paths::python_os_error(&e, Some(&paths::display(&results_path))))
    };
    // Results keep the ladder's board order whatever order the jobs finish in.
    let outcomes = run_all(&boards, &out_dir, &options, jobs as usize, out);
    for outcome in outcomes {
        let result = match outcome {
            Ok(result) => result,
            Err(message) => {
                let _ = writeln!(err, "{message}");
                return 1;
            }
        };
        if let Some(Value::Array(results)) = document.get_mut("results") {
            results.push(Value::Object(result));
        }
        if let Err(message) = write_results(&document) {
            let _ = writeln!(err, "{message}");
            return 1;
        }
    }
    let document = Value::Object(document);
    let mut problems = None;
    if let Some(baseline) = parsed.last("--baseline") {
        let baseline_path = PathBuf::from(crate::pypath_display(baseline));
        let baseline = match std::fs::read_to_string(&baseline_path) {
            Ok(text) => match copper_json::parse(&text) {
                Ok(value) => value,
                Err(error) => {
                    let _ = writeln!(err, "{error}");
                    return 1;
                }
            },
            Err(error) => {
                let _ = writeln!(err, "{}", paths::python_os_error(&error, Some(&paths::display(&baseline_path))));
                return 1;
            }
        };
        problems = Some(compare(&baseline, &document));
    }
    let text = markdown(&document, problems.as_deref());
    let markdown_path = out_dir.join("results.md");
    if let Err(error) = copper_json::text::write_text(&markdown_path, &text) {
        let _ = writeln!(err, "{}", paths::python_os_error(&error, Some(&paths::display(&markdown_path))));
        return 1;
    }
    let _ = copper_json::text::print(out, &text);
    u8::from(problems.as_ref().is_some_and(|items| !items.is_empty()))
}

/// Route every board, up to `jobs` at once, returning the records in ladder order.
fn run_all(
    boards: &[Board],
    out_dir: &Path,
    options: &RunOptions,
    jobs: usize,
    out: &mut dyn Write,
) -> Vec<Result<Object, String>> {
    let announce = |out: &mut dyn Write, result: &Result<Object, String>, name: &str| {
        if let Ok(result) = result {
            let row = row(&Value::Object(result.clone()));
            let _ = copper_json::text::print(
                out,
                &format!(
                    "[bench] {name}: {} completion={} opens={} drc={} time={}s",
                    python_str(&row["status"]),
                    python_str(&row["completion"]),
                    python_str(&row["opens"]),
                    python_str(&row["drc_errors"]),
                    python_str(&row["elapsed_s"])
                ),
            );
            let _ = out.flush();
        }
    };
    if jobs <= 1 {
        let mut results = Vec::new();
        for board in boards {
            let _ = copper_json::text::print(out, &format!("[bench] {} ...", board.name));
            let _ = out.flush();
            let result = run_board(board, out_dir, options);
            announce(out, &result, &board.name);
            results.push(result);
        }
        return results;
    }
    let (sender, receiver) = mpsc::channel::<(usize, Result<Object, String>)>();
    let mut results: Vec<Option<Result<Object, String>>> = (0..boards.len()).map(|_| None).collect();
    let next = std::sync::Mutex::new(0usize);
    let announced = std::sync::Mutex::new(());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(boards.len().max(1)) {
            let sender = sender.clone();
            let next = &next;
            let announced = &announced;
            scope.spawn(move || {
                loop {
                    let index = {
                        let mut guard = next.lock().unwrap_or_else(|poison| poison.into_inner());
                        let index = *guard;
                        *guard += 1;
                        index
                    };
                    let Some(board) = boards.get(index) else { break };
                    {
                        let _guard = announced.lock().unwrap_or_else(|poison| poison.into_inner());
                        println!("[bench] {} ...", board.name);
                    }
                    let result = run_board(board, out_dir, options);
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, result) in receiver {
            announce(out, &result, &boards[index].name);
            results[index] = Some(result);
        }
    });
    results.into_iter().map(|result| result.unwrap_or_else(|| Err("board not run".to_owned()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Value {
        copper_json::parse(
            r#"{"schema":"copperscript-bench/v1","label":"run","started_utc":"2026-10-27T20:00:00+00:00",
            "copperscript_commit":null,"ladder":"bench/ladder.json","results":[
            {"name":"a","project":"/p","tags":[],"status":"skipped","reason":"missing /p/board.copper"},
            {"name":"b","project":"/p","tags":["x"],"exit_code":0,"elapsed_s":12.5,"command":["copperscript","route-board","board.copper"],
             "status":"completed","route_status":"pass","summary":{"signal":{"completion":1.0,"opens":0,"connections":12},
             "plane":{"estimated_opens":2},"drc":{"errors":0},"native":{"unconnected":null},"copper":{"vias":7,"track_length_mm":123.4}},"phases":{}},
            {"name":"c","project":"/p","tags":[],"exit_code":1,"elapsed_s":0.3,"command":[],"status":"error","reason":"boom"}]}"#,
        )
        .unwrap()
    }

    fn baseline() -> Value {
        copper_json::parse(
            r#"{"results":[{"name":"b","status":"completed","elapsed_s":10.0,"summary":{"signal":{"completion":1.0,"opens":0,"connections":12},
            "drc":{"errors":0},"copper":{"vias":7,"track_length_mm":123.4}}},{"name":"c","status":"completed","elapsed_s":1.0,"summary":{}}]}"#,
        )
        .unwrap()
    }

    const TABLE: &str = "# Benchmark ladder: run\n\nCopperScript `unknown`, 2026-10-27T20:00:00+00:00\n\n\
| Board | Status | Completion | Opens / connections | Plane opens (est.) | DRC errors | Native unconnected | Vias | Length mm | Time s |\n\
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n\
| a | skipped | – | – | – | – | – | – | – | – |\n\
| b | completed | 1.0 | 0 / 12 | 2 | 0 | – | 7 | 123.4 | 12.5 |\n\
| c | error | – | – | – | – | – | – | – | 0.3 |\n";

    #[test]
    fn markdown_and_comparison_match_python() {
        let problems = compare(&baseline(), &document());
        assert_eq!(problems, ["c: run error (baseline completed)"]);
        assert_eq!(markdown(&document(), None), TABLE);
        assert_eq!(
            markdown(&document(), Some(&problems)),
            format!("{TABLE}\n## Regressions against baseline\n\n- c: run error (baseline completed)\n")
        );
        assert_eq!(markdown(&document(), Some(&[])).trim_end(), format!("{TABLE}\n## Regressions against baseline\n\nNone.").as_str());
    }

    #[test]
    fn slowdowns_and_losses_are_regressions() {
        let before = copper_json::parse(
            r#"{"results":[{"name":"b","status":"completed","elapsed_s":100.0,"summary":{"signal":{"completion":1.0},"drc":{"errors":0},"native":{"unconnected":0}}}]}"#,
        )
        .unwrap();
        let after = copper_json::parse(
            r#"{"results":[{"name":"b","status":"completed","elapsed_s":140.5,"summary":{"signal":{"completion":0.5},"drc":{"errors":2},"native":{"unconnected":3}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            compare(&before, &after),
            [
                "b: completion 1.0 -> 0.5",
                "b: exact DRC errors 0 -> 2",
                "b: native unconnected 0 -> 3",
                "b: time 100.0s -> 140.5s"
            ]
        );
    }

    #[test]
    fn environment_variables_expand_like_expandvars() {
        assert_eq!(expandvars("plain"), "plain");
        assert_eq!(expandvars("${NO_SUCH_COPPER_VAR}/x"), "${NO_SUCH_COPPER_VAR}/x");
        assert_eq!(expandvars("$NO_SUCH_COPPER_VAR/x"), "$NO_SUCH_COPPER_VAR/x");
        assert_eq!(expandvars("a$"), "a$");
        assert_eq!(expandvars("${unterminated"), "${unterminated");
    }

    #[test]
    fn utc_timestamps_are_iso_seconds() {
        let text = utc_now_iso();
        assert_eq!(text.len(), 25, "{text}");
        assert!(text.ends_with("+00:00") && &text[10..11] == "T");
    }
}

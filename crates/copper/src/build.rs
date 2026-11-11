//! `copper build SOURCE [options] -- [ROUTE-BOARD OPTIONS...]`: the atomic
//! board build behind `make route`, run inside the binary (it replaced the bash
//! script `make/route-build.sh`).
//!
//! One build runs `check` (which resolves the board's imports and physical
//! macro assets), `route-board`, a saved refill of the routed board with a
//! design-rule check, and a completion gate, into a fresh directory below the
//! project's `build/`. `run.json` there binds the outputs by SHA-256. The
//! verification is CopperScript's own refill and DRC (`--verify native`, the
//! default) or KiCad's (`--verify kicad`, which runs `kicad-cli` exactly as the
//! script does). The steps of CopperScript's own commands run in-process;
//! only `kicad-cli` and `git` (for provenance) are spawned.
//!
//! Exit status: 0 passed, 1 unmet completion gates, 2 setup or runner error,
//! 3 unsupported, 130 interrupted by the child `kicad-cli`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use copper_json::{Dumps, Object, Value, text};
use copper_lang::paths;
use sha2::{Digest, Sha256};

use crate::option_parser::{self, flag, value};

const USAGE: &str = "usage: copper build [-h] [--project-root DIR] [--output-root DIR] [--output-dir DIR] \
[--verify {native,kicad}] [--kicad-cli PATH] [--kicad-footprints DIR] [--locked] [--offline] [--dry-run] \
source [-- ROUTE_BOARD_OPTIONS ...]";

/// The schema of `run.json`.
const MANIFEST_SCHEMA: &str = "copperscript-build/v1";

/// Where `kicad-cli` is looked for when neither an option, `KICAD_CLI` nor
/// `PATH` names it (the standard Windows KiCad 10 installation).
const DEFAULT_KICAD_CLI: &str = "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe";

/// Where KiCad's footprints are looked for when they are not beside `kicad-cli`.
const DEFAULT_KICAD_FOOTPRINTS: &str = "/usr/share/kicad/footprints";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verify {
    Native,
    Kicad,
}

impl Verify {
    fn name(self) -> &'static str {
        match self {
            Verify::Native => "native",
            Verify::Kicad => "kicad",
        }
    }
}

/// A setup or runner error: reported as `SETUP ERROR: ...` with exit status 2.
struct Setup(String);

fn setup<T>(message: impl Into<String>) -> Result<T, Setup> {
    Err(Setup(message.into()))
}

// ---------------------------------------------------------------- helpers

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect());
        }
        hasher.update(&buffer[..count]);
    }
}

/// A path as it is written in commands and the manifest: forward slashes.
fn shown(path: &Path) -> String {
    paths::as_posix(path)
}

/// One argument quoted for a POSIX shell, so a recorded command can be pasted.
fn quote(argument: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !argument.is_empty() && argument.chars().all(plain) {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', r"'\''"))
    }
}

fn command_line(program: &str, arguments: &[String]) -> String {
    std::iter::once(program.to_owned())
        .chain(arguments.iter().map(|argument| quote(argument)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Seconds since the epoch as `(year, month, day, hour, minute, second)`, UTC.
fn civil(seconds: u64) -> (u64, u64, u64, u64, u64, u64) {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day, rest / 3_600, rest % 3_600 / 60, rest % 60)
}

fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

/// `2026-11-10T09:30:00Z`.
fn iso_utc(seconds: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(seconds);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261110T093000Z`, as in default run directory names.
fn compact_utc(seconds: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(seconds);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// The arguments after `--` that the runner controls itself.
fn controlled(argument: &str) -> bool {
    let name = argument.split_once('=').map_or(argument, |(name, _)| name);
    matches!(name, "-o" | "--output" | "--report" | "--verify-plane-fill" | "--locked" | "--offline")
        || (argument.starts_with("-o") && !argument.starts_with("--"))
}

/// An executable named by a path or found on `PATH` (as `command -v` does).
fn find_executable(name: &str) -> Option<PathBuf> {
    let candidates = |base: PathBuf| {
        let mut found = vec![base.clone()];
        if cfg!(windows) && base.extension().is_none() {
            found.push(base.with_extension("exe"));
        }
        found
    };
    if name.contains('/') || name.contains('\\') {
        return candidates(PathBuf::from(name)).into_iter().find(|path| path.is_file());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|directory| candidates(directory.join(name))).find(|path| path.is_file())
}

/// `kicad-cli`: the explicit path, `KICAD_CLI`, `PATH`, then the standard
/// Windows installation. `None` when nothing there is a file.
fn locate_kicad_cli(explicit: Option<&str>) -> Option<PathBuf> {
    let named = explicit.map(str::to_owned).or_else(|| std::env::var("KICAD_CLI").ok().filter(|v| !v.is_empty()));
    let chosen = match named {
        Some(named) => named,
        None => match find_executable("kicad-cli") {
            Some(found) => shown(&found),
            None => DEFAULT_KICAD_CLI.to_owned(),
        },
    };
    find_executable(&chosen).map(|path| paths::resolve(&path))
}

/// KiCad's footprint libraries: the explicit directory, else the one beside
/// `kicad-cli`, else the Linux package location.
fn locate_footprints(explicit: Option<&str>, cli: &Path) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return Some(PathBuf::from(explicit)).filter(|path| path.is_dir()).map(|path| paths::resolve(&path));
    }
    let beside = cli.parent().and_then(Path::parent).map(|root| root.join("share").join("kicad").join("footprints"));
    beside
        .filter(|path| path.is_dir())
        .or_else(|| Some(PathBuf::from(DEFAULT_KICAD_FOOTPRINTS)).filter(|path| path.is_dir()))
        .map(|path| paths::resolve(&path))
}

/// Writes to the console and to a log file.
struct Tee<'a> {
    console: &'a mut dyn Write,
    log: File,
}

impl Write for Tee<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let _ = self.console.write_all(buffer);
        self.log.write_all(buffer)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.console.flush();
        self.log.flush()
    }
}

/// A temporary file removed when dropped.
struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

// --------------------------------------------------------------- settings

struct Settings {
    source: PathBuf,
    project: PathBuf,
    output: PathBuf,
    verify: Verify,
    /// `kicad-cli` and the footprint libraries: required to verify with KiCad,
    /// otherwise only detected (`cli`) or an extra footprint root (`footprints`).
    kicad_cli: Option<PathBuf>,
    kicad_footprints: Option<PathBuf>,
    locked: bool,
    offline: bool,
    dry_run: bool,
    /// The route-board options after `--`.
    options: Vec<String>,
    started: u64,
}

/// Parse the command line into settings; `Ok(None)` when help was shown.
fn parse(arguments: &[String], out: &mut dyn Write) -> Result<Option<Settings>, Setup> {
    let (own, options) = match arguments.iter().position(|argument| argument == "--") {
        Some(split) => (&arguments[..split], arguments[split + 1..].to_vec()),
        None => (arguments, Vec::new()),
    };
    let specs = [
        value("--project-root"),
        value("--output-root"),
        value("--output-dir"),
        value("--verify"),
        value("--kicad-cli"),
        value("--kicad-footprints"),
        value("--copper"),
        flag("--locked"),
        flag("--offline"),
        flag("--dry-run"),
    ];
    let parsed = option_parser::parse_exact(own, &specs).map_err(|message| Setup(format!("{message}\n{USAGE}")))?;
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\n\
             route a board, refill and check it, and write a run.json that binds the outputs by SHA-256\n\n\
             Runs copper check, copper route-board, a saved refill with a design-rule check, and a completion\n\
             gate, in order, into a new directory below the project's build/. Everything after -- goes to\n\
             route-board; the output, report, verification and resolution options are set by the build and\n\
             rejected there. The plan is printed first. run.json records the inputs and their hashes, the\n\
             commands, the verification mode, the status and the exit code. make route runs this\n\
             command.\n\n\
             exit status: 0 passed, 1 unmet completion gates, 2 setup or runner error, 3 unsupported"
        );
        crate::help::options(out, "build");
        return Ok(None);
    }
    if parsed.flag("--copper") {
        return setup("--copper has no meaning here: copper build runs the compiler in-process");
    }
    let verify = match parsed.value("--verify") {
        None | Some("native") => Verify::Native,
        Some("kicad") => Verify::Kicad,
        Some(other) => {
            return setup(format!("--verify: invalid choice: '{other}' (choose from native, kicad)\n{USAGE}"));
        }
    };
    let source = match parsed.positionals.as_slice() {
        [] => return setup("a SOURCE board is required"),
        [source] => source.clone(),
        [_, extra, ..] => return setup(format!("unexpected argument: {extra}")),
    };
    if !Path::new(&source).is_file() {
        return setup(format!("source missing: {source}"));
    }
    if options.iter().any(|option| controlled(option)) {
        return setup("output, report, verification and resolution flags are controlled by the build runner");
    }
    let started = now_seconds();
    let kicad_value = parsed.value("--kicad-cli");
    let footprints_value = parsed.value("--kicad-footprints");
    let kicad_cli = locate_kicad_cli(kicad_value);
    if let (Some(named), None) = (kicad_value, &kicad_cli) {
        return setup(format!("kicad-cli missing: {named}"));
    }
    // A native build resolves footprints the same way on every machine: only
    // an explicit directory is passed, never one found by looking for KiCad.
    let kicad_footprints = match (footprints_value, &kicad_cli, verify) {
        (Some(explicit), _, _) => {
            Some(locate_footprints(Some(explicit), Path::new("")).ok_or_else(|| match verify {
                Verify::Kicad => Setup("KiCad CLI or footprint directory missing; supply explicit paths".into()),
                Verify::Native => Setup(format!("KiCad footprint directory missing: {explicit}")),
            })?)
        }
        (None, Some(cli), Verify::Kicad) => locate_footprints(None, cli),
        _ => None,
    };
    if verify == Verify::Kicad && (kicad_cli.is_none() || kicad_footprints.is_none()) {
        return setup("KiCad CLI or footprint directory missing; supply explicit paths");
    }

    let project_text = parsed.value("--project-root").map_or_else(|| ".".to_owned(), str::to_owned);
    let project_path = Path::new(&project_text);
    if !project_path.is_dir() {
        return setup(format!("project root missing: {project_text}"));
    }
    let project = paths::resolve(project_path);
    let source = paths::resolve(Path::new(&source));
    if !source.starts_with(&project) {
        return setup("source must be inside --project-root");
    }
    let build = project.join("build");
    let stem = source.file_stem().map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let output_root =
        parsed.value("--output-root").map_or_else(|| build.join(&stem), |root| paths::resolve(Path::new(root)));
    let output = match parsed.value("--output-dir") {
        Some(directory) => paths::resolve(Path::new(directory)),
        None => output_root.join(format!("{}-{}", compact_utc(started), std::process::id())),
    };
    if !output.starts_with(&build) || output == build {
        return setup("output must stay below project build/");
    }
    if output.exists() && (!output.is_dir() || fs::read_dir(&output).is_ok_and(|mut entries| entries.next().is_some()))
    {
        return setup(format!("output is not empty: {}", shown(&output)));
    }
    Ok(Some(Settings {
        source,
        project,
        output,
        verify,
        kicad_cli,
        kicad_footprints,
        locked: parsed.flag("--locked"),
        offline: parsed.flag("--offline"),
        dry_run: parsed.flag("--dry-run"),
        options,
        started,
    }))
}

// ------------------------------------------------------------------ plan

/// The commands of one build, as argument lists (the program is `copper`
/// except for `kicad-cli`).
struct Plan {
    route: Vec<String>,
    /// Native: `copper fill`. KiCad: the combined refill and DRC.
    fill: Vec<String>,
    /// Native only: `copper drc`.
    drc: Option<Vec<String>>,
    drc_report: PathBuf,
}

fn plan(settings: &Settings) -> Plan {
    let output = &settings.output;
    let board = shown(&output.join("board.kicad_pcb"));
    let mut route = vec!["route-board".to_owned(), shown(&settings.source)];
    if settings.locked {
        route.push("--locked".to_owned());
    }
    route.push("--offline".to_owned());
    if let Some(footprints) = &settings.kicad_footprints {
        route.extend(["--footprint-root".to_owned(), shown(footprints)]);
    }
    route.extend(settings.options.iter().cloned());
    let checker = match (settings.verify, &settings.kicad_cli) {
        (Verify::Kicad, Some(cli)) => shown(cli),
        _ => "native".to_owned(),
    };
    route.extend([
        "--verify-plane-fill".to_owned(),
        checker,
        "--report".to_owned(),
        shown(&output.join("route-report.json")),
        "-o".to_owned(),
        board.clone(),
    ]);
    match settings.verify {
        Verify::Kicad => {
            let report = output.join("kicad-drc.json");
            let cli = settings.kicad_cli.as_ref().map_or_else(String::new, |cli| shown(cli));
            let fill =
                ["pcb", "drc", "--refill-zones", "--save-board", "--severity-all", "--format", "json", "--output"]
                    .map(str::to_owned)
                    .into_iter()
                    .chain([shown(&report), board])
                    .collect::<Vec<_>>();
            // The program is carried as the first argument.
            let mut with_program = vec![cli];
            with_program.extend(fill);
            Plan { route, fill: with_program, drc: None, drc_report: report }
        }
        Verify::Native => {
            let filled = shown(&output.join("board.filled.kicad_pcb"));
            let report = output.join("native-drc.json");
            Plan {
                route,
                fill: vec!["fill".to_owned(), board.clone(), "-o".to_owned(), filled],
                drc: Some(vec!["drc".to_owned(), board, "--report".to_owned(), shown(&report)]),
                drc_report: report,
            }
        }
    }
}

fn plan_lines(settings: &Settings, plan: &Plan) -> Vec<String> {
    let mut lines = vec![format!(
        "Build {} (verify: {}{})",
        shown(&settings.source),
        settings.verify.name(),
        if settings.dry_run { ", dry run" } else { "" }
    )];
    lines.push(format!("Output: {}", shown(&settings.output)));
    let mut check = vec![shown(&settings.source)];
    if settings.locked {
        check.push("--locked".to_owned());
    }
    if settings.offline || settings.dry_run {
        check.push("--offline".to_owned());
    }
    check.extend(["--macro-inputs".to_owned(), "<list of macro inputs>".to_owned()]);
    let mut steps = vec![
        ("check", format!("copper check {}", check.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "))),
        ("route", command_line("copper", &plan.route)),
    ];
    match &plan.drc {
        Some(drc) => {
            steps.push(("fill", format!("{}  (then replaces board.kicad_pcb)", command_line("copper", &plan.fill))));
            steps.push(("drc", command_line("copper", drc)));
        }
        None => steps.push(("fill", command_line(&plan.fill[0], &plan.fill[1..]))),
    }
    steps.push((
        "gate",
        "route report passes, ERC passes, routing complete, planes verified, no unrouted ordinary nets, \
         no DRC violations or unconnected items"
            .to_owned(),
    ));
    steps.push(("manifest", format!("{}/run.json binds the outputs by SHA-256", shown(&settings.output))));
    lines.push("Steps:".to_owned());
    for (index, (name, what)) in steps.iter().enumerate() {
        lines.push(format!("  {}. {name:<8} {what}", index + 1));
    }
    if settings.verify == Verify::Native {
        lines.push(match &settings.kicad_cli {
            Some(cli) => {
                format!("KiCad cross-check: kicad-cli found at {}; run with --verify kicad to use it", shown(cli))
            }
            None => "KiCad cross-check: kicad-cli not found (not needed for --verify native)".to_owned(),
        });
    }
    lines
}

// ------------------------------------------------------------ provenance

/// Project-owned sources and manifests, excluding generated and cache trees.
fn project_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if !matches!(name.as_str(), "build" | "dist" | ".git" | ".copper-cache" | "target") {
                project_files(&entry.path(), found);
            }
        } else if kind.is_file()
            && (name.ends_with(".copper")
                || name.ends_with(".mk")
                || matches!(name.as_str(), "copper.mod" | "copper.lock" | "Makefile"))
        {
            found.push(entry.path());
        }
    }
}

/// The files named by `--hard-macro` and `--placement-templates`.
fn option_files(options: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (index, option) in options.iter().enumerate() {
        let path = match option.split_once('=') {
            Some(("--hard-macro" | "--placement-templates", path)) => Some(path),
            _ if matches!(option.as_str(), "--hard-macro" | "--placement-templates") => {
                options.get(index + 1).map(String::as_str).filter(|path| !path.is_empty())
            }
            _ => None,
        };
        found.extend(path.map(|path| paths::resolve(Path::new(path))));
    }
    found
}

/// Relative path to the SHA-256 of every input file, in byte order of the
/// absolute paths.
fn input_hashes(settings: &Settings, macro_inputs: &str) -> Object {
    let mut files = vec![settings.source.clone()];
    files.extend(macro_inputs.lines().filter(|line| !line.is_empty()).map(|line| paths::resolve(Path::new(line))));
    project_files(&settings.project, &mut files);
    files.extend(option_files(&settings.options));
    let mut named: Vec<(String, PathBuf)> = files.into_iter().map(|path| (paths::display(&path), path)).collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    named.dedup_by(|a, b| a.0 == b.0);
    let mut inputs = Object::new();
    for (_, path) in named {
        if !path.is_file() {
            continue;
        }
        let Ok(hash) = sha256_file(&path) else { continue };
        let key = path.strip_prefix(&settings.project).map_or_else(|_| shown(&path), shown);
        inputs.insert(key, Value::from(hash));
    }
    inputs
}

fn git(project: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn trimmed(bytes: Option<Vec<u8>>) -> String {
    String::from_utf8_lossy(&bytes.unwrap_or_default()).trim_end_matches('\n').to_owned()
}

fn provenance(settings: &Settings, inputs: Object) -> Result<Value, Setup> {
    let commit = trimmed(git(&settings.project, &["rev-parse", "HEAD"]));
    let changes = trimmed(git(&settings.project, &["status", "--porcelain", "--untracked-files=no"]));
    let diff = git(&settings.project, &["diff", "HEAD", "--no-ext-diff"]).unwrap_or_default();
    let executable =
        std::env::current_exe().map_err(|error| Setup(format!("cannot locate the copper executable: {error}")))?;
    let compiler_sha =
        sha256_file(&executable).map_err(|error| Setup(format!("cannot read {}: {error}", shown(&executable))))?;
    let mut compiler = Object::new();
    compiler.insert("executable".to_owned(), Value::from(shown(&executable)));
    compiler.insert("sha256".to_owned(), Value::from(compiler_sha));
    compiler.insert("version".to_owned(), Value::from(crate::commands::version()));
    let mut document = Object::new();
    document.insert("platform".to_owned(), Value::from(format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)));
    document.insert("build_runner".to_owned(), Value::from("copper build"));
    document.insert("git_commit".to_owned(), if commit.is_empty() { Value::Null } else { Value::from(commit) });
    document.insert("tracked_changes".to_owned(), Value::from(changes));
    document.insert("tracked_diff_sha256".to_owned(), Value::from(sha256_hex(&diff)));
    document.insert("compiler".to_owned(), Value::Object(compiler));
    document.insert("input_sha256".to_owned(), Value::Object(inputs));
    Ok(Value::Object(document))
}

// -------------------------------------------------------------- manifest

fn dumps(manifest: &Object) -> String {
    Dumps::new().indent(2).ensure_ascii(false).dumps(&Value::Object(manifest.clone())) + "\n"
}

/// Replace `run.json` whole: a reader sees the old manifest or the new one.
fn write_manifest(path: &Path, manifest: &Object) -> io::Result<()> {
    let staging = path.with_extension("json.tmp");
    let written = fs::write(&staging, dumps(manifest)).and_then(|()| fs::rename(&staging, path));
    if written.is_err() {
        let _ = fs::remove_file(&staging);
    }
    written
}

fn read_json(path: &Path) -> Option<Value> {
    copper_json::parse(&fs::read_to_string(path).ok()?).ok()
}

fn is_empty_list(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(<[Value]>::is_empty)
}

/// Whether the route report and the DRC report meet every completion gate.
fn completed(route: &Value, drc: &Value) -> bool {
    let connectivity = route.get("connectivity");
    let true_at = |value: Option<&Value>| value == Some(&Value::Bool(true));
    route.get("status").and_then(Value::as_str) == Some("pass")
        && true_at(route.get("erc_pass"))
        && true_at(route.get("routing_complete"))
        && true_at(connectivity.and_then(|c| c.get("native_fill_verified")))
        && is_empty_list(connectivity.and_then(|c| c.get("unrouted_ordinary_nets")))
        && is_empty_list(drc.get("violations"))
        && is_empty_list(drc.get("unconnected_items"))
}

fn status_name(code: i32) -> &'static str {
    match code {
        0 => "passed",
        1 => "unmet_gates",
        3 => "unsupported",
        130 => "interrupted",
        _ => "error",
    }
}

// ------------------------------------------------------------- the steps

/// Run `kicad-cli`, copying its output to the console and `log` as it arrives.
fn run_kicad(program: &str, arguments: &[String], mut log: File, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let spawned = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let message = format!("SETUP ERROR: cannot run {program}: {error}");
            let _ = text::print(err, &message);
            let _ = writeln!(log, "{message}");
            return 2;
        }
    };
    let (sender, receiver) = mpsc::channel::<(bool, Vec<u8>)>();
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(pump(stdout, false, sender.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(pump(stderr, true, sender.clone()));
    }
    drop(sender);
    for (is_error, chunk) in receiver {
        let _ = log.write_all(&chunk);
        let _ = if is_error { err.write_all(&chunk) } else { out.write_all(&chunk) };
    }
    for reader in readers {
        let _ = reader.join();
    }
    child.wait().ok().and_then(|status| status.code()).unwrap_or(2)
}

fn pump(
    mut stream: impl Read + Send + 'static,
    is_error: bool,
    sender: mpsc::Sender<(bool, Vec<u8>)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 || sender.send((is_error, buffer[..count].to_vec())).is_err() {
                break;
            }
        }
    })
}

/// The native refill of the routed board and its design-rule check, in
/// process. The filled board replaces `board.kicad_pcb` whole.
fn native_fill(
    output: &Path,
    plan: &Plan,
    route_report: Option<&Value>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let board = output.join("board.kicad_pcb");
    let filled = output.join("board.filled.kicad_pcb");
    let code = crate::fill::run(&plan.fill[1..], out, err);
    if code != 0 {
        let _ = fs::remove_file(&filled);
        return i32::from(code);
    }
    if let Err(error) = fs::rename(&filled, &board) {
        let _ =
            text::print(out, &format!("FILL ERROR: cannot replace {} with the filled board: {error}", shown(&board)));
        let _ = fs::remove_file(&filled);
        return 2;
    }
    let mut drc = plan.drc.clone().unwrap_or_default();
    let expected = route_report
        .and_then(|report| report.get("plane_verification"))
        .and_then(|verification| verification.get("expected_open_ends"));
    if expected.and_then(Value::as_array).is_some_and(|ends| !ends.is_empty()) {
        drc.extend(["--expected-open-ends".to_owned(), shown(&output.join("route-report.json"))]);
    }
    // 0: clean; 1: findings, which the completion gate reads from the report.
    match crate::drc::run(&drc[1..], out, err) {
        0 | 1 => 0,
        other => i32::from(other),
    }
}

/// Run `copper check` with the macro-input list. Its report is not shown
/// unless it fails.
fn check(settings: &Settings, list: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), Setup> {
    let mut arguments = vec![shown(&settings.source)];
    if settings.locked {
        arguments.push("--locked".to_owned());
    }
    if settings.offline || settings.dry_run {
        arguments.push("--offline".to_owned());
    }
    arguments.extend(["--macro-inputs".to_owned(), shown(list)]);
    let mut captured = Vec::new();
    let code = crate::check::run(&arguments, &mut captured, err);
    if code == 0 {
        return Ok(());
    }
    let _ = out.write_all(&captured);
    setup(format!("copper check failed for {}", shown(&settings.source)))
}

fn unique_name() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.subsec_nanos());
    format!("copper-build-{}-{nanos}.macro-inputs", std::process::id())
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    match build(arguments, out, err) {
        Ok(code) => u8::try_from(code).unwrap_or(2),
        Err(Setup(message)) => {
            let _ = text::print(err, &format!("SETUP ERROR: {message}"));
            2
        }
    }
}

fn build(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, Setup> {
    let Some(settings) = parse(arguments, out)? else { return Ok(0) };
    let plan = plan(&settings);
    for line in plan_lines(&settings, &plan) {
        let _ = text::print(out, &line);
    }
    let list = Temporary(std::env::temp_dir().join(unique_name()));
    check(&settings, &list.0, out, err)?;
    if settings.dry_run {
        let _ = text::print(out, "Dry run: nothing was written.");
        return Ok(0);
    }
    let macro_inputs = fs::read_to_string(&list.0).unwrap_or_default();
    let provenance = provenance(&settings, input_hashes(&settings, &macro_inputs))?;
    drop(list);

    let output = &settings.output;
    fs::create_dir_all(output).map_err(|error| Setup(format!("cannot create {}: {error}", shown(output))))?;
    let manifest_path = output.join("run.json");
    let mut manifest = Object::new();
    manifest.insert("schema".to_owned(), Value::from(MANIFEST_SCHEMA));
    manifest.insert("started_utc".to_owned(), Value::from(iso_utc(settings.started)));
    manifest.insert("status".to_owned(), Value::from("running"));
    manifest.insert("verify".to_owned(), Value::from(settings.verify.name()));
    manifest.insert("routing_command".to_owned(), Value::from(command_line("copper", &plan.route)));
    let fill_command = match &plan.drc {
        Some(_) => command_line("copper", &plan.fill),
        None => command_line(&plan.fill[0], &plan.fill[1..]),
    };
    manifest.insert("fill_command".to_owned(), Value::from(fill_command));
    if let Some(drc) = &plan.drc {
        manifest.insert("drc_command".to_owned(), Value::from(command_line("copper", drc)));
    }
    manifest.insert("fabrication_ready".to_owned(), Value::Bool(false));
    manifest.insert("provenance".to_owned(), provenance);
    // Claim the run exclusively: never overwrite another caller's evidence.
    match OpenOptions::new().write(true).create_new(true).open(&manifest_path) {
        Ok(mut claimed) => {
            claimed
                .write_all(dumps(&manifest).as_bytes())
                .map_err(|error| Setup(format!("cannot write {}: {error}", shown(&manifest_path))))?;
        }
        Err(_) => return setup(format!("output is not empty: {}", shown(output))),
    }

    let tick = Instant::now();
    let log = |name: &str| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(output.join(name))
            .map_err(|error| Setup(format!("cannot create {}: {error}", shown(&output.join(name)))))
    };
    let routing_log = log("routing.log")?;
    let mut tee_out = Tee { console: &mut *out, log: routing_log.try_clone().map_err(|e| Setup(e.to_string()))? };
    let mut tee_err = Tee { console: &mut *err, log: routing_log };
    let routed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::route_board::run(&plan.route[1..], &mut tee_out, &mut tee_err)
    }));
    let mut code = match routed {
        Ok(code) => i32::from(code),
        Err(_) => {
            let _ = text::print(&mut tee_err, "ROUTING ERROR: route-board stopped on an internal error");
            2
        }
    };
    drop((tee_out, tee_err));

    let board = output.join("board.kicad_pcb");
    let intent = output.join("board.editor-intent.json");
    if intent.is_file()
        && board.is_file()
        && let (Ok(intent_hash), Ok(board_hash)) = (sha256_file(&intent), sha256_file(&board))
    {
        manifest.insert("editor_intent_sha256".to_owned(), Value::from(intent_hash));
        manifest.insert("draft_board_sha256".to_owned(), Value::from(board_hash));
    }
    if code == 0 {
        let fill_log = log("fill.log")?;
        let route_report = read_json(&output.join("route-report.json"));
        code = match settings.verify {
            Verify::Kicad => run_kicad(&plan.fill[0], &plan.fill[1..], fill_log, &mut *out, &mut *err),
            Verify::Native => {
                let mut tee_out =
                    Tee { console: &mut *out, log: fill_log.try_clone().map_err(|e| Setup(e.to_string()))? };
                let mut tee_err = Tee { console: &mut *err, log: fill_log };
                native_fill(output, &plan, route_report.as_ref(), &mut tee_out, &mut tee_err)
            }
        };
        if code == 0 {
            let gate = match (route_report.as_ref(), read_json(&plan.drc_report)) {
                (Some(route), Some(drc)) => completed(route, &drc) && board.is_file(),
                _ => false,
            };
            if !gate {
                code = 1;
            }
        }
        if code == 0
            && let (Ok(board_hash), Ok(drc_hash)) = (sha256_file(&board), sha256_file(&plan.drc_report))
        {
            manifest.insert("filled_board_sha256".to_owned(), Value::from(board_hash));
            manifest.insert("native_drc_sha256".to_owned(), Value::from(drc_hash));
            let report_name =
                plan.drc_report.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            manifest.insert("drc_report".to_owned(), Value::from(report_name));
        }
    }
    manifest.insert("status".to_owned(), Value::from(status_name(code)));
    manifest.insert("exit_code".to_owned(), Value::from(i64::from(code)));
    manifest.insert("elapsed_seconds".to_owned(), Value::from(tick.elapsed().as_secs()));
    manifest.insert("finished_utc".to_owned(), Value::from(iso_utc(now_seconds())));
    if let Err(error) = write_manifest(&manifest_path, &manifest) {
        let _ = writeln!(err, "MANIFEST ERROR: cannot write {}: {error}", shown(&manifest_path));
        code = 2;
    }
    let _ = text::print(out, &format!("Build exit code: {code}; outputs: {}", shown(output)));
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_text_matches_known_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(compact_utc(951_782_400), "20000229T000000Z");
    }

    #[test]
    fn runner_controlled_flags_are_recognised() {
        for flag in
            ["-o", "--output", "--output=x", "--report", "--verify-plane-fill", "--locked", "--offline", "-ofile"]
        {
            assert!(controlled(flag), "{flag}");
        }
        for flag in ["--layers", "--footprint-root", "--outline", "--hard-macro", "4"] {
            assert!(!controlled(flag), "{flag}");
        }
    }

    #[test]
    fn commands_are_quoted_for_a_shell() {
        assert_eq!(quote("C:/a/b.copper"), "C:/a/b.copper");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn the_gate_needs_every_condition() {
        let route = |status: &str, unrouted: &str| {
            copper_json::parse(&format!(
                r#"{{"status":"{status}","erc_pass":true,"routing_complete":true,
                "connectivity":{{"native_fill_verified":true,"unrouted_ordinary_nets":{unrouted}}}}}"#
            ))
            .unwrap()
        };
        let drc = |violations: &str| {
            copper_json::parse(&format!(r#"{{"violations":{violations},"unconnected_items":[]}}"#)).unwrap()
        };
        assert!(completed(&route("pass", "[]"), &drc("[]")));
        assert!(!completed(&route("fail", "[]"), &drc("[]")));
        assert!(!completed(&route("pass", r#"["N"]"#), &drc("[]")));
        assert!(!completed(&route("pass", "[]"), &drc("[{}]")));
    }

    #[test]
    fn statuses_follow_the_exit_codes() {
        let names: Vec<_> = [0, 1, 2, 3, 130, 4].map(status_name).to_vec();
        assert_eq!(names, ["passed", "unmet_gates", "error", "unsupported", "interrupted", "error"]);
    }
}

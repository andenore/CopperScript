//! `copper sim BOARD --watch`: run the benches, then rerun the benches a
//! changed file affects, until interrupted.
//!
//! The workspace has no file-watching dependency, so this polls modification
//! times and sizes (every [`POLL`]) of every file under the board's directory
//! (sources, parts, benches, local model files), skipping the output
//! directory, `build/`, `target/` and hidden directories, and of the
//! project's vendor model cache (`.copper-cache/spice/`, filled by `copper
//! models fetch`). Changes are debounced: the rerun starts once nothing has
//! changed for [`QUIET`]. A changed bench file reruns the benches it
//! declares (and drops the results of benches it no longer declares); any
//! other change (board, parts, models, a fixture, which any bench may use)
//! reruns every bench, and so does every change after a run that could not
//! write its results (a compile error), so the results never pair the
//! sources of one version with runs of another. The results document and
//! pages hold the latest run of every bench, under the source identity of
//! the files they were run from. Files of package dependencies outside the
//! board's directory are not watched.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use copper_json::text;
use copper_lang::compiler::{LoadError, load_design};
use copper_simulate::{
    BenchRun, ModelChoice, SimulateError, failed, simulate_matching_with_layout, write_outputs_from,
};

/// How often the files are checked.
pub const POLL: Duration = Duration::from_millis(300);
/// How long nothing may change before a rerun starts.
pub const QUIET: Duration = Duration::from_millis(500);

/// Modification time and size of every watched file.
pub type Snapshot = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

/// The directories a watch of `board` covers, and the ones it skips.
pub fn roots(board: &Path, out: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let directory =
        copper_lang::paths::resolve(board).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let cache = copper_lang::spice_models::cache_directory(&directory);
    let mut roots = vec![directory.clone()];
    if !cache.starts_with(&directory) {
        roots.push(cache);
    }
    let skip = vec![copper_lang::paths::resolve(out), directory.join("build"), directory.join("target")];
    (roots, skip)
}

/// Every file under `roots`, not under `skip` and not in a hidden directory
/// (except the model cache, which is a root of its own).
pub fn snapshot(roots: &[PathBuf], skip: &[PathBuf]) -> Snapshot {
    let mut files = Snapshot::new();
    for root in roots {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
                    if !hidden && !skip.iter().any(|skipped| path.starts_with(skipped)) {
                        pending.push(path);
                    }
                } else if meta.is_file() {
                    files.insert(path, (meta.modified().ok(), meta.len()));
                }
            }
        }
    }
    files
}

/// The files added, removed or changed between two snapshots.
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<PathBuf> {
    let mut changed: Vec<PathBuf> =
        new.iter().filter(|(path, stamp)| old.get(*path) != Some(stamp)).map(|(path, _)| path.clone()).collect();
    changed.extend(old.keys().filter(|path| !new.contains_key(*path)).cloned());
    changed.sort();
    changed
}

/// The bench sources that declare benches (resolved): every file of the
/// board's `benches/` but its fixtures. A file that does not parse is not
/// among them, so its change reruns every bench (and reports the error).
pub fn bench_declarations(board: &Path) -> Vec<PathBuf> {
    copper_lang::bench::bench_sources(board)
        .iter()
        .filter(|path| {
            let Ok(source) = std::fs::read_to_string(path) else { return false };
            copper_lang::parse(&source, &copper_lang::paths::display(path))
                .is_ok_and(|document| document.kind != copper_lang::syntax::DocumentKind::Fixture)
        })
        .map(|path| copper_lang::paths::resolve(path))
        .collect()
}

/// What a set of changed files reruns: the benches of changed bench files
/// (`Some`), or every bench (`None`) when anything else changed. `bench_files`
/// are the files that declare benches ([`bench_declarations`]); a fixture is
/// not one, since any bench may use it.
pub fn affected(changed: &[PathBuf], bench_files: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in changed {
        let path = copper_lang::paths::resolve(path);
        if bench_files.contains(&path) {
            files.push(path);
        } else {
            return None;
        }
    }
    Some(files)
}

/// The options of a watched run.
pub struct Watch {
    pub board: PathBuf,
    pub out: PathBuf,
    pub only: Option<String>,
    pub choice: ModelChoice,
    pub locked: bool,
    pub offline: bool,
    /// `--layout RUN`: the routed board for the benches that use layout, read again on every run.
    pub layout: Option<String>,
    /// Stop after this many runs (the first included); `None` runs until interrupted.
    pub runs: Option<usize>,
}

/// Run the benches once, then on every change; returns the last run's exit status when `runs` is reached.
pub fn run(watch: &Watch, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let (roots, skip) = roots(&watch.board, &watch.out);
    let mut latest: Vec<BenchRun> = Vec::new();
    let mut seen = snapshot(&roots, &skip);
    let (mut status, mut written) = run_once(watch, None, &mut latest, out, err);
    let mut count = 1;
    let shown: Vec<String> = roots.iter().map(|root| copper_lang::paths::display(root)).collect();
    let _ = text::print(out, &format!("watching {} files under {}; Ctrl-C stops", seen.len(), shown.join(" and ")));
    loop {
        if watch.runs.is_some_and(|runs| count >= runs) {
            return status;
        }
        std::thread::sleep(POLL);
        let now = snapshot(&roots, &skip);
        let mut changed = changes(&seen, &now);
        if changed.is_empty() {
            continue;
        }
        // Debounce: wait until the files have been still for QUIET.
        let mut last = now;
        let mut still = Instant::now();
        while still.elapsed() < QUIET {
            std::thread::sleep(POLL.min(QUIET));
            let again = snapshot(&roots, &skip);
            let more = changes(&last, &again);
            if !more.is_empty() {
                changed.extend(more);
                still = Instant::now();
            }
            last = again;
        }
        changed.sort();
        changed.dedup();
        seen = last;
        // After a run that wrote nothing, the results are of older sources than the last change.
        let selection = if written { affected(&changed, &bench_declarations(&watch.board)) } else { None };
        let names: Vec<String> = changed.iter().map(|path| copper_lang::paths::display(path)).collect();
        let what = if selection.is_some() { "rerunning its benches" } else { "rerunning every bench" };
        let _ = text::print(out, &format!("changed: {}; {what}", names.join(", ")));
        (status, written) = run_once(watch, selection.as_deref(), &mut latest, out, err);
        count += 1;
    }
}

/// One run: compile, run the selected benches (all when `files` is `None`),
/// print, and write the latest result of every bench. Returns the exit
/// status and whether the results were written.
fn run_once(
    watch: &Watch,
    files: Option<&[PathBuf]>,
    latest: &mut Vec<BenchRun>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> (u8, bool) {
    let design = match load_design(&watch.board, watch.locked, watch.offline) {
        Ok(design) => design,
        Err(failure @ (LoadError::Compile(_) | LoadError::Load(_))) => {
            let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
            return (2, false);
        }
    };
    let keep = |bench: &copper_lang::bench::Bench| match files {
        None => true,
        Some(files) => files.contains(&copper_lang::paths::resolve(Path::new(bench.location.filename.as_ref()))),
    };
    let layout = crate::sim_layout::input(&watch.board, &design, watch.only.as_deref(), watch.layout.as_deref(), out);
    let runs =
        match simulate_matching_with_layout(&watch.board, &design, watch.only.as_deref(), &keep, watch.choice, &layout)
        {
            Ok(runs) => runs,
            Err(SimulateError::Compile(failure)) => {
                let _ = text::print(err, &format!("COMPILE ERROR: {failure}"));
                return (2, false);
            }
            Err(SimulateError::NoBench(message)) => {
                let _ = text::print(err, &format!("SIM ERROR: {message}"));
                return (2, false);
            }
        };
    let _ = out.write_all(copper_simulate::results::text_table(&runs).as_bytes());
    // The latest run of every bench, in run order of the first run. A rerun
    // file's old results go: a bench it no longer declares has no run now.
    match files {
        None => latest.clear(),
        Some(files) => latest.retain(|run| !files.contains(&copper_lang::paths::resolve(Path::new(&run.file)))),
    }
    for run in runs {
        match latest.iter_mut().find(|other| other.name == run.name) {
            Some(slot) => *slot = run,
            None => latest.push(run),
        }
    }
    let source = copper_simulate::evidence::SourceIdentity::of(&watch.board, &design);
    match write_outputs_from(latest, &design.electrical.name, Some(&source), &watch.out) {
        Ok(paths) => {
            let _ =
                text::print(out, &format!("wrote {} ({} files)", copper_lang::paths::display(&watch.out), paths.len()));
        }
        Err(failure) => {
            let _ = text::print(
                err,
                &format!("SIM ERROR: cannot write {}: {failure}", copper_lang::paths::display(&watch.out)),
            );
            return (1, false);
        }
    }
    (u8::from(failed(latest)), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_and_what_they_rerun() {
        // Resolved, and joined a component at a time, as the watcher names the files it compares: on Windows the
        // temporary directory is an 8.3 path (RUNNER~1) that resolving expands, and `/` is not its separator.
        let directory = copper_lang::paths::resolve(
            &std::env::temp_dir().join(format!("copper-sim-watch-unit-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("benches")).unwrap();
        std::fs::create_dir_all(directory.join("build/sim")).unwrap();
        std::fs::create_dir_all(directory.join(".git")).unwrap();
        let board = directory.join("board.copper");
        let bench = directory.join("benches").join("a.copper");
        std::fs::write(&board, "board B {}").unwrap();
        std::fs::write(&bench, "bench A for B { analysis op; }").unwrap();
        std::fs::write(directory.join("build/sim/A.html"), "page").unwrap();
        std::fs::write(directory.join(".git/HEAD"), "ref").unwrap();
        let (roots, skip) = roots(&board, &directory.join("build/sim"));
        let before = snapshot(&roots, &skip);
        assert_eq!(before.len(), 2, "the board and the bench; not build/ or .git: {before:?}");
        // A bench file's change reruns its benches; a board change reruns all.
        std::fs::write(&bench, "bench A for B { analysis op; measure m = V(X); }").unwrap();
        std::fs::write(directory.join("build/sim/A.html"), "a new page").unwrap();
        let after = snapshot(&roots, &skip);
        let changed = changes(&before, &after);
        assert_eq!(changed, vec![bench.clone()]);
        let bench_files = vec![copper_lang::paths::resolve(&bench)];
        assert_eq!(affected(&changed, &bench_files), Some(bench_files.clone()));
        assert_eq!(affected(std::slice::from_ref(&board), &bench_files), None);
        // A fixture declares no bench: any bench may use it, so it reruns all.
        let fixture = directory.join("benches").join("f.copper");
        std::fs::write(&fixture, "fixture F for B { param P = 1V; }").unwrap();
        let declarations = bench_declarations(&board);
        assert_eq!(declarations, bench_files, "the bench file, not the fixture");
        assert_eq!(affected(std::slice::from_ref(&fixture), &declarations), None);
        std::fs::remove_file(&fixture).unwrap();
        // A removed file is a change.
        std::fs::remove_file(&bench).unwrap();
        assert_eq!(changes(&after, &snapshot(&roots, &skip)), vec![bench]);
        let _ = std::fs::remove_dir_all(&directory);
    }
}

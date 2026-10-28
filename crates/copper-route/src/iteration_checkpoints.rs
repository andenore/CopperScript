//! Iteration records of `route-board` (`pcbir.iteration_checkpoints`): the
//! stage tracker and the error record of a failed run. Placement checkpoints
//! (`--checkpoint-dir`, `--reuse-placement`) are not ported yet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use copper_json::{Dumps, Object, Value};

use crate::progress::Progress;

pub const ERROR_SCHEMA: &str = "copperscript-route-error/v1";

/// `StageTracker`: wraps a progress callback and remembers where a run got to.
pub struct StageTracker<'a> {
    pub callback: Option<&'a mut dyn Progress>,
    pub state: StageState,
}

/// The fields of a `StageTracker` an error record reads.
#[derive(Clone, Debug)]
pub struct StageState {
    pub started: Vec<String>,
    pub finished: Vec<String>,
    pub origin: Instant,
}

impl<'a> StageTracker<'a> {
    pub fn new(callback: Option<&'a mut dyn Progress>) -> Self {
        StageTracker {
            callback,
            state: StageState { started: Vec::new(), finished: Vec::new(), origin: Instant::now() },
        }
    }
}

impl StageState {
    pub fn open_stages(&self) -> Vec<String> {
        let mut remaining = self.started.clone();
        for phase in &self.finished {
            if let Some(index) = remaining.iter().position(|item| item == phase) {
                remaining.remove(index);
            }
        }
        remaining
    }
}

impl Progress for StageTracker<'_> {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        if event == "started" {
            self.state.started.push(phase.to_owned());
        } else if ["finished", "reused", "stopped", "blocked", "failed"].contains(&event) {
            self.state.finished.push(phase.to_owned());
        }
        if let Some(callback) = self.callback.as_mut() {
            callback.observe(phase, event, details);
        }
    }
}

/// `re.findall(r"component '([^']+)'", message)`.
fn quoted_components(message: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = message;
    while let Some(index) = rest.find("component '") {
        let after = &rest[index + "component '".len()..];
        match after.find('\'') {
            Some(end) if end > 0 => {
                found.insert(after[..end].to_owned());
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    found
}

/// `report_path.with_name(report_path.stem + ".error.json")`.
pub fn error_record_path(report_path: &Path) -> PathBuf {
    let name = report_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[..index],
        _ => &name[..],
    };
    report_path.with_file_name(format!("{stem}.error.json"))
}

/// `write_error_record` for an error without a diagnostic board: persist a
/// failed run's message and stage, so `copper issues` can show it.
pub fn write_error_record(
    report_path: &Path,
    board_name: &str,
    error_type: &str,
    message: &str,
    tracker: Option<&StageState>,
    args_summary: Object,
) -> std::io::Result<PathBuf> {
    let strings = |items: Vec<String>| Value::Array(items.into_iter().map(Value::Str).collect());
    let mut document = Object::new();
    document.insert("schema".to_owned(), Value::from(ERROR_SCHEMA));
    document.insert("status".to_owned(), Value::from("error"));
    document.insert("board".to_owned(), Value::from(board_name));
    document.insert("error_type".to_owned(), Value::from(error_type));
    document.insert("message".to_owned(), Value::Str(message.chars().take(4000).collect()));
    document.insert("components".to_owned(), strings(quoted_components(message).into_iter().collect()));
    let (open, completed, elapsed) = match tracker {
        Some(tracker) => {
            let mut completed: Vec<String> = Vec::new();
            for phase in &tracker.finished {
                if !completed.contains(phase) {
                    completed.push(phase.clone());
                }
            }
            (
                strings(tracker.open_stages()),
                strings(completed),
                Value::Float(crate::progress::round_seconds(tracker.origin.elapsed().as_secs_f64())),
            )
        }
        None => (Value::Null, Value::Null, Value::Null),
    };
    document.insert("open_stages".to_owned(), open);
    document.insert("completed_stages".to_owned(), completed);
    document.insert("elapsed_seconds".to_owned(), elapsed);
    document.insert("options".to_owned(), Value::Object(args_summary));
    document.insert("note".to_owned(), Value::from("Working record of a failed run; not routing or signoff evidence."));
    let path = error_record_path(report_path);
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let text = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n";
    copper_json::text::write_text(&path, &text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_and_paths() {
        let found = quoted_components("component 'U1' has no pad '3'; component 'C2' and component ''");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["C2", "U1"]);
        assert_eq!(error_record_path(Path::new("out/route.json")), Path::new("out/route.error.json"));
        assert_eq!(error_record_path(Path::new("route")), Path::new("route.error.json"));
    }

    #[test]
    fn tracker_open_stages() {
        let mut tracker = StageTracker::new(None);
        tracker.observe("a", "started", Vec::new());
        tracker.observe("b", "started", Vec::new());
        tracker.observe("a", "finished", Vec::new());
        assert_eq!(tracker.state.open_stages(), ["b"]);
    }
}

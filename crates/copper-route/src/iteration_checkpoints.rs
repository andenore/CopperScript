//! Iteration records of `route-board` (`pcbir.iteration_checkpoints`): the
//! stage tracker, the error record of a failed run and placement checkpoints.
//!
//! A placement checkpoint lets the next `route-board` run skip placement and
//! global routing (`--reuse-placement`). It is keyed on the physical input
//! board, the placement/global/feedback options and the build identity of the
//! binary that wrote it. Reuse is refused when the board or options differ; a
//! different build is allowed but recorded in the report, because the caller
//! is deliberately holding placement fixed while changing later stages.
//! The board and options digests keep Python's definitions; the build
//! identity is this binary's own (see `crates/copper/build.rs`), so a
//! checkpoint Python wrote always reads as made by a different build.
//!
//! Checkpoints and error records are working artifacts, never evidence.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use copper_json::{Dumps, Object, Value};
use copper_pcb::board::PhysicalBoard;
use copper_place::PlannerOptions;
use copper_state::{StateDocument, StateError, StateValue};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::progress::Progress;
use crate::routeflow::{FeedbackIteration, FeedbackStatus, PlacementRoutingFeedbackOptions, PlacementRoutingResult};
use crate::routing::GlobalRouterOptions;

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

/// `_options_digest(placement, global, feedback, None, None)`: SHA-256 of the
/// compact, key-sorted JSON of the encoded options tuple. Sets are already
/// in canonical order and these options hold no mappings.
pub fn options_digest(
    placement: &PlannerOptions,
    global: &GlobalRouterOptions,
    feedback: &PlacementRoutingFeedbackOptions,
) -> String {
    use crate::state_codec::{encode_feedback_options, encode_global_options, encode_planner_options};
    let options = StateValue::Tuple(vec![
        encode_planner_options(placement),
        encode_global_options(global),
        encode_feedback_options(feedback),
        StateValue::Null,
        StateValue::Null,
    ]);
    let body = Dumps::compact().sort_keys(true).dumps(&options.encode());
    let hex: String = Sha256::digest(body.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// `placement_key`: what a placement checkpoint applies to. `code` is the
/// build identity of the binary.
pub fn placement_key(
    board: &PhysicalBoard,
    placement: &PlannerOptions,
    global: &GlobalRouterOptions,
    feedback: &PlacementRoutingFeedbackOptions,
    code: &str,
) -> Object {
    Object::from([
        ("stage".to_owned(), Value::from("placement")),
        ("board".to_owned(), Value::from(copper_drc::digest::physical_board_digest(board))),
        ("options".to_owned(), Value::from(options_digest(placement, global, feedback))),
        ("code".to_owned(), Value::from(code)),
    ])
}

/// The file name `--checkpoint-dir` saves the placement under.
pub const PLACEMENT_CHECKPOINT: &str = "placement.state.json";

/// `save_placement_checkpoint`: the file text, with the key and the UTC
/// creation time (`%Y-%m-%dT%H:%M:%SZ`) as metadata.
pub fn placement_checkpoint_text(placement: &PlacementRoutingResult, key: &Object, created_utc: &str) -> String {
    let metadata = Object::from([
        ("key".to_owned(), Value::Object(key.clone())),
        ("created_utc".to_owned(), Value::from(created_utc)),
    ]);
    StateDocument::new(
        "pcbir.routeflow:PlacementRoutingResult",
        metadata,
        crate::state_codec::encode_placement_result(placement),
    )
    .to_text()
}

/// The current UTC time as `%Y-%m-%dT%H:%M:%SZ`.
pub fn created_utc_now() -> String {
    crate::ladder::utc_now_iso().replace("+00:00", "Z")
}

/// The provenance of a reused placement (the report's `placement_reuse`,
/// without `final_candidate`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementReuse {
    pub path: String,
    pub content_digest: Value,
    pub created_utc: Value,
    pub code_changed: bool,
    pub candidate: String,
}

impl PlacementReuse {
    pub fn document(&self) -> Object {
        Object::from([
            ("path".to_owned(), Value::from(self.path.as_str())),
            ("content_digest".to_owned(), self.content_digest.clone()),
            ("created_utc".to_owned(), self.created_utc.clone()),
            ("code_changed".to_owned(), Value::Bool(self.code_changed)),
            ("candidate".to_owned(), Value::from(self.candidate.as_str())),
        ])
    }
}

/// Why a checkpoint does not apply: Python's exception class and message.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub error_type: &'static str,
    pub message: String,
}

/// `load_placement_checkpoint`: the saved placement and its provenance, or
/// why it does not apply (`Ok(Err(..))`). `display` is the path as the
/// command line names it. A checkpoint this implementation cannot decode yet (one
/// with package patterns) is not implemented.
pub fn load_placement_checkpoint(
    path: &Path,
    display: &str,
    key: &Object,
) -> Result<std::result::Result<(PlacementRoutingResult, PlacementReuse), Refusal>> {
    let refuse = |error_type: &'static str, message: String| Ok(Err(Refusal { error_type, message }));
    let not_checkpoint = || refuse("ValueError", format!("{display} is not a placement checkpoint"));
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let text = error.to_string();
            let reason = text.rfind(" (os error ").map_or(text.as_str(), |index| &text[..index]);
            return refuse("ValueError", format!("cannot read placement checkpoint {display}: {reason}"));
        }
    };
    let Ok(text) = String::from_utf8(bytes) else { return not_checkpoint() };
    let document = match StateDocument::read(&text) {
        Ok(document) => document,
        Err(StateError::Json(error)) => return refuse("JSONDecodeError", error.python_message(&text)),
        Err(StateError::NotAStateFile) => {
            return refuse("StateError", format!("{display}: not a {} file", copper_state::STATE_SCHEMA));
        }
        Err(StateError::DigestMismatch) => return refuse("StateError", format!("{display}: content digest mismatch")),
        Err(StateError::ForeignType(name)) => return refuse("StateError", StateError::ForeignType(name).to_string()),
        Err(StateError::Malformed(_)) => return not_checkpoint(),
    };
    let saved = match document.metadata.get("key") {
        Some(Value::Object(saved)) => saved.clone(),
        _ => Object::new(),
    };
    let is_placement = matches!(&document.value, StateValue::Dataclass { name, .. }
        if name == "pcbir.routeflow:PlacementRoutingResult");
    if !is_placement || saved.get("stage").and_then(Value::as_str) != Some("placement") {
        return not_checkpoint();
    }
    if saved.get("board") != key.get("board") {
        return refuse(
            "ValueError",
            format!(
                "{display} was made for a different physical board (source, footprints, layers or \
                 fabrication profile changed); run without --reuse-placement"
            ),
        );
    }
    if saved.get("options") != key.get("options") {
        return refuse(
            "ValueError",
            format!(
                "{display} was made with different placement/global/feedback options; run without \
                 --reuse-placement or restore those options"
            ),
        );
    }
    let placement = placement_result(&document.value)?;
    let reuse = PlacementReuse {
        path: display.to_owned(),
        content_digest: copper_json::parse(&text)
            .ok()
            .and_then(|value| value.as_object().and_then(|object| object.get("content_digest").cloned()))
            .unwrap_or(Value::Null),
        created_utc: document.metadata.get("created_utc").cloned().unwrap_or(Value::Null),
        code_changed: saved.get("code") != key.get("code"),
        candidate: placement.placement_candidate.clone(),
    };
    Ok(Ok((placement, reuse)))
}

/// `routeflow.PlacementRoutingResult(**fields)` from its saved dataclass.
pub fn placement_result(value: &StateValue) -> Result<PlacementRoutingResult> {
    use copper_pcb::state_codec::{Fields, boolean, enumeration, integer, items, text};
    let fields = Fields::of(value, "pcbir.routeflow:PlacementRoutingResult")?;
    fields.allow(&[
        "status",
        "board",
        "global_route",
        "placement_candidate",
        "iterations",
        "accepted_moves",
        "full_route_certified",
        "package_patterns",
    ])?;
    let status = enumeration(fields.required("status")?, "FeedbackStatus", |member| match member {
        "pass" => Some(FeedbackStatus::Pass),
        "warning" => Some(FeedbackStatus::Warning),
        _ => None,
    })?;
    let board = copper_pcb::state_codec::board(fields.required("board")?)?;
    let global_route = crate::routing_state::global_routing_result(fields.required("global_route")?)?;
    let placement_candidate = text(fields.required("placement_candidate")?)?;
    let mut iterations = Vec::new();
    for iteration in items(fields.required("iterations")?)? {
        let iteration = Fields::of(iteration, "pcbir.routeflow:FeedbackIteration")?;
        iteration.allow(&[
            "iteration",
            "accepted",
            "movement_nm",
            "trials",
            "unrouted_net_count",
            "total_overflow",
            "maximum_overflow",
            "routing_fingerprint",
        ])?;
        let int = |name: &str| integer(iteration.required(name)?);
        iterations.push(FeedbackIteration {
            iteration: int("iteration")?,
            accepted: boolean(iteration.required("accepted")?)?,
            movement_nm: int("movement_nm")?,
            trials: int("trials")?,
            unrouted_net_count: int("unrouted_net_count")?,
            total_overflow: int("total_overflow")?,
            maximum_overflow: int("maximum_overflow")?,
            routing_fingerprint: text(iteration.required("routing_fingerprint")?)?,
        });
    }
    let accepted_moves = integer(fields.required("accepted_moves")?)?;
    let full_route_certified = boolean(fields.required("full_route_certified")?)?;
    if fields.get("package_patterns").is_some_and(|patterns| !items(patterns).is_ok_and(<[StateValue]>::is_empty)) {
        return copper_pcb::not_implemented("a placement checkpoint with package patterns");
    }
    Ok(PlacementRoutingResult {
        status,
        board,
        global_route,
        placement_candidate,
        iterations,
        accepted_moves,
        full_route_certified,
    })
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

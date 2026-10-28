//! `pcbir.editor.jobs`: one cancellable placement run per editor session.
//!
//! Python runs the planner in a spawned process and reads its phase reports
//! from a pipe. Here it runs on a thread: a cancelled job's result is
//! discarded when it arrives, and `copper-place` reports no intermediate
//! phases, so `progress` stays at its starting value.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use copper_json::Value;
use copper_pcb::board::PhysicalBoard;
use copper_place::options::PlannerOptions;

use crate::{Error, Result, object};

type Outcome = Arc<Mutex<Option<std::result::Result<PhysicalBoard, String>>>>;

/// `PlacementJob`.
pub struct PlacementJob {
    pub revision: i64,
    pub source_revision: String,
    started: Instant,
    finished: Option<Instant>,
    pub status: String,
    candidates: i64,
    pub result: Option<PhysicalBoard>,
    pub error: Option<String>,
    outcome: Outcome,
}

impl PlacementJob {
    pub fn new(board: &PhysicalBoard, options: PlannerOptions, revision: i64, source_revision: &str) -> Result<Self> {
        if board.placements.len() > 5000 {
            return Err(Error::Value("editor placement supports at most 5000 components".to_owned()));
        }
        let outcome: Outcome = Arc::new(Mutex::new(None));
        let (shared, board, candidates) = (outcome.clone(), board.clone(), options.candidate_count.max(2));
        std::thread::spawn(move || {
            let result = match copper_place::layout::plan_placement(&board, &options) {
                Ok(plan) => Ok(plan.board),
                Err(copper_pcb::Error::Value(message)) => Err(format!("ValueError: {message}")),
                Err(copper_pcb::Error::NotImplemented(what)) => Err(format!("NotImplemented: {what}")),
            };
            *shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
        });
        Ok(PlacementJob {
            revision,
            source_revision: source_revision.to_owned(),
            started: Instant::now(),
            finished: None,
            status: "running".to_owned(),
            candidates,
            result: None,
            error: None,
            outcome,
        })
    }

    /// `poll`: the job's status record.
    pub fn poll(&mut self) -> Result<Value> {
        if self.status == "running" {
            let taken = self.outcome.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            match taken {
                Some(Ok(board)) => {
                    self.result = Some(board);
                    self.status = "completed".to_owned();
                }
                Some(Err(message)) if message.starts_with("NotImplemented: ") => {
                    return Err(Error::NotImplemented(message["NotImplemented: ".len()..].to_owned()));
                }
                Some(Err(message)) => {
                    self.status = "failed".to_owned();
                    self.error = Some(message);
                }
                None => {}
            }
            if self.status != "running" {
                self.finished = Some(self.finished.unwrap_or_else(Instant::now));
            }
        }
        let elapsed = self.finished.unwrap_or_else(Instant::now).duration_since(self.started).as_secs_f64();
        Ok(object(vec![
            ("status", Value::from(self.status.as_str())),
            (
                "progress",
                object(vec![
                    ("phase", Value::from("starting")),
                    ("candidate", Value::from(0i64)),
                    ("candidates", Value::from(self.candidates)),
                ]),
            ),
            ("error", self.error.as_deref().map_or(Value::Null, Value::from)),
            ("elapsed_seconds", Value::from((elapsed * 1000.0).round() / 1000.0)),
            ("profile", Value::Null),
            ("input_revision", Value::from(self.revision)),
        ]))
    }

    /// `cancel`: a running job stops counting and its result is discarded.
    pub fn cancel(&mut self, status: &str, error: Option<&str>) {
        if self.status != "running" {
            return;
        }
        self.status = status.to_owned();
        self.error = error.map(str::to_owned);
        self.finished = Some(Instant::now());
    }
}

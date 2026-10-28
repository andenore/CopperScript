//! Optional operational telemetry (`pcbir.progress`); never routing evidence.
//!
//! PENDING: owned by `rust-m4-flow`; the console callback and `emit` only.

use std::io::Write;
use std::time::Instant;

use copper_json::{Dumps, Object, Value};

/// A progress observer: `(phase, event, details)`.
pub trait Progress {
    fn observe(&mut self, phase: &str, event: &str, details: Object);
}

/// `emit(callback, phase, event, **details)`.
pub fn emit(callback: &mut Option<&mut dyn Progress>, phase: &str, event: &str, details: Object) {
    if let Some(callback) = callback {
        callback.observe(phase, event, details);
    }
}

/// `console_progress()`: one `PROGRESS` JSON line per event, with the
/// wall-clock seconds since the callback was made.
pub struct ConsoleProgress<'a> {
    started: Instant,
    out: &'a mut dyn Write,
}

impl<'a> ConsoleProgress<'a> {
    pub fn new(out: &'a mut dyn Write) -> Self {
        ConsoleProgress { started: Instant::now(), out }
    }
}

impl Progress for ConsoleProgress<'_> {
    fn observe(&mut self, phase: &str, event: &str, details: Object) {
        let elapsed = round(self.started.elapsed().as_secs_f64(), 3);
        let document = Value::Object(Object::from([
            ("phase".to_owned(), Value::from(phase)),
            ("event".to_owned(), Value::from(event)),
            ("elapsed_seconds".to_owned(), Value::Float(elapsed)),
            ("details".to_owned(), Value::Object(details)),
        ]));
        let line = format!("PROGRESS {}", Dumps::new().sort_keys(true).dumps(&document));
        let _ = copper_json::text::print(self.out, &line);
        let _ = self.out.flush();
    }
}

/// Python's `round(value, digits)` for the digit counts progress uses.
pub fn round(value: f64, digits: i32) -> f64 {
    copper_physical::pyfloat::round_digits(value, digits)
}

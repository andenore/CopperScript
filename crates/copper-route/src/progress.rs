//! Optional operational telemetry (`pcbir.progress`); never part of routing or
//! signoff evidence.
//!
//! A progress callback receives `(phase, event, details)`. The console
//! callback prints one `PROGRESS {json}` line per event with the seconds since
//! it was created, which is why the comparison harness never compares these
//! lines byte for byte.

use std::io::Write;
use std::time::Instant;

use copper_json::{Dumps, Object, Value};

/// A progress observer: `(phase, event, details)`, details in keyword order.
pub trait Progress {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>);
}

pub type ProgressCallback<'a> = &'a mut dyn Progress;

/// `emit`: forward one event when an observer is installed.
pub fn emit(
    callback: &mut Option<ProgressCallback<'_>>,
    phase: &str,
    event: &str,
    details: Vec<(&'static str, Value)>,
) {
    if let Some(callback) = callback {
        callback.observe(phase, event, details);
    }
}

pub fn int(value: i64) -> Value {
    Value::Int(num_bigint::BigInt::from(value))
}

pub fn text(value: &str) -> Value {
    Value::Str(value.to_owned())
}

/// Python's `round(x, 3)` for the elapsed seconds: correctly rounded, as `%.3f` is.
pub fn round_seconds(seconds: f64) -> f64 {
    format!("{seconds:.3}").parse().unwrap_or(seconds)
}

/// `console_progress()`: `PROGRESS {...}` lines, flushed as they happen.
pub struct ConsoleProgress<W: Write> {
    out: W,
    started: Instant,
}

impl<W: Write> ConsoleProgress<W> {
    pub fn new(out: W) -> Self {
        ConsoleProgress { out, started: Instant::now() }
    }

    /// The JSON line of one event at `elapsed` seconds (sorted keys, Python's defaults).
    pub fn line(phase: &str, event: &str, elapsed: f64, details: Vec<(&'static str, Value)>) -> String {
        let mut fields = Object::new();
        for (key, value) in details {
            fields.insert(key.to_owned(), value);
        }
        let mut document = Object::new();
        document.insert("phase".to_owned(), Value::from(phase));
        document.insert("event".to_owned(), Value::from(event));
        document.insert("elapsed_seconds".to_owned(), Value::Float(round_seconds(elapsed)));
        document.insert("details".to_owned(), Value::Object(fields));
        format!("PROGRESS {}", Dumps::new().sort_keys(true).dumps(&Value::Object(document)))
    }
}

impl<W: Write> Progress for ConsoleProgress<W> {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        let elapsed = self.started.elapsed().as_secs_f64();
        let _ = copper_json::text::print(&mut self.out, &Self::line(phase, event, elapsed, details));
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_lines_sort_keys_and_round_seconds() {
        let details = vec![("nets", int(3)), ("status", text("finished"))];
        let line = ConsoleProgress::<Vec<u8>>::line("ordinary_area", "finished", 1.23456, details);
        assert_eq!(
            line,
            "PROGRESS {\"details\": {\"nets\": 3, \"status\": \"finished\"}, \"elapsed_seconds\": 1.235, \
             \"event\": \"finished\", \"phase\": \"ordinary_area\"}"
        );
        assert_eq!(round_seconds(0.0005), 0.001);
        assert_eq!(round_seconds(2.0), 2.0);
        assert_eq!(
            ConsoleProgress::<Vec<u8>>::line("a", "b", 2.0, Vec::new()),
            "PROGRESS {\"details\": {}, \"elapsed_seconds\": 2.0, \"event\": \"b\", \"phase\": \"a\"}"
        );
    }
}

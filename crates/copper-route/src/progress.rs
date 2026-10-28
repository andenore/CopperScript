//! Optional operational telemetry (`pcbir.progress`); never routing or signoff
//! evidence. `console_progress` (timed `PROGRESS` lines) belongs to the
//! `route-board` stream; the routing foundation only emits events.

use copper_json::Value;

/// A progress observer: `(phase, event, details)`.
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

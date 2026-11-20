//! Opt-in wall-clock accounting of where an analysis spends its time.
//!
//! Off by default: [`span`] then costs one relaxed atomic load. With
//! [`enable`] (`copper sim --timing`, or `COPPER_SIM_TIMING=1`), every span
//! adds its elapsed nanoseconds to a process-wide counter per [`Phase`];
//! [`take`] reads and clears them. Spans on several threads add up, so with
//! parallel runs the totals are thread time, not wall time.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// What a span measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Building the real system of one Newton iteration (stamping), devices excluded.
    Assembly,
    /// Evaluating element currents and linearisations (diodes, transistors, laws, op-amp limits).
    Devices,
    /// LU factorisation, with the check whether the previous factors still serve.
    Factor,
    /// Forward and back substitution with iterative refinement.
    Solve,
    /// Truncation error, step size, breakpoints and switch events of a transient run.
    StepControl,
    /// Recording accepted time points.
    Storage,
    /// Evaluating a bench's lets, measures and plots over the stored results.
    Evaluation,
}

impl Phase {
    /// Every phase, in report order.
    pub const ALL: [Phase; 7] = [
        Phase::Assembly,
        Phase::Devices,
        Phase::Factor,
        Phase::Solve,
        Phase::StepControl,
        Phase::Storage,
        Phase::Evaluation,
    ];

    /// The phase's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Assembly => "assembly",
            Phase::Devices => "devices",
            Phase::Factor => "factor",
            Phase::Solve => "solve",
            Phase::StepControl => "step_control",
            Phase::Storage => "storage",
            Phase::Evaluation => "evaluation",
        }
    }
}

/// Events counted alongside the phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// Sparse factorisations that chose a pivot order.
    Ordered,
    /// Sparse refactorisations in the previous pivot order.
    Refactored,
    /// Matrices left to the dense factorisation.
    Dense,
}

impl Counter {
    /// Every counter, in report order.
    pub const ALL: [Counter; 3] = [Counter::Ordered, Counter::Refactored, Counter::Dense];

    /// The counter's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Counter::Ordered => "ordered",
            Counter::Refactored => "refactored",
            Counter::Dense => "dense",
        }
    }
}

static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Count one `counter` event (a no-op unless [`enabled`]).
pub fn count(counter: Counter) {
    if enabled() {
        COUNTS[counter as usize].fetch_add(1, Ordering::Relaxed);
    }
}

/// The events counted since the last call, in [`Counter::ALL`] order; clears them.
pub fn take_counts() -> [u64; 3] {
    let mut out = [0; 3];
    for (value, counter) in out.iter_mut().zip(&COUNTS) {
        *value = counter.swap(0, Ordering::Relaxed);
    }
    out
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static NANOS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// Turn the accounting on or off for the whole process.
pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether the accounting is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A running span; its time is added to its phase when dropped.
#[must_use = "a span measures until it is dropped"]
pub struct Span(Option<(Phase, Instant)>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((phase, start)) = self.0.take() {
            let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            NANOS[phase as usize].fetch_add(nanos, Ordering::Relaxed);
        }
    }
}

/// Start measuring `phase` (a no-op unless [`enabled`]).
pub fn span(phase: Phase) -> Span {
    Span(enabled().then(|| (phase, Instant::now())))
}

/// The seconds accumulated per phase since the last call, in [`Phase::ALL`] order; clears them.
pub fn take() -> [f64; 7] {
    let mut out = [0.0; 7];
    for (value, counter) in out.iter_mut().zip(&NANOS) {
        *value = counter.swap(0, Ordering::Relaxed) as f64 * 1e-9;
    }
    out
}

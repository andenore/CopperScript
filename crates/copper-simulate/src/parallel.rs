//! Independent runs on threads (`copper sim --jobs N`): the steps of a
//! bench, its Monte Carlo runs and its worst-case corners.
//!
//! [`map`] runs a function over a list on up to [`jobs`] scoped threads and
//! returns the results in list order. Every run is computed exactly as the
//! serial loop would (each owns its circuit and solver; nothing numeric is
//! shared), so the results are identical to a serial run's for any number of
//! jobs; only the order the runs finish in varies, and it is not observed. A
//! `map` inside a `map`'s function runs serially, so nesting never
//! oversubscribes the cores.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The number of threads; 0 is the available cores.
static JOBS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    static OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Use up to `jobs` threads (0: one per available core; 1: serial).
pub fn set_jobs(jobs: usize) {
    JOBS.store(jobs, Ordering::Relaxed);
}

/// Run `f` with [`map`]s called from this thread using `jobs` threads
/// (as [`set_jobs`], for this thread only).
pub fn with_jobs<R>(jobs: usize, f: impl FnOnce() -> R) -> R {
    let previous = OVERRIDE.with(|cell| cell.replace(Some(jobs)));
    let result = f();
    OVERRIDE.with(|cell| cell.set(previous));
    result
}

/// The number of threads [`map`] uses.
pub fn jobs() -> usize {
    match OVERRIDE.with(Cell::get).unwrap_or_else(|| JOBS.load(Ordering::Relaxed)) {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    }
}

/// `f` of every item, in item order, on up to [`jobs`] threads.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let jobs = jobs().min(items.len());
    if jobs <= 1 || INSIDE.with(Cell::get) {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(|| {
                    INSIDE.with(|inside| inside.set(true));
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(k) else { break };
                        done.push((k, f(item)));
                    }
                    done
                })
            })
            .collect();
        for worker in workers {
            match worker.join() {
                Ok(done) => {
                    for (k, result) in done {
                        results[k] = Some(result);
                    }
                }
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
    });
    results.into_iter().map(|result| result.expect("every item ran")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_item_order_and_nested_maps_run_serially() {
        let items: Vec<u64> = (0..200).collect();
        let squares = map(&items, |k| {
            let inner = map(&[1u64, 2, 3], |j| j * k);
            assert_eq!(inner, vec![*k, 2 * k, 3 * k]);
            k * k
        });
        assert_eq!(squares, items.iter().map(|k| k * k).collect::<Vec<_>>());
        assert!(map(&[] as &[u64], |k| *k).is_empty());
    }
}

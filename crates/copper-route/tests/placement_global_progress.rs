//! Progress callbacks observe placement and global routing without changing
//! them (ported from the former `tests/test_placement_global_progress.py`).

mod support_routers;

use copper_json::Value;
use copper_route::progress::placement_progress;
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use copper_route::{GlobalRouterOptions, Progress, route_global};
use support_routers::*;

/// `tests/test_progress.py::board`: two one-pad parts joined by SIGNAL.
fn progress_board() -> copper_pcb::board::PhysicalBoard {
    let one = footprint("one", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 1.0, 1.0);
    board(
        "progress",
        12.0,
        12.0,
        vec![one],
        vec![placement("J1", "one", 3.0, 6.0), placement("J2", "one", 9.0, 6.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    )
}

#[test]
fn adapter_preserves_raw_seed_indices_and_final_total_semantics() {
    let mut events = Events::default();
    {
        let mut callback: Option<&mut dyn Progress> = Some(&mut events);
        let mut observe = placement_progress(&mut callback);
        for (stage, seed, count) in [
            ("analytical", 0, 2),
            ("legalization", 0, 2),
            ("legalization_failed", 0, 2),
            ("analytical", 1, 2),
            ("refinement", 1, 2),
            ("finished", 2, 2),
        ] {
            observe(stage, seed, count);
        }
    }
    let shape = |(p, e, d): &Event| {
        (p.clone(), e.clone(), d.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<Vec<_>>())
    };
    let stage = |name: &str, seed: i64| {
        vec![
            ("stage".to_owned(), Value::from(name)),
            ("seed".to_owned(), Value::from(seed)),
            ("seed_count".to_owned(), Value::from(2i64)),
        ]
    };
    assert_eq!(shape(&events.events[0]), ("placement_seed".to_owned(), "stage".to_owned(), stage("analytical", 0)));
    assert_eq!(
        shape(&events.events[2]),
        ("placement_seed".to_owned(), "failed".to_owned(), stage("legalization_failed", 0))
    );
    assert_eq!(
        shape(events.events.last().expect("events")),
        ("placement_candidates".to_owned(), "finished".to_owned(), vec![("seed_count".to_owned(), Value::from(2i64))])
    );
    assert!(!events.events.iter().any(|(_, _, d)| d.iter().any(|(k, v)| *k == "seed" && *v == Value::from(2i64))));
}

#[test]
fn global_real_result_is_identical_with_callback() {
    // Python also recorded the internal call order by patching the router.
    let source = progress_board();
    let options = GlobalRouterOptions { maximum_iterations: 3, ..GlobalRouterOptions::default() };
    let plain = route_global(&source, &options, None, None, None).expect("route");
    let mut events = Events::default();
    let observed = route_global(&source, &options, Some(&mut events), None, None).expect("route");
    assert_eq!(observed, plain);
    assert_eq!(observed.to_json(), plain.to_json());
    let phases: Vec<(&str, &str)> = events.events.iter().map(|(p, e, _)| (p.as_str(), e.as_str())).collect();
    assert_eq!(
        phases[..5],
        [
            ("global_route", "started"),
            ("global_graph", "started"),
            ("global_graph", "finished"),
            ("global_pin_access", "started"),
            ("global_pin_access", "finished")
        ]
    );
    assert_eq!(*phases.last().expect("events"), ("global_route", "finished"));
    let starts = events.of("global_iteration", "started");
    let finishes = events.of("global_iteration", "finished");
    assert!(starts.len() == finishes.len() && starts.len() as i64 == observed.metrics.iterations && starts.len() == 1);
    assert_eq!(int_of(detail(starts[0], "iteration")), 1);
    assert_eq!(int_of(detail(starts[0], "maximum_iterations")), 3);
    let last = &events.events.last().expect("events").2;
    assert_eq!(int_of(detail(last, "total_overflow")), observed.metrics.total_overflow);
    assert!(!events.events.iter().any(|(_, _, d)| d.iter().any(|(k, _)| *k == "fabrication_ready")));
}

#[test]
fn invalid_global_feedback_budget() {
    // Python also refused True and 1.5; the Rust field is a typed integer.
    let options = PlacementRoutingFeedbackOptions {
        maximum_trials_per_iteration: -1,
        ..PlacementRoutingFeedbackOptions::default()
    };
    let error = options.validated().unwrap_err();
    assert!(error.to_string().contains("trial budget"), "{error}");
}

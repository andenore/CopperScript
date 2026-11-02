//! Shared board builders for the general-router behaviour tests ported from
//! the former Python suite (`tests/test_unified_router.py` and friends).
//!
//! `boards.rs` depends only on the board crates (so `copper-place` tests can
//! include it); this module adds progress recording and routing helpers.
//! `src/test_support_routers.rs` includes it for crate-private unit tests.
#![allow(dead_code)]

mod boards;

pub use boards::*;

use copper_json::Value;

/// One recorded progress event: `(phase, event, details)`.
pub type Event = (String, String, Vec<(&'static str, Value)>);

/// A progress observer that records every event.
#[derive(Default)]
pub struct Events {
    pub events: Vec<Event>,
}

impl copper_route::Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        self.events.push((phase.to_owned(), event.to_owned(), details));
    }
}

impl Events {
    /// The details of every `(phase, event)` event.
    pub fn of(&self, phase: &str, event: &str) -> Vec<&Vec<(&'static str, Value)>> {
        self.events.iter().filter(|(p, e, _)| p == phase && e == event).map(|(_, _, d)| d).collect()
    }

    pub fn phase(&self, phase: &str) -> Vec<&Vec<(&'static str, Value)>> {
        self.events.iter().filter(|(p, _, _)| p == phase).map(|(_, _, d)| d).collect()
    }

    pub fn count(&self, phase: &str, event: &str) -> usize {
        self.of(phase, event).len()
    }
}

/// One detail of a progress event.
pub fn detail<'a>(details: &'a [(&'static str, Value)], key: &str) -> &'a Value {
    &details.iter().find(|(k, _)| *k == key).unwrap_or_else(|| panic!("detail {key}: {details:?}")).1
}

pub fn int_of(value: &Value) -> i64 {
    match value {
        Value::Int(number) => i64::try_from(number.clone()).expect("i64"),
        Value::Bool(flag) => i64::from(*flag),
        other => panic!("not an int: {other:?}"),
    }
}

/// `summary[a][b]` of a `connection_summary` as an integer.
pub fn summary_int(summary: &[(String, Value)], section: &str, key: &str) -> i64 {
    let value = &summary.iter().find(|(k, _)| k == section).unwrap_or_else(|| panic!("section {section}")).1;
    int_of(value.get(key).unwrap_or_else(|| panic!("{section}.{key}")))
}

/// `summary[a][b]` of a `connection_summary`.
pub fn summary_value<'a>(summary: &'a [(String, Value)], section: &str, key: &str) -> &'a Value {
    let value = &summary.iter().find(|(k, _)| k == section).unwrap_or_else(|| panic!("section {section}")).1;
    value.get(key).unwrap_or_else(|| panic!("{section}.{key}"))
}

/// Routing helpers over the public API.
pub mod routes {
    use copper_drc::{Policy, Report, Severity};
    use copper_json::Value;
    use copper_pcb::board::PhysicalBoard;
    use copper_route::detailed::DetailedRoutingResult;
    use copper_route::route_summary::connection_summary;
    use copper_route::unified_router::UnifiedRouterOptions;
    use copper_route::unified_router::route::route_unified;
    use copper_route::{GlobalRouterOptions, GlobalRoutingResult, route_global};

    use super::{Events, mm};

    /// `route_global` with `tile_size_nm` mm tiles and Python's other defaults.
    pub fn global(board: &PhysicalBoard, tile_mm: f64) -> GlobalRoutingResult {
        let options = GlobalRouterOptions { tile_size_nm: mm(tile_mm), ..GlobalRouterOptions::default() };
        route_global(board, &options, None, None, None).expect("global route")
    }

    /// Global guides planned without existing copper, as in the pipeline.
    pub fn guides(board: &PhysicalBoard) -> GlobalRoutingResult {
        let mut bare = board.clone();
        bare.tracks.clear();
        bare.vias.clear();
        global(&bare, 2.0)
    }

    pub fn drc(board: &PhysicalBoard) -> Report {
        copper_drc::run_physical_drc(board, Policy::default()).expect("drc")
    }

    /// Error findings, except the opens and the incomplete-route flag.
    pub fn errors_except_opens(report: &Report) -> Vec<String> {
        report
            .findings
            .iter()
            .filter(|f| {
                f.severity == Severity::Error && !["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"].contains(&f.code.as_str())
            })
            .map(|f| format!("{} {}", f.code, f.message))
            .collect()
    }

    pub fn errors(report: &Report) -> Vec<String> {
        report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .map(|f| format!("{} {}", f.code, f.message))
            .collect()
    }

    pub struct Routed {
        pub result: DetailedRoutingResult,
        pub drc: Report,
        pub summary: Vec<(String, Value)>,
    }

    impl Routed {
        pub fn opens(&self) -> i64 {
            super::summary_int(&self.summary, "signal", "opens")
        }
        pub fn errors(&self) -> i64 {
            super::summary_int(&self.summary, "drc", "errors")
        }
        pub fn net(&self, name: &str) -> &copper_route::detailed::DetailedNetResult {
            self.result.nets.iter().find(|n| n.net == name).unwrap_or_else(|| panic!("net {name}"))
        }
    }

    /// `tests/test_unified_router.py::_route`: guides, then the unified router, DRC and summary.
    pub fn route(board: &PhysicalBoard, options: &UnifiedRouterOptions) -> Routed {
        let global_route = guides(board);
        let result = route_unified(board, &global_route, options, None, None, None, None, None).expect("unified route");
        let drc = drc(&result.board);
        let summary = connection_summary(&result.board, Some(&drc)).expect("summary");
        Routed { result, drc, summary }
    }

    pub fn route_with(
        board: &PhysicalBoard,
        global_route: &GlobalRoutingResult,
        options: &UnifiedRouterOptions,
        events: &mut Events,
    ) -> DetailedRoutingResult {
        route_unified(board, global_route, options, None, None, None, None, Some(events)).expect("unified route")
    }
}

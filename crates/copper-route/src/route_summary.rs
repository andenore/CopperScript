//! Pipeline-independent completion metrics for a routed board
//! (`pcbir.route_summary`).
//!
//! The benchmark ladder compares runs of different pipelines with these
//! numbers, so they depend only on the final copper, never on stage-internal
//! bookkeeping. A plane (zone) net is counted separately. Without native fill
//! evidence its explicit-copper opens are an upper bound, because a refill can
//! join them.

use indexmap::{IndexMap, IndexSet};

use copper_drc::Severity;
use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::{Object, Value};
use copper_pcb::board::PhysicalBoard;
use copper_physical::Point;
use copper_physical::pyfloat::round_digits;

use crate::Result;
use crate::drc_report::{DrcReport, decision_value};

/// `drc.ROUTING_BUDGET_WARNING_CODES`.
pub const ROUTING_BUDGET_WARNING_CODES: [&str; 3] = ["DRC-MAX-LENGTH", "DRC-DIFF-SKEW", "DRC-LENGTH-MATCH"];

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn counts(codes: &IndexMap<String, i64>) -> Value {
    let mut sorted: Vec<(&String, &i64)> = codes.iter().collect();
    sorted.sort();
    Value::Object(sorted.into_iter().map(|(code, count)| (code.clone(), Value::from(*count))).collect())
}

/// `connection_summary(board, drc)`: connection, open, copper and DRC counts.
///
/// Plane estimates (`estimate_planes` with declared zones) and native fill
/// evidence are not ported yet and report not implemented.
pub fn connection_summary(board: &PhysicalBoard, drc: Option<&DrcReport>) -> Result<Value> {
    let graph = explicit_copper_connectivity(board, Candidate::default())?;
    let plane_nets: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    let (mut signal_nets, mut signal_connections, mut signal_opens) = (0i64, 0i64, 0i64);
    let mut open_nets: Vec<Value> = Vec::new();
    let (mut plane_count, mut plane_connections, mut plane_opens) = (0i64, 0i64, 0i64);
    let mut nets: Vec<_> = board.nets.iter().collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    for net in nets {
        if net.pads.len() < 2 {
            continue;
        }
        let mut roots: IndexSet<&String> = IndexSet::new();
        let mut isolated = 0i64;
        for pad in &net.pads {
            match graph.pad_nodes.get(pad) {
                Some(nodes) if !nodes.is_empty() => roots.extend(nodes.iter().map(|node| &graph.roots[node])),
                _ => isolated += 1,
            }
        }
        let opens = (roots.len() as i64 + isolated - 1).max(0);
        let connections = net.pads.len() as i64 - 1;
        if plane_nets.contains(net.name.as_str()) {
            plane_count += 1;
            plane_connections += connections;
            plane_opens += opens;
            continue;
        }
        signal_nets += 1;
        signal_connections += connections;
        signal_opens += opens;
        if opens != 0 {
            open_nets.push(Value::from(net.name.as_str()));
        }
    }
    let length: i64 = board.tracks.iter().map(|track| length_nm(track.start, track.end)).sum();
    let completion = if signal_connections != 0 {
        round_digits(1.0 - signal_opens as f64 / signal_connections as f64, 4)
    } else {
        1.0
    };
    let signal = object(vec![
        ("nets", Value::from(signal_nets)),
        ("connections", Value::from(signal_connections)),
        ("opens", Value::from(signal_opens)),
        ("open_nets", Value::Array(open_nets)),
        ("completion", Value::Float(completion)),
    ]);
    let plane = object(vec![
        ("nets", Value::from(plane_count)),
        ("connections", Value::from(plane_connections)),
        ("explicit_opens", Value::from(plane_opens)),
    ]);
    let copper = object(vec![
        ("tracks", Value::from(board.tracks.len() as i64)),
        ("vias", Value::from(board.vias.len() as i64)),
        ("track_length_mm", Value::Float(round_digits(length as f64 / 1_000_000.0, 3))),
    ]);
    let mut result: Object = Object::new();
    result.insert("schema".to_owned(), Value::from("copperscript-route-summary/v1"));
    result.insert("signal".to_owned(), signal);
    result.insert("plane".to_owned(), plane);
    result.insert("copper".to_owned(), copper);
    if !board.zones.is_empty() {
        let estimate = crate::plane_estimate::estimate_plane_connectivity(board)?;
        let Value::Object(report) = estimate.report() else { unreachable!("an object") };
        let Some(Value::Object(plane)) = result.get_mut("plane") else { unreachable!("an object") };
        plane.insert("estimated_opens".to_owned(), Value::from(estimate.opens));
        plane.insert("estimated_open_nets".to_owned(), report["open_nets"].clone());
        plane.insert("estimated_open_pads".to_owned(), report["open_pads"].clone());
    }
    if let Some(drc) = drc {
        // Every finding is active: the routing commands never give waivers.
        let mut errors: IndexMap<String, i64> = IndexMap::new();
        let mut budget: IndexMap<String, i64> = IndexMap::new();
        for finding in drc.findings() {
            if finding.severity == Severity::Error {
                *errors.entry(finding.code.clone()).or_default() += 1;
            }
            if ROUTING_BUDGET_WARNING_CODES.contains(&finding.code.as_str()) && finding.severity == Severity::Warning {
                *budget.entry(finding.code.clone()).or_default() += 1;
            }
        }
        // Opens are counted above, per connection; so is DRC-ROUTE-INCOMPLETE.
        errors.shift_remove("DRC-OPEN-NET");
        errors.shift_remove("DRC-ROUTE-INCOMPLETE");
        let total: i64 = errors.values().sum();
        result.insert(
            "drc".to_owned(),
            object(vec![
                ("decision", Value::from(decision_value(drc.decision()))),
                ("errors_by_code", counts(&errors)),
                ("errors", Value::from(total)),
                ("budget_warnings_by_code", counts(&budget)),
            ]),
        );
    }
    Ok(Value::Object(result))
}

/// `_length`: Python's `int(round((dx*dx + dy*dy) ** 0.5))`.
fn length_nm(start: Point, end: Point) -> i64 {
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    copper_physical::pyfloat::round_int(((dx * dx + dy * dy) as f64).powf(0.5))
}

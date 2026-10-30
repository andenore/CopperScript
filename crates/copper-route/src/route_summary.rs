//! Pipeline-independent completion metrics for a routed board
//! (`pcbir.route_summary.connection_summary`). The raster plane estimate of
//! boards with zones is not implemented yet (reported not implemented).

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_drc::{Report, Severity};
use copper_json::{Object, Value};
use copper_pcb::board::PhysicalBoard;
use copper_physical::pyfloat::{round_digits, round_int};

use crate::unified_error::Result;

const ROUTING_BUDGET_WARNING_CODES: [&str; 3] = ["DRC-MAX-LENGTH", "DRC-DIFF-SKEW", "DRC-LENGTH-MATCH"];

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Object>())
}

fn counts(codes: impl Iterator<Item = String>) -> IndexMap<String, i64> {
    let mut result: IndexMap<String, i64> = IndexMap::new();
    for code in codes {
        *result.entry(code).or_insert(0) += 1;
    }
    result.sort_keys();
    result
}

/// `connection_summary(board, drc)` as `(key, value)` entries of the summary dict.
pub fn connection_summary(board: &PhysicalBoard, drc: Option<&Report>) -> Result<Vec<(String, Value)>> {
    let graph = explicit_copper_connectivity(board, Candidate::default())?;
    let plane_nets: IndexSet<&str> = board.zones.iter().map(|z| z.net.as_str()).collect();
    let (mut s_nets, mut s_connections, mut s_opens) = (0i64, 0i64, 0i64);
    let mut open_nets: Vec<Value> = Vec::new();
    let (mut p_nets, mut p_connections, mut p_opens) = (0i64, 0i64, 0i64);
    let mut nets: Vec<_> = board.nets.iter().collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    for net in nets {
        if net.pads.len() < 2 {
            continue;
        }
        let mut roots: IndexSet<&String> = IndexSet::new();
        let mut isolated = 0;
        for pad in &net.pads {
            let nodes = graph.pad_nodes.get(pad).map_or(&[][..], Vec::as_slice);
            if nodes.is_empty() {
                isolated += 1;
            }
            roots.extend(nodes.iter().map(|n| &graph.roots[n]));
        }
        let opens = 0.max(roots.len() as i64 + isolated - 1);
        if plane_nets.contains(net.name.as_str()) {
            p_nets += 1;
            p_connections += net.pads.len() as i64 - 1;
            p_opens += opens;
            continue;
        }
        s_nets += 1;
        s_connections += net.pads.len() as i64 - 1;
        s_opens += opens;
        if opens != 0 {
            open_nets.push(Value::from(net.name.as_str()));
        }
    }
    let length: i64 = board
        .tracks
        .iter()
        .map(|t| {
            let (dx, dy) = ((t.end.x_nm - t.start.x_nm) as i128, (t.end.y_nm - t.start.y_nm) as i128);
            round_int(((dx * dx + dy * dy) as f64).powf(0.5))
        })
        .sum();
    let completion = if s_connections != 0 {
        round_digits(1.0 - crate::routing_lattice::true_div(s_opens as i128, s_connections as i128), 4)
    } else {
        1.0
    };
    let mut result = vec![
        ("schema".to_owned(), Value::from("copperscript-route-summary/v1")),
        (
            "signal".to_owned(),
            object(vec![
                ("nets", int(s_nets)),
                ("connections", int(s_connections)),
                ("opens", int(s_opens)),
                ("open_nets", Value::Array(open_nets)),
                ("completion", Value::Float(completion)),
            ]),
        ),
        ("plane".to_owned(), {
            let mut plane =
                vec![("nets", int(p_nets)), ("connections", int(p_connections)), ("explicit_opens", int(p_opens))];
            if !board.zones.is_empty() {
                let estimate = crate::plane_estimate::estimate_plane_connectivity(board)?;
                let mut open_nets: Vec<&String> =
                    estimate.net_groups.iter().filter(|(_, groups)| **groups > 1).map(|(net, _)| net).collect();
                open_nets.sort();
                let mut open_pads: Vec<(&String, &Vec<String>)> = estimate.open_pads.iter().collect();
                open_pads.sort_by(|a, b| a.0.cmp(b.0));
                plane.push(("estimated_opens", int(estimate.opens)));
                plane.push((
                    "estimated_open_nets",
                    Value::Array(open_nets.into_iter().map(|n| Value::from(n.as_str())).collect()),
                ));
                plane.push((
                    "estimated_open_pads",
                    Value::Object(
                        open_pads
                            .into_iter()
                            .map(|(net, pads)| {
                                (net.clone(), Value::Array(pads.iter().map(|p| Value::from(p.as_str())).collect()))
                            })
                            .collect(),
                    ),
                ));
            }
            object(plane)
        }),
        (
            "copper".to_owned(),
            object(vec![
                ("tracks", int(board.tracks.len() as i64)),
                ("vias", int(board.vias.len() as i64)),
                (
                    "track_length_mm",
                    Value::Float(round_digits(crate::routing_lattice::true_div(length as i128, 1_000_000), 3)),
                ),
            ]),
        ),
    ];
    if let Some(drc) = drc {
        let mut codes = counts(drc.findings.iter().filter(|f| f.severity == Severity::Error).map(|f| f.code.clone()));
        codes.shift_remove("DRC-OPEN-NET");
        codes.shift_remove("DRC-ROUTE-INCOMPLETE");
        let budget = counts(
            drc.findings
                .iter()
                .filter(|f| ROUTING_BUDGET_WARNING_CODES.contains(&f.code.as_str()) && f.severity == Severity::Warning)
                .map(|f| f.code.clone()),
        );
        let to_object = |map: &IndexMap<String, i64>| {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), int(*v))).collect::<Object>())
        };
        result.push((
            "drc".to_owned(),
            object(vec![
                ("decision", Value::from(if drc.decision == copper_drc::Decision::Pass { "pass" } else { "fail" })),
                ("errors_by_code", to_object(&codes)),
                ("errors", int(codes.values().sum())),
                ("budget_warnings_by_code", to_object(&budget)),
            ]),
        ));
    }
    Ok(result)
}

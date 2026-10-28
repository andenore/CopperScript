//! Detailed routing result types (`pcbir.detailed`) and the route fingerprint.

use copper_json::{Dumps, Value};
use copper_pcb::board::PhysicalBoard;
use sha2::{Digest, Sha256};

text_enum!(DetailedRoutingStatus { Success = "success", Partial = "partial" });

/// `DetailedNetResult` (the search-policy and search-stats telemetry of the
/// retired A* router is always absent on the unified path).
#[derive(Clone, Debug, PartialEq)]
pub struct DetailedNetResult {
    pub net: String,
    pub connected: bool,
    pub track_count: i64,
    pub via_count: i64,
    pub length_nm: i64,
    pub guide_deviation_count: i64,
    pub diagnostics: Vec<String>,
    pub orthogonal_mode_used: bool,
    pub resumed_branch_count: i64,
    pub search_budget_exhausted: bool,
    pub retry_heuristic_weight_percent: Option<i64>,
    pub warnings: Vec<String>,
}

/// `DetailedRoutingMetrics`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailedRoutingMetrics {
    pub routed_net_count: i64,
    pub unrouted_net_count: i64,
    pub conflict_resource_count: i64,
    pub total_conflict_overflow: i64,
    pub track_count: i64,
    pub via_count: i64,
    pub total_length_nm: i64,
    pub passes: i64,
}

impl DetailedRoutingMetrics {
    /// `quality_vector`.
    pub fn quality_vector(&self) -> [i64; 5] {
        [
            self.unrouted_net_count,
            self.total_conflict_overflow,
            self.conflict_resource_count,
            self.via_count,
            self.total_length_nm,
        ]
    }
}

/// `DetailedRoutingResult` as the unified router produces it: no repair
/// trials, no search work; `congestion` is the exported congestion map.
#[derive(Clone, Debug)]
pub struct DetailedRoutingResult {
    pub status: DetailedRoutingStatus,
    pub board: PhysicalBoard,
    pub nets: Vec<DetailedNetResult>,
    pub metrics: DetailedRoutingMetrics,
    pub locked_track_count: i64,
    pub locked_via_count: i64,
    pub global_routing_fingerprint: String,
    pub routing_fingerprint: String,
    pub congestion: Option<crate::unified_router::CongestionMap>,
}

/// `_fingerprint`: the SHA-256 of the key-sorted JSON of the global
/// fingerprint, every track and via, and the quality vector.
pub fn fingerprint(global_fingerprint: &str, board: &PhysicalBoard, metrics: &DetailedRoutingMetrics) -> String {
    let tracks: Vec<Value> = board
        .tracks
        .iter()
        .map(|item| {
            Value::Array(vec![
                Value::from(item.net.as_str()),
                Value::from(item.layer.value()),
                Value::from(item.start.x_nm),
                Value::from(item.start.y_nm),
                Value::from(item.end.x_nm),
                Value::from(item.end.y_nm),
                Value::from(item.width_nm),
            ])
        })
        .collect();
    let vias: Vec<Value> = board
        .vias
        .iter()
        .map(|item| {
            Value::Array(vec![
                Value::from(item.net.as_str()),
                Value::from(item.position.x_nm),
                Value::from(item.position.y_nm),
                Value::from(item.from_layer.value()),
                Value::from(item.to_layer.value()),
                Value::from(item.size_nm),
                Value::from(item.drill_nm),
            ])
        })
        .collect();
    let document = Value::Object(copper_json::Object::from([
        ("global".to_owned(), Value::from(global_fingerprint)),
        ("tracks".to_owned(), Value::Array(tracks)),
        ("vias".to_owned(), Value::Array(vias)),
        ("quality".to_owned(), Value::Array(metrics.quality_vector().iter().map(|v| Value::from(*v)).collect())),
    ]));
    let text = Dumps::new().sort_keys(true).dumps(&document);
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

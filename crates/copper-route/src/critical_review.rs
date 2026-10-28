//! Report-only review of the exported critical copper (`pcbir.critical_review`).
//!
//! Only boards without reviewed lanes, coupled pairs or match groups are
//! ported; the lane geometry (spacing, coupling, bends) reports not
//! implemented.

use copper_json::{Object, Value};
use copper_pcb::board::PhysicalBoard;

use crate::Result;
use crate::critical::{CriticalNetResult, critical_lane_table};

pub const LANE_REVIEW_RADIUS_NM: i64 = 1_000_000;
pub const COUPLING_FACTOR: i64 = 2;
pub const SHARP_BEND_DEGREES: i64 = 45;

/// `critical_lane_review(board, results, match_tuning)`.
pub fn critical_lane_review(board: &PhysicalBoard, results: &[CriticalNetResult]) -> Result<Object> {
    let lanes = critical_lane_table(board, results)?;
    if !lanes.is_empty() {
        return copper_pcb::not_implemented("the critical lane review of routed critical nets");
    }
    if results.iter().any(|item| item.nets.len() == 2) {
        return copper_pcb::not_implemented("the critical lane review of coupled pairs");
    }
    if !board.match_groups.is_empty() {
        return copper_pcb::not_implemented("the critical lane review of match groups (verify_match_groups)");
    }
    let mut zone_nets: Vec<String> = board.zones.iter().map(|zone| zone.net.clone()).collect();
    zone_nets.sort();
    zone_nets.dedup();
    let mut review = Object::new();
    review.insert("search_radius_nm".to_owned(), Value::from(LANE_REVIEW_RADIUS_NM));
    review.insert("coupling_factor".to_owned(), Value::from(COUPLING_FACTOR));
    review.insert("sharp_bend_degrees".to_owned(), Value::from(SHARP_BEND_DEGREES));
    review.insert("ignored_zone_nets".to_owned(), Value::Array(zone_nets.into_iter().map(Value::Str).collect()));
    review.insert("nets".to_owned(), Value::Array(Vec::new()));
    review.insert("pairs".to_owned(), Value::Array(Vec::new()));
    review.insert("match_groups".to_owned(), Value::Array(Vec::new()));
    Ok(review)
}

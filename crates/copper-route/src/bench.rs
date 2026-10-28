//! Board-file routing benchmark helpers (`pcbir.bench`).

use copper_json::Value;
use indexmap::IndexMap;

use crate::{Error, Result};

/// `dataclasses.fields(UnifiedRouterOptions)` names, in declaration order.
// PENDING: owned by rust-m4-unified (unified_router.UnifiedRouterOptions).
pub const UNIFIED_OPTION_FIELDS: [&str; 77] = [
    "pitch_nm",
    "maximum_iterations",
    "stall_iterations",
    "present_factor",
    "present_growth",
    "history_increment",
    "via_cost_mm",
    "wrong_way_cost",
    "outer_layer_cost",
    "attach_radius_nm",
    "neck_radius_nm",
    "attach_candidates",
    "window_margin_nm",
    "present_limit",
    "window_growth_limit",
    "maximum_expansions",
    "commit_rounds",
    "pour_recheck",
    "buses_first",
    "bus_length_budget",
    "bus_skew_tuning",
    "length_priority",
    "full_reroute_every",
    "rejection_marker_cost",
    "route_smoothing",
    "ball_field_cost",
    "reuse_plane_vias",
    "ball_field_via_sites",
    "ball_field_band",
    "ball_field_exact",
    "ball_field_exact_reach_nm",
    "exit_reservation_cost",
    "bus_bundles",
    "bga_fanout",
    "raster_margin_nm",
    "time_budget_s",
    "grid_window_nm",
    "critical_pair_state_limit",
    "critical_pair_state_escalation",
    "power_islands",
    "power_islands_ball_grid_only",
    "plane_fill_width",
    "implicit_neck_nm",
    "local_escape_radius_nm",
    "local_escape_step_nm",
    "save_inputs_path",
    "local_escape_limit",
    "pre_route_decouplers",
    "direct",
    "escapes_first",
    "escape_net_order",
    "escape_layer_biases",
    "retain_escape_copper",
    "planned_escapes_as_copper",
    "share_commit_index",
    "failed_retry_every",
    "failed_retry_limit",
    "failed_retry_before_exit",
    "outward_axis_threshold_nm",
    "attach_extension_steps_nm",
    "attach_extension_reach_nm",
    "local_escape_detour_nm",
    "local_escape_states",
    "open_flood_limit",
    "open_space_area_nm2",
    "plane_piece_area_nm2",
    "plane_piece_fraction",
    "repair_neighbour_reach_nm",
    "via_budget_factor",
    "via_overrun_factor",
    "bend_cost",
    "guide_cost",
    "guide_margin_nm",
    "congestion_tile_nm",
    "export_congestion",
    "fine_regions",
    "fine_region_options",
];

/// `unified_overrides`: `OPTION=VALUE` strings as UnifiedRouterOptions
/// overrides (JSON values, else text).
pub fn unified_overrides<S: AsRef<str>>(items: &[S]) -> Result<IndexMap<String, Value>> {
    let mut overrides = IndexMap::new();
    for item in items {
        let item = item.as_ref();
        let (name, text) = match item.split_once('=') {
            Some((name, text)) if UNIFIED_OPTION_FIELDS.contains(&name) => (name, text),
            Some((name, _)) => return Err(unknown(name)),
            None => return Err(unknown(item)),
        };
        let value = copper_json::parse(text).unwrap_or_else(|_| Value::Str(text.to_owned()));
        // A repeated name keeps its first position, as a dict assignment does.
        overrides.insert(name.to_owned(), value);
    }
    Ok(overrides)
}

fn unknown(name: &str) -> Error {
    Error::Value(format!("unknown unified option {}", copper_json::python_str_repr(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_parse_json_else_text() {
        let overrides = unified_overrides(&["buses_first=true", "power_islands=auto", "pitch_nm=250000"]).unwrap();
        assert_eq!(overrides["buses_first"], Value::Bool(true));
        assert_eq!(overrides["power_islands"], Value::Str("auto".to_owned()));
        let error = unified_overrides(&["nonsense=1"]).unwrap_err();
        assert_eq!(error.to_string(), "unknown unified option 'nonsense'");
        assert_eq!(unified_overrides(&["direct"]).unwrap_err().to_string(), "unknown unified option 'direct'");
    }
}

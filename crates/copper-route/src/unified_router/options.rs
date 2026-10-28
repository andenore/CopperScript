//! `UnifiedRouterOptions`, with `__post_init__`'s validation and the
//! `OPTION=VALUE` overrides of `replay-unified --set` and
//! `route-board --unified-set`.

use copper_json::{Value, python_str_repr};
use num_traits::ToPrimitive;

use crate::fine_regions::FineRegionOptions;
use crate::routing_costs::LayerBias;
use crate::{Error, Result, fail};

/// `FINE_REGION_MODES` (`"auto"` is an alias of `"dense"`).
pub const FINE_REGION_MODES: [&str; 3] = ["off", "arrays", "dense"];
/// `DEFAULT_PITCH_NM`: the base lattice pitch when `pitch_nm` is None.
pub const DEFAULT_PITCH_NM: i64 = 250_000;

/// `UnifiedRouterOptions`, field for field in Python's order.
#[derive(Clone, Debug, PartialEq)]
pub struct UnifiedRouterOptions {
    pub pitch_nm: Option<i64>,
    pub maximum_iterations: i64,
    pub stall_iterations: i64,
    pub present_factor: f64,
    pub present_growth: f64,
    pub history_increment: f64,
    pub via_cost_mm: f64,
    pub wrong_way_cost: f64,
    pub outer_layer_cost: f64,
    pub attach_radius_nm: i64,
    pub neck_radius_nm: i64,
    pub attach_candidates: i64,
    pub window_margin_nm: i64,
    pub present_limit: f64,
    pub window_growth_limit: i64,
    pub maximum_expansions: i64,
    pub commit_rounds: i64,
    pub pour_recheck: bool,
    pub buses_first: bool,
    pub bus_length_budget: bool,
    pub bus_skew_tuning: bool,
    pub length_priority: bool,
    pub full_reroute_every: i64,
    pub rejection_marker_cost: f64,
    pub route_smoothing: bool,
    pub ball_field_cost: f64,
    pub reuse_plane_vias: bool,
    pub ball_field_via_sites: bool,
    pub ball_field_band: i64,
    pub ball_field_exact: bool,
    pub ball_field_exact_reach_nm: Option<i64>,
    pub exit_reservation_cost: f64,
    pub bus_bundles: bool,
    pub bga_fanout: bool,
    pub raster_margin_nm: Option<i64>,
    pub time_budget_s: Option<f64>,
    pub grid_window_nm: Option<(i64, i64, i64, i64)>,
    pub critical_pair_state_limit: Option<i64>,
    pub critical_pair_state_escalation: i64,
    pub power_islands: Option<String>,
    pub power_islands_ball_grid_only: bool,
    pub plane_fill_width: bool,
    pub implicit_neck_nm: i64,
    pub local_escape_radius_nm: i64,
    pub local_escape_step_nm: i64,
    pub save_inputs_path: Option<String>,
    pub local_escape_limit: i64,
    pub pre_route_decouplers: bool,
    pub direct: bool,
    pub escapes_first: bool,
    pub escape_net_order: Vec<String>,
    pub escape_layer_biases: Vec<LayerBias>,
    pub retain_escape_copper: bool,
    pub planned_escapes_as_copper: bool,
    pub share_commit_index: bool,
    pub failed_retry_every: i64,
    pub failed_retry_limit: i64,
    pub failed_retry_before_exit: bool,
    pub outward_axis_threshold_nm: i64,
    pub attach_extension_steps_nm: Vec<i64>,
    pub attach_extension_reach_nm: i64,
    pub local_escape_detour_nm: i64,
    pub local_escape_states: i64,
    pub open_flood_limit: i64,
    pub open_space_area_nm2: i64,
    pub plane_piece_area_nm2: i64,
    pub plane_piece_fraction: f64,
    pub repair_neighbour_reach_nm: i64,
    pub via_budget_factor: f64,
    pub via_overrun_factor: f64,
    pub bend_cost: f64,
    pub guide_cost: f64,
    pub guide_margin_nm: i64,
    pub congestion_tile_nm: i64,
    pub export_congestion: bool,
    pub fine_regions: String,
    pub fine_region_options: Option<FineRegionOptions>,
}

/// The dataclass field names, in order (`fields(UnifiedRouterOptions)`).
pub const FIELDS: [&str; 81] = [
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

impl Default for UnifiedRouterOptions {
    fn default() -> Self {
        UnifiedRouterOptions {
            pitch_nm: None,
            maximum_iterations: 150,
            stall_iterations: 60,
            present_factor: 0.6,
            present_growth: 1.5,
            history_increment: 0.8,
            via_cost_mm: 1.5,
            wrong_way_cost: 0.5,
            outer_layer_cost: 0.0,
            attach_radius_nm: 1_500_000,
            neck_radius_nm: 1_200_000,
            attach_candidates: 6,
            window_margin_nm: 2_500_000,
            present_limit: 0.0,
            window_growth_limit: 6,
            maximum_expansions: 400_000,
            commit_rounds: 4,
            pour_recheck: true,
            buses_first: true,
            bus_length_budget: false,
            bus_skew_tuning: true,
            length_priority: false,
            full_reroute_every: 0,
            rejection_marker_cost: 0.0,
            route_smoothing: true,
            ball_field_cost: 2.0,
            reuse_plane_vias: true,
            ball_field_via_sites: false,
            ball_field_band: 0,
            ball_field_exact: true,
            ball_field_exact_reach_nm: None,
            exit_reservation_cost: 0.0,
            bus_bundles: false,
            bga_fanout: true,
            raster_margin_nm: None,
            time_budget_s: None,
            grid_window_nm: None,
            critical_pair_state_limit: None,
            critical_pair_state_escalation: 10,
            power_islands: None,
            power_islands_ball_grid_only: false,
            plane_fill_width: true,
            implicit_neck_nm: 2_500_000,
            local_escape_radius_nm: 2_000_000,
            local_escape_step_nm: 25_000,
            save_inputs_path: None,
            local_escape_limit: 40,
            pre_route_decouplers: true,
            direct: false,
            escapes_first: true,
            escape_net_order: Vec::new(),
            escape_layer_biases: Vec::new(),
            retain_escape_copper: false,
            planned_escapes_as_copper: true,
            share_commit_index: true,
            failed_retry_every: 3,
            failed_retry_limit: 0,
            failed_retry_before_exit: true,
            outward_axis_threshold_nm: 300_000,
            attach_extension_steps_nm: vec![300_000, 500_000, 750_000, 1_000_000, 1_500_000],
            attach_extension_reach_nm: 600_000,
            local_escape_detour_nm: 500_000,
            local_escape_states: 20_000,
            open_flood_limit: 600,
            open_space_area_nm2: 2_000_000 * 2_000_000,
            plane_piece_area_nm2: 800_000 * 800_000,
            plane_piece_fraction: 0.25,
            repair_neighbour_reach_nm: 1_000_000,
            via_budget_factor: 4.0,
            via_overrun_factor: 10.0,
            bend_cost: 0.25,
            guide_cost: 0.0,
            guide_margin_nm: 1_000_000,
            congestion_tile_nm: 1_000_000,
            export_congestion: true,
            fine_regions: "arrays".to_owned(),
            fine_region_options: None,
        }
    }
}

/// `type(value) is not int` on an override: the integer-only checks of
/// `__post_init__` fail on a float, bool or text value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Other,
}

impl UnifiedRouterOptions {
    /// `UnifiedRouterOptions.__post_init__`: validation and the `"auto"` alias.
    pub fn validated(mut self) -> Result<Self> {
        self.check(Kind::Int, Kind::Int, Kind::Int, Kind::Int)?;
        if self.fine_regions == "auto" {
            self.fine_regions = "dense".to_owned();
        }
        if !FINE_REGION_MODES.contains(&self.fine_regions.as_str()) {
            return fail("fine_regions must be \"off\", \"arrays\" or \"dense\"");
        }
        Ok(self)
    }

    /// The checks of `__post_init__` in order; `limit`, `escalation`, `steps`
    /// and `tile` say whether the integer-only fields held integers.
    fn check(&self, limit: Kind, escalation: Kind, steps: Kind, tile: Kind) -> Result<()> {
        if self.critical_pair_state_limit.is_some_and(|value| limit != Kind::Int || value <= 0) {
            return fail("critical pair state limit must be a positive integer or None");
        }
        if escalation != Kind::Int || self.critical_pair_state_escalation < 1 {
            return fail("critical pair state escalation must be an integer of at least 1");
        }
        if self.pitch_nm.is_some_and(|pitch| pitch <= 0) || self.maximum_iterations <= 0 || self.maximum_expansions <= 0 {
            return fail("unified router pitch, iterations and search states must be positive");
        }
        if self.escape_layer_biases.iter().any(|bias| bias.rate < 0) {
            return fail("escape layer biases cannot be negative");
        }
        if self.stall_iterations < 0 {
            return fail("unified router stall iterations must not be negative");
        }
        if self.failed_retry_every < 0 || self.failed_retry_limit < 0 {
            return fail("unified router failed-net retry interval and limit must not be negative");
        }
        if steps != Kind::Int
            || self.attach_extension_steps_nm.is_empty()
            || self.attach_extension_steps_nm.iter().any(|step| *step <= 0)
        {
            return fail("attach_extension_steps_nm requires positive integer nanometre steps");
        }
        if self.outward_axis_threshold_nm < 0
            || self.attach_extension_reach_nm < 0
            || self.local_escape_detour_nm < 0
            || self.repair_neighbour_reach_nm < 0
            || self.open_space_area_nm2 < 0
            || self.plane_piece_area_nm2 < 0
        {
            return fail("unified router distances and areas must not be negative");
        }
        if self.local_escape_states <= 0 || self.open_flood_limit <= 0 {
            return fail("unified router local escape states and open flood limit must be positive");
        }
        if !(0.0 < self.plane_piece_fraction && self.plane_piece_fraction <= 1.0) {
            return fail("plane_piece_fraction must be in (0, 1]");
        }
        if self.via_budget_factor <= 0.0 || self.via_overrun_factor < 1.0 {
            return fail("via_budget_factor must be positive and via_overrun_factor at least 1");
        }
        if !(self.bend_cost >= 0.0 && self.guide_cost >= 0.0) || self.guide_margin_nm < 0 {
            return fail("bend_cost, guide_cost and guide_margin_nm must not be negative");
        }
        if let Some((x0, y0, x1, y1)) = self.grid_window_nm
            && (x0 >= x1 || y0 >= y1)
        {
            return fail("grid_window_nm requires four ordered integer nanometre coordinates");
        }
        if tile != Kind::Int
            || self.congestion_tile_nm <= 0
            || self.pitch_nm.is_some_and(|pitch| self.congestion_tile_nm < pitch)
        {
            return fail("congestion_tile_nm must be a positive integer of at least one pitch");
        }
        Ok(())
    }

    /// `dataclasses.replace(options, **overrides)` with JSON (or text) values,
    /// then `__post_init__`. A value whose Python behaviour is dynamic (a
    /// text in a number field, a float in an int field) is not implemented
    /// unless `__post_init__` rejects it with a `ValueError`.
    pub fn with_overrides(&self, overrides: &[(String, Value)]) -> Result<Self> {
        let mut options = self.clone();
        let (mut limit, mut escalation, mut steps, mut tile) = (Kind::Int, Kind::Int, Kind::Int, Kind::Int);
        for (name, value) in overrides {
            let unsupported = |what: &str| -> Error {
                Error::not_implemented(format!(
                    "unified option {name}={} ({what}) relies on Python's dynamic typing",
                    python_str_repr(&copper_json::Dumps::compact().dumps(value)),
                ))
            };
            let int = |value: &Value| -> Result<i64> {
                match value {
                    Value::Int(number) => number.to_i64().ok_or_else(|| unsupported("an integer beyond 64 bits")),
                    _ => Err(unsupported("not an integer")),
                }
            };
            let float = |value: &Value| -> Result<f64> {
                match value {
                    Value::Float(number) => Ok(*number),
                    Value::Int(number) => {
                        number.to_i64().filter(|n| n.abs() < 1 << 53).map(|n| n as f64).ok_or_else(|| unsupported("a large integer in a float field"))
                    }
                    _ => Err(unsupported("not a number")),
                }
            };
            let boolean = |value: &Value| -> Result<bool> {
                match value {
                    Value::Bool(flag) => Ok(*flag),
                    _ => Err(unsupported("not a boolean")),
                }
            };
            let optional_int = |value: &Value| -> Result<Option<i64>> {
                match value {
                    Value::Null => Ok(None),
                    other => int(other).map(Some),
                }
            };
            let text = |value: &Value| -> Result<String> {
                match value {
                    Value::Str(text) => Ok(text.clone()),
                    _ => Err(unsupported("not text")),
                }
            };
            let optional_text = |value: &Value| -> Result<Option<String>> {
                match value {
                    Value::Null => Ok(None),
                    other => text(other).map(Some),
                }
            };
            // An integer-only field holding another JSON type fails in
            // `__post_init__` with its ValueError (reported after the loop).
            let kind = |value: &Value| if matches!(value, Value::Int(_)) { Kind::Int } else { Kind::Other };
            match name.as_str() {
                "pitch_nm" => options.pitch_nm = optional_int(value)?,
                "maximum_iterations" => options.maximum_iterations = int(value)?,
                "stall_iterations" => options.stall_iterations = int(value)?,
                "present_factor" => options.present_factor = float(value)?,
                "present_growth" => options.present_growth = float(value)?,
                "history_increment" => options.history_increment = float(value)?,
                "via_cost_mm" => options.via_cost_mm = float(value)?,
                "wrong_way_cost" => options.wrong_way_cost = float(value)?,
                "outer_layer_cost" => options.outer_layer_cost = float(value)?,
                "attach_radius_nm" => options.attach_radius_nm = int(value)?,
                "neck_radius_nm" => options.neck_radius_nm = int(value)?,
                "attach_candidates" => options.attach_candidates = int(value)?,
                "window_margin_nm" => options.window_margin_nm = int(value)?,
                "present_limit" => options.present_limit = float(value)?,
                "window_growth_limit" => options.window_growth_limit = int(value)?,
                "maximum_expansions" => options.maximum_expansions = int(value)?,
                "commit_rounds" => options.commit_rounds = int(value)?,
                "pour_recheck" => options.pour_recheck = boolean(value)?,
                "buses_first" => options.buses_first = boolean(value)?,
                "bus_length_budget" => options.bus_length_budget = boolean(value)?,
                "bus_skew_tuning" => options.bus_skew_tuning = boolean(value)?,
                "length_priority" => options.length_priority = boolean(value)?,
                "full_reroute_every" => options.full_reroute_every = int(value)?,
                "rejection_marker_cost" => options.rejection_marker_cost = float(value)?,
                "route_smoothing" => options.route_smoothing = boolean(value)?,
                "ball_field_cost" => options.ball_field_cost = float(value)?,
                "reuse_plane_vias" => options.reuse_plane_vias = boolean(value)?,
                "ball_field_via_sites" => options.ball_field_via_sites = boolean(value)?,
                "ball_field_band" => options.ball_field_band = int(value)?,
                "ball_field_exact" => options.ball_field_exact = boolean(value)?,
                "ball_field_exact_reach_nm" => options.ball_field_exact_reach_nm = optional_int(value)?,
                "exit_reservation_cost" => options.exit_reservation_cost = float(value)?,
                "bus_bundles" => options.bus_bundles = boolean(value)?,
                "bga_fanout" => options.bga_fanout = boolean(value)?,
                "raster_margin_nm" => options.raster_margin_nm = optional_int(value)?,
                "time_budget_s" => {
                    options.time_budget_s = match value {
                        Value::Null => None,
                        other => Some(float(other)?),
                    }
                }
                "grid_window_nm" => {
                    options.grid_window_nm = match value {
                        Value::Null => None,
                        Value::Array(items) => {
                            if items.len() != 4 || items.iter().any(|item| !matches!(item, Value::Int(_))) {
                                return fail("grid_window_nm requires four ordered integer nanometre coordinates");
                            }
                            let bounds: Vec<i64> = items.iter().map(int).collect::<Result<_>>()?;
                            Some((bounds[0], bounds[1], bounds[2], bounds[3]))
                        }
                        _ => return fail("grid_window_nm requires four ordered integer nanometre coordinates"),
                    }
                }
                "critical_pair_state_limit" => {
                    limit = kind(value);
                    options.critical_pair_state_limit = match (value, limit) {
                        (Value::Null, _) => None,
                        (other, Kind::Int) => Some(int(other)?),
                        // Any other value fails `__post_init__`'s type check.
                        _ => Some(1),
                    }
                }
                "critical_pair_state_escalation" => {
                    escalation = kind(value);
                    options.critical_pair_state_escalation = if escalation == Kind::Int { int(value)? } else { 1 };
                }
                "power_islands" => options.power_islands = optional_text(value)?,
                "power_islands_ball_grid_only" => options.power_islands_ball_grid_only = boolean(value)?,
                "plane_fill_width" => options.plane_fill_width = boolean(value)?,
                "implicit_neck_nm" => options.implicit_neck_nm = int(value)?,
                "local_escape_radius_nm" => options.local_escape_radius_nm = int(value)?,
                "local_escape_step_nm" => options.local_escape_step_nm = int(value)?,
                "save_inputs_path" => options.save_inputs_path = optional_text(value)?,
                "local_escape_limit" => options.local_escape_limit = int(value)?,
                "pre_route_decouplers" => options.pre_route_decouplers = boolean(value)?,
                "direct" => options.direct = boolean(value)?,
                "escapes_first" => options.escapes_first = boolean(value)?,
                "escape_net_order" => {
                    options.escape_net_order = match value {
                        Value::Array(items) => items.iter().map(text).collect::<Result<_>>()?,
                        _ => return Err(unsupported("not a list of net names")),
                    }
                }
                "escape_layer_biases" => {
                    options.escape_layer_biases = match value {
                        Value::Array(items) if items.is_empty() => Vec::new(),
                        _ => return Err(unsupported("layer biases are dataclasses")),
                    }
                }
                "retain_escape_copper" => options.retain_escape_copper = boolean(value)?,
                "planned_escapes_as_copper" => options.planned_escapes_as_copper = boolean(value)?,
                "share_commit_index" => options.share_commit_index = boolean(value)?,
                "failed_retry_every" => options.failed_retry_every = int(value)?,
                "failed_retry_limit" => options.failed_retry_limit = int(value)?,
                "failed_retry_before_exit" => options.failed_retry_before_exit = boolean(value)?,
                "outward_axis_threshold_nm" => options.outward_axis_threshold_nm = int(value)?,
                "attach_extension_steps_nm" => {
                    options.attach_extension_steps_nm = match value {
                        Value::Array(items) => {
                            steps = if items.iter().all(|item| matches!(item, Value::Int(_))) { Kind::Int } else { Kind::Other };
                            if steps == Kind::Int { items.iter().map(int).collect::<Result<_>>()? } else { vec![1] }
                        }
                        _ => return fail("attach_extension_steps_nm requires positive integer nanometre steps"),
                    }
                }
                "attach_extension_reach_nm" => options.attach_extension_reach_nm = int(value)?,
                "local_escape_detour_nm" => options.local_escape_detour_nm = int(value)?,
                "local_escape_states" => options.local_escape_states = int(value)?,
                "open_flood_limit" => options.open_flood_limit = int(value)?,
                "open_space_area_nm2" => options.open_space_area_nm2 = int(value)?,
                "plane_piece_area_nm2" => options.plane_piece_area_nm2 = int(value)?,
                "plane_piece_fraction" => options.plane_piece_fraction = float(value)?,
                "repair_neighbour_reach_nm" => options.repair_neighbour_reach_nm = int(value)?,
                "via_budget_factor" => options.via_budget_factor = float(value)?,
                "via_overrun_factor" => options.via_overrun_factor = float(value)?,
                "bend_cost" => options.bend_cost = float(value)?,
                "guide_cost" => options.guide_cost = float(value)?,
                "guide_margin_nm" => options.guide_margin_nm = int(value)?,
                "congestion_tile_nm" => {
                    tile = kind(value);
                    options.congestion_tile_nm = if tile == Kind::Int { int(value)? } else { 1 };
                }
                "export_congestion" => options.export_congestion = boolean(value)?,
                "fine_regions" => options.fine_regions = text(value)?,
                "fine_region_options" => {
                    options.fine_region_options = match value {
                        Value::Null => None,
                        _ => return fail("fine_region_options must be a FineRegionOptions or None"),
                    }
                }
                other => return fail(format!("unknown unified option {}", python_str_repr(other))),
            }
        }
        options.check(limit, escalation, steps, tile)?;
        if options.fine_regions == "auto" {
            options.fine_regions = "dense".to_owned();
        }
        if !FINE_REGION_MODES.contains(&options.fine_regions.as_str()) {
            return fail("fine_regions must be \"off\", \"arrays\" or \"dense\"");
        }
        Ok(options)
    }
}

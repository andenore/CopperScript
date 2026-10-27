//! `PlacementPlannerOptions`: search bounds and soft weights.

use indexmap::IndexSet;

use copper_drc::legality::Policy;
use copper_pcb::board::DEFAULT_ADDITIONAL_COURTYARD_GAP_NM;
use copper_pcb::{Error, Result};

/// `PlacementPlannerOptions`, with its `__post_init__` validation in [`PlannerOptions::validated`].
#[derive(Clone, Debug, PartialEq)]
pub struct PlannerOptions {
    pub grid_step_nm: i64,
    pub edge_clearance_nm: i64,
    pub component_clearance_nm: i64,
    pub congestion_bin_nm: i64,
    pub analytical_iterations: i64,
    /// Seed ungrouped parts at their connected pads.
    pub satellite_seeding: bool,
    /// Keep movable parts out of fixed differential pairs' paths.
    pub auto_pair_corridors: bool,
    pub analytical_step_mm: f64,
    pub analytical_momentum: f64,
    pub refinement_passes: i64,
    pub refinement_radius_steps: i64,
    pub candidate_count: i64,
    pub legalization_candidates: i64,
    pub exact_repair_limit: i64,
    pub exact_repair_candidates: i64,
    pub fixed_references: IndexSet<String>,
    pub escape_margin_nm: i64,
    pub escape_transit_lanes: i64,
    pub escape_spacing_passes: i64,
    pub escape_max_movement_nm: i64,
    pub power_domain_weight: f64,
    pub support_pattern_trials: i64,
    pub package_support_trials: i64,
    pub decoupling_placement_trials: i64,
    pub seed_count: Option<i64>,
}

impl Default for PlannerOptions {
    fn default() -> Self {
        PlannerOptions {
            grid_step_nm: 1_000_000,
            edge_clearance_nm: 2_000_000,
            component_clearance_nm: DEFAULT_ADDITIONAL_COURTYARD_GAP_NM,
            congestion_bin_nm: 5_000_000,
            analytical_iterations: 40,
            satellite_seeding: true,
            auto_pair_corridors: true,
            analytical_step_mm: 0.08,
            analytical_momentum: 0.85,
            refinement_passes: 2,
            refinement_radius_steps: 2,
            candidate_count: 3,
            legalization_candidates: 96,
            exact_repair_limit: 6,
            exact_repair_candidates: 28,
            fixed_references: IndexSet::new(),
            escape_margin_nm: 500_000,
            escape_transit_lanes: 1,
            escape_spacing_passes: 2,
            escape_max_movement_nm: 16_000_000,
            power_domain_weight: 0.25,
            support_pattern_trials: 0,
            package_support_trials: 0,
            decoupling_placement_trials: 64,
            seed_count: None,
        }
    }
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

impl PlannerOptions {
    /// `PlacementPlannerOptions.__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if self.seed_count.is_some_and(|count| count < 1) {
            return invalid("placement seed count must be a positive integer");
        }
        if self.decoupling_placement_trials < 0 {
            return invalid("decoupling placement trials must be a nonnegative integer");
        }
        if self.support_pattern_trials < 0 {
            return invalid("support pattern trials must be a nonnegative integer");
        }
        if self.package_support_trials < 0 {
            return invalid("package support trials must be a nonnegative integer");
        }
        if [self.grid_step_nm, self.edge_clearance_nm, self.component_clearance_nm, self.congestion_bin_nm]
            .iter()
            .any(|value| *value <= 0)
        {
            return invalid("placement planner dimensions must be positive");
        }
        if self.analytical_iterations < 0 || self.analytical_step_mm <= 0.0 {
            return invalid("analytical placement settings are invalid");
        }
        if !self.power_domain_weight.is_finite() || !(0.0..=1.0).contains(&self.power_domain_weight) {
            return invalid("power-domain weight must be finite and in [0, 1]");
        }
        if !(0.0..1.0).contains(&self.analytical_momentum) {
            return invalid("analytical momentum must be in [0, 1)");
        }
        if self.refinement_passes < 0 || self.refinement_radius_steps < 1 {
            return invalid("placement refinement settings are invalid");
        }
        if self.candidate_count < 1 {
            return invalid("placement candidate count must be positive");
        }
        if self.escape_margin_nm < 0
            || self.escape_transit_lanes < 0
            || self.escape_spacing_passes < 0
            || self.escape_max_movement_nm <= 0
        {
            return invalid("escape-spacing settings are invalid");
        }
        if self.legalization_candidates < 1 || self.exact_repair_limit < 2 || self.exact_repair_candidates < 1 {
            return invalid("exact repair settings are invalid");
        }
        Ok(self)
    }

    /// The hard-predicate policy these options imply.
    pub fn policy(&self) -> Policy {
        Policy {
            edge_clearance_nm: self.edge_clearance_nm,
            component_clearance_nm: self.component_clearance_nm,
            auto_pair_corridors: self.auto_pair_corridors,
            fixed_references: self.fixed_references.clone(),
        }
    }

    /// `options.legalization_candidates * 64`, the raw probe budget per member.
    pub(crate) fn probe_limit(&self) -> usize {
        (self.legalization_candidates * 64) as usize
    }
}

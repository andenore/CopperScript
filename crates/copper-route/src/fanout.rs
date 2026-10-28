//! Package pad escapes before routing (`pcbir.fanout`).

use std::collections::BTreeSet;

use copper_pcb::board::PadReference;

use crate::escape_assignment::EscapeAssignmentOptions;
use crate::{Error, Result, nm_from_mm};

/// `FanoutOptions`.
#[derive(Clone, Debug, PartialEq)]
pub struct FanoutOptions {
    pub minimum_component_pads: i64,
    pub escape_small_dense_packages: bool,
    pub maximum_neighbor_distance_nm: i64,
    pub step_nm: i64,
    pub maximum_radius_nm: i64,
    pub constrained_pins_first: bool,
    pub two_leg_escapes: bool,
    pub maximum_two_leg_candidates: i64,
    pub joint_escapes: bool,
    pub assignment_options: EscapeAssignmentOptions,
    pub refinement_step_nm: i64,
    pub maximum_refined_two_leg_candidates: i64,
    pub maze_escapes: bool,
    pub maze_state_budget: i64,
    pub surface_fallback: bool,
    /// Runtime planning policy for early ordinary reservations. Prefer a
    /// checked surface channel before adding a transition; keep via
    /// alternatives for joint conflicts. Source layer/width/length rules
    /// remain authoritative.
    pub prefer_surface_escapes: bool,
    pub surface_preference_depth: Option<i64>,
    /// Pins an escape plan keeps on their own surface (escape planning E3):
    /// a checked surface escape first, as `prefer_surface_escapes`, at any depth.
    pub surface_pads: BTreeSet<PadReference>,
    /// Runtime policy: keep these source associations pending for the native
    /// power-preparation ledger. This alone never certifies a bypass connection.
    pub plane_return_nets: BTreeSet<String>,
    pub diagnostic_samples_per_pad: i64,
    /// Experimental extra site family. Zero retains established generation work.
    pub maximum_geometry_candidates: i64,
}

impl Default for FanoutOptions {
    fn default() -> Self {
        FanoutOptions {
            minimum_component_pads: 12,
            escape_small_dense_packages: true,
            maximum_neighbor_distance_nm: 1_500_000,
            step_nm: 500_000,
            maximum_radius_nm: nm_from_mm(3),
            constrained_pins_first: true,
            two_leg_escapes: true,
            maximum_two_leg_candidates: 256,
            joint_escapes: true,
            assignment_options: EscapeAssignmentOptions::default(),
            refinement_step_nm: 100_000,
            maximum_refined_two_leg_candidates: 1500,
            maze_escapes: false,
            maze_state_budget: 12_000,
            surface_fallback: true,
            prefer_surface_escapes: false,
            surface_preference_depth: None,
            surface_pads: BTreeSet::new(),
            plane_return_nets: BTreeSet::new(),
            diagnostic_samples_per_pad: 24,
            maximum_geometry_candidates: 0,
        }
    }
}

impl FanoutOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        let fail = |message: &str| Err(Error::Value(message.to_owned()));
        if self.prefer_surface_escapes && !self.surface_fallback {
            return fail("surface preference requires enabled surface fallback");
        }
        if self.surface_preference_depth.is_some_and(|depth| depth < 0) {
            return fail("surface preference depth must be a nonnegative integer or None");
        }
        if [
            self.minimum_component_pads,
            self.maximum_neighbor_distance_nm,
            self.step_nm,
            self.maximum_radius_nm,
            self.maximum_two_leg_candidates,
            self.refinement_step_nm,
            self.maximum_refined_two_leg_candidates,
            self.maze_state_budget,
        ]
        .into_iter()
        .min()
        .expect("bounds")
            <= 0
        {
            return fail("fanout options must be positive");
        }
        if self.refinement_step_nm > self.step_nm {
            return fail("fanout refinement step must not exceed coarse step");
        }
        if self.diagnostic_samples_per_pad < 0 {
            return fail("fanout diagnostic samples must be a nonnegative integer");
        }
        if self.maximum_geometry_candidates < 0 {
            return fail("fanout geometry candidates must be a nonnegative integer");
        }
        Ok(self)
    }
}

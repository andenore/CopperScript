//! Placement/routing feedback (`pcbir.routeflow`).
// PENDING: owned by rust-m4-global (only the options the pipeline validates).

use crate::{Error, Result, nm_from_mm};

/// `PlacementRoutingFeedbackOptions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementRoutingFeedbackOptions {
    pub maximum_iterations: i64,
    pub initial_movement_nm: i64,
    pub minimum_movement_nm: i64,
    pub maximum_trials_per_iteration: i64,
    pub stagnation_limit: i64,
    pub preferred_candidate_id: Option<String>,
}

impl Default for PlacementRoutingFeedbackOptions {
    fn default() -> Self {
        PlacementRoutingFeedbackOptions {
            maximum_iterations: 4,
            initial_movement_nm: nm_from_mm(5),
            minimum_movement_nm: 500_000,
            maximum_trials_per_iteration: 24,
            stagnation_limit: 3,
            preferred_candidate_id: None,
        }
    }
}

impl PlacementRoutingFeedbackOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        let fail = |message: &str| Err(Error::Value(message.to_owned()));
        if [self.maximum_iterations, self.initial_movement_nm, self.minimum_movement_nm, self.stagnation_limit]
            .into_iter()
            .min()
            .expect("bounds")
            <= 0
        {
            return fail("placement-routing feedback options must be positive");
        }
        if self.maximum_trials_per_iteration < 0 {
            return fail("placement-routing trial budget must be a nonnegative integer");
        }
        if self.minimum_movement_nm > self.initial_movement_nm {
            return fail("minimum feedback movement exceeds initial movement");
        }
        Ok(self)
    }
}

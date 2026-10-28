//! Joint package escape assignment (`pcbir.escape_assignment`): its options.

use crate::{Error, Result};

/// `EscapeAssignmentOptions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapeAssignmentOptions {
    /// None tries each missing root once, within the shared pair/query budgets.
    pub maximum_trials: Option<i64>,
    pub maximum_cluster_pins: i64,
    pub maximum_search_states: i64,
    pub maximum_pair_checks: i64,
    pub maximum_pair_queries: i64,
}

impl Default for EscapeAssignmentOptions {
    fn default() -> Self {
        EscapeAssignmentOptions {
            maximum_trials: None,
            maximum_cluster_pins: 12,
            maximum_search_states: 20_000,
            maximum_pair_checks: 200_000,
            maximum_pair_queries: 2_000_000,
        }
    }
}

impl EscapeAssignmentOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if self.maximum_trials.is_some_and(|trials| trials <= 0) {
            return Err(Error::Value("escape assignment trial budget must be a positive integer or None".to_owned()));
        }
        if [self.maximum_cluster_pins, self.maximum_search_states, self.maximum_pair_checks, self.maximum_pair_queries]
            .into_iter()
            .min()
            .expect("budgets")
            <= 0
        {
            return Err(Error::Value("escape assignment budgets must be positive".to_owned()));
        }
        Ok(self)
    }
}

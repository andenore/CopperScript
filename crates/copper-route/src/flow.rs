//! Compatible orchestration of placement feedback, routing, and signoff
//! (`pcbir.flow`).
//!
//! The stage order is Python's: placement and global routing, escape frames,
//! critical routing, plane stitching, ordinary (unified) routing, placement
//! feedback and signoff. Stages owned by streams that have not landed report
//! [`Error::NotImplemented`] when the run reaches them, never a wrong answer.

use copper_pcb::board::PhysicalBoard;
use copper_place::PlannerOptions;

use crate::escape_plan::EscapePlanOptions;
use crate::fanout::FanoutOptions;
use crate::plane::PlaneStitchOptions;
use crate::progress::{ProgressCallback, emit};
use crate::routeflow::PlacementRoutingFeedbackOptions;
use crate::routing::GlobalRouterOptions;
use crate::{Error, Result};

/// `PhysicalFlowStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalFlowStatus {
    Pass,
    Fail,
}

impl PhysicalFlowStatus {
    pub fn value(self) -> &'static str {
        match self {
            PhysicalFlowStatus::Pass => "pass",
            PhysicalFlowStatus::Fail => "fail",
        }
    }
}

/// Stage boundaries where an iteration may stop early (`stop_after`).
/// Stopping never passes a gate: the result is a FAIL with `stopped_after` set.
pub const STOP_STAGES: [&str; 1] = ["placement"];

/// The unified router's options as far as the pipeline reads them before
/// routing (`UnifiedRouterOptions`).
// PENDING: owned by rust-m4-unified.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnifiedPipelineOptions {
    pub critical_pair_state_limit: Option<i64>,
    pub direct: bool,
    pub save_inputs_path: Option<String>,
}

/// The keyword arguments of `run_routing_pipeline`.
#[derive(Clone, Debug)]
pub struct RoutingPipelineOptions {
    pub placement_options: Option<PlannerOptions>,
    pub global_options: Option<GlobalRouterOptions>,
    pub feedback_options: Option<PlacementRoutingFeedbackOptions>,
    pub fanout_options: Option<FanoutOptions>,
    pub plane_stitch_options: Option<PlaneStitchOptions>,
    pub detailed_feedback_trials: i64,
    pub detailed_feedback_movement_nm: i64,
    pub critical_feedback_trials: i64,
    pub critical_pair_state_limit: i64,
    pub unified_options: Option<UnifiedPipelineOptions>,
    pub stop_after: Option<String>,
    pub escape_plan_options: Option<EscapePlanOptions>,
    pub placement_feedback_trials: i64,
}

impl Default for RoutingPipelineOptions {
    fn default() -> Self {
        RoutingPipelineOptions {
            placement_options: None,
            global_options: None,
            feedback_options: None,
            fanout_options: None,
            plane_stitch_options: None,
            detailed_feedback_trials: 0,
            detailed_feedback_movement_nm: 500_000,
            critical_feedback_trials: 0,
            critical_pair_state_limit: 6_000,
            unified_options: None,
            stop_after: None,
            escape_plan_options: None,
            placement_feedback_trials: 0,
        }
    }
}

/// `RoutingPipelineResult`; the stage results join as their streams land.
#[derive(Clone, Debug)]
pub struct RoutingPipelineResult {
    pub status: PhysicalFlowStatus,
    pub stopped_after: Option<String>,
}

/// `run_routing_pipeline`: steps 4-8 in dependency order with the unified
/// negotiated router.
pub fn run_routing_pipeline(
    board: PhysicalBoard,
    options: RoutingPipelineOptions,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<RoutingPipelineResult> {
    let unified_options = options.unified_options.clone().unwrap_or_default();
    let board = copper_pcb::hard_macros::macro_source(board)?;
    let placement_options = match options.placement_options {
        Some(placement_options) => placement_options,
        None => PlannerOptions::default().validated()?,
    };
    let global_options = match options.global_options {
        Some(global_options) => global_options,
        None => GlobalRouterOptions::default().validated()?,
    };
    let feedback_options = match options.feedback_options {
        Some(feedback_options) => feedback_options,
        None => PlacementRoutingFeedbackOptions {
            initial_movement_nm: global_options.tile_size_nm,
            ..PlacementRoutingFeedbackOptions::default()
        }
        .validated()?,
    };
    if options.detailed_feedback_trials < 0
        || options.critical_feedback_trials < 0
        || options.placement_feedback_trials < 0
        || options.detailed_feedback_movement_nm <= 0
    {
        return Err(Error::Value("routing placement feedback bounds are invalid".to_owned()));
    }
    if options.critical_pair_state_limit < 0 {
        return Err(Error::Value("critical pair state limit must be non-negative".to_owned()));
    }
    if let Some(stop_after) = &options.stop_after
        && !STOP_STAGES.contains(&stop_after.as_str())
    {
        return Err(Error::Value(format!("stop_after must be one of {}", STOP_STAGES.join(", "))));
    }
    // The placement stage consumes these once rust-m4-global lands it.
    let _ = (&board, &placement_options, &feedback_options, &unified_options);
    emit(&mut on_progress, "placement_global", "started", Vec::new());
    copper_pcb::not_implemented(
        "the placement and global routing stage (routeflow.optimize_placement_for_routing, rust-m4-global)",
    )
}

//! Unified negotiated-congestion detailed router (`pcbir.unified_router`).
//!
//! Every routed net competes for one shared grid. Paths may overlap during
//! search at a price (PathFinder's present and history costs); only fixed
//! copper is immovable. Legality works in two layers: conservative rasters
//! of node legality per width class drive the search, and an exact commit
//! check with the clearance index decides every track and via written.

pub mod astar;
pub mod geometry;
pub mod negotiate;
mod options;
pub mod rasters;
mod replay;
pub mod route;
pub mod router;
pub mod search;

pub use options::*;
pub use replay::*;

/// `fine_region_plan`: the fine-patch plan the lattice is built from; `None` for "off".
pub fn fine_region_plan(
    board: &copper_pcb::board::PhysicalBoard,
    options: &UnifiedRouterOptions,
) -> crate::unified_error::Result<Option<crate::fine_regions::FineRegionPlan>> {
    if options.fine_regions == "off" {
        return Ok(None);
    }
    let mut planner = options.fine_region_options.clone().unwrap_or_default();
    planner.dense_packages = options.fine_regions == "dense";
    let pitch = options.pitch_nm.filter(|p| *p != 0).unwrap_or(DEFAULT_PITCH_NM);
    crate::fine_regions::plan_fine_regions(board, pitch, Some(&planner)).map(Some)
}

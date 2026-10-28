//! Unified negotiated-congestion detailed router (`pcbir.unified_router`).
//!
//! Every routed net competes for one shared grid. Paths may overlap during
//! search at a price (PathFinder's present and history costs); only fixed
//! copper is immovable. Legality works in two layers: conservative rasters
//! of node legality per width class drive the search, and an exact commit
//! check with the clearance index decides every track and via written.

mod options;

pub use options::*;

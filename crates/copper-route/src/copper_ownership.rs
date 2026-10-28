//! Snapshot-bound occurrence ownership for disposable contact preparations
//! (`pcbir.copper_ownership`): producer evidence, not electrical qualification.

use copper_pcb::board::{PadReference, TrackSegment, Via};

/// `CopperUse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopperUse {
    pub id: String,
    pub kind: String,
    pub terminals: Vec<PadReference>,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
    pub dependencies: Vec<String>,
}

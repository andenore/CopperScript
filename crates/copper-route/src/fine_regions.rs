//! Fine-region plan types (`pcbir.fine_regions`).
// PENDING: owned by rust-m4-unified (the planner itself, `report` and the
// derived properties); only the dataclasses the search lattice reads are here.

use copper_physical::CopperLayer;

/// `FinePatch`: a rectangle of base cells refined by `factor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinePatch {
    pub x0_nm: i64,
    pub y0_nm: i64,
    pub x1_nm: i64,
    pub y1_nm: i64,
    pub factor: i64,
    pub references: Vec<String>,
    pub reason: String,
    pub layers: Option<Vec<CopperLayer>>,
    pub waste_nodes: i64,
}

impl FinePatch {
    pub fn pitch_nm(&self, base_pitch_nm: i64) -> i64 {
        base_pitch_nm.div_euclid(self.factor)
    }
}

/// `FineRegionPlan`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FineRegionPlan {
    pub base_pitch_nm: i64,
    pub patches: Vec<FinePatch>,
    pub base_nodes: i64,
    pub fine_nodes: i64,
    pub uniform_nodes_at_finest: i64,
    pub layer_count: i64,
    pub skipped: Vec<(String, String)>,
}

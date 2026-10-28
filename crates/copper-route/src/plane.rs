//! Declared copper planes: pad-to-plane stitching (`pcbir.plane`).

use std::collections::BTreeSet;

use copper_pcb::board::PadReference;
use copper_physical::CopperKeepout;

use crate::{Error, Result, nm_from_mm};

/// `PlaneStitchOptions`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneStitchOptions {
    pub step_nm: i64,
    pub maximum_radius_nm: i64,
    pub maximum_contact_radius_nm: i64,
    pub maximum_detour_nm: i64,
    pub maze_step_nm: i64,
    pub maze_state_budget: i64,
    pub candidate_bias: Option<String>,
    pub escape_width_nm: Option<i64>,
    pub only_pads: Option<BTreeSet<PadReference>>,
    /// None prefers every GND pad; an empty set restores reuse-first behavior.
    pub preferred_ground_pads: Option<BTreeSet<PadReference>>,
    pub ground_via_in_pad: bool,
    /// Opt-in opposite-side surface zones, e.g. a two-layer rear ground pour.
    /// Same-side-only zones are refill intent, not a reason to invent a via.
    pub include_surface_zones: bool,
    /// Repair mode: a pad inside a same-side pour still needs explicit copper
    /// (a via, or a surface path to an already connected same-net pad or via).
    /// Used after routing, when a later trace may have walled a pad off.
    pub require_explicit: bool,
    /// Keepouts that bind this stitch only, never the board: the launch
    /// corridors beyond macro ports, which a critical pair needs for its own
    /// transition after the stitch has prepared its return contacts.
    pub keepouts: Vec<CopperKeepout>,
}

impl Default for PlaneStitchOptions {
    fn default() -> Self {
        PlaneStitchOptions {
            step_nm: 500_000,
            maximum_radius_nm: nm_from_mm(3),
            maximum_contact_radius_nm: 0,
            maximum_detour_nm: 0,
            maze_step_nm: 100_000,
            maze_state_budget: 12_000,
            candidate_bias: None,
            escape_width_nm: None,
            only_pads: None,
            preferred_ground_pads: None,
            ground_via_in_pad: false,
            include_surface_zones: false,
            require_explicit: false,
            keepouts: Vec::new(),
        }
    }
}

impl PlaneStitchOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if !matches!(self.candidate_bias.as_deref(), None | Some("east" | "south" | "west" | "north")) {
            return Err(Error::Value("plane-stitch candidate bias must be a cardinal direction".to_owned()));
        }
        if self.step_nm <= 0
            || self.maximum_radius_nm < self.step_nm
            || self.maximum_contact_radius_nm < 0
            || self.maximum_detour_nm < 0
            || self.maze_step_nm <= 0
            || self.maze_state_budget <= 0
            || self.escape_width_nm.is_some_and(|width| width < 90_000)
        {
            return Err(Error::Value("plane-stitch search bounds or escape width are invalid".to_owned()));
        }
        Ok(self)
    }
}

/// `PlaneStitchResult` (`copper_uses` are not modelled yet).
#[derive(Clone, Debug)]
pub struct PlaneStitchResult {
    pub board: copper_pcb::board::PhysicalBoard,
    pub stitched_pads: Vec<PadReference>,
    pub pending_pads: Vec<PadReference>,
    pub added_track_count: i64,
    pub added_via_count: i64,
    /// Remote pads of explicitly regional routing reservations remain
    /// ordinary feeder obligations; not a connected-pad inventory.
    pub explicit_feeder_pads: Vec<PadReference>,
}

impl PlaneStitchResult {
    /// `complete`: all local targets have prospective contacts.
    pub fn complete(&self) -> bool {
        self.pending_pads.is_empty()
    }
}

//! Shared exact octilinear terminal access (`pcbir.pin_escape`).

use copper_pcb::board::TrackSegment;
use copper_physical::{CopperLayer, Point};

/// `RoutingAccess`: an owned, layer-specific port reached from an existing pad launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingAccess {
    pub position: Point,
    pub layer: CopperLayer,
    pub launch_position: Point,
    pub path: Vec<TrackSegment>,
}

impl RoutingAccess {
    /// `RoutingAccess.__post_init__`.
    pub fn new(position: Point, layer: CopperLayer, launch_position: Point, path: Vec<TrackSegment>) -> crate::Result<Self> {
        if path.is_empty() {
            return crate::fail("routing access requires an explicit path");
        }
        Ok(RoutingAccess { position, layer, launch_position, path })
    }
}

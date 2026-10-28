//! The global router's result types (`pcbir.routing`), as the unified router
//! and its state files read them.
//!
//! PENDING: owned by `rust-m4-global`; the router itself is not here.

use copper_pcb::board::{PadReference, TrackSegment, Via};
use copper_physical::{CopperLayer, Point};

text_enum!(GlobalRoutingStatus { Success = "success", Overflow = "overflow", Unreachable = "unreachable" });

/// `GridNode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridNode {
    pub layer_index: i64,
    pub x_index: i64,
    pub y_index: i64,
}

/// `PinAccess`.
#[derive(Clone, Debug, PartialEq)]
pub struct PinAccess {
    pub pad: PadReference,
    pub pad_position: Point,
    pub node: GridNode,
    pub access_position: Point,
    pub layer: CopperLayer,
    pub tracks: Vec<TrackSegment>,
    pub via: Option<Via>,
    pub region_only: bool,
    pub retained_via: Option<Via>,
}

/// `GlobalRouteSegment`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRouteSegment {
    pub net: String,
    pub layer: CopperLayer,
    pub start: Point,
    pub end: Point,
    pub guide_half_width_nm: i64,
    pub resource_id: String,
}

/// `GlobalViaProposal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalViaProposal {
    pub net: String,
    pub position: Point,
    pub from_layer: CopperLayer,
    pub to_layer: CopperLayer,
    pub resource_id: String,
}

/// `GlobalNetRoute`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlobalNetRoute {
    pub net: String,
    pub connected: bool,
    pub accesses: Vec<PinAccess>,
    pub segments: Vec<GlobalRouteSegment>,
    pub vias: Vec<GlobalViaProposal>,
    pub length_nm: i64,
    pub diagnostics: Vec<String>,
    pub deferred_to_zone: bool,
}

/// `RoutingHotspot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingHotspot {
    pub resource_id: String,
    pub layer: Option<CopperLayer>,
    pub start: Point,
    pub end: Point,
    pub capacity: i64,
    pub usage: i64,
    pub overflow: i64,
    pub contributors: Vec<String>,
}

/// `GlobalRoutingMetrics`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRoutingMetrics {
    pub routed_net_count: i64,
    pub unrouted_net_count: i64,
    pub total_overflow: i64,
    pub maximum_overflow: i64,
    pub overfull_resource_count: i64,
    pub maximum_utilization_ppm: i64,
    pub total_length_nm: i64,
    pub proposed_via_count: i64,
    pub iterations: i64,
    pub region_only_access_count: i64,
    pub deferred_net_count: i64,
}

/// `GlobalRoutingResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlobalRoutingResult {
    pub status: GlobalRoutingStatus,
    pub routes: Vec<GlobalNetRoute>,
    pub hotspots: Vec<RoutingHotspot>,
    pub metrics: GlobalRoutingMetrics,
    pub placement_fingerprint: String,
    pub routing_fingerprint: String,
    pub fixed_prefix_digest: Option<String>,
}

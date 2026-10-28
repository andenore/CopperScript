//! `pcbir.routing` values of a `copperscript.state/v1` file: the global route
//! saved with critical inputs, decoded field by field as Python's
//! `state_codec.decode` constructs the dataclasses.
//!
//! PENDING: owned by rust-m4-unified (the routing state codec); this is the
//! minimum `copper replay-critical` needs.

use copper_pcb::state_codec::{
    Fields, boolean, enumeration, integer, layer, optional, optional_text, pad_reference, point, sequence, text, texts,
    track, via,
};
use copper_state::StateValue;

use crate::Result;
use crate::routing::{
    GlobalNetRoute, GlobalRouteSegment, GlobalRoutingMetrics, GlobalRoutingResult, GlobalRoutingStatus,
    GlobalViaProposal, GridNode, PinAccess, RoutingHotspot,
};

fn grid_node(value: &StateValue) -> Result<GridNode> {
    let fields = Fields::of(value, "pcbir.routing:GridNode")?;
    fields.allow(&["layer_index", "x_index", "y_index"])?;
    Ok(GridNode {
        layer_index: integer(fields.required("layer_index")?)?,
        x_index: integer(fields.required("x_index")?)?,
        y_index: integer(fields.required("y_index")?)?,
    })
}

fn pin_access(value: &StateValue) -> Result<PinAccess> {
    let fields = Fields::of(value, "pcbir.routing:PinAccess")?;
    fields.allow(&[
        "pad",
        "pad_position",
        "node",
        "access_position",
        "layer",
        "tracks",
        "via",
        "region_only",
        "retained_via",
    ])?;
    Ok(PinAccess {
        pad: pad_reference(fields.required("pad")?)?,
        pad_position: point(fields.required("pad_position")?)?,
        node: grid_node(fields.required("node")?)?,
        access_position: point(fields.required("access_position")?)?,
        layer: layer(fields.required("layer")?)?,
        tracks: sequence(fields.get("tracks"), track)?,
        via: optional(fields.get("via"), via)?,
        region_only: fields.get("region_only").map(boolean).transpose()?.unwrap_or(false),
        retained_via: optional(fields.get("retained_via"), via)?,
    })
}

fn route_segment(value: &StateValue) -> Result<GlobalRouteSegment> {
    let fields = Fields::of(value, "pcbir.routing:GlobalRouteSegment")?;
    fields.allow(&["net", "layer", "start", "end", "guide_half_width_nm", "resource_id"])?;
    Ok(GlobalRouteSegment {
        net: text(fields.required("net")?)?,
        layer: layer(fields.required("layer")?)?,
        start: point(fields.required("start")?)?,
        end: point(fields.required("end")?)?,
        guide_half_width_nm: integer(fields.required("guide_half_width_nm")?)?,
        resource_id: text(fields.required("resource_id")?)?,
    })
}

fn via_proposal(value: &StateValue) -> Result<GlobalViaProposal> {
    let fields = Fields::of(value, "pcbir.routing:GlobalViaProposal")?;
    fields.allow(&["net", "position", "from_layer", "to_layer", "resource_id"])?;
    Ok(GlobalViaProposal {
        net: text(fields.required("net")?)?,
        position: point(fields.required("position")?)?,
        from_layer: layer(fields.required("from_layer")?)?,
        to_layer: layer(fields.required("to_layer")?)?,
        resource_id: text(fields.required("resource_id")?)?,
    })
}

fn net_route(value: &StateValue) -> Result<GlobalNetRoute> {
    let fields = Fields::of(value, "pcbir.routing:GlobalNetRoute")?;
    fields.allow(&[
        "net",
        "connected",
        "accesses",
        "segments",
        "vias",
        "length_nm",
        "diagnostics",
        "deferred_to_zone",
    ])?;
    Ok(GlobalNetRoute {
        net: text(fields.required("net")?)?,
        connected: boolean(fields.required("connected")?)?,
        accesses: sequence(Some(fields.required("accesses")?), pin_access)?,
        segments: sequence(Some(fields.required("segments")?), route_segment)?,
        vias: sequence(Some(fields.required("vias")?), via_proposal)?,
        length_nm: integer(fields.required("length_nm")?)?,
        diagnostics: texts(fields.get("diagnostics"))?,
        deferred_to_zone: fields.get("deferred_to_zone").map(boolean).transpose()?.unwrap_or(false),
    })
}

fn hotspot(value: &StateValue) -> Result<RoutingHotspot> {
    let fields = Fields::of(value, "pcbir.routing:RoutingHotspot")?;
    fields.allow(&["resource_id", "layer", "start", "end", "capacity", "usage", "overflow", "contributors"])?;
    Ok(RoutingHotspot {
        resource_id: text(fields.required("resource_id")?)?,
        layer: optional(Some(fields.required("layer")?), layer)?,
        start: point(fields.required("start")?)?,
        end: point(fields.required("end")?)?,
        capacity: integer(fields.required("capacity")?)?,
        usage: integer(fields.required("usage")?)?,
        overflow: integer(fields.required("overflow")?)?,
        contributors: texts(Some(fields.required("contributors")?))?,
    })
}

fn metrics(value: &StateValue) -> Result<GlobalRoutingMetrics> {
    let fields = Fields::of(value, "pcbir.routing:GlobalRoutingMetrics")?;
    fields.allow(&[
        "routed_net_count",
        "unrouted_net_count",
        "total_overflow",
        "maximum_overflow",
        "overfull_resource_count",
        "maximum_utilization_ppm",
        "total_length_nm",
        "proposed_via_count",
        "iterations",
        "region_only_access_count",
        "deferred_net_count",
    ])?;
    let count = |name: &str| -> Result<i64> { integer(fields.required(name)?) };
    let defaulted =
        |name: &str| -> Result<i64> { fields.get(name).map(integer).transpose().map(Option::unwrap_or_default) };
    Ok(GlobalRoutingMetrics {
        routed_net_count: count("routed_net_count")?,
        unrouted_net_count: count("unrouted_net_count")?,
        total_overflow: count("total_overflow")?,
        maximum_overflow: count("maximum_overflow")?,
        overfull_resource_count: count("overfull_resource_count")?,
        maximum_utilization_ppm: count("maximum_utilization_ppm")?,
        total_length_nm: count("total_length_nm")?,
        proposed_via_count: count("proposed_via_count")?,
        iterations: count("iterations")?,
        region_only_access_count: defaulted("region_only_access_count")?,
        deferred_net_count: defaulted("deferred_net_count")?,
    })
}

fn status(value: &str) -> Option<GlobalRoutingStatus> {
    match value {
        "success" => Some(GlobalRoutingStatus::Success),
        "overflow" => Some(GlobalRoutingStatus::Overflow),
        "unreachable" => Some(GlobalRoutingStatus::Unreachable),
        _ => None,
    }
}

/// `pcbir.routing:GlobalRoutingResult` from its saved fields.
pub fn global_routing_result(value: &StateValue) -> Result<GlobalRoutingResult> {
    let fields = Fields::of(value, "pcbir.routing:GlobalRoutingResult")?;
    fields.allow(&[
        "status",
        "routes",
        "hotspots",
        "metrics",
        "placement_fingerprint",
        "routing_fingerprint",
        "fixed_prefix_digest",
    ])?;
    Ok(GlobalRoutingResult {
        status: enumeration(fields.required("status")?, "GlobalRoutingStatus", status)?,
        routes: sequence(Some(fields.required("routes")?), net_route)?,
        hotspots: sequence(Some(fields.required("hotspots")?), hotspot)?,
        metrics: metrics(fields.required("metrics")?)?,
        placement_fingerprint: text(fields.required("placement_fingerprint")?)?,
        routing_fingerprint: text(fields.required("routing_fingerprint")?)?,
        fixed_prefix_digest: optional_text(fields.get("fixed_prefix_digest"))?,
    })
}

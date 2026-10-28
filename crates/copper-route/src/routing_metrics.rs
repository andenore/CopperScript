//! Read-only final-copper measurements (`pcbir.routing_metrics`); no inferred
//! electrical acceptance limits.

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_drc::digest::physical_board_digest;
use copper_json::{Object, Value};
use copper_pcb::board::{PhysicalNet, TrackSegment};
use copper_pcb::transform::transformed_pad_position;
use copper_pcb::Result;
use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;
use copper_physical::BoardSide;
use indexmap::IndexSet;

use crate::plane_verify::PlaneVerification;

/// `isqrt(dx*dx + dy*dy)`.
pub fn isqrt(value: i128) -> i64 {
    if value <= 0 {
        return 0;
    }
    let mut root = (value as f64).sqrt() as i128;
    while root * root > value {
        root -= 1;
    }
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    root as i64
}

/// `track_length_nm`: integer-nanometre floor of segment length, consistent across reports.
pub fn track_length_nm(track: &TrackSegment) -> i64 {
    let dx = i128::from(track.end.x_nm - track.start.x_nm);
    let dy = i128::from(track.end.y_nm - track.start.y_nm);
    isqrt(dx * dx + dy * dy)
}

/// `routing_quality(board, native=None)`.
pub fn routing_quality(board: &PhysicalBoard, native: Option<&PlaneVerification>) -> Result<Value> {
    let digest = physical_board_digest(board);
    let graph = explicit_copper_connectivity(board, Candidate::default())?;
    let (mut straight, mut diagonal, mut other) = (0i64, 0i64, 0i64);
    let mut layers: Object =
        board.stackup.copper_layers.iter().map(|layer| (layer.value().to_owned(), Value::from(0i64))).collect();
    let mut nets = Object::new();
    let mut sorted_nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    sorted_nets.sort_by(|a, b| a.name.cmp(&b.name));
    for net in sorted_nets {
        let tracks: Vec<&TrackSegment> = board.tracks.iter().filter(|t| t.net == net.name).collect();
        let mut entry = Object::new();
        entry.insert("track_length_nm".to_owned(), Value::from(tracks.iter().map(|t| track_length_nm(t)).sum::<i64>()));
        entry.insert("track_count".to_owned(), Value::from(tracks.len() as i64));
        entry.insert("via_count".to_owned(), Value::from(board.vias.iter().filter(|v| v.net == net.name).count() as i64));
        entry.insert("explicit_copper_connected".to_owned(), Value::from(graph.net_connected(net)));
        nets.insert(net.name.clone(), Value::Object(entry));
    }
    for track in &board.tracks {
        let dx = (track.end.x_nm - track.start.x_nm).abs();
        let dy = (track.end.y_nm - track.start.y_nm).abs();
        let length = track_length_nm(track);
        if dx == 0 || dy == 0 {
            straight += length;
        } else if dx == dy {
            diagonal += length;
        } else {
            other += length;
        }
        let key = track.layer.value();
        let current = layers.get(key).and_then(as_i64).unwrap_or(0);
        layers.insert(key.to_owned(), Value::from(current + length));
    }
    let poses: indexmap::IndexMap<&str, &copper_pcb::board::Placement> =
        board.placements.iter().map(|pose| (pose.reference.as_str(), pose)).collect();
    let mut surface_graphs: Vec<((String, CopperLayer), copper_drc::connectivity::Connectivity)> = Vec::new();
    let mut bypasses = Vec::new();
    let mut links: Vec<&copper_pcb::board::DecouplingLink> = board.decoupling_links.iter().collect();
    links.sort_by(|a, b| a.capacitor.cmp(&b.capacitor));
    for link in links {
        let cap = poses[link.capacitor.component.as_str()];
        let target = poses[link.target.component.as_str()];
        let layer = if cap.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let key = (link.net.clone(), layer);
        let filtered: Vec<TrackSegment> =
            board.tracks.iter().filter(|t| t.net == link.net && t.layer == layer).cloned().collect();
        if !surface_graphs.iter().any(|(k, _)| *k == key) {
            let only: IndexSet<String> = IndexSet::from([link.net.clone()]);
            let candidate = Candidate {
                only_nets: Some(&only),
                exclude_internal_connections: true,
                tracks: Some(&filtered),
                vias: Some(&[]),
                polygons: Some(&[]),
            };
            surface_graphs.push((key.clone(), explicit_copper_connectivity(board, candidate)?));
        }
        let surface = &surface_graphs.iter().find(|(k, _)| *k == key).expect("built above").1;
        let connected = cap.side == target.side
            && surface.net_connected(&PhysicalNet {
                name: link.net.clone(),
                pads: vec![link.capacitor.clone(), link.target.clone()],
            });
        let a = transformed_pad_position(board, cap, &link.capacitor.pad).map_err(copper_pcb::Error::Value)?;
        let b = transformed_pad_position(board, target, &link.target.pad).map_err(copper_pcb::Error::Value)?;
        let roots: IndexSet<&str> = surface
            .pad_nodes
            .get(&link.capacitor)
            .map(|nodes| nodes.iter().map(|node| surface.root(node)).collect())
            .unwrap_or_default();
        // The graph indices refer to the filtered track inventory, not board indices.
        let component_length: i64 = filtered
            .iter()
            .enumerate()
            .filter(|(index, _)| surface.roots.get(&format!("track:{index}")).is_some_and(|r| roots.contains(r.as_str())))
            .map(|(_, t)| track_length_nm(t))
            .sum();
        let dx = i128::from(a.x_nm - b.x_nm);
        let dy = i128::from(a.y_nm - b.y_nm);
        let mut entry = Object::new();
        entry.insert("capacitor".to_owned(), Value::from(format!("{}.{}", link.capacitor.component, link.capacitor.pad)));
        entry.insert("target".to_owned(), Value::from(format!("{}.{}", link.target.component, link.target.pad)));
        entry.insert("net".to_owned(), Value::from(link.net.as_str()));
        entry.insert("layer".to_owned(), Value::from(layer.value()));
        entry.insert("surface_connected".to_owned(), Value::from(connected));
        entry.insert("pad_distance_nm".to_owned(), Value::from(isqrt(dx * dx + dy * dy)));
        entry.insert("surface_component_track_length_nm".to_owned(), Value::from(component_length));
        entry.insert(
            "length_scope".to_owned(),
            Value::from("entire connected surface track component, including branches; not shortest path"),
        );
        bypasses.push(Value::Object(entry));
    }
    let native_current = match native {
        Some(native) => native.matches(board)?,
        None => false,
    };
    let mut references = Vec::new();
    let mut rules: Vec<&copper_pcb::board::NetRoutingRule> = board.net_routing_rules.iter().collect();
    rules.sort_by(|a, b| a.net.cmp(&b.net));
    for rule in rules {
        let kinds = ["rf_feed", "differential", "clock", "can_bus"];
        if !kinds.contains(&rule.kind.value()) && rule.shared_reference_layer.is_none() {
            continue;
        }
        if !native_current || rule.shared_reference_layer.is_none() || rule.return_via_net.is_none() {
            let mut entry = Object::new();
            entry.insert("net".to_owned(), Value::from(rule.net.as_str()));
            entry.insert("status".to_owned(), Value::from("incomplete"));
            entry.insert(
                "reason".to_owned(),
                Value::from("requires current native fill and an explicit reference net/layer"),
            );
            references.push(Value::Object(entry));
            continue;
        }
        // `rf_geometry.reference_coverage` belongs to the independent RF checks
        // that stay on the Python CLI (plan decision D4).
        return copper_pcb::not_implemented("reference coverage of a natively filled board (rf_geometry)");
    }
    let mut angles = Object::new();
    angles.insert("straight_nm".to_owned(), Value::from(straight));
    angles.insert("diagonal_45_nm".to_owned(), Value::from(diagonal));
    angles.insert("other_angle_nm".to_owned(), Value::from(other));
    let total: i64 = layers.values().filter_map(as_i64).sum();
    let mut document = Object::new();
    document.insert("schema".to_owned(), Value::from("copperscript-routing-quality/v0.1"));
    document.insert("board_digest".to_owned(), Value::from(digest));
    document.insert(
        "scope".to_owned(),
        Value::from("explicit track geometry and surface connectivity; no impedance, thermal or production qualification"),
    );
    document.insert("length_method".to_owned(), Value::from("sum of integer-nanometre segment lengths rounded down"));
    document.insert("track_count".to_owned(), Value::from(board.tracks.len() as i64));
    document.insert("via_count".to_owned(), Value::from(board.vias.len() as i64));
    document.insert("track_length_nm".to_owned(), Value::from(total));
    document.insert("layer_length_nm".to_owned(), Value::Object(layers));
    document.insert("angle_length_nm".to_owned(), Value::Object(angles));
    document.insert("nets".to_owned(), Value::Object(nets));
    document.insert("decoupling".to_owned(), Value::Array(bypasses));
    document.insert("reference_coverage".to_owned(), Value::Array(references));
    document.insert("native_fill_current".to_owned(), Value::from(native_current));
    document.insert(
        "native_report_digest".to_owned(),
        match native {
            Some(native) if native_current => Value::from(native.report_digest.as_str()),
            _ => Value::Null,
        },
    );
    Ok(Value::Object(document))
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_square_roots_floor() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(2), 1);
        assert_eq!(isqrt(10_000_000_000_000_000), 100_000_000);
        assert_eq!(isqrt(10_000_000_000_000_001), 100_000_000);
        assert_eq!(isqrt(9_999_999_999_999_999), 99_999_999);
    }
}

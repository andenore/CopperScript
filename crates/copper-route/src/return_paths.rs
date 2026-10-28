//! Conservative declared shared-reference topology (`pcbir.return_paths`):
//! the single-ended return-via checks launch planning uses.

use indexmap::IndexSet;

use copper_pcb::Result;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment, Via};
use copper_physical::geometry::{point_on_segment, segment_in_polygon, shapes_clear};
use copper_physical::{CopperLayer, Point, RoundedConvexShape};

/// `return_span_covers`: the actual source barrel span covers `first..last`.
pub fn return_span_covers(board: &PhysicalBoard, returned: &Via, first: CopperLayer, last: CopperLayer) -> bool {
    let layers = &board.stackup.copper_layers;
    let index = |layer: CopperLayer| layers.iter().position(|item| *item == layer);
    let (Some(f), Some(l), Some(x), Some(y)) =
        (index(first), index(last), index(returned.from_layer), index(returned.to_layer))
    else {
        return false;
    };
    x.min(y) <= f.min(l) && x.max(y) >= f.max(l)
}

/// `covering_return_vias`: exact nearby existing return barrels.
pub fn covering_return_vias<'a>(board: &'a PhysicalBoard, rule: &NetRoutingRule, signal: &Via) -> Vec<&'a Via> {
    let Some(limit) = rule.maximum_return_via_distance_nm else { return Vec::new() };
    if !rule.require_return_vias || rule.return_via_net.as_deref() == Some(signal.net.as_str()) {
        return Vec::new();
    }
    let limit = i128::from(limit) * i128::from(limit);
    board
        .vias
        .iter()
        .filter(|via| {
            Some(via.net.as_str()) == rule.return_via_net.as_deref()
                && return_span_covers(board, via, signal.from_layer, signal.to_layer)
                && {
                    let (dx, dy) = (
                        i128::from(via.position.x_nm - signal.position.x_nm),
                        i128::from(via.position.y_nm - signal.position.y_nm),
                    );
                    dx * dx + dy * dy <= limit
                }
        })
        .collect()
}

/// `existing_returns_permit`: the single-ended requirement used by launch planning and exact commit.
pub fn existing_returns_permit(board: &PhysicalBoard, rule: Option<&NetRoutingRule>, signal: &Via) -> bool {
    match rule {
        None => true,
        Some(rule) => {
            !rule.require_return_vias
                || rule.differential_partner.is_some()
                || !covering_return_vias(board, rule, signal).is_empty()
        }
    }
}

/// `shared_reference_plane`: only explicit, matching
/// adjacent-layer declarations can omit a return via.
pub fn shared_reference_plane(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    signal_layers: &[CopperLayer],
    points: &[Point],
) -> Result<Option<CopperLayer>> {
    use copper_pcb::board::ReturnViaPolicy;
    if ![first, second].iter().all(|r| r.require_return_vias && r.return_via_policy == ReturnViaPolicy::ReferenceChange)
    {
        return Ok(None);
    }
    let Some(layer) = first.shared_reference_layer else { return Ok(None) };
    let distinct: IndexSet<&CopperLayer> = signal_layers.iter().collect();
    if Some(layer) != second.shared_reference_layer
        || first.return_via_net != second.return_via_net
        || distinct.len() != 2
    {
        return Ok(None);
    }
    let layers = &board.stackup.copper_layers;
    let Some(reference) = layers.iter().position(|l| *l == layer) else { return Ok(None) };
    for value in signal_layers {
        let Some(at) = layers.iter().position(|l| l == value) else { return Ok(None) };
        if at.abs_diff(reference) != 1 {
            return Ok(None);
        }
    }
    if copper_pcb::zones::dedicated_plane_layers(board).get(&layer) != first.return_via_net.as_ref() {
        return Ok(None);
    }
    let inside = |point: &Point, vertices: &[Point]| copper_physical::geometry::point_in_polygon(*point, vertices);
    let covered = board
        .zones
        .iter()
        .filter(|z| z.layers.contains(&layer) && Some(&z.net) == first.return_via_net.as_ref())
        .any(|zone| zone.holes.is_empty() && points.iter().all(|p| inside(p, &zone.outer)));
    if !covered {
        return Ok(None);
    }
    for keepout in copper_pcb::keepouts::resolved_copper_keepouts(board)? {
        if keepout.block_zones && keepout.layers.contains(&layer) && points.iter().any(|p| inside(p, &keepout.vertices))
        {
            return Ok(None);
        }
    }
    Ok(Some(layer))
}

/// `transition_contact_layers`: the actual centred signal contacts of a via,
/// not its through-barrel endpoints.
pub fn transition_contact_layers(tracks: &[TrackSegment], via: &Via) -> Vec<CopperLayer> {
    let layers: IndexSet<CopperLayer> = tracks
        .iter()
        .filter(|t| t.net == via.net && point_on_segment(via.position, t.start, t.end))
        .map(|t| t.layer)
        .collect();
    let mut layers: Vec<CopperLayer> = layers.into_iter().collect();
    layers.sort_by_key(|layer| layer.value());
    layers
}

/// `pair_reference_intent_covers`: one unperforated declared zone covers the
/// complete pair, clear of every footprint-local zone keepout.
pub fn pair_reference_intent_covers(
    board: &PhysicalBoard,
    reference: CopperLayer,
    net: Option<&str>,
    tracks: &[TrackSegment],
) -> Result<bool> {
    if tracks.is_empty() {
        return Ok(false);
    }
    let layers = &board.stackup.copper_layers;
    let Some(at) = layers.iter().position(|l| *l == reference) else { return Ok(false) };
    for track in tracks {
        let Some(index) = layers.iter().position(|l| *l == track.layer) else { return Ok(false) };
        if index.abs_diff(at) != 1 {
            return Ok(false);
        }
    }
    let envelope = |track: &TrackSegment| {
        RoundedConvexShape::new(vec![track.start, track.end], (track.width_nm + 1).div_euclid(2))
    };
    let within_zone = |track: &TrackSegment, vertices: &[Point]| -> Result<bool> {
        if !segment_in_polygon(track.start, track.end, vertices) {
            return Ok(false);
        }
        let shape = envelope(track)?;
        for (index, start) in vertices.iter().enumerate() {
            let end = vertices[(index + 1) % vertices.len()];
            if !shapes_clear(&shape, &RoundedConvexShape::new(vec![*start, end], 0)?, 0) {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let mut covered = false;
    for zone in &board.zones {
        if Some(zone.net.as_str()) != net || !zone.layers.contains(&reference) || !zone.holes.is_empty() {
            continue;
        }
        let mut all = true;
        for track in tracks {
            if !within_zone(track, &zone.outer)? {
                all = false;
                break;
            }
        }
        if all {
            covered = true;
            break;
        }
    }
    if !covered {
        return Ok(false);
    }
    for keepout in copper_pcb::keepouts::resolved_copper_keepouts(board)? {
        if !keepout.block_zones || !keepout.layers.contains(&reference) {
            continue;
        }
        let blocked = RoundedConvexShape::new(keepout.vertices.clone(), 0)?;
        for track in tracks {
            if !shapes_clear(&envelope(track)?, &blocked, 0) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

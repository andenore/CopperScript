//! Report-only review of the exported critical copper (`pcbir.critical_review`).
//!
//! Per net: the lane-table row, the least edge-to-edge spacing to other
//! nets' copper (inside or outside the net's breakout regions, against
//! critical or ordinary signal nets), the length coupled to other critical
//! pairs, and the bends. Per pair: the intra-pair skew; per `length_match`
//! group: its skew and tuning outcome. The review never changes copper.

use std::f64::consts::PI;

use indexmap::{IndexMap, IndexSet};
use num_traits::ToPrimitive;

use copper_json::{Object, Value};
use copper_pcb::board::{PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::geometry::{
    RoundedConvexShape, point_in_polygon, segments_intersect, shape_distance_squared, shapes_clear,
};
use copper_physical::pyfloat::{hypot, round_digits, round_int};
use copper_physical::{CopperLayer, Point};

use crate::Result;
use crate::critical::{CriticalNetResult, critical_lane_table};
use crate::routing_clearance::{CopperObject, RoutingClearanceIndex};

pub const LANE_REVIEW_RADIUS_NM: i64 = 1_000_000;
pub const COUPLING_FACTOR: i64 = 2;
pub const SHARP_BEND_DEGREES: i64 = 45;

const CLASSES: [&str; 2] = ["critical", "signal"];

fn is_pair_kind(kind: RouteKind) -> bool {
    matches!(kind, RouteKind::Differential | RouteKind::CanBus)
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> Result<RoundedConvexShape> {
    RoundedConvexShape::new(spine, radius_nm).map_err(copper_pcb::Error::Value)
}

/// `critical_lane_review(board, results, match_tuning)`: JSON-ready review of
/// the critical copper on `board`. `match_tuning` holds the critical stage's
/// tuning documents (their `id` and `status`).
pub fn critical_lane_review(
    board: &PhysicalBoard,
    results: &[CriticalNetResult],
    match_tuning: &[Value],
) -> Result<Object> {
    let lanes = critical_lane_table(board, results)?;
    let rules: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let critical: IndexSet<&str> =
        rules.iter().filter(|(_, rule)| rule.kind != RouteKind::General).map(|(net, _)| *net).collect();
    let zone_nets: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    let partner_of = |net: &str| -> Option<String> {
        let rule = rules.get(net)?;
        let partner = rule.differential_partner.as_deref()?;
        if !is_pair_kind(rule.kind) {
            return None;
        }
        let other = rules.get(partner)?;
        (other.differential_partner.as_deref() == Some(net)).then(|| partner.to_owned())
    };
    let paired: IndexSet<&str> = rules.keys().copied().filter(|net| partner_of(net).is_some()).collect();
    let mut nets: Vec<Value> = Vec::new();
    if !lanes.is_empty() {
        let index = RoutingClearanceIndex::new(board)?;
        let order = &board.stackup.copper_layers;
        let reviewed: IndexSet<&str> = lanes.iter().filter_map(|lane| lane_text(lane, "net")).collect();
        let mut tracks: IndexMap<&str, Vec<&TrackSegment>> = IndexMap::new();
        for track in &board.tracks {
            if reviewed.contains(track.net.as_str()) {
                tracks.entry(track.net.as_str()).or_default().push(track);
            }
        }
        let mut copper: IndexMap<&str, Vec<(RoundedConvexShape, Vec<CopperLayer>)>> = IndexMap::new();
        for (net, items) in &tracks {
            let mut shapes = Vec::new();
            for track in items {
                shapes.push((shape(vec![track.start, track.end], track.width_nm / 2)?, vec![track.layer]));
            }
            copper.insert(net, shapes);
        }
        for via in &board.vias {
            if reviewed.contains(via.net.as_str()) {
                let position = |layer: CopperLayer| {
                    order.iter().position(|item| *item == layer).ok_or_else(|| {
                        copper_pcb::Error::Value(format!(
                            "{} is not in list",
                            copper_json::python_str_repr(layer.value())
                        ))
                    })
                };
                let (a, b) = (position(via.from_layer)?, position(via.to_layer)?);
                let (first, last) = (a.min(b), a.max(b));
                copper
                    .entry(via.net.as_str())
                    .or_default()
                    .push((shape(vec![via.position], via.size_nm / 2)?, order[first..=last].to_vec()));
            }
        }
        let empty_tracks: Vec<&TrackSegment> = Vec::new();
        let empty_copper: Vec<(RoundedConvexShape, Vec<CopperLayer>)> = Vec::new();
        for lane in &lanes {
            let net = lane_text(lane, "net").unwrap_or_default();
            let partner = partner_of(net);
            let own_tracks = tracks.get(net).unwrap_or(&empty_tracks);
            let own_copper = copper.get(net).unwrap_or(&empty_copper);
            let outside: Vec<(&TrackSegment, &RoundedConvexShape)> = own_tracks
                .iter()
                .zip(own_copper.iter())
                .filter(|(_, (shape, _))| index.breakout.region(net, &shape.spine).is_none())
                .map(|(track, (shape, _))| (*track, shape))
                .collect();
            let gap = partner.as_ref().and_then(|_| rules[net].pair_gap_nm);
            let threshold = gap.filter(|gap| *gap != 0).map(|gap| COUPLING_FACTOR * gap);
            let (coupled, coupled_nets) = match threshold {
                Some(threshold) => {
                    let neighbours: IndexSet<&str> = paired
                        .iter()
                        .copied()
                        .filter(|name| *name != net && Some(*name) != partner.as_deref())
                        .collect();
                    let (length, names) = coupled_length(&index, &outside, &neighbours, threshold);
                    (Value::from(length), names)
                }
                None => (Value::Null, Vec::new()),
            };
            let mut ignored: IndexSet<&str> = zone_nets.clone();
            ignored.insert(net);
            if let Some(partner) = &partner {
                ignored.insert(partner.as_str());
            }
            let Value::Object(mut entry) = lane.clone() else { unreachable!("lane rows are objects") };
            entry.insert("partner".to_owned(), partner.as_deref().map_or(Value::Null, Value::from));
            entry.insert("breakout".to_owned(), Value::Bool(index.breakout.declared.contains(net)));
            entry.insert("spacing".to_owned(), Value::Object(spacing(&index, net, own_copper, &ignored, &critical)?));
            entry.insert("coupling_threshold_nm".to_owned(), threshold.map_or(Value::Null, Value::from));
            entry.insert("coupled_length_nm".to_owned(), coupled);
            entry.insert("coupled_nets".to_owned(), Value::Array(coupled_nets.into_iter().map(Value::from).collect()));
            entry.insert("bends".to_owned(), Value::Object(bends(own_tracks)));
            nets.push(Value::Object(entry));
        }
    }
    let lengths: IndexMap<&str, i64> =
        lanes.iter().filter_map(|lane| Some((lane_text(lane, "net")?, lane_int(lane, "routed_length_nm")?))).collect();
    let mut pairs = Vec::new();
    for item in results {
        if item.nets.len() != 2 || partner_of(&item.nets[0]).as_deref() != Some(item.nets[1].as_str()) {
            continue;
        }
        let limit = item.nets.iter().filter_map(|net| rules[net.as_str()].max_skew_nm).min();
        let length =
            |net: &str| lengths.get(net).copied().ok_or_else(|| copper_pcb::Error::Value(format!("KeyError: '{net}'")));
        let (first, second) = (length(&item.nets[0])?, length(&item.nets[1])?);
        let skew = (first - second).abs();
        let status = if !item.connected {
            "incomplete"
        } else {
            match limit {
                None => "no_limit",
                Some(limit) if skew <= limit => "pass",
                Some(_) => "fail",
            }
        };
        let mut pair = Object::new();
        pair.insert("nets".to_owned(), Value::Array(item.nets.iter().map(|n| Value::from(n.as_str())).collect()));
        pair.insert("lengths_nm".to_owned(), Value::Array(vec![Value::from(first), Value::from(second)]));
        pair.insert("skew_nm".to_owned(), Value::from(skew));
        pair.insert("max_skew_nm".to_owned(), limit.map_or(Value::Null, Value::from));
        pair.insert("status".to_owned(), Value::from(status));
        pairs.push(Value::Object(pair));
    }
    let mut tuning: IndexMap<String, Value> = IndexMap::new();
    for item in match_tuning {
        if let Value::Object(document) = item
            && let Some(Value::Str(id)) = document.get("id")
        {
            tuning.insert(id.clone(), document.get("status").cloned().unwrap_or(Value::Null));
        }
    }
    let mut groups = Vec::new();
    for check in copper_drc::signal_integrity::verify_match_groups(board)? {
        let Value::Object(mut document) = check.to_value() else { unreachable!("match groups are objects") };
        document.insert("tuning_status".to_owned(), tuning.get(&check.id).cloned().unwrap_or(Value::Null));
        groups.push(Value::Object(document));
    }
    let mut zone_list: Vec<&str> = zone_nets.iter().copied().collect();
    zone_list.sort_unstable();
    let mut review = Object::new();
    review.insert("search_radius_nm".to_owned(), Value::from(LANE_REVIEW_RADIUS_NM));
    review.insert("coupling_factor".to_owned(), Value::from(COUPLING_FACTOR));
    review.insert("sharp_bend_degrees".to_owned(), Value::from(SHARP_BEND_DEGREES));
    review.insert("ignored_zone_nets".to_owned(), Value::Array(zone_list.into_iter().map(Value::from).collect()));
    review.insert("nets".to_owned(), Value::Array(nets));
    review.insert("pairs".to_owned(), Value::Array(pairs));
    review.insert("match_groups".to_owned(), Value::Array(groups));
    Ok(review)
}

fn lane_text<'a>(lane: &'a Value, key: &str) -> Option<&'a str> {
    match lane {
        Value::Object(row) => row.get(key).and_then(Value::as_str),
        _ => None,
    }
}

fn lane_int(lane: &Value, key: &str) -> Option<i64> {
    match lane {
        Value::Object(row) => match row.get(key) {
            Some(Value::Int(number)) => number.to_i64(),
            _ => None,
        },
        _ => None,
    }
}

/// `_spacing`: least edge-to-edge distance per own-copper region and neighbour class.
fn spacing(
    index: &RoutingClearanceIndex<'_>,
    net: &str,
    own: &[(RoundedConvexShape, Vec<CopperLayer>)],
    ignored: &IndexSet<&str>,
    critical: &IndexSet<&str>,
) -> Result<Object> {
    let mut best: IndexMap<(&str, &str), (f64, &CopperObject, &RoundedConvexShape, &[CopperLayer])> = IndexMap::new();
    for (shape, layers) in own {
        let region =
            if index.breakout.region(net, &shape.spine).is_none() { "outside_breakout" } else { "inside_breakout" };
        for other in index.nearby_copper(shape, layers, LANE_REVIEW_RADIUS_NM) {
            if ignored.contains(other.net.as_str()) || other.net.starts_with('<') {
                continue;
            }
            let key = (region, if critical.contains(other.net.as_str()) { "critical" } else { "signal" });
            let limit = best.get(&key).map_or(LANE_REVIEW_RADIUS_NM as f64, |entry| entry.0);
            // Exact integer pruning first; only a possibly nearer object is measured.
            if shapes_clear(shape, &other.shape, limit.ceil() as i64) {
                continue;
            }
            let squared = shape_distance_squared(shape, &other.shape).to_f64().unwrap_or(f64::INFINITY);
            let distance = squared.sqrt() - shape.radius_nm as f64 - other.shape.radius_nm as f64;
            if distance < limit {
                best.insert(key, (distance, other, shape, layers));
            }
        }
    }
    let mut document = Object::new();
    for region in ["inside_breakout", "outside_breakout"] {
        let mut classes = Object::new();
        for kind in CLASSES {
            let Some((distance, other, shape, layers)) = best.get(&(region, kind)) else {
                classes.insert(kind.to_owned(), Value::Null);
                continue;
            };
            let pad = if other.is_pad { other.pad_reference.as_ref() } else { None };
            let object = if other.is_pad {
                "pad"
            } else if other.shape.spine.len() == 1 {
                "via"
            } else {
                "track"
            };
            let layer = layers.iter().find(|layer| other.layers.contains(layer)).expect("a shared layer");
            let at = nearest_point(&shape.spine, &other.shape.spine);
            let mut entry = Object::new();
            entry.insert("distance_nm".to_owned(), Value::from(round_int(*distance).max(0)));
            entry.insert("neighbour".to_owned(), Value::from(other.net.as_str()));
            entry.insert("object".to_owned(), Value::from(object));
            entry.insert(
                "pad".to_owned(),
                pad.map_or(Value::Null, |pad| Value::from(format!("{}.{}", pad.component, pad.pad))),
            );
            entry.insert("layer".to_owned(), Value::from(layer.value()));
            entry.insert("at_nm".to_owned(), Value::Array(vec![Value::from(at.0), Value::from(at.1)]));
            classes.insert(kind.to_owned(), Value::Object(entry));
        }
        document.insert(region.to_owned(), Value::Object(classes));
    }
    Ok(document)
}

/// `_coupled_length`: length of `tracks` closer than `threshold_nm` (edge to
/// edge) to copper of `neighbours`, and those neighbours sorted.
fn coupled_length(
    index: &RoutingClearanceIndex<'_>,
    tracks: &[(&TrackSegment, &RoundedConvexShape)],
    neighbours: &IndexSet<&str>,
    threshold_nm: i64,
) -> (i64, Vec<String>) {
    let mut total = 0.0;
    let mut nets: IndexSet<String> = IndexSet::new();
    for (track, shape) in tracks {
        let mut intervals: Vec<(f64, f64)> = Vec::new();
        for other in index.nearby_copper(shape, &[track.layer], threshold_nm) {
            if !neighbours.contains(other.net.as_str()) || shapes_clear(shape, &other.shape, threshold_nm) {
                continue;
            }
            let reach = (threshold_nm + shape.radius_nm + other.shape.radius_nm) as f64;
            if let Some(interval) = reach_interval(track.start, track.end, &other.shape.spine, reach) {
                intervals.push(interval);
                nets.insert(other.net.clone());
            }
        }
        intervals.sort_by(|a, b| a.partial_cmp(b).expect("finite intervals"));
        let mut reached = f64::NEG_INFINITY;
        for (low, high) in intervals {
            if high > reached {
                total += high - low.max(reached);
                reached = high;
            }
        }
    }
    let mut nets: Vec<String> = nets.into_iter().collect();
    nets.sort();
    (round_int(total), nets)
}

/// `_bends`: direction changes where exactly two tracks meet on one layer.
fn bends(tracks: &[&TrackSegment]) -> Object {
    let mut far_ends: IndexMap<(CopperLayer, Point), Vec<Point>> = IndexMap::new();
    for track in tracks {
        if track.start != track.end {
            far_ends.entry((track.layer, track.start)).or_default().push(track.end);
            far_ends.entry((track.layer, track.end)).or_default().push(track.start);
        }
    }
    let mut sharpest: Option<f64> = None;
    let mut sharp: Vec<Value> = Vec::new();
    for ((layer, node), ends) in &far_ends {
        if ends.len() != 2 {
            continue;
        }
        let (ux, uy) = ((node.x_nm - ends[0].x_nm) as i128, (node.y_nm - ends[0].y_nm) as i128);
        let (vx, vy) = ((ends[1].x_nm - node.x_nm) as i128, (ends[1].y_nm - node.y_nm) as i128);
        let cross = (ux * vy - uy * vx).abs() as f64;
        let dot = (ux * vx + uy * vy) as f64;
        let angle = round_digits(cross.atan2(dot) / (PI / 180.0), 2);
        sharpest = Some(sharpest.map_or(angle, |value| value.max(angle)));
        if angle > SHARP_BEND_DEGREES as f64 {
            let mut entry = Object::new();
            entry.insert("at_nm".to_owned(), Value::Array(vec![Value::from(node.x_nm), Value::from(node.y_nm)]));
            entry.insert("layer".to_owned(), Value::from(layer.value()));
            entry.insert("degrees".to_owned(), Value::Float(angle));
            sharp.push(Value::Object(entry));
        }
    }
    let mut document = Object::new();
    document.insert("sharpest_degrees".to_owned(), sharpest.map_or(Value::Null, Value::Float));
    document.insert("sharp_count".to_owned(), Value::from(sharp.len() as i64));
    document.insert("sharp".to_owned(), Value::Array(sharp));
    document
}

fn edges(spine: &[Point]) -> Vec<(Point, Point)> {
    if spine.len() <= 2 {
        return vec![(spine[0], spine[spine.len() - 1])];
    }
    (0..spine.len()).map(|index| (spine[index], spine[(index + 1) % spine.len()])).collect()
}

/// `_project`: the point of segment `start`-`end` nearest `point`.
fn project(point: Point, start: Point, end: Point) -> (f64, f64) {
    let (dx, dy) = ((end.x_nm - start.x_nm) as f64, (end.y_nm - start.y_nm) as f64);
    let squared = (end.x_nm - start.x_nm) as i128 * (end.x_nm - start.x_nm) as i128
        + (end.y_nm - start.y_nm) as i128 * (end.y_nm - start.y_nm) as i128;
    if squared == 0 {
        return (start.x_nm as f64, start.y_nm as f64);
    }
    let numerator = (point.x_nm - start.x_nm) as i128 * (end.x_nm - start.x_nm) as i128
        + (point.y_nm - start.y_nm) as i128 * (end.y_nm - start.y_nm) as i128;
    let t = (numerator as f64 / squared as f64).clamp(0.0, 1.0);
    (start.x_nm as f64 + t * dx, start.y_nm as f64 + t * dy)
}

/// `_nearest_point`: the point of `own` (a via centre or track centreline)
/// nearest `other`'s spine.
fn nearest_point(own: &[Point], other: &[Point]) -> (i64, i64) {
    let (a, b) = (own[0], own[own.len() - 1]);
    if other.len() >= 3 && point_in_polygon(a, other) {
        return (a.x_nm, a.y_nm);
    }
    let mut best: Option<(f64, (f64, f64))> = None;
    for (c, d) in edges(other) {
        if a != b && c != d && segments_intersect(a, b, c, d) {
            let (rx, ry) = ((b.x_nm - a.x_nm) as i128, (b.y_nm - a.y_nm) as i128);
            let (sx, sy) = ((d.x_nm - c.x_nm) as i128, (d.y_nm - c.y_nm) as i128);
            let denominator = rx * sy - ry * sx;
            if denominator != 0 {
                let numerator = (c.x_nm - a.x_nm) as i128 * sy - (c.y_nm - a.y_nm) as i128 * sx;
                let t = numerator as f64 / denominator as f64;
                return (round_int(a.x_nm as f64 + t * rx as f64), round_int(a.y_nm as f64 + t * ry as f64));
            }
        }
        let point = |p: Point| (p.x_nm as f64, p.y_nm as f64);
        for (mine, theirs) in [
            (point(a), project(a, c, d)),
            (point(b), project(b, c, d)),
            (project(c, a, b), point(c)),
            (project(d, a, b), point(d)),
        ] {
            let distance = hypot(mine.0 - theirs.0, mine.1 - theirs.1);
            if best.is_none_or(|(least, _)| distance < least) {
                best = Some((distance, mine));
            }
        }
    }
    let (_, (x, y)) = best.expect("a spine edge");
    (round_int(x), round_int(y))
}

/// `_reach_interval`: positions along `start`-`end` (from `start`) within
/// `reach` of a convex spine, as one interval, or None.
fn reach_interval(start: Point, end: Point, spine: &[Point], reach: f64) -> Option<(f64, f64)> {
    let length = hypot((end.x_nm - start.x_nm) as f64, (end.y_nm - start.y_nm) as f64);
    if length == 0.0 {
        return None;
    }
    let (ux, uy) = ((end.x_nm - start.x_nm) as f64 / length, (end.y_nm - start.y_nm) as f64 / length);
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for (c, d) in edges(spine) {
        let mut pieces = vec![disk(start, ux, uy, c, reach), disk(start, ux, uy, d, reach)];
        let size = hypot((d.x_nm - c.x_nm) as f64, (d.y_nm - c.y_nm) as f64);
        if size != 0.0 {
            let (ex, ey) = ((d.x_nm - c.x_nm) as f64 / size, (d.y_nm - c.y_nm) as f64 / size);
            let (wx, wy) = ((start.x_nm - c.x_nm) as f64, (start.y_nm - c.y_nm) as f64);
            let along = slab(wx * ex + wy * ey, ux * ex + uy * ey, 0.0, size);
            let across = slab(wx * -ey + wy * ex, ux * -ey + uy * ex, -reach, reach);
            pieces.push((along.0.max(across.0), along.1.min(across.1)));
        }
        for (piece_low, piece_high) in pieces {
            if piece_low <= piece_high {
                low = low.min(piece_low);
                high = high.max(piece_high);
            }
        }
    }
    let (low, high) = (low.max(0.0), high.min(length));
    (low < high).then_some((low, high))
}

fn disk(start: Point, ux: f64, uy: f64, centre: Point, radius: f64) -> (f64, f64) {
    let (wx, wy) = ((start.x_nm - centre.x_nm) as f64, (start.y_nm - centre.y_nm) as f64);
    let middle = -(ux * wx + uy * wy);
    let discriminant = middle * middle - (wx * wx + wy * wy - radius * radius);
    if discriminant < 0.0 {
        return (f64::INFINITY, f64::NEG_INFINITY);
    }
    let root = discriminant.sqrt();
    (middle - root, middle + root)
}

/// `_slab`: positions `s` with `low <= offset + rate * s <= high`.
fn slab(offset: f64, rate: f64, low: f64, high: f64) -> (f64, f64) {
    if rate == 0.0 {
        return if low <= offset && offset <= high {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (f64::INFINITY, f64::NEG_INFINITY)
        };
    }
    let (first, second) = ((low - offset) / rate, (high - offset) / rate);
    (first.min(second), first.max(second))
}

/// `lane_review_line`: one console line summarising a review.
pub fn lane_review_line(review: &Object) -> String {
    fn mm(value: i64) -> String {
        format!("{:.3} mm", value as f64 / 1e6)
    }
    let empty = Vec::new();
    let array = |key: &str| match review.get(key) {
        Some(Value::Array(items)) => items,
        _ => &empty,
    };
    let field = |item: &Value, key: &str| match item {
        Value::Object(entry) => entry.get(key).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let int = |value: &Value| match value {
        Value::Int(number) => number.to_i64(),
        _ => None,
    };
    let (nets, pairs, groups) = (array("nets"), array("pairs"), array("match_groups"));
    let worst = pairs.iter().filter_map(|pair| int(&field(pair, "skew_nm"))).max();
    let radius = review.get("search_radius_nm").and_then(int).unwrap_or(LANE_REVIEW_RADIUS_NM);
    let mut spacing_parts = Vec::new();
    for kind in CLASSES {
        let mut least: Option<(i64, Value, String)> = None;
        for item in nets {
            let found = field(&field(&field(item, "spacing"), "outside_breakout"), kind);
            if let Some(distance) = int(&field(&found, "distance_nm"))
                && least.as_ref().is_none_or(|(best, _, _)| distance < *best)
            {
                let net = field(item, "net").as_str().unwrap_or_default().to_owned();
                least = Some((distance, found, net));
            }
        }
        spacing_parts.push(match least {
            Some((distance, found, net)) => format!(
                "{kind}={} ({net} to {})",
                mm(distance),
                field(&found, "neighbour").as_str().unwrap_or_default()
            ),
            None => format!("{kind}=none within {}", mm(radius)),
        });
    }
    let sharpest = nets
        .iter()
        .filter_map(|item| match field(&field(item, "bends"), "sharpest_degrees") {
            Value::Float(value) => Some(value),
            _ => None,
        })
        .fold(None, |best: Option<f64>, value| Some(best.map_or(value, |best| best.max(value))));
    let status_count =
        |items: &[Value]| items.iter().filter(|item| field(item, "status").as_str() == Some("fail")).count();
    let coupled: i64 = nets.iter().filter_map(|item| int(&field(item, "coupled_length_nm"))).sum();
    let sharp: i64 = nets.iter().filter_map(|item| int(&field(&field(item, "bends"), "sharp_count"))).sum();
    let degrees = review.get("sharp_bend_degrees").and_then(int).unwrap_or(SHARP_BEND_DEGREES);
    [
        format!("CRITICAL LANES: {} nets", nets.len()),
        format!(
            "pairs={}, worst skew={}, over max_skew={}",
            pairs.len(),
            worst.map_or_else(|| "n/a".to_owned(), mm),
            status_count(pairs)
        ),
        format!("match groups={}, over max_skew={}", groups.len(), status_count(groups)),
        format!("min spacing outside breakout: {}", spacing_parts.join(", ")),
        format!("coupled length={}", mm(coupled)),
        format!(
            "bends>{degrees}deg={sharp}{}",
            sharpest
                .map_or_else(String::new, |value| format!(" (sharpest {} deg)", copper_json::python_float_repr(value)))
        ),
    ]
    .join("; ")
}

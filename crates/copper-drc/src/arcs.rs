//! Exact-DRC checks of arched copper (`TrackArc`, CS-174).
//!
//! Arc distances are measured in floating point from the exact rational
//! centre of the three stored points (`copper_physical::arcs`); a check fails
//! when the distance falls more than [`ARC_DISTANCE_EPSILON_NM`] short of
//! its requirement. The board edge is checked on the inscribed chord chain
//! widened by its sagitta bound, which is conservative.

use indexmap::IndexMap;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackArc};
use copper_pcb::keepouts::resolved_copper_keepouts;
use copper_pcb::{Error, Result};
use copper_physical::arcs::{ARC_DISTANCE_EPSILON_NM, ArcGeometry, arc_distance, point_distance, segment_distance, shape_distance};
use copper_physical::geometry::point_in_polygon;
use copper_physical::mechanical::shape_in_outline;
use copper_physical::pyfloat::round_int;
use copper_physical::{Point, RoundedConvexShape};

use crate::breakout::BreakoutSpacing;
use crate::{Finding, PadCopper, Severity, copper_pads, non_plated_holes, shape, sorted_pair, via_covers_layer};

fn geometry(arc: &TrackArc) -> Result<ArcGeometry> {
    arc.geometry().map_err(|error| Error::Value(format!("arc copper: {error}")))
}

/// `short`: whether `measured` (a centre-line distance) falls short of `required`.
fn short(measured: f64, required: i64) -> bool {
    measured + ARC_DISTANCE_EPSILON_NM < required as f64
}

fn spacing_finding(measured: f64, message: String, objects: [String; 2], nets: Vec<String>, layers: Vec<String>, required: i64) -> Finding {
    let code = if measured <= ARC_DISTANCE_EPSILON_NM { "DRC-SHORT" } else { "DRC-CLEARANCE" };
    Finding::error(code, message).objects(objects).nets(nets).layers(layers).required(required).measured(round_int(measured).max(0))
}

/// `routed_arc_length_nm`: the arcs of `net`, each at its true length.
pub fn routed_arc_length_nm(board: &PhysicalBoard, net: &str) -> i64 {
    board.arcs.iter().filter(|arc| arc.net == net).map(TrackArc::length_nm).sum()
}

/// `check_arcs`: width, layer, edge, spacing, holes, keep-outs, polygons and
/// routing reservations of every arc, as the track checks do for tracks.
pub(crate) fn check_arcs(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    if board.arcs.is_empty() {
        return Ok(());
    }
    let rules: IndexMap<&str, &NetRoutingRule> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let spacing = BreakoutSpacing::new(board);
    let pads: Vec<PadCopper> = copper_pads(board);
    let pad_lands: Vec<Option<String>> = pads.iter().map(|p| spacing.land(&p.net, &p.shape.spine)).collect();
    let track_lands: Vec<Option<String>> = board.tracks.iter().map(|t| spacing.land(&t.net, &[t.start, t.end])).collect();
    let via_lands: Vec<Option<String>> = board.vias.iter().map(|v| spacing.land(&v.net, &[v.position])).collect();
    let arc_lands: Vec<Option<String>> = board.arcs.iter().map(|a| spacing.land(&a.net, &[a.start, a.end])).collect();
    let geometries: Vec<Result<ArcGeometry>> = board.arcs.iter().map(geometry).collect();
    let edge_clearance = board.rules.minimum_clearance_nm;
    let hole_clearance = board.rules.minimum_hole_clearance_nm;
    let holes = non_plated_holes(board);
    let keepouts = resolved_copper_keepouts(board)?;
    let net_clearance = |net: &str| rules.get(net).and_then(|r| r.clearance_nm).unwrap_or(0);
    for (index, arc) in board.arcs.iter().enumerate() {
        let object = format!("arc:{index}");
        let layer = arc.layer.value();
        let geometry = match &geometries[index] {
            Ok(geometry) => *geometry,
            Err(error) => {
                findings.push(
                    Finding::error("DRC-ARC-GEOMETRY", format!("arc {index} has no valid circular geometry: {error}"))
                        .objects([object.as_str()])
                        .nets([arc.net.as_str()])
                        .layers([layer]),
                );
                continue;
            }
        };
        let half = arc.width_nm.div_euclid(2);
        let rule = rules.get(arc.net.as_str()).copied();
        let required_width = board.rules.minimum_track_width_nm.max(rule.and_then(|r| r.width_nm).unwrap_or(0));
        if arc.width_nm < required_width {
            findings.push(
                Finding::error(
                    "DRC-TRACK-WIDTH",
                    format!("arc {index} on {} is below its minimum or routing profile width", copper_json::python_str_repr(&arc.net)),
                )
                .objects([object.as_str()])
                .nets([arc.net.as_str()])
                .layers([layer])
                .required(required_width)
                .measured(arc.width_nm),
            );
        }
        if let Some(rule) = rule
            && !rule.allowed_layers.is_empty()
            && !rule.allowed_layers.contains(&arc.layer)
        {
            findings.push(
                Finding::error(
                    "DRC-LAYER",
                    format!("net {} uses a layer excluded by its routing profile", copper_json::python_str_repr(&arc.net)),
                )
                .nets([arc.net.as_str()])
                .layers([layer]),
            );
        }
        // Board edge: conservative chord capsules.
        let mut inside = true;
        for piece in arc.clearance_pieces() {
            if !shape_in_outline(&piece, &board.outline, edge_clearance)? {
                inside = false;
                break;
            }
        }
        if !inside {
            findings.push(
                Finding::error("DRC-BOARD-EDGE", format!("arc {index} violates copper-to-board-edge clearance"))
                    .objects([object.as_str()])
                    .nets([arc.net.as_str()])
                    .layers([layer])
                    .required(edge_clearance),
            );
        }
        // Foreign tracks.
        for (track_index, track) in board.tracks.iter().enumerate() {
            if track.net == arc.net || track.layer != arc.layer {
                continue;
            }
            let (clearance, _) = spacing.between(&arc.net, arc_lands[index].as_deref(), &track.net, track_lands[track_index].as_deref());
            let required = half + track.width_nm.div_euclid(2) + clearance;
            let measured = segment_distance(&geometry, track.start, track.end);
            if short(measured, required) {
                findings.push(spacing_finding(
                    measured,
                    format!("arc {index} and track {track_index} violate {}", spacing.subject(&arc.net, &track.net)),
                    [object.clone(), format!("track:{track_index}")],
                    sorted_pair(&arc.net, &track.net),
                    vec![layer.to_owned()],
                    required,
                ));
            }
        }
        // Foreign arcs.
        for (other_index, other) in board.arcs.iter().enumerate().skip(index + 1) {
            if other.net == arc.net || other.layer != arc.layer {
                continue;
            }
            let Ok(other_geometry) = &geometries[other_index] else { continue };
            let (clearance, _) = spacing.between(&arc.net, arc_lands[index].as_deref(), &other.net, arc_lands[other_index].as_deref());
            let required = half + other.width_nm.div_euclid(2) + clearance;
            let measured = arc_distance(&geometry, other_geometry);
            if short(measured, required) {
                findings.push(spacing_finding(
                    measured,
                    format!("arcs {index} and {other_index} violate {}", spacing.subject(&arc.net, &other.net)),
                    [object.clone(), format!("arc:{other_index}")],
                    sorted_pair(&arc.net, &other.net),
                    vec![layer.to_owned()],
                    required,
                ));
            }
        }
        // Foreign vias: copper and drills.
        for (via_index, via) in board.vias.iter().enumerate() {
            if via.net == arc.net || !via_covers_layer(board, via, arc.layer)? {
                continue;
            }
            let measured = point_distance(&geometry, (via.position.x_nm as f64, via.position.y_nm as f64));
            let (clearance, _) = spacing.between(&arc.net, arc_lands[index].as_deref(), &via.net, via_lands[via_index].as_deref());
            let required = half + via.size_nm.div_euclid(2) + clearance;
            if short(measured, required) {
                findings.push(spacing_finding(
                    measured,
                    format!("arc {index} and via {via_index} violate {}", spacing.subject(&arc.net, &via.net)),
                    [object.clone(), format!("via:{via_index}")],
                    sorted_pair(&arc.net, &via.net),
                    vec![layer.to_owned()],
                    required,
                ));
            }
            let drill_required = half + via.drill_nm.div_euclid(2) + hole_clearance;
            if short(measured, drill_required) {
                findings.push(
                    Finding::error("DRC-HOLE-CLEARANCE", format!("via {via_index} drill violates arc {index} clearance"))
                        .objects([format!("via:{via_index}"), object.clone()])
                        .nets(sorted_pair(&arc.net, &via.net))
                        .required(hole_clearance)
                        .measured((round_int(measured) - half - via.drill_nm.div_euclid(2)).max(0)),
                );
            }
        }
        // Foreign lands.
        for (pad, land) in pads.iter().zip(&pad_lands) {
            if pad.net == arc.net || !pad.layers.contains(&arc.layer) {
                continue;
            }
            let (relaxed, _) = spacing.between(&arc.net, arc_lands[index].as_deref(), &pad.net, land.as_deref());
            let clearance = pad.clearance_nm.max(relaxed);
            let measured = shape_distance(&geometry, &pad.shape);
            if short(measured, half + clearance) {
                findings.push(spacing_finding(
                    measured,
                    format!("arc {index} and {} violate {}", pad.identity, spacing.subject(&arc.net, &pad.net)),
                    [object.clone(), pad.identity.clone()],
                    sorted_pair(&arc.net, &pad.net),
                    vec![layer.to_owned()],
                    half + pad.shape.radius_nm + clearance,
                ));
            }
        }
        // Fixed polygons of other nets.
        for polygon in &board.polygons {
            if polygon.net == arc.net || polygon.layer != arc.layer {
                continue;
            }
            let clearance = board.rules.minimum_clearance_nm.max(net_clearance(&arc.net)).max(net_clearance(&polygon.net));
            let measured = shape_distance(&geometry, &shape(polygon.vertices.clone(), 0)?);
            if short(measured, half + clearance) {
                findings.push(
                    Finding::error("DRC-CLEARANCE", format!("polygon {} violates arc {index} clearance", polygon.id))
                        .objects([format!("polygon:{}", polygon.id), object.clone()])
                        .nets(sorted_pair(&arc.net, &polygon.net))
                        .layers([layer])
                        .required(half + clearance)
                        .measured(round_int(measured).max(0)),
                );
            }
        }
        // Non-plated holes.
        for (identity, hole) in &holes {
            let measured = shape_distance(&geometry, hole);
            if short(measured, half + hole_clearance) {
                findings.push(
                    Finding::error("DRC-HOLE-CLEARANCE", format!("arc {index} violates {identity} drill clearance"))
                        .objects([object.clone(), identity.clone()])
                        .nets([arc.net.as_str()])
                        .layers([layer])
                        .required(hole_clearance)
                        .measured((round_int(measured) - half).max(0)),
                );
            }
        }
        // Copper keep-outs that block tracks.
        for keepout in &keepouts {
            if !keepout.block_tracks || !keepout.layers.contains(&arc.layer) {
                continue;
            }
            let measured = shape_distance(&geometry, &shape(keepout.vertices.clone(), 0)?);
            if measured < half as f64 + ARC_DISTANCE_EPSILON_NM {
                findings.push(
                    Finding::error("DRC-COPPER-KEEPOUT", format!("arc {index} enters copper keepout {}", keepout.id))
                        .objects([keepout.id.clone(), object.clone()])
                        .nets([arc.net.as_str()])
                        .layers([layer]),
                );
            }
        }
        // Routing reservations of other nets' zones.
        for zone in board.zones.iter().filter(|z| z.reserve_routing) {
            if zone.net == arc.net || !zone.layers.contains(&arc.layer) {
                continue;
            }
            let margin = board
                .rules
                .minimum_clearance_nm
                .max(zone.clearance_nm.unwrap_or(0))
                .max(net_clearance(&arc.net))
                .max(net_clearance(&zone.net));
            if !short(shape_distance(&geometry, &shape(zone.outer.clone(), 0)?), half + margin) {
                continue;
            }
            let chords = arc.chord_points();
            let in_hole = zone.holes.iter().any(|hole| {
                chords.iter().all(|point| point_in_polygon(*point, hole))
                    && hole_edges_clear(&geometry, hole, half + margin)
            });
            if in_hole {
                continue;
            }
            findings.push(
                Finding::error(
                    "DRC-ZONE-RESERVATION",
                    format!("arc {index} enters routing reservation {} for {}", zone.id, zone.net),
                )
                .objects([zone.id.clone(), object.clone()])
                .nets([zone.net.as_str(), arc.net.as_str()]),
            );
        }
        let _ = Severity::Error;
    }
    Ok(())
}

fn hole_edges_clear(geometry: &ArcGeometry, hole: &[Point], required: i64) -> bool {
    (0..hole.len()).all(|i| !short(segment_distance(geometry, hole[i], hole[(i + 1) % hole.len()]), required))
}

/// `arc_contact_shapes`: the deflated chord capsules of `arc` (inside its copper).
pub fn arc_contact_shapes(arc: &TrackArc) -> Vec<RoundedConvexShape> {
    arc.contact_pieces()
}

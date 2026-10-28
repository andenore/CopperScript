//! Explicit source copper-region boundaries (`pcbir.zone_geometry`).

use indexmap::IndexMap;

use copper_lang::model::{Constraint, ConstraintKind, ParameterValue};
use copper_physical::geometry::{point_in_polygon, ring_edges, segment_in_polygon, segments_intersect};
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_units::Dimension;

use crate::board::{CopperZone, NetRoutingRule, PhysicalBoard};
use crate::{Error, Result, fail};

/// The value of a length parameter, in integer nanometres truncated toward zero.
pub(crate) fn length_nm(value: &ParameterValue) -> Option<i64> {
    let ParameterValue::Quantity(quantity) = value else { return None };
    if quantity.dimension() != Dimension::Length {
        return None;
    }
    let mm = quantity.in_unit("mm").ok()?;
    mm.mul(&copper_units::Decimal::from_i64(1_000_000)).ok()?.to_i64()
}

fn zone_length(parameters: &IndexMap<String, ParameterValue>, name: &str) -> Result<i64> {
    match parameters.get(name) {
        None => Ok(0),
        Some(value) => length_nm(value).ok_or_else(|| Error::Value(format!("copper_zone '{name}' must be a length"))),
    }
}

fn rectangle(parameters: &IndexMap<String, ParameterValue>) -> Result<BoardOutline> {
    if !["x", "y", "width", "height"].iter().all(|name| parameters.contains_key(*name)) {
        return fail("copper_zone rectangle requires x, y, width and height");
    }
    let (width, height) = (zone_length(parameters, "width")?, zone_length(parameters, "height")?);
    let origin = Point::new(zone_length(parameters, "x")?, zone_length(parameters, "y")?);
    Ok(BoardOutline::rectangle(width, height, Some(origin))?)
}

/// `polygon_boundary`: an exact fixed-point mm polygon `"x,y; x,y; ..."`.
pub fn polygon_boundary(value: &ParameterValue, context: &str) -> Result<BoardOutline> {
    let ParameterValue::Text(text) = value else {
        return Err(Error::Value(format!("{context} polygon_mm must be a string of x,y pairs")));
    };
    let invalid = |message: &str| Error::Value(format!("invalid {context} polygon_mm: {message}"));
    let pairs: Vec<Vec<&str>> =
        text.split(';').map(|vertex| vertex.split(',').map(crate::py_strip).collect()).collect();
    if pairs.iter().any(|pair| pair.len() != 2) {
        return Err(invalid("expected x,y pairs"));
    }
    let mut points = Vec::new();
    for pair in pairs {
        let coordinate = |text: &str| -> Result<i64> {
            match crate::nm_from_mm_text(text) {
                Ok(nm) => Ok(nm),
                Err(Error::Value(_)) => Err(invalid("[<class 'decimal.ConversionSyntax'>]")),
                Err(other) => Err(other),
            }
        };
        points.push(Point::new(coordinate(pair[0])?, coordinate(pair[1])?));
    }
    BoardOutline::polygon(points).map_err(|message| invalid(&message))
}

/// `lower_zone_outline`: rectangle, mm polygon, placement region or the board
/// outline, inset and with fully contained cutouts as holes.
pub fn lower_zone_outline(
    parameters: &IndexMap<String, ParameterValue>,
    board_outline: &BoardOutline,
    constraints: &[Constraint],
) -> Result<(Vec<Point>, Vec<Vec<Point>>)> {
    let is_rectangle = ["x", "y", "width", "height"].iter().any(|name| parameters.contains_key(*name));
    let modes = usize::from(is_rectangle)
        + usize::from(parameters.contains_key("polygon_mm"))
        + usize::from(parameters.contains_key("region"));
    if modes > 1 {
        return fail("copper_zone boundary parameters are mutually exclusive");
    }
    let mut boundary = board_outline.clone();
    if is_rectangle {
        boundary = rectangle(parameters)?;
    } else if let Some(polygon) = parameters.get("polygon_mm") {
        boundary = polygon_boundary(polygon, "copper_zone")?;
    } else if let Some(name) = parameters.get("region") {
        let regions: Vec<&Constraint> = constraints
            .iter()
            .enumerate()
            .filter(|(index, c)| {
                c.kind == ConstraintKind::PlacementRegion
                    && match c.parameters.get("name") {
                        Some(value) => crate::physicalize::parameter_eq(value, name),
                        None => matches!(name, ParameterValue::Text(text) if *text == format!("region:{index}")),
                    }
            })
            .map(|(_, c)| c)
            .collect();
        let ParameterValue::Text(_) = name else {
            return Err(Error::Value(format!(
                "copper_zone references unknown or ambiguous placement region {}",
                crate::physicalize::parameter_repr(name)
            )));
        };
        if regions.len() != 1 {
            return Err(Error::Value(format!(
                "copper_zone references unknown or ambiguous placement region {}",
                crate::physicalize::parameter_repr(name)
            )));
        }
        let region = &regions[0].parameters;
        if let Some(polygon) = region.get("polygon_mm") {
            if ["x", "y", "width", "height"].iter().any(|name| region.contains_key(*name)) {
                return fail("placement_region boundary parameters are mutually exclusive");
            }
            boundary = polygon_boundary(polygon, "placement_region")?;
        } else {
            boundary = rectangle(region)?;
        }
    }
    let inset = zone_length(parameters, "inset")?;
    if inset < 0 {
        return fail("copper_zone inset cannot be negative");
    }
    if let Some(circle) = boundary.circular_boundary {
        let radius = circle.radius_nm - inset;
        if radius <= 0 {
            return fail("copper_zone inset consumes the board outline");
        }
        boundary = BoardOutline::circle(2 * radius, Some(circle.center), circle.maximum_chord_error_nm)?;
    } else if inset != 0 {
        let xs: Vec<i64> = boundary.vertices.iter().map(|p| p.x_nm).collect();
        let ys: Vec<i64> = boundary.vertices.iter().map(|p| p.y_nm).collect();
        let (left, right) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        let (top, bottom) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
        let corners =
            [Point::new(left, top), Point::new(right, top), Point::new(right, bottom), Point::new(left, bottom)];
        let distinct: Vec<Point> = boundary.vertices.iter().fold(Vec::new(), |mut seen, point| {
            if !seen.contains(point) {
                seen.push(*point);
            }
            seen
        });
        if distinct.len() != 4 || !corners.iter().all(|corner| distinct.contains(corner)) {
            return fail("nonzero copper_zone inset requires a rectangular or circular board outline");
        }
        if right - left <= 2 * inset || bottom - top <= 2 * inset {
            return fail("copper_zone inset consumes the board outline");
        }
        boundary = BoardOutline::rectangle(
            right - left - 2 * inset,
            bottom - top - 2 * inset,
            Some(Point::new(left + inset, top + inset)),
        )?;
    }
    let edges = ring_edges(&boundary.vertices);
    if !edges.iter().all(|(a, b)| segment_in_polygon(*a, *b, &board_outline.vertices)) {
        return fail("copper_zone boundary must be inside the board outline");
    }
    let mut holes = Vec::new();
    for cutout in &board_outline.cutouts {
        let contained: Vec<bool> = cutout.vertices.iter().map(|p| point_in_polygon(*p, &boundary.vertices)).collect();
        let cut_edges = ring_edges(&cutout.vertices);
        let intersects = edges.iter().any(|(a, b)| cut_edges.iter().any(|(c, d)| segments_intersect(*a, *b, *c, *d)));
        if contained.iter().all(|inside| *inside) && !intersects {
            holes.push(copper_physical::polygon_ring(&cutout.vertices)?);
        } else if contained.iter().any(|inside| *inside)
            || intersects
            || boundary.vertices.iter().any(|p| point_in_polygon(*p, &cutout.vertices))
        {
            return fail("copper_zone boundary intersects a board cutout");
        }
    }
    Ok((copper_physical::polygon_ring(&boundary.vertices)?, holes))
}

fn inside(point: Point, outer: &[Point], holes: &[Vec<Point>]) -> bool {
    point_in_polygon(point, outer) && !holes.iter().any(|hole| point_in_polygon(point, hole))
}

/// `validate_routing_reservations`: reserved rails on one layer stay disjoint.
pub fn validate_routing_reservations(zones: &[CopperZone]) -> Result<()> {
    let reserved: Vec<&CopperZone> = zones.iter().filter(|zone| zone.reserve_routing).collect();
    for (index, first) in reserved.iter().enumerate() {
        for second in &reserved[..index] {
            if first.net == second.net || !first.layers.iter().any(|layer| second.layers.contains(layer)) {
                continue;
            }
            let ring_set = |zone: &CopperZone| -> Vec<(Point, Point)> {
                std::iter::once(&zone.outer).chain(zone.holes.iter()).flat_map(|ring| ring_edges(ring)).collect()
            };
            let (edges_a, edges_b) = (ring_set(first), ring_set(second));
            if edges_a.iter().any(|(a, b)| edges_b.iter().any(|(c, d)| segments_intersect(*a, *b, *c, *d)))
                || first.outer.iter().any(|p| inside(*p, &second.outer, &second.holes))
                || second.outer.iter().any(|p| inside(*p, &first.outer, &first.holes))
            {
                return Err(Error::Value(format!(
                    "reserved copper zones {} and {} overlap or touch on the same layer",
                    copper_json::python_str_repr(&first.id),
                    copper_json::python_str_repr(&second.id)
                )));
            }
        }
    }
    Ok(())
}

/// `distribution_zone_nets`: nets with declared (non-macro) copper zones.
pub fn distribution_zone_nets(board: &PhysicalBoard) -> indexmap::IndexSet<String> {
    let local: Vec<&str> = board.hard_macros.iter().flat_map(|m| m.zones.iter().map(|z| z.id.as_str())).collect();
    board.zones.iter().filter(|z| !local.contains(&z.id.as_str())).map(|z| z.net.clone()).collect()
}

fn area_twice(vertices: &[Point]) -> i128 {
    let mut total: i128 = 0;
    for (index, first) in vertices.iter().enumerate() {
        let second = vertices[(index + 1) % vertices.len()];
        total += i128::from(first.x_nm) * i128::from(second.y_nm) - i128::from(second.x_nm) * i128::from(first.y_nm);
    }
    total.abs()
}

/// `dedicated_plane_layers`: unambiguous, nearly board-wide inner-layer planes.
pub fn dedicated_plane_layers(board: &PhysicalBoard) -> IndexMap<CopperLayer, String> {
    let board_area = area_twice(&board.outline.vertices);
    let mut result = IndexMap::new();
    if board_area == 0 {
        return result;
    }
    let layers = &board.stackup.copper_layers;
    for layer in layers.iter().skip(1).take(layers.len().saturating_sub(2)) {
        let zones: Vec<&CopperZone> = board.zones.iter().filter(|z| z.layers.contains(layer)).collect();
        let wide: Vec<&&CopperZone> =
            zones.iter().filter(|z| z.holes.is_empty() && 5 * area_twice(&z.outer) >= 4 * board_area).collect();
        let nets: indexmap::IndexSet<&str> = wide.iter().map(|z| z.net.as_str()).collect();
        if nets.len() != 1 {
            continue;
        }
        let net = &wide[0].net;
        let base = wide.iter().map(|z| z.priority).min().expect("wide zones");
        // A split plane stays a plane: other nets may only carve islands with a higher priority.
        if zones.iter().all(|z| z.net == *net || z.priority > base) {
            result.insert(*layer, net.clone());
        }
    }
    result
}

/// `routing_layers`: legal track layers, preserving explicit restrictions and planes.
pub fn routing_layers(board: &PhysicalBoard, net: &str, rule: Option<&NetRoutingRule>) -> Vec<CopperLayer> {
    let selected = match rule {
        Some(rule) if !rule.allowed_layers.is_empty() => &rule.allowed_layers,
        _ => &board.stackup.copper_layers,
    };
    let planes = dedicated_plane_layers(board);
    selected.iter().filter(|layer| planes.get(*layer).is_none_or(|owner| owner == net)).copied().collect()
}

/// `deferred_zone_nets`: nets whose every terminal lies in one declared (non-macro) zone.
///
/// Routing intent, never filled connectivity: regional islands must not
/// suppress external feeders or bridges between separate islands.
pub fn deferred_zone_nets(board: &PhysicalBoard) -> Result<indexmap::IndexSet<String>> {
    let local: Vec<&str> = board.hard_macros.iter().flat_map(|m| m.zones.iter().map(|z| z.id.as_str())).collect();
    let poses = crate::keepouts::placement_map(board);
    let mut members: IndexMap<&str, Vec<Point>> = IndexMap::new();
    for net in &board.nets {
        let mut points = Vec::new();
        for pad in &net.pads {
            let pose = poses
                .get(&pad.component)
                .ok_or_else(|| Error::Value(copper_json::python_str_repr(&pad.component)))?;
            points.push(crate::transform::transformed_pad_position(board, pose, &pad.pad)?);
        }
        members.insert(&net.name, points);
    }
    let mut result = indexmap::IndexSet::new();
    for zone in &board.zones {
        if local.contains(&zone.id.as_str()) {
            continue;
        }
        let Some(points) = members.get(zone.net.as_str()).filter(|points| !points.is_empty()) else { continue };
        if points.iter().all(|p| {
            point_in_polygon(*p, &zone.outer) && !zone.holes.iter().any(|hole| point_in_polygon(*p, hole))
        }) {
            result.insert(zone.net.clone());
        }
    }
    Ok(result)
}

/// `ZoneRoutingReservations`: net-aware track reservations shared by access,
/// routing and final DRC. Routing intent, never filled copper or via/pad
/// obstructions.
pub struct ZoneRoutingReservations {
    pub zones: Vec<(CopperZone, copper_physical::RoundedConvexShape)>,
    pub clearances: IndexMap<String, i64>,
    pub minimum_clearance_nm: i64,
}

impl ZoneRoutingReservations {
    pub fn new(board: &PhysicalBoard) -> Self {
        let zones = board
            .zones
            .iter()
            .filter(|zone| zone.reserve_routing)
            .map(|zone| {
                (zone.clone(), copper_physical::RoundedConvexShape::new(zone.outer.clone(), 0).expect("zone outline"))
            })
            .collect();
        let clearances = board.net_routing_rules.iter().map(|r| (r.net.clone(), r.clearance_nm.unwrap_or(0))).collect();
        ZoneRoutingReservations { zones, clearances, minimum_clearance_nm: board.rules.minimum_clearance_nm }
    }

    /// `blocking_zones`: the reserved zones of other nets that `shape` on `layer` consumes.
    pub fn blocking_zones(
        &self,
        net: &str,
        shape: &copper_physical::RoundedConvexShape,
        layer: CopperLayer,
    ) -> Vec<&CopperZone> {
        use copper_physical::geometry::shapes_clear;
        let mut result = Vec::new();
        for (zone, outer) in &self.zones {
            if zone.net == net || !zone.layers.contains(&layer) {
                continue;
            }
            let margin = self
                .minimum_clearance_nm
                .max(zone.clearance_nm.unwrap_or(0))
                .max(self.clearances.get(net).copied().unwrap_or(0))
                .max(self.clearances.get(&zone.net).copied().unwrap_or(0));
            if shapes_clear(shape, outer, margin) {
                continue;
            }
            // A swept shape wholly inside a polygon hole does not consume the
            // reserved copper. Keep the radius and clearance from its boundary.
            let in_hole = zone.holes.iter().any(|hole| {
                ring_edges(&shape.spine).iter().all(|(a, b)| segment_in_polygon(*a, *b, hole))
                    && ring_edges(hole).iter().all(|(a, b)| {
                        shapes_clear(shape, &copper_physical::RoundedConvexShape::new(vec![*a, *b], 0).expect("edge"), margin)
                    })
            });
            if in_hole {
                continue;
            }
            result.push(zone);
        }
        result
    }

    pub fn can_area(&self, net: &str, shape: &copper_physical::RoundedConvexShape, layer: CopperLayer) -> bool {
        self.blocking_zones(net, shape, layer).is_empty()
    }
}

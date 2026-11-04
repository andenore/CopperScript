//! Native copper zone fill, following KiCad 10's filler (iterative refill):
//!
//! 1. Start from the zone outline, clipped to the board.
//! 2. Knock out thermal relief gaps around pads connected by thermals.
//! 3. Knock out other-net pads, holes, vias, tracks, copper graphics and text,
//!    the board edge and keepouts, each grown by its rule clearance (plus
//!    KiCad's 0.5 um fill margin), and keep the thermal spokes that reach copper.
//! 4. Drop features thinner than the zone's minimum width (shrink, regrow round).
//! 5. Knock out the fills of higher-priority zones on other nets, grown by
//!    their clearance, and prune minimum width again; give higher-priority
//!    same-net zones their own area.
//! 6. Remove isolated islands: fill outlines whose connected copper reaches no
//!    pad, by the zone's island removal mode; then refill until stable, so
//!    zones reclaim space freed by removed islands.
//!
//! Zones are processed by descending (priority, UUID), KiCad's ordering.
//! Curves become polygons that err outside the true shape, so knockouts are
//! never smaller than required.

use indexmap::IndexMap;

use crate::board::{Board, Footprint, Layer, Pad, PadKind, PadShape, Shape, TrackPath, Zone, ZoneConnection};
use crate::font;
use crate::geometry::{Contour as Path, Point, flatten_arc, rotate_f64};
use crate::poly::{self, Shapes};
use crate::rules::{Item, Rules};

pub type Result<T> = std::result::Result<T, String>;

/// KiCad's `ExtraFillMargin`: 0.0005 mm added to every fill knockout.
const EXTRA_MARGIN: i64 = 500;
/// KiCad's minimum-width epsilon, 0.001 mm.
const EPSILON: i64 = 1_000;
const MAX_REFILL_WAVES: usize = 8;

/// The fill of one zone on one copper layer.
#[derive(Clone, Debug)]
pub struct ZoneFill {
    /// Index into `Board::zones`.
    pub zone: usize,
    pub layer: usize,
    pub shapes: Shapes,
    pub islands_removed: usize,
}

type Bounds = (i64, i64, i64, i64);

fn grow(bounds: Bounds, by: i64) -> Bounds {
    (bounds.0 - by, bounds.1 - by, bounds.2 + by, bounds.3 + by)
}

fn overlaps(a: Bounds, b: Bounds) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

fn shapes_bounds(shapes: &Shapes) -> Option<Bounds> {
    shapes
        .iter()
        .map(|shape| poly::bounds(shape))
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

fn point_bounds(point: Point, radius: i64) -> Bounds {
    (point.x - radius, point.y - radius, point.x + radius, point.y + radius)
}

/// The contour as board points, arcs flattened to `tolerance`.
fn path_points(path: &Path, tolerance: f64) -> Vec<(f64, f64)> {
    let mut points = path.flattened(tolerance);
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

fn int_contour(points: &[(f64, f64)]) -> poly::Contour {
    points
        .iter()
        .map(|&(x, y)| i_overlay::i_float::int::point::IntPoint::new(x.round() as i64, y.round() as i64))
        .collect()
}

fn corner_radius(pad: &Pad) -> f64 {
    match pad.shape {
        PadShape::RoundRect(ratio) => (pad.width.min(pad.height) as f64 * ratio).round(),
        PadShape::Oval => pad.width.min(pad.height) as f64 / 2.0,
        PadShape::Circle => pad.width as f64 / 2.0,
        PadShape::Rect => 0.0,
    }
}

/// Whether the pad has copper on `layer` (an NPTH pad no larger than its hole has none).
pub(crate) fn flashes(pad: &Pad, layer: usize) -> bool {
    pad.layers.has(Layer::Copper(layer))
        && !(pad.kind == PadKind::NonPlated
            && matches!(pad.shape, PadShape::Circle | PadShape::Oval)
            && pad.drill.is_some_and(|(w, h)| pad.width <= w && pad.height <= h))
}

struct Filler<'a> {
    board: &'a Board,
    rules: &'a Rules,
    max_error: f64,
    board_area: Shapes,
    worst: i64,
    /// Zones in KiCad's fill order: highest (priority, UUID) first.
    order: Vec<usize>,
}

impl<'a> Filler<'a> {
    fn new(board: &'a Board, rules: &'a Rules) -> Result<Self> {
        let max_error = rules.max_error as f64;
        let board_area = board_area(board, max_error)?;
        let mut order: Vec<usize> = (0..board.zones.len()).filter(|&i| board.zones[i].keepout.is_none()).collect();
        order.sort_by(|&a, &b| {
            let (za, zb) = (&board.zones[a], &board.zones[b]);
            (zb.priority, &zb.uuid).cmp(&(za.priority, &za.uuid))
        });
        Ok(Filler { board, rules, max_error, board_area, worst: rules.worst_clearance(), order })
    }

    fn zone_item(zone: &'a Zone) -> Item<'a> {
        Item { net: zone.net.as_deref(), reference: None, local_clearance: zone.clearance }
    }

    fn outline(&self, zone: &Zone) -> Shapes {
        let contours: Vec<poly::Contour> =
            zone.outline.iter().map(|path| int_contour(&path_points(path, self.max_error))).collect();
        poly::even_odd(&contours)
    }

    fn higher(&self, a: usize, b: usize) -> bool {
        let (za, zb) = (&self.board.zones[a], &self.board.zones[b]);
        (za.priority, &za.uuid) > (zb.priority, &zb.uuid)
    }

    fn pad_shape(&self, pad: &Pad, growth: f64) -> Shapes {
        let (w, h) = match pad.shape {
            PadShape::Circle => (pad.width as f64, pad.width as f64),
            _ => (pad.width as f64, pad.height as f64),
        };
        poly::rounded_rect(
            pad.center,
            w + 2.0 * growth,
            h + 2.0 * growth,
            corner_radius(pad) + growth,
            pad.angle,
            self.max_error,
        )
    }

    fn hole_shape(&self, pad: &Pad, growth: f64) -> Shapes {
        let Some((w, h)) = pad.drill else { return Vec::new() };
        let (w, h) = (w as f64 + 2.0 * growth, h as f64 + 2.0 * growth);
        poly::rounded_rect(pad.center, w, h, w.min(h) / 2.0, pad.angle, self.max_error)
    }

    fn connection(&self, footprint: &Footprint, pad: &Pad, zone: &Zone, layer: usize) -> ZoneConnection {
        let connection = pad.zones.connection.or(footprint.zones.connection).unwrap_or(zone.connection);
        let connection = match connection {
            ZoneConnection::ThroughHoleThermal if pad.kind == PadKind::ThroughHole => ZoneConnection::Thermal,
            ZoneConnection::ThroughHoleThermal => ZoneConnection::Solid,
            other => other,
        };
        if connection == ZoneConnection::Thermal && !flashes(pad, layer) { ZoneConnection::None } else { connection }
    }

    /// Clearance knockouts on `layer` for `zone`, and its thermally connected pads.
    fn clearance_holes(
        &self,
        zone: &'a Zone,
        layer: usize,
        outline: &Shapes,
    ) -> (Shapes, Shapes, Vec<(&'a Footprint, &'a Pad)>) {
        let board = self.board;
        let reach = self.worst + zone.thermal_gap + zone.min_thickness + EXTRA_MARGIN;
        let area = grow(shapes_bounds(outline).unwrap_or((0, 0, 0, 0)), reach);
        let zone_item = Self::zone_item(zone);
        let extra = EXTRA_MARGIN as f64;
        let mut holes: Shapes = Vec::new();
        let mut reliefs: Shapes = Vec::new();
        let mut thermal = Vec::new();
        let same_net = |net: &Option<String>| zone.net.is_some() && *net == zone.net;
        for (footprint, pad) in board.pads() {
            let has_hole = pad.drill.is_some();
            if !pad.layers.has(Layer::Copper(layer)) && !(pad.kind == PadKind::NonPlated && has_hole) {
                continue;
            }
            let size = pad.width.max(pad.height).max(pad.drill.map_or(0, |(w, h)| w.max(h)));
            if !overlaps(area, point_bounds(pad.center, size)) {
                continue;
            }
            let flash = flashes(pad, layer);
            let item = Item {
                net: pad.net.as_deref(),
                reference: Some(&footprint.reference),
                local_clearance: pad.zones.clearance.or(footprint.zones.clearance),
            };
            if same_net(&pad.net) {
                match self.connection(footprint, pad, zone, layer) {
                    ZoneConnection::Solid | ZoneConnection::ThroughHoleThermal => {}
                    ZoneConnection::Thermal => {
                        if !poly::intersection(&self.pad_shape(pad, 0.0), outline).is_empty() {
                            let gap = pad.zones.thermal_gap.or(footprint.zones.thermal_gap).unwrap_or(zone.thermal_gap);
                            reliefs.extend(self.pad_shape(pad, gap as f64));
                            thermal.push((footprint, pad));
                        }
                    }
                    ZoneConnection::None => {
                        let gap = zone.clearance.unwrap_or(0) as f64;
                        if flash {
                            holes.extend(self.pad_shape(pad, gap));
                        } else if has_hole {
                            holes.extend(self.hole_shape(pad, gap));
                        }
                    }
                }
                continue;
            }
            let plated = has_hole && pad.kind == PadKind::ThroughHole;
            let gap = if flash || plated { self.rules.clearance(&zone_item, &item) } else { -1 };
            if flash && gap >= 0 {
                holes.extend(self.pad_shape(pad, gap as f64 + extra));
            }
            if let Some((w, h)) = pad.drill {
                let mut hole_gap = if pad.kind == PadKind::NonPlated { -1 } else { gap };
                hole_gap = hole_gap.max(self.rules.hole_clearance(&zone_item, &item));
                if pad.kind == PadKind::NonPlated && w != h {
                    hole_gap = hole_gap.max(self.rules.edge_clearance(&zone_item));
                }
                holes.extend(self.hole_shape(pad, hole_gap as f64 + extra));
            }
        }
        for via in &board.vias {
            if same_net(&via.net) || !overlaps(area, point_bounds(via.center, via.size)) {
                continue;
            }
            let item = Item { net: via.net.as_deref(), ..Item::default() };
            let gap = self.rules.clearance(&zone_item, &item);
            holes.extend(poly::circle(via.center, via.size as f64 / 2.0 + (gap + EXTRA_MARGIN) as f64, self.max_error));
            let hole_gap = gap.max(self.rules.hole_clearance(&zone_item, &item));
            holes.extend(poly::circle(
                via.center,
                via.drill as f64 / 2.0 + (hole_gap + EXTRA_MARGIN) as f64,
                self.max_error,
            ));
        }
        for track in board.tracks.iter().filter(|track| track.layer == layer && !same_net(&track.net)) {
            let item = Item { net: track.net.as_deref(), ..Item::default() };
            let width = (track.width + 2 * (self.rules.clearance(&zone_item, &item) + EXTRA_MARGIN)) as f64;
            match track.path {
                TrackPath::Line { start, end } => {
                    let bounds = (start.x.min(end.x), start.y.min(end.y), start.x.max(end.x), start.y.max(end.y));
                    if overlaps(area, grow(bounds, track.width)) {
                        holes.extend(poly::capsule(start, end, width, self.max_error));
                    }
                }
                TrackPath::Arc { start, mid, end } => {
                    holes.extend(poly::polyline(
                        &flatten_arc(start, mid, end, self.max_error),
                        width,
                        false,
                        self.max_error,
                    ));
                }
            }
        }
        let no_net = Item::default();
        let graphic_gap = (self.rules.clearance(&zone_item, &no_net) + EXTRA_MARGIN) as f64;
        for drawing in board.drawings.iter().filter(|drawing| drawing.layer == Layer::Copper(layer)) {
            // A graphic of the zone's net merges with the fill.
            if same_net(&drawing.net) {
                continue;
            }
            let item = Item { net: drawing.net.as_deref(), ..Item::default() };
            let gap = (self.rules.clearance(&zone_item, &item) + EXTRA_MARGIN) as f64;
            holes.extend(self.drawing_shape(&drawing.shape, drawing.width as f64, drawing.filled, gap));
        }
        for text in board.texts.iter().filter(|text| text.layer == Layer::Copper(layer)) {
            if let Ok((strokes, pen)) = font::strokes(text) {
                for stroke in strokes {
                    holes.extend(poly::polyline(&stroke, pen as f64 + 2.0 * graphic_gap, false, self.max_error));
                }
            }
        }
        // Board edges: clearance from the edge line itself, ignoring its width.
        let edge = 2.0 * (self.rules.edge_clearance(&zone_item) + EXTRA_MARGIN) as f64;
        for drawing in board.drawings.iter().filter(|drawing| drawing.layer == Layer::EdgeCuts) {
            holes.extend(self.drawing_shape(&drawing.shape, 0.0, false, edge / 2.0));
        }
        for keepout in board.zones.iter().filter(|z| z.keepout == Some(true) && z.layers.contains(&layer)) {
            holes.extend(self.outline(keepout));
        }
        (holes, reliefs, thermal)
    }

    /// A graphic grown by `gap`: strokes as round-ended lines of `width + 2 gap`.
    fn drawing_shape(&self, shape: &Shape, width: f64, filled: bool, gap: f64) -> Shapes {
        let e = self.max_error;
        match shape {
            Shape::Line { start, end } => poly::capsule(*start, *end, width + 2.0 * gap, e),
            Shape::Arc { start, mid, end } => {
                poly::polyline(&flatten_arc(*start, *mid, *end, e), width + 2.0 * gap, false, e)
            }
            Shape::Circle { center, radius } => {
                if filled {
                    poly::circle(*center, *radius as f64 + width / 2.0 + gap, e)
                } else {
                    let ring = poly::circle(*center, *radius as f64, e);
                    let points: Vec<(f64, f64)> = ring[0][0].iter().map(|p| (p.x as f64, p.y as f64)).collect();
                    poly::polyline(&points, width + 2.0 * gap, true, e)
                }
            }
            Shape::Polygon(path) => {
                let points = path_points(path, e);
                let mut shapes = poly::polyline(&points, width + 2.0 * gap, true, e);
                if filled {
                    shapes.extend(poly::polygon(&points));
                }
                shapes
            }
        }
    }

    /// KiCad's four spokes per thermal pad: closed outlines and their test points.
    fn spokes(&self, zone: &Zone, footprint: &Footprint, pad: &Pad) -> Vec<(Shapes, (f64, f64), poly::Contour)> {
        let gap = pad.zones.thermal_gap.or(footprint.zones.thermal_gap).unwrap_or(zone.thermal_gap) as f64;
        let width = pad.zones.spoke_width.or(footprint.zones.spoke_width).unwrap_or(zone.spoke_width);
        let width = width.min(pad.width.min(pad.height));
        if width < zone.min_thickness {
            return Vec::new();
        }
        let half_w = (width / 2) as f64;
        let circular = pad.shape == PadShape::Circle || (pad.shape == PadShape::Oval && pad.width == pad.height);
        let (w, h) = if pad.shape == PadShape::Circle { (pad.width, pad.width) } else { (pad.width, pad.height) };
        let inflate = gap + self.max_error + (zone.min_thickness / 2) as f64;
        let (hx, hy) = ((w as f64 / 2.0 + inflate).round(), (h as f64 / 2.0 + inflate).round());
        let base = if circular { 0.0 } else { pad.spoke_angle };
        let mut result = Vec::new();
        for quarter in 0..4 {
            let angle = (base + 90.0 * quarter as f64).to_radians();
            let (dx, dy) = (angle.cos(), angle.sin());
            let (dx, dy) = (if dx.abs() < 1e-12 { 0.0 } else { dx }, if dy.abs() < 1e-12 { 0.0 } else { dy });
            let (side, end) = if dx == 0.0 {
                ((half_w, 0.0), (0.0, (dy * hy).round()))
            } else if dy == 0.0 {
                ((0.0, half_w), ((dx * hx).round(), 0.0))
            } else {
                let (dist_x, dist_y) = (hx / dx.abs(), hy / dy.abs());
                if dist_x < dist_y {
                    (
                        (0.0, (half_w / (std::f64::consts::FRAC_PI_2 - angle).sin()).round()),
                        ((dx * dist_x).round(), (dy * dist_x).round()),
                    )
                } else {
                    (((half_w / angle.sin()).round(), 0.0), ((dx * dist_y).round(), (dy * dist_y).round()))
                }
            };
            let local = [
                (side.0, side.1),
                (-side.0, -side.1),
                (end.0 - side.0, end.1 - side.1),
                (end.0, end.1),
                (end.0 + side.0, end.1 + side.1),
            ];
            let place = |(x, y): (f64, f64)| {
                let (x, y) = if circular { rotate_f64(x, y, pad.spoke_angle) } else { (x, y) };
                let (x, y) = rotate_f64(x, y, pad.angle);
                ((pad.center.x as f64 + x).round(), (pad.center.y as f64 + y).round())
            };
            let points: Vec<(f64, f64)> = local.iter().map(|&p| place(p)).collect();
            let outline = int_contour(&points);
            result.push((poly::polygon(&points), points[3], outline));
        }
        result
    }

    /// Remove features narrower than the zone's minimum width.
    fn prune(&self, shapes: &Shapes, zone: &Zone) -> Shapes {
        let shrink = zone.min_thickness / 2 - EPSILON;
        if shrink <= EPSILON {
            return shapes.clone();
        }
        let mut deflated = poly::offset(shapes, -shrink, false, self.max_error);
        deflated.retain(|shape| {
            let (x0, y0, x1, y1) = poly::bounds(shape);
            (x1 - x0).max(y1 - y0) >= zone.min_thickness
        });
        poly::offset(&deflated, shrink, true, self.max_error)
    }

    /// Higher-priority other-net zone fills on `layer`, grown by their clearance.
    fn zone_clearances(
        &self,
        zone_index: usize,
        layer: usize,
        fills: &IndexMap<(usize, usize), Shapes>,
        area: Bounds,
    ) -> Shapes {
        let zone = &self.board.zones[zone_index];
        let mut holes = Vec::new();
        for (&(other, other_layer), shapes) in fills {
            let other_zone = &self.board.zones[other];
            if other_layer != layer
                || other == zone_index
                || !self.higher(other, zone_index)
                || other_zone.net == zone.net
            {
                continue;
            }
            if shapes_bounds(shapes).is_none_or(|bounds| !overlaps(area, bounds)) {
                continue;
            }
            let gap = self.rules.clearance(&Self::zone_item(zone), &Self::zone_item(other_zone)).max(0);
            holes.extend(poly::offset(shapes, gap + EXTRA_MARGIN + self.rules.max_error, true, self.max_error));
        }
        holes
    }

    /// Steps 1-4: the fill before zone-to-zone knockouts.
    fn first_fill(&self, zone_index: usize, layer: usize) -> (Shapes, Shapes) {
        let zone = &self.board.zones[zone_index];
        let extents = poly::intersection(&self.outline(zone), &self.board_area);
        let (holes, reliefs, thermal) = self.clearance_holes(zone, layer, &extents);
        let mut fill = poly::difference(&extents, &reliefs);
        // Spokes that reach copper surviving the clearances and minimum width.
        let spokes: Vec<_> = thermal.iter().flat_map(|(footprint, pad)| self.spokes(zone, footprint, pad)).collect();
        if !spokes.is_empty() {
            let test = self.prune(&poly::difference(&fill, &holes), zone);
            for (index, (shape, test_point, outline)) in spokes.iter().enumerate() {
                let reaches = poly::shapes_contain(&test, *test_point)
                    || spokes.iter().enumerate().any(|(other, (_, other_point, other_outline))| {
                        other != index
                            && poly::contour_contains(other_outline, *test_point)
                            && poly::contour_contains(outline, *other_point)
                    });
                if reaches {
                    fill.extend(shape.iter().cloned());
                }
            }
            fill = poly::union(&fill);
        }
        fill = poly::difference(&fill, &holes);
        fill = self.prune(&fill, zone);
        let mut trim = holes;
        for (_, pad) in &thermal {
            trim.extend(self.hole_shape(pad, 0.0));
        }
        let fill = poly::difference(&poly::intersection(&fill, &extents), &trim);
        (fill, extents)
    }

    /// Step 5: zone-to-zone knockouts on top of the first fill.
    fn knock_zones(
        &self,
        zone_index: usize,
        layer: usize,
        first: &Shapes,
        extents: &Shapes,
        fills: &IndexMap<(usize, usize), Shapes>,
        same_net_fills: bool,
    ) -> Shapes {
        let zone = &self.board.zones[zone_index];
        let area = shapes_bounds(extents).map_or((0, 0, 0, 0), |b| grow(b, self.worst + zone.min_thickness));
        let clearances = self.zone_clearances(zone_index, layer, fills, area);
        let mut fill = first.clone();
        if !clearances.is_empty() {
            fill = poly::difference(&fill, &clearances);
            let pruned = self.prune(&fill, zone);
            fill = poly::intersection(&pruned, &fill);
        }
        // Higher-priority same-net zones govern their own area: their outline
        // on the first pass, their actual fill when refilling.
        let mut same: Shapes = Vec::new();
        for &other in &self.order {
            let other_zone = &self.board.zones[other];
            if other == zone_index
                || other_zone.net != zone.net
                || other_zone.priority <= zone.priority
                || !other_zone.layers.contains(&layer)
            {
                continue;
            }
            if same_net_fills {
                same.extend(fills.get(&(other, layer)).cloned().unwrap_or_default());
            } else {
                same.extend(self.outline(other_zone));
            }
        }
        if !same.is_empty() {
            fill = poly::difference(&fill, &same);
        }
        let noise = self.max_error * self.max_error;
        fill.retain(|shape| poly::shape_area(shape) >= noise);
        fill
    }
}

/// The board interior from its Edge.Cuts outline (even-odd, so cutouts are holes).
pub(crate) fn board_area(board: &Board, max_error: f64) -> Result<Shapes> {
    let mut contours: Vec<poly::Contour> = Vec::new();
    let mut open: Vec<Vec<(f64, f64)>> = Vec::new();
    for drawing in board.drawings.iter().filter(|drawing| drawing.layer == Layer::EdgeCuts) {
        match &drawing.shape {
            Shape::Polygon(path) => contours.push(int_contour(&path_points(path, max_error))),
            Shape::Circle { center, radius } => {
                contours.extend(poly::circle(*center, *radius as f64, max_error).remove(0))
            }
            Shape::Line { start, end } => {
                open.push(vec![(start.x as f64, start.y as f64), (end.x as f64, end.y as f64)])
            }
            Shape::Arc { start, mid, end } => open.push(flatten_arc(*start, *mid, *end, max_error)),
        }
    }
    // Chain open segments into closed loops by matching ends (within 1 um).
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() <= 1_000.0 && (a.1 - b.1).abs() <= 1_000.0;
    while let Some(mut chain) = open.pop() {
        loop {
            let end = *chain.last().expect("a point");
            if chain.len() > 2 && near(end, chain[0]) {
                break;
            }
            let Some(index) = open.iter().position(|piece| near(piece[0], end) || near(*piece.last().unwrap(), end))
            else {
                return Err(format!(
                    "the Edge.Cuts outline is not closed near ({:.4}, {:.4}) mm",
                    end.0 / 1e6,
                    end.1 / 1e6
                ));
            };
            let mut piece = open.swap_remove(index);
            if !near(piece[0], end) {
                piece.reverse();
            }
            chain.extend(piece.into_iter().skip(1));
        }
        chain.pop();
        contours.push(int_contour(&chain));
    }
    if contours.is_empty() {
        return Err("the board has no Edge.Cuts outline".to_owned());
    }
    Ok(poly::even_odd(&contours))
}

// ---------------------------------------------------------------------------
// islands

/// Union-find over node indices.
pub(crate) struct Sets(pub(crate) Vec<usize>);

impl Sets {
    pub(crate) fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }

    pub(crate) fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

fn near_shape(shape: &[poly::Contour], p: (f64, f64), tolerance: f64) -> bool {
    poly::shape_contains(shape, p)
        || shape.iter().any(|contour| {
            (0..contour.len()).any(|i| {
                let (a, b) = (contour[i], contour[(i + 1) % contour.len()]);
                segment_distance(p, (a.x as f64, a.y as f64), (b.x as f64, b.y as f64)) <= tolerance
            })
        })
}

/// For each fill shape, whether its connected copper reaches a pad of the net.
fn connected_to_pads(filler: &Filler, fills: &IndexMap<(usize, usize), Shapes>) -> IndexMap<(usize, usize), Vec<bool>> {
    let board = filler.board;
    let mut result = IndexMap::new();
    let mut nets: Vec<&String> = fills.keys().filter_map(|(zone, _)| board.zones[*zone].net.as_ref()).collect();
    nets.sort();
    nets.dedup();
    for net in nets {
        // Nodes: fill shapes, then pads, vias and tracks of the net.
        let mut shapes: Vec<(usize, usize, usize, Bounds)> = Vec::new();
        for (&(zone, layer), fill) in fills {
            if board.zones[zone].net.as_ref() == Some(net) {
                for (index, shape) in fill.iter().enumerate() {
                    shapes.push((zone, layer, index, poly::bounds(shape)));
                }
            }
        }
        let pads: Vec<&Pad> = board.pads().map(|(_, pad)| pad).filter(|pad| pad.net.as_ref() == Some(net)).collect();
        let vias: Vec<_> = board.vias.iter().filter(|via| via.net.as_ref() == Some(net)).collect();
        let tracks: Vec<_> = board.tracks.iter().filter(|track| track.net.as_ref() == Some(net)).collect();
        let (s0, p0, v0, t0) = (0, shapes.len(), shapes.len() + pads.len(), shapes.len() + pads.len() + vias.len());
        let mut sets = Sets((0..t0 + tracks.len()).collect());
        let pad_shapes: Vec<Shapes> = pads.iter().map(|pad| filler.pad_shape(pad, 0.0)).collect();
        let fill_shape = |zone: usize, layer: usize, index: usize| &fills[&(zone, layer)][index];
        for (si, &(zone, layer, index, bounds)) in shapes.iter().enumerate() {
            let shape = fill_shape(zone, layer, index);
            let as_shapes: Shapes = vec![shape.clone()];
            for (pi, pad) in pads.iter().enumerate() {
                let size = pad.width.max(pad.height);
                if pad.layers.has(Layer::Copper(layer))
                    && overlaps(bounds, point_bounds(pad.center, size))
                    && (poly::shape_contains(shape, (pad.center.x as f64, pad.center.y as f64))
                        || !poly::intersection(&pad_shapes[pi], &as_shapes).is_empty())
                {
                    sets.join(s0 + si, p0 + pi);
                }
            }
            for (vi, via) in vias.iter().enumerate() {
                let point = (via.center.x as f64, via.center.y as f64);
                if overlaps(bounds, point_bounds(via.center, via.size))
                    && near_shape(shape, point, via.size as f64 / 2.0)
                {
                    sets.join(s0 + si, v0 + vi);
                }
            }
            for (ti, track) in tracks.iter().enumerate().filter(|(_, track)| track.layer == layer) {
                let ends = match track.path {
                    TrackPath::Line { start, end } | TrackPath::Arc { start, end, .. } => [start, end],
                };
                if ends.iter().any(|end| {
                    overlaps(bounds, point_bounds(*end, track.width))
                        && near_shape(shape, (end.x as f64, end.y as f64), track.width as f64 / 2.0)
                }) {
                    sets.join(s0 + si, t0 + ti);
                }
            }
            for (sj, &(other_zone, other_layer, other_index, other_bounds)) in shapes.iter().enumerate().skip(si + 1) {
                if other_layer == layer && overlaps(bounds, other_bounds) {
                    let other: Shapes = vec![fill_shape(other_zone, other_layer, other_index).clone()];
                    if !poly::intersection(&as_shapes, &other).is_empty() {
                        sets.join(s0 + si, s0 + sj);
                    }
                }
            }
        }
        // Copper connections among pads, vias and tracks.
        for (ti, track) in tracks.iter().enumerate() {
            let (start, end) = match track.path {
                TrackPath::Line { start, end } | TrackPath::Arc { start, end, .. } => (start, end),
            };
            let ends = [(start.x as f64, start.y as f64), (end.x as f64, end.y as f64)];
            for (pi, pad) in pads.iter().enumerate() {
                if pad.layers.has(Layer::Copper(track.layer))
                    && ends.iter().any(|&p| near_shape(&pad_shapes[pi][0], p, track.width as f64 / 2.0))
                {
                    sets.join(t0 + ti, p0 + pi);
                }
            }
            for (vi, via) in vias.iter().enumerate() {
                let centre = (via.center.x as f64, via.center.y as f64);
                if ends.iter().any(|p| (p.0 - centre.0).hypot(p.1 - centre.1) <= (via.size + track.width) as f64 / 2.0)
                {
                    sets.join(t0 + ti, v0 + vi);
                }
            }
            for (tj, other) in tracks.iter().enumerate().skip(ti + 1) {
                if other.layer != track.layer {
                    continue;
                }
                let (os, oe) = match other.path {
                    TrackPath::Line { start, end } | TrackPath::Arc { start, end, .. } => (start, end),
                };
                let (a, b) = ((os.x as f64, os.y as f64), (oe.x as f64, oe.y as f64));
                let reach = (track.width.max(other.width)) as f64 / 2.0;
                if ends.iter().any(|&p| segment_distance(p, a, b) <= reach)
                    || [a, b].iter().any(|&p| segment_distance(p, ends[0], ends[1]) <= reach)
                {
                    sets.join(t0 + ti, t0 + tj);
                }
            }
        }
        for (vi, via) in vias.iter().enumerate() {
            for (pi, pad_shape) in pad_shapes.iter().enumerate() {
                if near_shape(&pad_shape[0], (via.center.x as f64, via.center.y as f64), via.size as f64 / 2.0) {
                    sets.join(v0 + vi, p0 + pi);
                }
            }
        }
        let mut with_pad = vec![false; t0 + tracks.len()];
        for pi in 0..pads.len() {
            let root = sets.find(p0 + pi);
            with_pad[root] = true;
        }
        for (si, &(zone, layer, index, _)) in shapes.iter().enumerate() {
            let root = sets.find(s0 + si);
            let entry: &mut Vec<bool> =
                result.entry((zone, layer)).or_insert_with(|| vec![false; fills[&(zone, layer)].len()]);
            entry[index] = with_pad[root];
        }
    }
    result
}

/// Remove islands by each zone's mode; the number removed per zone-layer.
fn remove_islands(filler: &Filler, fills: &mut IndexMap<(usize, usize), Shapes>) -> IndexMap<(usize, usize), usize> {
    let connected = connected_to_pads(filler, fills);
    let mut removed = IndexMap::new();
    let zones: Vec<usize> = {
        let mut zones: Vec<usize> = fills.keys().map(|(zone, _)| *zone).collect();
        zones.dedup();
        zones
    };
    for zone_index in zones {
        let zone = &filler.board.zones[zone_index];
        let layers: Vec<usize> = fills.keys().filter(|(z, _)| *z == zone_index).map(|(_, layer)| *layer).collect();
        // Islands per layer. A zone isolated on every layer (empty layers
        // count as isolated, as in KiCad) is an unconnected pour: kept as-is.
        let isolated: Vec<(usize, Vec<bool>)> = layers
            .iter()
            .map(|&layer| {
                let flags = connected.get(&(zone_index, layer)).map_or_else(
                    || vec![true; fills[&(zone_index, layer)].len()],
                    |flags| flags.iter().map(|c| !c).collect(),
                );
                (layer, flags)
            })
            .collect();
        if isolated.iter().all(|(_, flags)| flags.iter().all(|&i| i)) || zone.island_mode == 1 {
            continue;
        }
        for (layer, flags) in isolated {
            let shapes = fills.get_mut(&(zone_index, layer)).expect("a fill");
            let before = shapes.len();
            let mut index = 0;
            shapes.retain(|shape| {
                let island = flags[index];
                index += 1;
                !(island && (zone.island_mode == 0 || poly::shape_area(shape) < zone.island_area_min))
            });
            if shapes.len() != before {
                removed.insert((zone_index, layer), before - shapes.len());
            }
        }
    }
    removed
}

/// Fill every copper zone of the board.
pub fn fill_zones(board: &Board, rules: &Rules) -> Result<Vec<ZoneFill>> {
    let filler = Filler::new(board, rules)?;
    let mut first: IndexMap<(usize, usize), (Shapes, Shapes)> = IndexMap::new();
    let mut fills: IndexMap<(usize, usize), Shapes> = IndexMap::new();
    for &zone in &filler.order {
        for &layer in &board.zones[zone].layers {
            let (fill, extents) = filler.first_fill(zone, layer);
            let knocked = filler.knock_zones(zone, layer, &fill, &extents, &fills, false);
            first.insert((zone, layer), (fill, extents));
            fills.insert((zone, layer), knocked);
        }
    }
    let mut removed = remove_islands(&filler, &mut fills);
    let mut totals: IndexMap<(usize, usize), usize> = removed.clone();
    // Refill waves from the first fills against a snapshot of the current
    // fills, until nothing changes.
    for _ in 0..MAX_REFILL_WAVES {
        let snapshot = fills.clone();
        let mut next: IndexMap<(usize, usize), Shapes> = IndexMap::new();
        for &zone in &filler.order {
            for &layer in &board.zones[zone].layers {
                let (fill, extents) = &first[&(zone, layer)];
                next.insert((zone, layer), filler.knock_zones(zone, layer, fill, extents, &snapshot, true));
            }
        }
        removed = remove_islands(&filler, &mut next);
        for (key, count) in &removed {
            *totals.entry(*key).or_default() += count;
        }
        let stable = next == snapshot;
        fills = next;
        if stable {
            break;
        }
    }
    Ok(fills
        .into_iter()
        .map(|((zone, layer), shapes)| ZoneFill {
            zone,
            layer,
            shapes,
            islands_removed: totals.get(&(zone, layer)).copied().unwrap_or(0),
        })
        .collect())
}

/// The board's zone fills as `filled_polygon` contours: `(zone, layer, contour)`.
pub fn fractured(fills: &[ZoneFill]) -> Vec<(usize, usize, Path)> {
    let mut result = Vec::new();
    for fill in fills {
        for shape in &fill.shapes {
            let contour = poly::fracture(shape);
            if contour.len() < 3 {
                continue;
            }
            let points: Vec<Point> = contour.iter().map(|p| Point::new(p.x, p.y)).collect();
            result.push((fill.zone, fill.layer, Path::polygon(&points)));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::parse_board;

    const PROJECT: &str = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.2,
      "min_copper_edge_clearance": 0.5, "min_hole_clearance": 0.25}}},
      "net_settings": {"classes": [{"name": "Default", "clearance": 0.2}]}}"#;

    /// A 20 x 20 mm board, a GND pour over all of it, a GND pad at (5, 5),
    /// SIG tracks across the board at y = 10 and y = 17, and a GND
    /// through-hole pad at (15, 15) joined by thermal reliefs. The strip
    /// beyond y = 17 reaches no GND pad.
    fn board(connection: &str) -> Board {
        let text = format!(
            "(kicad_pcb (version 20260206)
  (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (25 \"Edge.Cuts\" user))
  (footprint \"L:P\" (layer \"F.Cu\") (at 5 5)
    (property \"Reference\" \"P1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (net \"GND\")))
  (footprint \"L:T\" (layer \"F.Cu\") (at 15 15)
    (property \"Reference\" \"T1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" thru_hole circle (at 0 0) (size 2 2) (drill 1) (layers \"*.Cu\") (net \"GND\")))
  (gr_rect (start 0 0) (end 20 20) (stroke (width 0.1) (type default)) (fill no) (layer \"Edge.Cuts\"))
  (segment (start 0 10) (end 20 10) (width 0.3) (layer \"F.Cu\") (net \"SIG\"))
  (segment (start 0 17) (end 20 17) (width 0.3) (layer \"F.Cu\") (net \"SIG\"))
  (zone (net \"GND\") (layer \"F.Cu\") (uuid \"a\") (name \"gnd\") (connect_pads{connection} (clearance 0.2))
    (min_thickness 0.25) (filled_areas_thickness no)
    (fill yes (thermal_gap 0.4) (thermal_bridge_width 0.4) (island_removal_mode 0))
    (polygon (pts (xy -1 -1) (xy 21 -1) (xy 21 21) (xy -1 21)))))"
        );
        parse_board(&text, "test").unwrap()
    }

    fn filled(connection: &str) -> Shapes {
        let rules = Rules::load(PROJECT, None).unwrap();
        let fills = fill_zones(&board(connection), &rules).unwrap();
        assert_eq!(fills.len(), 1);
        fills.into_iter().next().unwrap().shapes
    }

    fn at(shapes: &Shapes, x: f64, y: f64) -> bool {
        poly::shapes_contain(shapes, (x * 1e6, y * 1e6))
    }

    #[test]
    fn clearances_edges_and_islands() {
        let fill = filled(" yes");
        // Both regions with a GND pad stay; the strip beyond y = 17 is an
        // island and is removed.
        assert_eq!(fill.len(), 2);
        assert!(at(&fill, 5.0, 8.0) && at(&fill, 5.0, 12.0) && !at(&fill, 5.0, 18.5));
        // Track edge at y = 9.85; clearance 0.2 (+0.5 um margin).
        assert!(!at(&fill, 5.0, 9.66));
        assert!(at(&fill, 5.0, 9.6));
        // Board edge clearance 0.5 from the edge line, ignoring its width.
        assert!(!at(&fill, 0.45, 5.0) && at(&fill, 0.6, 5.0));
        // Solid connection: the pads are covered.
        assert!(at(&fill, 5.0, 5.0));
    }

    #[test]
    fn thermal_reliefs_keep_spokes() {
        let fill = filled("");
        // Relief gap 0.4 around the 2 mm pad; the round pad's spokes run at 45 degrees.
        let r = 1.0 + 0.2;
        assert!(!at(&fill, 15.0 + r, 15.0), "relief on the axis");
        let d = r / std::f64::consts::SQRT_2;
        assert!(at(&fill, 15.0 + d, 15.0 + d), "spoke on the diagonal");
        assert!(!at(&fill, 15.0, 15.0), "the hole itself is not filled");
    }
}

//! Traced copper paths for `copper emi-check`: the shortest path through one
//! net's routed copper between two pads.
//!
//! The graph is built from the net's tracks, arcs, vias, pads and
//! polygons/zones in integer nanometres:
//!
//! * nodes are `(layer, point)` pairs: track and arc end points, via positions
//!   on every layer of their span, pad centres on every layer of the pad;
//! * a track is cut at every other conductor's end point, via centre or pad
//!   centre that lies within its copper (so a T junction is a node); an arc
//!   is one edge between its end points (a junction in the middle of an arc is
//!   not recognised);
//! * a via joins its layers in span order, the edge weighted by the stack-up
//!   distance of the two layers;
//! * a pad centre joins every conductor node on the same layer that lies inside
//!   the pad shape by a straight stub (a track that ends on the pad centre
//!   needs none);
//! * a polygon or zone joins every node it contains on its layer to every
//!   other with a straight-line edge, flagged as a plane shortcut: the
//!   current spreads in the sheet, so a straight line between the nodes is the
//!   screening estimate. Fill, holes added by other nets and slots are not
//!   known here.
//!
//! The graph is deterministic: nodes are created in input order and ties in
//! the shortest-path search break by node number.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use copper_physical::{CopperLayer, Point, RoundedConvexShape};

fn pt(p: Point) -> [f64; 2] {
    [p.x_nm as f64, p.y_nm as f64]
}

fn dist(a: Point, b: Point) -> f64 {
    let (a, b) = (pt(a), pt(b));
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (p, a, b) = (pt(p), pt(a), pt(b));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return (p[0] - a[0]).hypot(p[1] - a[1]);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_squared).clamp(0.0, 1.0);
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

/// Parameter of the projection of `p` on `a -> b` (0 at `a`, 1 at `b`, not clamped).
fn segment_parameter(p: Point, a: Point, b: Point) -> f64 {
    let (p, a, b) = (pt(p), pt(a), pt(b));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return 0.0;
    }
    ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_squared
}

/// Whether `p` lies inside the convex rounded shape (its copper).
pub fn inside_shape(p: Point, shape: &RoundedConvexShape) -> bool {
    let radius = shape.radius_nm as f64;
    let spine = &shape.spine;
    match spine.len() {
        0 => false,
        1 => dist(p, spine[0]) <= radius,
        2 => segment_distance(p, spine[0], spine[1]) <= radius,
        n => {
            let (mut positive, mut negative) = (false, false);
            let mut edge_distance = f64::INFINITY;
            for i in 0..n {
                let (a, b) = (spine[i], spine[(i + 1) % n]);
                let cross = (b.x_nm - a.x_nm) as f64 * (p.y_nm - a.y_nm) as f64
                    - (b.y_nm - a.y_nm) as f64 * (p.x_nm - a.x_nm) as f64;
                positive |= cross > 0.0;
                negative |= cross < 0.0;
                edge_distance = edge_distance.min(segment_distance(p, a, b));
            }
            !(positive && negative) || edge_distance <= radius
        }
    }
}

/// Even-odd containment of `p` in the polygon `ring` (boundary counts as inside by tolerance of 1 nm).
pub fn inside_ring(p: Point, ring: &[Point]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if segment_distance(p, a, b) <= 1.0 {
            return true;
        }
        let (ay, by) = (a.y_nm as f64, b.y_nm as f64);
        if (ay > p.y_nm as f64) != (by > p.y_nm as f64) {
            let x = a.x_nm as f64 + (p.y_nm as f64 - ay) / (by - ay) * (b.x_nm - a.x_nm) as f64;
            if (p.x_nm as f64) < x {
                inside = !inside;
            }
        }
    }
    inside
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// A pad of the traced net.
#[derive(Clone, Debug)]
pub struct TracePad {
    pub shape: RoundedConvexShape,
    /// Copper layers the pad exists on (one for an SMD pad, all for a through-hole pad).
    pub layers: Vec<CopperLayer>,
    pub centre: Point,
    /// Hole diameter of a through-hole pad, nm (its barrel joins the layers).
    pub drill_nm: Option<i64>,
}

impl TracePad {
    /// The centre is the mean of the spine points.
    pub fn new(shape: RoundedConvexShape, layers: Vec<CopperLayer>, drill_nm: Option<i64>) -> Self {
        let n = shape.spine.len().max(1) as i64;
        let centre = Point {
            x_nm: shape.spine.iter().map(|p| p.x_nm).sum::<i64>() / n,
            y_nm: shape.spine.iter().map(|p| p.y_nm).sum::<i64>() / n,
        };
        TracePad { shape, layers, centre, drill_nm }
    }

    /// The shorter side of the pad, nm.
    pub fn small_dimension_nm(&self) -> i64 {
        let spine = &self.shape.spine;
        let side = if spine.len() < 3 {
            0.0
        } else {
            (0..spine.len()).map(|i| dist(spine[i], spine[(i + 1) % spine.len()])).fold(f64::INFINITY, f64::min)
        };
        (side + 2.0 * self.shape.radius_nm as f64).round() as i64
    }
}

#[derive(Clone, Debug)]
pub struct TraceTrack {
    pub start: Point,
    pub end: Point,
    pub width_nm: i64,
    pub layer: CopperLayer,
}

#[derive(Clone, Debug)]
pub struct TraceArc {
    pub start: Point,
    pub end: Point,
    /// The inscribed chord chain from `start` to `end`.
    pub chords: Vec<Point>,
    pub length_nm: i64,
    pub width_nm: i64,
    pub layer: CopperLayer,
}

#[derive(Clone, Debug)]
pub struct TraceVia {
    pub position: Point,
    pub size_nm: i64,
    pub drill_nm: i64,
    /// The copper layers of the span, in stack-up order.
    pub layers: Vec<CopperLayer>,
}

#[derive(Clone, Debug)]
pub struct TracePolygon {
    pub layers: Vec<CopperLayer>,
    pub outer: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
}

/// The copper of one net.
#[derive(Clone, Debug, Default)]
pub struct TraceCopper {
    pub tracks: Vec<TraceTrack>,
    pub arcs: Vec<TraceArc>,
    pub vias: Vec<TraceVia>,
    pub polygons: Vec<TracePolygon>,
}

impl TraceCopper {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.arcs.is_empty() && self.vias.is_empty() && self.polygons.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Track,
    Arc,
    Via,
    PadStub,
    PlaneShortcut,
}

#[derive(Clone, Debug)]
struct Edge {
    a: usize,
    b: usize,
    kind: StepKind,
    width_nm: Option<i64>,
    drill_nm: Option<i64>,
    weight: f64,
    /// Planar points from `a` to `b`.
    polyline: Vec<Point>,
}

/// One edge of a traced path, in the direction of travel.
#[derive(Clone, Debug)]
pub struct Step {
    pub kind: StepKind,
    pub layer: CopperLayer,
    /// The layer reached: differs from `layer` only for a via.
    pub to_layer: CopperLayer,
    pub from: Point,
    pub to: Point,
    /// Copper width: track and arc width, pad small dimension for a stub, `None` for vias and shortcuts.
    pub width_nm: Option<i64>,
    pub drill_nm: Option<i64>,
    pub length_nm: f64,
    /// Planar points from `from` to `to` (arcs: the chord chain).
    pub polyline: Vec<Point>,
}

#[derive(Clone, Debug, Default)]
pub struct TracedPath {
    pub steps: Vec<Step>,
}

impl TracedPath {
    /// Centre-line length including via barrels, nm.
    pub fn length_nm(&self) -> f64 {
        self.steps.iter().map(|s| s.length_nm).sum()
    }

    pub fn via_count(&self) -> usize {
        self.steps.iter().filter(|s| s.kind == StepKind::Via).count()
    }

    /// Layer changes along the path (every via or barrel step).
    pub fn layer_changes(&self) -> usize {
        self.steps.iter().filter(|s| s.layer != s.to_layer).count()
    }

    /// The narrowest track or arc width, nm; pad stubs, vias and shortcuts do not count.
    pub fn narrowest_width_nm(&self) -> Option<i64> {
        self.steps.iter().filter(|s| matches!(s.kind, StepKind::Track | StepKind::Arc)).filter_map(|s| s.width_nm).min()
    }

    pub fn plane_shortcut(&self) -> bool {
        self.steps.iter().any(|s| s.kind == StepKind::PlaneShortcut)
    }

    /// The layers visited, in order of first use.
    pub fn layers(&self) -> Vec<CopperLayer> {
        let mut layers: Vec<CopperLayer> = Vec::new();
        for step in &self.steps {
            for layer in [step.layer, step.to_layer] {
                if !layers.contains(&layer) {
                    layers.push(layer);
                }
            }
        }
        layers
    }

    /// The planar polyline from the first to the last point, vias contributing their position once.
    pub fn points(&self) -> Vec<Point> {
        let mut points: Vec<Point> = Vec::new();
        for step in &self.steps {
            for p in &step.polyline {
                if points.last() != Some(p) {
                    points.push(*p);
                }
            }
        }
        points
    }
}

/// The path graph of one net.
pub struct TraceGraph {
    layers: Vec<(CopperLayer, i64)>,
    nodes: Vec<(usize, Point)>,
    index: BTreeMap<(usize, i64, i64), usize>,
    edges: Vec<Edge>,
    adjacency: Vec<Vec<usize>>,
    pad_nodes: Vec<Vec<usize>>,
}

impl TraceGraph {
    /// Build the graph. `layers` lists the board's copper layers in stack-up
    /// order with their depth in nm (via edges are weighted by depth difference).
    pub fn build(pads: &[TracePad], copper: &TraceCopper, layers: &[(CopperLayer, i64)]) -> Self {
        let mut graph = TraceGraph {
            layers: layers.to_vec(),
            nodes: Vec::new(),
            index: BTreeMap::new(),
            edges: Vec::new(),
            adjacency: Vec::new(),
            pad_nodes: Vec::new(),
        };
        graph.add_conductors(pads, copper);
        graph.add_pads(pads);
        graph.add_polygons(copper);
        graph
    }

    fn layer_index(&self, layer: CopperLayer) -> Option<usize> {
        self.layers.iter().position(|(l, _)| *l == layer)
    }

    fn node(&mut self, layer: usize, at: Point) -> usize {
        if let Some(&id) = self.index.get(&(layer, at.x_nm, at.y_nm)) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push((layer, at));
        self.adjacency.push(Vec::new());
        self.index.insert((layer, at.x_nm, at.y_nm), id);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn edge(
        &mut self,
        a: usize,
        b: usize,
        kind: StepKind,
        width_nm: Option<i64>,
        drill_nm: Option<i64>,
        weight: f64,
        polyline: Vec<Point>,
    ) {
        if a == b {
            return;
        }
        let id = self.edges.len();
        self.edges.push(Edge { a, b, kind, width_nm, drill_nm, weight, polyline });
        self.adjacency[a].push(id);
        self.adjacency[b].push(id);
    }

    fn add_conductors(&mut self, pads: &[TracePad], copper: &TraceCopper) {
        // Key points that cut tracks: (layer, point, extra reach in nm).
        let mut keys: Vec<(usize, Point, f64)> = Vec::new();
        for track in &copper.tracks {
            if let Some(l) = self.layer_index(track.layer) {
                keys.push((l, track.start, 0.0));
                keys.push((l, track.end, 0.0));
            }
        }
        for arc in &copper.arcs {
            if let Some(l) = self.layer_index(arc.layer) {
                keys.push((l, arc.start, 0.0));
                keys.push((l, arc.end, 0.0));
            }
        }
        for via in &copper.vias {
            for layer in &via.layers {
                if let Some(l) = self.layer_index(*layer) {
                    keys.push((l, via.position, via.size_nm as f64 / 2.0));
                }
            }
        }
        for pad in pads {
            for layer in &pad.layers {
                if let Some(l) = self.layer_index(*layer) {
                    keys.push((l, pad.centre, 0.0));
                }
            }
        }
        for track in &copper.tracks {
            let Some(l) = self.layer_index(track.layer) else { continue };
            if track.start == track.end {
                continue;
            }
            let half = track.width_nm as f64 / 2.0;
            let length = dist(track.start, track.end);
            // Only a key point whose projection falls inside the track cuts it: a point beyond an end
            // would fold the track back on itself (the track must reach the via or pad centre).
            let mut cuts: Vec<(f64, Point)> = keys
                .iter()
                .filter(|(kl, at, reach)| {
                    let t = segment_parameter(*at, track.start, track.end);
                    *kl == l
                        && *at != track.start
                        && *at != track.end
                        && t * length > 1.0
                        && (1.0 - t) * length > 1.0
                        && segment_distance(*at, track.start, track.end) <= half + reach
                })
                .map(|(_, at, _)| (segment_parameter(*at, track.start, track.end), *at))
                .collect();
            cuts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            cuts.dedup_by(|a, b| a.1 == b.1);
            let mut chain = vec![track.start];
            chain.extend(cuts.into_iter().map(|(_, at)| at));
            chain.push(track.end);
            for pair in chain.windows(2) {
                let (a, b) = (self.node(l, pair[0]), self.node(l, pair[1]));
                self.edge(
                    a,
                    b,
                    StepKind::Track,
                    Some(track.width_nm),
                    None,
                    dist(pair[0], pair[1]),
                    vec![pair[0], pair[1]],
                );
            }
        }
        for arc in &copper.arcs {
            let Some(l) = self.layer_index(arc.layer) else { continue };
            let (a, b) = (self.node(l, arc.start), self.node(l, arc.end));
            let chords = if arc.chords.len() >= 2 { arc.chords.clone() } else { vec![arc.start, arc.end] };
            self.edge(a, b, StepKind::Arc, Some(arc.width_nm), None, arc.length_nm as f64, chords);
        }
        for via in &copper.vias {
            let layer_ids: Vec<usize> = via.layers.iter().filter_map(|layer| self.layer_index(*layer)).collect();
            let ids: Vec<(usize, usize)> = layer_ids.into_iter().map(|l| (l, self.node(l, via.position))).collect();
            for pair in ids.windows(2) {
                let height = (self.layers[pair[0].0].1 - self.layers[pair[1].0].1).abs() as f64;
                self.edge(pair[0].1, pair[1].1, StepKind::Via, None, Some(via.drill_nm), height, vec![via.position]);
            }
        }
    }

    fn add_pads(&mut self, pads: &[TracePad]) {
        let conductors: Vec<usize> = (0..self.nodes.len()).collect();
        for pad in pads {
            let mut ids = Vec::new();
            for layer in &pad.layers {
                let Some(l) = self.layer_index(*layer) else { continue };
                let centre = self.node(l, pad.centre);
                ids.push((l, centre));
                for &other in &conductors {
                    let (ol, at) = self.nodes[other];
                    if ol == l && other != centre && inside_shape(at, &pad.shape) {
                        let width = Some(pad.small_dimension_nm());
                        self.edge(
                            centre,
                            other,
                            StepKind::PadStub,
                            width,
                            None,
                            dist(pad.centre, at),
                            vec![pad.centre, at],
                        );
                    }
                }
            }
            for pair in ids.windows(2) {
                let height = (self.layers[pair[0].0].1 - self.layers[pair[1].0].1).abs() as f64;
                self.edge(pair[0].1, pair[1].1, StepKind::Via, None, pad.drill_nm, height, vec![pad.centre]);
            }
            self.pad_nodes.push(ids.into_iter().map(|(_, id)| id).collect());
        }
    }

    fn add_polygons(&mut self, copper: &TraceCopper) {
        for polygon in &copper.polygons {
            for layer in &polygon.layers {
                let Some(l) = self.layer_index(*layer) else { continue };
                let inside: Vec<usize> = (0..self.nodes.len())
                    .filter(|&id| {
                        let (nl, at) = self.nodes[id];
                        nl == l && inside_ring(at, &polygon.outer) && !polygon.holes.iter().any(|h| inside_ring(at, h))
                    })
                    .collect();
                for (i, &a) in inside.iter().enumerate() {
                    for &b in &inside[i + 1..] {
                        let (pa, pb) = (self.nodes[a].1, self.nodes[b].1);
                        self.edge(a, b, StepKind::PlaneShortcut, None, None, dist(pa, pb), vec![pa, pb]);
                    }
                }
            }
        }
    }

    /// The shortest copper path between two pads (indices into the `pads` given
    /// to [`TraceGraph::build`]), or `None` when no copper joins them.
    pub fn shortest_path(&self, from: usize, to: usize) -> Option<TracedPath> {
        let sources = self.pad_nodes.get(from)?;
        let targets: BTreeSet<usize> = self.pad_nodes.get(to)?.iter().copied().collect();
        let mut best = vec![f64::INFINITY; self.nodes.len()];
        let mut came: Vec<Option<(usize, usize)>> = vec![None; self.nodes.len()];
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
        for &s in sources {
            best[s] = 0.0;
            heap.push(Reverse((0f64.to_bits(), s)));
        }
        let mut reached = None;
        while let Some(Reverse((bits, node))) = heap.pop() {
            let cost = f64::from_bits(bits);
            if cost > best[node] {
                continue;
            }
            if targets.contains(&node) {
                reached = Some(node);
                break;
            }
            for &edge_id in &self.adjacency[node] {
                let edge = &self.edges[edge_id];
                let next = if edge.a == node { edge.b } else { edge.a };
                let candidate = cost + edge.weight;
                if candidate < best[next] {
                    best[next] = candidate;
                    came[next] = Some((node, edge_id));
                    heap.push(Reverse((candidate.to_bits(), next)));
                }
            }
        }
        let mut node = reached?;
        let mut steps = Vec::new();
        while let Some((previous, edge_id)) = came[node] {
            let edge = &self.edges[edge_id];
            let forward = edge.a == previous;
            let mut polyline = edge.polyline.clone();
            if !forward {
                polyline.reverse();
            }
            let (from_layer, to_layer) = (self.layers[self.nodes[previous].0].0, self.layers[self.nodes[node].0].0);
            steps.push(Step {
                kind: edge.kind,
                layer: from_layer,
                to_layer,
                from: self.nodes[previous].1,
                to: self.nodes[node].1,
                width_nm: edge.width_nm,
                drill_nm: edge.drill_nm,
                length_nm: edge.weight,
                polyline,
            });
            node = previous;
        }
        steps.reverse();
        Some(TracedPath { steps })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MM: i64 = 1_000_000;

    fn p(x_mm: f64, y_mm: f64) -> Point {
        Point { x_nm: (x_mm * MM as f64).round() as i64, y_nm: (y_mm * MM as f64).round() as i64 }
    }

    fn square_pad(x: f64, y: f64, layer: CopperLayer) -> TracePad {
        let half = 0.4;
        let spine = vec![p(x - half, y - half), p(x + half, y - half), p(x + half, y + half), p(x - half, y + half)];
        TracePad::new(RoundedConvexShape::new(spine, 0).unwrap(), vec![layer], None)
    }

    fn track(a: Point, b: Point, width_mm: f64, layer: CopperLayer) -> TraceTrack {
        TraceTrack { start: a, end: b, width_nm: (width_mm * MM as f64) as i64, layer }
    }

    fn stack() -> Vec<(CopperLayer, i64)> {
        vec![(CopperLayer::Front, 0), (CopperLayer::In1, 200_000), (CopperLayer::Back, 1_600_000)]
    }

    fn mm(nm: f64) -> f64 {
        nm / MM as f64
    }

    #[test]
    fn a_straight_track_is_one_step_with_its_width() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(5.0, 0.0, CopperLayer::Front)];
        let copper = TraceCopper {
            tracks: vec![track(p(0.0, 0.0), p(5.0, 0.0), 0.3, CopperLayer::Front)],
            ..TraceCopper::default()
        };
        let graph = TraceGraph::build(&pads, &copper, &stack());
        let path = graph.shortest_path(0, 1).expect("a path");
        assert_eq!(path.steps.len(), 1);
        assert!((mm(path.length_nm()) - 5.0).abs() < 1e-9);
        assert_eq!(path.via_count(), 0);
        assert_eq!(path.layer_changes(), 0);
        assert_eq!(path.narrowest_width_nm(), Some(300_000));
        assert!(!path.plane_shortcut());
        assert_eq!(path.points(), vec![p(0.0, 0.0), p(5.0, 0.0)]);
        // The path is symmetric.
        assert!((mm(graph.shortest_path(1, 0).unwrap().length_nm()) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn an_l_shaped_route_follows_both_legs_and_the_narrowest_width_wins() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(3.0, 4.0, CopperLayer::Front)];
        let copper = TraceCopper {
            tracks: vec![
                track(p(0.0, 0.0), p(3.0, 0.0), 0.5, CopperLayer::Front),
                track(p(3.0, 0.0), p(3.0, 4.0), 0.2, CopperLayer::Front),
            ],
            ..TraceCopper::default()
        };
        let path = TraceGraph::build(&pads, &copper, &stack()).shortest_path(0, 1).expect("a path");
        assert!((mm(path.length_nm()) - 7.0).abs() < 1e-9, "the L is 3 + 4 mm, not the 5 mm diagonal");
        assert_eq!(path.points(), vec![p(0.0, 0.0), p(3.0, 0.0), p(3.0, 4.0)]);
        assert_eq!(path.narrowest_width_nm(), Some(200_000));
    }

    #[test]
    fn a_route_through_a_via_counts_the_via_and_the_layer_change() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(5.0, 0.0, CopperLayer::Back)];
        let copper = TraceCopper {
            tracks: vec![
                track(p(0.0, 0.0), p(2.0, 0.0), 0.4, CopperLayer::Front),
                track(p(2.0, 0.0), p(5.0, 0.0), 0.3, CopperLayer::Back),
            ],
            vias: vec![TraceVia {
                position: p(2.0, 0.0),
                size_nm: 600_000,
                drill_nm: 300_000,
                layers: vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::Back],
            }],
            ..TraceCopper::default()
        };
        let path = TraceGraph::build(&pads, &copper, &stack()).shortest_path(0, 1).expect("a path");
        assert_eq!(path.via_count(), 2, "the via is two barrel steps through the inner layer");
        assert_eq!(path.layer_changes(), 2);
        assert_eq!(path.layers(), vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::Back]);
        // 2 mm + 1.6 mm of barrel + 3 mm.
        assert!((mm(path.length_nm()) - 6.6).abs() < 1e-9, "{}", mm(path.length_nm()));
        assert_eq!(path.narrowest_width_nm(), Some(300_000));
        // The planar polyline passes the via position once.
        assert_eq!(path.points(), vec![p(0.0, 0.0), p(2.0, 0.0), p(5.0, 0.0)]);
    }

    #[test]
    fn a_pad_without_copper_to_it_is_unrouted() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(5.0, 0.0, CopperLayer::Front)];
        let copper = TraceCopper {
            tracks: vec![track(p(0.0, 0.0), p(3.0, 0.0), 0.3, CopperLayer::Front)],
            ..TraceCopper::default()
        };
        let graph = TraceGraph::build(&pads, &copper, &stack());
        assert!(graph.shortest_path(0, 1).is_none(), "the track stops 2 mm short of the second pad");
        assert!(TraceGraph::build(&pads, &TraceCopper::default(), &stack()).shortest_path(0, 1).is_none());
    }

    #[test]
    fn a_t_junction_in_the_middle_of_a_track_is_a_node() {
        let pads = [
            square_pad(0.0, 0.0, CopperLayer::Front),
            square_pad(4.0, 0.0, CopperLayer::Front),
            square_pad(2.0, 3.0, CopperLayer::Front),
        ];
        let copper = TraceCopper {
            tracks: vec![
                track(p(0.0, 0.0), p(4.0, 0.0), 0.3, CopperLayer::Front),
                track(p(2.0, 0.0), p(2.0, 3.0), 0.3, CopperLayer::Front),
            ],
            ..TraceCopper::default()
        };
        let graph = TraceGraph::build(&pads, &copper, &stack());
        let path = graph.shortest_path(0, 2).expect("a path");
        assert!((mm(path.length_nm()) - 5.0).abs() < 1e-9);
        assert_eq!(path.steps.len(), 2);
    }

    #[test]
    fn a_pad_stub_joins_a_track_that_ends_inside_the_pad() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(5.0, 0.0, CopperLayer::Front)];
        let copper = TraceCopper {
            tracks: vec![track(p(0.3, 0.0), p(4.7, 0.0), 0.3, CopperLayer::Front)],
            ..TraceCopper::default()
        };
        let path = TraceGraph::build(&pads, &copper, &stack()).shortest_path(0, 1).expect("a path");
        assert_eq!(path.steps.len(), 3);
        assert!((mm(path.length_nm()) - 5.0).abs() < 1e-9);
        assert_eq!(path.narrowest_width_nm(), Some(300_000), "stubs do not count as narrow copper");
    }

    #[test]
    fn a_polygon_joins_its_nodes_with_flagged_straight_edges() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(6.0, 8.0, CopperLayer::Front)];
        let copper = TraceCopper {
            polygons: vec![TracePolygon {
                layers: vec![CopperLayer::Front],
                outer: vec![p(-1.0, -1.0), p(7.0, -1.0), p(7.0, 9.0), p(-1.0, 9.0)],
                holes: Vec::new(),
            }],
            ..TraceCopper::default()
        };
        let path = TraceGraph::build(&pads, &copper, &stack()).shortest_path(0, 1).expect("a path");
        assert!(path.plane_shortcut());
        assert!((mm(path.length_nm()) - 10.0).abs() < 1e-9);
        // A hole over the second pad takes it out of the pour.
        let holed = TraceCopper {
            polygons: vec![TracePolygon {
                layers: vec![CopperLayer::Front],
                outer: vec![p(-1.0, -1.0), p(7.0, -1.0), p(7.0, 9.0), p(-1.0, 9.0)],
                holes: vec![vec![p(5.0, 7.0), p(7.0, 7.0), p(7.0, 9.0), p(5.0, 9.0)]],
            }],
            ..TraceCopper::default()
        };
        assert!(TraceGraph::build(&pads, &holed, &stack()).shortest_path(0, 1).is_none());
    }

    #[test]
    fn an_arc_is_one_edge_with_its_arc_length() {
        let pads = [square_pad(0.0, 0.0, CopperLayer::Front), square_pad(2.0, 2.0, CopperLayer::Front)];
        let copper = TraceCopper {
            arcs: vec![TraceArc {
                start: p(0.0, 0.0),
                end: p(2.0, 2.0),
                chords: vec![p(0.0, 0.0), p(1.4, 0.0), p(2.0, 0.6), p(2.0, 2.0)],
                length_nm: 3_300_000,
                width_nm: 250_000,
                layer: CopperLayer::Front,
            }],
            ..TraceCopper::default()
        };
        let path = TraceGraph::build(&pads, &copper, &stack()).shortest_path(0, 1).expect("a path");
        assert_eq!(path.steps.len(), 1);
        assert!((mm(path.length_nm()) - 3.3).abs() < 1e-9);
        assert_eq!(path.points().len(), 4);
        assert_eq!(path.narrowest_width_nm(), Some(250_000));
    }
}

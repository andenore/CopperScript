//! `DRC-STUB-LENGTH`: a routing rule's `maximum_stub_length` against the
//! routed copper of its net.
//!
//! The copper of a connected net is a graph of junctions joined by stretches
//! of track and arc. Its route ends are the pads, pours and synthesized
//! antenna copper (the terminal junctions). Copper is route copper when it
//! lies on a simple path between two route ends; all other copper is stub
//! copper. A stub is a connected part of the stub copper, it attaches to the
//! route (or to a lone route end) at one junction, and its length is the
//! greatest along-copper shortest distance from that junction to any junction
//! in the stub. Arcs count at their true length and a via barrel does not
//! count. Branches that end on a pad are routes, not stubs. Only nets that
//! are completely connected are measured, since the open net is the finding
//! on a partly routed one.
//!
//! The junctions come from the geometry. Ends that overlap on a layer share
//! a junction; an end that lands on the body of another piece, and a pad or
//! via on a piece's body, split it there; pieces whose centre lines cross
//! are split and joined at the crossing; an end on a via, pad or pour joins
//! that object's junction, as do objects that overlap each other. Copper that
//! overlaps itself needs no separate normalisation: overlapping pieces join at
//! their ends and landings, and the copies between those junctions become
//! parallel stretches, which count once because lengths are shortest
//! distances and a loop is never route copper unless it joins two route ends.
//!
//! Route copper is found with the biconnected components of the graph. In
//! a biconnected block every edge lies on a simple path between any two of its
//! junctions, so a block is route copper exactly when it lies on the
//! block-cut tree path between two route ends. Those blocks are what remains
//! of the block-cut tree after repeatedly dropping leaves that are not route
//! ends.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::Result;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::pyfloat::round_int;
use copper_physical::{CopperLayer, Point};

use crate::connectivity::{
    Candidate, Connectivity, CopperContact, copper_contact_roots, copper_contacts_overlap, explicit_copper_contacts,
    positive_area_overlap,
};
use crate::{Finding, shape};

/// A piece and a vertex (or segment) index within its path.
type Place = (usize, usize);

/// One track, or one arc as its chord chain, with its copper length.
struct Piece {
    layer: CopperLayer,
    path: Vec<Point>,
    /// The position of each path vertex along the piece, from 0 to 1.
    at: Vec<f64>,
    radius_nm: i64,
    length_nm: f64,
    label: String,
}

impl Piece {
    fn new(layer: CopperLayer, path: Vec<Point>, radius_nm: i64, length_nm: f64, label: String) -> Piece {
        let spans: Vec<f64> = path.windows(2).map(|pair| distance(pair[0], pair[1])).collect();
        let total: f64 = spans.iter().sum();
        let mut at = vec![0.0];
        let mut before = 0.0;
        for span in &spans {
            before += span;
            at.push(if total > 0.0 { before / total } else { 0.0 });
        }
        if let Some(last) = at.last_mut() {
            *last = 1.0;
        }
        Piece { layer, path, at, radius_nm, length_nm, label }
    }

    fn segment(&self, segment: usize) -> [Point; 2] {
        [self.path[segment], self.path[segment + 1]]
    }

    fn ends(&self) -> [Point; 2] {
        [self.path[0], self.path[self.path.len() - 1]]
    }

    /// The position along the piece of the point `t` along its `segment`.
    fn position(&self, segment: usize, t: f64) -> f64 {
        self.at[segment] + t * (self.at[segment + 1] - self.at[segment])
    }

    /// The distance from `point` to the path and the position of the nearest
    /// point along it as a fraction of its length.
    fn project(&self, point: Point) -> (f64, f64) {
        let mut best = (f64::INFINITY, 0.0);
        for (index, pair) in self.path.windows(2).enumerate() {
            let (away, t) = project(point, pair[0], pair[1]);
            if away < best.0 {
                best = (away, self.position(index, t));
            }
        }
        best
    }
}

#[derive(Default)]
struct Junctions {
    parent: Vec<usize>,
}

impl Junctions {
    fn add(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
    }

    fn find(&mut self, node: usize) -> usize {
        let mut root = node;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = node;
        while self.parent[current] != root {
            current = std::mem::replace(&mut self.parent[current], root);
        }
        root
    }

    fn union(&mut self, first: usize, second: usize) {
        let (a, b) = (self.find(first), self.find(second));
        self.parent[a.max(b)] = a.min(b);
    }
}

fn xy(point: Point) -> (f64, f64) {
    (point.x_nm as f64, point.y_nm as f64)
}

fn distance(a: Point, b: Point) -> f64 {
    ((b.x_nm - a.x_nm) as f64).hypot((b.y_nm - a.y_nm) as f64)
}

/// The distance from `point` to the segment `a`-`b` and the position of its
/// nearest point along it (0 at `a`, 1 at `b`).
fn project(point: Point, a: Point, b: Point) -> (f64, f64) {
    let (p, a, b) = (xy(point), xy(a), xy(b));
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let squared = dx * dx + dy * dy;
    let t = if squared == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / squared).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).hypot(p.1 - cy), t)
}

/// Whether a contact is a pad, pour or synthesized antenna (a place the net's route ends) rather
/// than track, arc or via copper.
fn is_terminal(key: &str) -> bool {
    key.starts_with("pad:") || key.starts_with("polygon:") || key.starts_with("antenna:")
}

/// The pieces of the net's tracks (antenna copper excluded) and arcs.
fn pieces(board: &PhysicalBoard, net: &str, antenna: &IndexSet<copper_pcb::board::TrackSegment>) -> Vec<Piece> {
    let tracks =
        board.tracks.iter().enumerate().filter(|(_, t)| t.net == net && !antenna.contains(*t)).map(|(index, track)| {
            let length = distance(track.start, track.end);
            Piece::new(track.layer, vec![track.start, track.end], track.width_nm / 2, length, format!("track:{index}"))
        });
    let arcs = board.arcs.iter().enumerate().filter(|(_, a)| a.net == net).map(|(index, arc)| {
        let length = arc.length_nm() as f64;
        Piece::new(arc.layer, arc.chord_points(), arc.width_nm / 2, length, format!("arc:{index}"))
    });
    tracks.chain(arcs).collect()
}

/// The distance from `point` to a segment.
fn project_gap(point: Point, segment: [Point; 2]) -> f64 {
    project(point, segment[0], segment[1]).0
}

/// How far apart the centre lines of two segments are: nothing when they
/// cross, else the least distance of an end to the other segment.
fn segment_gap(a: [Point; 2], b: [Point; 2]) -> f64 {
    if crossing(a[0], a[1], b[0], b[1]).is_some() {
        return 0.0;
    }
    [project_gap(a[0], b), project_gap(a[1], b), project_gap(b[0], a), project_gap(b[1], a)]
        .into_iter()
        .fold(f64::INFINITY, f64::min)
}

/// The positions along segments `a0`-`a1` and `b0`-`b1` where their centre
/// lines cross, if they do.
fn crossing(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<(f64, f64)> {
    let (p, q) = (xy(a0), xy(b0));
    let (r, s) = ((xy(a1).0 - p.0, xy(a1).1 - p.1), (xy(b1).0 - q.0, xy(b1).1 - q.1));
    let cross = |u: (f64, f64), v: (f64, f64)| u.0 * v.1 - u.1 * v.0;
    let denominator = cross(r, s);
    if denominator == 0.0 {
        return None;
    }
    let offset = (q.0 - p.0, q.1 - p.1);
    let (t, u) = (cross(offset, s) / denominator, cross(offset, r) / denominator);
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// The junctions of the pieces: the two ends of each piece and the places
/// where other copper splits it.
#[derive(Default)]
struct Sites {
    junctions: Junctions,
    /// The junction of each end of each piece, `2 * piece` and `2 * piece + 1`.
    ends: Vec<usize>,
    /// The junctions inside each piece, by position along it.
    cuts: Vec<Vec<(f64, usize)>>,
    made: IndexMap<(usize, u64), usize>,
}

impl Sites {
    /// The junction at `position` (0 to 1) along `piece`: an end, or a split
    /// made there once.
    fn at(&mut self, piece: usize, position: f64) -> usize {
        if position <= 0.0 {
            return self.ends[2 * piece];
        }
        if position >= 1.0 {
            return self.ends[2 * piece + 1];
        }
        if let Some(cut) = self.made.get(&(piece, position.to_bits())) {
            return *cut;
        }
        let cut = self.junctions.add();
        self.cuts[piece].push((position, cut));
        self.made.insert((piece, position.to_bits()), cut);
        cut
    }

    /// Joins a vertex of piece `from` to the nearest point of a segment of
    /// piece `onto`. A point within reach of the other piece's own end joins
    /// that end instead of splitting the piece beside it.
    fn touch(&mut self, pieces: &[Piece], from: Place, onto: Place, reach: f64) {
        let (point, [start, end]) = (pieces[from.0].path[from.1], pieces[onto.0].segment(onto.1));
        let (_, t) = project(point, start, end);
        let spot = Point::new(
            start.x_nm + (t * (end.x_nm - start.x_nm) as f64).round() as i64,
            start.y_nm + (t * (end.y_nm - start.y_nm) as f64).round() as i64,
        );
        let [first, last] = pieces[onto.0].ends();
        let there = if distance(spot, first) < reach {
            self.ends[2 * onto.0]
        } else if distance(spot, last) < reach {
            self.ends[2 * onto.0 + 1]
        } else {
            self.at(onto.0, pieces[onto.0].position(onto.1, t))
        };
        let here = self.at(from.0, pieces[from.0].at[from.1]);
        self.junctions.union(here, there);
    }

    /// Joins two pieces on one layer wherever their copper overlaps.
    ///
    /// The segment pairs whose centre lines are closer than `reach` (or cross)
    /// fall into patches of neighbouring segments, and each patch is one
    /// contact: the ends of either piece that lie on the other, else the
    /// crossings of the centre lines, else the closest points. A crossing is
    /// one junction however many chords of an arc lie within reach of it.
    fn join(&mut self, pieces: &[Piece], i: usize, j: usize) {
        let (first, second) = (&pieces[i], &pieces[j]);
        let reach = (first.radius_nm + second.radius_nm) as f64;
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for sa in 0..first.path.len() - 1 {
            for sb in 0..second.path.len() - 1 {
                if segment_gap(first.segment(sa), second.segment(sb)) < reach {
                    pairs.push((sa, sb));
                }
            }
        }
        let mut patches = Junctions::default();
        for _ in &pairs {
            patches.add();
        }
        for (x, p) in pairs.iter().enumerate() {
            for (y, q) in pairs.iter().enumerate().skip(x + 1) {
                if p.0.abs_diff(q.0) <= 1 && p.1.abs_diff(q.1) <= 1 {
                    patches.union(x, y);
                }
            }
        }
        let mut contacts: IndexMap<usize, Vec<(usize, usize)>> = IndexMap::new();
        for (index, pair) in pairs.iter().enumerate() {
            contacts.entry(patches.find(index)).or_default().push(*pair);
        }
        let last = (first.path.len() - 1, second.path.len() - 1);
        for patch in contacts.values() {
            let mut landed = false;
            for &(sa, sb) in patch {
                for k in 0..2 {
                    let (a, b) = (sa + k, sb + k);
                    if (a == 0 || a == last.0) && project_gap(first.path[a], second.segment(sb)) < reach {
                        self.touch(pieces, (i, a), (j, sb), reach);
                        landed = true;
                    }
                    if (b == 0 || b == last.1) && project_gap(second.path[b], first.segment(sa)) < reach {
                        self.touch(pieces, (j, b), (i, sa), reach);
                        landed = true;
                    }
                }
            }
            if landed {
                continue;
            }
            let mut crossed = false;
            for &(sa, sb) in patch {
                let [a0, a1] = first.segment(sa);
                let [b0, b1] = second.segment(sb);
                if let Some((ta, tb)) = crossing(a0, a1, b0, b1) {
                    let (here, there) = (self.at(i, first.position(sa, ta)), self.at(j, second.position(sb, tb)));
                    self.junctions.union(here, there);
                    crossed = true;
                }
            }
            if crossed {
                continue;
            }
            // Close without crossing: join at the closest of the vertices.
            let mut closest: Option<(f64, Place, Place)> = None;
            for &(sa, sb) in patch {
                for k in 0..2 {
                    for (gap, from, onto) in [
                        (project_gap(first.path[sa + k], second.segment(sb)), (i, sa + k), (j, sb)),
                        (project_gap(second.path[sb + k], first.segment(sa)), (j, sb + k), (i, sa)),
                    ] {
                        if closest.is_none_or(|(best, _, _)| gap < best) {
                            closest = Some((gap, from, onto));
                        }
                    }
                }
            }
            if let Some((_, from, onto)) = closest {
                self.touch(pieces, from, onto, reach);
            }
        }
    }
}

/// A stretch of copper between two junctions.
struct Edge {
    ends: [usize; 2],
    length_nm: f64,
    label: String,
}

/// The junction graph of a net: vertices are junctions, edges are stretches
/// of copper between them (parallel edges are possible).
#[derive(Default)]
struct Graph {
    index: IndexMap<usize, usize>,
    /// For each vertex its `(neighbour, edge)` pairs.
    adjacency: Vec<Vec<(usize, usize)>>,
    edges: Vec<GraphEdge>,
    /// Route ends: pads, pours and antenna copper.
    terminal: Vec<bool>,
    /// The via at a vertex, which names a stub that ends in one.
    via: Vec<Option<String>>,
}

struct GraphEdge {
    ends: [usize; 2],
    length_nm: f64,
    label: String,
}

/// The biconnected components of a graph, each as its edges (Tarjan's
/// algorithm, with the parent edge rather than the parent vertex skipped so
/// that parallel edges form a block).
struct Blocks<'a> {
    adjacency: &'a [Vec<(usize, usize)>],
    discovered: Vec<usize>,
    low: Vec<usize>,
    clock: usize,
    stack: Vec<usize>,
    blocks: Vec<Vec<usize>>,
}

impl Blocks<'_> {
    fn visit(&mut self, vertex: usize, parent_edge: Option<usize>) {
        self.clock += 1;
        self.discovered[vertex] = self.clock;
        self.low[vertex] = self.clock;
        for &(next, edge) in &self.adjacency[vertex] {
            if Some(edge) == parent_edge {
                continue;
            }
            if self.discovered[next] == 0 {
                self.stack.push(edge);
                self.visit(next, Some(edge));
                self.low[vertex] = self.low[vertex].min(self.low[next]);
                if self.low[next] >= self.discovered[vertex] {
                    let mut block = Vec::new();
                    while let Some(top) = self.stack.pop() {
                        block.push(top);
                        if top == edge {
                            break;
                        }
                    }
                    self.blocks.push(block);
                }
            } else if self.discovered[next] < self.discovered[vertex] {
                self.stack.push(edge);
                self.low[vertex] = self.low[vertex].min(self.discovered[next]);
            }
        }
    }
}

impl Graph {
    /// The vertex of a junction, added on first use.
    fn vertex(&mut self, junction: usize) -> usize {
        let next = self.index.len();
        let vertex = *self.index.entry(junction).or_insert(next);
        if vertex == next {
            self.adjacency.push(Vec::new());
            self.terminal.push(false);
            self.via.push(None);
        }
        vertex
    }

    /// The connected component of every vertex.
    fn components(&self) -> Vec<usize> {
        let mut component = vec![usize::MAX; self.adjacency.len()];
        for start in 0..component.len() {
            if component[start] != usize::MAX {
                continue;
            }
            let mut pending = vec![start];
            component[start] = start;
            while let Some(vertex) = pending.pop() {
                for &(next, _) in &self.adjacency[vertex] {
                    if component[next] == usize::MAX {
                        component[next] = start;
                        pending.push(next);
                    }
                }
            }
        }
        component
    }

    /// Whether each edge is route copper: it lies on a simple path between
    /// two route ends.
    ///
    /// The vertices and blocks form a forest (the block-cut tree, with its
    /// non-cut vertices as leaves). The route is the part of it between the
    /// route ends: what remains after dropping, again and again, a leaf that
    /// is not a route end. Components with fewer than two route ends have no
    /// route.
    fn route_edges(&self) -> Vec<bool> {
        let count = self.adjacency.len();
        let mut tarjan = Blocks {
            adjacency: &self.adjacency,
            discovered: vec![0; count],
            low: vec![0; count],
            clock: 0,
            stack: Vec::new(),
            blocks: Vec::new(),
        };
        for vertex in 0..count {
            if tarjan.discovered[vertex] == 0 {
                tarjan.visit(vertex, None);
            }
        }
        let blocks = tarjan.blocks;
        let component = self.components();
        let mut ends_in = vec![0usize; count];
        for (vertex, component) in component.iter().enumerate() {
            ends_in[*component] += usize::from(self.terminal[vertex]);
        }
        // Tree nodes: the vertices, then the blocks.
        let mut tree: Vec<Vec<usize>> = vec![Vec::new(); count + blocks.len()];
        for (block, edges) in blocks.iter().enumerate() {
            let members: IndexSet<usize> = edges.iter().flat_map(|edge| self.edges[*edge].ends).collect();
            for vertex in members {
                tree[vertex].push(count + block);
                tree[count + block].push(vertex);
            }
        }
        let mut degree: Vec<usize> = tree.iter().map(Vec::len).collect();
        let mut alive = vec![true; tree.len()];
        for vertex in 0..count {
            alive[vertex] = ends_in[component[vertex]] >= 2;
        }
        for (block, edges) in blocks.iter().enumerate() {
            alive[count + block] = ends_in[component[self.edges[edges[0]].ends[0]]] >= 2;
        }
        let droppable = |node: usize| node >= count || !self.terminal[node];
        let mut pending: Vec<usize> =
            (0..tree.len()).filter(|node| alive[*node] && degree[*node] <= 1 && droppable(*node)).collect();
        while let Some(node) = pending.pop() {
            if !alive[node] {
                continue;
            }
            alive[node] = false;
            for &next in &tree[node] {
                if alive[next] {
                    degree[next] -= 1;
                    if degree[next] <= 1 && droppable(next) {
                        pending.push(next);
                    }
                }
            }
        }
        let mut route = vec![false; self.edges.len()];
        for (block, edges) in blocks.iter().enumerate() {
            for edge in edges {
                route[*edge] = alive[count + block];
            }
        }
        route
    }

    /// The longest stub as `(length, name of its far end)`.
    ///
    /// A stub attaches to the route at one junction, or hangs off a route end
    /// that has no route; its length is the greatest shortest distance, along
    /// stub copper only, from that junction to any point of the stub. In a
    /// loop that point is inside an edge: on an edge of length `L` whose ends
    /// lie `a` and `b` away, the farthest point is `(a + b + L) / 2` away, so
    /// splitting an edge into several never changes the result.
    fn longest_stub(&self) -> Option<(f64, String)> {
        let route = self.route_edges();
        let attached =
            |vertex: usize| self.terminal[vertex] || self.adjacency[vertex].iter().any(|(_, edge)| route[*edge]);
        let mut longest: Option<(f64, String)> = None;
        for source in (0..self.adjacency.len()).filter(|vertex| attached(*vertex)) {
            let mut best = vec![f64::INFINITY; self.adjacency.len()];
            let mut arrived: Vec<Option<usize>> = vec![None; self.adjacency.len()];
            let mut queue = BinaryHeap::from([Reverse((0.0f64.to_bits(), source))]);
            best[source] = 0.0;
            while let Some(Reverse((bits, vertex))) = queue.pop() {
                let reached = f64::from_bits(bits);
                if reached > best[vertex] {
                    continue;
                }
                if vertex != source {
                    if attached(vertex) {
                        continue;
                    }
                    let name = self.via[vertex]
                        .clone()
                        .or_else(|| arrived[vertex].map(|edge| self.edges[edge].label.clone()))
                        .unwrap_or_default();
                    if longest.as_ref().is_none_or(|(length, _)| reached > *length) {
                        longest = Some((reached, name));
                    }
                }
                for &(next, edge) in self.adjacency[vertex].iter().filter(|(_, edge)| !route[*edge]) {
                    let total = reached + self.edges[edge].length_nm;
                    if total < best[next] {
                        best[next] = total;
                        arrived[next] = Some(edge);
                        queue.push(Reverse((total.to_bits(), next)));
                    }
                }
            }
            // Edge interiors: a stub edge between two reached stub junctions
            // (or the attachment itself) peaks between its ends when it closes
            // a loop. Half a nanometre above both ends keeps rounding on a
            // straight branch from relabelling its far end.
            let inside = |vertex: usize| vertex == source || (best[vertex].is_finite() && !attached(vertex));
            for (index, edge) in self.edges.iter().enumerate() {
                let [a, b] = edge.ends;
                if route[index] || !inside(a) || !inside(b) {
                    continue;
                }
                let peak = (best[a] + best[b] + edge.length_nm) / 2.0;
                if peak > best[a].max(best[b]) + 0.5 && longest.as_ref().is_none_or(|(length, _)| peak > *length) {
                    longest = Some((peak, edge.label.clone()));
                }
            }
        }
        longest
    }
}

/// The longest stub of one net's copper as `(length, dead-end object)`.
///
/// `objects` are the net's pad, pour and via contacts grouped by object.
fn longest_stub(pieces: &[Piece], objects: &IndexMap<String, Vec<&CopperContact>>) -> Result<Option<(i64, String)>> {
    let mut sites = Sites::default();
    sites.ends = pieces.iter().flat_map(|_| [sites.junctions.add(), sites.junctions.add()]).collect();
    sites.cuts = vec![Vec::new(); pieces.len()];
    let object_junction: Vec<usize> = objects.iter().map(|_| sites.junctions.add()).collect();
    // Pieces on one layer join where their copper overlaps.
    for (i, first) in pieces.iter().enumerate() {
        for (j, _) in pieces.iter().enumerate().skip(i + 1).filter(|(_, p)| p.layer == first.layer) {
            sites.join(pieces, i, j);
        }
    }
    for (i, from) in pieces.iter().enumerate() {
        for (e, point) in from.ends().into_iter().enumerate() {
            // Ends that land on a pad, pour or via join its junction.
            let disk = shape(vec![point], from.radius_nm)?;
            for (group, contacts) in objects.values().enumerate() {
                if contacts.iter().any(|c| c.layers.contains(&from.layer) && positive_area_overlap(&disk, &c.shape)) {
                    sites.junctions.union(sites.ends[2 * i + e], object_junction[group]);
                }
            }
        }
    }
    let Sites { mut junctions, mut cuts, ends, .. } = sites;
    // Objects that overlap each other (a via in a pad) are one junction.
    for (a, first) in objects.values().enumerate() {
        for (b, second) in objects.values().enumerate().skip(a + 1) {
            if first.iter().any(|f| second.iter().any(|s| copper_contacts_overlap(f, s))) {
                junctions.union(object_junction[a], object_junction[b]);
            }
        }
    }
    // A pad or via on the body of a piece, with no end of the piece on it,
    // splits the piece there, so copper running past it is a branch of its own.
    for (j, piece) in pieces.iter().enumerate() {
        let capsules = piece
            .path
            .windows(2)
            .map(|pair| shape(vec![pair[0], pair[1]], piece.radius_nm))
            .collect::<Result<Vec<_>>>()?;
        for (group, (key, contacts)) in objects.iter().enumerate() {
            let object = junctions.find(object_junction[group]);
            if !(key.starts_with("pad:") || key.starts_with("via:"))
                || [ends[2 * j], ends[2 * j + 1]].into_iter().any(|end| junctions.find(end) == object)
            {
                continue;
            }
            let lands = |c: &&&CopperContact| {
                c.layers.contains(&piece.layer) && capsules.iter().any(|body| positive_area_overlap(body, &c.shape))
            };
            if let Some(contact) = contacts.iter().find(lands) {
                let spine = &contact.shape.spine;
                let count = spine.len() as i128;
                let mean = |value: fn(&Point) -> i64| {
                    (spine.iter().map(|p| i128::from(value(p))).sum::<i128>() / count) as i64
                };
                let centre = Point::new(mean(|p| p.x_nm), mean(|p| p.y_nm));
                cuts[j].push((piece.project(centre).1, object_junction[group]));
            }
        }
    }
    let mut edges: Vec<Edge> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let mut chain: Vec<(f64, usize)> = vec![(0.0, ends[2 * index])];
        let mut inner = std::mem::take(&mut cuts[index]);
        inner.sort_by(|a, b| a.0.total_cmp(&b.0));
        chain.extend(inner);
        chain.push((1.0, ends[2 * index + 1]));
        for pair in chain.windows(2) {
            let (from, to) = (junctions.find(pair[0].1), junctions.find(pair[1].1));
            if from != to {
                edges.push(Edge {
                    ends: [from, to],
                    length_nm: piece.length_nm * (pair[1].0 - pair[0].0),
                    label: piece.label.clone(),
                });
            }
        }
    }
    let mut graph = Graph::default();
    for edge in &edges {
        let ends = edge.ends.map(|end| graph.vertex(junctions.find(end)));
        let id = graph.edges.len();
        graph.edges.push(GraphEdge { ends, length_nm: edge.length_nm, label: edge.label.clone() });
        graph.adjacency[ends[0]].push((ends[1], id));
        graph.adjacency[ends[1]].push((ends[0], id));
    }
    for (key, junction) in objects.keys().zip(&object_junction) {
        let index = graph.vertex(junctions.find(*junction));
        if is_terminal(key) {
            graph.terminal[index] = true;
        } else {
            graph.via[index] = Some(key.clone());
        }
    }
    Ok(graph.longest_stub().map(|(length, end)| (round_int(length), end)))
}

/// `maximum_stub_length` of every net whose rule declares one.
pub(crate) fn check_stub_lengths(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let rules: IndexMap<&str, &NetRoutingRule> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let limits: IndexMap<&str, i64> =
        rules.iter().filter_map(|(net, rule)| rule.maximum_stub_length_nm.map(|limit| (*net, limit))).collect();
    if limits.is_empty() {
        return Ok(());
    }
    let selected: IndexSet<String> = limits.keys().map(|net| (*net).to_owned()).collect();
    let (contacts, pad_nodes, internal) =
        explicit_copper_contacts(board, Candidate { only_nets: Some(&selected), ..Candidate::default() })?;
    let graph =
        Connectivity { roots: copper_contact_roots(&contacts, &internal)?, pad_nodes, internal_connections: internal };
    let antenna = copper_pcb::antenna::antenna_tracks(board)?;
    let mut nets: Vec<&str> = limits.keys().copied().collect();
    nets.sort_unstable();
    for net in nets {
        let Some(physical) = board.nets.iter().find(|n| n.name == net) else { continue };
        if !graph.net_connected(physical) {
            continue;
        }
        // Pads, pours and vias by object: the chord pieces of an arc and the
        // triangles of a pour are not objects.
        let mut objects: IndexMap<String, Vec<&CopperContact>> = IndexMap::new();
        for contact in contacts.iter().filter(|c| c.net == net) {
            let identity = contact.identity.as_str();
            if identity.starts_with("pad:") || identity.starts_with("via:") {
                objects.entry(identity.to_owned()).or_default().push(contact);
            } else if let Some(index) = identity.strip_prefix("track:").and_then(|i| i.parse::<usize>().ok())
                && antenna.contains(&board.tracks[index])
            {
                // Synthesized antenna copper is where the feed ends.
                objects.insert(format!("antenna:{index}"), vec![contact]);
            } else if let Some(("polygon", _)) = identity.split_once(':') {
                let key = identity.rsplit_once(':').map_or(identity, |(key, _)| key);
                objects.entry(key.to_owned()).or_default().push(contact);
            }
        }
        let limit = limits[net];
        if let Some((length, end)) = longest_stub(&pieces(board, net, &antenna), &objects)?
            && length > limit
        {
            findings.push(
                Finding::error(
                    "DRC-STUB-LENGTH",
                    format!("net {} has a stub longer than its maximum stub length", copper_json::python_str_repr(net)),
                )
                .objects([end])
                .nets([net])
                .required(limit)
                .measured(length),
            );
        }
    }
    Ok(())
}

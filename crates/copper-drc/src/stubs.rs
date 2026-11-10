//! `DRC-STUB-LENGTH`: a routing rule's `maximum_stub_length` against the
//! routed copper of its net.
//!
//! A stub is a branch of a connected net's copper that leaves a junction and
//! ends at a dead end that is not a pad or a pour: a dangling track or arc end
//! or an unused via. Its length is measured along the copper, from the
//! junction to the farthest dead end of the branch; arcs count at their true
//! length and a via barrel does not count. Branches that end on a pad are
//! routes, not stubs. Only nets that are completely connected are measured,
//! since the open net is the finding on a partly routed one.
//!
//! The copper becomes a graph of junctions joined by track and arc pieces.
//! Track and arc ends that overlap on a layer share a junction; an end that
//! lands on the body of another piece splits it there; an end that lands on a
//! via, pad or pour joins that object's junction, as do objects that overlap
//! each other. The stubs are what pruning dead-end junctions removes.

use std::collections::VecDeque;

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

/// One track, or one arc as its chord chain, with its copper length.
struct Piece {
    layer: CopperLayer,
    path: Vec<Point>,
    radius_nm: i64,
    length_nm: f64,
    label: String,
}

impl Piece {
    fn ends(&self) -> [Point; 2] {
        [self.path[0], self.path[self.path.len() - 1]]
    }

    /// The distance from `point` to the path and the position of the nearest
    /// point along it as a fraction of its length.
    fn project(&self, point: Point) -> (f64, f64) {
        let spans: Vec<f64> = self.path.windows(2).map(|pair| distance(pair[0], pair[1])).collect();
        let total: f64 = spans.iter().sum();
        let mut best = (f64::INFINITY, 0.0);
        let mut before = 0.0;
        for (pair, span) in self.path.windows(2).zip(&spans) {
            let (away, t) = project(point, pair[0], pair[1]);
            if away < best.0 {
                best = (away, if total > 0.0 { (before + t * span) / total } else { 0.0 });
            }
            before += span;
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
            Piece {
                layer: track.layer,
                path: vec![track.start, track.end],
                radius_nm: track.width_nm / 2,
                length_nm: distance(track.start, track.end),
                label: format!("track:{index}"),
            }
        });
    let arcs = board.arcs.iter().enumerate().filter(|(_, a)| a.net == net).map(|(index, arc)| Piece {
        layer: arc.layer,
        path: arc.chord_points(),
        radius_nm: arc.width_nm / 2,
        length_nm: arc.length_nm() as f64,
        label: format!("arc:{index}"),
    });
    tracks.chain(arcs).collect()
}

/// A stretch of copper between two junctions.
struct Edge {
    ends: [usize; 2],
    length_nm: f64,
    label: String,
}

/// The longest stub of one net's copper as `(length, dead-end object)`.
///
/// `objects` are the net's pad, pour and via contacts grouped by object.
fn longest_stub(pieces: &[Piece], objects: &IndexMap<String, Vec<&CopperContact>>) -> Result<Option<(i64, String)>> {
    let mut junctions = Junctions::default();
    let ends: Vec<usize> = pieces.iter().flat_map(|_| [junctions.add(), junctions.add()]).collect();
    let object_junction: Vec<usize> = objects.iter().map(|_| junctions.add()).collect();
    // Overlapping ends join, or split the piece they land on.
    let mut cuts: Vec<Vec<(f64, usize)>> = vec![Vec::new(); pieces.len()];
    for (i, from) in pieces.iter().enumerate() {
        for (e, point) in from.ends().into_iter().enumerate() {
            for (j, onto) in pieces.iter().enumerate().filter(|(j, o)| *j != i && o.layer == from.layer) {
                let reach = (from.radius_nm + onto.radius_nm) as f64;
                let mut joined = false;
                for (k, end) in onto.ends().into_iter().enumerate() {
                    if distance(point, end) < reach {
                        junctions.union(ends[2 * i + e], ends[2 * j + k]);
                        joined = true;
                    }
                }
                if !joined {
                    let (away, along) = onto.project(point);
                    if away < reach {
                        cuts[j].push((along, ends[2 * i + e]));
                    }
                }
            }
            // Ends that land on a pad, pour or via join its junction.
            let disk = shape(vec![point], from.radius_nm)?;
            for (group, contacts) in objects.values().enumerate() {
                if contacts.iter().any(|c| c.layers.contains(&from.layer) && positive_area_overlap(&disk, &c.shape)) {
                    junctions.union(ends[2 * i + e], object_junction[group]);
                }
            }
        }
    }
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
        for (along, port) in inner {
            let cut = junctions.add();
            junctions.union(cut, port);
            chain.push((along, cut));
        }
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
    let mut terminal: IndexSet<usize> = IndexSet::new();
    let mut via: IndexMap<usize, &str> = IndexMap::new();
    for (key, junction) in objects.keys().zip(&object_junction) {
        let root = junctions.find(*junction);
        if is_terminal(key) {
            terminal.insert(root);
        } else {
            via.insert(root, key);
        }
    }
    // Prune dead-end junctions; what each carries is the longest copper
    // hanging off it, ending at the labelled object.
    let mut incident: IndexMap<usize, Vec<usize>> = IndexMap::new();
    for (index, edge) in edges.iter().enumerate() {
        for end in edge.ends {
            incident.entry(end).or_default().push(index);
        }
    }
    let mut degree: IndexMap<usize, usize> = incident.iter().map(|(junction, list)| (*junction, list.len())).collect();
    let mut hanging: IndexMap<usize, (f64, String)> = IndexMap::new();
    let mut cut_edges = vec![false; edges.len()];
    let mut queue: VecDeque<usize> = degree
        .iter()
        .filter(|(junction, count)| **count <= 1 && !terminal.contains(*junction))
        .map(|(j, _)| *j)
        .collect();
    let mut longest: Option<(f64, String)> = None;
    while let Some(junction) = queue.pop_front() {
        let Some(&edge) = incident[&junction].iter().find(|edge| !cut_edges[**edge]) else { continue };
        cut_edges[edge] = true;
        let [first, second] = edges[edge].ends;
        let parent = if first == junction { second } else { first };
        *degree.get_mut(&junction).expect("junction") = 0;
        let (below, end) = hanging.get(&junction).cloned().unwrap_or_else(|| {
            (0.0, via.get(&junction).map_or_else(|| edges[edge].label.clone(), |k| (*k).to_owned()))
        });
        let carried = below + edges[edge].length_nm;
        *degree.get_mut(&parent).expect("parent") -= 1;
        if hanging.get(&parent).is_none_or(|(best, _)| *best <= carried) {
            hanging.insert(parent, (carried, end.clone()));
        }
        if terminal.contains(&parent) || degree[&parent] >= 2 {
            if longest.as_ref().is_none_or(|(best, _)| carried > *best) {
                longest = Some((carried, end));
            }
        } else {
            queue.push_back(parent);
        }
    }
    Ok(longest.map(|(length, end)| (round_int(length), end)))
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

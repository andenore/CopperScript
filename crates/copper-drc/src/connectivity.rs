//! Layer-aware connectivity of explicit copper (`pcbir.copper_connectivity`,
//! `drc.explicit_copper_connectivity`); zone outlines are not copper.
//!
//! Each plated object is one node spanning only its physical copper layers.
//! Positive-area copper overlap joins nodes; declared component-internal pad
//! groups are separate edges, never fabricated copper.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{CopperPolygon, PadReference, PhysicalBoard, PhysicalNet, TrackSegment, Via};
use copper_pcb::hard_macros::layer_index;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::{Error, Result};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{
    SpatialIndex, point_segment_distance_squared, polygon_triangles, rational, shape_distance_squared,
};
use copper_physical::{BoardSide, CopperLayer, RoundedConvexShape};

use crate::{drill_shape, shape};

/// `CopperContact`: one plated object on its copper layers.
#[derive(Clone, Debug)]
pub struct CopperContact {
    pub identity: String,
    pub net: String,
    pub layers: Vec<CopperLayer>,
    pub shape: RoundedConvexShape,
    pub drill: Option<RoundedConvexShape>,
    pub capped_layers: Vec<CopperLayer>,
}

/// `via_copper_contact`: the plated span, open drill and exterior caps of a via.
pub fn via_copper_contact(board: &PhysicalBoard, via: &Via, identity: String) -> Result<CopperContact> {
    let layers = &board.stackup.copper_layers;
    let (a, b) = (layer_index(board, via.from_layer)?, layer_index(board, via.to_layer)?);
    let capped = if via.finish == "filled-capped" { vec![layers[0], layers[layers.len() - 1]] } else { Vec::new() };
    Ok(CopperContact {
        identity,
        net: via.net.clone(),
        layers: layers[a.min(b)..=a.max(b)].to_vec(),
        shape: shape(vec![via.position], via.size_nm / 2)?,
        drill: Some(shape(vec![via.position], via.drill_nm / 2)?),
        capped_layers: capped,
    })
}

/// `_inside_drill`: copper inside a drilled void, including a touch of its rim.
fn inside_drill(shape: &RoundedConvexShape, drill: Option<&RoundedConvexShape>) -> bool {
    let Some(drill) = drill else { return false };
    if shape.radius_nm > drill.radius_nm {
        return false;
    }
    let (start, end) = (drill.spine[0], drill.spine[drill.spine.len() - 1]);
    let margin = i128::from(drill.radius_nm - shape.radius_nm);
    shape.spine.iter().all(|point| point_segment_distance_squared(*point, start, end) <= rational(margin * margin))
}

/// `_positive_area_overlap`: a shared boundary alone is not an electrical connection.
fn positive_area_overlap(first: &RoundedConvexShape, second: &RoundedConvexShape) -> bool {
    let radius = i128::from(first.radius_nm) + i128::from(second.radius_nm);
    if radius != 0 {
        return shape_distance_squared(first, second) < rational(radius * radius);
    }
    for shape in [first, second] {
        let points = &shape.spine;
        let edges: Vec<_> = (0..points.len()).map(|i| (points[i], points[(i + 1) % points.len()])).collect();
        let area: i128 = edges
            .iter()
            .map(|(a, b)| i128::from(a.x_nm) * i128::from(b.y_nm) - i128::from(a.y_nm) * i128::from(b.x_nm))
            .sum();
        if area == 0 {
            return false;
        }
        for (a, b) in edges {
            let (dx, dy) = (i128::from(b.x_nm - a.x_nm), i128::from(b.y_nm - a.y_nm));
            if dx == 0 && dy == 0 {
                continue;
            }
            let project = |s: &RoundedConvexShape| -> (i128, i128) {
                let values = s.spine.iter().map(|p| i128::from(p.x_nm) * dy - i128::from(p.y_nm) * dx);
                (values.clone().min().expect("spine"), values.max().expect("spine"))
            };
            let ((left_min, left_max), (right_min, right_max)) = (project(first), project(second));
            if left_max <= right_min || right_max <= left_min {
                return false;
            }
        }
    }
    true
}

/// `copper_contacts_overlap`: positive-area contact on a common copper layer.
pub fn copper_contacts_overlap(first: &CopperContact, second: &CopperContact) -> bool {
    if first.net != second.net || !positive_area_overlap(&first.shape, &second.shape) {
        return false;
    }
    first.layers.iter().filter(|layer| second.layers.contains(layer)).any(|layer| {
        let first_drill = if first.capped_layers.contains(layer) { None } else { first.drill.as_ref() };
        let second_drill = if second.capped_layers.contains(layer) { None } else { second.drill.as_ref() };
        !inside_drill(&second.shape, first_drill) && !inside_drill(&first.shape, second_drill)
    })
}

fn find(parent: &mut BTreeMap<String, String>, identity: &str) -> String {
    let mut root = identity.to_owned();
    while parent[&root] != root {
        root = parent[&root].clone();
    }
    let mut current = identity.to_owned();
    while parent[&current] != current {
        let next = parent[&current].clone();
        parent.insert(current, root.clone());
        current = next;
    }
    root
}

fn union(parent: &mut BTreeMap<String, String>, first: &str, second: &str) {
    let (left, right) = (find(parent, first), find(parent, second));
    let (low, high) = if left <= right { (left, right) } else { (right, left) };
    parent.insert(high, low);
}

/// `copper_contact_roots`: deterministic component roots, by identity.
pub fn copper_contact_roots(
    objects: &[CopperContact],
    internal_connections: &[Vec<String>],
) -> Result<BTreeMap<String, String>> {
    let by_id: IndexMap<&str, &CopperContact> = objects.iter().map(|o| (o.identity.as_str(), o)).collect();
    if by_id.len() != objects.len() {
        return Err(Error::Value("copper contact identities must be unique".to_owned()));
    }
    let mut parent: BTreeMap<String, String> = by_id.keys().map(|id| ((*id).to_owned(), (*id).to_owned())).collect();
    let mut by_layer_net: IndexMap<(CopperLayer, &str), Vec<&CopperContact>> = IndexMap::new();
    for item in objects {
        for layer in &item.layers {
            by_layer_net.entry((*layer, item.net.as_str())).or_default().push(item);
        }
    }
    for items in by_layer_net.values() {
        let index = SpatialIndex::new(items.iter().map(|item| (item.identity.clone(), item.shape.bounds)));
        for first in items {
            for identity in index.query(first.shape.bounds) {
                if identity <= first.identity.as_str()
                    || find(&mut parent, identity) == find(&mut parent, &first.identity)
                {
                    continue;
                }
                if copper_contacts_overlap(first, by_id[identity]) {
                    union(&mut parent, &first.identity, identity);
                }
            }
        }
    }
    for group in internal_connections {
        if group.len() < 2 || group.iter().any(|node| !by_id.contains_key(node.as_str())) {
            return Err(Error::Value("internal copper graph edge has missing contacts".to_owned()));
        }
        let nets: IndexSet<&str> = group.iter().map(|node| by_id[node.as_str()].net.as_str()).collect();
        if nets.len() != 1 {
            return Err(Error::Value("internal connection cannot bridge different nets".to_owned()));
        }
        for node in &group[1..] {
            union(&mut parent, &group[0], node);
        }
    }
    let identities: Vec<String> = parent.keys().cloned().collect();
    Ok(identities.into_iter().map(|id| (id.clone(), find(&mut parent, &id))).collect())
}

/// `PhysicalCopperConnectivity`: land roots including only declared internal edges.
#[derive(Clone, Debug)]
pub struct Connectivity {
    pub roots: BTreeMap<String, String>,
    pub pad_nodes: IndexMap<PadReference, Vec<String>>,
    pub internal_connections: Vec<Vec<String>>,
}

impl Connectivity {
    pub fn pad_connected(&self, pad: &PadReference) -> bool {
        let nodes = self.pad_nodes.get(pad).map_or(&[][..], Vec::as_slice);
        !nodes.is_empty() && nodes.iter().map(|n| &self.roots[n]).collect::<IndexSet<_>>().len() == 1
    }

    pub fn net_connected(&self, net: &PhysicalNet) -> bool {
        if net.pads.is_empty() {
            return true;
        }
        if net.pads.iter().any(|pad| self.pad_nodes.get(pad).is_none_or(Vec::is_empty)) {
            return false;
        }
        net.pads
            .iter()
            .flat_map(|pad| self.pad_nodes[pad].iter().map(|n| &self.roots[n]))
            .collect::<IndexSet<_>>()
            .len()
            == 1
    }

    /// The root of one node.
    pub fn root(&self, node: &str) -> &str {
        &self.roots[node]
    }
}

/// Candidate copper evaluated against the board's pads (`tracks=`/`vias=`/`polygons=`).
#[derive(Clone, Copy, Default)]
pub struct Candidate<'a> {
    pub only_nets: Option<&'a IndexSet<String>>,
    pub exclude_internal_connections: bool,
    pub tracks: Option<&'a [TrackSegment]>,
    pub vias: Option<&'a [Via]>,
    pub polygons: Option<&'a [CopperPolygon]>,
}

/// `_explicit_copper_contacts`: the exact graph's objects, pad nodes and internal edges.
#[allow(clippy::type_complexity)]
pub fn explicit_copper_contacts(
    board: &PhysicalBoard,
    candidate: Candidate,
) -> Result<(Vec<CopperContact>, IndexMap<PadReference, Vec<String>>, Vec<Vec<String>>)> {
    let selected = |net: &str| candidate.only_nets.is_none_or(|nets| nets.contains(net));
    let tracks = candidate.tracks.unwrap_or(&board.tracks);
    let vias = candidate.vias.unwrap_or(&board.vias);
    let polygons = candidate.polygons.unwrap_or(&board.polygons);
    let mut objects = Vec::new();
    let mut pad_nodes: IndexMap<PadReference, Vec<String>> = IndexMap::new();
    let mut internal = Vec::new();
    let mut placements: Vec<_> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in placements {
        let footprint = &board.footprints[&placement.footprint];
        let mut assigned: IndexMap<&str, &str> = IndexMap::new();
        for net in board.nets.iter().filter(|n| selected(&n.name)) {
            for pad in net.pads.iter().filter(|p| p.component == placement.reference) {
                assigned.insert(pad.pad.as_str(), net.name.as_str());
            }
        }
        for (index, pad) in footprint.pads.iter().enumerate() {
            let Some(net) = assigned.get(pad.number.as_str()) else { continue };
            if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            let layers = if pad.kind == PadKind::Smd {
                vec![if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                board.stackup.copper_layers.clone()
            };
            let identity = format!("pad:{}.{}:{index}", placement.reference, pad.number);
            let drill = (pad.kind == PadKind::ThroughHole && pad.drill.is_some())
                .then(|| drill_shape(position, pad, placement));
            objects.push(CopperContact {
                identity: identity.clone(),
                net: (*net).to_owned(),
                layers,
                shape: copper_pcb::transform::placed_pad_shape(position, pad, placement),
                drill,
                capped_layers: Vec::new(),
            });
            pad_nodes.entry(PadReference::new(&placement.reference, &pad.number)).or_default().push(identity);
        }
        if !candidate.exclude_internal_connections {
            for group in &footprint.internal_pad_groups {
                let nodes: Vec<String> = group
                    .numbers
                    .iter()
                    .flat_map(|number| {
                        pad_nodes.get(&PadReference::new(&placement.reference, number)).cloned().unwrap_or_default()
                    })
                    .collect();
                if nodes.len() > 1 {
                    internal.push(nodes);
                }
            }
        }
    }
    for (index, track) in tracks.iter().enumerate().filter(|(_, t)| selected(&t.net)) {
        objects.push(CopperContact {
            identity: format!("track:{index}"),
            net: track.net.clone(),
            layers: vec![track.layer],
            shape: shape(vec![track.start, track.end], track.width_nm / 2)?,
            drill: None,
            capped_layers: Vec::new(),
        });
    }
    for (index, via) in vias.iter().enumerate().filter(|(_, v)| selected(&v.net)) {
        objects.push(via_copper_contact(board, via, format!("via:{index}"))?);
    }
    for polygon in polygons.iter().filter(|p| selected(&p.net)) {
        let mut nodes = Vec::new();
        for (index, triangle) in polygon_triangles(&polygon.vertices)?.into_iter().enumerate() {
            let identity = format!("polygon:{}:{index}", polygon.id);
            objects.push(CopperContact {
                identity: identity.clone(),
                net: polygon.net.clone(),
                layers: vec![polygon.layer],
                shape: shape(triangle.to_vec(), 0)?,
                drill: None,
                capped_layers: Vec::new(),
            });
            nodes.push(identity);
        }
        if nodes.len() > 1 {
            internal.push(nodes);
        }
    }
    Ok((objects, pad_nodes, internal))
}

/// `explicit_copper_connectivity`: the exact explicit copper graph.
pub fn explicit_copper_connectivity(board: &PhysicalBoard, candidate: Candidate) -> Result<Connectivity> {
    let (objects, pad_nodes, internal) = explicit_copper_contacts(board, candidate)?;
    Ok(Connectivity { roots: copper_contact_roots(&objects, &internal)?, pad_nodes, internal_connections: internal })
}

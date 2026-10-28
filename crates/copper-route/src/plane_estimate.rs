//! Fast raster estimate of zone fill and plane-net connectivity
//! (`pcbir.plane_estimate`).
//!
//! PENDING: owned by rust-m4-unified (with the `unified_router` raster
//! helpers `_polygon_inside`, `_segment_distance` and `_spine_distance`); the
//! route summary of `replay-critical` needs it first.
//!
//! KiCad's native refill stays the authority for sign-off. Each zone layer is
//! filled on a raster: the zone outline inside board material with edge
//! clearance, minus foreign copper inflated by clearance, minus
//! higher-priority foreign zones and zone-blocking keepouts. Connected fill
//! regions then join the copper of their net that touches them. The raster
//! arithmetic is numpy's element-wise float64 (libm `hypot`), cell by cell.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::{Object, Value};
use copper_pcb::board::{PadReference, PhysicalBoard, ZoneConnection};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;

/// `PlaneEstimate`.
pub struct PlaneEstimate {
    pub pitch_nm: i64,
    /// net -> number of disconnected groups of its pads.
    pub net_groups: IndexMap<String, i64>,
    /// Sum over zone nets of (groups - 1).
    pub opens: i64,
    pub unfilled_zones: Vec<String>,
    /// net -> pads outside the net's largest connected group ("REF.PAD").
    pub open_pads: IndexMap<String, Vec<String>>,
}

impl PlaneEstimate {
    /// `report()`.
    pub fn report(&self) -> Value {
        let mut open_nets: Vec<&String> =
            self.net_groups.iter().filter(|(_, groups)| **groups > 1).map(|(net, _)| net).collect();
        open_nets.sort();
        let mut pads: Vec<(&String, &Vec<String>)> = self.open_pads.iter().collect();
        pads.sort();
        let mut open_pads = Object::new();
        for (net, items) in pads {
            open_pads.insert(net.clone(), Value::Array(items.iter().map(|pad| Value::from(pad.as_str())).collect()));
        }
        let mut document = Object::new();
        document.insert("pitch_mm".to_owned(), Value::Float(self.pitch_nm as f64 / 1_000_000.0));
        document.insert("opens".to_owned(), Value::from(self.opens));
        document.insert(
            "open_nets".to_owned(),
            Value::Array(open_nets.into_iter().map(|net| Value::from(net.as_str())).collect()),
        );
        document.insert("open_pads".to_owned(), Value::Object(open_pads));
        document.insert(
            "unfilled_zones".to_owned(),
            Value::Array(self.unfilled_zones.iter().map(|zone| Value::from(zone.as_str())).collect()),
        );
        Value::Object(document)
    }
}

/// CPython's `float.__floordiv__`.
fn floordiv(vx: f64, wx: f64) -> f64 {
    let mut modulo = vx % wx;
    let mut div = (vx - modulo) / wx;
    if modulo != 0.0 {
        if (wx < 0.0) != (modulo < 0.0) {
            modulo += wx;
            div -= 1.0;
        }
    } else {
        modulo = 0.0f64.copysign(wx);
    }
    let _ = modulo;
    if div != 0.0 {
        let mut floor = div.floor();
        if div - floor > 0.5 {
            floor += 1.0;
        }
        floor
    } else {
        0.0f64.copysign(vx / wx)
    }
}

/// The raster: cell centres `xs[i]`, `ys[j]` of an `ny` x `nx` grid.
struct Raster {
    x0: i64,
    y0: i64,
    nx: usize,
    ny: usize,
    pitch: i64,
    xs: Vec<f64>,
    ys: Vec<f64>,
}

/// An inclusive cell window `i0..=i1`, `j0..=j1` (empty when reversed).
#[derive(Clone, Copy)]
struct Window {
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
}

impl Window {
    fn is_empty(&self) -> bool {
        self.i0 > self.i1 || self.j0 > self.j1
    }

    fn cells(self) -> impl Iterator<Item = (usize, usize)> {
        (self.j0..=self.j1).flat_map(move |j| (self.i0..=self.i1).map(move |i| (j as usize, i as usize)))
    }
}

impl Raster {
    fn full(&self) -> Window {
        Window { i0: 0, i1: self.nx as i64 - 1, j0: 0, j1: self.ny as i64 - 1 }
    }

    /// The cells within `reach` of the bounding box of `spine` (`window`).
    fn window(&self, xs: &[i64], ys: &[i64], reach: f64) -> Window {
        let (min_x, max_x) = (*xs.iter().min().expect("points"), *xs.iter().max().expect("points"));
        let (min_y, max_y) = (*ys.iter().min().expect("points"), *ys.iter().max().expect("points"));
        let pitch = self.pitch as f64;
        let index = |value: f64, origin: i64| floordiv(value - origin as f64, pitch) as i64;
        Window {
            i0: index(min_x as f64 - reach, self.x0).max(0),
            i1: (index(max_x as f64 + reach, self.x0) + 1).min(self.nx as i64 - 1),
            j0: index(min_y as f64 - reach, self.y0).max(0),
            j1: (index(max_y as f64 + reach, self.y0) + 1).min(self.ny as i64 - 1),
        }
    }

    fn at(&self, j: usize, i: usize) -> usize {
        j * self.nx + i
    }
}

/// `unified_router._polygon_inside`: even-odd point in polygon per cell.
fn polygon_inside(vertices: &[Point], raster: &Raster) -> Vec<bool> {
    let mut inside = vec![false; raster.nx * raster.ny];
    let n = vertices.len();
    for index in 0..n {
        let (a, b) = (vertices[index], vertices[(index + 1) % n]);
        let (ax, ay, bx, by) = (a.x_nm as f64, a.y_nm as f64, b.x_nm as f64, b.y_nm as f64);
        for (j, py) in raster.ys.iter().enumerate() {
            let crosses = (ay > *py) != (by > *py);
            if !crosses {
                continue;
            }
            let x_at = ax + (py - ay) * (bx - ax) / (by - ay);
            for (i, px) in raster.xs.iter().enumerate() {
                if *px < x_at {
                    let cell = raster.at(j, i);
                    inside[cell] ^= true;
                }
            }
        }
    }
    inside
}

/// `unified_router._segment_distance` at one point.
fn segment_distance(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let length2 = dx * dx + dy * dy;
    if length2 == 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / length2).clamp(0.0, 1.0);
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// `unified_router._spine_distance`: distance to a convex point, segment or
/// polygon spine (0 inside).
fn spine_distance(spine: &[Point], px: f64, py: f64) -> f64 {
    let xs: Vec<f64> = spine.iter().map(|p| p.x_nm as f64).collect();
    let ys: Vec<f64> = spine.iter().map(|p| p.y_nm as f64).collect();
    match spine.len() {
        1 => (px - xs[0]).hypot(py - ys[0]),
        2 => segment_distance(px, py, xs[0], ys[0], xs[1], ys[1]),
        n => {
            let mut best = f64::NAN;
            let (mut inside_pos, mut inside_neg) = (true, true);
            for index in 0..n {
                let (ax, ay, bx, by) = (xs[index], ys[index], xs[(index + 1) % n], ys[(index + 1) % n]);
                let d = segment_distance(px, py, ax, ay, bx, by);
                // np.minimum propagates NaN.
                best = if index == 0 || d.is_nan() || best.is_nan() {
                    if index == 0 { d } else { f64::NAN }
                } else {
                    best.min(d)
                };
                let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
                inside_pos &= cross >= 0.0;
                inside_neg &= cross <= 0.0;
            }
            if inside_pos || inside_neg { 0.0 } else { best }
        }
    }
}

/// `_label`: 4-connected component labels via row runs and union-find.
fn label(mask: &[bool], raster: &Raster) -> Vec<i32> {
    let mut labels = vec![0i32; mask.len()];
    let mut parent: Vec<usize> = vec![0];
    fn find(parent: &mut [usize], mut a: usize) -> usize {
        while parent[a] != a {
            parent[a] = parent[parent[a]];
            a = parent[a];
        }
        a
    }
    let mut previous: Vec<(usize, usize, usize)> = Vec::new();
    for y in 0..raster.ny {
        let row = &mask[y * raster.nx..(y + 1) * raster.nx];
        if !row.iter().any(|cell| *cell) {
            previous.clear();
            continue;
        }
        let mut runs = Vec::new();
        let mut j = 0;
        let mut x = 0;
        while x < raster.nx {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < raster.nx && row[x] {
                x += 1;
            }
            let end = x;
            let current = parent.len();
            parent.push(current);
            while j < previous.len() && previous[j].1 <= start {
                j += 1;
            }
            let mut k = j;
            while k < previous.len() && previous[k].0 < end {
                let (a, b) = (find(&mut parent, current), find(&mut parent, previous[k].2));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
                k += 1;
            }
            for cell in &mut labels[y * raster.nx + start..y * raster.nx + end] {
                *cell = current as i32;
            }
            runs.push((start, end, current));
        }
        previous = runs;
    }
    let roots: Vec<i32> = (0..parent.len()).map(|index| find(&mut parent, index) as i32).collect();
    labels.iter().map(|label| roots[*label as usize]).collect()
}

/// A copper object: identity, layers, spine and radius.
struct Copper {
    identity: (String, String, String),
    layers: Vec<CopperLayer>,
    spine: Vec<Point>,
    radius: i64,
}

/// Union-find over node names; every root is its set's least name.
#[derive(Default)]
struct Sets {
    parent: BTreeMap<String, String>,
}

impl Sets {
    fn find(&mut self, name: &str) -> String {
        let mut a = name.to_owned();
        self.parent.entry(a.clone()).or_insert_with(|| a.clone());
        loop {
            let up = self.parent[&a].clone();
            if up == a {
                return a;
            }
            let grand = self.parent[&up].clone();
            self.parent.insert(a.clone(), grand.clone());
            a = grand;
        }
    }

    fn union(&mut self, a: &str, b: &str) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            self.parent.insert(high, low);
        }
    }
}

/// `estimate_plane_connectivity(board)` at the default 0.05 mm pitch.
pub fn estimate_plane_connectivity(board: &PhysicalBoard) -> Result<PlaneEstimate> {
    let pitch_nm: i64 = 50_000;
    let zone_nets: IndexSet<String> = board.zones.iter().map(|zone| zone.net.clone()).collect();
    if board.zones.is_empty() {
        return Ok(PlaneEstimate {
            pitch_nm,
            net_groups: IndexMap::new(),
            opens: 0,
            unfilled_zones: Vec::new(),
            open_pads: IndexMap::new(),
        });
    }
    let vertices = &board.outline.vertices;
    let x0 = vertices.iter().map(|p| p.x_nm).min().expect("outline");
    let y0 = vertices.iter().map(|p| p.y_nm).min().expect("outline");
    let x1 = vertices.iter().map(|p| p.x_nm).max().expect("outline");
    let y1 = vertices.iter().map(|p| p.y_nm).max().expect("outline");
    let nx = ((x1 - x0).div_euclid(pitch_nm) + 1) as usize;
    let ny = ((y1 - y0).div_euclid(pitch_nm) + 1) as usize;
    let xs = (0..nx).map(|i| x0 as f64 + pitch_nm as f64 * (i as f64 + 0.5)).collect();
    let ys = (0..ny).map(|j| y0 as f64 + pitch_nm as f64 * (j as f64 + 0.5)).collect();
    let raster = Raster { x0, y0, nx, ny, pitch: pitch_nm, xs, ys };
    let mut material = polygon_inside(vertices, &raster);
    for cutout in &board.outline.cutouts {
        for (cell, inside) in material.iter_mut().zip(polygon_inside(&cutout.vertices, &raster)) {
            *cell &= !inside;
        }
    }
    let minimum_clearance = board.rules.minimum_clearance_nm;
    let half = pitch_nm as f64 / 2.0;
    let edge_reach = minimum_clearance as f64 + half;
    let mut edge_ok = vec![true; nx * ny];
    let rings: Vec<&Vec<Point>> =
        std::iter::once(vertices).chain(board.outline.cutouts.iter().map(|c| &c.vertices)).collect();
    for ring in rings {
        for index in 0..ring.len() {
            let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
            let window = raster.window(&[a.x_nm, b.x_nm], &[a.y_nm, b.y_nm], edge_reach);
            if window.is_empty() {
                continue;
            }
            for (j, i) in window.cells() {
                let distance = segment_distance(
                    raster.xs[i],
                    raster.ys[j],
                    a.x_nm as f64,
                    a.y_nm as f64,
                    b.x_nm as f64,
                    b.y_nm as f64,
                );
                let cell = raster.at(j, i);
                edge_ok[cell] &= distance >= edge_reach;
            }
        }
    }
    let mut objects: Vec<Copper> = Vec::new();
    for placement in &board.placements {
        let footprint = &board.footprints[&placement.footprint];
        for pad in &footprint.pads {
            if pad.kind == PadKind::Aperture {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            let shape = placed_pad_shape(position, pad, placement);
            let layers = if pad.kind == PadKind::Smd {
                vec![if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                board.stackup.copper_layers.clone()
            };
            objects.push(Copper {
                identity: ("pad".to_owned(), placement.reference.clone(), pad.number.clone()),
                layers,
                spine: shape.spine,
                radius: shape.radius_nm,
            });
        }
    }
    for (index, track) in board.tracks.iter().enumerate() {
        objects.push(Copper {
            identity: ("track".to_owned(), index.to_string(), String::new()),
            layers: vec![track.layer],
            spine: vec![track.start, track.end],
            radius: track.width_nm.div_euclid(2),
        });
    }
    let order = &board.stackup.copper_layers;
    for (index, via) in board.vias.iter().enumerate() {
        let (a, b) = (crate::layer_index(order, via.from_layer), crate::layer_index(order, via.to_layer));
        objects.push(Copper {
            identity: ("via".to_owned(), index.to_string(), String::new()),
            layers: order[a.min(b)..=a.max(b)].to_vec(),
            spine: vec![via.position],
            radius: via.size_nm.div_euclid(2),
        });
    }
    let mut net_of_pad: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_of_pad.insert((pad.component.as_str(), pad.pad.as_str()), net.name.as_str());
        }
    }
    let net_of = |identity: &(String, String, String)| -> Option<&str> {
        match identity.0.as_str() {
            "pad" => net_of_pad.get(&(identity.1.as_str(), identity.2.as_str())).copied(),
            "track" => Some(board.tracks[identity.1.parse::<usize>().expect("index")].net.as_str()),
            _ => Some(board.vias[identity.1.parse::<usize>().expect("index")].net.as_str()),
        }
    };
    let graph = explicit_copper_connectivity(board, Candidate { only_nets: Some(&zone_nets), ..Default::default() })?;
    let mut sets = Sets::default();
    for (key, root) in &graph.roots {
        sets.union(key, &format!("root:{root}"));
    }
    let holes = copper_drc::non_plated_holes(board);
    let mut unfilled = Vec::new();
    let mut zones: Vec<_> = board.zones.iter().collect();
    zones.sort_by(|a, b| (-a.priority, &a.id).cmp(&(-b.priority, &b.id)));
    let spine_window = |spine: &[Point], reach: f64| {
        let xs: Vec<i64> = spine.iter().map(|p| p.x_nm).collect();
        let ys: Vec<i64> = spine.iter().map(|p| p.y_nm).collect();
        raster.window(&xs, &ys, reach)
    };
    for zone in zones {
        let clearance = zone.clearance_nm.unwrap_or(0).max(minimum_clearance);
        let mut outer = polygon_inside(&zone.outer, &raster);
        for hole in &zone.holes {
            for (cell, inside) in outer.iter_mut().zip(polygon_inside(hole, &raster)) {
                *cell &= !inside;
            }
        }
        for layer in &zone.layers {
            let mut fill: Vec<bool> = (0..nx * ny).map(|cell| outer[cell] && material[cell] && edge_ok[cell]).collect();
            for object in &objects {
                if !object.layers.contains(layer) || net_of(&object.identity) == Some(zone.net.as_str()) {
                    continue;
                }
                let reach = (object.radius + clearance) as f64 + half;
                let window = spine_window(&object.spine, reach);
                if window.is_empty() {
                    continue;
                }
                for (j, i) in window.cells() {
                    if spine_distance(&object.spine, raster.xs[i], raster.ys[j]) < reach {
                        fill[raster.at(j, i)] = false;
                    }
                }
            }
            for (_, hole) in &holes {
                let reach = (hole.radius_nm + board.rules.minimum_hole_clearance_nm) as f64 + half;
                let window = spine_window(&hole.spine, reach);
                if !window.is_empty() {
                    for (j, i) in window.cells() {
                        if spine_distance(&hole.spine, raster.xs[i], raster.ys[j]) < reach {
                            fill[raster.at(j, i)] = false;
                        }
                    }
                }
            }
            for other in &board.zones {
                if other.net != zone.net && other.priority > zone.priority && other.layers.contains(layer) {
                    for (cell, inside) in fill.iter_mut().zip(polygon_inside(&other.outer, &raster)) {
                        *cell &= !inside;
                    }
                }
            }
            for keepout in &board.copper_keepouts {
                if keepout.block_zones && keepout.layers.contains(layer) {
                    for (cell, inside) in fill.iter_mut().zip(polygon_inside(&keepout.vertices, &raster)) {
                        *cell &= !inside;
                    }
                }
            }
            if !fill.iter().any(|cell| *cell) {
                unfilled.push(format!("{}@{}", zone.id, layer.value()));
                continue;
            }
            let labels = label(&fill, &raster);
            let _ = raster.full();
            for object in &objects {
                if !object.layers.contains(layer) || net_of(&object.identity) != Some(zone.net.as_str()) {
                    continue;
                }
                if object.identity.0 == "pad" && zone.pad_connection == ZoneConnection::None {
                    continue;
                }
                let reach = object.radius as f64 + half;
                let window = spine_window(&object.spine, reach);
                if window.is_empty() {
                    continue;
                }
                let mut hit: Vec<i32> = Vec::new();
                for (j, i) in window.cells() {
                    let value = labels[raster.at(j, i)];
                    if value > 0 && spine_distance(&object.spine, raster.xs[i], raster.ys[j]) <= reach {
                        hit.push(value);
                    }
                }
                hit.sort_unstable();
                hit.dedup();
                let names: Vec<String> = if object.identity.0 == "pad" {
                    let pad = PadReference { component: object.identity.1.clone(), pad: object.identity.2.clone() };
                    let nodes = graph.pad_nodes.get(&pad).map_or(&[][..], Vec::as_slice);
                    if nodes.is_empty() {
                        vec![format!("pad:{}.{}", object.identity.1, object.identity.2)]
                    } else {
                        nodes.iter().map(|key| format!("root:{}", graph.roots[key])).collect()
                    }
                } else {
                    vec![format!("{}:{}", object.identity.0, object.identity.1)]
                };
                for value in hit {
                    let region = format!("fill:{}:{}:{value}", zone.id, layer.value());
                    for name in &names {
                        sets.union(name, &region);
                    }
                }
            }
        }
    }
    let mut groups: IndexMap<String, i64> = IndexMap::new();
    let mut open_pads: IndexMap<String, Vec<String>> = IndexMap::new();
    for net in &board.nets {
        if !zone_nets.contains(&net.name) || net.pads.len() < 2 {
            continue;
        }
        let mut members: IndexMap<String, Vec<String>> = IndexMap::new();
        for pad in &net.pads {
            let nodes = graph.pad_nodes.get(pad).map_or(&[][..], Vec::as_slice);
            let first = match nodes.first() {
                Some(key) => format!("root:{}", graph.roots[key]),
                None => format!("pad:{}.{}", pad.component, pad.pad),
            };
            let root = sets.find(&first);
            members.entry(root).or_default().push(format!("{}.{}", pad.component, pad.pad));
        }
        groups.insert(net.name.clone(), members.len() as i64);
        if members.len() > 1 {
            // `max(members.values(), key=(len, sorted))`: the first maximum.
            let mut largest = 0;
            let key = |pads: &Vec<String>| {
                let mut sorted = pads.clone();
                sorted.sort();
                (pads.len(), sorted)
            };
            for (index, pads) in members.values().enumerate() {
                if key(pads) > key(&members[largest]) {
                    largest = index;
                }
            }
            let mut outside: Vec<String> = members
                .values()
                .enumerate()
                .filter(|(index, _)| *index != largest)
                .flat_map(|(_, pads)| pads.iter().cloned())
                .collect();
            outside.sort();
            open_pads.insert(net.name.clone(), outside);
        }
    }
    let opens = groups.values().map(|count| count - 1).sum();
    Ok(PlaneEstimate { pitch_nm, net_groups: groups, opens, unfilled_zones: unfilled, open_pads })
}

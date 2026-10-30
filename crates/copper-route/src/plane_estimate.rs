//! Raster pour connectivity (`pcbir.plane_estimate`): the component
//! labelling the unified router and supply islands share. The estimate
//! itself (`estimate_plane_connectivity`) is not implemented yet.

/// `_label`: 4-connected component labels of a row-major `(ny, nx)` mask via
/// row runs and union-find. Labels are root run ids (0 outside the mask);
/// the count is the number of run ids plus one, as Python returns it.
pub fn label(mask: &[bool], ny: usize, nx: usize) -> (Vec<i32>, usize) {
    let mut labels = vec![0i32; ny * nx];
    let mut parent: Vec<usize> = vec![0];
    fn find(parent: &mut [usize], mut a: usize) -> usize {
        while parent[a] != a {
            parent[a] = parent[parent[a]];
            a = parent[a];
        }
        a
    }
    let mut previous: Vec<(usize, usize, usize)> = Vec::new();
    for y in 0..ny {
        let row = &mask[y * nx..(y + 1) * nx];
        if !row.iter().any(|&v| v) {
            previous.clear();
            continue;
        }
        let mut runs = Vec::new();
        let mut j = 0;
        let mut x = 0;
        while x < nx {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < nx && row[x] {
                x += 1;
            }
            let end = x;
            let label = parent.len();
            parent.push(label);
            while j < previous.len() && previous[j].1 <= start {
                j += 1;
            }
            let mut k = j;
            while k < previous.len() && previous[k].0 < end {
                let (a, b) = (find(&mut parent, label), find(&mut parent, previous[k].2));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
                k += 1;
            }
            for cell in &mut labels[y * nx + start..y * nx + end] {
                *cell = label as i32;
            }
            runs.push((start, end, label));
        }
        previous = runs;
    }
    let roots: Vec<i32> = (0..parent.len()).map(|i| find(&mut parent, i) as i32).collect();
    (labels.iter().map(|&l| roots[l as usize]).collect(), parent.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_runs_across_rows() {
        let mask = [true, false, true, true, true, true];
        let (labels, count) = label(&mask, 2, 3);
        assert_eq!(count, 4);
        assert_eq!(labels, vec![1, 0, 1, 1, 1, 1]);
    }
}

/// Python's `a // b` for floats (`float_floor_div`).
pub fn float_floor_div(a: f64, b: f64) -> f64 {
    let modulo = a % b;
    let mut div = (a - modulo) / b;
    if modulo != 0.0 && ((b < 0.0) != (modulo < 0.0)) {
        div -= 1.0;
    }
    if div == 0.0 {
        return 0.0f64.copysign(a / b);
    }
    let floor = div.floor();
    if div - floor > 0.5 { floor + 1.0 } else { floor }
}

/// `PlaneEstimate`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneEstimate {
    pub pitch_nm: i64,
    /// Net -> number of disconnected groups of its pads.
    pub net_groups: indexmap::IndexMap<String, i64>,
    pub opens: i64,
    pub unfilled_zones: Vec<String>,
    /// Net -> pads outside the net's largest connected group ("REF.PAD").
    pub open_pads: indexmap::IndexMap<String, Vec<String>>,
}

type Identity = (&'static str, String, String);

/// `estimate_plane_connectivity` at the default 0.05 mm pitch.
pub fn estimate_plane_connectivity(
    board: &copper_pcb::board::PhysicalBoard,
) -> crate::unified_error::Result<PlaneEstimate> {
    use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
    use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
    use copper_physical::CopperLayer;
    use copper_physical::footprint::{PadKind, ZoneConnection};
    use indexmap::{IndexMap, IndexSet};

    use crate::unified_router::geometry::{polygon_inside, segment_distance, spine_distance};

    let pitch_nm: i64 = 50_000;
    let zones = &board.zones;
    let zone_nets: IndexSet<String> = zones.iter().map(|z| z.net.clone()).collect();
    if zones.is_empty() {
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
    let p = pitch_nm as f64;
    let xs: Vec<f64> = (0..nx).map(|i| x0 as f64 + p * (i as f64 + 0.5)).collect();
    let ys: Vec<f64> = (0..ny).map(|j| y0 as f64 + p * (j as f64 + 0.5)).collect();
    let cell = |i: usize, j: usize| (xs[i], ys[j]);
    let inside = |ring: &[copper_physical::Point]| -> Vec<bool> {
        let mut mask = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let (x, y) = cell(i, j);
                mask.push(polygon_inside(ring, x, y));
            }
        }
        mask
    };
    let mut material = inside(vertices);
    for cutout in &board.outline.cutouts {
        for (m, c) in material.iter_mut().zip(inside(&cutout.vertices)) {
            *m &= !c;
        }
    }
    let minimum_clearance = board.rules.minimum_clearance_nm;
    let half = p / 2.0;
    let edge_reach = minimum_clearance as f64 + half;
    let index = |v: f64| float_floor_div(v, p) as i64;
    let mut edge_ok = vec![true; nx * ny];
    let rings: Vec<&[copper_physical::Point]> =
        std::iter::once(&vertices[..]).chain(board.outline.cutouts.iter().map(|c| &c.vertices[..])).collect();
    for ring in rings {
        let n = ring.len();
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            let i0 = 0.max(index(a.x_nm.min(b.x_nm) as f64 - edge_reach - x0 as f64));
            let i1 = (nx as i64 - 1).min(index(a.x_nm.max(b.x_nm) as f64 + edge_reach - x0 as f64) + 1);
            let j0 = 0.max(index(a.y_nm.min(b.y_nm) as f64 - edge_reach - y0 as f64));
            let j1 = (ny as i64 - 1).min(index(a.y_nm.max(b.y_nm) as f64 + edge_reach - y0 as f64) + 1);
            if i0 > i1 || j0 > j1 {
                continue;
            }
            for j in j0 as usize..=j1 as usize {
                for i in i0 as usize..=i1 as usize {
                    let (x, y) = cell(i, j);
                    let d = segment_distance(x, y, a.x_nm as f64, a.y_nm as f64, b.x_nm as f64, b.y_nm as f64);
                    edge_ok[j * nx + i] &= d >= edge_reach;
                }
            }
        }
    }
    // (identity, layers, spine, radius, is pad)
    let mut objects: Vec<(Identity, Vec<CopperLayer>, Vec<copper_physical::Point>, i64)> = Vec::new();
    for placement in &board.placements {
        for pad in &board.footprints[&placement.footprint].pads {
            if pad.kind == PadKind::Aperture {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            let shape = placed_pad_shape(position, pad, placement);
            let layers = if pad.kind == PadKind::Smd {
                vec![if placement.side.value() == "front" { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                board.stackup.copper_layers.clone()
            };
            objects.push((
                ("pad", placement.reference.clone(), pad.number.clone()),
                layers,
                shape.spine,
                shape.radius_nm,
            ));
        }
    }
    for (i, track) in board.tracks.iter().enumerate() {
        objects.push((
            ("track", i.to_string(), String::new()),
            vec![track.layer],
            vec![track.start, track.end],
            track.width_nm.div_euclid(2),
        ));
    }
    let order = &board.stackup.copper_layers;
    let position = |l: CopperLayer| order.iter().position(|x| *x == l).expect("stackup layer");
    for (i, via) in board.vias.iter().enumerate() {
        let (a, b) = (position(via.from_layer), position(via.to_layer));
        objects.push((
            ("via", i.to_string(), String::new()),
            order[a.min(b)..=a.max(b)].to_vec(),
            vec![via.position],
            via.size_nm.div_euclid(2),
        ));
    }
    let mut net_of_pad: IndexMap<(String, String), String> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_of_pad.insert((pad.component.clone(), pad.pad.clone()), net.name.clone());
        }
    }
    let net_of = |identity: &Identity| -> Option<String> {
        match identity.0 {
            "pad" => net_of_pad.get(&(identity.1.clone(), identity.2.clone())).cloned(),
            "track" => Some(board.tracks[identity.1.parse::<usize>().expect("index")].net.clone()),
            _ => Some(board.vias[identity.1.parse::<usize>().expect("index")].net.clone()),
        }
    };
    let graph = explicit_copper_connectivity(board, Candidate { only_nets: Some(&zone_nets), ..Candidate::default() })?;
    let mut parent: IndexMap<String, String> = IndexMap::new();
    fn find(parent: &mut IndexMap<String, String>, a: &str) -> String {
        if !parent.contains_key(a) {
            parent.insert(a.to_owned(), a.to_owned());
        }
        let mut a = a.to_owned();
        while parent[&a] != a {
            let grand = parent[&parent[&a]].clone();
            parent.insert(a.clone(), grand.clone());
            a = grand;
        }
        a
    }
    fn union(parent: &mut IndexMap<String, String>, a: &str, b: &str) {
        let (a, b) = (find(parent, a), find(parent, b));
        if a != b {
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            parent.insert(high, low);
        }
    }
    for (key, root) in &graph.roots {
        union(&mut parent, key, &format!("root:{root}"));
    }
    let window = |spine: &[copper_physical::Point], reach: f64| {
        let min_x = spine.iter().map(|p| p.x_nm).min().expect("spine") as f64;
        let max_x = spine.iter().map(|p| p.x_nm).max().expect("spine") as f64;
        let min_y = spine.iter().map(|p| p.y_nm).min().expect("spine") as f64;
        let max_y = spine.iter().map(|p| p.y_nm).max().expect("spine") as f64;
        (
            0.max(index(min_x - reach - x0 as f64)),
            (nx as i64 - 1).min(index(max_x + reach - x0 as f64) + 1),
            0.max(index(min_y - reach - y0 as f64)),
            (ny as i64 - 1).min(index(max_y + reach - y0 as f64) + 1),
        )
    };
    let holes = copper_drc::non_plated_holes(board);
    let mut unfilled = Vec::new();
    let mut sorted_zones: Vec<&copper_pcb::board::CopperZone> = zones.iter().collect();
    sorted_zones.sort_by(|a, b| (-a.priority, &a.id).cmp(&(-b.priority, &b.id)));
    for zone in sorted_zones {
        let clearance = zone.clearance_nm.unwrap_or(0).max(minimum_clearance);
        let mut outer = inside(&zone.outer);
        for hole in &zone.holes {
            for (m, c) in outer.iter_mut().zip(inside(hole)) {
                *m &= !c;
            }
        }
        for &layer in &zone.layers {
            let mut fill: Vec<bool> = (0..nx * ny).map(|k| outer[k] && material[k] && edge_ok[k]).collect();
            let clear = |fill: &mut Vec<bool>, spine: &[copper_physical::Point], reach: f64| {
                let (i0, i1, j0, j1) = window(spine, reach);
                if i0 > i1 || j0 > j1 {
                    return;
                }
                for j in j0 as usize..=j1 as usize {
                    for i in i0 as usize..=i1 as usize {
                        let (x, y) = cell(i, j);
                        if spine_distance(spine, x, y) < reach {
                            fill[j * nx + i] = false;
                        }
                    }
                }
            };
            for (identity, layers, spine, radius) in &objects {
                if !layers.contains(&layer) || net_of(identity).as_deref() == Some(zone.net.as_str()) {
                    continue;
                }
                clear(&mut fill, spine, (radius + clearance) as f64 + half);
            }
            for (_, hole) in &holes {
                clear(&mut fill, &hole.spine, (hole.radius_nm + board.rules.minimum_hole_clearance_nm) as f64 + half);
            }
            for other in zones {
                if other.net != zone.net && other.priority > zone.priority && other.layers.contains(&layer) {
                    for (f, r) in fill.iter_mut().zip(inside(&other.outer)) {
                        *f &= !r;
                    }
                }
            }
            for keepout in &board.copper_keepouts {
                if keepout.block_zones && keepout.layers.contains(&layer) {
                    for (f, r) in fill.iter_mut().zip(inside(&keepout.vertices)) {
                        *f &= !r;
                    }
                }
            }
            if !fill.iter().any(|&v| v) {
                unfilled.push(format!("{}@{}", zone.id, layer.value()));
                continue;
            }
            let (labels, _) = label(&fill, ny, nx);
            for (identity, layers, spine, radius) in &objects {
                if !layers.contains(&layer) || net_of(identity).as_deref() != Some(zone.net.as_str()) {
                    continue;
                }
                if identity.0 == "pad" && zone.pad_connection == ZoneConnection::None {
                    continue;
                }
                let reach = *radius as f64 + half;
                let (i0, i1, j0, j1) = window(spine, reach);
                if i0 > i1 || j0 > j1 {
                    continue;
                }
                let mut hit: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
                for j in j0 as usize..=j1 as usize {
                    for i in i0 as usize..=i1 as usize {
                        let (x, y) = cell(i, j);
                        let l = labels[j * nx + i];
                        if l > 0 && spine_distance(spine, x, y) <= reach {
                            hit.insert(l);
                        }
                    }
                }
                let names: Vec<String> = if identity.0 == "pad" {
                    let pad = copper_pcb::board::PadReference::new(&identity.1, &identity.2);
                    let nodes = graph.pad_nodes.get(&pad).map_or(&[][..], Vec::as_slice);
                    if nodes.is_empty() {
                        vec![format!("pad:{}.{}", identity.1, identity.2)]
                    } else {
                        nodes.iter().map(|n| format!("root:{}", graph.roots[n])).collect()
                    }
                } else {
                    vec![format!("{}:{}", identity.0, identity.1)]
                };
                for l in hit {
                    let region = format!("fill:{}:{}:{l}", zone.id, layer.value());
                    for name in &names {
                        union(&mut parent, name, &region);
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
                Some(node) => format!("root:{}", graph.roots[node]),
                None => format!("pad:{}.{}", pad.component, pad.pad),
            };
            let root = find(&mut parent, &first);
            members.entry(root).or_default().push(format!("{}.{}", pad.component, pad.pad));
        }
        groups.insert(net.name.clone(), members.len() as i64);
        if members.len() > 1 {
            let key = |pads: &Vec<String>| {
                let mut sorted = pads.clone();
                sorted.sort();
                (pads.len(), sorted)
            };
            let mut largest = 0;
            for (k, pads) in members.values().enumerate() {
                if key(pads) > key(&members[largest]) {
                    largest = k;
                }
            }
            let mut pads: Vec<String> = members
                .values()
                .enumerate()
                .filter(|(k, _)| *k != largest)
                .flat_map(|(_, pads)| pads.iter().cloned())
                .collect();
            pads.sort();
            open_pads.insert(net.name.clone(), pads);
        }
    }
    let opens = groups.values().map(|count| count - 1).sum();
    Ok(PlaneEstimate { pitch_nm, net_groups: groups, opens, unfilled_zones: unfilled, open_pads })
}

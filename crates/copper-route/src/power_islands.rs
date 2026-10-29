//! Automatic split-plane islands for supply rails (`pcbir.power_islands`).
//!
//! Each supply rail gets copper islands around its own pads and vias on one
//! plane layer, carved out of the layer's plane by a higher zone priority: a
//! raster Voronoi split with linked corridors, gaps between owners, and
//! outlines traced from the raster; under ball grids, the via-site matrix
//! partition (`plan_ball_grid_islands`).

use std::collections::VecDeque;

use indexmap::{IndexMap, IndexSet};

use copper_json::Value;
use copper_pcb::board::{CopperZone, IslandPolicy, PhysicalBoard, ZoneConnection};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::dedicated_plane_layers;
use copper_physical::{CopperLayer, Point};

use crate::package_patterns::ball_grid;
use crate::plane_estimate::label;
use crate::unified_error::{Result, fail};
use crate::unified_router::geometry::{polygon_inside, segment_distance};

/// `PowerPartition`.
#[derive(Clone, Debug)]
pub struct PowerPartition {
    pub layer: CopperLayer,
    pub zones: Vec<CopperZone>,
    pub rails: Vec<String>,
    pub skipped: Vec<String>,
    pub split: Vec<String>,
}

impl PowerPartition {
    /// `report()` as progress details.
    pub fn report(&self) -> Vec<(&'static str, Value)> {
        let texts = |items: &[String]| Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect());
        vec![
            ("layer", Value::from(self.layer.value())),
            ("rails", texts(&self.rails)),
            ("islands", Value::Int((self.zones.len() as i64).into())),
            ("skipped", texts(&self.skipped)),
            ("split", texts(&self.split)),
        ]
    }
}

/// `decoupled_rails`: supply nets with decoupling capacitors and no zone of their own.
pub fn decoupled_rails(board: &PhysicalBoard) -> Vec<String> {
    let zoned: IndexSet<&str> = board.zones.iter().map(|z| z.net.as_str()).collect();
    let returns: IndexSet<&str> = board.decoupling_links.iter().map(|l| l.return_net.as_str()).collect();
    let mut rails: Vec<String> = board
        .decoupling_links
        .iter()
        .map(|l| l.net.as_str())
        .filter(|n| !zoned.contains(n) && !returns.contains(n))
        .collect::<IndexSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect();
    rails.sort();
    rails
}

/// `auto_power_layer`: a plane layer that can host islands without losing a reference.
pub fn auto_power_layer(board: &PhysicalBoard) -> Option<CopperLayer> {
    let planes = dedicated_plane_layers(board);
    let mut by_net: IndexMap<&str, Vec<CopperLayer>> = IndexMap::new();
    for (layer, net) in &planes {
        by_net.entry(net.as_str()).or_default().push(*layer);
    }
    let order = &board.stackup.copper_layers;
    let index = |l: &CopperLayer| order.iter().position(|x| x == l).expect("stackup layer");
    // max() keeps the first maximal element, as Python's does.
    let latest = |layers: &[CopperLayer]| -> CopperLayer {
        let mut best = layers[0];
        for layer in &layers[1..] {
            if index(layer) > index(&best) {
                best = *layer;
            }
        }
        best
    };
    let candidates: Vec<CopperLayer> = by_net.values().filter(|l| l.len() >= 2).map(|l| latest(l)).collect();
    if candidates.is_empty() { None } else { Some(latest(&candidates)) }
}

/// `ball_grid_rails`: decoupled rails with a pad on a full ball grid.
pub fn ball_grid_rails(board: &PhysicalBoard) -> Result<Vec<String>> {
    let mut balls: IndexSet<(String, String)> = IndexSet::new();
    for pose in &board.placements {
        if let Some(grid) = ball_grid(board, &pose.reference)? {
            balls.extend(grid.balls.iter().map(|number| (pose.reference.clone(), number.clone())));
        }
    }
    let pads: IndexMap<&str, &[copper_pcb::board::PadReference]> =
        board.nets.iter().map(|net| (net.name.as_str(), net.pads.as_slice())).collect();
    Ok(decoupled_rails(board)
        .into_iter()
        .filter(|rail| {
            pads.get(rail.as_str())
                .is_some_and(|pads| pads.iter().any(|pad| balls.contains(&(pad.component.clone(), pad.pad.clone()))))
        })
        .collect())
}

/// `plan_ball_grid_islands(board, layer)` with the default options:
/// split-plane islands under ball grids from the via-site matrix, extended
/// to each rail's other pads by corridors.
pub fn plan_ball_grid_islands(board: &PhysicalBoard, layer: CopperLayer) -> Result<PowerPartition> {
    let gap_nm: i64 = 300_000;
    let pitch_nm: i64 = 50_000;
    let extend_nm: i64 = 12_000_000;
    let corridor_nm: i64 = 800_000;
    let pad_reach_nm: i64 = 600_000;
    let core_nm: i64 = 500_000;
    let rails = ball_grid_rails(board)?;
    let rules: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let excluded: Vec<String> = rails
        .iter()
        .filter(|r| {
            rules
                .get(r.as_str())
                .is_some_and(|rule| !rule.allowed_layers.is_empty() && !rule.allowed_layers.contains(&layer))
        })
        .cloned()
        .collect();
    let rails: Vec<String> = rails.into_iter().filter(|r| !excluded.contains(r)).collect();
    let host: Vec<&CopperZone> = board.zones.iter().filter(|z| z.layers.contains(&layer)).collect();
    if rails.is_empty() || host.is_empty() {
        return Ok(PowerPartition {
            layer,
            zones: Vec::new(),
            rails: Vec::new(),
            skipped: excluded,
            split: Vec::new(),
        });
    }
    let rail_index: IndexMap<&str, i32> = rails.iter().enumerate().map(|(i, r)| (r.as_str(), i as i32)).collect();
    let mut net_of: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_of.insert((pad.component.as_str(), pad.pad.as_str()), net.name.as_str());
        }
    }
    let poses = placement_map(board);
    let (rx, ry, x0, y0, grid, allowed) = host_raster(board, layer, &host, pitch_nm);
    let mut owner = vec![-1i32; rx * ry];
    // (j0, j1, i0, i1) of a square, clipped (may be empty).
    let window = |cx: i64, cy: i64, half: i64| -> (i64, i64, i64, i64) {
        (
            0.max((cy - half - y0).div_euclid(pitch_nm)),
            (ry as i64 - 1).min((cy + half - y0).div_euclid(pitch_nm)),
            0.max((cx - half - x0).div_euclid(pitch_nm)),
            (rx as i64 - 1).min((cx + half - x0).div_euclid(pitch_nm)),
        )
    };
    let paint = |owner: &mut [i32], cx: i64, cy: i64, half: i64, value: i32| {
        let (j0, j1, i0, i1) = window(cx, cy, half);
        for j in j0..=j1 {
            for i in i0..=i1 {
                owner[j as usize * rx + i as usize] = value;
            }
        }
    };

    // Under each ball grid: the via-site matrix.
    let mut placements: Vec<&copper_pcb::board::Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in placements {
        let Some(array) = ball_grid(board, &pose.reference)? else { continue };
        let (nx, ny, half) = (array.xs_nm.len() as i64, array.ys_nm.len() as i64, array.pitch_nm.div_euclid(2));
        let centre = |cx: i64, cy: i64| {
            transformed_local_point(
                pose,
                Point::new(array.xs_nm[0] + cx * array.pitch_nm + half, array.ys_nm[0] + cy * array.pitch_nm + half),
            )
        };
        let mut cells: IndexMap<(i64, i64), i32> = IndexMap::new();
        let sites: IndexMap<(i64, i64), Point> =
            (-1..nx).flat_map(|cx| (-1..ny).map(move |cy| (cx, cy))).map(|key| (key, centre(key.0, key.1))).collect();
        let tolerance = array.tolerance_nm + 1;
        for via in &board.vias {
            let Some(&index) = rail_index.get(via.net.as_str()) else { continue };
            for (key, point) in &sites {
                if (via.position.x_nm - point.x_nm).abs() <= tolerance
                    && (via.position.y_nm - point.y_nm).abs() <= tolerance
                {
                    cells.entry(*key).or_insert(index);
                }
            }
        }
        let mut lands: Vec<&copper_physical::footprint::FootprintPad> =
            board.footprints[&pose.footprint].pads.iter().filter(|p| array.balls.contains(&p.number)).collect();
        lands.sort_by(|a, b| a.number.cmp(&b.number));
        for land in lands {
            let Some(net) = net_of.get(&(pose.reference.as_str(), land.number.as_str())) else { continue };
            let Some(&index) = rail_index.get(net) else { continue };
            let Some((ix, iy)) = array.index(land.position) else {
                return fail("cannot unpack non-iterable NoneType object");
            };
            let (ix, iy) = (ix as i64, iy as i64);
            if [(0, 0), (0, 1), (1, 0), (1, 1)].iter().any(|(dx, dy)| cells.get(&(ix - dx, iy - dy)) == Some(&index)) {
                continue; // its via already marks a cell of the rail
            }
            let sx = if 2 * ix < nx - 1 { -1 } else { 1 };
            let sy = if 2 * iy < ny - 1 { -1 } else { 1 };
            cells.entry((ix - (sx < 0) as i64, iy - (sy < 0) as i64)).or_insert(index);
        }
        if cells.is_empty() {
            continue;
        }
        let mut seeds: Vec<((i64, i64), i32)> = cells.iter().map(|(k, v)| (*k, *v)).collect();
        seeds.sort();
        for key in sites.keys() {
            if !cells.contains_key(key) {
                let nearest = seeds
                    .iter()
                    .min_by_key(|(at, value)| ((at.0 - key.0).pow(2) + (at.1 - key.1).pow(2), *value, *at))
                    .expect("seeds");
                cells.insert(*key, nearest.1);
            }
        }
        for (key, &value) in &cells {
            paint(&mut owner, sites[key].x_nm, sites[key].y_nm, half, value);
        }
        for (&(cx, cy), &value) in &cells {
            for (dx, dy) in [(1, 1), (1, -1)] {
                if cells.get(&(cx + dx, cy + dy)) == Some(&value) {
                    let (a, b) = (sites[&(cx, cy)], sites[&(cx + dx, cy + dy)]);
                    paint(
                        &mut owner,
                        (a.x_nm + b.x_nm).div_euclid(2),
                        (a.y_nm + b.y_nm).div_euclid(2),
                        half.div_euclid(2),
                        value,
                    );
                }
            }
        }
    }
    for (o, a) in owner.iter_mut().zip(&allowed) {
        if !a {
            *o = -1;
        }
    }

    // Outside the arrays: corridors from each rail's other pads to its copper.
    let mut pads: IndexMap<i32, Vec<Point>> = rail_index.values().map(|&index| (index, Vec::new())).collect();
    for net in &board.nets {
        let Some(&index) = rail_index.get(net.name.as_str()) else { continue };
        for pad in &net.pads {
            let pose = &poses[pad.component.as_str()];
            if let Some(land) = board.footprints[&pose.footprint].pads.iter().find(|item| item.number == pad.pad) {
                pads[&index].push(transformed_local_point(pose, land.position));
            }
        }
    }
    let cell = |point: Point| -> (i64, i64) {
        (
            (ry as i64 - 1).min(0.max((point.y_nm - y0).div_euclid(pitch_nm))),
            (rx as i64 - 1).min(0.max((point.x_nm - x0).div_euclid(pitch_nm))),
        )
    };
    // The cells of a disc: (j, i) inside the window, row-major.
    let disc = |point: Point, radius: i64| -> Vec<usize> {
        let (j0, j1, i0, i1) = window(point.x_nm, point.y_nm, radius);
        let mut inside = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                let k = j as usize * rx + i as usize;
                if copper_physical::pyfloat::hypot(grid.px[k] - point.x_nm as f64, grid.py[k] - point.y_nm as f64)
                    <= radius as f64
                {
                    inside.push(k);
                }
            }
        }
        inside
    };
    let mut cores = vec![-1i32; rx * ry];
    for (&index, points) in &pads {
        for &point in points {
            for k in disc(point, core_nm) {
                if cores[k] != -1 && cores[k] != index {
                    cores[k] = -2;
                }
            }
            for k in disc(point, core_nm) {
                if cores[k] == -1 {
                    cores[k] = index;
                }
            }
        }
    }
    let kg = 1.max((gap_nm as f64 / pitch_nm as f64).ceil() as i64);
    let kw = 1.max(corridor_nm.div_euclid(2).div_euclid(pitch_nm));
    let band: Vec<(i64, i64)> = (-kw..=kw)
        .flat_map(|dy| (-kw..=kw).map(move |dx| (dy, dx)))
        .filter(|(dy, dx)| dx * dx + dy * dy <= kw * kw)
        .collect();
    let reach = extend_nm.div_euclid(pitch_nm);
    let (rxi, ryi) = (rx as i64, ry as i64);
    let mut indexes: Vec<i32> = pads.keys().copied().collect();
    indexes.sort();
    for index in indexes {
        let mine: Vec<bool> = owner.iter().map(|&o| o == index).collect();
        if !mine.iter().any(|&m| m) {
            continue;
        }
        let others: Vec<bool> = owner.iter().zip(&mine).map(|(&o, &m)| o >= 0 && !m).collect();
        // Other rails' copper plus the gap (np.roll wraps around).
        let mut grown = others.clone();
        let disk: Vec<(i64, i64)> = (-kg..=kg)
            .flat_map(|dy| (-kg..=kg).map(move |dx| (dy, dx)))
            .filter(|(dy, dx)| dx * dx + dy * dy <= kg * kg)
            .collect();
        for (k, _) in others.iter().enumerate().filter(|(_, o)| **o) {
            let (j, i) = ((k / rx) as i64, (k % rx) as i64);
            for &(dy, dx) in &disk {
                grown[((j + dy).rem_euclid(ryi) * rxi + (i + dx).rem_euclid(rxi)) as usize] = true;
            }
        }
        let blocked: Vec<bool> =
            (0..rx * ry).map(|k| !allowed[k] || grown[k] || (cores[k] != -1 && cores[k] != index)).collect();
        let copper: Vec<(i64, i64)> =
            (0..rx * ry).filter(|&k| mine[k]).map(|k| ((k / rx) as i64, (k % rx) as i64)).collect();
        let distance = |point: Point| -> i64 {
            let (j, i) = cell(point);
            copper.iter().map(|&(cj, ci)| (cj - j).abs() + (ci - i).abs()).min().expect("copper")
        };
        let mut order: Vec<(i64, Point)> = pads[&index].iter().map(|&p| (distance(p), p)).collect();
        order.sort_by_key(|&(d, p)| (d, p.x_nm, p.y_nm));
        for (far, point) in order {
            let start = cell(point);
            if owner[(start.0 * rxi + start.1) as usize] == index || far > reach {
                continue;
            }
            let (j0, j1) = (0.max(start.0 - reach), (ryi - 1).min(start.0 + reach));
            let (i0, i1) = (0.max(start.1 - reach), (rxi - 1).min(start.1 + reach));
            let mut previous: IndexMap<(i64, i64), Option<(i64, i64)>> = IndexMap::from([(start, None)]);
            let mut queue = VecDeque::from([start]);
            let mut found = None;
            while let Some((j, i)) = queue.pop_front() {
                if owner[(j * rxi + i) as usize] == index {
                    found = Some((j, i));
                    break;
                }
                for (dj, di) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
                    let (nj, ni) = (j + dj, i + di);
                    if j0 <= nj && nj <= j1 && i0 <= ni && ni <= i1 && !previous.contains_key(&(nj, ni)) {
                        let k = (nj * rxi + ni) as usize;
                        if owner[k] == index || !blocked[k] {
                            previous.insert((nj, ni), Some((j, i)));
                            queue.push_back((nj, ni));
                        }
                    }
                }
            }
            // None: walled in by other rails (the router joins this pad with a track).
            let Some(found) = found else { continue };
            let mut step = Some(found);
            while let Some((j, i)) = step {
                for &(dy, dx) in &band {
                    let (nj, ni) = (j + dy, i + dx);
                    if 0 <= nj && nj < ryi && 0 <= ni && ni < rxi {
                        let k = (nj * rxi + ni) as usize;
                        if !blocked[k] && owner[k] == -1 {
                            owner[k] = index;
                        }
                    }
                }
                step = previous[&(j, i)];
            }
            for k in disc(point, pad_reach_nm) {
                if owner[k] == -1 && !blocked[k] {
                    owner[k] = index;
                }
            }
        }
    }

    // Gaps between different owners (and the host plane), then islands.
    let k = 1.max((gap_nm as f64 / 2.0 / pitch_nm as f64).ceil() as i64);
    let mut kept = owner.clone();
    for dy in -k..=k {
        for dx in -k..=k {
            if dx * dx + dy * dy > k * k {
                continue;
            }
            for j in 0..ryi {
                for i in 0..rxi {
                    let (sj, si) = (j - dy, i - dx);
                    let shifted =
                        if 0 <= sj && sj < ryi && 0 <= si && si < rxi { owner[(sj * rxi + si) as usize] } else { -2 };
                    let at = (j * rxi + i) as usize;
                    if shifted != owner[at] {
                        kept[at] = -1;
                    }
                }
            }
        }
    }
    let priority = host.iter().map(|z| z.priority).max().expect("host") + 1;
    let mut zones = Vec::new();
    let mut used: IndexSet<String> = IndexSet::new();
    for (index, rail) in rails.iter().enumerate() {
        let mask: Vec<bool> = kept.iter().map(|&o| o == index as i32).collect();
        if !mask.iter().any(|&v| v) {
            continue;
        }
        let (labels, _) = label(&mask, ry, rx);
        let distinct: std::collections::BTreeSet<i32> = labels.iter().copied().filter(|&l| l > 0).collect();
        for (count, l) in distinct.into_iter().enumerate() {
            let region: Vec<bool> = labels.iter().map(|&v| v == l).collect();
            let Some((outer, holes)) = outline(&slit_holes(region, ry, rx), ry, rx, x0, y0, pitch_nm)? else {
                continue;
            };
            if outer.len() < 3 {
                continue;
            }
            used.insert(rail.clone());
            zones.push(CopperZone {
                id: format!("bga-power-{rail}-{}-{count}", layer.value()),
                net: rail.clone(),
                layers: vec![layer],
                outer,
                holes,
                priority,
                clearance_nm: None,
                minimum_width_nm: 250_000,
                pad_connection: ZoneConnection::Solid,
                thermal_gap_nm: 300_000,
                thermal_spoke_width_nm: 300_000,
                island_policy: IslandPolicy::RemoveAll,
                minimum_island_area_nm2: Some(10_000_000_000_000),
                reserve_routing: false,
                allow_same_net_hard_macro_overlap: false,
            });
        }
    }
    let kept_rails: Vec<String> = rails.iter().filter(|r| used.contains(*r)).cloned().collect();
    let mut skipped: Vec<String> = rails.iter().filter(|r| !used.contains(*r)).cloned().collect();
    skipped.extend(excluded);
    Ok(PowerPartition { layer, zones, rails: kept_rails, skipped, split: Vec::new() })
}

/// A row-major `(ny, nx)` raster.
struct Grid {
    px: Vec<f64>,
    py: Vec<f64>,
}

impl Grid {
    fn mask(&self, ring: &[Point]) -> Vec<bool> {
        self.px.iter().zip(&self.py).map(|(&x, &y)| polygon_inside(ring, x, y)).collect()
    }
}

/// `plan_power_islands(board, layer)` with the default options and `single_island`.
pub fn plan_power_islands(board: &PhysicalBoard, layer: CopperLayer) -> Result<PowerPartition> {
    let rails = decoupled_rails(board);
    let mut split: Vec<String> = Vec::new();
    loop {
        let selected: Vec<String> = rails.iter().filter(|r| !split.contains(r)).cloned().collect();
        let partition = partition(board, layer, &selected)?;
        let mut counts: IndexMap<&str, usize> = IndexMap::new();
        for zone in &partition.zones {
            *counts.entry(zone.net.as_str()).or_insert(0) += 1;
        }
        let mut more: Vec<String> = counts.iter().filter(|(_, c)| **c > 1).map(|(r, _)| (*r).to_owned()).collect();
        if more.is_empty() {
            let mut skipped = partition.skipped.clone();
            skipped.extend(split.iter().cloned());
            return Ok(PowerPartition {
                layer: partition.layer,
                zones: partition.zones,
                rails: partition.rails,
                skipped,
                split,
            });
        }
        more.sort();
        split.extend(more);
    }
}

/// The board raster of a host plane: `(nx, ny, x0, y0, cell centres, the
/// cells a rail may fill)` (inside a host zone, clear of the board edge and
/// of zone-blocking keepouts).
fn host_raster(
    board: &PhysicalBoard,
    layer: CopperLayer,
    host: &[&CopperZone],
    pitch_nm: i64,
) -> (usize, usize, i64, i64, Grid, Vec<bool>) {
    let vertices = &board.outline.vertices;
    let x0 = vertices.iter().map(|p| p.x_nm).min().expect("outline");
    let y0 = vertices.iter().map(|p| p.y_nm).min().expect("outline");
    let x1 = vertices.iter().map(|p| p.x_nm).max().expect("outline");
    let y1 = vertices.iter().map(|p| p.y_nm).max().expect("outline");
    let nx = ((x1 - x0).div_euclid(pitch_nm) + 1) as usize;
    let ny = ((y1 - y0).div_euclid(pitch_nm) + 1) as usize;
    let p = pitch_nm as f64;
    let mut grid = Grid { px: Vec::with_capacity(nx * ny), py: Vec::with_capacity(nx * ny) };
    for j in 0..ny {
        for i in 0..nx {
            grid.px.push(x0 as f64 + p * (i as f64 + 0.5));
            grid.py.push(y0 as f64 + p * (j as f64 + 0.5));
        }
    }
    let mut allowed = vec![false; nx * ny];
    for zone in host {
        let mut inside = grid.mask(&zone.outer);
        for hole in &zone.holes {
            for (m, h) in inside.iter_mut().zip(grid.mask(hole)) {
                *m &= !h;
            }
        }
        for (a, m) in allowed.iter_mut().zip(inside) {
            *a |= m;
        }
    }
    let threshold = (board.rules.minimum_clearance_nm + pitch_nm) as f64;
    for (k, a) in allowed.iter_mut().enumerate() {
        let (x, y) = (grid.px[k], grid.py[k]);
        let mut edge = f64::INFINITY;
        let n = vertices.len();
        for i in 0..n {
            let (u, v) = (vertices[i], vertices[(i + 1) % n]);
            edge = crate::unified_router::geometry::minimum(
                edge,
                segment_distance(x, y, u.x_nm as f64, u.y_nm as f64, v.x_nm as f64, v.y_nm as f64),
            );
        }
        *a &= edge > threshold;
    }
    for keepout in &board.copper_keepouts {
        if keepout.block_zones && keepout.layers.contains(&layer) {
            for (a, m) in allowed.iter_mut().zip(grid.mask(&keepout.vertices)) {
                *a &= !m;
            }
        }
    }
    (nx, ny, x0, y0, grid, allowed)
}

fn partition(board: &PhysicalBoard, layer: CopperLayer, rails: &[String]) -> Result<PowerPartition> {
    let reach_nm: i64 = 1_600_000;
    let gap_nm: i64 = 300_000;
    let corridor_nm: i64 = 1_600_000;
    let link_nm: i64 = 12_000_000;
    let pitch_nm: i64 = 100_000;
    let link_width_nm: i64 = 800_000;
    let core_nm: i64 = 500_000;
    let rules: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let excluded: Vec<String> = rails
        .iter()
        .filter(|r| {
            rules
                .get(r.as_str())
                .is_some_and(|rule| !rule.allowed_layers.is_empty() && !rule.allowed_layers.contains(&layer))
        })
        .cloned()
        .collect();
    let rails: Vec<String> = rails.iter().filter(|r| !excluded.contains(r)).cloned().collect();
    let host: Vec<&CopperZone> = board.zones.iter().filter(|z| z.layers.contains(&layer)).collect();
    if rails.is_empty() || host.is_empty() {
        return Ok(PowerPartition { layer, zones: Vec::new(), rails: Vec::new(), skipped: rails, split: Vec::new() });
    }
    let (nx, ny, x0, y0, grid, allowed) = host_raster(board, layer, &host, pitch_nm);
    let poses = placement_map(board);
    let mut seeds: IndexMap<&str, Vec<Point>> = rails.iter().map(|r| (r.as_str(), Vec::new())).collect();
    for net in &board.nets {
        let Some(points) = seeds.get_mut(net.name.as_str()) else { continue };
        for pad in &net.pads {
            let pose = &poses[pad.component.as_str()];
            for land in &board.footprints[&pose.footprint].pads {
                if land.number == pad.pad {
                    points.push(transformed_local_point(pose, land.position));
                }
            }
        }
    }
    for via in &board.vias {
        if let Some(points) = seeds.get_mut(via.net.as_str()) {
            points.push(via.position);
        }
    }
    let mut best = vec![f64::INFINITY; nx * ny];
    let mut owner = vec![-1i32; nx * ny];
    let (mut used, mut skipped) = (Vec::new(), Vec::new());
    for (index, rail) in rails.iter().enumerate() {
        let points = seeds[rail.as_str()].clone();
        if points.is_empty() {
            skipped.push(rail.clone());
            continue;
        }
        used.push(rail.clone());
        let scale = reach_nm as f64 / 1f64.max(corridor_nm as f64 / 2.0);
        let mut shapes: Vec<(Point, Point, f64)> = points.iter().map(|p| (*p, *p, 1.0)).collect();
        shapes.extend(spanning_edges(&points, link_nm).into_iter().map(|(a, b)| (a, b, scale)));
        for (a, b, factor) in shapes {
            let i0 = 0.max((a.x_nm.min(b.x_nm) - reach_nm - x0).div_euclid(pitch_nm));
            let i1 = (nx as i64 - 1).min((a.x_nm.max(b.x_nm) + reach_nm - x0).div_euclid(pitch_nm) + 1);
            let j0 = 0.max((a.y_nm.min(b.y_nm) - reach_nm - y0).div_euclid(pitch_nm));
            let j1 = (ny as i64 - 1).min((a.y_nm.max(b.y_nm) + reach_nm - y0).div_euclid(pitch_nm) + 1);
            if i0 > i1 || j0 > j1 {
                continue;
            }
            for j in j0 as usize..=j1 as usize {
                for i in i0 as usize..=i1 as usize {
                    let k = j * nx + i;
                    let d = factor
                        * segment_distance(
                            grid.px[k],
                            grid.py[k],
                            a.x_nm as f64,
                            a.y_nm as f64,
                            b.x_nm as f64,
                            b.y_nm as f64,
                        );
                    if d < reach_nm as f64 && d < best[k] {
                        best[k] = d;
                        owner[k] = index as i32;
                    }
                }
            }
        }
    }
    let claimed =
        link_corridors(&seeds, &used, &rails, &allowed, (nx, ny), (x0, y0), pitch_nm, link_nm, link_width_nm, core_nm);
    for (o, c) in owner.iter_mut().zip(&claimed) {
        if *c >= 0 {
            *o = *c;
        }
    }
    for (o, a) in owner.iter_mut().zip(&allowed) {
        if !a {
            *o = -1;
        }
    }
    let k = 1.max((gap_nm as f64 / 2.0 / pitch_nm as f64).ceil() as i64);
    let mut kept = owner.clone();
    for dy in -k..=k {
        for dx in -k..=k {
            if dx * dx + dy * dy > k * k {
                continue;
            }
            for j in 0..ny as i64 {
                for i in 0..nx as i64 {
                    let (sj, si) = (j - dy, i - dx);
                    let shifted = if 0 <= sj && sj < ny as i64 && 0 <= si && si < nx as i64 {
                        owner[sj as usize * nx + si as usize]
                    } else {
                        -2
                    };
                    let at = j as usize * nx + i as usize;
                    if shifted != owner[at] {
                        kept[at] = -1;
                    }
                }
            }
        }
    }
    let priority = host.iter().map(|z| z.priority).max().expect("host") + 1;
    let mut zones = Vec::new();
    for (index, rail) in rails.iter().enumerate() {
        if !used.contains(rail) {
            continue;
        }
        let mask: Vec<bool> = kept.iter().map(|&o| o == index as i32).collect();
        if !mask.iter().any(|&v| v) {
            continue;
        }
        let (labels, _) = label(&mask, ny, nx);
        let mut seed_cells: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
        for point in &seeds[rail.as_str()] {
            let i = (point.x_nm - x0).div_euclid(pitch_nm);
            let j = (point.y_nm - y0).div_euclid(pitch_nm);
            if 0 <= i && (i as usize) < nx && 0 <= j && (j as usize) < ny {
                let l = labels[j as usize * nx + i as usize];
                if l > 0 {
                    seed_cells.insert(l);
                }
            }
        }
        for (count, l) in seed_cells.into_iter().enumerate() {
            let region: Vec<bool> = labels.iter().map(|&v| v == l).collect();
            let Some((outer, holes)) = outline(&slit_holes(region, ny, nx), ny, nx, x0, y0, pitch_nm)? else {
                continue;
            };
            if outer.len() < 3 {
                continue;
            }
            zones.push(CopperZone {
                id: format!("auto-power-{rail}-{}-{count}", layer.value()),
                net: rail.clone(),
                layers: vec![layer],
                outer,
                holes,
                priority,
                clearance_nm: None,
                minimum_width_nm: 250_000,
                pad_connection: ZoneConnection::Solid,
                thermal_gap_nm: 300_000,
                thermal_spoke_width_nm: 300_000,
                island_policy: IslandPolicy::RemoveAll,
                minimum_island_area_nm2: Some(10_000_000_000_000),
                reserve_routing: false,
                allow_same_net_hard_macro_overlap: false,
            });
        }
    }
    skipped.extend(excluded);
    Ok(PowerPartition { layer, zones, rails: used, skipped, split: Vec::new() })
}

#[allow(clippy::too_many_arguments)]
fn link_corridors(
    seeds: &IndexMap<&str, Vec<Point>>,
    used: &[String],
    rails: &[String],
    allowed: &[bool],
    (nx, ny): (usize, usize),
    (x0, y0): (i64, i64),
    pitch: i64,
    link: i64,
    width: i64,
    core: i64,
) -> Vec<i32> {
    let cell = |point: &Point| -> (i64, i64) {
        (
            (ny as i64 - 1).min(0.max((point.y_nm - y0).div_euclid(pitch))),
            (nx as i64 - 1).min(0.max((point.x_nm - x0).div_euclid(pitch))),
        )
    };
    let at = |j: i64, i: i64| j as usize * nx + i as usize;
    let mut cores = vec![-1i32; nx * ny];
    let kc = 1.max(core.div_euclid(pitch));
    let disc: Vec<(i64, i64)> = (-kc..=kc)
        .flat_map(|dy| (-kc..=kc).map(move |dx| (dy, dx)))
        .filter(|(dy, dx)| dx * dx + dy * dy <= kc * kc)
        .collect();
    for (index, rail) in rails.iter().enumerate() {
        if !used.contains(rail) {
            continue;
        }
        for point in &seeds[rail.as_str()] {
            let (j, i) = cell(point);
            for &(dy, dx) in &disc {
                if 0 <= j + dy && j + dy < ny as i64 && 0 <= i + dx && i + dx < nx as i64 {
                    let c = &mut cores[at(j + dy, i + dx)];
                    *c = if *c == -1 || *c == index as i32 { index as i32 } else { -2 };
                }
            }
        }
    }
    let mut claimed = vec![-1i32; nx * ny];
    let kw = 1.max(width.div_euclid(2).div_euclid(pitch));
    let band: Vec<(i64, i64)> = (-kw..=kw)
        .flat_map(|dy| (-kw..=kw).map(move |dx| (dy, dx)))
        .filter(|(dy, dx)| dx * dx + dy * dy <= kw * kw)
        .collect();
    let margin = 3_000_000i64.div_euclid(pitch);
    for (index, rail) in rails.iter().enumerate() {
        if !used.contains(rail) {
            continue;
        }
        let me = index as i32;
        for (a, b) in spanning_edges(&seeds[rail.as_str()], link) {
            let ((ja, ia), (jb, ib)) = (cell(&a), cell(&b));
            let (j0, j1) = (0.max(ja.min(jb) - margin), (ny as i64 - 1).min(ja.max(jb) + margin));
            let (i0, i1) = (0.max(ia.min(ib) - margin), (nx as i64 - 1).min(ia.max(ib) + margin));
            let free = |claimed: &[i32], j: i64, i: i64| {
                let k = at(j, i);
                allowed[k] && (cores[k] == -1 || cores[k] == me) && (claimed[k] == -1 || claimed[k] == me)
            };
            let mut previous: IndexMap<(i64, i64), Option<(i64, i64)>> = IndexMap::new();
            previous.insert((ja, ia), None);
            let mut queue = VecDeque::from([(ja, ia)]);
            while !queue.is_empty() && !previous.contains_key(&(jb, ib)) {
                let (j, i) = queue.pop_front().expect("queue");
                for (dy, dx) in [(0, 1), (1, 0), (0, -1), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    let (nj, ni) = (j + dy, i + dx);
                    if j0 <= nj
                        && nj <= j1
                        && i0 <= ni
                        && ni <= i1
                        && !previous.contains_key(&(nj, ni))
                        && (free(&claimed, nj, ni) || (nj, ni) == (jb, ib))
                    {
                        previous.insert((nj, ni), Some((j, i)));
                        queue.push_back((nj, ni));
                    }
                }
            }
            if !previous.contains_key(&(jb, ib)) {
                continue;
            }
            let mut step = Some((jb, ib));
            while let Some((j, i)) = step {
                for &(dy, dx) in &band {
                    if 0 <= j + dy
                        && j + dy < ny as i64
                        && 0 <= i + dx
                        && i + dx < nx as i64
                        && free(&claimed, j + dy, i + dx)
                    {
                        claimed[at(j + dy, i + dx)] = me;
                    }
                }
                step = previous[&(j, i)];
            }
        }
    }
    claimed
}

/// `_slit_holes`: a one-cell slit from each enclosed hole up to the outside.
fn slit_holes(mut mask: Vec<bool>, ny: usize, nx: usize) -> Vec<bool> {
    loop {
        let inverse: Vec<bool> = mask.iter().map(|v| !v).collect();
        let (labels, _) = label(&inverse, ny, nx);
        let mut border: IndexSet<i32> = IndexSet::new();
        for i in 0..nx {
            border.insert(labels[i]);
            border.insert(labels[(ny - 1) * nx + i]);
        }
        for j in 0..ny {
            border.insert(labels[j * nx]);
            border.insert(labels[j * nx + nx - 1]);
        }
        let holes: std::collections::BTreeSet<i32> =
            labels.iter().copied().filter(|l| *l > 0 && !border.contains(l)).collect();
        let Some(&hole) = holes.iter().next() else { return mask };
        let first = labels.iter().position(|&l| l == hole).expect("hole cell");
        let (mut j, i) = ((first / nx) as i64, first % nx);
        j -= 1;
        while j >= 0 && mask[j as usize * nx + i] {
            mask[j as usize * nx + i] = false;
            j -= 1;
        }
    }
}

/// `_spanning_edges`: Prim's minimum spanning tree over distinct points; edges up to `limit`.
fn spanning_edges(points: &[Point], limit: i64) -> Vec<(Point, Point)> {
    let unique: Vec<(i64, i64)> =
        points.iter().map(|p| (p.x_nm, p.y_nm)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    if unique.len() < 2 {
        return Vec::new();
    }
    // abs(complex(dx, dy)) is the C library's hypot.
    let distance = |a: (i64, i64), b: (i64, i64)| ((a.0 - b.0) as f64).hypot((a.1 - b.1) as f64);
    let mut best: IndexMap<usize, (f64, usize)> =
        (1..unique.len()).map(|i| (i, (distance(unique[i], unique[0]), 0))).collect();
    let mut edges = Vec::new();
    while !best.is_empty() {
        let mut chosen = *best.keys().next().expect("best");
        for (&k, &(d, _)) in &best {
            let current = best[&chosen].0;
            if d < current || (d == current && k < chosen) {
                chosen = k;
            }
        }
        let (length, j) = best.shift_remove(&chosen).expect("chosen");
        if length <= limit as f64 {
            edges.push((Point::new(unique[chosen].0, unique[chosen].1), Point::new(unique[j].0, unique[j].1)));
        }
        for (&k, value) in best.iter_mut() {
            let d = distance(unique[k], unique[chosen]);
            if d < value.0 {
                *value = (d, chosen);
            }
        }
    }
    edges
}

type Ring = Vec<Point>;

/// `PolygonRing.__post_init__`: three distinct vertices and a nonzero area.
fn polygon_ring(vertices: Vec<Point>) -> Result<Ring> {
    let mut vertices = vertices;
    if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
        vertices.pop();
    }
    let distinct: IndexSet<Point> = vertices.iter().copied().collect();
    if vertices.len() < 3 || distinct.len() < 3 {
        return fail("a polygon ring requires three distinct vertices");
    }
    let n = vertices.len();
    let area: i128 = (0..n)
        .map(|i| {
            let (a, b) = (vertices[i], vertices[(i + 1) % n]);
            a.x_nm as i128 * b.y_nm as i128 - b.x_nm as i128 * a.y_nm as i128
        })
        .sum();
    if area == 0 {
        return fail("a polygon ring cannot have zero area");
    }
    Ok(vertices)
}

/// `_outline`: outer boundary (and holes) of one 4-connected raster region.
fn outline(mask: &[bool], ny: usize, nx: usize, x0: i64, y0: i64, pitch: i64) -> Result<Option<(Ring, Vec<Ring>)>> {
    let padded = |y: i64, x: i64| -> bool {
        // padded[y][x] with the mask at [1..=ny][1..=nx].
        if y < 1 || x < 1 || y > ny as i64 || x > nx as i64 {
            false
        } else {
            mask[(y - 1) as usize * nx + (x - 1) as usize]
        }
    };
    let mut edges: IndexMap<(i64, i64), Vec<(i64, i64)>> = IndexMap::new();
    for y in 0..ny as i64 {
        for x in 0..nx as i64 {
            if !mask[y as usize * nx + x as usize] {
                continue;
            }
            if !padded(y, x + 1) {
                edges.entry((x, y)).or_default().push((x + 1, y));
            }
            if !padded(y + 2, x + 1) {
                edges.entry((x + 1, y + 1)).or_default().push((x, y + 1));
            }
            if !padded(y + 1, x) {
                edges.entry((x, y + 1)).or_default().push((x, y));
            }
            if !padded(y + 1, x + 2) {
                edges.entry((x + 1, y)).or_default().push((x + 1, y + 1));
            }
        }
    }
    let mut loops: Vec<Vec<(i64, i64)>> = Vec::new();
    while let Some((&start, _)) = edges.first() {
        let mut lp = vec![start];
        let mut current = start;
        let mut previous: Option<(i64, i64)> = None;
        while let Some(targets) = edges.get_mut(&current).filter(|t| !t.is_empty()) {
            let pinch = if targets.len() > 1 { previous } else { None };
            let next = if let Some(prev) = pinch {
                let (dx, dy) = (current.0 - prev.0, current.1 - prev.1);
                let order = [
                    (current.0 - dy, current.1 + dx),
                    (current.0 + dx, current.1 + dy),
                    (current.0 + dy, current.1 - dx),
                ];
                let next = *order.iter().find(|t| targets.contains(t)).expect("pinch turn");
                let position = targets.iter().position(|t| *t == next).expect("target");
                targets.remove(position);
                next
            } else {
                targets.pop().expect("target")
            };
            if targets.is_empty() {
                edges.shift_remove(&current);
            }
            previous = Some(current);
            if next == start {
                break;
            }
            lp.push(next);
            current = next;
        }
        if lp.len() >= 4 {
            loops.push(lp);
        }
    }
    if loops.is_empty() {
        return Ok(None);
    }
    let area = |lp: &[(i64, i64)]| -> f64 {
        let n = lp.len();
        let twice: i64 = (0..n).map(|i| lp[i].0 * lp[(i + 1) % n].1 - lp[(i + 1) % n].0 * lp[i].1).sum();
        twice as f64 / 2.0
    };
    let ring = |lp: &[(i64, i64)]| -> Result<Ring> {
        let n = lp.len();
        let corners: Vec<Point> = (0..n)
            .filter(|&i| {
                let prev = lp[(i + n - 1) % n];
                let next = lp[(i + 1) % n];
                (lp[i].0 - prev.0) * (next.1 - lp[i].1) - (lp[i].1 - prev.1) * (next.0 - lp[i].0) != 0
            })
            .map(|i| Point::new(x0 + lp[i].0 * pitch, y0 + lp[i].1 * pitch))
            .collect();
        polygon_ring(corners)
    };
    // Stable sort, largest area first (Python's reverse sort keeps ties in order).
    let mut keyed: Vec<(f64, Vec<(i64, i64)>)> = loops.into_iter().map(|lp| (area(&lp).abs(), lp)).collect();
    keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
    let sign = area(&keyed[0].1) > 0.0;
    let outer = ring(&keyed[0].1)?;
    let mut holes = Vec::new();
    for (_, lp) in &keyed[1..] {
        if (area(lp) > 0.0) != sign {
            holes.push(ring(lp)?);
        }
    }
    Ok(Some((outer, holes)))
}

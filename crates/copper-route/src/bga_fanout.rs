//! Deterministic dogbone fanout for full-grid ball arrays (`pcbir.bga_fanout`).
//!
//! Every ball of a complete square-pitch array that needs a layer change gets
//! a short stub to a through via at the centre of one of its neighbouring
//! cells, outward from the package centre first; each stub and via is checked
//! exactly against the board and the copper accepted so far. There is no
//! search over alternatives: a ball whose sites are all blocked is reported.

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::{Object, Value};
use copper_pcb::board::{PadReference, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::{deferred_zone_nets, routing_layers};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::escape_plan::ArrayEscape;
use crate::package_patterns::ball_grid;
use crate::pair_terminals::macro_owned_pads;
use crate::pin_escape::checked_access_path;
use crate::routing_clearance::RoutingClearanceIndex;

/// `BgaFanoutResult`.
#[derive(Clone, Debug)]
pub struct BgaFanoutResult {
    pub board: PhysicalBoard,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
    pub packages: Vec<String>,
    pub fanned: Vec<PadReference>,
    pub blocked: Vec<PadReference>,
    pub surface: Vec<PadReference>,
}

impl BgaFanoutResult {
    /// `report()`, in keyword order.
    pub fn report(&self) -> Vec<(&'static str, Value)> {
        let count = |n: usize| Value::from(n as i64);
        vec![
            ("packages", Value::Array(self.packages.iter().map(|p| Value::from(p.as_str())).collect())),
            ("fanned", count(self.fanned.len())),
            ("surface", count(self.surface.len())),
            ("blocked", Value::Array(self.blocked.iter().map(|p| Value::from(p.to_string())).collect())),
            ("new_vias", count(self.vias.len())),
            ("new_tracks", count(self.tracks.len())),
        ]
    }

    pub fn report_object(&self) -> Object {
        self.report().into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
    }
}

/// `plan_bga_fanouts(board, surface_rings=2, stub_width_nm=None, array_plans=None)`.
pub fn plan_bga_fanouts(
    board: &PhysicalBoard,
    surface_rings: i64,
    stub_width_nm: Option<i64>,
    array_plans: Option<&IndexMap<String, ArrayEscape>>,
) -> Result<BgaFanoutResult> {
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let mut net_of: IndexMap<PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_of.insert(pad.clone(), net.name.as_str());
        }
    }
    let plane_nets = deferred_zone_nets(board)?;
    let owned = macro_owned_pads(board);
    let connectivity = explicit_copper_connectivity(board, Candidate::default())?;
    let copper_roots: IndexSet<&String> =
        connectivity.roots.iter().filter(|(key, _)| !key.starts_with("pad:")).map(|(_, root)| root).collect();
    let mut index = RoutingClearanceIndex::new(board)?;
    let layers = &board.stackup.copper_layers;
    let via_size = board.rules.default_via_size_nm;
    let via_drill = board.rules.default_via_drill_nm;
    let width = stub_width_nm.unwrap_or(board.rules.minimum_track_width_nm);
    let mut tracks: Vec<TrackSegment> = Vec::new();
    // A cell centre serves four balls; a same-net neighbour's via there would
    // pass the clearance check but put two holes in one place.
    let mut taken: IndexSet<Point> = board.vias.iter().map(|via| via.position).collect();
    let mut vias: Vec<Via> = Vec::new();
    let (mut packages, mut fanned, mut blocked, mut surface) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut poses: Vec<_> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        // `ball_grid` raising ValueError means no array.
        let array = match ball_grid(board, &pose.reference) {
            Ok(Some(array)) => array,
            Ok(None) | Err(crate::unified_error::Error::Value(_)) => continue,
            Err(error) => return copper_pcb::not_implemented(&error.crash_line()),
        };
        packages.push(pose.reference.clone());
        let plan = array_plans.and_then(|plans| plans.get(&pose.reference));
        let planned_surface: IndexSet<&PadReference> =
            plan.map(|plan| plan.surface.iter().collect()).unwrap_or_default();
        let side = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let footprint = &board.footprints[&pose.footprint];
        let (nx, ny) = (array.xs_nm.len() as i64, array.ys_nm.len() as i64);
        let half = array.pitch_nm.div_euclid(2);
        let cell = |position: Point| -> (i64, i64) {
            let (ix, iy) = array.index(position).expect("a ball on its grid");
            (ix as i64, iy as i64)
        };
        // Inner balls first: they have the fewest alternatives.
        let depth = |position: Point| -> i64 {
            let (ix, iy) = cell(position);
            ix.min(iy).min(nx - 1 - ix).min(ny - 1 - iy)
        };
        let mut lands: Vec<_> = footprint.pads.iter().filter(|land| array.balls.contains(&land.number)).collect();
        lands.sort_by(|a, b| (-depth(a.position), &a.number).cmp(&(-depth(b.position), &b.number)));
        for land in lands {
            let reference = PadReference { component: pose.reference.clone(), pad: land.number.clone() };
            let Some(&net) = net_of.get(&reference) else { continue };
            if owned.contains(&reference) {
                continue;
            }
            let rule = rules.get(net).copied();
            if rule.is_some_and(|rule| rule.kind != RouteKind::General) && !plane_nets.contains(net) {
                continue; // Critical routers own their own escapes.
            }
            let allowed = routing_layers(board, net, rule);
            if !allowed.iter().any(|layer| *layer != side) && !plane_nets.contains(net) {
                continue;
            }
            let ring = depth(land.position);
            let keeps_surface =
                if plan.is_some() { planned_surface.contains(&reference) } else { ring < surface_rings };
            if !plane_nets.contains(net) && keeps_surface {
                surface.push(reference);
                continue;
            }
            let launched = connectivity
                .pad_nodes
                .get(&reference)
                .is_some_and(|nodes| nodes.iter().any(|node| copper_roots.contains(&connectivity.roots[node])));
            if launched {
                continue; // Already has a launch (critical route, macro, earlier fanout).
            }
            let (ix, iy) = cell(land.position);
            let sx = if 2 * ix < nx - 1 { -1 } else { 1 };
            let sy = if 2 * iy < ny - 1 { -1 } else { 1 };
            // The four adjacent cell centres (outward first), then the next
            // ring of cell centres reached by a checked octilinear stub.
            let mut options: Vec<(i64, i64)> = vec![(sx, sy), (sx, -sy), (-sx, sy), (-sx, -sy)];
            if let Some(plan) = plan {
                // A cell centre no planned surface track crosses first.
                options.sort_by_key(|d| {
                    plan.tile_load.get(&(ix - i64::from(d.0 < 0), iy - i64::from(d.1 < 0))).copied().unwrap_or(0) > 0
                });
            }
            let mut ring3: Vec<(i64, i64)> = Vec::new();
            for dx in [-3, -1, 1, 3] {
                for dy in [-3, -1, 1, 3] {
                    if i64::max(i64::abs(dx), i64::abs(dy)) == 3 {
                        ring3.push((dx, dy));
                    }
                }
            }
            ring3.sort_by_key(|d| (-(d.0 * sx + d.1 * sy), *d));
            options.extend(ring3);
            let start = transformed_local_point(pose, land.position);
            let mut accepted = false;
            for (dx, dy) in options {
                let local = Point { x_nm: land.position.x_nm + dx * half, y_nm: land.position.y_nm + dy * half };
                let site = transformed_local_point(pose, local);
                let via = Via {
                    net: net.to_owned(),
                    position: site,
                    size_nm: via_size,
                    drill_nm: via_drill,
                    from_layer: layers[0],
                    to_layer: layers[layers.len() - 1],
                    technology: None,
                    finish: "standard".to_owned(),
                };
                if taken.contains(&site)
                    || !index.can_via(
                        net,
                        site,
                        via_size,
                        via.from_layer,
                        via.to_layer,
                        Some(via_drill),
                        false,
                        None,
                    )?
                {
                    continue;
                }
                let stubs = if dx.abs() == 1 {
                    if index.can_track(net, start, site, width, side)? {
                        Some(vec![TrackSegment { net: net.to_owned(), start, end: site, width_nm: width, layer: side }])
                    } else {
                        None
                    }
                } else {
                    checked_access_path(board, &index, net, start, site, width, side, false, true, None)?
                        .filter(|path| !path.is_empty())
                };
                let Some(stubs) = stubs.filter(|stubs| !stubs.is_empty()) else { continue };
                for stub in stubs {
                    index.add_track(stub.clone(), false, None)?;
                    tracks.push(stub);
                }
                index.add_via(via.clone(), false, None)?;
                vias.push(via);
                taken.insert(site);
                fanned.push(reference.clone());
                accepted = true;
                break;
            }
            if !accepted {
                blocked.push(reference);
            }
        }
    }
    drop(index);
    if tracks.is_empty() {
        return Ok(BgaFanoutResult {
            board: board.clone(),
            tracks: Vec::new(),
            vias: Vec::new(),
            packages,
            fanned: Vec::new(),
            blocked,
            surface,
        });
    }
    let mut result = board.clone();
    result.tracks.extend(tracks.iter().cloned());
    result.vias.extend(vias.iter().cloned());
    Ok(BgaFanoutResult { board: result, tracks, vias, packages, fanned, blocked, surface })
}

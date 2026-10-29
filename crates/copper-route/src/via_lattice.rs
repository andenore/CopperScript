//! Package via-site lattices, reserved before anything else is routed
//! (`pcbir.via_lattice`, escape plan E7).
//!
//! Every connected ball that leaves its surface layer gets one dogbone via
//! of its own, before decoupling, critical or general routing. The sites are
//! chosen jointly: each ball may use one of the cell centres around it (an
//! edge ball also the cells just outside the array), a cell holds one via,
//! and an integer min-cost assignment gives the most balls a site, preferring
//! the outward diagonal and keeping signal vias out of the outer cell rings.
//! Every site and stub is checked exactly against the board.

use indexmap::{IndexMap, IndexSet};

use copper_json::Value;
use copper_pcb::board::{PadReference, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::{deferred_zone_nets, routing_layers};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::escape_plan::MinCostFlow;
use crate::package_patterns::ball_grid;
use crate::pair_terminals::macro_owned_pads;
use crate::routing_clearance::RoutingClearanceIndex;

/// `LatticeSite`: a ball's via site; `cell` -1 or n-1 lies just outside the array.
#[derive(Clone, Debug)]
pub struct LatticeSite {
    pub pad: PadReference,
    pub net: String,
    pub position: Point,
    pub cell: (i64, i64),
    /// The outward (quadrant-to-corner) diagonal.
    pub preferred: bool,
}

/// `PackageLattice`: the sites of one package, and its balls without one.
#[derive(Clone, Debug)]
pub struct PackageLattice {
    pub component: String,
    pub pitch_nm: i64,
    pub sites: Vec<LatticeSite>,
    /// `(ball, reason)`: "no legal site", "sites taken" or "conflicts with another site".
    pub unsited: Vec<(PadReference, String)>,
}

impl PackageLattice {
    fn report(&self) -> Value {
        Value::Object(
            [
                ("component", Value::from(self.component.as_str())),
                ("sites", Value::from(self.sites.len() as i64)),
                ("preferred", Value::from(self.sites.iter().filter(|site| site.preferred).count() as i64)),
                (
                    "unsited",
                    Value::Array(
                        self.unsited
                            .iter()
                            .map(|(pad, reason)| Value::from(format!("{}.{}: {reason}", pad.component, pad.pad)))
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        )
    }
}

/// `LatticeResult`: the board with the stubs and vias added.
#[derive(Clone, Debug)]
pub struct LatticeResult {
    pub board: PhysicalBoard,
    pub lattices: Vec<PackageLattice>,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
}

impl LatticeResult {
    /// `report()`, in keyword order.
    pub fn report(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("packages", Value::Array(self.lattices.iter().map(PackageLattice::report).collect())),
            ("new_vias", Value::from(self.vias.len() as i64)),
            ("new_tracks", Value::from(self.tracks.len() as i64)),
        ]
    }
}

// Assignment costs: the outward diagonal is free; another diagonal, a cell in
// the outer band (for a signal ball) or outside the array cost extra.
const OTHER_DIAGONAL: i64 = 2;
const BAND: i64 = 4;
const OUTSIDE: i64 = 1;

/// One ball that needs a via: its reference, net, array cell, land position
/// and whether it is a signal ball (kept out of the band).
struct Ball {
    reference: PadReference,
    net: String,
    ix: i64,
    iy: i64,
    position: Point,
    signal: bool,
}

/// `plan_via_lattices(board, surface=..., surface_rings=2, band_rings=...)`:
/// one via site per ball that needs one, for every full ball grid. `surface`
/// (from `plan_array_escapes`) names the signal balls that escape on the
/// surface; without it, signal balls within `surface_rings` of the edge do.
pub fn plan_via_lattices(
    board: &PhysicalBoard,
    surface: Option<&IndexSet<PadReference>>,
    surface_rings: i64,
    band_rings: i64,
) -> Result<LatticeResult> {
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let net_of: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|net| net.pads.iter().map(move |pad| (pad, net.name.as_str()))).collect();
    let mut supplies: IndexSet<String> = deferred_zone_nets(board)?;
    for link in &board.decoupling_links {
        supplies.insert(link.net.clone());
        supplies.insert(link.return_net.clone());
    }
    let owned = macro_owned_pads(board);
    let mut index = RoutingClearanceIndex::new(board)?;
    let layers = &board.stackup.copper_layers;
    let (first, last) = (layers[0], layers[layers.len() - 1]);
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    let width = board.rules.minimum_track_width_nm;
    let (mut lattices, mut tracks, mut vias) = (Vec::new(), Vec::new(), Vec::new());
    let mut poses: Vec<_> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        // `ball_grid` raising ValueError means no array.
        let array = match ball_grid(board, &pose.reference) {
            Ok(Some(array)) => array,
            Ok(None) | Err(crate::unified_error::Error::Value(_)) => continue,
            Err(error) => return copper_pcb::not_implemented(&error.crash_line()),
        };
        let side = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let (nx, ny) = (array.xs_nm.len() as i64, array.ys_nm.len() as i64);
        let half = array.pitch_nm.div_euclid(2);
        let mut lands: Vec<_> =
            board.footprints[&pose.footprint].pads.iter().filter(|land| array.balls.contains(&land.number)).collect();
        lands.sort_by(|a, b| a.number.cmp(&b.number));
        let mut balls: Vec<Ball> = Vec::new();
        for land in lands {
            let reference = PadReference { component: pose.reference.clone(), pad: land.number.clone() };
            let Some(&net) = net_of.get(&reference) else { continue };
            if owned.contains(&reference) {
                continue;
            }
            let rule = rules.get(net).copied();
            let supply = supplies.contains(net);
            if rule.is_some_and(|rule| rule.kind != RouteKind::General) && !supply {
                continue; // critical routers own their escapes
            }
            if !supply && !routing_layers(board, net, rule).iter().any(|layer| *layer != side) {
                continue;
            }
            let (ix, iy) = array.index(land.position).expect("a ball on its grid");
            let (ix, iy) = (ix as i64, iy as i64);
            let ring = ix.min(iy).min(nx - 1 - ix).min(ny - 1 - iy);
            let on_surface = match surface {
                Some(surface) => surface.contains(&reference),
                None => ring < surface_rings,
            };
            if !supply && on_surface {
                continue;
            }
            balls.push(Ball { reference, net: net.to_owned(), ix, iy, position: land.position, signal: !supply });
        }
        if balls.is_empty() {
            continue;
        }
        // Candidate cells per ball, each checked exactly (via and stub).
        let mut cells: IndexMap<(i64, i64), usize> = IndexMap::new();
        let mut candidates: Vec<Vec<((i64, i64), Point, i64, bool)>> = Vec::new();
        for ball in &balls {
            let sx = if 2 * ball.ix < nx - 1 { -1 } else { 1 };
            let sy = if 2 * ball.iy < ny - 1 { -1 } else { 1 };
            let start = transformed_local_point(pose, ball.position);
            let mut options = Vec::new();
            for (dx, dy) in [(sx, sy), (sx, -sy), (-sx, sy), (-sx, -sy)] {
                let cell = (ball.ix - i64::from(dx < 0), ball.iy - i64::from(dy < 0));
                let outside = !((0..=nx - 2).contains(&cell.0) && (0..=ny - 2).contains(&cell.1));
                if outside && ball.signal {
                    continue; // a signal ball that leaves the array does so on the surface
                }
                let band = !outside && cell.0.min(cell.1).min(nx - 2 - cell.0).min(ny - 2 - cell.1) < band_rings;
                let site = transformed_local_point(
                    pose,
                    Point { x_nm: ball.position.x_nm + dx * half, y_nm: ball.position.y_nm + dy * half },
                );
                if !index.can_via(&ball.net, site, size, first, last, Some(drill), false, None)? {
                    continue;
                }
                if !index.can_track(&ball.net, start, site, width, side)? {
                    continue;
                }
                let preferred = (dx, dy) == (sx, sy);
                let cost = (if preferred { 0 } else { OTHER_DIAGONAL })
                    + (if band && ball.signal { BAND } else { 0 })
                    + (if outside { OUTSIDE } else { 0 });
                options.push((cell, site, cost, preferred));
                let next = cells.len();
                cells.entry(cell).or_insert(next);
            }
            candidates.push(options);
        }
        let mut flow = MinCostFlow::new(2 + balls.len() + cells.len());
        let (source, sink) = (0, 1);
        let mut edges: Vec<Vec<(usize, (i64, i64), Point, bool)>> = Vec::new();
        for (number, options) in candidates.iter().enumerate() {
            flow.add(source, 2 + number, 1, 0);
            edges.push(
                options
                    .iter()
                    .map(|&(cell, site, cost, preferred)| {
                        (flow.add(2 + number, 2 + balls.len() + cells[&cell], 1, cost), cell, site, preferred)
                    })
                    .collect(),
            );
        }
        for &number in cells.values() {
            flow.add(2 + balls.len() + number, sink, 1, 0);
        }
        flow.run(source, sink);
        let (mut sites, mut unsited) = (Vec::new(), Vec::new());
        for (number, ball) in balls.iter().enumerate() {
            let chosen = edges[number]
                .iter()
                .find(|(edge, _, _, _)| flow.edge_flow(2 + number, *edge) != 0)
                .map(|&(_, cell, site, preferred)| (cell, site, preferred));
            let Some((cell, site, preferred)) = chosen else {
                let reason = if candidates[number].is_empty() { "no legal site" } else { "sites taken" };
                unsited.push((ball.reference.clone(), reason.to_owned()));
                continue;
            };
            let stub = TrackSegment {
                net: ball.net.clone(),
                start: transformed_local_point(pose, ball.position),
                end: site,
                width_nm: width,
                layer: side,
            };
            let via = Via {
                net: ball.net.clone(),
                position: site,
                size_nm: size,
                drill_nm: drill,
                from_layer: first,
                to_layer: last,
                technology: None,
                finish: "standard".to_owned(),
            };
            // Sites were checked against the board alone; check them against
            // the sites already accepted too (one via per cell is not a proof).
            if !(index.can_via(&ball.net, site, size, first, last, Some(drill), false, None)?
                && index.can_track(&ball.net, stub.start, site, width, side)?)
            {
                unsited.push((ball.reference.clone(), "conflicts with another site".to_owned()));
                continue;
            }
            sites.push(LatticeSite {
                pad: ball.reference.clone(),
                net: ball.net.clone(),
                position: site,
                cell,
                preferred,
            });
            index.add_track(stub.clone(), false, None)?;
            index.add_via(via.clone(), false, None)?;
            tracks.push(stub);
            vias.push(via);
        }
        lattices.push(PackageLattice { component: pose.reference.clone(), pitch_nm: array.pitch_nm, sites, unsited });
    }
    let mut result = board.clone();
    if !vias.is_empty() {
        result.tracks.extend(tracks.iter().cloned());
        result.vias.extend(vias.iter().cloned());
    }
    Ok(LatticeResult { board: result, lattices, tracks, vias })
}

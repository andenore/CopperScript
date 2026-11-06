//! Package via-site lattices, reserved before anything else is routed
//! (`pcbir.via_lattice`, escape plan E7).
//!
//! Every connected recognized ball has an explicit local-site coverage state.
//! Inner balls need a dogbone independently of abstract surface-flow hints;
//! outer surface exceptions remain visibly unverified. The sites are
//! chosen jointly: each ball may use one of the cell centres around it (an
//! edge ball also the cells just outside the array), a cell holds one via,
//! and an integer min-cost assignment gives the most balls a site, preferring
//! the outward diagonal and keeping signal vias out of the outer cell rings.
//! Every site and stub is checked exactly against the board.

use indexmap::{IndexMap, IndexSet};
use std::ops::Range;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, Placement, RouteKind, TrackSegment, Via};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::routing_layers;
use copper_physical::footprint::FootprintPad;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::escape_plan::MinCostFlow;
use crate::package_patterns::ball_grid;
use crate::pair_terminals::macro_owned_pads;
use crate::pin_escape::verified_via_launch;
use crate::return_paths::existing_returns_permit;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_vias::physical_via_span;
use crate::surface_path::{track_inside_board, via_inside_board};

/// Local site coverage only; none of these states proves a boundary channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BallSiteStatus {
    Allocated,
    /// Exact occurrences in the untouched input board, freshly checked here.
    ExistingSourceLaunch {
        via_occurrence: usize,
        track_occurrences: Vec<usize>,
    },
    /// An outer ball may remain on the surface; its physical path is unverified.
    SurfacePolicy,
    Pending(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BallCoverage {
    pub pad: PadReference,
    pub net: String,
    pub ring: i64,
    pub mandatory_inner: bool,
    /// Supply destinations masked by a current foreign dedicated plane.
    /// A local contact may precede island generation; plane access is pending.
    pub pending_plane_layers: Vec<CopperLayer>,
    pub status: BallSiteStatus,
}

impl BallCoverage {
    fn report(&self) -> Value {
        let (status, reason) = match &self.status {
            BallSiteStatus::Allocated => ("allocated", Value::Null),
            BallSiteStatus::ExistingSourceLaunch { .. } => ("verified_source_launch", Value::Null),
            BallSiteStatus::SurfacePolicy => ("outer_surface_policy_unverified", Value::Null),
            BallSiteStatus::Pending(reason) => ("pending", Value::from(reason.as_str())),
        };
        let mut fields = vec![
            ("pad", Value::from(format!("{}.{}", self.pad.component, self.pad.pad))),
            ("net", Value::from(self.net.as_str())),
            ("ring", Value::from(self.ring)),
            ("mandatory_inner", Value::from(self.mandatory_inner)),
            (
                "pending_plane_layers",
                Value::Array(self.pending_plane_layers.iter().map(|layer| Value::from(layer.value())).collect()),
            ),
            ("status", Value::from(status)),
            ("reason", reason),
        ];
        if let BallSiteStatus::ExistingSourceLaunch { via_occurrence, track_occurrences } = &self.status {
            fields.push(("source_via_occurrence", Value::from(*via_occurrence as i64)));
            fields.push((
                "source_track_occurrences",
                Value::Array(track_occurrences.iter().map(|n| Value::from(*n as i64)).collect()),
            ));
        }
        Value::Object(fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
    }
}

/// `LatticeSite`: a ball's via site; `cell` -1 or n-1 lies just outside the array.
#[derive(Clone, Debug)]
pub struct LatticeSite {
    pub pad: PadReference,
    pub net: String,
    pub position: Point,
    pub cell: (i64, i64),
    /// The outward (quadrant-to-corner) diagonal.
    pub preferred: bool,
    /// Exact occurrences in LatticeResult.tracks/vias (not board indices).
    /// These locate the emitted pieces; they are not cached admission proof.
    pub track_range: Range<usize>,
    pub via_occurrence: usize,
}

/// `PackageLattice`: the sites of one package, and its balls without one.
#[derive(Clone, Debug)]
pub struct PackageLattice {
    pub component: String,
    pub pitch_nm: i64,
    pub sites: Vec<LatticeSite>,
    /// Every pending local-site requirement, including unsupported rule/source proof.
    pub unsited: Vec<(PadReference, String)>,
    /// Every connected recognized ball, including source launches and policy exceptions.
    pub coverage: Vec<BallCoverage>,
}

impl PackageLattice {
    fn report(&self) -> Value {
        Value::Object(
            [
                ("component", Value::from(self.component.as_str())),
                ("sites", Value::from(self.sites.len() as i64)),
                ("connected_balls", Value::from(self.coverage.len() as i64)),
                ("coverage_scope", Value::from("local_dogbones_and_sites_only")),
                ("boundary_channel_verification", Value::from("not_performed")),
                ("coverage", Value::Array(self.coverage.iter().map(BallCoverage::report).collect())),
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
    coverage_index: usize,
    width_nm: i64,
    span: (CopperLayer, CopperLayer, Option<String>),
}

pub(crate) fn requested_width(board: &PhysicalBoard, rule: Option<&NetRoutingRule>) -> i64 {
    rule.and_then(|rule| rule.width_nm)
        .filter(|width| *width != 0)
        .unwrap_or(board.rules.default_track_width_nm)
        .max(board.rules.minimum_track_width_nm)
}

fn coupled(rule: Option<&NetRoutingRule>) -> bool {
    rule.is_some_and(|rule| rule.kind == RouteKind::Differential || rule.differential_partner.is_some())
}

fn via_count(board: &PhysicalBoard, net: &str) -> i64 {
    board.vias.iter().filter(|via| via.net == net).count() as i64
}

/// This verifies only explicit source copper. A pad binding or coordinate alone
/// is not evidence, and a duplicate barrel at one coordinate is ambiguous.
#[allow(clippy::too_many_arguments)]
fn source_launch(
    board: &PhysicalBoard,
    source_index: &RoutingClearanceIndex<'_>,
    pose: &Placement,
    land: &FootprintPad,
    net: &str,
    side: CopperLayer,
    rule: Option<&NetRoutingRule>,
    destinations: &[CopperLayer],
) -> Result<Option<BallSiteStatus>> {
    Ok(verified_via_launch(board, source_index, pose, land, net, side, rule, destinations)?.map(|launch| {
        BallSiteStatus::ExistingSourceLaunch {
            via_occurrence: launch.via_occurrence,
            track_occurrences: launch.track_occurrences,
        }
    }))
}

pub(crate) fn stub_pieces(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width: i64,
    layer: CopperLayer,
) -> Result<Option<Vec<TrackSegment>>> {
    let pieces = index.route_pieces(net, start, end, width, layer)?;
    for piece in &pieces {
        if piece.width_nm < board.rules.minimum_track_width_nm
            || !track_inside_board(board, piece.start, piece.end, piece.width_nm)?
            || !index.can_track(net, piece.start, piece.end, piece.width_nm, layer)?
        {
            return Ok(None);
        }
    }
    Ok(Some(pieces))
}

/// `plan_via_lattices(board, surface=..., surface_rings=2, band_rings=...)`:
/// one via site per ball that needs one, for every full ball grid. `surface`
/// (from `plan_array_escapes`) may select outer signal balls for surface
/// policy only. It cannot waive an inner ball's local dogbone requirement.
pub fn plan_via_lattices(
    board: &PhysicalBoard,
    surface: Option<&IndexSet<PadReference>>,
    surface_rings: i64,
    band_rings: i64,
) -> Result<LatticeResult> {
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let net_of: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|net| net.pads.iter().map(move |pad| (pad, net.name.as_str()))).collect();
    let supplies = crate::escape_plan::ball_grid_supply_nets(board)?;
    let owned = macro_owned_pads(board);
    let source_index = RoutingClearanceIndex::new(board)?;
    let mut index = RoutingClearanceIndex::new(board)?;
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    let mut via_counts: IndexMap<String, i64> = IndexMap::new();
    for via in &board.vias {
        *via_counts.entry(via.net.clone()).or_default() += 1;
    }
    let mut taken: IndexSet<Point> = board.vias.iter().map(|via| via.position).collect();
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
        let mut coverage = Vec::new();
        for land in lands {
            let reference = PadReference { component: pose.reference.clone(), pad: land.number.clone() };
            let Some(&net) = net_of.get(&reference) else { continue };
            let rule = rules.get(net).copied();
            let supply = supplies.contains(net);
            let allowed = routing_layers(board, net, rule);
            let declared = rule
                .filter(|rule| !rule.allowed_layers.is_empty())
                .map(|rule| rule.allowed_layers.as_slice())
                .unwrap_or(&board.stackup.copper_layers);
            // Physical contact destinations differ from track availability
            // under today's host planes. Requiring the future island now
            // would make accepted-site-driven island planning circular.
            let destinations = if supply { declared } else { &allowed };
            let pending_plane_layers = if supply {
                declared.iter().filter(|layer| **layer != side && !allowed.contains(layer)).copied().collect()
            } else {
                Vec::new()
            };
            let (ix, iy) = array.index(land.position).expect("a ball on its grid");
            let (ix, iy) = (ix as i64, iy as i64);
            let ring = ix.min(iy).min(nx - 1 - ix).min(ny - 1 - iy);
            let mandatory_inner = ring >= surface_rings.max(0);
            let on_surface = !mandatory_inner
                && match surface {
                    Some(surface) => surface.contains(&reference),
                    None => true,
                };
            let coverage_index = coverage.len();
            coverage.push(BallCoverage {
                pad: reference.clone(),
                net: net.to_owned(),
                ring,
                mandatory_inner,
                pending_plane_layers,
                status: BallSiteStatus::Pending("not planned".to_owned()),
            });
            if board.nets.iter().filter(|net| net.pads.contains(&reference)).count() != 1 {
                coverage[coverage_index].status =
                    BallSiteStatus::Pending("ambiguous electrical pad binding".to_owned());
                continue;
            }
            if let Some(status) = source_launch(board, &source_index, pose, land, net, side, rule, destinations)? {
                coverage[coverage_index].status = status;
                continue;
            }
            if !supply && on_surface {
                coverage[coverage_index].status = BallSiteStatus::SurfacePolicy;
                continue;
            }
            let reason = if !declared.contains(&side) {
                Some("pad-face layer excluded by source rule")
            } else if coupled(rule) {
                Some("coupled launch qualification unsupported")
            } else if owned.contains(&reference) {
                Some("macro-owned pad has no verified rule-qualified source dogbone")
            } else if !declared.iter().any(|layer| *layer != side) {
                Some("no declared off-surface destination")
            } else if !allowed.contains(&side) || !destinations.iter().any(|layer| *layer != side) {
                Some("declared routing layer reserved by current plane")
            } else if rule.and_then(|rule| rule.max_vias).is_some_and(|maximum| via_count(board, net) >= maximum) {
                Some("via budget exhausted")
            } else if rule
                .is_some_and(|rule| rule.maximum_stub_length_nm.is_some() || rule.maximum_uncoupled_length_nm.is_some())
            {
                Some("local stub or uncoupled topology qualification unsupported")
            } else {
                None
            };
            if let Some(reason) = reason {
                coverage[coverage_index].status = BallSiteStatus::Pending(reason.to_owned());
                continue;
            }
            let Some(span) = destinations
                .iter()
                .filter(|layer| **layer != side)
                .find_map(|layer| physical_via_span(board, side, *layer))
            else {
                coverage[coverage_index].status = BallSiteStatus::Pending("no legal via technology".to_owned());
                continue;
            };
            balls.push(Ball {
                reference,
                net: net.to_owned(),
                ix,
                iy,
                position: land.position,
                signal: !supply,
                coverage_index,
                width_nm: requested_width(board, rule),
                span,
            });
        }
        // Candidate cells per ball, each checked exactly (via and stub).
        let mut cells: IndexMap<(i64, i64), usize> = IndexMap::new();
        let mut candidates: Vec<Vec<((i64, i64), Point, i64, bool)>> = Vec::new();
        let mut candidate_reasons = Vec::new();
        for ball in &balls {
            let sx = if 2 * ball.ix < nx - 1 { -1 } else { 1 };
            let sy = if 2 * ball.iy < ny - 1 { -1 } else { 1 };
            let start = transformed_local_point(pose, ball.position);
            let mut options = Vec::new();
            let mut missing_return = false;
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
                let via = Via {
                    net: ball.net.clone(),
                    position: site,
                    size_nm: size,
                    drill_nm: drill,
                    from_layer: ball.span.0,
                    to_layer: ball.span.1,
                    technology: ball.span.2.clone(),
                    finish: "standard".to_owned(),
                };
                if taken.contains(&site)
                    || !via_inside_board(board, site, size)?
                    || !index.can_via(&ball.net, site, size, ball.span.0, ball.span.1, Some(drill), false, None)?
                {
                    continue;
                }
                if stub_pieces(board, &index, &ball.net, start, site, ball.width_nm, side)?.is_none() {
                    continue;
                }
                if !existing_returns_permit(board, rules.get(ball.net.as_str()).copied(), &via) {
                    missing_return = true;
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
            candidate_reasons.push(if missing_return {
                "required existing return via unavailable"
            } else {
                "no legal site"
            });
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
        let mut sites = Vec::new();
        for (number, ball) in balls.iter().enumerate() {
            let chosen = edges[number]
                .iter()
                .find(|(edge, _, _, _)| flow.edge_flow(2 + number, *edge) != 0)
                .map(|&(_, cell, site, preferred)| (cell, site, preferred));
            let Some((cell, site, preferred)) = chosen else {
                let reason = if candidates[number].is_empty() { candidate_reasons[number] } else { "sites taken" };
                coverage[ball.coverage_index].status = BallSiteStatus::Pending(reason.to_owned());
                continue;
            };
            let via = Via {
                net: ball.net.clone(),
                position: site,
                size_nm: size,
                drill_nm: drill,
                from_layer: ball.span.0,
                to_layer: ball.span.1,
                technology: ball.span.2.clone(),
                finish: "standard".to_owned(),
            };
            // Sites were checked against the board alone; check them against
            // the sites already accepted too (one via per cell is not a proof).
            let rule = rules.get(ball.net.as_str()).copied();
            if rule
                .and_then(|rule| rule.max_vias)
                .is_some_and(|maximum| via_counts.get(&ball.net).copied().unwrap_or(0) >= maximum)
            {
                coverage[ball.coverage_index].status = BallSiteStatus::Pending("via budget exhausted".to_owned());
                continue;
            }
            let stubs = stub_pieces(
                board,
                &index,
                &ball.net,
                transformed_local_point(pose, ball.position),
                site,
                ball.width_nm,
                side,
            )?;
            if taken.contains(&site)
                || !index.can_via(&ball.net, site, size, ball.span.0, ball.span.1, Some(drill), false, None)?
                || stubs.is_none()
            {
                coverage[ball.coverage_index].status =
                    BallSiteStatus::Pending("conflicts with another site".to_owned());
                continue;
            }
            let stubs = stubs.expect("checked pieces");
            if let Some(maximum) = rule.and_then(|rule| rule.max_length_nm) {
                let all: Vec<&TrackSegment> = board
                    .tracks
                    .iter()
                    .chain(tracks.iter())
                    .chain(stubs.iter())
                    .filter(|track| track.net == ball.net)
                    .collect();
                if crate::critical::track_length(&all) > maximum {
                    coverage[ball.coverage_index].status =
                        BallSiteStatus::Pending("local copper exceeds net length budget".to_owned());
                    continue;
                }
            }
            sites.push(LatticeSite {
                pad: ball.reference.clone(),
                net: ball.net.clone(),
                position: site,
                cell,
                preferred,
                track_range: tracks.len()..tracks.len() + stubs.len(),
                via_occurrence: vias.len(),
            });
            for stub in stubs {
                index.add_track(stub.clone(), false, None)?;
                tracks.push(stub);
            }
            index.add_via(via.clone(), false, None)?;
            taken.insert(site);
            vias.push(via);
            *via_counts.entry(ball.net.clone()).or_default() += 1;
            coverage[ball.coverage_index].status = BallSiteStatus::Allocated;
        }
        let unsited = coverage
            .iter()
            .filter_map(|ball| match &ball.status {
                BallSiteStatus::Pending(reason) => Some((ball.pad.clone(), reason.clone())),
                _ => None,
            })
            .collect();
        lattices.push(PackageLattice {
            component: pose.reference.clone(),
            pitch_nm: array.pitch_nm,
            sites,
            unsited,
            coverage,
        });
    }
    let mut result = board.clone();
    if !vias.is_empty() {
        result.tracks.extend(tracks.iter().cloned());
        result.vias.extend(vias.iter().cloned());
    }
    Ok(LatticeResult { board: result, lattices, tracks, vias })
}

#[cfg(test)]
#[path = "via_lattice_coverage_tests.rs"]
pub(crate) mod coverage_tests;

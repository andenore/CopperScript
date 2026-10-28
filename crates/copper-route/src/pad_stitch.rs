//! Connect separate copper lands that represent one logical footprint pad
//! (`pcbir.pad_stitch`).
//!
//! KiCad reports disconnected same-number lands even when the electrical IR
//! treats them as one pin. This conservative closure stage adds bounded,
//! DRC-checked surface tracks; blocked lands remain explicit pending work.

use std::collections::BTreeSet;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, Connectivity, explicit_copper_connectivity};
use copper_pcb::board::{PadReference, PhysicalBoard, RouteKind, TrackSegment};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::distribution_zone_nets;
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::surface_path_detour;

/// `DuplicatePadStitchResult`.
#[derive(Clone, Debug)]
pub struct DuplicatePadStitchResult {
    pub board: PhysicalBoard,
    pub stitched: Vec<PadReference>,
    pub pending: Vec<PadReference>,
    pub added_track_count: i64,
    pub already_connected: Vec<PadReference>,
}

fn with_tracks(board: &PhysicalBoard, extra: &[&[TrackSegment]]) -> PhysicalBoard {
    let mut current = board.clone();
    for tracks in extra {
        current.tracks.extend(tracks.iter().cloned());
    }
    current
}

fn net_graph(board: &PhysicalBoard, net: &str) -> Result<Connectivity> {
    let only: IndexSet<String> = IndexSet::from([net.to_owned()]);
    explicit_copper_connectivity(board, Candidate { only_nets: Some(&only), ..Candidate::default() })
}

/// `stitch_duplicate_pads`: join disconnected SMD land groups; reuse any
/// existing physical path.
pub fn stitch_duplicate_pads(board: &PhysicalBoard) -> Result<DuplicatePadStitchResult> {
    let mut net_by_pad: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for reference in &net.pads {
            net_by_pad.insert(reference, net.name.as_str());
        }
    }
    let zone_nets = distribution_zone_nets(board);
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let mut clearance = RoutingClearanceIndex::new(board)?;
    let mut added: Vec<TrackSegment> = Vec::new();
    let (mut stitched, mut pending, mut already_connected) = (Vec::new(), Vec::new(), Vec::new());
    let mut placements: Vec<_> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in placements {
        let footprint = &board.footprints[&placement.footprint];
        let numbers: BTreeSet<&str> =
            footprint.pads.iter().filter(|pad| !pad.number.is_empty()).map(|pad| pad.number.as_str()).collect();
        for number in numbers {
            let reference = PadReference { component: placement.reference.clone(), pad: number.to_owned() };
            let Some(&net) = net_by_pad.get(&reference) else { continue };
            let lands: Vec<(usize, _)> = footprint
                .pads
                .iter()
                .enumerate()
                .filter(|(_, pad)| {
                    pad.number == number && !matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
                })
                .collect();
            if lands.len() < 2 {
                continue;
            }
            let current = with_tracks(board, &[&added]);
            let mut graph = net_graph(&current, net)?;
            if graph.pad_connected(&reference) {
                stitched.push(reference.clone());
                already_connected.push(reference);
                continue;
            }
            if zone_nets.contains(net) {
                // Separate lands may each reach the same filled plane; leave
                // their connectivity to the independent refill check.
                pending.push(reference);
                continue;
            }
            if lands.iter().any(|(_, pad)| pad.kind != PadKind::Smd) {
                pending.push(reference);
                continue;
            }
            let positions: Vec<(String, Point)> = lands
                .iter()
                .map(|(index, pad)| {
                    (
                        format!("pad:{}.{}:{}", placement.reference, number, index),
                        transformed_local_point(placement, pad.position),
                    )
                })
                .collect();
            let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
            let rule = rules.get(net).copied();
            if rule.is_some_and(|rule| rule.kind != RouteKind::General) {
                // Only the owning critical router may synthesize new copper here.
                pending.push(reference);
                continue;
            }
            if rule.is_some_and(|rule| !rule.allowed_layers.is_empty() && !rule.allowed_layers.contains(&side)) {
                pending.push(reference);
                continue;
            }
            let width = rule
                .and_then(|rule| rule.width_nm)
                .unwrap_or(board.rules.default_track_width_nm)
                .max(board.rules.minimum_track_width_nm);
            let mut local_tracks: Vec<TrackSegment> = Vec::new();
            let mut blocked = false;
            while !graph.pad_connected(&reference) {
                let mut groups: IndexMap<String, Vec<Point>> = IndexMap::new();
                for (node, position) in &positions {
                    groups.entry(graph.roots[node].clone()).or_default().push(*position);
                }
                let mut proposals: Vec<(i64, Point, Point)> = Vec::new();
                for (first, starts) in &groups {
                    for (second, ends) in &groups {
                        if first >= second {
                            continue;
                        }
                        for start in starts {
                            for end in ends {
                                let distance = (start.x_nm - end.x_nm).abs() + (start.y_nm - end.y_nm).abs();
                                proposals.push((distance, *start, *end));
                            }
                        }
                    }
                }
                proposals.sort_by_key(|(distance, start, end)| (*distance, start.x_nm, start.y_nm, end.x_nm, end.y_nm));
                let mut chosen = None;
                for (_, start, end) in proposals {
                    let committed: Vec<TrackSegment> =
                        board.tracks.iter().chain(&added).chain(&local_tracks).cloned().collect();
                    let path = surface_path_detour(
                        board, &clearance, net, start, end, width, side, &committed, 3_000_000, 500_000,
                    )?;
                    if let Some(path) = path.filter(|path| !path.is_empty()) {
                        let candidate = with_tracks(board, &[&added, &local_tracks, &path]);
                        let candidate_graph = net_graph(&candidate, net)?;
                        let roots: IndexSet<&String> =
                            positions.iter().map(|(node, _)| &candidate_graph.roots[node]).collect();
                        if roots.len() < groups.len() {
                            chosen = Some((path, candidate_graph));
                            break;
                        }
                    }
                }
                let Some((tracks, next)) = chosen else {
                    pending.push(reference.clone());
                    added.append(&mut local_tracks);
                    blocked = true;
                    break;
                };
                graph = next;
                for track in tracks {
                    clearance.add_track(track.clone(), false, None)?;
                    local_tracks.push(track);
                }
            }
            if !blocked {
                stitched.push(reference);
                added.append(&mut local_tracks);
            }
        }
    }
    drop(clearance);
    let added_track_count = added.len() as i64;
    let routed = if added.is_empty() { board.clone() } else { with_tracks(board, &[&added]) };
    Ok(DuplicatePadStitchResult { board: routed, stitched, pending, added_track_count, already_connected })
}

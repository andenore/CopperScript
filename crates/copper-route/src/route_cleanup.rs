//! Conservative cleanup of *owned* ordinary tracks, never input copper
//! (`pcbir.route_cleanup`).
//!
//! Only the final sweep (`trim_dangling_overhangs`) and the robust contact
//! score it measures with are ported. A net whose score shows open track ends
//! needs `_prune_open_ends` (the stub pruner), which reports not implemented.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, CopperContact, copper_contacts_overlap, explicit_copper_connectivity, via_copper_contact};
use copper_pcb::board::{PhysicalBoard, PhysicalNet, RouteKind, TrackSegment, Via};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::shapes_clear;
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::{Error, Result};

/// Released copper must not leave connectivity that depends on a grazing
/// touch: every contact is judged with tracks and vias shrunk by this much.
pub const CONTACT_MARGIN_NM: i64 = 1_000;

/// A multiset of tracks (`collections.Counter`).
pub type TrackCounter = IndexMap<TrackSegment, i64>;

pub fn counter<'a>(tracks: impl IntoIterator<Item = &'a TrackSegment>) -> TrackCounter {
    let mut counts = TrackCounter::new();
    for track in tracks {
        *counts.entry(track.clone()).or_insert(0) += 1;
    }
    counts
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> RoundedConvexShape {
    RoundedConvexShape::new(spine, radius_nm).expect("a non-empty spine and radius")
}

fn robust_shape(track: &TrackSegment) -> RoundedConvexShape {
    shape(vec![track.start, track.end], 1.max(track.width_nm.div_euclid(2) - CONTACT_MARGIN_NM))
}

fn robust_via(via: &Via) -> RoundedConvexShape {
    shape(vec![via.position], 1.max(via.size_nm.div_euclid(2) - CONTACT_MARGIN_NM))
}

/// `_net_pad_shapes`: (layers, shape) of every physical land of `net`, with group members.
fn net_pad_shapes(board: &PhysicalBoard, net: &str) -> Vec<(Vec<CopperLayer>, RoundedConvexShape)> {
    let mut numbers: IndexMap<&str, IndexSet<&str>> = IndexMap::new();
    for item in board.nets.iter().filter(|item| item.name == net) {
        for pad in &item.pads {
            numbers.entry(pad.component.as_str()).or_default().insert(pad.pad.as_str());
        }
    }
    let mut shapes = Vec::new();
    for placement in &board.placements {
        let Some(assigned) = numbers.get(placement.reference.as_str()) else { continue };
        let footprint = &board.footprints[&placement.footprint];
        let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let mut assigned: IndexSet<&str> = assigned.clone();
        for group in &footprint.internal_pad_groups {
            if group.numbers.iter().any(|number| assigned.contains(number.as_str())) {
                assigned.extend(group.numbers.iter().map(String::as_str));
            }
        }
        for pad in &footprint.pads {
            if !assigned.contains(pad.number.as_str()) || matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
            {
                continue;
            }
            let layers = if pad.kind == PadKind::Smd { vec![side] } else { board.stackup.copper_layers.clone() };
            shapes.push((layers, placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement)));
        }
    }
    shapes
}

/// `_Settler`: the robust contact score for one net's copper.
struct Settler<'a> {
    board: &'a PhysicalBoard,
    name: &'a str,
    net: &'a PhysicalNet,
    pads: Vec<(Vec<CopperLayer>, RoundedConvexShape)>,
    order: Vec<CopperLayer>,
}

impl Settler<'_> {
    fn index(&self, layer: CopperLayer) -> usize {
        self.order.iter().position(|item| *item == layer).expect("a stackup layer")
    }

    fn via_layers(&self, via: &Via, tracks: &[TrackSegment]) -> Result<IndexSet<CopperLayer>> {
        let mut contact = via_copper_contact(self.board, via, "via".to_owned())?;
        let mut layers: IndexSet<CopperLayer> = IndexSet::new();
        for track in tracks {
            let other = CopperContact {
                identity: "track".to_owned(),
                net: track.net.clone(),
                layers: vec![track.layer],
                shape: robust_shape(track),
                drill: None,
                capped_layers: Vec::new(),
            };
            if copper_contacts_overlap(&contact, &other) {
                layers.insert(track.layer);
            }
        }
        contact.shape = robust_via(via);
        for (pad_layers, pad) in &self.pads {
            for layer in pad_layers {
                let other = CopperContact {
                    identity: "pad".to_owned(),
                    net: via.net.clone(),
                    layers: vec![*layer],
                    shape: pad.clone(),
                    drill: None,
                    capped_layers: Vec::new(),
                };
                if copper_contacts_overlap(&contact, &other) {
                    layers.insert(*layer);
                }
            }
        }
        Ok(layers)
    }

    fn covers(&self, via: &Via, layer: CopperLayer) -> bool {
        let (a, b) = (self.index(via.from_layer), self.index(via.to_layer));
        (a.min(b)..=a.max(b)).contains(&self.index(layer))
    }

    /// `score`: (islands, open track ends, single-layer vias), or None if open.
    fn score(&self, tracks: &[TrackSegment], vias: &[Via]) -> Result<Option<(usize, usize, usize)>> {
        let only: IndexSet<String> = IndexSet::from([self.name.to_owned()]);
        let shrunk_tracks: Vec<TrackSegment> = tracks
            .iter()
            .map(|t| TrackSegment { width_nm: 1.max(t.width_nm - 2 * CONTACT_MARGIN_NM), ..t.clone() })
            .collect();
        let shrunk_vias: Vec<Via> = vias
            .iter()
            .map(|v| Via { size_nm: (v.drill_nm + 1).max(v.size_nm - 2 * CONTACT_MARGIN_NM), ..v.clone() })
            .collect();
        let graph = explicit_copper_connectivity(
            self.board,
            Candidate { only_nets: Some(&only), tracks: Some(&shrunk_tracks), vias: Some(&shrunk_vias), ..Candidate::default() },
        )?;
        if !graph.net_connected(self.net) {
            return Ok(None);
        }
        let mut single = 0;
        for via in vias {
            if self.via_layers(via, tracks)?.len() < 2 {
                single += 1;
            }
        }
        let roots: IndexSet<&String> = graph.roots.values().collect();
        Ok(Some((roots.len(), self.open_track_ends(tracks, vias).len(), single)))
    }

    /// `open_track_ends`: track indices, repeated if both robust endpoints are unconnected.
    fn open_track_ends(&self, tracks: &[TrackSegment], vias: &[Via]) -> Vec<usize> {
        let mut by_layer: BTreeMap<usize, Vec<(Option<usize>, RoundedConvexShape)>> = BTreeMap::new();
        for (index, track) in tracks.iter().enumerate() {
            by_layer.entry(self.index(track.layer)).or_default().push((Some(index), robust_shape(track)));
        }
        for via in vias {
            for layer in &self.order {
                if self.covers(via, *layer) {
                    by_layer.entry(self.index(*layer)).or_default().push((None, robust_via(via)));
                }
            }
        }
        for (layers, pad) in &self.pads {
            for layer in layers {
                by_layer.entry(self.index(*layer)).or_default().push((None, pad.clone()));
            }
        }
        let mut open = Vec::new();
        for (index, track) in tracks.iter().enumerate() {
            let shapes = by_layer.get(&self.index(track.layer)).map(Vec::as_slice).unwrap_or(&[]);
            for point in [track.start, track.end] {
                let end = shape(vec![point], 1.max(track.width_nm.div_euclid(2) - CONTACT_MARGIN_NM));
                if !shapes.iter().any(|(other, item)| *other != Some(index) && !shapes_clear(&end, item, 1)) {
                    open.push(index);
                }
            }
        }
        open
    }
}

/// `trim_dangling_overhangs`: cut routed tracks back to the land they cross
/// when their far end is open. `protected` occurrences (input and hard-macro
/// copper) and critical, zone and incomplete nets never change.
pub fn trim_dangling_overhangs(board: &PhysicalBoard, protected: &TrackCounter) -> Result<PhysicalBoard> {
    let mut skip: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    skip.extend(board.net_routing_rules.iter().filter(|rule| rule.kind != RouteKind::General).map(|rule| rule.net.as_str()));
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|net| (net.name.as_str(), net)).collect();
    let mut by_net: BTreeMap<&str, Vec<TrackSegment>> = BTreeMap::new();
    for track in &board.tracks {
        by_net.entry(track.net.as_str()).or_default().push(track.clone());
    }
    for (name, tracks) in &by_net {
        let mut owned = counter(tracks);
        for (track, count) in protected {
            if let Some(value) = owned.get_mut(track) {
                *value -= count;
            }
        }
        owned.retain(|_, count| *count > 0);
        let Some(net) = nets.get(name) else { continue };
        if skip.contains(name) || owned.is_empty() {
            continue;
        }
        let settle = Settler {
            board,
            name,
            net,
            pads: net_pad_shapes(board, name),
            order: board.stackup.copper_layers.clone(),
        };
        let vias: Vec<Via> = board.vias.iter().filter(|via| via.net == *name).cloned().collect();
        let Some(score) = settle.score(tracks, &vias)? else { continue };
        if score.1 == 0 {
            continue;
        }
        return Err(Error::NotImplemented(format!(
            "route_cleanup._prune_open_ends (net {} has open track ends)",
            copper_json::python_str_repr(name)
        )));
    }
    Ok(board.clone())
}

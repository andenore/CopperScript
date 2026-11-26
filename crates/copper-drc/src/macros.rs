//! Hard-macro materialization and its physical acceptance
//! (`hard_macros.materialize_hard_macros`, `materialize_macro_copper`).

use indexmap::{IndexMap, IndexSet};

use copper_json::python_str_repr as repr;
use copper_pcb::board::*;
use copper_pcb::hard_macros::{
    cluster_placement_matches, host_zone_macro_overlap, resolved_macro_geometry, resolved_macro_polygons,
    resolved_macro_zones, validate_hard_macros, zone_outlines_overlap,
};
use copper_pcb::keepouts::placement_map;
use copper_pcb::{Error, Result};
use copper_physical::geometry::{point_in_polygon, shapes_clear};

use crate::connectivity::{Candidate, explicit_copper_connectivity};
use crate::{Policy, Severity, run_physical_drc, shape};

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

/// Commits required via-in-pad arrays
/// (`pad_via_arrays.materialize_via_in_pad_arrays`). It checks every site
/// against the routing clearance index, which lives in `copper-route` (a
/// dependent of this crate), so the binary registers it.
pub type ArrayMaterializer = fn(&PhysicalBoard) -> Result<PhysicalBoard>;

static ARRAY_MATERIALIZER: std::sync::OnceLock<ArrayMaterializer> = std::sync::OnceLock::new();

/// Install the via-in-pad array materializer; the first registration wins.
pub fn register_array_materializer(materializer: ArrayMaterializer) {
    let _ = ARRAY_MATERIALIZER.set(materializer);
}

/// `materialize_via_in_pad_arrays` through the registered materializer.
pub fn materialize_via_in_pad_arrays(board: PhysicalBoard) -> Result<PhysicalBoard> {
    if !board.via_in_pad_rules.iter().any(|rule| rule.rows.is_some()) {
        return Ok(board);
    }
    match ARRAY_MATERIALIZER.get() {
        Some(materialize) => materialize(&board),
        None => copper_pcb::not_implemented("via-in-pad via arrays (no array materializer is registered)"),
    }
}

/// `materialize_hard_macros`: commit the entire set, or reject the board
/// unchanged. Required via-in-pad arrays join this immutable prefix after
/// macro copper.
pub fn materialize_hard_macros(board: PhysicalBoard) -> Result<PhysicalBoard> {
    match try_materialize_hard_macros(board)? {
        MacroMaterialization::Accepted(board) => Ok(board),
        MacroMaterialization::Rejected(rejection) => fail(rejection.message()),
    }
}

/// Exact physical rejection of a fully resolved trial, never an accepted board.
#[derive(Clone, Debug)]
pub struct MacroRejection {
    pub board: PhysicalBoard,
    pub findings: Vec<crate::Finding>,
}

impl MacroRejection {
    /// Prefer an implicated macro anchor; otherwise identify the ordinary pad
    /// component actually named by DRC. No unrelated macro is a fallback.
    pub fn failed_reference(&self) -> Option<&str> {
        if let Some(cluster) = self.failed_clusters().into_iter().next() {
            return Some(cluster.anchor_reference.as_str());
        }
        self.findings.iter().flat_map(|finding| &finding.objects).find_map(|object| {
            let pad = object.strip_prefix("pad:")?;
            self.board
                .placements
                .iter()
                .filter(|pose| pad.starts_with(&format!("{}.", pose.reference)))
                .max_by_key(|pose| pose.reference.len())
                .map(|pose| pose.reference.as_str())
        })
    }

    /// Owners of the actual rejected objects, in source order. A conflict can
    /// involve two owners; never substitute the first unrelated cluster.
    pub fn failed_clusters(&self) -> Vec<&RigidPlacementCluster> {
        let objects: IndexSet<&str> = self.findings.iter().flat_map(|f| f.objects.iter().map(String::as_str)).collect();
        let (mut track_start, mut via_start) = (0, 0);
        let mut owners = Vec::new();
        for hard in &self.board.hard_macros {
            let tracks = track_start..track_start + hard.tracks.len();
            let vias = via_start..via_start + hard.vias.len();
            track_start = tracks.end;
            via_start = vias.end;
            let Some(cluster) = self.board.rigid_clusters.iter().find(|c| c.name == hard.cluster) else { continue };
            if objects.contains(format!("macro:{}", hard.cluster).as_str())
                || tracks.into_iter().any(|i| objects.contains(format!("track:{i}").as_str()))
                || vias.into_iter().any(|i| objects.contains(format!("via:{i}").as_str()))
                || hard.zones.iter().any(|z| objects.contains(z.id.as_str()))
                || hard.polygons.iter().any(|p| objects.contains(format!("polygon:{}", p.id).as_str()))
                || objects.iter().any(|object| {
                    object.strip_prefix("pad:").is_some_and(|pad| {
                        cluster.members.iter().any(|member| pad.starts_with(&format!("{}.", member.reference)))
                    })
                })
            {
                owners.push(cluster);
            }
        }
        owners
    }

    pub fn message(&self) -> String {
        format!(
            "hard-macro physical acceptance failed: {}",
            self.findings.iter().map(|f| format!("{}: {}", f.code, f.message)).collect::<Vec<_>>().join("; ")
        )
    }
}

/// A candidate-local physical rejection is distinct from invalid source or
/// unsupported materialization. Callers must never route or certify Rejected.
pub enum MacroMaterialization {
    Accepted(PhysicalBoard),
    Rejected(Box<MacroRejection>),
}

pub fn try_materialize_hard_macros(board: PhysicalBoard) -> Result<MacroMaterialization> {
    if !board.materialized_macros.is_empty() {
        validate_hard_macros(&board)?;
        let owners: IndexSet<&str> = board.hard_macros.iter().map(|m| m.cluster.as_str()).collect();
        let materialized: IndexSet<&str> = board.materialized_macros.iter().map(String::as_str).collect();
        if owners != materialized {
            return fail("partial hard-macro materialization is unsupported");
        }
        return materialize_via_in_pad_arrays(board).map(MacroMaterialization::Accepted); // idempotent
    }
    if board.hard_macros.is_empty() {
        return materialize_via_in_pad_arrays(board).map(MacroMaterialization::Accepted);
    }
    match try_materialize_macro_copper(board)? {
        MacroMaterialization::Accepted(board) => {
            materialize_via_in_pad_arrays(board).map(MacroMaterialization::Accepted)
        }
        rejected => Ok(rejected),
    }
}

/// `materialize_macro_copper`: macro copper plus its physical acceptance.
pub fn materialize_macro_copper(board: PhysicalBoard) -> Result<PhysicalBoard> {
    match try_materialize_macro_copper(board)? {
        MacroMaterialization::Accepted(board) => Ok(board),
        MacroMaterialization::Rejected(rejection) => fail(rejection.message()),
    }
}

fn try_materialize_macro_copper(board: PhysicalBoard) -> Result<MacroMaterialization> {
    if !board.tracks.is_empty() || !board.vias.is_empty() || !board.polygons.is_empty() {
        return fail("macro materialization must precede all routing and fill");
    }
    validate_hard_macros(&board)?;
    let poses = placement_map(&board);
    if !cluster_placement_matches(&board, &poses)? {
        return fail("place the complete rigid macro before materializing copper");
    }
    let (mut tracks, mut vias, mut zones, mut polygons) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for hard in &board.hard_macros {
        let geometry = resolved_macro_geometry(&board, hard, &poses)?;
        tracks.extend(geometry.tracks);
        vias.extend(geometry.vias);
        zones.extend(resolved_macro_zones(&board, hard, &poses)?);
        polygons.extend(resolved_macro_polygons(&board, hard, &poses)?);
    }
    let mut pose_findings = Vec::new();
    let zone_owners: Vec<&str> =
        board.hard_macros.iter().flat_map(|hard| hard.zones.iter().map(move |_| hard.cluster.as_str())).collect();
    for (index, zone) in zones.iter().enumerate() {
        for (other_index, other) in zones[..index].iter().enumerate() {
            if !zone_outlines_overlap(&zone.layers, &zone.outer, &other.layers, &other.outer) {
                continue;
            }
            if zone_owners[index] == zone_owners[other_index] {
                return fail("private hard-macro zones overlap");
            }
            pose_findings.push(
                crate::Finding::error(
                    "MACRO-ZONE-OVERLAP",
                    format!(
                        "private zones {} of macro {} and {} of macro {} overlap at this pose",
                        repr(&zone.id),
                        repr(zone_owners[index]),
                        repr(&other.id),
                        repr(zone_owners[other_index])
                    ),
                )
                .objects([
                    format!("macro:{}", zone_owners[index]),
                    format!("macro:{}", zone_owners[other_index]),
                    zone.id.clone(),
                    other.id.clone(),
                ]),
            );
        }
    }
    let mut result = board.clone();
    result.tracks = tracks;
    result.vias = vias;
    result.zones.extend(zones);
    result.polygons = polygons;
    result.materialized_macros = board.hard_macros.iter().map(|m| m.cluster.clone()).collect();
    result.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    // Prove authored connectivity before returning a pose rejection: a host
    // plane miss must not hide a malformed private net or port in another owner.
    for hard in &board.hard_macros {
        pose_findings.extend(prove_ports(&result, hard)?);
    }
    let hosts: Vec<&CopperZone> = board.zones.iter().collect();
    for hard in &board.hard_macros {
        if let Some(conflict) = host_zone_macro_overlap(&board, hard, &hosts)? {
            pose_findings.push(
                crate::Finding::error("MACRO-HOST-ZONE", conflict.message())
                    .objects([format!("macro:{}", conflict.cluster), conflict.zone])
                    .nets([conflict.net]),
            );
        }
    }
    if !pose_findings.is_empty() {
        return Ok(MacroMaterialization::Rejected(Box::new(MacroRejection { board: result, findings: pose_findings })));
    }
    copper_pcb::physicalize::revalidate(&mut result)?;
    let report =
        run_physical_drc(&result, Policy { require_completed_detailed_route: false, fail_on_warnings: false })?;
    let fatal: Vec<crate::Finding> =
        report.findings.iter().filter(|f| f.severity == Severity::Error && f.code != "DRC-OPEN-NET").cloned().collect();
    if !fatal.is_empty() {
        return Ok(MacroMaterialization::Rejected(Box::new(MacroRejection { board: result, findings: fatal })));
    }
    Ok(MacroMaterialization::Accepted(result))
}

/// Port, plane-return and private-net continuity of one macro's actual copper.
fn prove_ports(result: &PhysicalBoard, hard: &PhysicalHardMacro) -> Result<Vec<crate::Finding>> {
    let mut pose_findings = Vec::new();
    let plane_miss = |message: String, net: &str| {
        crate::Finding::error("MACRO-PLANE-CONTACT", message).objects([format!("macro:{}", hard.cluster)]).nets([net])
    };
    let poses = placement_map(result);
    let geometry = resolved_macro_geometry(result, hard, &poses)?;
    let (tracks, vias, ports) = (&geometry.tracks, &geometry.vias, &geometry.ports);
    let cluster = result.rigid_clusters.iter().find(|c| c.name == hard.cluster).expect("macro cluster");
    let members: IndexSet<&str> = cluster.members.iter().map(|m| m.reference.as_str()).collect();
    let member_poses: Vec<Placement> =
        result.placements.iter().filter(|p| members.contains(p.reference.as_str())).cloned().collect();
    // A port proof cannot borrow a different macro's or outsider's pads.
    let mut owner = result.clone();
    owner.footprints =
        member_poses.iter().map(|p| (p.footprint.clone(), result.footprints[&p.footprint].clone())).collect();
    owner.placements = member_poses;
    owner.nets = result
        .nets
        .iter()
        .map(|n| PhysicalNet {
            name: n.name.clone(),
            pads: n.pads.iter().filter(|p| members.contains(p.component.as_str())).cloned().collect(),
        })
        .collect();
    owner.tracks = tracks.clone();
    owner.vias = vias.clone();
    let macro_polygons = resolved_macro_polygons(result, hard, &poses)?;
    let graph =
        explicit_copper_connectivity(&owner, Candidate { polygons: Some(&macro_polygons), ..Candidate::default() })?;
    let port_nets: IndexSet<&str> = ports.iter().map(|p| p.net.as_str()).collect();
    let planes: IndexMap<&str, &MacroPlaneReturn> = hard.plane_returns.iter().map(|p| (p.net.as_str(), p)).collect();
    let pad_roots = |pad: &PadReference| -> Option<Vec<&str>> {
        graph
            .pad_nodes
            .get(pad)
            .filter(|nodes| !nodes.is_empty())
            .map(|nodes| nodes.iter().map(|n| graph.root(n)).collect())
    };
    for net in &owner.nets {
        let bound: Vec<&PadReference> =
            hard.pad_bindings.iter().filter(|(_, n)| *n == net.name).map(|(p, _)| p).collect();
        if bound.is_empty() {
            continue;
        }
        if port_nets.contains(net.name.as_str()) {
            let covered: IndexSet<&PadReference> =
                ports.iter().filter(|p| p.net == net.name).flat_map(|p| p.pads.iter()).collect();
            if !bound.iter().all(|pad| covered.contains(pad)) {
                return fail(format!("macro ports omit private pads on net {}", repr(&net.name)));
            }
            continue;
        }
        if let Some(plane) = planes.get(net.name.as_str()) {
            let zones: Vec<&CopperZone> = result
                .zones
                .iter()
                .filter(|z| z.net == net.name && z.layers.iter().any(|l| plane.layers.contains(l)))
                .collect();
            if zones.is_empty() {
                return fail(format!("macro plane return {} has no declared plane", repr(&net.name)));
            }
            let span = &result.stackup.copper_layers;
            let mut usable: IndexMap<copper_physical::Point, String> = IndexMap::new();
            let mut capable: IndexMap<copper_physical::Point, String> = IndexMap::new();
            for (index, via) in vias.iter().enumerate() {
                let a = copper_pcb::hard_macros::layer_index(result, via.from_layer)?;
                let b = copper_pcb::hard_macros::layer_index(result, via.to_layer)?;
                let barrel = &span[a.min(b)..=a.max(b)];
                if via.net == net.name && plane.layers.iter().any(|layer| barrel.contains(layer)) {
                    capable.insert(via.position, graph.root(&format!("via:{index}")).to_owned());
                }
                let reaches = zones.iter().any(|zone| {
                    plane.layers.iter().any(|layer| {
                        zone.layers.contains(layer)
                            && barrel.contains(layer)
                            && point_in_polygon(via.position, &zone.outer)
                            && !zone.holes.iter().any(|h| point_in_polygon(via.position, h))
                    })
                });
                if via.net == net.name && reaches {
                    usable.insert(via.position, graph.root(&format!("via:{index}")).to_owned());
                }
            }
            let capable_roots: IndexSet<&str> = capable.values().map(String::as_str).collect();
            if bound
                .iter()
                .any(|pad| pad_roots(pad).is_none_or(|roots| roots.iter().any(|r| !capable_roots.contains(r))))
            {
                return fail(format!("macro plane return {} lacks a local via for a private pad", repr(&net.name)));
            }
            let usable_roots: IndexSet<&str> = usable.values().map(String::as_str).collect();
            if bound
                .iter()
                .any(|pad| pad_roots(pad).is_none_or(|roots| roots.iter().any(|r| !usable_roots.contains(r))))
            {
                pose_findings.push(plane_miss(
                    format!(
                        "macro {} plane return {} does not reach the declared host plane at this pose",
                        repr(&hard.cluster),
                        repr(&net.name)
                    ),
                    &net.name,
                ));
            }
            for (pad, local) in &plane.dedicated_contacts {
                let matching: Vec<&Via> = hard
                    .vias
                    .iter()
                    .enumerate()
                    .filter(|(_, source)| source.net == net.name && source.position == *local)
                    .map(|(index, _)| &vias[index])
                    .collect();
                let missing = matching.len() != 1 || {
                    let via = matching[0];
                    match capable.get(&via.position) {
                        None => true,
                        Some(root) => graph
                            .pad_nodes
                            .get(pad)
                            .map_or(&[][..], Vec::as_slice)
                            .iter()
                            .any(|node| graph.root(node) != root),
                    }
                };
                if missing {
                    return fail(format!("macro dedicated plane contact for {pad} is missing"));
                }
                if !usable.contains_key(&matching[0].position) {
                    pose_findings.push(plane_miss(format!("macro {} dedicated plane contact for {pad} does not reach the declared host plane at this pose", repr(&hard.cluster)), &net.name));
                }
            }
            continue;
        }
        let full = result.nets.iter().find(|n| n.name == net.name).expect("owner net");
        if full.pads.iter().any(|p| !bound.contains(&p)) {
            return fail(format!("macro net {} requires an external port", repr(&net.name)));
        }
        let mut roots: IndexSet<&str> = IndexSet::new();
        for pad in &bound {
            roots.extend(graph.pad_nodes.get(*pad).map_or(&[][..], Vec::as_slice).iter().map(|n| graph.root(n)));
        }
        if roots.len() != 1 || bound.iter().any(|pad| graph.pad_nodes.get(*pad).is_none_or(Vec::is_empty)) {
            return fail(format!("macro internal net {} is not connected", repr(&net.name)));
        }
    }
    for port in ports {
        let point = shape(vec![port.position], 0)?;
        let mut roots: IndexSet<&str> = IndexSet::new();
        for (index, track) in tracks.iter().enumerate() {
            if track.net == port.net
                && track.layer == port.layer
                && !shapes_clear(&point, &shape(vec![track.start, track.end], track.width_nm / 2)?, 1)
            {
                roots.insert(graph.root(&format!("track:{index}")));
            }
        }
        for (index, via) in vias.iter().enumerate() {
            let a = copper_pcb::hard_macros::layer_index(result, via.from_layer)?;
            let b = copper_pcb::hard_macros::layer_index(result, via.to_layer)?;
            let barrel = &result.stackup.copper_layers[a.min(b)..=a.max(b)];
            if via.net == port.net && via.position == port.position && barrel.contains(&port.layer) {
                roots.insert(graph.root(&format!("via:{index}")));
            }
        }
        if roots.is_empty() {
            return fail("macro port has no owner copper contact");
        }
        let disconnected = port.pads.iter().any(|pad| {
            graph
                .pad_nodes
                .get(pad)
                .filter(|nodes| !nodes.is_empty())
                .is_none_or(|nodes| nodes.iter().any(|n| !roots.contains(graph.root(n))))
        });
        if roots.len() != 1 || disconnected {
            return fail("macro port is not connected to its declared private pads");
        }
    }
    Ok(pose_findings)
}

//! Copper keepouts at their placed poses (`pcbir.placement.resolved_copper_keepouts`,
//! `pcbir.clusters.resolved_cluster_keepouts`).

use indexmap::{IndexMap, IndexSet};

use copper_physical::{BoardSide, CopperKeepout, CopperLayer, Point};

use crate::board::{PhysicalBoard, Placement};
use crate::transform::transformed_local_point;
use crate::{Error, Result};

/// The rigid frame of a cluster whose anchor sits at `anchor`.
pub(crate) fn cluster_frame(members: &[crate::board::RigidPlacementMember], anchor: &Placement) -> Placement {
    let local = members.iter().find(|m| m.reference == anchor.reference).expect("anchor member");
    let origin = transformed_local_point(anchor, Point::new(-local.position.x_nm, -local.position.y_nm));
    Placement { position: origin, ..anchor.clone() }
}

/// The board's placements by reference.
pub fn placement_map(board: &PhysicalBoard) -> IndexMap<String, Placement> {
    board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect()
}

/// `resolved_cluster_keepouts`: cluster-local keepouts at their anchors' poses.
pub fn resolved_cluster_keepouts(
    board: &PhysicalBoard,
    placements: &IndexMap<String, Placement>,
) -> Vec<CopperKeepout> {
    let mut result = Vec::new();
    for cluster in &board.rigid_clusters {
        let Some(anchor) = placements.get(&cluster.anchor_reference) else { continue };
        let frame = cluster_frame(&cluster.members, anchor);
        for keepout in &cluster.keepouts {
            result.push(CopperKeepout {
                id: format!("cluster/{}/{}", cluster.name, keepout.id),
                vertices: keepout.vertices.iter().map(|p| transformed_local_point(&frame, *p)).collect(),
                ..keepout.clone()
            });
        }
    }
    result
}

/// `resolved_copper_keepouts`: board keepouts plus footprint-local keepouts at placed locations.
pub fn resolved_copper_keepouts(board: &PhysicalBoard) -> Result<Vec<CopperKeepout>> {
    let mut result: Vec<CopperKeepout> = board.copper_keepouts.clone();
    result.extend(resolved_cluster_keepouts(board, &placement_map(board)));
    let mut seen: IndexSet<String> = result.iter().map(|k| k.id.clone()).collect();
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in placements {
        for local in &board.footprints[&placement.footprint].keepouts {
            let id = format!("{}/{}", placement.reference, local.id);
            if !seen.insert(id.clone()) {
                return Err(Error::Value(format!(
                    "duplicate resolved copper keepout id {}",
                    copper_json::python_str_repr(&id)
                )));
            }
            let back = placement.side == BoardSide::Back;
            let layers = local
                .layers
                .iter()
                .map(|layer| match layer {
                    CopperLayer::Front if back => CopperLayer::Back,
                    CopperLayer::Back if back => CopperLayer::Front,
                    other => *other,
                })
                .collect();
            result.push(CopperKeepout {
                id,
                layers,
                vertices: local.vertices.iter().map(|p| transformed_local_point(placement, *p)).collect(),
                ..local.clone()
            });
        }
    }
    Ok(result)
}

//! Conservative component grouping for native-fill observations
//! (`pcbir.filled_connectivity`).
//!
//! Bare copper contacts are shared with DRC; source-zone outlines and
//! installed-part internal edges do not join components. Fill fragments join
//! contacts and each other by positive overlap area (`rf_geometry`).

use std::collections::BTreeMap;

use indexmap::IndexSet;

use copper_drc::connectivity::{Candidate, copper_contact_roots, explicit_copper_contacts};
use copper_pcb::board::PhysicalBoard;

use crate::Result;
use crate::plane_verify::{NativeCopperItem, PlaneVerification};
use crate::rf_geometry::{Convex, FillPolygon, NativeFill, buffered_hull, native_fill_polygon};

/// `native_item_identity`: the contact identity of a native object, if any.
pub fn native_item_identity(item: &NativeCopperItem) -> Option<String> {
    match item.kind.as_str() {
        "pad" => Some(format!(
            "pad:{}.{}:{}",
            item.component.as_deref().unwrap_or("None"),
            item.pad.as_deref().unwrap_or("None"),
            item.index.map_or_else(|| "None".to_owned(), |i| i.to_string())
        )),
        "segment" => Some(format!("track:{}", item.index.map_or_else(|| "None".to_owned(), |i| i.to_string()))),
        "via" => Some(format!("via:{}", item.index.map_or_else(|| "None".to_owned(), |i| i.to_string()))),
        _ => None,
    }
}

/// `filled_component_roots(board, evidence, nets)`.
pub fn filled_component_roots(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    nets: &IndexSet<String>,
) -> Result<BTreeMap<String, String>> {
    let candidate = Candidate { only_nets: Some(nets), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, _, groups) = explicit_copper_contacts(board, candidate)?;
    let parent = copper_contact_roots(&contacts, &groups)?;
    if !evidence.matches(board)? {
        return Ok(parent);
    }
    let mut parent = parent;
    fn root(parent: &mut BTreeMap<String, String>, identity: &str) -> String {
        let mut current = identity.to_owned();
        loop {
            let up = parent[&current].clone();
            if up == current {
                return current;
            }
            let grand = parent[&up].clone();
            parent.insert(current, grand.clone());
            current = grand;
        }
    }
    fn join(parent: &mut BTreeMap<String, String>, a: &str, b: &str) {
        let (a, b) = (root(parent, a), root(parent, b));
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        parent.insert(high, low);
    }
    let shape = |shape: &copper_physical::geometry::RoundedConvexShape, drill: bool| {
        let spine: Vec<(i64, i64)> = shape.spine.iter().map(|p| (p.x_nm, p.y_nm)).collect();
        let radius = shape.radius_nm as f64;
        // 16 segments per quadrant. Expand ONLY the subtracted drilled void.
        buffered_hull(&spine, if drill { radius / (std::f64::consts::PI / 64.0).cos() } else { radius })
    };
    let geometries: Vec<(Convex, Option<Convex>)> = contacts
        .iter()
        .map(|contact| (shape(&contact.shape, false), contact.drill.as_ref().map(|drill| shape(drill, true))))
        .collect();
    let mut fills: Vec<(&crate::plane_verify::NativeFilledRegion, String, FillPolygon)> = Vec::new();
    for fragment in &evidence.filled_regions {
        if !nets.contains(&fragment.net) {
            continue;
        }
        let NativeFill::Polygon(geometry) = native_fill_polygon(&fragment.vertices_nm)? else {
            // Invalid, or disconnected faces in one native walk: not one component.
            continue;
        };
        let identity = format!("fill:{}", fragment.fragment_digest);
        parent.insert(identity.clone(), identity.clone());
        for (contact, (copper, drill)) in contacts.iter().zip(&geometries) {
            if contact.net != fragment.net || !contact.layers.iter().any(|layer| layer.value() == fragment.layer) {
                continue;
            }
            let mut area = geometry.convex_overlap(copper);
            if area <= 0.0 {
                continue;
            }
            if let Some(drill) = drill
                && !contact.capped_layers.iter().any(|layer| layer.value() == fragment.layer)
            {
                area -= geometry.convex_overlap(&copper.intersection(drill));
            }
            if area > 0.0 {
                join(&mut parent, &identity, &contact.identity);
            }
        }
        fills.push((fragment, identity, geometry));
    }
    for index in 0..fills.len() {
        for other in 0..index {
            let ((fragment, identity, geometry), (prior, prior_id, prior_geometry)) = (&fills[index], &fills[other]);
            if fragment.net == prior.net
                && fragment.layer == prior.layer
                && root(&mut parent, identity) != root(&mut parent, prior_id)
                && geometry.overlap(prior_geometry) > 0.0
            {
                let (a, b) = (identity.clone(), prior_id.clone());
                join(&mut parent, &a, &b);
            }
        }
    }
    let identities: Vec<String> = parent.keys().cloned().collect();
    Ok(identities.into_iter().map(|identity| (identity.clone(), root(&mut parent, &identity))).collect())
}

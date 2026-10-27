//! Normalized, soft supply-domain distribution heuristics
//! (`pcbir.power_planning` `power_domain_penalty` and `power_domain_gradient`).
//! Not a current, impedance, voltage-drop or return-path solver.

use indexmap::IndexMap;

use copper_pcb::Result;
use copper_pcb::board::{PadReference, PhysicalPowerDomain};
use copper_physical::Point;
use copper_physical::pyfloat::{round_int, sum};

use crate::scene::{Poses, Scene};

/// `domain_terminal_points`: placed member pads.
fn terminal_points(
    scene: &Scene,
    placements: &Poses,
    domain: &PhysicalPowerDomain,
) -> Result<Vec<(PadReference, Point)>> {
    let mut points = Vec::new();
    for pad in &domain.members {
        if let Some(pose) = placements.get(&pad.component) {
            points.push((pad.clone(), scene.pad_position(pose, &pad.pad)?));
        }
    }
    Ok(points)
}

/// `_weights`: each consumer has equal rail weight, regardless of supply-pin count.
fn weights<'p>(pads: impl Iterator<Item = &'p PadReference>) -> Vec<f64> {
    let pads: Vec<&PadReference> = pads.collect();
    let mut counts: IndexMap<&str, usize> = IndexMap::new();
    for pad in &pads {
        *counts.entry(pad.component.as_str()).or_insert(0) += 1;
    }
    pads.iter().map(|pad| 1.0 / (counts.len() * counts[pad.component.as_str()]) as f64).collect()
}

/// `_targets`: the rail's placed sources, else its weighted centroid.
fn targets(points: &[(PadReference, Point)], domain: &PhysicalPowerDomain) -> Vec<(Option<PadReference>, Point)> {
    let sources: Vec<(Option<PadReference>, Point)> = domain
        .sources
        .iter()
        .filter_map(|pad| points.iter().find(|(p, _)| p == pad).map(|(p, point)| (Some(p.clone()), *point)))
        .collect();
    if !sources.is_empty() || points.is_empty() {
        return sources;
    }
    let weight = weights(points.iter().map(|(pad, _)| pad));
    let x = sum(points.iter().zip(&weight).map(|((_, p), w)| p.x_nm as f64 * w));
    let y = sum(points.iter().zip(&weight).map(|((_, p), w)| p.y_nm as f64 * w));
    vec![(None, Point::new(round_int(x), round_int(y)))]
}

fn manhattan(a: Point, b: Point) -> i64 {
    (a.x_nm - b.x_nm).abs() + (a.y_nm - b.y_nm).abs()
}

/// `power_domain_penalty`: per-rail mean L1 distribution distance, softly weighted.
pub fn power_domain_penalty(scene: &Scene, placements: &Poses, weight: f64) -> Result<i64> {
    if weight == 0.0 {
        return Ok(0);
    }
    let mut total = 0.0f64;
    for domain in &scene.board.power_domains {
        let points = terminal_points(scene, placements, domain)?;
        let targets = targets(&points, domain);
        let loads: Vec<&(PadReference, Point)> =
            points.iter().filter(|(pad, _)| !domain.sources.contains(pad)).collect();
        let load_weights = weights(loads.iter().map(|(pad, _)| pad));
        if !targets.is_empty() && !loads.is_empty() {
            total += sum(loads
                .iter()
                .zip(&load_weights)
                .map(|((_, p), w)| w * targets.iter().map(|(_, t)| manhattan(*p, *t)).min().expect("targets") as f64));
        }
    }
    Ok(round_int(total * weight))
}

fn sign(a: i64, b: i64) -> f64 {
    ((a > b) as i64 - (a < b) as i64) as f64
}

/// `power_domain_gradient`: the L1 subgradient; source and load forces balance within each rail.
pub fn power_domain_gradient(scene: &Scene, placements: &Poses, weight: f64) -> Result<IndexMap<String, [f64; 2]>> {
    let mut gradients: IndexMap<String, [f64; 2]> = placements.keys().map(|r| (r.clone(), [0.0, 0.0])).collect();
    if weight == 0.0 {
        return Ok(gradients);
    }
    for domain in &scene.board.power_domains {
        let points = terminal_points(scene, placements, domain)?;
        let targets = targets(&points, domain);
        let loads: Vec<&(PadReference, Point)> =
            points.iter().filter(|(pad, _)| !domain.sources.contains(pad)).collect();
        let load_weights = weights(loads.iter().map(|(pad, _)| pad));
        if targets.is_empty() || loads.is_empty() {
            continue;
        }
        let mut forces: Vec<[f64; 2]> = Vec::new();
        for ((pad, point), pad_weight) in loads.iter().zip(&load_weights) {
            let (source, target) = targets
                .iter()
                .min_by(|a, b| {
                    let key = |item: &(Option<PadReference>, Point)| {
                        let id = item
                            .0
                            .as_ref()
                            .map_or((String::new(), String::new()), |p| (p.component.clone(), p.pad.clone()));
                        (manhattan(*point, item.1), id)
                    };
                    key(a).cmp(&key(b))
                })
                .expect("targets");
            let force = [
                weight * pad_weight * sign(point.x_nm, target.x_nm),
                weight * pad_weight * sign(point.y_nm, target.y_nm),
            ];
            forces.push(force);
            for axis in 0..2 {
                gradients[&pad.component][axis] += force[axis];
                if let Some(source) = source {
                    gradients[&source.component][axis] -= force[axis];
                }
            }
        }
        if targets[0].0.is_none() {
            for ((pad, _), pad_weight) in loads.iter().zip(&load_weights) {
                for axis in 0..2 {
                    gradients[&pad.component][axis] -= sum(forces.iter().map(|f| f[axis])) * pad_weight;
                }
            }
        }
    }
    Ok(gradients)
}

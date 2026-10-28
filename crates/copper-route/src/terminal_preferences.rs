//! Soft, footprint-local outward preferences for newly generated lead-ins
//! (`pcbir.terminal_preferences`). Geometric rankings only: central and
//! through-hole lands have no inferred exit direction, and coincident lands
//! with conflicting directions are left unranked.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_pcb::transform::transformed_local_point;
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};

pub struct TerminalPreferences {
    /// `(net, layer, launch point)` to the outward normals of that SMD land.
    pub normals: IndexMap<(String, CopperLayer, Point), Vec<(i64, i64)>>,
}

impl TerminalPreferences {
    pub fn new(board: &PhysicalBoard) -> Self {
        let mut normals: IndexMap<(String, CopperLayer, Point), Vec<(i64, i64)>> = IndexMap::new();
        let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
        for net in &board.nets {
            for pad in &net.pads {
                assigned.insert(pad, &net.name);
            }
        }
        let mut ambiguous: IndexSet<(String, CopperLayer, Point)> = IndexSet::new();
        let unit = crate::nm_from_mm(1);
        for pose in &board.placements {
            let fp = &board.footprints[&pose.footprint];
            let layer = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
            for pad in &fp.pads {
                let Some(net) = assigned.get(&PadReference::new(&pose.reference, &pad.number)) else { continue };
                if pad.kind != PadKind::Smd {
                    continue;
                }
                let (x, y) = (pad.position.x_nm, pad.position.y_nm);
                if x == 0 && y == 0 {
                    continue;
                }
                // Compare the distance from the centre in footprint-local axes,
                // normalized by body size, then transform the chosen normals.
                let ax = i128::from(x.abs()) * i128::from(fp.body_size.height_nm);
                let ay = i128::from(y.abs()) * i128::from(fp.body_size.width_nm);
                let mut offsets = Vec::new();
                if ax >= ay && x != 0 {
                    offsets.push(Point::new(x + if x > 0 { unit } else { -unit }, y));
                }
                if ay >= ax && y != 0 {
                    offsets.push(Point::new(x, y + if y > 0 { unit } else { -unit }));
                }
                let start = transformed_local_point(pose, pad.position);
                let found: Vec<(i64, i64)> = offsets
                    .iter()
                    .map(|p| transformed_local_point(pose, *p))
                    .map(|end| (end.x_nm - start.x_nm, end.y_nm - start.y_nm))
                    .collect();
                let key = ((*net).to_owned(), layer, start);
                if let Some(known) = normals.get(&key)
                    && *known != found
                {
                    ambiguous.insert(key.clone());
                }
                normals.insert(key, found);
            }
        }
        for key in &ambiguous {
            normals.shift_remove(key);
        }
        TerminalPreferences { normals }
    }

    /// Rank both the outward departure and the reversed arrival; lower is better.
    pub fn score(&self, net: &str, layer: CopperLayer, points: &[Point]) -> i64 {
        if points.len() < 2 {
            return 0;
        }
        let departure = |start: Point, end: Point| -> i64 {
            let Some(normals) = self.normals.get(&(net.to_owned(), layer, start)) else { return 0 };
            if normals.is_empty() {
                return 0;
            }
            let (dx, dy) = (i128::from(end.x_nm - start.x_nm), i128::from(end.y_nm - start.y_nm));
            normals
                .iter()
                .map(|(nx, ny)| {
                    let (nx, ny) = (i128::from(*nx), i128::from(*ny));
                    let dot = dx * nx + dy * ny;
                    if dot < 0 {
                        3
                    } else if dot == 0 {
                        2
                    } else if dx * ny == dy * nx {
                        0
                    } else {
                        1
                    }
                })
                .min()
                .expect("normals")
        };
        departure(points[0], points[1]) + departure(points[points.len() - 1], points[points.len() - 2])
    }

    /// Paths by score; a stable sort keeps the original order on ties.
    pub fn ordered(&self, net: &str, layer: CopperLayer, paths: Vec<Vec<Point>>) -> Vec<Vec<Point>> {
        let mut paths = paths;
        paths.sort_by_key(|points| self.score(net, layer, points));
        paths
    }
}

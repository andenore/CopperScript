//! The real zone fills of a board: the physical board is exported in memory (`copper_pcb::kicad_pcb::generate`),
//! parsed with `copper_cam::board::parse_board`, and its zones are filled natively with `copper_cam::fill`
//! under the rules of the exported `.kicad_pro` and `.kicad_dru`.
//!
//! `copper_cam::fill` works on its own drawing model of a `.kicad_pcb`, not on a `PhysicalBoard`, so this export is
//! the bridge; nothing is written to disk. The fills carry the zone clearances, thermal reliefs, openings for the
//! vias and pads of other nets, the minimum width and the removal of islands the zone asks for. Both the quasi-static
//! extraction (`copper-extract`) and the full-wave region model (`RegionSpec::zone_fills`) take their pours from
//! here, so the two tiers see the same copper.

use std::sync::Arc;

use copper_cam::board::{Board, parse_board};
use copper_cam::fill::{ZoneFill, fill_zones};
use copper_cam::poly::Shapes;
use copper_cam::rules::Rules;
use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;

/// The exported, parsed and filled board.
pub struct FilledBoard {
    pub cam: Board,
    pub fills: Vec<ZoneFill>,
    pub fill_seconds: f64,
    /// Zone fills made, islands removed by the fill.
    pub zone_fills: usize,
    pub islands_removed: usize,
}

/// Export `board` in memory, parse it and fill its zones natively.
pub fn export_and_fill(board: &PhysicalBoard) -> Result<FilledBoard, String> {
    let started = std::time::Instant::now();
    let manifest = copper_pcb::kicad_pcb::generate(board).map_err(|e| format!("KiCad export for the fill: {e:?}"))?;
    let find = |suffix: &str| manifest.artifacts.iter().find(|a| a.name.ends_with(suffix));
    let pcb = find(".kicad_pcb").ok_or("the KiCad export holds no .kicad_pcb")?;
    let project = find(".kicad_pro").ok_or("the KiCad export holds no .kicad_pro")?;
    let rules = Rules::load(&project.content, find(".kicad_dru").map(|a| a.content.as_str()))
        .map_err(|e| format!("design rules of the export: {e}"))?;
    let cam = parse_board(&pcb.content, &pcb.name)?;
    let fills = fill_zones(&cam, &rules)?;
    Ok(FilledBoard {
        zone_fills: fills.len(),
        islands_removed: fills.iter().map(|f| f.islands_removed).sum(),
        cam,
        fills,
        fill_seconds: started.elapsed().as_secs_f64(),
    })
}

/// One filled pour: the copper of one zone on one layer, integer-nanometre polygons.
#[derive(Clone, Debug)]
pub struct RealFill {
    pub net: String,
    pub layer: CopperLayer,
    pub shapes: Shapes,
}

/// The fills of a board, by net and layer, for the full-wave region model.
#[derive(Clone, Debug, Default)]
pub struct ZoneFillSet {
    pub fills: Vec<RealFill>,
    /// Islands the fill removed, and the seconds it took.
    pub islands_removed: usize,
    pub seconds: f64,
}

impl ZoneFillSet {
    /// Whether some fill covers `net` on `layer`.
    pub fn covers(&self, net: &str, layer: CopperLayer) -> bool {
        self.fills.iter().any(|f| f.layer == layer && f.net == net)
    }
}

impl FilledBoard {
    /// The fills as net and layer polygons (zones without a net, rule areas and layers outside the stack-up are
    /// left out; a fill with no copper stays in the set, so that its zone is not drawn as an outline instead).
    pub fn fill_set(&self) -> ZoneFillSet {
        let mut fills = Vec::new();
        for fill in &self.fills {
            let zone = &self.cam.zones[fill.zone];
            let (Some(net), Some(name)) = (zone.net.as_ref(), self.cam.copper_layers.get(fill.layer)) else {
                continue;
            };
            let Some(layer) = CopperLayer::parse(name) else { continue };
            if zone.keepout.is_some() {
                continue;
            }
            fills.push(RealFill { net: net.clone(), layer, shapes: fill.shapes.clone() });
        }
        ZoneFillSet { fills, islands_removed: self.islands_removed, seconds: self.fill_seconds }
    }
}

/// The native fills of `board` as a shareable set for `RegionSpec::zone_fills`.
pub fn zone_fill_set(board: &PhysicalBoard) -> Result<Arc<ZoneFillSet>, String> {
    Ok(Arc::new(export_and_fill(board)?.fill_set()))
}

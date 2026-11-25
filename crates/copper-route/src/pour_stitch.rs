//! Pour stitching: a grid of through vias that joins the layers of a zone that asks for it
//! (`constraint copper_zone(NET) { stitch_pitch = 5mm; }`).
//!
//! Why. Two pours of one net joined only where pads and routed vias happen to join them are, at radio frequencies,
//! a parallel-plate cavity: every port inside it couples to every other through the cavity's field. On
//! `examples/coupling` (two GND pours, three vias) the full-wave coupling from a 48 MHz clock to a 2.4 GHz radio's pad
//! fell by 7 to 27 dB once the pours were stitched every 4.9 mm (`docs/coupling-query-plan.md`, section 3.1).
//!
//! What. After routing, the board is exported and its zones filled natively (`copper_cam::fill`, the same fill the
//! native verification uses). For each zone with a `stitch_pitch`, a grid of that pitch over the zone's outline
//! proposes sites; a site takes a through via of the board's default size when:
//!
//! - the zone's net is filled there on **every** copper layer the via passes, with the via's pad inside the fill
//!   (the fill already keeps the board, zone and net clearances from other nets' copper, keepouts and holes), so
//!   the via meets nothing but its own net;
//! - its drill is at least the board's hole clearance from every other drill (vias, plated holes, mechanical
//!   holes, and the vias added before it);
//! - its pad is at least the board clearance from every pad of its own net (no via in a pad).
//!
//! A zone that is not on every copper layer gets no via (a through via would pierce a layer it does not fill):
//! [`StitchOutcome::skipped`] says so. The vias are ordinary board copper afterwards: the exact DRC, the native
//! refill and its verification see them like any other.

use copper_cam::poly::{self, Shapes};
use copper_pcb::board::{PhysicalBoard, Via};
use copper_physical::{CopperLayer, Point};

/// What stitching did for one zone.
#[derive(Clone, Debug, PartialEq)]
pub struct StitchOutcome {
    pub zone: String,
    pub net: String,
    pub pitch_nm: i64,
    /// Grid sites over the zone's outline.
    pub sites: usize,
    pub added: usize,
    /// Why the zone got no via at all, when it got none for a reason other than the sites.
    pub skipped: Option<String>,
}

/// The native fills of `board` by `(net, layer)`.
fn fills(board: &PhysicalBoard) -> Result<Vec<(String, CopperLayer, Shapes)>, String> {
    let manifest = copper_pcb::kicad_pcb::generate(board).map_err(|e| format!("KiCad export for the fill: {e:?}"))?;
    let find = |suffix: &str| manifest.artifacts.iter().find(|a| a.name.ends_with(suffix));
    let pcb = find(".kicad_pcb").ok_or("the KiCad export holds no .kicad_pcb")?;
    let project = find(".kicad_pro").ok_or("the KiCad export holds no .kicad_pro")?;
    let rules = copper_cam::rules::Rules::load(&project.content, find(".kicad_dru").map(|a| a.content.as_str()))
        .map_err(|e| format!("design rules of the export: {e}"))?;
    let cam = copper_cam::board::parse_board(&pcb.content, &pcb.name)?;
    let filled = copper_cam::fill::fill_zones(&cam, &rules)?;
    let mut out = Vec::new();
    for fill in filled {
        let zone = &cam.zones[fill.zone];
        let (Some(net), Some(name)) = (zone.net.as_ref(), cam.copper_layers.get(fill.layer)) else { continue };
        if zone.keepout.is_some() {
            continue;
        }
        if let Some(layer) = CopperLayer::parse(name) {
            out.push((net.clone(), layer, fill.shapes));
        }
    }
    Ok(out)
}

/// Every drill of `board`: `(centre, diameter)`, nm.
fn drills(board: &PhysicalBoard) -> Vec<(Point, i64)> {
    let mut out: Vec<(Point, i64)> = board.vias.iter().map(|v| (v.position, v.drill_nm)).collect();
    for placement in &board.placements {
        let Some(footprint) = board.footprints.get(&placement.footprint) else { continue };
        for pad in &footprint.pads {
            if let Some(drill) = pad.drill {
                let at = copper_pcb::transform::transformed_local_point(placement, pad.position);
                out.push((at, drill.width_nm.max(drill.height_nm)));
            }
        }
    }
    out.extend(board.mechanical_holes.iter().map(|h| (h.position, h.diameter_nm)));
    out
}

/// The bounds `[x0, y0, x1, y1]` (nm) of the pads of `net`.
fn net_pads(board: &PhysicalBoard, net: &str) -> Vec<[i64; 4]> {
    let mut out = Vec::new();
    for n in board.nets.iter().filter(|n| n.name == net) {
        for reference in &n.pads {
            let Some(placement) = board.placements.iter().find(|p| p.reference == reference.component) else {
                continue;
            };
            let Some(footprint) = board.footprints.get(&placement.footprint) else { continue };
            for pad in footprint.pads.iter().filter(|p| p.number == reference.pad) {
                let at = copper_pcb::transform::transformed_local_point(placement, pad.position);
                let b = copper_pcb::transform::placed_pad_shape(at, pad, placement).bounds;
                out.push([b.min_x, b.min_y, b.max_x, b.max_y]);
            }
        }
    }
    out
}

fn distance_to_box(p: Point, b: [i64; 4]) -> f64 {
    let dx = (b[0] - p.x_nm).max(p.x_nm - b[2]).max(0) as f64;
    let dy = (b[1] - p.y_nm).max(p.y_nm - b[3]).max(0) as f64;
    dx.hypot(dy)
}

/// Stitch every zone of `board` that declares a `stitch_pitch`. Returns the board with the vias added (unchanged
/// when no zone asks) and an outcome per such zone.
pub fn stitch_pours(board: &PhysicalBoard) -> Result<(PhysicalBoard, Vec<StitchOutcome>), String> {
    let wanted: Vec<_> = board.zones.iter().filter(|z| z.stitch_pitch_nm.is_some()).collect();
    if wanted.is_empty() {
        return Ok((board.clone(), Vec::new()));
    }
    let layers: Vec<CopperLayer> = board.stackup.copper_layers.clone();
    let (Some(&top), Some(&bottom)) = (layers.first(), layers.last()) else {
        return Err("pour stitching needs a stack-up with copper layers".to_owned());
    };
    let fills = fills(board)?;
    let rules = &board.rules;
    let (size, drill) = (rules.default_via_size_nm, rules.default_via_drill_nm);
    let mut existing = drills(board);
    let mut out = board.clone();
    let mut outcomes = Vec::new();
    for zone in wanted {
        let pitch = zone.stitch_pitch_nm.expect("a pitch").max(size + rules.minimum_clearance_nm);
        let mut outcome = StitchOutcome {
            zone: zone.id.clone(),
            net: zone.net.clone(),
            pitch_nm: pitch,
            sites: 0,
            added: 0,
            skipped: None,
        };
        if let Some(missing) = layers.iter().find(|l| !zone.layers.contains(l)) {
            outcome.skipped = Some(format!(
                "the zone is not on {} (a through via would pierce a layer it does not fill)",
                missing.value()
            ));
            outcomes.push(outcome);
            continue;
        }
        // Each layer's fill of the net, shrunk by the via's pad radius: a centre inside every one of them puts the
        // whole pad inside the fill on every layer.
        let shrink = size / 2 + 10_000;
        let shrunk: Vec<Shapes> = layers
            .iter()
            .map(|layer| {
                let shapes: Shapes = fills
                    .iter()
                    .filter(|(net, l, _)| *net == zone.net && l == layer)
                    .flat_map(|(_, _, s)| s.iter().cloned())
                    .collect();
                poly::offset(&shapes, -shrink, false, 1_000.0)
            })
            .collect();
        let pads = net_pads(board, &zone.net);
        let (x0, y0) = (zone.outer.iter().map(|p| p.x_nm).min(), zone.outer.iter().map(|p| p.y_nm).min());
        let (x1, y1) = (zone.outer.iter().map(|p| p.x_nm).max(), zone.outer.iter().map(|p| p.y_nm).max());
        let (Some(x0), Some(y0), Some(x1), Some(y1)) = (x0, y0, x1, y1) else {
            outcomes.push(outcome);
            continue;
        };
        let mut y = y0 + pitch / 2;
        while y < y1 {
            let mut x = x0 + pitch / 2;
            while x < x1 {
                outcome.sites += 1;
                let p = Point::new(x, y);
                let inside = shrunk.iter().all(|s| poly::shapes_contain(s, (x as f64, y as f64)));
                let hole_clear = existing.iter().all(|(q, d)| {
                    let gap = ((q.x_nm - x) as f64).hypot((q.y_nm - y) as f64) - (*d + drill) as f64 / 2.0;
                    gap >= rules.minimum_hole_clearance_nm as f64
                });
                let pad_clear =
                    pads.iter().all(|b| distance_to_box(p, *b) >= (size / 2 + rules.minimum_clearance_nm) as f64);
                if inside && hole_clear && pad_clear {
                    out.vias.push(Via {
                        net: zone.net.clone(),
                        position: p,
                        size_nm: size,
                        drill_nm: drill,
                        from_layer: top,
                        to_layer: bottom,
                        technology: None,
                        finish: "standard".to_owned(),
                    });
                    existing.push((p, drill));
                    outcome.added += 1;
                }
                x += pitch;
            }
            y += pitch;
        }
        outcomes.push(outcome);
    }
    Ok((out, outcomes))
}

#[cfg(test)]
mod tests {
    use crate::state_codec::{encode_board, physical_board};
    use crate::test_support_critical::{board, mm, net, zone};
    use copper_physical::CopperLayer;

    #[test]
    fn a_stitch_pitch_survives_the_state_round_trip_and_its_absence_writes_nothing() {
        let mut board = board("stitch", 20, 20, Vec::new(), Vec::new(), vec![net("GND", &[])]);
        let corners = vec![mm(0, 0), mm(20, 0), mm(20, 20), mm(0, 20)];
        board.zones.push(zone("gnd", "GND", &[CopperLayer::Front, CopperLayer::Back], corners));
        let plain = format!("{:?}", encode_board(&board));
        assert!(!plain.contains("stitch_pitch_nm"), "a zone without a pitch writes no key");
        board.zones[0].stitch_pitch_nm = Some(5_000_000);
        let decoded = physical_board(&encode_board(&board)).expect("the board decodes");
        assert_eq!(decoded.zones[0].stitch_pitch_nm, Some(5_000_000));
    }
}

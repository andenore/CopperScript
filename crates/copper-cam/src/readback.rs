//! CAM read-back: the connectivity of the plotted artwork itself, checked
//! against the netlist, independently of the board model that drew it.
//!
//! Every copper Gerber is read back and rasterised (10 um by default), and
//! its 8-connected copper regions are labelled. Plated drill hits join the
//! regions they pass through on every layer. Each test point of the board
//! netlist (every pad and via with a net, as in its IPC-D-356 file) is
//! located in the region under it on each of its copper layers; layers join
//! only through plated holes. Pads a part joins internally (jumper pad
//! groups) count as joined. A region holding test points of two nets is a
//! short; a net whose test points lie in several regions is open. A test
//! point with no copper under it is reported too.

use indexmap::IndexMap;

use crate::board::{Board, PadKind};
use crate::compare::drill;
use crate::compare::gerber;
use crate::compare::raster::{Frame, Raster};
use crate::fill::{Sets, flashes};

pub type Result<T> = std::result::Result<T, String>;

/// One terminal of the board netlist.
#[derive(Clone, Debug, PartialEq)]
pub struct TestPoint {
    pub name: String,
    pub net: String,
    /// Gerber nanometres (Y up).
    pub at: (f64, f64),
    /// Copper layers (0 = top) it has copper on.
    pub layers: Vec<usize>,
    /// Footprint and jumper group of a pad the part joins internally.
    pub group: Option<(usize, usize)>,
}

/// The test points of `board`: its vias and the pads with a net and copper.
pub fn test_points(board: &Board) -> Vec<TestPoint> {
    let count = board.copper_count();
    let mut points = Vec::new();
    for via in &board.vias {
        let Some(net) = &via.net else { continue };
        let at = (via.center.x as f64, -(via.center.y as f64));
        points.push(TestPoint {
            name: format!("via at ({:.4}, {:.4}) mm", at.0 / 1e6, -at.1 / 1e6),
            net: net.clone(),
            at,
            layers: (0..count).collect(),
            group: None,
        });
    }
    for (index, footprint) in board.footprints.iter().enumerate() {
        for pad in &footprint.pads {
            let Some(net) = &pad.net else { continue };
            let layers: Vec<usize> = (0..count).filter(|&layer| flashes(pad, layer)).collect();
            if layers.is_empty() || pad.kind == PadKind::NonPlated {
                continue;
            }
            points.push(TestPoint {
                name: format!("{}.{}", footprint.reference, pad.number),
                net: net.clone(),
                at: (pad.center.x as f64, -(pad.center.y as f64)),
                layers,
                group: footprint
                    .jumper_groups
                    .iter()
                    .position(|group| group.contains(&pad.number))
                    .map(|group| (index, group)),
            });
        }
    }
    points
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readback {
    pub shorts: Vec<String>,
    pub opens: Vec<String>,
    /// Test points with no plotted copper under them.
    pub uncovered: Vec<String>,
    pub copper_layers: usize,
    pub test_points: usize,
    pub plated_holes: usize,
    pub regions: usize,
}

impl Readback {
    pub fn passed(&self) -> bool {
        self.shorts.is_empty() && self.opens.is_empty() && self.uncovered.is_empty()
    }

    pub fn findings(&self) -> Vec<String> {
        let tag = |kind: &str, list: &[String]| list.iter().map(|text| format!("{kind}: {text}")).collect::<Vec<_>>();
        [tag("short", &self.shorts), tag("open", &self.opens), tag("no copper", &self.uncovered)].concat()
    }

    pub fn summary(&self) -> String {
        let mut lines: Vec<String> = self.findings().into_iter().map(|line| format!("  {line}")).collect();
        lines.push(format!(
            "CAM read-back {}: {} short(s), {} open(s), {} test point(s) without copper; {} copper layer(s), {} region(s), {} test point(s), {} plated hole(s)",
            if self.passed() { "PASSED" } else { "FAILED" },
            self.shorts.len(),
            self.opens.len(),
            self.uncovered.len(),
            self.copper_layers,
            self.regions,
            self.test_points,
            self.plated_holes
        ));
        lines.join("\n")
    }
}

/// The 8-connected regions of one raster: runs per row with their region.
struct Labels {
    frame: Frame,
    /// Per row: (first column, end column, run index).
    rows: Vec<Vec<(usize, usize, usize)>>,
    /// Run index to region index (dense from 0).
    region: Vec<usize>,
    count: usize,
}

impl Labels {
    fn new(raster: &Raster) -> Labels {
        let frame = raster.frame;
        let mut rows: Vec<Vec<(usize, usize, usize)>> = Vec::with_capacity(frame.height);
        let mut runs = 0usize;
        for y in 0..frame.height {
            let mut row = Vec::new();
            let mut x = 0;
            while x < frame.width {
                if raster.get(x, y) {
                    let from = x;
                    while x < frame.width && raster.get(x, y) {
                        x += 1;
                    }
                    row.push((from, x, runs));
                    runs += 1;
                } else {
                    x += 1;
                }
            }
            rows.push(row);
        }
        let mut sets = Sets((0..runs).collect());
        for y in 1..rows.len() {
            let (below, above) = (&rows[y - 1], &rows[y]);
            let (mut i, mut j) = (0, 0);
            while i < below.len() && j < above.len() {
                let (a, b) = (below[i], above[j]);
                // Overlapping or diagonally adjacent runs touch.
                if a.0 <= b.1 && b.0 <= a.1 {
                    sets.join(a.2, b.2);
                }
                if a.1 < b.1 { i += 1 } else { j += 1 }
            }
        }
        let mut dense: IndexMap<usize, usize> = IndexMap::new();
        let mut region = Vec::with_capacity(runs);
        for run in 0..runs {
            let root = sets.find(run);
            let next = dense.len();
            region.push(*dense.entry(root).or_insert(next));
        }
        Labels { frame, rows, region, count: dense.len() }
    }

    /// The region under the point (Gerber nanometres), if it is copper.
    fn at(&self, p: (f64, f64)) -> Option<usize> {
        let f = &self.frame;
        let (column, row) = (((p.0 - f.x0) / f.pitch).floor(), ((p.1 - f.y0) / f.pitch).floor());
        if column < 0.0 || row < 0.0 || column as usize >= f.width || row as usize >= f.height {
            return None;
        }
        let (column, row) = (column as usize, row as usize);
        let runs = &self.rows[row];
        let index = runs.partition_point(|run| run.1 <= column);
        runs.get(index).filter(|run| run.0 <= column).map(|run| self.region[run.2])
    }
}

/// Copper layer number (1 = top) of a `TF.FileFunction` like `Copper,L2,Inr`.
fn copper_layer(function: &str) -> Option<usize> {
    let rest = function.strip_prefix("Copper,L")?;
    rest.split(',').next()?.parse().ok()
}

/// Read back `gerbers` and `drills` (name, text) against the test `points`
/// of a board with `copper_count` layers, at `pitch` nanometres.
pub fn check(
    gerbers: &[(String, String)],
    drills: &[(String, String)],
    points: &[TestPoint],
    copper_count: usize,
    pitch: f64,
) -> Result<Readback> {
    let mut images: Vec<Option<gerber::Image>> = vec![None; copper_count];
    for (name, text) in gerbers {
        let image = gerber::read(text).map_err(|failure| format!("{name}: {failure}"))?;
        let Some(layer) = image.file_function.as_deref().and_then(copper_layer) else { continue };
        if layer == 0 || layer > copper_count {
            return Err(format!("{name}: copper layer L{layer} outside the {copper_count}-layer stack"));
        }
        if images[layer - 1].is_some() {
            return Err(format!("{name}: a second Gerber for copper layer L{layer}"));
        }
        images[layer - 1] = Some(image);
    }
    let images: Vec<gerber::Image> = images
        .into_iter()
        .enumerate()
        .map(|(index, image)| image.ok_or_else(|| format!("no Gerber for copper layer L{}", index + 1)))
        .collect::<Result<_>>()?;
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for b in images.iter().filter_map(gerber::Image::bounds) {
        let total = bounds.get_or_insert(b);
        *total = (total.0.min(b.0), total.1.min(b.1), total.2.max(b.2), total.3.max(b.3));
    }
    let Some(bounds) = bounds else { return Err("the copper Gerbers are empty".to_owned()) };
    let frame = Frame::covering(bounds, pitch);
    let labels: Vec<Labels> =
        images.iter().map(|image| Labels::new(&crate::compare::rasterise(image, frame))).collect();
    let offsets: Vec<usize> = labels
        .iter()
        .scan(0, |total, layer| {
            let offset = *total;
            *total += layer.count;
            Some(offset)
        })
        .collect();
    let regions: usize = labels.iter().map(|layer| layer.count).sum();
    let mut sets = Sets((0..regions).collect());
    let node = |layer: usize, p: (f64, f64)| labels[layer].at(p).map(|region| offsets[layer] + region);

    let mut plated_holes = 0;
    for (name, text) in drills {
        for hit in drill::read(text, name).map_err(|failure| format!("{name}: {failure}"))? {
            if !hit.plated {
                continue;
            }
            plated_holes += 1;
            let mut ends = vec![hit.start];
            ends.extend(hit.end);
            let found: Vec<usize> =
                (0..copper_count).flat_map(|layer| ends.iter().filter_map(move |&p| node(layer, p))).collect();
            for pair in found.windows(2) {
                sets.join(pair[0], pair[1]);
            }
        }
    }

    let mut uncovered = Vec::new();
    let mut located: Vec<(&TestPoint, usize)> = Vec::new();
    let mut groups: IndexMap<((usize, usize), &str), usize> = IndexMap::new();
    for point in points {
        if let Some(&layer) = point.layers.iter().find(|&&layer| layer >= copper_count) {
            return Err(format!("test point {} is on copper layer {layer} of {copper_count}", point.name));
        }
        // Each layer's copper under a through test point counts on its
        // own: only plated holes join layers.
        let found: Vec<usize> = point.layers.iter().filter_map(|&layer| node(layer, point.at)).collect();
        if found.is_empty() {
            uncovered.push(format!(
                "{} [{}] at ({:.4}, {:.4}) mm",
                point.name,
                point.net,
                point.at.0 / 1e6,
                -point.at.1 / 1e6
            ));
            continue;
        }
        if let Some(group) = point.group {
            match groups.get(&(group, point.net.as_str())) {
                Some(&first) => sets.join(first, found[0]),
                None => {
                    groups.insert((group, point.net.as_str()), found[0]);
                }
            }
        }
        located.extend(found.iter().map(|&region| (point, region)));
    }
    let test_points = points.len();
    let mut nets_of: IndexMap<usize, IndexMap<&str, Vec<String>>> = IndexMap::new();
    let mut groups_of: IndexMap<&str, IndexMap<usize, Vec<String>>> = IndexMap::new();
    for &(point, region) in &located {
        let root = sets.find(region);
        let names = nets_of.entry(root).or_default().entry(&point.net).or_default();
        if !names.contains(&point.name) {
            names.push(point.name.clone());
        }
        let names = groups_of.entry(&point.net).or_default().entry(root).or_default();
        if !names.contains(&point.name) {
            names.push(point.name.clone());
        }
    }
    let sample = |points: &Vec<String>| {
        let shown: Vec<&str> = points.iter().take(3).map(String::as_str).collect();
        if points.len() > 3 { format!("{} ...", shown.join(", ")) } else { shown.join(", ") }
    };
    let shorts = nets_of
        .values()
        .filter(|nets| nets.len() > 1)
        .map(|nets| {
            let parts: Vec<String> = nets.iter().map(|(net, points)| format!("[{net}] ({})", sample(points))).collect();
            format!("plotted copper joins {}", parts.join(" and "))
        })
        .collect();
    let opens = groups_of
        .iter()
        .filter(|(_, groups)| groups.len() > 1)
        .map(|(net, groups)| {
            let parts: Vec<String> = groups.values().map(|points| format!("({})", sample(points))).collect();
            format!("net [{net}] is split into {} plotted regions: {}", groups.len(), parts.join(" | "))
        })
        .collect();
    Ok(Readback { shorts, opens, uncovered, copper_layers: copper_count, test_points, plated_holes, regions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::parse_board;

    type Files = Vec<(String, String)>;

    /// Two pads per net on F.Cu, joined by a track per net, and a via.
    fn plotted(extra: &str) -> (Files, Files, Vec<TestPoint>) {
        let text = format!(
            "(kicad_pcb (version 20260206)
  (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (25 \"Edge.Cuts\" user))
  (footprint \"L:R\" (layer \"F.Cu\") (at 10 10)
    (property \"Reference\" \"R1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd rect (at -5 0) (size 1 1) (layers \"F.Cu\") (net \"A\"))
    (pad \"2\" smd rect (at 5 0) (size 1 1) (layers \"F.Cu\") (net \"A\"))
    (pad \"3\" smd rect (at -5 3) (size 1 1) (layers \"F.Cu\") (net \"B\"))
    (pad \"4\" thru_hole circle (at 5 3) (size 1 1) (drill 0.5) (layers \"*.Cu\") (net \"B\")))
  (gr_rect (start 0 0) (end 20 20) (stroke (width 0.1) (type default)) (fill no) (layer \"Edge.Cuts\"))
  (segment (start 5 10) (end 15 10) (width 0.3) (layer \"F.Cu\") (net \"A\"))
  (segment (start 5 13) (end 10 13) (width 0.3) (layer \"F.Cu\") (net \"B\"))
  (via (at 10 13) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net \"B\"))
  (segment (start 10 13) (end 15 13) (width 0.3) (layer \"B.Cu\") (net \"B\"))
  {extra})"
        );
        let board = parse_board(&text, "test").unwrap();
        let files = crate::plot::plot(&board, "test", "0").unwrap();
        let pick = |suffix: &str| -> Vec<(String, String)> {
            files.iter().filter(|(name, _)| name.ends_with(suffix)).cloned().collect()
        };
        (pick(".gbr"), pick(".drl"), test_points(&board))
    }

    fn read(extra: &str) -> Readback {
        let (gerbers, drills, points) = plotted(extra);
        check(&gerbers, &drills, &points, 2, 10_000.0).unwrap()
    }

    #[test]
    fn clean_artwork_passes_and_vias_join_layers() {
        let clean = read("");
        assert!(clean.passed(), "{}", clean.summary());
        assert_eq!((clean.test_points, clean.plated_holes), (5, 2));
        // Without the via's hole the B.Cu half of net B is cut off.
        let (gerbers, _, points) = plotted("");
        let cut = check(&gerbers, &[], &points, 2, 10_000.0).unwrap();
        assert_eq!(cut.opens.len(), 1, "{}", cut.summary());
        assert!(cut.opens[0].contains("[B]"), "{}", cut.summary());
    }

    #[test]
    fn shorts_and_opens_are_found() {
        let short = read("(segment (start 15 10) (end 15 13) (width 0.3) (layer \"F.Cu\"))");
        assert_eq!(short.shorts.len(), 1, "{}", short.summary());
        assert!(short.shorts[0].contains("[A]") && short.shorts[0].contains("[B]"));
        let (gerbers, drills, points) = plotted("");
        let open: Vec<(String, String)> = gerbers
            .into_iter()
            .map(|(name, text)| {
                // Drop the A track from F.Cu.
                let text = if name.contains("F_Cu") { text.replace("X15000000Y-10000000D01*\n", "") } else { text };
                (name, text)
            })
            .collect();
        let report = check(&open, &drills, &points, 2, 10_000.0).unwrap();
        assert!(report.opens.iter().any(|text| text.contains("[A]")), "{}", report.summary());
    }
}

//! The magnetic near-field map of a hot loop (`copper emi-check --plot-field`,
//! `copper view RUN --issue M<n> --emi-field`): the quasi-static `|H|` of the
//! loop's current path in a plane above the board, as a heat map over the
//! layout (dB relative to the map maximum, with 10 dB contours), the loop drawn
//! from its traced path (solid on the component layer, dashed on another layer, a
//! circle at each via), and the field level below the peak at the pads of
//! sensitive nets and cable connectors.
//!
//! The source is the `loop_path_3d_mm` measure of an `EMI-HOT-LOOP` finding
//! (the loop as a closed 3D polyline, millimetres, z = 0 at the top copper
//! layer) with the `loop_reference_plane_z_mm` of the reference plane, and the
//! `field_victims` pad names. The amplitude is the finding's own
//! `loop_current.field_amplitude_a`: `I_out + dI/2` (the peak of the high-side switch current) in the
//! `declared_operating_point` scenario, the cell's `current_peak` (the switch current limit, a worst case) in the
//! `current_limit` scenario, or 1 A ("per ampere") when unknown.
//! The field itself is `copper_emi::field`.
//!
//! **Screening estimate, not a field prediction.** Quasi-static (the region is
//! much smaller than a wavelength), one static current as the amplitude,
//! only the hot loop carries current (the power inductor's own field, usually
//! the largest low-frequency source of an unshielded inductor, is NOT
//! included: coverage `unsupported EMI-FIELD:inductor`), an ideal infinite
//! ground plane by images, no shielding by other copper. A ranking and
//! placement aid.

use std::collections::BTreeMap;

use copper_emi::field::{self, Grid};
use copper_emi::inductance::Segment;
use copper_json::Value;

use crate::emi_plot::{escape, n};
use crate::scene::{Point, Scene};

/// Edge of a map cell by default, mm.
pub const DEFAULT_STEP_MM: f64 = 0.25;
/// Largest grid side, cells.
pub const MAX_CELLS: usize = 200;
/// Default height of the map plane above the top copper, mm.
pub const DEFAULT_HEIGHT_MM: f64 = 1.0;
/// Range of the colour scale below the map maximum, dB.
pub const RANGE_DB: f64 = 40.0;
/// Steps of the colour scale over [`RANGE_DB`] (one dB each).
pub const STEPS: usize = 40;
/// Distance between contour lines, dB.
pub const CONTOUR_DB: f64 = 10.0;
/// The window always holds the loop's box plus this margin on every side, mm.
const MARGIN_MM: f64 = 3.0;
/// The field is first computed this far around the loop's box, mm; the window is cropped to what it needs.
const WIDE_MM: f64 = 20.0;
/// The window also holds every cell this close to the maximum (dB below it), so the field is not cut at the edge.
const EXTENT_DB: f64 = 40.0;
/// Victim pads within this distance of the loop's box extend the window, mm.
const REACH_MM: f64 = 12.0;

/// Nine anchors of the viridis scale (perceptually uniform), dark (weak) to yellow (strong).
const VIRIDIS: [[f64; 3]; 9] = [
    [68.0, 1.0, 84.0],
    [71.0, 45.0, 123.0],
    [59.0, 82.0, 139.0],
    [44.0, 114.0, 142.0],
    [33.0, 145.0, 140.0],
    [40.0, 174.0, 128.0],
    [94.0, 201.0, 98.0],
    [173.0, 220.0, 48.0],
    [253.0, 231.0, 37.0],
];

/// The colour at `t` in 0..=1 of the scale, interpolated between the anchors.
pub(crate) fn scale_color(t: f64) -> String {
    let position = t.clamp(0.0, 1.0) * (VIRIDIS.len() - 1) as f64;
    let index = (position.floor() as usize).min(VIRIDIS.len() - 2);
    let fraction = position - index as f64;
    let channel = |k: usize| {
        (VIRIDIS[index][k] + (VIRIDIS[index + 1][k] - VIRIDIS[index][k]) * fraction).round().clamp(0.0, 255.0) as u8
    };
    format!("#{:02x}{:02x}{:02x}", channel(0), channel(1), channel(2))
}

const BACKGROUND: &str = "#ffffff";
pub(crate) const BOARD: &str = "#eef1f4";
pub(crate) const COPPER: &str = "#c4ccd4";
pub(crate) const INK: &str = "#1f2933";
pub(crate) const MUTED: &str = "#52606d";
pub(crate) const LOOP: &str = "#e00000";
pub(crate) const LOOP_OTHER: &str = "#0b4fff";
const VICTIM: &str = "#ff2bd6";

/// A pad whose field level is reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Victim {
    /// `sensitive_net` or `cable_connector`.
    pub kind: String,
    /// The net or connector name.
    pub name: String,
    pub pad: String,
    pub at: Point,
    /// `|H|` at the pad centre, dB(uA/m).
    pub level_db: f64,
    /// Distance in the board plane from the loop's path, mm.
    pub distance_mm: f64,
}

/// A pad drawn on the map (schematically, from the board scene).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnPad {
    pub label: String,
    pub polygon: Vec<Point>,
    pub in_loop: bool,
}

/// The computed map: everything the renderer draws.
#[derive(Clone, Debug)]
pub struct FieldMap {
    pub cell: String,
    pub loop_name: Option<String>,
    pub tags: Vec<String>,
    pub window: [f64; 4],
    pub grid: Grid,
    /// `|H|` in dB(uA/m) per cell, row-major from the window's `y0`.
    pub levels_db: Vec<f64>,
    pub max_db: f64,
    pub height_mm: f64,
    /// Amplitude, A; `None` = normalised to 1 A.
    pub current_a: Option<f64>,
    /// The scenario of the amplitude (`declared_operating_point` or `current_limit`); `None` for a report written
    /// before the finding recorded it (the amplitude is then the cell's current limit).
    pub scenario: Option<String>,
    /// What the amplitude is, in words (the finding's `loop_current.field_amplitude_basis`).
    pub amplitude_basis: Option<String>,
    pub plane_z_mm: Option<f64>,
    pub loop_xy: Vec<Point>,
    /// The loop as the report traced it: a closed 3D polyline, mm.
    pub loop_path: Vec<[f64; 3]>,
    /// Edge segments of the board outline in the scene, mm.
    pub board_edges: Vec<(Point, Point)>,
    pub victims: Vec<Victim>,
    pub pads: Vec<DrawnPad>,
    pub scene_tracks: Vec<(Point, Point, f64)>,
    pub scene_pads: Vec<Vec<Point>>,
}

/// One decimal, no negative zero.
fn n1(value: f64) -> String {
    let text = format!("{value:.1}");
    if text == "-0.0" { "0.0".to_owned() } else { text }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

/// `4.5 A` / `300 mA` as amperes.
pub fn parse_current(text: &str) -> Option<f64> {
    let text = text.trim();
    let end =
        text.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))).unwrap_or(text.len());
    let (value, unit) = text.split_at(end);
    let value: f64 = value.parse().ok()?;
    let scale = match unit.trim() {
        "A" | "" => 1.0,
        "mA" => 1e-3,
        "uA" | "µA" => 1e-6,
        "kA" => 1e3,
        _ => return None,
    };
    Some(value * scale).filter(|i| *i > 0.0)
}

/// The amplitude for a cell of an `emi` section written before the hot-loop finding recorded its own
/// (`loop_current`): the conducted prediction's `operating_point.current_limit_a` (the cell's current limit), else
/// the cell's `current_peak`.
pub fn cell_current(section: &Value, cell: &str) -> Option<f64> {
    let from_conducted = section
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .filter(|f| f.get("cell").and_then(Value::as_str) == Some(cell))
        .find_map(|f| f.get("measures")?.get("operating_point")?.get("current_limit_a").and_then(number));
    from_conducted.filter(|i| *i > 0.0).or_else(|| {
        section
            .get("cells")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .find(|c| c.get("controller").and_then(Value::as_str) == Some(cell))?
            .get("current_peak")?
            .as_str()
            .and_then(parse_current)
    })
}

/// The loop's 3D polyline in mm, or why the finding has none.
fn path_of(finding: &Value) -> Result<Vec<[f64; 3]>, String> {
    let measures = finding.get("measures");
    let path: Vec<[f64; 3]> = measures
        .and_then(|m| m.get("loop_path_3d_mm"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "the finding carries no loop_path_3d_mm (a report from before the near-field map: rerun `copper emi-check BOARD --run RUN --write`)"
                .to_owned()
        })?
        .iter()
        .filter_map(|v| {
            let v = v.as_array()?;
            Some([number(v.first()?)?, number(v.get(1)?)?, number(v.get(2)?)?])
        })
        .collect();
    if path.len() < 3 {
        return Err("the hot loop has fewer than three vertices".to_owned());
    }
    Ok(path)
}

fn distance_to_path(p: Point, path: &[Point]) -> f64 {
    let n = path.len();
    (0..n)
        .map(|i| {
            let (a, b) = (path[i], path[(i + 1) % n]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let length2 = dx * dx + dy * dy;
            let t = if length2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length2).clamp(0.0, 1.0) } else { 0.0 };
            (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
        })
        .fold(f64::INFINITY, f64::min)
}

/// The map of one `EMI-HOT-LOOP` finding of `section` over `scene`, in the plane `height_mm` above the top
/// copper, with cells of about `step_mm`.
pub fn hot_loop_field(
    finding: &Value,
    section: &Value,
    scene: &Scene,
    height_mm: f64,
    step_mm: f64,
) -> Result<FieldMap, String> {
    if finding.get("code").and_then(Value::as_str) != Some("EMI-HOT-LOOP") {
        return Err("the near-field map needs a hot-loop finding (EMI-HOT-LOOP)".to_owned());
    }
    let measures = finding.get("measures").ok_or("the finding has no measures")?;
    let path = path_of(finding)?;
    let cell = finding.get("cell").and_then(Value::as_str).unwrap_or("").to_owned();
    let plane_z_mm = measures.get("loop_reference_plane_z_mm").and_then(number);
    // The amplitude is the hot-loop finding's own: the same scenario as its radiated estimate and ringing. A report
    // written before the finding recorded it falls back to the cell's current limit.
    let recorded = measures.get("loop_current");
    let scenario = recorded.and_then(|c| c.get("scenario")).and_then(Value::as_str).map(str::to_owned);
    let amplitude_basis =
        recorded.and_then(|c| c.get("field_amplitude_basis")).and_then(Value::as_str).map(str::to_owned);
    let current_a = match recorded {
        Some(c) => c.get("field_amplitude_a").and_then(number).filter(|i| *i > 0.0),
        None => cell_current(section, &cell),
    };
    let amplitude = current_a.unwrap_or(1.0);
    let segments: Vec<Segment> = (0..path.len())
        .map(|i| {
            let (a, b) = (path[i], path[(i + 1) % path.len()]);
            field::filament([a[0] * 1e-3, a[1] * 1e-3, a[2] * 1e-3], [b[0] * 1e-3, b[1] * 1e-3, b[2] * 1e-3])
        })
        .collect();
    let plane_z = plane_z_mm.map(|z| z * 1e-3);
    let z = height_mm * 1e-3;
    let loop_xy: Vec<Point> = path.iter().map(|p| (p[0], p[1])).collect();

    // Victim pads from the scene.
    let mut victims = Vec::new();
    for entry in measures.get("field_victims").and_then(Value::as_array).unwrap_or_default() {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("sensitive_net");
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        for pad in entry.get("pads").and_then(Value::as_array).unwrap_or_default().iter().filter_map(Value::as_str) {
            let Some((reference, number)) = pad.rsplit_once('.') else { continue };
            for found in scene.pad(reference, number) {
                let at = found.center;
                let h = field::field_magnitude(&segments, amplitude, plane_z, [at.0 * 1e-3, at.1 * 1e-3, z]);
                victims.push(Victim {
                    kind: kind.to_owned(),
                    name: name.to_owned(),
                    pad: pad.to_owned(),
                    at,
                    level_db: field::db_ua_per_m(h),
                    distance_mm: distance_to_path(at, &loop_xy),
                });
            }
        }
    }
    victims.sort_by(|a, b| b.level_db.total_cmp(&a.level_db).then_with(|| a.pad.cmp(&b.pad)));

    // Window: the loop's box plus a margin on every side, victims close enough to be worth seeing, and every cell
    // within EXTENT_DB of the maximum (so the field is not cut at the edge); it may reach past the board outline.
    let bounds = |points: &[Point]| {
        points.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
            [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]
        })
    };
    let union = |a: [f64; 4], b: [f64; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    let loop_box = bounds(&loop_xy);
    let mut needed =
        [loop_box[0] - MARGIN_MM, loop_box[1] - MARGIN_MM, loop_box[2] + MARGIN_MM, loop_box[3] + MARGIN_MM];
    for victim in victims.iter().filter(|v| v.distance_mm <= REACH_MM) {
        let pad_points: Vec<Point> = scene
            .pad(victim.pad.rsplit_once('.').map_or("", |p| p.0), victim.pad.rsplit_once('.').map_or("", |p| p.1))
            .iter()
            .flat_map(|p| p.polygon.iter().copied())
            .collect();
        if pad_points.is_empty() {
            continue;
        }
        let b = bounds(&pad_points);
        needed = union(needed, [b[0] - 1.0, b[1] - 1.0, b[2] + 1.0, b[3] + 1.0]);
    }
    let wide =
        union(needed, [loop_box[0] - WIDE_MM, loop_box[1] - WIDE_MM, loop_box[2] + WIDE_MM, loop_box[3] + WIDE_MM]);
    let wide_grid =
        Grid::covering(wide[0] * 1e-3, wide[1] * 1e-3, wide[2] * 1e-3, wide[3] * 1e-3, step_mm * 1e-3, MAX_CELLS);
    let wide_levels: Vec<f64> =
        field::field_map(&segments, amplitude, plane_z, &wide_grid, z).into_iter().map(field::db_ua_per_m).collect();
    let max_db = wide_levels.iter().copied().filter(|v| v.is_finite()).fold(f64::NEG_INFINITY, f64::max);
    // The cell range of the window: what is needed, and what is within EXTENT_DB of the maximum.
    let mut range = [usize::MAX, usize::MAX, 0usize, 0usize];
    let mut take = |i: usize, j: usize| {
        range = [range[0].min(i), range[1].min(j), range[2].max(i), range[3].max(j)];
    };
    let cell_of = |value: f64, origin: f64, count: usize| {
        (((value - origin) / wide_grid.step).floor().max(0.0) as usize).min(count - 1)
    };
    take(cell_of(needed[0] * 1e-3, wide_grid.x0, wide_grid.nx), cell_of(needed[1] * 1e-3, wide_grid.y0, wide_grid.ny));
    take(cell_of(needed[2] * 1e-3, wide_grid.x0, wide_grid.nx), cell_of(needed[3] * 1e-3, wide_grid.y0, wide_grid.ny));
    for j in 0..wide_grid.ny {
        for i in 0..wide_grid.nx {
            if wide_levels[j * wide_grid.nx + i] >= max_db - EXTENT_DB {
                take(i, j);
            }
        }
    }
    let grid = Grid {
        x0: wide_grid.x0 + range[0] as f64 * wide_grid.step,
        y0: wide_grid.y0 + range[1] as f64 * wide_grid.step,
        step: wide_grid.step,
        nx: range[2] - range[0] + 1,
        ny: range[3] - range[1] + 1,
    };
    let levels_db: Vec<f64> = (range[1]..=range[3])
        .flat_map(|j| (range[0]..=range[2]).map(move |i| (i, j)))
        .map(|(i, j)| wide_levels[j * wide_grid.nx + i])
        .collect();
    // The window is whole cells.
    let window = [
        grid.x0 * 1e3,
        grid.y0 * 1e3,
        grid.x0 * 1e3 + grid.nx as f64 * grid.step * 1e3,
        grid.y0 * 1e3 + grid.ny as f64 * grid.step * 1e3,
    ];

    // Pads of the cell's loop components (labelled) and the other pads in the window (copper under the map).
    let loop_pads: Vec<&str> = measures
        .get("pad_sequence")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|label| label.rsplit(' ').next())
        .collect();
    let mut components: Vec<String> = Vec::new();
    components.push(cell.clone());
    components.extend(measures.get("loop_capacitor").and_then(Value::as_str).map(str::to_owned));
    for key in ["magnetic", "catch_diode"] {
        let name = section
            .get("cells")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .find(|c| c.get("controller").and_then(Value::as_str) == Some(cell.as_str()))
            .and_then(|c| c.get(key))
            .and_then(Value::as_str);
        components.extend(name.map(str::to_owned));
    }
    let inside = |polygon: &[Point]| {
        polygon.iter().any(|p| p.0 >= window[0] && p.0 <= window[2] && p.1 >= window[1] && p.1 <= window[3])
    };
    let top = scene.copper_layers.first().cloned().unwrap_or_default();
    let pads: Vec<DrawnPad> = scene
        .pads
        .iter()
        .filter(|p| components.contains(&p.reference) && inside(&p.polygon))
        .map(|p| DrawnPad {
            label: format!("{}.{}", p.reference, p.pad),
            polygon: p.polygon.clone(),
            in_loop: loop_pads.contains(&format!("{}.{}", p.reference, p.pad).as_str()),
        })
        .collect();
    let scene_pads: Vec<Vec<Point>> = scene
        .pads
        .iter()
        .filter(|p| p.layers.contains(&top) && inside(&p.polygon))
        .map(|p| p.polygon.clone())
        .collect();
    let scene_tracks: Vec<(Point, Point, f64)> = scene
        .tracks
        .iter()
        .filter(|t| t.layer == top && (inside(&[t.start]) || inside(&[t.end])))
        .map(|t| (t.start, t.end, t.width))
        .collect();

    let board_edges: Vec<(Point, Point)> = scene
        .edges
        .iter()
        .filter(|(a, b)| {
            let (lo, hi) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
            hi.0 >= window[0] && lo.0 <= window[2] && hi.1 >= window[1] && lo.1 <= window[3]
        })
        .copied()
        .collect();

    Ok(FieldMap {
        cell,
        loop_name: measures.get("loop").and_then(Value::as_str).map(str::to_owned),
        tags: finding
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect(),
        window,
        grid,
        levels_db,
        max_db,
        height_mm,
        current_a,
        scenario,
        amplitude_basis,
        plane_z_mm,
        loop_xy,
        loop_path: path,
        board_edges,
        victims,
        pads,
        scene_tracks,
        scene_pads,
    })
}

/// A picture of one map: SVG elements and the size they need.
pub struct FieldPicture {
    pub content: String,
    pub width: f64,
    pub height: f64,
}

/// Width of `text` at `size` px, with a margin: the sans-serif advance is about half an em.
pub(crate) fn text_px(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * 0.56
}

/// Wrap `text` at spaces into lines that fit `width` px at `size` px; a word longer than that stays whole.
pub(crate) fn wrap_px(text: &str, size: f64, width: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if text_px(line, size) + text_px(&format!(" {word}"), size) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// The colour bin of a level (`None` below the scale): one dB per step, the true maximum in the last.
fn bin(level: f64, max_db: f64) -> Option<usize> {
    if !level.is_finite() || level < max_db - RANGE_DB {
        return None;
    }
    let position = (level - (max_db - RANGE_DB)) / RANGE_DB * STEPS as f64;
    Some((position.floor() as usize).min(STEPS - 1))
}

/// Contour polylines (in the plane the sampler maps to) of `values` on a `nx` x `ny` grid at `level`, by marching
/// squares; closed rings repeat their first point. Deterministic: chains start at the lowest edge id.
fn contour(values: &[f64], nx: usize, ny: usize, level: f64, at: &dyn Fn(f64, f64) -> Point) -> Vec<Vec<Point>> {
    if nx < 2 || ny < 2 {
        return Vec::new();
    }
    let v = |i: usize, j: usize| values[j * nx + i].max(level - 1000.0);
    // An edge id: horizontal edge from sample (i, j) to (i + 1, j) is 2 (j nx + i), vertical to (i, j + 1) is one more.
    let (h, vert) = (|i: usize, j: usize| 2 * (j * nx + i), |i: usize, j: usize| 2 * (j * nx + i) + 1);
    let cross = |id: usize| -> Point {
        let (index, vertical) = (id / 2, id % 2 == 1);
        let (i, j) = (index % nx, index / nx);
        let (j2, i2) = if vertical { (j + 1, i) } else { (j, i + 1) };
        let (a, b) = (v(i, j), v(i2, j2));
        let t = if (b - a).abs() > 0.0 { ((level - a) / (b - a)).clamp(0.0, 1.0) } else { 0.5 };
        let (p, q) = (at(i as f64, j as f64), at(i2 as f64, j2 as f64));
        (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)
    };
    let mut segments: Vec<(usize, usize)> = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1));
            let mask = u8::from(a >= level)
                | u8::from(b >= level) << 1
                | u8::from(c >= level) << 2
                | u8::from(d >= level) << 3;
            // Bottom (a-b), right (b-c), top (d-c), left (a-d).
            let (e0, e1, e2, e3) = (h(i, j), vert(i + 1, j), h(i, j + 1), vert(i, j));
            let centre = (a + b + c + d) / 4.0 >= level;
            match mask {
                1 | 14 => segments.push((e3, e0)),
                2 | 13 => segments.push((e0, e1)),
                3 | 12 => segments.push((e3, e1)),
                4 | 11 => segments.push((e1, e2)),
                6 | 9 => segments.push((e0, e2)),
                7 | 8 => segments.push((e2, e3)),
                5 => {
                    if centre {
                        segments.extend([(e0, e1), (e2, e3)]);
                    } else {
                        segments.extend([(e3, e0), (e1, e2)]);
                    }
                }
                10 => {
                    if centre {
                        segments.extend([(e3, e0), (e1, e2)]);
                    } else {
                        segments.extend([(e0, e1), (e2, e3)]);
                    }
                }
                _ => {}
            }
        }
    }
    let mut at_edge: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, (a, b)) in segments.iter().enumerate() {
        at_edge.entry(*a).or_default().push(k);
        at_edge.entry(*b).or_default().push(k);
    }
    let mut used = vec![false; segments.len()];
    // Open chains start at an edge with one segment, rings anywhere.
    let mut starts: Vec<usize> = at_edge.iter().filter(|(_, list)| list.len() == 1).map(|(_, list)| list[0]).collect();
    starts.extend(0..segments.len());
    let mut lines: Vec<Vec<Point>> = Vec::new();
    for start in starts {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, tail) = segments[start];
        // Begin at the open end when there is one.
        let (first, mut tail) =
            if at_edge[&first].len() == 1 || at_edge[&tail].len() != 1 { (first, tail) } else { (tail, first) };
        let mut ids = vec![first, tail];
        while let Some(next) = at_edge[&tail].iter().copied().find(|k| !used[*k]) {
            used[next] = true;
            let (a, b) = segments[next];
            tail = if a == tail { b } else { a };
            ids.push(tail);
        }
        lines.push(ids.into_iter().map(cross).collect());
    }
    lines
}

pub(crate) fn path_data(points: &[Point], close: bool) -> String {
    let mut text = String::new();
    for (k, p) in points.iter().enumerate() {
        text.push_str(&format!("{}{} {}", if k == 0 { "M" } else { "L" }, n(p.0), n(p.1)));
    }
    if close {
        text.push('Z');
    }
    text
}

/// Free space for labels: rectangles already taken.
pub(crate) struct Placer {
    pub(crate) taken: Vec<[f64; 4]>,
    pub(crate) bounds: [f64; 4],
    /// Clearance kept round every label, in the units of the rectangles.
    pub(crate) pad: f64,
}

impl Placer {
    fn overlap(&self, r: [f64; 4]) -> f64 {
        self.taken
            .iter()
            .map(|t| ((r[2].min(t[2]) - r[0].max(t[0])).max(0.0)) * ((r[3].min(t[3]) - r[1].max(t[1])).max(0.0)))
            .sum()
    }

    /// Keep labels off the line from `a` to `b`: a small rectangle every `step`, `half` round the line.
    pub(crate) fn reserve_line(&mut self, a: Point, b: Point, step: f64, half: f64) {
        let length = (b.0 - a.0).hypot(b.1 - a.1);
        let count = (length / step).ceil().max(1.0) as usize;
        for k in 0..=count {
            let t = k as f64 / count as f64;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            self.taken.push([x - half, y - half, x + half, y + half]);
        }
    }

    pub(crate) fn reserve(&mut self, r: [f64; 4]) {
        self.taken.push(r);
    }

    /// The text of width `w` and height `h` near `anchor`: the first of the candidate offsets (of the text's lower-left
    /// corner) that stays inside the bounds and clear of everything taken, else the least overlapping one.
    pub(crate) fn place(&mut self, anchor: Point, w: f64, h: f64, offsets: &[Point]) -> Point {
        let mut best: Option<(f64, Point)> = None;
        for (dx, dy) in offsets {
            let (x, y) = (anchor.0 + dx, anchor.1 + dy);
            let rect = [x - self.pad, y - h, x + w + self.pad, y + 0.27 * h];
            let outside = rect[0] < self.bounds[0]
                || rect[2] > self.bounds[2]
                || rect[1] < self.bounds[1]
                || rect[3] > self.bounds[3];
            let cost = self.overlap(rect) + if outside { 1e6 } else { 0.0 };
            if best.is_none_or(|b| cost < b.0) {
                best = Some((cost, (x, y)));
            }
            if cost == 0.0 {
                break;
            }
        }
        let (_, (x, y)) = best.unwrap_or((0.0, anchor));
        self.taken.push([x - self.pad, y - h, x + w + self.pad, y + 0.27 * h]);
        (x, y)
    }
}

/// Text with a white halo (a stroke painted first), so it reads on the heat map and over copper.
pub(crate) fn halo(x: f64, y: f64, size: f64, weight: &str, anchor: &str, class: &str, text: &str) -> String {
    format!(
        "<text{} x=\"{}\" y=\"{}\" font-size=\"{}\" font-weight=\"{weight}\" text-anchor=\"{anchor}\" fill=\"{INK}\" stroke=\"#ffffff\" stroke-width=\"3\" stroke-linejoin=\"round\" paint-order=\"stroke\">{}</text>",
        if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") },
        n(x),
        n(y),
        n(size),
        escape(text)
    )
}

/// The amplitude sentence of a map, naming its scenario.
fn amplitude_text(map: &FieldMap) -> String {
    match (map.current_a, map.scenario.as_deref()) {
        (Some(i), Some("declared_operating_point")) => format!(
            "{} A, scenario declared_operating_point: {}",
            n(i),
            map.amplitude_basis.as_deref().unwrap_or("I_out + dI/2, the peak of the high-side switch current")
        ),
        (Some(i), Some(_)) => format!(
            "{} A, scenario current_limit: {}; declare load_current on the converter for an operating point",
            n(i),
            map.amplitude_basis
                .as_deref()
                .unwrap_or("current_peak, the switch current limit (a worst case, not a load current)")
        ),
        (Some(i), None) => format!(
            "{} A (the cell's current_peak, the switch current limit, a worst case, not a load current; this report does not record the scenario)",
            n(i)
        ),
        (None, _) => "1 A: the cell declares no usable current, so levels are per ampere".to_owned(),
    }
}

/// The amplitude in a few words, for the scale's footnote.
fn amplitude_short(map: &FieldMap) -> String {
    match (map.current_a, map.scenario.as_deref()) {
        (Some(i), Some(scenario)) => format!("{} A, scenario {scenario}", n(i)),
        (Some(i), None) => format!("{} A, current limit, scenario not recorded", n(i)),
        (None, _) => "per ampere".to_owned(),
    }
}

/// How the loop was drawn from the report, in words.
fn loop_basis(map: &FieldMap) -> &'static str {
    let has = |tag: &str| map.tags.iter().any(|t| t == tag);
    if has("traced_copper") {
        "traced copper path"
    } else if has("partially_traced") {
        "partly traced copper path (unrouted legs straight)"
    } else {
        "pad-centre estimate (no traced copper)"
    }
}

/// One run of the drawn loop: a polyline of one style.
pub(crate) struct LoopRun {
    /// `true` on the component layer (the layer of the first vertex), `false` on another.
    pub(crate) component_layer: bool,
    pub(crate) points: Vec<Point>,
}

/// The loop's legs by style, and the vias (a z change at one place): legs on the layer of the first vertex are
/// solid, legs on another layer (a different z) dashed.
pub(crate) fn loop_runs(path: &[[f64; 3]]) -> (Vec<LoopRun>, Vec<Point>) {
    const SAME: f64 = 1e-3;
    let count = path.len();
    let z0 = path.first().map_or(0.0, |p| p[2]);
    let mut runs: Vec<LoopRun> = Vec::new();
    let mut vias: Vec<Point> = Vec::new();
    for i in 0..count {
        let (a, b) = (path[i], path[(i + 1) % count]);
        let apart = (a[0] - b[0]).hypot(a[1] - b[1]);
        if (a[2] - b[2]).abs() < SAME {
            let component_layer = (a[2] - z0).abs() < SAME;
            match runs.last_mut() {
                Some(run) if run.component_layer == component_layer && run.points.last() == Some(&(a[0], a[1])) => {
                    run.points.push((b[0], b[1]));
                }
                _ => runs.push(LoopRun { component_layer, points: vec![(a[0], a[1]), (b[0], b[1])] }),
            }
        } else if apart < 0.05 {
            vias.push((b[0], b[1]));
        } else {
            // A slanted jump between layers (a leg kept straight): dashed.
            runs.push(LoopRun { component_layer: false, points: vec![(a[0], a[1]), (b[0], b[1])] });
        }
    }
    (runs, vias)
}

/// A tracks's outline: a rounded rectangle round the segment.
fn track_outline(
    a: Point,
    b: Point,
    width: f64,
    px: &dyn Fn(f64) -> f64,
    py: &dyn Fn(f64) -> f64,
    scale: f64,
) -> String {
    let (x1, y1, x2, y2) = (px(a.0), py(a.1), px(b.0), py(b.1));
    let w = (width * scale).max(1.0);
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" transform=\"translate({} {}) rotate({})\"/>",
        n(-w / 2.0),
        n(-w / 2.0),
        n((x2 - x1).hypot(y2 - y1) + w),
        n(w),
        n(w / 2.0),
        n(x1),
        n(y1),
        n((y2 - y1).atan2(x2 - x1).to_degrees())
    )
}

/// Draw one map.
pub fn field_picture(map: &FieldMap, scene_source: &str) -> FieldPicture {
    let [wx0, wy0, wx1, wy1] = map.window;
    let (ww, wh) = (wx1 - wx0, wy1 - wy0);
    let scale = (640.0 / ww).min(440.0 / wh).min(60.0);
    let ox = 24.0;
    let (map_w, map_h) = (ww * scale, wh * scale);
    let px = |x: f64| ox + (x - wx0) * scale;
    let side = ox + map_w + 28.0;
    let rel = |db: f64| db - map.max_db;

    // The right-hand column: sized from its text, so nothing runs off the canvas.
    let victim_rows: Vec<String> = map
        .victims
        .iter()
        .take(14)
        .map(|v| {
            format!(
                "{} {}: {} dB below peak, {} mm from the loop",
                v.name,
                v.pad,
                n1(-rel(v.level_db)),
                n(v.distance_mm)
            )
        })
        .collect();
    let heading = "field at sensitive pads and connectors";
    let longest = victim_rows.iter().map(|r| text_px(r, 11.0)).fold(text_px(heading, 12.0) * 1.1, f64::max);
    let column = longest.clamp(250.0, 400.0);
    let title = {
        let label = match &map.loop_name {
            Some(name) => format!("{} {name}", map.cell),
            None => map.cell.clone(),
        };
        format!("{label} hot loop: magnetic near field |H|, {} mm above the top copper", n(map.height_mm))
    };
    let width = (side + column + ox).max(text_px(&title, 16.0) * 1.07 + 2.0 * ox).max(560.0);

    let mut out: Vec<String> = Vec::new();
    out.push(format!(
        "<text x=\"{}\" y=\"26\" font-size=\"16\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(ox),
        escape(&title)
    ));
    let plane = match map.plane_z_mm {
        Some(z) => format!("ideal ground plane at z = {} mm by image currents", n(z)),
        None => "no reference plane: free space".to_owned(),
    };
    let mut y = 46.0;
    for text in [
        "Screening estimate, a ranking and placement aid: the quasi-static field of the hot-loop current only. The power inductor's own field is NOT included (coverage unsupported EMI-FIELD:inductor).".to_owned(),
        format!("Amplitude {}; loop: {}; {plane}.", amplitude_text(map), loop_basis(map)),
    ] {
        for line in wrap_px(&text, 12.0, width - 2.0 * ox) {
            out.push(format!(
                "<text class=\"note\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
                n(ox),
                n(y),
                escape(&line)
            ));
            y += 15.0;
        }
    }
    let oy = y + 8.0;
    let py = |y: f64| oy + (y - wy0) * scale;
    let bounds = [ox, oy, ox + map_w, oy + map_h];

    // The map window: board, copper, heat, copper outlines, contours, the loop and the labels.
    out.push(format!(
        "<svg x=\"{0}\" y=\"{1}\" width=\"{2}\" height=\"{3}\" viewBox=\"{0} {1} {2} {3}\"><rect x=\"{0}\" y=\"{1}\" width=\"{2}\" height=\"{3}\" fill=\"{BOARD}\"/>",
        n(ox),
        n(oy),
        n(map_w),
        n(map_h)
    ));
    out.push(format!("<g class=\"copper\" fill=\"{COPPER}\" stroke=\"{COPPER}\" stroke-linecap=\"round\">"));
    for (a, b, w) in &map.scene_tracks {
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"{}\"/>",
            n(px(a.0)),
            n(py(a.1)),
            n(px(b.0)),
            n(py(b.1)),
            n(w * scale)
        ));
    }
    for polygon in &map.scene_pads {
        out.push(format!("<polygon points=\"{}\" stroke=\"none\"/>", points(polygon, &px, &py)));
    }
    out.push("</g>".to_owned());

    // Heat map: one compound path per colour step, in cell units, runs of one colour in a row as one box.
    let step_px = map.grid.step * 1e3 * scale;
    let mut by_bin: BTreeMap<usize, String> = BTreeMap::new();
    for j in 0..map.grid.ny {
        let row = &map.levels_db[j * map.grid.nx..(j + 1) * map.grid.nx];
        let mut i = 0;
        while i < row.len() {
            let Some(color) = bin(row[i], map.max_db) else {
                i += 1;
                continue;
            };
            let start = i;
            while i < row.len() && bin(row[i], map.max_db) == Some(color) {
                i += 1;
            }
            by_bin.entry(color).or_default().push_str(&format!("M{start} {j}h{}v1h-{}z", i - start, i - start));
        }
    }
    out.push(format!(
        "<g class=\"heat\" opacity=\"0.8\" shape-rendering=\"crispEdges\" transform=\"translate({} {}) scale({})\">",
        n(px(map.grid.x0 * 1e3)),
        n(py(map.grid.y0 * 1e3)),
        n(step_px)
    ));
    for (color, d) in &by_bin {
        let fill = scale_color((*color as f64 + 0.5) / STEPS as f64);
        out.push(format!("<path fill=\"{fill}\" stroke=\"{fill}\" stroke-width=\"0.1\" d=\"{d}\"/>"));
    }
    out.push("</g>".to_owned());

    // The copper again above the map, as thin outlines, and the board outline.
    out.push(format!(
        "<g class=\"copper-outline\" fill=\"none\" stroke=\"{INK}\" stroke-opacity=\"0.6\" stroke-width=\"0.6\">"
    ));
    for (a, b, w) in &map.scene_tracks {
        out.push(track_outline(*a, *b, *w, &px, &py, scale));
    }
    for polygon in &map.scene_pads {
        out.push(format!("<polygon points=\"{}\"/>", points(polygon, &px, &py)));
    }
    out.push("</g>".to_owned());
    if !map.board_edges.is_empty() {
        let d: String = map
            .board_edges
            .iter()
            .map(|(a, b)| format!("M{} {}L{} {}", n(px(a.0)), n(py(a.1)), n(px(b.0)), n(py(b.1))))
            .collect();
        out.push(format!(
            "<path class=\"board-outline\" d=\"{d}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"1.2\"/>"
        ));
    }

    // Contours every CONTOUR_DB below the maximum.
    let mut placer = Placer { taken: Vec::new(), bounds, pad: 1.0 };
    let sample = |i: f64, j: f64| {
        (
            px(map.grid.x0 * 1e3 + (i + 0.5) * map.grid.step * 1e3),
            py(map.grid.y0 * 1e3 + (j + 0.5) * map.grid.step * 1e3),
        )
    };
    let mut contour_labels: Vec<(f64, Point)> = Vec::new();
    let mut under = String::new();
    let mut over = String::new();
    let mut level_db = -CONTOUR_DB;
    while level_db >= -RANGE_DB + 1e-9 {
        let lines = contour(&map.levels_db, map.grid.nx, map.grid.ny, map.max_db + level_db, &sample);
        for line in &lines {
            let d = path_data(line, line.len() > 3 && line.first() == line.last());
            under.push_str(&d);
            over.push_str(&d);
        }
        if let Some(longest) = lines.iter().max_by_key(|l| l.len()).filter(|l| l.len() >= 6) {
            // The highest point of the longest line, if it has room for a label.
            if let Some(top) = longest
                .iter()
                .copied()
                .filter(|p| p.1 > oy + 14.0 && p.0 > ox + 24.0 && p.0 < ox + map_w - 24.0)
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)))
            {
                contour_labels.push((level_db, top));
            }
        }
        level_db -= CONTOUR_DB;
    }
    if !under.is_empty() {
        out.push(format!(
            "<g class=\"contours\" fill=\"none\" stroke-linejoin=\"round\"><path d=\"{under}\" stroke=\"{INK}\" stroke-opacity=\"0.45\" stroke-width=\"2.4\"/><path d=\"{over}\" stroke=\"#ffffff\" stroke-width=\"1\"/></g>"
        ));
    }

    // Pads of the cell's components.
    out.push(format!("<g class=\"pads\" fill=\"#ffffff\" fill-opacity=\"0.25\" stroke=\"{INK}\" stroke-width=\"1\">"));
    for pad in &map.pads {
        out.push(format!(
            "<polygon class=\"pad{}\" data-pad=\"{}\" points=\"{}\"/>",
            if pad.in_loop { " loop" } else { "" },
            escape(&pad.label),
            points(&pad.polygon, &px, &py)
        ));
    }
    out.push("</g>".to_owned());

    // The loop as the report traced it: solid on the component layer, dashed on another, a circle at each via.
    let (runs, vias) = loop_runs(&map.loop_path);
    out.push("<g class=\"emi_loop\" fill=\"none\" stroke-linejoin=\"round\" stroke-linecap=\"round\">".to_owned());
    for casing in [true, false] {
        for run in &runs {
            let pixels: Vec<Point> = run.points.iter().map(|p| (px(p.0), py(p.1))).collect();
            let d = path_data(&pixels, false);
            if casing {
                out.push(format!("<path d=\"{d}\" stroke=\"#ffffff\" stroke-opacity=\"0.85\" stroke-width=\"4.5\"/>"));
            } else if run.component_layer {
                out.push(format!("<path class=\"loop-component\" d=\"{d}\" stroke=\"{LOOP}\" stroke-width=\"2\"/>"));
            } else {
                out.push(format!(
                    "<path class=\"loop-other\" d=\"{d}\" stroke=\"{LOOP_OTHER}\" stroke-width=\"2\" stroke-dasharray=\"6 4\"/>"
                ));
            }
        }
    }
    for via in &vias {
        out.push(format!(
            "<circle class=\"loop-via\" cx=\"{}\" cy=\"{}\" r=\"3.5\" fill=\"#ffffff\" stroke=\"{LOOP}\" stroke-width=\"1.8\"/>",
            n(px(via.0)),
            n(py(via.1))
        ));
    }
    out.push("</g>".to_owned());

    // Labels, placed so none overlaps another or a ring: victims first, then contour levels, then loop pads.
    let in_window = |p: Point| p.0 >= wx0 && p.0 <= wx1 && p.1 >= wy0 && p.1 <= wy1;
    let shown_victims: Vec<&Victim> = map.victims.iter().filter(|v| in_window(v.at)).collect();
    for run in &runs {
        for pair in run.points.windows(2) {
            placer.reserve_line((px(pair[0].0), py(pair[0].1)), (px(pair[1].0), py(pair[1].1)), 4.0, 2.5);
        }
    }
    for victim in &shown_victims {
        let (x, y) = (px(victim.at.0), py(victim.at.1));
        placer.reserve([x - 8.0, y - 8.0, x + 8.0, y + 8.0]);
    }
    let mut labels: Vec<String> = Vec::new();
    for (level, at) in &contour_labels {
        let text = format!("{} dB", n(*level));
        let w = text_px(&text, 9.0);
        let (x, y) = placer.place(*at, w, 9.0, &[(-w / 2.0, -3.0), (-w / 2.0, 11.0), (4.0, -3.0), (-w - 4.0, -3.0)]);
        labels.push(halo(x, y, 9.0, "normal", "start", "contour-label", &text));
    }
    for victim in &shown_victims {
        let (x, y) = (px(victim.at.0), py(victim.at.1));
        let text = format!("{} {} dB", victim.pad, n1(rel(victim.level_db)));
        let w = text_px(&text, 11.0) * 1.07;
        let (lx, ly) = placer.place(
            (x, y),
            w,
            11.0,
            &[
                (11.0, 4.0),
                (-w - 11.0, 4.0),
                (-w / 2.0, -12.0),
                (-w / 2.0, 22.0),
                (11.0, -10.0),
                (-w - 11.0, -10.0),
                (11.0, 18.0),
                (-w - 11.0, 18.0),
            ],
        );
        out.push(format!(
            "<g class=\"victim\" data-pad=\"{}\" data-net=\"{}\" data-level-db=\"{}\" data-below-peak-db=\"{}\"><circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"none\" stroke=\"{VICTIM}\" stroke-width=\"2\"/>{}</g>",
            escape(&victim.pad),
            escape(&victim.name),
            n1(victim.level_db),
            n1(-rel(victim.level_db)),
            n(x),
            n(y),
            halo(lx, ly, 11.0, "bold", "start", "", &text)
        ));
    }
    for pad in map.pads.iter().filter(|p| p.in_loop) {
        let c = centroid(&pad.polygon);
        let (x, y) = (px(c.0), py(c.1));
        let w = text_px(&pad.label, 10.0);
        let (lx, ly) = placer.place(
            (x, y),
            w,
            10.0,
            &[
                (5.0, -5.0),
                (5.0, 12.0),
                (-w - 5.0, -5.0),
                (-w - 5.0, 12.0),
                (-w / 2.0, -9.0),
                (-w / 2.0, 18.0),
                (5.0, 26.0),
                (-w - 5.0, 26.0),
            ],
        );
        labels.push(halo(lx, ly, 10.0, "normal", "start", "pad-label", &pad.label));
    }
    out.extend(labels);
    out.push("</svg>".to_owned());
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
        n(ox),
        n(oy),
        n(map_w),
        n(map_h)
    ));
    // Scale bar.
    let bar = [1.0, 2.0, 5.0, 10.0].iter().copied().rev().find(|b| b * scale <= map_w / 3.0).unwrap_or(1.0);
    out.push(format!(
        "<g class=\"scale\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"3\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{} mm</text></g>",
        n(ox),
        n(oy + map_h + 16.0),
        n(ox + bar * scale),
        n(oy + map_h + 16.0),
        n(ox + bar * scale + 6.0),
        n(oy + map_h + 20.0),
        n(bar)
    ));

    // Right column: colour bar in dB below the maximum, then the legend, then the field at the victims.
    let (bar_x, bar_y, bar_h) = (side, oy + 14.0, 200.0_f64.min(map_h));
    out.push("<g class=\"colorbar\">".to_owned());
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">dB relative to the map maximum</text>",
        n(bar_x),
        n(oy + 4.0)
    ));
    let block = bar_h / STEPS as f64;
    for k in 0..STEPS {
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"18\" height=\"{}\" fill=\"{}\"/>",
            n(bar_x),
            n(bar_y + bar_h - (k as f64 + 1.0) * block),
            n(block + 0.3),
            scale_color((k as f64 + 0.5) / STEPS as f64)
        ));
    }
    let ticks = (RANGE_DB / CONTOUR_DB).round() as usize;
    for k in 0..=ticks {
        let level = -RANGE_DB + CONTOUR_DB * k as f64;
        let y = bar_y + bar_h - bar_h * k as f64 / ticks as f64;
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{} dB</text>",
            n(bar_x + 18.0),
            n(y),
            n(bar_x + 23.0),
            n(y),
            n(bar_x + 27.0),
            n(y + 4.0),
            n(level)
        ));
    }
    let mut cy = bar_y + bar_h + 18.0;
    let note = format!(
        "0 dB = {} dB(uA/m) at the maximum ({}), a ranking aid. Contours every {} dB; transparent below {} dB.",
        n1(map.max_db),
        amplitude_short(map),
        n(CONTOUR_DB),
        n(-RANGE_DB)
    );
    for line in wrap_px(&note, 10.0, column) {
        out.push(format!(
            "<text class=\"scale-note\" x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"{MUTED}\">{}</text>",
            n(bar_x),
            n(cy),
            escape(&line)
        ));
        cy += 12.0;
    }
    out.push("</g>".to_owned());

    // Legend: the styles present in this map.
    cy += 10.0;
    let mut legend: Vec<(String, &str)> = Vec::new();
    let component_runs = runs.iter().any(|r| r.component_layer);
    if component_runs || runs.is_empty() {
        legend.push((
            format!("<line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"{LOOP}\" stroke-width=\"2\"/>"),
            "hot loop, component layer",
        ));
    }
    if runs.iter().any(|r| !r.component_layer) {
        legend.push((
            format!("<line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"{LOOP_OTHER}\" stroke-width=\"2\" stroke-dasharray=\"6 4\"/>"),
            "hot loop, another layer (dashed)",
        ));
    }
    if !vias.is_empty() {
        legend.push((
            format!("<circle cx=\"11\" cy=\"0\" r=\"3.5\" fill=\"#ffffff\" stroke=\"{LOOP}\" stroke-width=\"1.8\"/>"),
            "via in the loop",
        ));
    }
    if !shown_victims.is_empty() {
        legend.push((
            format!("<circle cx=\"11\" cy=\"0\" r=\"5\" fill=\"none\" stroke=\"{VICTIM}\" stroke-width=\"2\"/>"),
            "sensitive pad or connector",
        ));
    }
    if !under.is_empty() {
        legend.push((format!("<line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"{INK}\" stroke-opacity=\"0.45\" stroke-width=\"2.4\"/><line x1=\"0\" y1=\"0\" x2=\"22\" y2=\"0\" stroke=\"#ffffff\" stroke-width=\"1\"/>"), "contour"));
    }
    out.push("<g class=\"legend\">".to_owned());
    for (glyph, label) in &legend {
        out.push(format!("<g transform=\"translate({} {})\">{glyph}</g>", n(bar_x), n(cy)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{label}</text>",
            n(bar_x + 30.0),
            n(cy + 4.0)
        ));
        cy += 17.0;
    }
    out.push("</g>".to_owned());

    // The numbers that matter: the field at the victims, below the peak.
    cy += 14.0;
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"12\" font-weight=\"bold\" fill=\"{INK}\">{heading}</text>",
        n(bar_x),
        n(cy)
    ));
    cy += 17.0;
    if map.victims.is_empty() {
        for line in wrap_px("none: no sensitive net or cable connector has placed pads outside the cell", 11.0, column)
        {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                n(bar_x),
                n(cy),
                escape(&line)
            ));
            cy += 14.0;
        }
    }
    for row in &victim_rows {
        for (k, line) in wrap_px(row, 11.0, column - 12.0).iter().enumerate() {
            out.push(format!(
                "<text class=\"victim-row\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
                n(bar_x + if k == 0 { 0.0 } else { 12.0 }),
                n(cy),
                escape(line)
            ));
            cy += 14.0;
        }
        cy += 2.0;
    }

    // Footer: what the map is, in one neutral line or two.
    let mut foot = (oy + map_h + 44.0).max(cy + 8.0);
    for line in wrap_px(
        &format!(
            "Quasi-static, one static current round the loop, no shielding by other copper. Pads, copper and the loop are drawn from the board scene ({scene_source})."
        ),
        11.0,
        width - 2.0 * ox,
    ) {
        out.push(format!(
            "<text class=\"footer\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
            n(ox),
            n(foot),
            escape(&line)
        ));
        foot += 14.0;
    }
    FieldPicture { content: out.join("\n"), width, height: foot + 8.0 }
}

pub(crate) fn points(polygon: &[Point], px: &dyn Fn(f64) -> f64, py: &dyn Fn(f64) -> f64) -> String {
    polygon.iter().map(|p| format!("{},{}", n(px(p.0)), n(py(p.1)))).collect::<Vec<_>>().join(" ")
}

pub(crate) fn centroid(polygon: &[Point]) -> Point {
    let count = polygon.len().max(1) as f64;
    (polygon.iter().map(|p| p.0).sum::<f64>() / count, polygon.iter().map(|p| p.1).sum::<f64>() / count)
}

/// The SVG document of one or more maps, stacked.
pub fn field_svg(title: &str, pictures: &[FieldPicture]) -> String {
    stacked_svg(&format!("{} magnetic near field of the hot loop (screening estimate)", escape(title)), pictures, 560.0)
}

/// One SVG document of pictures stacked from the top; `title` is the (already escaped) document title.
pub(crate) fn stacked_svg(title: &str, pictures: &[FieldPicture], min_width: f64) -> String {
    let width = pictures.iter().map(|p| p.width).fold(min_width, f64::max);
    let height: f64 = pictures.iter().map(|p| p.height).sum::<f64>().max(1.0);
    let mut out = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\">",
        n(width),
        n(height),
        n(width),
        n(height)
    )];
    out.push(format!("<title>{title}</title>"));
    out.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(width), n(height)));
    let mut top = 0.0;
    for picture in pictures {
        out.push(format!(
            "<g transform=\"translate(0,{})\">
{}
</g>",
            n(top),
            picture.content
        ));
        top += picture.height;
    }
    out.push(
        "</svg>
"
        .to_owned(),
    );
    out.join(
        "
",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ScenePad;

    fn pad(reference: &str, number: &str, net: &str, x: f64, y: f64) -> ScenePad {
        ScenePad {
            reference: reference.to_owned(),
            pad: number.to_owned(),
            net: net.to_owned(),
            layers: vec!["F.Cu".to_owned()],
            polygon: vec![(x - 0.3, y - 0.3), (x + 0.3, y - 0.3), (x + 0.3, y + 0.3), (x - 0.3, y + 0.3)],
            center: (x, y),
            drill: 0.0,
        }
    }

    fn scene(r_at: Point) -> Scene {
        Scene {
            copper_layers: vec!["F.Cu".to_owned()],
            pads: vec![
                pad("U1", "1", "VIN", 10.0, 10.0),
                pad("CIN", "1", "VIN", 4.0, 10.0),
                pad("R1", "2", "FB", r_at.0, r_at.1),
            ],
            ..Scene::default()
        }
    }

    fn section(current: &str) -> Value {
        copper_json::parse(&format!(
            r#"{{"cells":[{{"controller":"U1","current_peak":{current},"magnetic":"L1","catch_diode":"D1"}}],"findings":[
              {{"code":"EMI-HOT-LOOP","cell":"U1","tags":["placement_estimate"],"measures":{{
                "loop_capacitor":"CIN","pad_sequence":["input capacitor CIN.1","hot supply U1.1"],
                "loop_path_3d_mm":[[4,10,0],[10,10,0],[10,13,0],[4,13,0]],"loop_reference_plane_z_mm":-0.2,
                "field_victims":[{{"kind":"sensitive_net","name":"FB","pads":["R1.2"]}}]}}}}]}}"#
        ))
        .unwrap()
    }

    fn finding(section: &Value) -> &Value {
        &section.get("findings").unwrap().as_array().unwrap()[0]
    }

    #[test]
    fn currents_parse_with_their_prefix() {
        assert_eq!(parse_current("4.5 A"), Some(4.5));
        assert_eq!(parse_current("300 mA"), Some(0.3));
        assert_eq!(parse_current("0 A"), None);
        assert_eq!(parse_current("5 V"), None);
    }

    #[test]
    fn the_victim_level_is_higher_the_nearer_the_pad_is_to_the_loop() {
        let s = section("\"4.5 A\"");
        let near = hot_loop_field(finding(&s), &s, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let far = hot_loop_field(finding(&s), &s, &scene((8.0, 30.0)), 1.0, 0.5).unwrap();
        assert_eq!((near.victims.len(), far.victims.len()), (1, 1));
        assert!(near.victims[0].level_db > far.victims[0].level_db + 10.0, "{near:?} {far:?}");
        assert!(near.victims[0].distance_mm < far.victims[0].distance_mm);
        assert_eq!(near.current_a, Some(4.5));
        // The hottest cell is next to the loop's wires, within the loop's box plus the height.
        let (index, _) = near.levels_db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        let (cx, cy) = near.grid.center(index % near.grid.nx, index / near.grid.nx);
        let (x, y) = (cx * 1e3, cy * 1e3);
        assert!((3.0..=11.0).contains(&x) && (9.0..=14.0).contains(&y), "{x} {y}");
    }

    fn section_with_scenario(scenario: &str, amplitude: &str, basis: &str) -> Value {
        copper_json::parse(&format!(
            r#"{{"cells":[{{"controller":"U1","current_peak":"4.5 A","magnetic":"L1","catch_diode":"D1"}}],"findings":[
              {{"code":"EMI-HOT-LOOP","cell":"U1","tags":["placement_estimate"],"measures":{{
                "loop_capacitor":"CIN","pad_sequence":["input capacitor CIN.1","hot supply U1.1"],
                "loop_current":{{"scenario":"{scenario}","field_amplitude_a":{amplitude},"field_amplitude_basis":"{basis}"}},
                "loop_path_3d_mm":[[4,10,0],[10,10,0],[10,13,0],[4,13,0]],"loop_reference_plane_z_mm":-0.2,
                "field_victims":[{{"kind":"sensitive_net","name":"FB","pads":["R1.2"]}}]}}}}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_map_takes_the_amplitude_and_scenario_the_hot_loop_finding_recorded() {
        // A declared operating point: the peak of the high-side switch current, not the 4.5 A limit of the cell.
        let declared = section_with_scenario(
            "declared_operating_point",
            "2.62",
            "I_out + dI/2, the peak of the high-side switch current",
        );
        let map = hot_loop_field(finding(&declared), &declared, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        assert_eq!(map.current_a, Some(2.62));
        assert_eq!(map.scenario.as_deref(), Some("declared_operating_point"));
        let svg = field_picture(&map, "test").content;
        assert!(svg.contains("Amplitude 2.62 A, scenario declared_operating_point"), "{svg}");
        assert!(!svg.contains("worst_case_current"), "the declared scenario is not the current limit");
        // The same field at the limit scales by 20 log10(4.5 / 2.62) at the victim.
        let limit = section_with_scenario(
            "current_limit",
            "4.5",
            "current_peak, the switch current limit (a worst case, not a load current)",
        );
        let at_limit = hot_loop_field(finding(&limit), &limit, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        assert_eq!(at_limit.current_a, Some(4.5));
        assert!((at_limit.victims[0].level_db - map.victims[0].level_db - 20.0 * (4.5f64 / 2.62).log10()).abs() < 1e-9);
        let svg = field_picture(&at_limit, "test").content;
        assert!(svg.contains("Amplitude 4.5 A, scenario current_limit") && svg.contains("load_current"), "{svg}");
        // A report from before the finding recorded its scenario falls back to the cell's current limit and says so.
        let old = section("\"4.5 A\"");
        let old_map = hot_loop_field(finding(&old), &old, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        assert_eq!((old_map.current_a, old_map.scenario.clone()), (Some(4.5), None));
        assert!(amplitude_text(&old_map).contains("does not record the scenario"));
    }

    #[test]
    fn without_a_current_the_map_is_per_ampere_and_scales_linearly() {
        let known = section("\"2 A\"");
        let unknown = section("null");
        let a = hot_loop_field(finding(&known), &known, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let b = hot_loop_field(finding(&unknown), &unknown, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        assert_eq!(b.current_a, None);
        assert!((a.victims[0].level_db - b.victims[0].level_db - 20.0 * 2.0f64.log10()).abs() < 1e-9);
        assert!(field_picture(&b, "test").content.contains("per ampere"));
    }

    #[test]
    fn the_svg_has_a_colour_bar_the_loop_outline_the_victim_label_and_the_caveats() {
        let s = section("\"4.5 A\"");
        let map = hot_loop_field(finding(&s), &s, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let svg = field_svg("EmiBuck", &[field_picture(&map, "test scene")]);
        assert_eq!(svg.matches("<g class=\"colorbar\">").count(), 1);
        assert_eq!(svg.matches("class=\"emi_loop\"").count(), 1);
        assert!(svg.contains("class=\"victim\" data-pad=\"R1.2\" data-net=\"FB\" data-level-db=\""), "{svg}");
        assert!(svg.contains("dB(uA/m)") && svg.contains("EMI-FIELD:inductor"));
        assert!(svg.contains("Screening estimate, a ranking and placement aid"));
        // One neutral note, no warning banner and no raw snake_case tags.
        assert!(!svg.contains("class=\"caveat\"") && !svg.contains("placement_estimate"), "{svg}");
        assert!(svg.contains("fill=\"#ffffff\"/>"));
        // Palette colours, 16 colour-bar steps, at most one rect per run of equal cells.
        assert!(svg.matches("<rect x=").count() > 16);
        assert!(map.grid.nx <= MAX_CELLS && map.grid.ny <= MAX_CELLS);
        assert_eq!(svg, field_svg("EmiBuck", &[field_picture(&map, "test scene")]));
    }

    fn section_with_path(path: &str) -> Value {
        copper_json::parse(&format!(
            r#"{{"cells":[{{"controller":"U1","current_peak":"4.5 A"}}],"findings":[
              {{"code":"EMI-HOT-LOOP","cell":"U1","tags":["traced_copper"],"measures":{{
                "loop_capacitor":"CIN","pad_sequence":["input capacitor CIN.1","hot supply U1.1"],
                "loop_path_3d_mm":{path},"loop_reference_plane_z_mm":null,
                "field_victims":[{{"kind":"sensitive_net","name":"FB","pads":["R1.2"]}}]}}}}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_loop_is_drawn_from_its_traced_path_solid_on_the_component_layer_dashed_on_another_with_vias() {
        // Two vias: the return leg runs on another layer (z = -0.2).
        let s = section_with_path("[[4,10,0],[10,10,0],[10,13,0],[10,13,-0.2],[4,13,-0.2],[4,13,0]]");
        let map = hot_loop_field(finding(&s), &s, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let svg = field_picture(&map, "test").content;
        assert_eq!(svg.matches("class=\"emi_loop\"").count(), 1, "{svg}");
        assert!(
            svg.matches("class=\"loop-component\"").count() >= 1 && svg.matches("class=\"loop-other\"").count() == 1
        );
        assert_eq!(svg.matches("class=\"loop-via\"").count(), 2, "{svg}");
        assert!(svg.contains("hot loop, another layer (dashed)") && svg.contains("via in the loop"), "{svg}");
        assert!(svg.contains("stroke-dasharray=\"6 4\""));
        assert!(svg.contains("loop: traced copper path"), "{svg}");
        // A loop on one layer has neither dashes nor vias, and the legend does not list them.
        let flat = section_with_path("[[4,10,0],[10,10,0],[10,13,0],[4,13,0]]");
        let flat_map = hot_loop_field(finding(&flat), &flat, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let svg = field_picture(&flat_map, "test").content;
        assert!(
            !svg.contains("class=\"loop-other\"")
                && !svg.contains("class=\"loop-via\"")
                && !svg.contains("another layer"),
            "{svg}"
        );
        assert!(svg.contains("hot loop, component layer"));
    }

    #[test]
    fn the_window_holds_the_loop_box_with_a_margin_on_every_side_and_the_board_outline_is_drawn() {
        let s = section("\"4.5 A\"");
        let mut with_edges = scene((8.0, 16.0));
        with_edges.edges = vec![
            ((2.0, 6.0), (30.0, 6.0)),
            ((30.0, 6.0), (30.0, 20.0)),
            ((30.0, 20.0), (2.0, 20.0)),
            ((2.0, 20.0), (2.0, 6.0)),
        ];
        let map = hot_loop_field(finding(&s), &s, &with_edges, 1.0, 0.25).unwrap();
        // The loop spans 4..10 x 10..13 mm.
        let [x0, y0, x1, y1] = map.window;
        assert!(
            x0 <= 4.0 - MARGIN_MM && y0 <= 10.0 - MARGIN_MM && x1 >= 10.0 + MARGIN_MM && y1 >= 13.0 + MARGIN_MM,
            "{:?}",
            map.window
        );
        assert!(
            !map.board_edges.is_empty() && map.board_edges.len() < 4,
            "only the edges that reach the window: {:?}",
            map.board_edges
        );
        let svg = field_picture(&map, "test").content;
        assert!(svg.contains("class=\"board-outline\""), "{svg}");
        // The field is not cut at the window edge: every edge row and column is weaker than EXTENT_DB below the maximum.
        let (nx, ny) = (map.grid.nx, map.grid.ny);
        let edge_max = (0..nx)
            .map(|i| map.levels_db[i])
            .chain((0..nx).map(|i| map.levels_db[(ny - 1) * nx + i]))
            .chain((0..ny).map(|j| map.levels_db[j * nx]))
            .chain((0..ny).map(|j| map.levels_db[j * nx + nx - 1]))
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(edge_max <= map.max_db - EXTENT_DB + 3.0, "cut at the window edge: {edge_max} against {}", map.max_db);
    }

    #[test]
    fn the_colour_scale_resolves_the_peak_and_contours_close_round_it() {
        // One dB per step; only the true maximum is in the last step.
        assert_eq!(bin(100.0, 100.0), Some(STEPS - 1));
        assert_eq!(bin(98.9, 100.0), Some(STEPS - 2));
        assert_eq!(bin(60.0, 100.0), Some(0));
        assert_eq!(bin(59.9, 100.0), None);
        assert_ne!(scale_color(0.0), scale_color(1.0));
        assert_eq!(scale_color(1.0), "#fde725");
        // A cone of 21 x 21 samples: the contour at 5 is one closed ring of radius about 5.
        let values: Vec<f64> =
            (0..21 * 21).map(|k| 10.0 - ((k % 21) as f64 - 10.0).hypot((k / 21) as f64 - 10.0)).collect();
        let rings = contour(&values, 21, 21, 5.0, &|i, j| (i, j));
        assert_eq!(rings.len(), 1, "{rings:?}");
        assert_eq!(rings[0].first(), rings[0].last(), "closed");
        assert!(rings[0].iter().all(|p| ((p.0 - 10.0).hypot(p.1 - 10.0) - 5.0).abs() < 0.2), "{:?}", rings[0]);
    }

    #[test]
    fn nothing_in_the_right_column_runs_past_the_canvas() {
        // A victim with a long name.
        let long = copper_json::parse(
            r#"{"cells":[{"controller":"U1","current_peak":"4.5 A"}],"findings":[
              {"code":"EMI-HOT-LOOP","cell":"U1","tags":[],"measures":{"loop_capacitor":"CIN",
               "pad_sequence":["input capacitor CIN.1","hot supply U1.1"],
               "loop_path_3d_mm":[[4,10,0],[10,10,0],[10,13,0],[4,13,0]],
               "field_victims":[{"kind":"sensitive_net","name":"A_RATHER_LONG_FEEDBACK_NET_NAME_FOR_THE_REGULATOR","pads":["R1.2"]}]}}]}"#,
        )
        .unwrap();
        let map = hot_loop_field(finding(&long), &long, &scene((8.0, 16.0)), 1.0, 0.5).unwrap();
        let picture = field_picture(&map, "test");
        let number = |line: &str, key: &str| -> Option<f64> {
            line.split(key).nth(1).and_then(|r| r.split('"').next()).and_then(|v| v.parse().ok())
        };
        for line in picture.content.lines().filter(|l| l.starts_with("<text")) {
            let (x, size) = (number(line, " x=\"").unwrap_or(0.0), number(line, "font-size=\"").unwrap_or(11.0));
            let text = line.split('>').nth(1).and_then(|r| r.split('<').next()).unwrap_or("");
            // Labels inside the map may be anchored at their end; the column and the notes start at x.
            if !line.contains("text-anchor=\"end\"") && !line.contains("text-anchor=\"middle\"") {
                assert!(x + text_px(text, size) <= picture.width, "{text:?} runs past {}: {line}", picture.width);
            }
        }
        // The row wraps instead of widening the column without bound.
        assert!(picture.content.matches("class=\"victim-row\"").count() >= 2, "a long row wraps");
    }

    #[test]
    fn a_finding_without_a_path_says_how_to_get_one() {
        let s = copper_json::parse(r#"{"findings":[{"code":"EMI-HOT-LOOP","cell":"U1","measures":{}}]}"#).unwrap();
        let error = hot_loop_field(finding(&s), &s, &Scene::default(), 1.0, 0.5).unwrap_err();
        assert!(error.contains("loop_path_3d_mm") && error.contains("--write"), "{error}");
        let other = copper_json::parse(r#"{"findings":[{"code":"EMI-PROXIMITY","cell":"U1","measures":{}}]}"#).unwrap();
        assert!(hot_loop_field(finding(&other), &other, &Scene::default(), 1.0, 0.5).is_err());
    }
}

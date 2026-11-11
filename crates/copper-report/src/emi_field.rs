//! The magnetic near-field map of a hot loop (`copper emi-check --plot-field`,
//! `copper view RUN --issue M<n> --emi-field`): the quasi-static `|H|` of the
//! loop's current path in a plane above the board, as a heat map over the
//! layout, with the field level at the pads of sensitive nets and cable
//! connectors.
//!
//! The source is the `loop_path_3d_mm` measure of an `EMI-HOT-LOOP` finding
//! (the loop as a closed 3D polyline, millimetres, z = 0 at the top copper
//! layer) with the `loop_reference_plane_z_mm` of the reference plane, and the
//! `field_victims` pad names. The amplitude is the cell's `current_peak` (the
//! switch current limit, a worst case), or 1 A ("per ampere") when unknown.
//! The field itself is `copper_emi::field`.
//!
//! **Screening estimate, not a field prediction.** Quasi-static (the region is
//! much smaller than a wavelength), the peak switch current as the amplitude,
//! only the hot loop carries current (the power inductor's own field, usually
//! the largest low-frequency source of an unshielded inductor, is NOT
//! included: coverage `unsupported EMI-FIELD:inductor`), an ideal infinite
//! ground plane by images, no shielding by other copper. A ranking and
//! placement aid.

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
/// Margin around the loop, mm.
const MARGIN_MM: f64 = 3.0;
/// Victim pads within this distance of the loop's box extend the window, mm.
const REACH_MM: f64 = 12.0;

/// Sixteen steps of a viridis-like scale, dark (weak) to yellow (strong).
pub const PALETTE: [&str; 16] = [
    "#440154", "#482475", "#463480", "#414487", "#3b528b", "#2f6c8e", "#2a788e", "#25848e", "#21918c", "#1e9c89",
    "#22a884", "#2fb47c", "#44bf70", "#7ad151", "#bddf26", "#fde725",
];

const BACKGROUND: &str = "#ffffff";
const BOARD: &str = "#eef1f4";
const COPPER: &str = "#c4ccd4";
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
const LOOP: &str = "#e00000";
const VICTIM: &str = "#ff2bd6";
const WARNING: &str = "#d4380d";

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
    pub plane_z_mm: Option<f64>,
    pub loop_xy: Vec<Point>,
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

/// The amplitude for a cell of an `emi` section: the conducted prediction's `current_amplitude_a`
/// (the same worst-case `current_peak`), else the cell's `current_peak`.
pub fn cell_current(section: &Value, cell: &str) -> Option<f64> {
    let from_conducted = section
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some("EMI-CONDUCTED"))
        .filter(|f| f.get("cell").and_then(Value::as_str) == Some(cell))
        .find_map(|f| f.get("measures")?.get("current_amplitude_a").and_then(number));
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
    let current_a = cell_current(section, &cell);
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

    // Window: the loop plus a margin, and victims close enough to be worth seeing.
    let bounds = |points: &[Point]| {
        points.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
            [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]
        })
    };
    let loop_box = bounds(&loop_xy);
    let mut window =
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
        window = [
            window[0].min(b[0] - 1.0),
            window[1].min(b[1] - 1.0),
            window[2].max(b[2] + 1.0),
            window[3].max(b[3] + 1.0),
        ];
    }
    let grid = Grid::covering(
        window[0] * 1e-3,
        window[1] * 1e-3,
        window[2] * 1e-3,
        window[3] * 1e-3,
        step_mm * 1e-3,
        MAX_CELLS,
    );
    // The window is widened to whole cells.
    let window = [
        window[0],
        window[1],
        grid.x0 * 1e3 + grid.nx as f64 * grid.step * 1e3,
        grid.y0 * 1e3 + grid.ny as f64 * grid.step * 1e3,
    ];
    let levels_db: Vec<f64> =
        field::field_map(&segments, amplitude, plane_z, &grid, z).into_iter().map(field::db_ua_per_m).collect();
    let max_db = levels_db.iter().copied().filter(|v| v.is_finite()).fold(f64::NEG_INFINITY, f64::max);

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
        plane_z_mm,
        loop_xy,
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

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// The palette index of a level, `None` below the range.
fn bin(level: f64, max_db: f64) -> Option<usize> {
    if !level.is_finite() || level < max_db - RANGE_DB {
        return None;
    }
    let position = (level - (max_db - RANGE_DB)) / RANGE_DB * PALETTE.len() as f64;
    Some((position.floor() as usize).min(PALETTE.len() - 1))
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
    let width = (side + 270.0).max(880.0);
    let mut out: Vec<String> = Vec::new();

    let label = match &map.loop_name {
        Some(name) => format!("{} {name}", map.cell),
        None => map.cell.clone(),
    };
    out.push(format!(
        "<text x=\"{}\" y=\"26\" font-size=\"16\" font-weight=\"bold\" fill=\"{INK}\">{} hot loop: magnetic near field |H|, {} mm above the top copper</text>",
        n(ox),
        escape(&label),
        n(map.height_mm)
    ));
    let amplitude = match map.current_a {
        Some(i) => format!(
            "{} A (the cell's current_peak, the limit, worst case, tag worst_case_current; typical operation is about {} A, 0.4 x the limit, a rule of thumb that is not applied to the map)",
            n(i),
            n(i * 0.4)
        ),
        None => "1 A: the cell declares no current_peak, so levels are per ampere".to_owned(),
    };
    let plane = match map.plane_z_mm {
        Some(z) => format!("ideal ground plane at z = {} mm by image currents", n(z)),
        None => "no reference plane: free space".to_owned(),
    };
    let tags = if map.tags.is_empty() { String::new() } else { format!("; loop tags: {}", map.tags.join(", ")) };
    let mut y = 46.0;
    for text in [
        format!(
            "Screening estimate. Level in dB(uA/m), amplitude {amplitude}; the colour scale spans {} dB below the map maximum of {} dB(uA/m).",
            n(RANGE_DB),
            n1(map.max_db)
        ),
        format!("Quasi-static Biot-Savart field of the hot-loop current path; {plane}{tags}."),
    ] {
        for line in wrap(&text, 125) {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
                n(ox),
                n(y),
                escape(&line)
            ));
            y += 15.0;
        }
    }
    // The inductor caveat, prominently.
    let caveat = wrap(
        "The power inductor's own field is NOT included (usually the largest low-frequency source of an unshielded inductor; coverage unsupported EMI-FIELD:inductor).",
        108,
    );
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fff4e5\" stroke=\"{WARNING}\" stroke-width=\"1\"/>",
        n(ox),
        n(y - 4.0),
        n(width - 2.0 * ox),
        n(caveat.len() as f64 * 15.0 + 8.0)
    ));
    y += 10.0;
    for line in &caveat {
        out.push(format!(
            "<text class=\"caveat\" x=\"{}\" y=\"{}\" font-size=\"12\" font-weight=\"bold\" fill=\"{WARNING}\">{}</text>",
            n(ox + 8.0),
            n(y),
            escape(line)
        ));
        y += 15.0;
    }
    let oy = y + 12.0;
    let py = |y: f64| oy + (y - wy0) * scale;

    // Board window, copper under the map.
    out.push(format!(
        "<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{BOARD}\"/>",
        n(ox),
        n(oy),
        n(map_w),
        n(map_h),
        n(ox),
        n(oy),
        n(map_w),
        n(map_h),
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

    // Heat map: runs of one colour in a row are one rect.
    out.push("<g class=\"heat\" opacity=\"0.85\" shape-rendering=\"crispEdges\">".to_owned());
    let step_px = map.grid.step * 1e3 * scale;
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
            out.push(format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                n(px(map.grid.x0 * 1e3 + start as f64 * map.grid.step * 1e3)),
                n(py(map.grid.y0 * 1e3 + j as f64 * map.grid.step * 1e3)),
                n((i - start) as f64 * step_px + 0.3),
                n(step_px + 0.5),
                PALETTE[color]
            ));
        }
    }
    out.push("</g>".to_owned());

    // Pads of the cell's components, the loop outline, victims.
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
    let outline: Vec<String> = map.loop_xy.iter().map(|p| format!("{},{}", n(px(p.0)), n(py(p.1)))).collect();
    out.push(format!(
        "<polygon class=\"emi_loop\" points=\"{}\" fill=\"none\" stroke=\"{LOOP}\" stroke-width=\"2\" stroke-linejoin=\"round\"/>",
        outline.join(" ")
    ));
    for pad in map.pads.iter().filter(|p| p.in_loop) {
        let c = centroid(&pad.polygon);
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"0.4\" paint-order=\"stroke\">{}</text>",
            n(px(c.0) + 4.0),
            n(py(c.1) - 4.0),
            escape(&pad.label)
        ));
    }
    for victim in &map.victims {
        let inside = victim.at.0 >= wx0 && victim.at.0 <= wx1 && victim.at.1 >= wy0 && victim.at.1 <= wy1;
        if !inside {
            continue;
        }
        let (x, y) = (px(victim.at.0), py(victim.at.1));
        let right_side = victim.at.0 > (wx0 + wx1) / 2.0;
        out.push(format!(
            "<g class=\"victim\" data-pad=\"{}\" data-net=\"{}\" data-level-db=\"{}\"><circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"none\" stroke=\"{VICTIM}\" stroke-width=\"2\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" font-weight=\"bold\" text-anchor=\"{}\" fill=\"#ffffff\" stroke=\"{INK}\" stroke-width=\"0.5\" paint-order=\"stroke\">{} {}</text></g>",
            escape(&victim.pad),
            escape(&victim.name),
            n1(victim.level_db),
            n(x),
            n(y),
            n(if right_side { x - 9.0 } else { x + 9.0 }),
            n(y + 4.0),
            if right_side { "end" } else { "start" },
            escape(&victim.pad),
            n1(victim.level_db)
        ));
    }
    out.push("</svg>".to_owned());
    out.push(format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
        n(ox),
        n(oy),
        n(map_w),
        n(map_h)
    ));
    // Scale bar: 5 mm or the nearest round length.
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

    // Colour bar: strong at the top.
    let (bar_x, bar_y, bar_h) = (side, oy, 260.0_f64.min(map_h));
    out.push("<g class=\"colorbar\">".to_owned());
    let block = bar_h / PALETTE.len() as f64;
    for (k, color) in PALETTE.iter().enumerate() {
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"18\" height=\"{}\" fill=\"{color}\"/>",
            n(bar_x),
            n(bar_y + bar_h - (k as f64 + 1.0) * block),
            n(block + 0.3)
        ));
    }
    for k in 0..=4 {
        let level = map.max_db - RANGE_DB + RANGE_DB * f64::from(k) / 4.0;
        let y = bar_y + bar_h - bar_h * f64::from(k) / 4.0;
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
            n(bar_x + 18.0),
            n(y),
            n(bar_x + 23.0),
            n(y),
            n(bar_x + 27.0),
            n(y + 4.0),
            n1(level)
        ));
    }
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">dB(uA/m)</text><text x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"{MUTED}\">transparent below the scale</text></g>",
        n(bar_x),
        n(bar_y - 6.0),
        n(bar_x),
        n(bar_y + bar_h + 16.0)
    ));

    // The numbers that matter: the field at the victims.
    let mut y = bar_y + bar_h + 44.0;
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"12\" font-weight=\"bold\" fill=\"{INK}\">field at sensitive pads and connectors</text>",
        n(side),
        n(y)
    ));
    y += 16.0;
    if map.victims.is_empty() {
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">none: no sensitive net or cable connector has placed pads outside the cell</text>",
            n(side),
            n(y)
        ));
    }
    for victim in map.victims.iter().take(14) {
        out.push(format!(
            "<text class=\"victim-row\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{} {}: {} dB(uA/m), {} mm from the loop</text>",
            n(side),
            n(y),
            escape(&victim.name),
            escape(&victim.pad),
            n1(victim.level_db),
            n(victim.distance_mm)
        ));
        y += 15.0;
    }
    let table_end = y;

    // Footer: what the map is and is not.
    let mut foot = (oy + map_h + 44.0).max(table_end + 10.0);
    out.push(format!(
        "<text class=\"footer\" x=\"{}\" y=\"{}\" font-size=\"13\" font-weight=\"bold\" fill=\"{WARNING}\">screening estimate, not a field prediction and not EMC qualification: a ranking and placement aid</text>",
        n(ox),
        n(foot)
    ));
    foot += 17.0;
    for text in [
        "Quasi-static: valid where the board region is much smaller than a wavelength (fine up to a few hundred MHz over a few cm). The peak switch current is the amplitude; only the hot loop carries current.",
        "An ideal infinite ground plane is modelled by image currents; no shielding by other copper. The power inductor's own field is not included.",
        &format!(
            "Pads and the loop are drawn schematically from the board scene ({scene_source}); copper of the top layer lies under the map, cells below the scale are transparent."
        ),
    ] {
        for line in wrap(text, 150) {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                n(ox),
                n(foot),
                escape(&line)
            ));
            foot += 14.0;
        }
    }
    FieldPicture { content: out.join("\n"), width, height: foot + 12.0 }
}

fn points(polygon: &[Point], px: &dyn Fn(f64) -> f64, py: &dyn Fn(f64) -> f64) -> String {
    polygon.iter().map(|p| format!("{},{}", n(px(p.0)), n(py(p.1)))).collect::<Vec<_>>().join(" ")
}

fn centroid(polygon: &[Point]) -> Point {
    let count = polygon.len().max(1) as f64;
    (polygon.iter().map(|p| p.0).sum::<f64>() / count, polygon.iter().map(|p| p.1).sum::<f64>() / count)
}

/// The SVG document of one or more maps, stacked.
pub fn field_svg(title: &str, pictures: &[FieldPicture]) -> String {
    let width = pictures.iter().map(|p| p.width).fold(880.0, f64::max);
    let height: f64 = pictures.iter().map(|p| p.height).sum::<f64>().max(1.0);
    let mut out = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" font-family=\"Helvetica, Arial, sans-serif\">",
        n(width),
        n(height),
        n(width),
        n(height)
    )];
    out.push(format!("<title>{} magnetic near field of the hot loop (screening estimate)</title>", escape(title)));
    out.push(format!("<rect width=\"{}\" height=\"{}\" fill=\"{BACKGROUND}\"/>", n(width), n(height)));
    let mut top = 0.0;
    for picture in pictures {
        out.push(format!("<g transform=\"translate(0,{})\">\n{}\n</g>", n(top), picture.content));
        top += picture.height;
    }
    out.push("</svg>\n".to_owned());
    out.join("\n")
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
        assert!(svg.contains("screening estimate, not a field prediction"));
        assert!(svg.contains("fill=\"#ffffff\"/>"));
        // Palette colours, 16 colour-bar steps, at most one rect per run of equal cells.
        assert!(svg.matches("<rect x=").count() > 16);
        assert!(map.grid.nx <= MAX_CELLS && map.grid.ny <= MAX_CELLS);
        assert_eq!(svg, field_svg("EmiBuck", &[field_picture(&map, "test scene")]));
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

//! Deterministic fallback 3D body for a footprint with no model file.
//!
//! The body is built from footprint data only: pad geometry, the fabrication
//! and courtyard outlines, the footprint name and `height_nm`. The same input
//! always gives the same bytes. The result is an *approximation* for fit and
//! clearance review, not a manufacturer model, and says so in
//! [`GeneratedBody::approximate`].
//!
//! Frame: millimetres in KiCad's 3D-model axes (x as in the footprint, y
//! pointing up the page so `y = -footprint_y`, z up from the board surface),
//! so the mesh needs no transform when referenced from a `(model ...)`.

use sha2::{Digest, Sha256};

use crate::footprint::{FootprintGraphic, FootprintLayer, FootprintPad, PadKind, PhysicalFootprint};

/// Bumping this changes every [`GeneratedBody::hash`].
pub const GENERATOR_VERSION: &str = "copper-body/1";

const MM: f64 = 1_000_000.0;

/// What the generator recognised the footprint as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// Two-terminal chip (R, C, L, LED, diode) with a body and end caps.
    Chip,
    /// Gull-wing perimeter package (SOIC, SOT, QFP, ...): body plus leads.
    Leaded,
    /// Pads under or at the body edge (QFN, DFN, BGA-like): body only.
    NoLead,
    /// A regular grid of through-hole pins (pin headers).
    PinHeader,
    /// Anything else: an extruded outline.
    Block,
}

impl BodyKind {
    pub fn value(self) -> &'static str {
        match self {
            BodyKind::Chip => "chip",
            BodyKind::Leaded => "leaded",
            BodyKind::NoLead => "no_lead",
            BodyKind::PinHeader => "pin_header",
            BodyKind::Block => "block",
        }
    }
}

/// A coloured triangle mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPart {
    pub name: &'static str,
    pub color: [f32; 3],
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl MeshPart {
    fn new(name: &'static str, color: [f32; 3]) -> Self {
        MeshPart { name, color, vertices: Vec::new(), triangles: Vec::new() }
    }

    /// An axis-aligned box from `min` to `max`, outward-facing.
    fn push_box(&mut self, min: [f64; 3], max: [f64; 3]) {
        let base = self.vertices.len() as u32;
        for index in 0..8u32 {
            let pick = |bit: u32, axis: usize| if index & bit == 0 { min[axis] } else { max[axis] };
            self.vertices.push([pick(1, 0), pick(2, 1), pick(4, 2)]);
        }
        const FACES: [[u32; 4]; 6] =
            [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];
        for face in FACES {
            let [a, b, c, d] = face.map(|v| base + v);
            self.triangles.push([a, b, c]);
            self.triangles.push([a, c, d]);
        }
    }
}

/// The generated body of one footprint.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedBody {
    pub kind: BodyKind,
    /// Always true: the shape is inferred, never measured from a datasheet.
    pub approximate: bool,
    pub parts: Vec<MeshPart>,
    /// SHA-256 over [`GENERATOR_VERSION`] and the VRML text.
    pub hash: String,
}

impl GeneratedBody {
    /// The bounding box `(min, max)` over all parts, in millimetres.
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for vertex in self.parts.iter().flat_map(|part| &part.vertices) {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
        (min, max)
    }

    /// VRML 2.0 text, one `Shape` per part, coordinates in millimetres.
    pub fn to_wrl(&self) -> String {
        wrl(&self.parts, self.kind)
    }
}

fn wrl(parts: &[MeshPart], kind: BodyKind) -> String {
    let mut out =
        format!("#VRML V2.0 utf8\n# {GENERATOR_VERSION} approximate {} body; units are millimetres\n", kind.value());
    for part in parts {
        let [r, g, b] = part.color;
        out.push_str(&format!(
            "DEF {} Shape {{\n appearance Appearance {{ material Material {{ diffuseColor {r:.3} {g:.3} {b:.3} }} }}\n geometry IndexedFaceSet {{\n  coord Coordinate {{ point [\n",
            part.name
        ));
        for [x, y, z] in &part.vertices {
            out.push_str(&format!("   {x:.6} {y:.6} {z:.6},\n"));
        }
        out.push_str("  ] }\n  coordIndex [\n");
        for [a, b, c] in &part.triangles {
            out.push_str(&format!("   {a}, {b}, {c}, -1,\n"));
        }
        out.push_str("  ]\n }\n}\n");
    }
    out
}

const BODY_COLOR: [f32; 3] = [0.15, 0.15, 0.17];
const METAL_COLOR: [f32; 3] = [0.78, 0.78, 0.74];
const CERAMIC_COLOR: [f32; 3] = [0.72, 0.62, 0.45];

/// An axis-aligned rectangle in model-frame millimetres.
#[derive(Clone, Copy, Debug)]
struct Rect {
    min: [f64; 2],
    max: [f64; 2],
}

impl Rect {
    fn size(&self) -> [f64; 2] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }

    fn center(&self) -> [f64; 2] {
        [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0]
    }

    fn grow(&self, by: f64) -> Rect {
        Rect { min: [self.min[0] - by, self.min[1] - by], max: [self.max[0] + by, self.max[1] + by] }
    }

    fn include(&mut self, point: [f64; 2]) {
        for (axis, value) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }

    fn empty() -> Rect {
        Rect { min: [f64::INFINITY; 2], max: [f64::NEG_INFINITY; 2] }
    }

    fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }

    fn contains(&self, point: [f64; 2]) -> bool {
        (0..2).all(|axis| point[axis] >= self.min[axis] - 1e-6 && point[axis] <= self.max[axis] + 1e-6)
    }
}

/// Footprint (x, y_down) in nanometres to model-frame millimetres.
fn model_point(x_nm: i64, y_nm: i64) -> [f64; 2] {
    [x_nm as f64 / MM, -(y_nm as f64) / MM]
}

/// A pad's extent after its rotation, as (width, height) in millimetres.
fn pad_extent(pad: &FootprintPad) -> [f64; 2] {
    let rotation = pad.rotation_degrees.to_string().parse::<f64>().unwrap_or(0.0).rem_euclid(180.0);
    let (w, h) = (pad.size.width_nm as f64 / MM, pad.size.height_nm as f64 / MM);
    if (rotation - 90.0).abs() < 1.0 { [h, w] } else { [w, h] }
}

fn pad_center(pad: &FootprintPad) -> [f64; 2] {
    model_point(pad.position.x_nm, pad.position.y_nm)
}

fn pad_rect(pad: &FootprintPad) -> Rect {
    let [x, y] = pad_center(pad);
    let [w, h] = pad_extent(pad);
    Rect { min: [x - w / 2.0, y - h / 2.0], max: [x + w / 2.0, y + h / 2.0] }
}

fn graphic_points(graphic: &FootprintGraphic) -> Vec<[f64; 2]> {
    let at = |p: &crate::geometry::Point| model_point(p.x_nm, p.y_nm);
    match graphic {
        FootprintGraphic::Line { start, end, .. } | FootprintGraphic::Rectangle { start, end, .. } => {
            vec![at(start), at(end)]
        }
        FootprintGraphic::Arc { start, midpoint, end, .. } => vec![at(start), at(midpoint), at(end)],
        FootprintGraphic::Circle { center, end, .. } => {
            let [cx, cy] = at(center);
            let radius = (((end.x_nm - center.x_nm) as f64).hypot((end.y_nm - center.y_nm) as f64)) / MM;
            vec![[cx - radius, cy - radius], [cx + radius, cy + radius]]
        }
        FootprintGraphic::Polygon { points, .. } => points.iter().map(at).collect(),
    }
}

fn layer_rect(footprint: &PhysicalFootprint, layer: FootprintLayer) -> Option<Rect> {
    let mut rect = Rect::empty();
    for graphic in footprint.graphics.iter().filter(|graphic| graphic.layer() == layer) {
        for point in graphic_points(graphic) {
            rect.include(point);
        }
    }
    (!rect.is_empty()).then_some(rect)
}

fn courtyard_rect(footprint: &PhysicalFootprint) -> Option<Rect> {
    let mut rect = Rect::empty();
    for point in &footprint.courtyard {
        rect.include(model_point(point.x_nm, point.y_nm));
    }
    (!rect.is_empty()).then_some(rect).or_else(|| layer_rect(footprint, FootprintLayer::Courtyard))
}

fn electrical_pads(footprint: &PhysicalFootprint) -> Vec<&FootprintPad> {
    footprint.pads.iter().filter(|pad| pad.kind.is_electrical()).collect()
}

fn pads_rect(pads: &[&FootprintPad]) -> Rect {
    let mut rect = Rect::empty();
    for pad in pads {
        let r = pad_rect(pad);
        rect.include(r.min);
        rect.include(r.max);
    }
    rect
}

/// EIA chip code -> (length, width, height, end-cap length), millimetres.
const CHIP_SIZES: [(&str, [f64; 4]); 6] = [
    ("0201", [0.60, 0.30, 0.30, 0.12]),
    ("0402", [1.00, 0.50, 0.50, 0.20]),
    ("0603", [1.60, 0.80, 0.80, 0.30]),
    ("0805", [2.00, 1.25, 0.85, 0.40]),
    ("1206", [3.20, 1.60, 0.90, 0.50]),
    ("1210", [3.20, 2.50, 0.90, 0.50]),
];

fn chip_code(name: &str) -> Option<[f64; 4]> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|token| CHIP_SIZES.iter().find(|(code, _)| *code == token).map(|(_, size)| *size))
}

fn height_mm(footprint: &PhysicalFootprint, fallback: f64) -> f64 {
    footprint.height_nm.map_or(fallback, |nm| nm as f64 / MM)
}

/// Default body height from the package family named in the footprint.
fn family_height(name: &str, leaded: bool) -> f64 {
    let upper = name.to_ascii_uppercase();
    let has = |needle: &str| upper.contains(needle);
    if has("TQFP") || has("LQFP") {
        1.4
    } else if has("SOIC") || has("SOP") || has("SO-") {
        1.75
    } else if has("TSSOP") || has("MSOP") || has("SSOP") || has("SOT") {
        1.1
    } else if has("QFN") || has("DFN") || has("LGA") {
        0.9
    } else if leaded {
        1.5
    } else {
        0.9
    }
}

/// Generate the fallback body for `footprint`.
pub fn generate_body(footprint: &PhysicalFootprint) -> GeneratedBody {
    let pads = electrical_pads(footprint);
    let (kind, parts) = chip(footprint, &pads)
        .or_else(|| pin_header(footprint, &pads))
        .or_else(|| perimeter(footprint, &pads))
        .unwrap_or_else(|| block(footprint, &pads));
    let mut body = GeneratedBody { kind, approximate: true, parts, hash: String::new() };
    let digest = Sha256::digest(format!("{GENERATOR_VERSION}\n{}", body.to_wrl()).as_bytes());
    body.hash = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    body
}

type Built = (BodyKind, Vec<MeshPart>);

fn chip(footprint: &PhysicalFootprint, pads: &[&FootprintPad]) -> Option<Built> {
    let [a, b] = pads else { return None };
    if a.kind != PadKind::Smd || b.kind != PadKind::Smd {
        return None;
    }
    let (ca, cb) = (pad_center(a), pad_center(b));
    let along_x = (ca[0] - cb[0]).abs() >= (ca[1] - cb[1]).abs();
    let axis = usize::from(!along_x);
    let cross = 1 - axis;
    let outer = pads_rect(pads);
    let center = outer.center();
    let span = outer.size()[axis];
    let pad_cross = outer.size()[cross];
    let (length, width, default_height, cap) = match chip_code(&footprint.name) {
        Some([l, w, h, c]) => (l, w, h, c),
        // Pads overhang the part by about a third of the span.
        None => (span * 0.62, pad_cross * 0.8, 0.8, span * 0.12),
    };
    let height = height_mm(footprint, default_height);
    let at = |along: f64, across: f64| -> [f64; 2] {
        let mut p = [0.0; 2];
        p[axis] = center[axis] + along;
        p[cross] = center[cross] + across;
        p
    };
    let mut body = MeshPart::new("body", CERAMIC_COLOR);
    let (lo, hi) = (at(-length / 2.0 + cap, -width / 2.0), at(length / 2.0 - cap, width / 2.0));
    body.push_box([lo[0].min(hi[0]), lo[1].min(hi[1]), 0.0], [lo[0].max(hi[0]), lo[1].max(hi[1]), height]);
    let mut caps = MeshPart::new("terminals", METAL_COLOR);
    for sign in [-1.0, 1.0] {
        let near = at(sign * (length / 2.0 - cap), -width / 2.0);
        let far = at(sign * length / 2.0, width / 2.0);
        caps.push_box(
            [near[0].min(far[0]), near[1].min(far[1]), 0.0],
            [near[0].max(far[0]), near[1].max(far[1]), height],
        );
    }
    Some((BodyKind::Chip, vec![body, caps]))
}

fn pin_header(footprint: &PhysicalFootprint, pads: &[&FootprintPad]) -> Option<Built> {
    if pads.len() < 2 || pads.iter().any(|pad| pad.kind != PadKind::ThroughHole) {
        return None;
    }
    let centers: Vec<[f64; 2]> = pads.iter().map(|pad| pad_center(pad)).collect();
    let mut pitch = f64::INFINITY;
    for (i, a) in centers.iter().enumerate() {
        for b in &centers[i + 1..] {
            let distance = (a[0] - b[0]).hypot(a[1] - b[1]);
            if distance > 1e-6 {
                pitch = pitch.min(distance);
            }
        }
    }
    if !pitch.is_finite() || !(1.0..=5.1).contains(&pitch) {
        return None;
    }
    let on_grid = centers.iter().all(|c| {
        (0..2).all(|axis| {
            let steps = (c[axis] - centers[0][axis]) / pitch;
            (steps - steps.round()).abs() < 0.01
        })
    });
    if !on_grid {
        return None;
    }
    let mut extent = Rect::empty();
    for c in &centers {
        extent.include(*c);
    }
    let outline = extent.grow(pitch / 2.0);
    let body_height = height_mm(footprint, 2.5);
    let mut body = MeshPart::new("body", BODY_COLOR);
    body.push_box([outline.min[0], outline.min[1], 0.0], [outline.max[0], outline.max[1], body_height]);
    let mut pins = MeshPart::new("pins", METAL_COLOR);
    let half = pitch * 0.25 / 2.0;
    for c in &centers {
        pins.push_box([c[0] - half, c[1] - half, -3.0], [c[0] + half, c[1] + half, body_height + 6.0]);
    }
    Some((BodyKind::PinHeader, vec![body, pins]))
}

/// Gull-wing and no-lead packages: the body is the fabrication outline.
fn perimeter(footprint: &PhysicalFootprint, pads: &[&FootprintPad]) -> Option<Built> {
    if pads.len() < 3 || pads.iter().any(|pad| pad.kind != PadKind::Smd) {
        return None;
    }
    let all = pads_rect(pads);
    let body_rect = layer_rect(footprint, FootprintLayer::Fabrication)
        .or_else(|| courtyard_rect(footprint).map(|r| r.grow(-0.25)))
        .unwrap_or(all);
    if body_rect.size().iter().any(|side| *side < 0.2) {
        return None;
    }
    let outside = pads.iter().filter(|pad| !body_rect.contains(pad_center(pad))).count();
    // Packages whose pads sit outside the body outline have leads; the rest do not.
    let leaded = outside * 2 >= pads.len();
    let height = height_mm(footprint, family_height(&footprint.name, leaded));
    let standoff = if leaded { 0.1 } else { 0.0 };
    let mut body = MeshPart::new("body", BODY_COLOR);
    body.push_box([body_rect.min[0], body_rect.min[1], standoff], [body_rect.max[0], body_rect.max[1], height]);
    let mut parts = vec![body];
    if leaded {
        let mut leads = MeshPart::new("leads", METAL_COLOR);
        let center = body_rect.center();
        for pad in pads.iter().filter(|pad| !body_rect.contains(pad_center(pad))) {
            let [x, y] = pad_center(pad);
            let [w, h] = pad_extent(pad);
            let (dx, dy) = (x - center[0], y - center[1]);
            // Lead runs from the body edge to just past the pad centre, along
            // whichever axis the pad lies on.
            if dx.abs() / (body_rect.size()[0] / 2.0) >= dy.abs() / (body_rect.size()[1] / 2.0) {
                let edge = center[0] + dx.signum() * body_rect.size()[0] / 2.0;
                let tip = x + dx.signum() * w * 0.27;
                leads.push_box([edge.min(tip), y - h * 0.25, 0.0], [edge.max(tip), y + h * 0.25, 0.2]);
            } else {
                let edge = center[1] + dy.signum() * body_rect.size()[1] / 2.0;
                let tip = y + dy.signum() * h * 0.27;
                leads.push_box([x - w * 0.25, edge.min(tip), 0.0], [x + w * 0.25, edge.max(tip), 0.2]);
            }
        }
        parts.push(leads);
    }
    Some((if leaded { BodyKind::Leaded } else { BodyKind::NoLead }, parts))
}

/// The catch-all: the courtyard (else fabrication, else pad) outline, extruded.
fn block(footprint: &PhysicalFootprint, pads: &[&FootprintPad]) -> Built {
    let rect = layer_rect(footprint, FootprintLayer::Fabrication)
        .or_else(|| courtyard_rect(footprint))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| {
            let r = pads_rect(pads);
            if r.is_empty() { Rect { min: [-0.5, -0.5], max: [0.5, 0.5] } } else { r }
        });
    let mut body = MeshPart::new("body", BODY_COLOR);
    body.push_box([rect.min[0], rect.min[1], 0.0], [rect.max[0], rect.max[1], height_mm(footprint, 1.0)]);
    (BodyKind::Block, vec![body])
}

#[cfg(test)]
mod tests {
    use copper_units::Decimal;
    use indexmap::IndexMap;

    use super::*;
    use crate::footprint::{PadShape, Size};
    use crate::geometry::Point;

    fn pad(number: &str, x_mm: f64, y_mm: f64, w_mm: f64, h_mm: f64, kind: PadKind) -> FootprintPad {
        let nm = |mm: f64| (mm * MM).round() as i64;
        FootprintPad {
            number: number.to_owned(),
            position: Point::new(nm(x_mm), nm(y_mm)),
            size: Size::new(nm(w_mm), nm(h_mm)).unwrap(),
            kind,
            shape: PadShape::Rectangle,
            rotation_degrees: Decimal::from_i64(0),
            drill: None,
            roundrect_ratio_ppm: 0,
            has_solder_mask: true,
            has_solder_paste: true,
            zone_connection: None,
            heatsink: false,
            remove_unused_layers: false,
            connector_contact: false,
        }
    }

    fn footprint(name: &str, pads: Vec<FootprintPad>, graphics: Vec<FootprintGraphic>) -> PhysicalFootprint {
        PhysicalFootprint {
            name: name.to_owned(),
            body_size: Size::new(1_000_000, 1_000_000).unwrap(),
            pads,
            source_library_id: None,
            graphics,
            keepouts: Vec::new(),
            metadata: IndexMap::new(),
            courtyard: Vec::new(),
            height_nm: None,
            clearance_nm: None,
            exclude_from_bom: false,
            exclude_from_pos_files: false,
            internal_pad_groups: Vec::new(),
        }
    }

    fn fab_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> FootprintGraphic {
        let nm = |mm: f64| (mm * MM).round() as i64;
        FootprintGraphic::Rectangle {
            start: Point::new(nm(x0), nm(y0)),
            end: Point::new(nm(x1), nm(y1)),
            width_nm: 100_000,
            layer: FootprintLayer::Fabrication,
            filled: false,
        }
    }

    fn soic8() -> PhysicalFootprint {
        let mut pads = Vec::new();
        for i in 0..4 {
            let y = -1.905 + 1.27 * f64::from(i);
            pads.push(pad(&format!("{}", i + 1), -2.475, y, 1.95, 0.6, PadKind::Smd));
            pads.push(pad(&format!("{}", 8 - i), 2.475, y, 1.95, 0.6, PadKind::Smd));
        }
        footprint("SOIC-8_3.9x4.9mm_P1.27mm", pads, vec![fab_rect(-1.95, -2.45, 1.95, 2.45)])
    }

    fn close(actual: [f64; 3], expected: [f64; 3]) {
        for axis in 0..3 {
            assert!((actual[axis] - expected[axis]).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn chip_0402_has_the_standard_envelope() {
        let fp = footprint(
            "C_0402_1005Metric",
            vec![pad("1", -0.48, 0.0, 0.56, 0.62, PadKind::Smd), pad("2", 0.48, 0.0, 0.56, 0.62, PadKind::Smd)],
            Vec::new(),
        );
        let body = generate_body(&fp);
        assert_eq!(body.kind, BodyKind::Chip);
        assert!(body.approximate);
        let (min, max) = body.bounds();
        close(min, [-0.5, -0.25, 0.0]);
        close(max, [0.5, 0.25, 0.5]);
    }

    #[test]
    fn soic8_body_is_the_fabrication_outline_with_leads() {
        let body = generate_body(&soic8());
        assert_eq!(body.kind, BodyKind::Leaded);
        let part = |name: &str| body.parts.iter().find(|p| p.name == name).unwrap();
        let extent = |part: &MeshPart, axis: usize| {
            let values = part.vertices.iter().map(|v| v[axis]);
            (values.clone().fold(f64::INFINITY, f64::min), values.fold(f64::NEG_INFINITY, f64::max))
        };
        let body_part = part("body");
        assert_eq!(extent(body_part, 0), (-1.95, 1.95));
        assert_eq!(extent(body_part, 1), (-2.45, 2.45));
        assert_eq!(extent(body_part, 2), (0.1, 1.75));
        let leads = part("leads");
        assert_eq!(leads.vertices.len(), 8 * 8);
        let (lead_min, lead_max) = extent(leads, 0);
        assert!((lead_min + 3.0015).abs() < 1e-6 && (lead_max - 3.0015).abs() < 1e-6);
    }

    #[test]
    fn height_override_applies() {
        let mut fp = soic8();
        fp.height_nm = Some(2_000_000);
        assert_eq!(generate_body(&fp).bounds().1[2], 2.0);
    }

    #[test]
    fn pin_header_is_recognised_on_its_pitch() {
        let pads = (0..4).map(|i| pad(&format!("{}", i + 1), 0.0, 2.54 * f64::from(i), 1.7, 1.7, PadKind::ThroughHole));
        let mut pads: Vec<FootprintPad> = pads.collect();
        for p in &mut pads {
            p.drill = Some(Size::new(1_000_000, 1_000_000).unwrap());
        }
        let body = generate_body(&footprint("PinHeader_1x04_P2.54mm_Vertical", pads, Vec::new()));
        assert_eq!(body.kind, BodyKind::PinHeader);
        let (min, max) = body.bounds();
        assert_eq!((min[2], max[2]), (-3.0, 8.5));
        assert!((max[0] - min[0] - 2.54).abs() < 1e-9);
    }

    #[test]
    fn unknown_footprints_fall_back_to_a_block() {
        let fp = footprint("Odd", vec![pad("1", 0.0, 0.0, 2.0, 3.0, PadKind::Smd)], Vec::new());
        let body = generate_body(&fp);
        assert_eq!(body.kind, BodyKind::Block);
        assert!(body.approximate);
        close(body.bounds().1, [1.0, 1.5, 1.0]);
    }

    #[test]
    fn output_is_deterministic_and_input_sensitive() {
        let (a, b) = (generate_body(&soic8()), generate_body(&soic8()));
        assert_eq!(a, b);
        assert_eq!(a.to_wrl(), b.to_wrl());
        assert_eq!(a.hash.len(), 64);
        let mut taller = soic8();
        taller.height_nm = Some(2_000_000);
        assert_ne!(a.hash, generate_body(&taller).hash);
    }

    #[test]
    fn y_axis_is_flipped_into_the_model_frame() {
        let fp = footprint("Odd", vec![pad("1", 0.0, 2.0, 1.0, 1.0, PadKind::Smd)], Vec::new());
        let (min, max) = generate_body(&fp).bounds();
        assert!(max[1] <= -1.5 + 1e-9 && min[1] >= -2.5 - 1e-9);
    }
}

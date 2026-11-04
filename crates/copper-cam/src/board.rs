//! The plotted content of a `.kicad_pcb`: what the fabrication artwork,
//! drills, netlist and positions are made from, in board coordinates.
//!
//! The reader accepts both CopperScript-written boards and boards KiCad saved
//! after a refill. It fails closed: any construct whose plotted geometry this
//! crate does not reproduce (custom or chamfered pads, pad stacks, unused-layer
//! removal, blind vias, hatched fills, dashed strokes, knockout or visible
//! footprint text, dimensions, images ...) is an error, never skipped.

use copper_physical::kicad_mod::{self, List, Node};

use crate::geometry::{Contour, Edge, Point, rotate};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Front,
    Back,
}

impl Side {
    pub fn prefix(self) -> &'static str {
        match self {
            Side::Front => "F",
            Side::Back => "B",
        }
    }
}

/// A plotted layer. Copper layers are numbered in stack order, `F.Cu` = 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Copper(usize),
    Mask(Side),
    Paste(Side),
    Silk(Side),
    EdgeCuts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadKind {
    Smd,
    ThroughHole,
    NonPlated,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadShape {
    Circle,
    Rect,
    Oval,
    /// Corner radius ratio of the smaller side.
    RoundRect(f64),
}

/// The layers a pad or via occupies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerSet {
    pub copper: Vec<bool>,
    pub mask: [bool; 2],
    pub paste: [bool; 2],
}

impl LayerSet {
    pub fn has(&self, layer: Layer) -> bool {
        match layer {
            Layer::Copper(index) => self.copper.get(index).copied().unwrap_or(false),
            Layer::Mask(side) => self.mask[side as usize],
            Layer::Paste(side) => self.paste[side as usize],
            Layer::Silk(_) | Layer::EdgeCuts => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    pub number: String,
    pub kind: PadKind,
    pub shape: PadShape,
    pub center: Point,
    /// Absolute pad angle in degrees, as KiCad stores it.
    pub angle: f64,
    pub width: i64,
    pub height: i64,
    /// Drill width and height; unequal sizes are a slot.
    pub drill: Option<(i64, i64)>,
    pub layers: LayerSet,
    pub net: Option<String>,
    pub mask_margin: Option<i64>,
    pub paste_margin: Option<i64>,
    pub paste_ratio: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    pub reference: String,
    pub value: String,
    pub package: String,
    pub position: Point,
    pub angle: f64,
    pub side: Side,
    pub exclude_from_pos_files: bool,
    pub dnp: bool,
    pub mask_margin: Option<i64>,
    pub paste_margin: Option<i64>,
    pub paste_ratio: Option<f64>,
    pub pads: Vec<Pad>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackPath {
    Line { start: Point, end: Point },
    Arc { start: Point, mid: Point, end: Point },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub layer: usize,
    pub width: i64,
    pub net: Option<String>,
    pub path: TrackPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Via {
    pub center: Point,
    pub size: i64,
    pub drill: i64,
    pub net: Option<String>,
    /// `Some(true)` tents (covers) the via on that side; `None` uses the board default.
    pub tented: [Option<bool>; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Line { start: Point, end: Point },
    Arc { start: Point, mid: Point, end: Point },
    Circle { center: Point, radius: i64 },
    Polygon(Contour),
}

/// A board or footprint graphic, in board coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drawing {
    pub layer: Layer,
    pub width: i64,
    pub filled: bool,
    pub shape: Shape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub layer: Layer,
    pub text: String,
    pub position: Point,
    pub angle: f64,
    /// Glyph width and height.
    pub size: (i64, i64),
    pub thickness: i64,
    pub bold: bool,
    pub italic: bool,
    pub mirrored: bool,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub line_spacing: f64,
}

/// One filled area of a zone, as the refill left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub layer: Layer,
    pub net: Option<String>,
    pub contour: Contour,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub mask_margin: i64,
    pub paste_margin: i64,
    pub paste_ratio: f64,
    pub tented: [bool; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub copper_layers: Vec<String>,
    pub setup: Setup,
    pub footprints: Vec<Footprint>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub drawings: Vec<Drawing>,
    pub texts: Vec<Text>,
    pub fills: Vec<Fill>,
}

impl Board {
    pub fn copper_count(&self) -> usize {
        self.copper_layers.len()
    }

    /// Every plotted layer, copper first, in the order the files are listed.
    pub fn layers(&self) -> Vec<Layer> {
        let mut layers: Vec<Layer> = (0..self.copper_count()).map(Layer::Copper).collect();
        layers.extend([
            Layer::Mask(Side::Front),
            Layer::Mask(Side::Back),
            Layer::Silk(Side::Front),
            Layer::Silk(Side::Back),
            Layer::Paste(Side::Front),
            Layer::Paste(Side::Back),
            Layer::EdgeCuts,
        ]);
        layers
    }

    /// The KiCad layer name, e.g. `In1.Cu` or `F.Silkscreen`.
    pub fn layer_name(&self, layer: Layer) -> String {
        match layer {
            Layer::Copper(index) => self.copper_layers[index].clone(),
            Layer::Mask(side) => format!("{}.Mask", side.prefix()),
            Layer::Paste(side) => format!("{}.Paste", side.prefix()),
            Layer::Silk(side) => format!("{}.Silkscreen", side.prefix()),
            Layer::EdgeCuts => "Edge.Cuts".to_owned(),
        }
    }

    pub fn pads(&self) -> impl Iterator<Item = (&Footprint, &Pad)> {
        self.footprints.iter().flat_map(|footprint| footprint.pads.iter().map(move |pad| (footprint, pad)))
    }

    /// Whether the via is covered by mask on `side`.
    pub fn via_tented(&self, via: &Via, side: Side) -> bool {
        via.tented[side as usize].unwrap_or(self.setup.tented[side as usize])
    }
}

// ---------------------------------------------------------------------------
// s-expression helpers

fn tag(node: &List) -> Option<&str> {
    kicad_mod::atom(node, 0)
}

fn lists(node: &List) -> impl Iterator<Item = &List> {
    node.iter().skip(1).filter_map(|item| match item {
        Node::List(list) => Some(list),
        Node::Atom(_) => None,
    })
}

fn has_atom(node: &List, name: &str) -> bool {
    node.iter().skip(1).any(|item| matches!(item, Node::Atom(text) if text == name))
}

/// `(name yes)`, `(name)` or a bare `name` atom.
fn flag(node: &List, name: &str) -> bool {
    has_atom(node, name)
        || kicad_mod::first(node, name).is_some_and(|child| !matches!(kicad_mod::atom(child, 1), Some("no" | "false")))
}

fn string(node: &List, index: usize) -> Option<String> {
    kicad_mod::atom(node, index).map(str::to_owned)
}

/// A KiCad millimetre decimal as integer nanometres, half away from zero
/// beyond six decimals.
pub fn parse_nm(text: &str) -> Option<i64> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut value: i64 = if whole.is_empty() { 0 } else { whole.parse::<i64>().ok()?.checked_mul(1_000_000)? };
    let digits = fraction.as_bytes();
    let mut scale = 100_000;
    for &digit in digits.iter().take(6) {
        value += i64::from(digit - b'0') * scale;
        scale /= 10;
    }
    if digits.get(6).is_some_and(|&digit| digit >= b'5') {
        value += 1;
    }
    Some(if negative { -value } else { value })
}

fn number(node: &List, index: usize, what: &str) -> Result<i64> {
    let text = kicad_mod::atom(node, index).ok_or_else(|| format!("missing {what}"))?;
    parse_nm(text).ok_or_else(|| format!("invalid {what} {text:?}"))
}

fn real(node: &List, index: usize, what: &str) -> Result<f64> {
    let text = kicad_mod::atom(node, index).ok_or_else(|| format!("missing {what}"))?;
    text.parse::<f64>().ok().filter(|value| value.is_finite()).ok_or_else(|| format!("invalid {what} {text:?}"))
}

fn point(node: &List, what: &str) -> Result<Point> {
    Ok(Point::new(number(node, 1, what)?, number(node, 2, what)?))
}

fn child<'a>(node: &'a List, name: &str, what: &str) -> Result<&'a List> {
    kicad_mod::first(node, name).ok_or_else(|| format!("{what} without ({name})"))
}

fn child_point(node: &List, name: &str, what: &str) -> Result<Point> {
    point(child(node, name, what)?, &format!("{what} {name}"))
}

fn optional_nm(node: &List, name: &str) -> Result<Option<i64>> {
    kicad_mod::first(node, name).map(|child| number(child, 1, name)).transpose()
}

fn optional_real(node: &List, name: &str) -> Result<Option<f64>> {
    kicad_mod::first(node, name).map(|child| real(child, 1, name)).transpose()
}

/// The net of an item: `(net "NAME")` (KiCad 10), `(net CODE "NAME")`, or
/// `(net CODE)` resolved through the board's net table (KiCad 9 and older).
fn net(node: &List, stack: &Stack) -> Option<String> {
    let net = kicad_mod::first(node, "net")?;
    let name = match (net.len(), &stack.nets) {
        (2, Some(table)) => table.get(kicad_mod::atom(net, 1)?)?.clone(),
        (2 | 3, _) => string(net, net.len() - 1)?,
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

/// `(at x y [angle])`.
fn placement(node: &List, what: &str) -> Result<(Point, f64)> {
    let at = child(node, "at", what)?;
    let angle = if kicad_mod::atom(at, 3).is_some() { real(at, 3, &format!("{what} angle"))? } else { 0.0 };
    Ok((point(at, &format!("{what} position"))?, angle))
}

// ---------------------------------------------------------------------------
// layers

struct Stack {
    copper: Vec<String>,
    /// Net code to name, for boards that still carry a net table.
    nets: Option<indexmap::IndexMap<String, String>>,
}

impl Stack {
    fn layer(&self, name: &str) -> Option<Layer> {
        if let Some(index) = self.copper.iter().position(|copper| copper == name) {
            return Some(Layer::Copper(index));
        }
        let (side, rest) = match name.split_once('.') {
            Some(("F", rest)) => (Side::Front, rest),
            Some(("B", rest)) => (Side::Back, rest),
            _ => return (name == "Edge.Cuts").then_some(Layer::EdgeCuts),
        };
        match rest {
            "Mask" => Some(Layer::Mask(side)),
            "Paste" => Some(Layer::Paste(side)),
            "SilkS" | "Silkscreen" => Some(Layer::Silk(side)),
            _ => None,
        }
    }

    fn layer_set(&self, node: &List, what: &str) -> Result<LayerSet> {
        let mut set = LayerSet { copper: vec![false; self.copper.len()], ..LayerSet::default() };
        let Some(layers) = kicad_mod::first(node, "layers") else {
            return Err(format!("{what} without (layers)"));
        };
        for index in 1..layers.len() {
            let name = string(layers, index).unwrap_or_default();
            let (sides, kind): (&[Side], &str) = match name.split_once('.') {
                Some(("*" | "F&B", kind)) => (&[Side::Front, Side::Back], kind),
                Some(("F", kind)) => (&[Side::Front], kind),
                Some(("B", kind)) => (&[Side::Back], kind),
                _ => (&[], name.as_str()),
            };
            if kind == "Cu" && name.starts_with('*') {
                set.copper.iter_mut().for_each(|layer| *layer = true);
                continue;
            }
            if let Some(Layer::Copper(index)) = self.layer(&name) {
                set.copper[index] = true;
                continue;
            }
            if kind == "Cu" {
                // F&B.Cu: both outer layers.
                if let Some(first) = set.copper.first_mut() {
                    *first = true;
                }
                if let Some(last) = set.copper.last_mut() {
                    *last = true;
                }
                continue;
            }
            for &side in sides {
                match kind {
                    "Mask" => set.mask[side as usize] = true,
                    "Paste" => set.paste[side as usize] = true,
                    _ => {}
                }
            }
        }
        Ok(set)
    }

    /// The plotted layer of a graphic, or `None` for a documentation layer.
    fn graphic_layer(&self, node: &List, what: &str) -> Result<Option<Layer>> {
        let layer = child(node, "layer", what)?;
        if has_atom(layer, "knockout") {
            return Err(format!("{what}: knockout text is not plotted natively"));
        }
        let name = string(layer, 1).unwrap_or_default();
        Ok(self.layer(&name))
    }
}

// ---------------------------------------------------------------------------
// graphics and text

fn stroke_width(node: &List, what: &str) -> Result<i64> {
    if let Some(stroke) = kicad_mod::first(node, "stroke") {
        if let Some(kind) = kicad_mod::first(stroke, "type").and_then(|kind| kicad_mod::atom(kind, 1))
            && !matches!(kind, "default" | "solid")
        {
            return Err(format!("{what}: {kind} strokes are not plotted natively"));
        }
        return optional_nm(stroke, "width").map(Option::unwrap_or_default);
    }
    optional_nm(node, "width").map(Option::unwrap_or_default)
}

fn filled(node: &List, what: &str) -> Result<bool> {
    match kicad_mod::first(node, "fill").and_then(|fill| kicad_mod::atom(fill, 1)) {
        None | Some("no" | "none") => Ok(false),
        Some("yes" | "solid") => Ok(true),
        Some(other) => Err(format!("{what}: {other} fill is not plotted natively")),
    }
}

/// A `(pts ...)` list of `xy` points and `arc`s as a closed contour.
fn contour(pts: &List, what: &str) -> Result<Contour> {
    let mut start = None;
    let mut edges = Vec::new();
    for item in lists(pts) {
        match tag(item) {
            Some("xy") => {
                let end = point(item, what)?;
                if start.is_none() {
                    start = Some(end);
                } else {
                    edges.push(Edge::Line { end });
                }
            }
            Some("arc") => {
                let arc_start = child_point(item, "start", what)?;
                if start.is_none() {
                    start = Some(arc_start);
                } else if edges.last().map(Edge::end) != Some(arc_start) {
                    edges.push(Edge::Line { end: arc_start });
                }
                edges.push(Edge::Arc { mid: child_point(item, "mid", what)?, end: child_point(item, "end", what)? });
            }
            _ => return Err(format!("{what}: unsupported polygon point")),
        }
    }
    let start = start.ok_or_else(|| format!("{what}: empty polygon"))?;
    if edges.last().map(Edge::end) != Some(start) {
        edges.push(Edge::Line { end: start });
    }
    Ok(Contour { start, edges })
}

/// A graphic in local coordinates, placed by `place`.
fn drawing(node: &List, layer: Layer, place: &dyn Fn(Point) -> Point, what: &str) -> Result<Drawing> {
    let width = stroke_width(node, what)?;
    let kind = tag(node).unwrap_or_default();
    let filled = filled(node, what)?;
    let shape = match kind.trim_start_matches("gr_").trim_start_matches("fp_") {
        "line" => {
            Shape::Line { start: place(child_point(node, "start", what)?), end: place(child_point(node, "end", what)?) }
        }
        "arc" => Shape::Arc {
            start: place(child_point(node, "start", what)?),
            mid: place(child_point(node, "mid", what)?),
            end: place(child_point(node, "end", what)?),
        },
        "circle" => {
            let center = child_point(node, "center", what)?;
            let end = child_point(node, "end", what)?;
            let radius = ((end.x - center.x) as f64).hypot((end.y - center.y) as f64).round() as i64;
            Shape::Circle { center: place(center), radius }
        }
        "rect" => {
            let (a, b) = (child_point(node, "start", what)?, child_point(node, "end", what)?);
            let corners = [a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)].map(place);
            Shape::Polygon(Contour::polygon(&corners))
        }
        "poly" => Shape::Polygon(contour(child(node, "pts", what)?, what)?.transformed(place)),
        _ => return Err(format!("{what}: unsupported graphic {kind}")),
    };
    Ok(Drawing { layer, width, filled, shape })
}

fn text_item(node: &List, text: String, layer: Layer, what: &str) -> Result<Text> {
    let (position, angle) = placement(node, what)?;
    let effects = child(node, "effects", what)?;
    let font = child(effects, "font", what)?;
    if kicad_mod::first(font, "face").is_some() {
        return Err(format!("{what}: TrueType text is not plotted natively"));
    }
    let size = child(font, "size", what)?;
    // KiCad writes (size HEIGHT WIDTH).
    let (height, width) = (number(size, 1, "text height")?, number(size, 2, "text width")?);
    let justify: Vec<&str> = kicad_mod::first(effects, "justify")
        .map(|justify| {
            justify[1..]
                .iter()
                .filter_map(|item| match item {
                    Node::Atom(atom) => Some(atom.as_str()),
                    Node::List(_) => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let h_align = if justify.contains(&"left") {
        HAlign::Left
    } else if justify.contains(&"right") {
        HAlign::Right
    } else {
        HAlign::Center
    };
    let v_align = if justify.contains(&"top") {
        VAlign::Top
    } else if justify.contains(&"bottom") {
        VAlign::Bottom
    } else {
        VAlign::Center
    };
    if text.contains("${") || ["~{", "^{", "_{"].iter().any(|markup| text.contains(markup)) {
        return Err(format!("{what}: text variables and markup are not plotted natively"));
    }
    Ok(Text {
        layer,
        text,
        position,
        angle,
        size: (width, height),
        thickness: optional_nm(font, "thickness")?.unwrap_or(0),
        bold: flag(font, "bold"),
        italic: flag(font, "italic"),
        mirrored: justify.contains(&"mirror"),
        h_align,
        v_align,
        line_spacing: optional_real(font, "line_spacing")?.unwrap_or(1.0),
    })
}

fn hidden(node: &List) -> bool {
    flag(node, "hide") || kicad_mod::first(node, "effects").is_some_and(|effects| flag(effects, "hide"))
}

// ---------------------------------------------------------------------------
// items

const UNSUPPORTED: &[&str] = &[
    "dimension",
    "gr_dimension",
    "image",
    "gr_curve",
    "fp_curve",
    "gr_text_box",
    "fp_text_box",
    "table",
    "target",
    "gr_bbox",
];

fn pad(node: &List, stack: &Stack, footprint_position: Point, footprint_angle: f64, what: &str) -> Result<Pad> {
    let pad_number = string(node, 1).unwrap_or_default();
    let what = format!("{what} pad {pad_number:?}");
    let kind = match kicad_mod::atom(node, 2) {
        Some("smd" | "connect") => PadKind::Smd,
        Some("thru_hole") => PadKind::ThroughHole,
        Some("np_thru_hole") => PadKind::NonPlated,
        other => return Err(format!("{what}: unsupported pad kind {other:?}")),
    };
    let shape = match kicad_mod::atom(node, 3) {
        Some("circle") => PadShape::Circle,
        Some("rect") => PadShape::Rect,
        Some("oval") => PadShape::Oval,
        Some("roundrect") => PadShape::RoundRect(optional_real(node, "roundrect_rratio")?.unwrap_or(0.25)),
        other => return Err(format!("{what}: {} pads are not plotted natively", other.unwrap_or("unnamed"))),
    };
    for unsupported in ["chamfer", "primitives", "padstack", "die_length"] {
        if kicad_mod::first(node, unsupported).is_some() {
            return Err(format!("{what}: ({unsupported}) is not plotted natively"));
        }
    }
    if flag(node, "remove_unused_layers") {
        return Err(format!("{what}: unused-layer removal is not plotted natively"));
    }
    let (local, angle) = placement(node, &what)?;
    let size = child(node, "size", &what)?;
    let drill = match kicad_mod::first(node, "drill") {
        None => None,
        Some(drill) => {
            if kicad_mod::first(drill, "offset").is_some() {
                return Err(format!("{what}: drill offsets are not plotted natively"));
            }
            if kicad_mod::atom(drill, 1) == Some("oval") {
                Some((number(drill, 2, "drill width")?, number(drill, 3, "drill height")?))
            } else {
                let diameter = number(drill, 1, "drill")?;
                Some((
                    diameter,
                    kicad_mod::atom(drill, 2)
                        .map(|_| number(drill, 2, "drill height"))
                        .transpose()?
                        .unwrap_or(diameter),
                ))
            }
        }
    }
    .filter(|&(width, height)| width > 0 && height > 0);
    Ok(Pad {
        number: pad_number,
        kind,
        shape,
        center: footprint_position.plus(rotate(local, footprint_angle)),
        angle,
        width: number(size, 1, "pad width")?,
        height: number(size, 2, "pad height")?,
        drill,
        layers: stack.layer_set(node, &what)?,
        net: net(node, stack),
        mask_margin: optional_nm(node, "solder_mask_margin")?,
        paste_margin: optional_nm(node, "solder_paste_margin")?,
        paste_ratio: match optional_real(node, "solder_paste_margin_ratio")? {
            Some(ratio) => Some(ratio),
            None => optional_real(node, "solder_paste_ratio")?,
        },
    })
}

fn footprint(node: &List, stack: &Stack, board: &mut Board) -> Result<()> {
    let package = string(node, 1).unwrap_or_default();
    let (position, angle) = placement(node, &format!("footprint {package:?}"))?;
    let side = match kicad_mod::first(node, "layer").and_then(|layer| string(layer, 1)).as_deref() {
        Some("B.Cu") => Side::Back,
        _ => Side::Front,
    };
    let property = |name: &str| -> Option<String> {
        lists(node).find_map(|item| match tag(item) {
            Some("property") if string(item, 1).as_deref() == Some(name) => string(item, 2),
            Some("fp_text") if kicad_mod::atom(item, 1) == Some(&name.to_lowercase()) => string(item, 2),
            _ => None,
        })
    };
    let reference = property("Reference").unwrap_or_default();
    let what = format!("footprint {reference}");
    let attr = kicad_mod::first(node, "attr");
    let place = |local: Point| position.plus(rotate(local, angle));
    let mut pads = Vec::new();
    for item in lists(node) {
        let item_tag = tag(item).unwrap_or_default();
        match item_tag {
            "pad" => pads.push(pad(item, stack, position, angle, &what)?),
            "fp_line" | "fp_arc" | "fp_circle" | "fp_rect" | "fp_poly" => {
                if let Some(layer) = stack.graphic_layer(item, &what)? {
                    board.drawings.push(drawing(item, layer, &place, &what)?);
                }
            }
            "property" | "fp_text" => {
                if !hidden(item)
                    && kicad_mod::first(item, "layer").is_some()
                    && stack.graphic_layer(item, &what)?.is_some()
                {
                    return Err(format!("{what}: visible footprint text on a plotted layer is not plotted natively"));
                }
            }
            "zone" => {
                if !kicad_mod::children(item, "filled_polygon").is_empty() {
                    return Err(format!("{what}: filled footprint zones are not plotted natively"));
                }
            }
            name if UNSUPPORTED.contains(&name) => {
                return Err(format!("{what}: {name} is not plotted natively"));
            }
            _ => {}
        }
    }
    board.footprints.push(Footprint {
        reference,
        value: property("Value").unwrap_or_default(),
        package: package.rsplit_once(':').map_or(package.clone(), |(_, name)| name.to_owned()),
        position,
        angle,
        side,
        exclude_from_pos_files: attr.is_some_and(|attr| has_atom(attr, "exclude_from_pos_files")),
        dnp: attr.is_some_and(|attr| has_atom(attr, "dnp")) || flag(node, "dnp"),
        mask_margin: optional_nm(node, "solder_mask_margin")?,
        paste_margin: optional_nm(node, "solder_paste_margin")?,
        paste_ratio: match optional_real(node, "solder_paste_margin_ratio")? {
            Some(ratio) => Some(ratio),
            None => optional_real(node, "solder_paste_ratio")?,
        },
        pads,
    });
    Ok(())
}

fn tenting(node: &List) -> [Option<bool>; 2] {
    let Some(tenting) = kicad_mod::first(node, "tenting") else {
        return [None, None];
    };
    // KiCad 8: (tenting front back) lists the tented sides; KiCad 9+:
    // (tenting (front yes) (back no)).
    let side = |name: &str| {
        if has_atom(tenting, name) {
            return Some(true);
        }
        kicad_mod::first(tenting, name).map(|child| kicad_mod::atom(child, 1) == Some("yes"))
    };
    let (front, back) = (side("front"), side("back"));
    if front.is_none() && back.is_none() && tenting.len() == 1 {
        return [Some(false), Some(false)];
    }
    if tenting.iter().skip(1).all(|item| matches!(item, Node::Atom(_))) {
        return [Some(front.unwrap_or(false)), Some(back.unwrap_or(false))];
    }
    [front, back]
}

fn via(node: &List, stack: &Stack) -> Result<Via> {
    let center = child_point(node, "at", "via")?;
    let what = format!("via at ({}, {}) nm", center.x, center.y);
    if has_atom(node, "blind") || has_atom(node, "micro") || has_atom(node, "buried") {
        return Err(format!("{what}: blind, buried and micro vias are not plotted natively"));
    }
    let layers = stack.layer_set(node, &what)?;
    if !layers.copper.first().copied().unwrap_or(false) || !layers.copper.last().copied().unwrap_or(false) {
        return Err(format!("{what}: only through vias are plotted natively"));
    }
    if kicad_mod::first(node, "padstack").is_some() || flag(node, "remove_unused_layers") {
        return Err(format!("{what}: via pad stacks and unused-layer removal are not plotted natively"));
    }
    Ok(Via {
        center,
        size: number(child(node, "size", &what)?, 1, "via size")?,
        drill: number(child(node, "drill", &what)?, 1, "via drill")?,
        net: net(node, stack),
        tented: tenting(node),
    })
}

fn track(node: &List, stack: &Stack, arc: bool) -> Result<Option<Track>> {
    let what = if arc { "arc track" } else { "track segment" };
    let layer_name = string(child(node, "layer", what)?, 1).unwrap_or_default();
    let Some(Layer::Copper(layer)) = stack.layer(&layer_name) else {
        return Err(format!("{what} on unknown copper layer {layer_name:?}"));
    };
    let start = child_point(node, "start", what)?;
    let end = child_point(node, "end", what)?;
    let path = if arc {
        TrackPath::Arc { start, mid: child_point(node, "mid", what)?, end }
    } else {
        TrackPath::Line { start, end }
    };
    Ok(Some(Track {
        layer,
        width: number(child(node, "width", what)?, 1, "track width")?,
        net: net(node, stack),
        path,
    }))
}

fn zone(node: &List, stack: &Stack, board: &mut Board) -> Result<()> {
    let net = net(node, stack).or_else(|| {
        kicad_mod::first(node, "net_name").and_then(|name| string(name, 1)).filter(|name| !name.is_empty())
    });
    let fills = kicad_mod::children(node, "filled_polygon");
    if fills.is_empty() {
        return Ok(());
    }
    if let Some(fill) = kicad_mod::first(node, "fill")
        && kicad_mod::first(fill, "mode").and_then(|mode| kicad_mod::atom(mode, 1)) == Some("hatch")
    {
        return Err("hatched zone fills are not plotted natively".to_owned());
    }
    let default_layer = kicad_mod::first(node, "layer").and_then(|layer| string(layer, 1));
    for fill in fills {
        let name = kicad_mod::first(fill, "layer")
            .and_then(|layer| string(layer, 1))
            .or_else(|| default_layer.clone())
            .unwrap_or_default();
        let Some(layer) = stack.layer(&name) else { continue };
        board.fills.push(Fill {
            layer,
            net: net.clone(),
            contour: contour(child(fill, "pts", "zone fill")?, "zone fill")?,
        });
    }
    Ok(())
}

fn setup(root: &List) -> Result<Setup> {
    let Some(setup) = kicad_mod::first(root, "setup") else {
        return Ok(Setup { mask_margin: 0, paste_margin: 0, paste_ratio: 0.0, tented: [true, true] });
    };
    if optional_nm(setup, "solder_mask_min_width")?.unwrap_or(0) != 0 {
        return Err("a nonzero solder_mask_min_width merges mask openings, which is not plotted natively".to_owned());
    }
    let tented = tenting(setup);
    Ok(Setup {
        mask_margin: optional_nm(setup, "pad_to_mask_clearance")?.unwrap_or(0),
        paste_margin: optional_nm(setup, "pad_to_paste_clearance")?.unwrap_or(0),
        paste_ratio: optional_real(setup, "pad_to_paste_clearance_ratio")?.unwrap_or(0.0),
        tented: [tented[0].unwrap_or(true), tented[1].unwrap_or(true)],
    })
}

/// Parse a `.kicad_pcb` text.
pub fn parse_board(text: &str, source: &str) -> Result<Board> {
    let root = kicad_mod::parse_sexpr(text, source).map_err(|failure| failure.to_string())?;
    if tag(&root) != Some("kicad_pcb") {
        return Err(format!("{source}: expected a .kicad_pcb board"));
    }
    let layers = child(&root, "layers", "board")?;
    let mut numbered: Vec<(String, i64)> = Vec::new();
    for item in lists(layers) {
        let name = string(item, 1).unwrap_or_default();
        if name.ends_with(".Cu") {
            numbered.push((name, kicad_mod::atom(item, 0).and_then(|id| id.parse().ok()).unwrap_or(0)));
        }
    }
    // Stack order: F.Cu, In1.Cu ... InN.Cu, B.Cu.
    let rank = |name: &str| match name {
        "F.Cu" => 0,
        "B.Cu" => usize::MAX,
        inner => inner
            .strip_prefix("In")
            .and_then(|rest| rest.strip_suffix(".Cu"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(usize::MAX - 1),
    };
    numbered.sort_by_key(|(name, _)| rank(name));
    let table: indexmap::IndexMap<String, String> = kicad_mod::children(&root, "net")
        .into_iter()
        .filter_map(|net| Some((string(net, 1)?, string(net, 2).unwrap_or_default())))
        .collect();
    let stack = Stack {
        copper: numbered.into_iter().map(|(name, _)| name).collect(),
        nets: (!table.is_empty()).then_some(table),
    };
    if stack.copper.first().map(String::as_str) != Some("F.Cu")
        || stack.copper.last().map(String::as_str) != Some("B.Cu")
    {
        return Err(format!("{source}: the board needs F.Cu and B.Cu"));
    }
    let mut board = Board {
        copper_layers: stack.copper.clone(),
        setup: setup(&root).map_err(|failure| format!("{source}: {failure}"))?,
        footprints: Vec::new(),
        tracks: Vec::new(),
        vias: Vec::new(),
        drawings: Vec::new(),
        texts: Vec::new(),
        fills: Vec::new(),
    };
    let identity = |point: Point| point;
    let parsed: Result<()> = (|| {
        for item in lists(&root) {
            let item_tag = tag(item).unwrap_or_default();
            match item_tag {
                "footprint" | "module" => footprint(item, &stack, &mut board)?,
                "segment" => board.tracks.extend(track(item, &stack, false)?),
                "arc" => board.tracks.extend(track(item, &stack, true)?),
                "via" => board.vias.push(via(item, &stack)?),
                "zone" => zone(item, &stack, &mut board)?,
                "gr_line" | "gr_arc" | "gr_circle" | "gr_rect" | "gr_poly" => {
                    if let Some(layer) = stack.graphic_layer(item, item_tag)? {
                        board.drawings.push(drawing(item, layer, &identity, item_tag)?);
                    }
                }
                "gr_text" => {
                    if let Some(layer) = stack.graphic_layer(item, "gr_text")?
                        && !hidden(item)
                    {
                        let content = string(item, 1).unwrap_or_default();
                        board.texts.push(text_item(item, content, layer, "gr_text")?);
                    }
                }
                name if UNSUPPORTED.contains(&name) => {
                    let on_plotted = kicad_mod::first(item, "layer")
                        .and_then(|layer| string(layer, 1))
                        .is_none_or(|layer| stack.layer(&layer).is_some());
                    if on_plotted {
                        return Err(format!("{name} is not plotted natively"));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })();
    parsed.map_err(|failure| format!("{source}: {failure}"))?;
    Ok(board)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetres_parse_to_exact_nanometres() {
        assert_eq!(parse_nm("1.5"), Some(1_500_000));
        assert_eq!(parse_nm("-0.000001"), Some(-1));
        assert_eq!(parse_nm("0.0000005"), Some(1));
        assert_eq!(parse_nm("-12"), Some(-12_000_000));
        assert_eq!(parse_nm(".25"), Some(250_000));
        assert_eq!(parse_nm("1e3"), None);
        assert_eq!(parse_nm(""), None);
    }

    const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user "F.Silkscreen") (44 "Edge.Cuts" user))
  (setup (pad_to_mask_clearance 0.05) (tenting front back))
  (net 0 "") (net 1 "GND")
  (footprint "Lib:R" (layer "B.Cu") (at 10 20 90)
    (property "Reference" "R1" (at 0 0 0) (layer "B.Fab") (effects (font (size 1 1))))
    (property "Value" "1k" (at 0 0 0) (layer "B.Fab") (hide yes) (effects (font (size 1 1))))
    (attr smd)
    (fp_line (start -1 0) (end 1 0) (stroke (width 0.12) (type solid)) (layer "B.SilkS"))
    (pad "1" smd roundrect (at -1 0 90) (size 1 0.5) (layers "B.Cu" "B.Paste" "B.Mask") (roundrect_rratio 0.25) (net 1 "GND")))
  (gr_rect (start 0 0) (end 30 40) (stroke (width 0.05) (type default)) (fill no) (layer "Edge.Cuts"))
  (gr_text "Rev A" (at 15 5 0) (layer "F.SilkS") (effects (font (size 1 1.5) (thickness 0.15)) (justify left)))
  (segment (start 1 1) (end 2 2) (width 0.2) (layer "F.Cu") (net 1))
  (via (at 3 3) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1 "GND")))"#;

    #[test]
    fn board_items_are_placed_in_board_coordinates() {
        let board = parse_board(BOARD, "test.kicad_pcb").unwrap();
        assert_eq!(board.copper_layers, ["F.Cu", "B.Cu"]);
        assert_eq!(board.setup.mask_margin, 50_000);
        assert_eq!(board.setup.tented, [true, true]);
        let footprint = &board.footprints[0];
        assert_eq!(
            (footprint.reference.as_str(), footprint.value.as_str(), footprint.package.as_str()),
            ("R1", "1k", "R")
        );
        assert_eq!(footprint.side, Side::Back);
        let pad = &footprint.pads[0];
        // Local (-1, 0) rotated 90 degrees: (0, 1) from (10, 20).
        assert_eq!(pad.center, Point::new(10_000_000, 21_000_000));
        assert_eq!(pad.net.as_deref(), Some("GND"));
        assert!(
            pad.layers.has(Layer::Copper(1))
                && pad.layers.has(Layer::Paste(Side::Back))
                && !pad.layers.has(Layer::Copper(0))
        );
        assert_eq!(board.drawings.len(), 2);
        assert_eq!(board.drawings[0].layer, Layer::Silk(Side::Back));
        assert_eq!(board.texts[0].size, (1_500_000, 1_000_000));
        assert_eq!(board.texts[0].h_align, HAlign::Left);
        assert_eq!(board.tracks[0].net.as_deref(), Some("GND"));
        assert_eq!(board.vias[0].drill, 300_000);
    }

    #[test]
    fn unsupported_constructs_fail_closed() {
        let custom = BOARD.replace("smd roundrect", "smd custom");
        assert!(parse_board(&custom, "x").unwrap_err().contains("custom pads"));
        let dashed = BOARD.replace("(type solid)", "(type dash)");
        assert!(parse_board(&dashed, "x").unwrap_err().contains("dash strokes"));
        let visible = BOARD.replace("(layer \"B.Fab\") (effects", "(layer \"B.SilkS\") (effects");
        assert!(parse_board(&visible, "x").unwrap_err().contains("visible footprint text"));
    }
}

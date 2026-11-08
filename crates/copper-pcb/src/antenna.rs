//! `constraint antenna(COMPONENT.PIN) { ... }`: a synthesized trace antenna.
//!
//! The declaration resolves to an [`AntennaSite`]: the board-edge region, its
//! local frame, the feed pad and its pose, the track rules, the stack-up and
//! the bands. A [`Geometry`] (from the source's `geometry`, or the first-order
//! template when absent) realizes a [`Layout`] in the site's local frame,
//! which is lowered to a generated one-member hard macro in the
//! `copperlib-physical-hard-macro/v0.5` contract and bound through the same
//! immutable binder as a library macro: the feed component is the anchor, the
//! radiator is locked owner copper, the antenna region is a private region on
//! every copper layer with a zone-blocking keepout opened to the board
//! perimeter, and the feed pad is the macro's one external port. Other
//! components are kept out of the region by a placement keepout.
//!
//! Geometry tuned by `copper synthesize-antenna` (the full-wave solver in the
//! `copper-antenna` crate) carries `tuned_for`, the [`site_digest`] of every
//! input that model used; a changed input fails closed. An inverted-F ties its
//! arm to ground through `short`, a two-pad link component (a 0 ohm link or a
//! tuning inductor) that becomes the macro's second member: a feed-net leg
//! reaches its first pad, and a ground-net stub and through via at its second
//! pad return to the ground plane. Loops stay unimplemented. Nothing here
//! establishes resonance, matching, radiation or regulatory compliance; the
//! generated macro says so in `unresolved`. See
//! `docs/pcb-antenna-synthesis-research.md`.

use std::path::Path;

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use copper_json::{Dumps, Object, Value as Json, python_str_repr as repr};
use copper_lang::antenna::{Band, parse_bands};
use copper_lang::model::{Constraint, ConstraintKind, FlatElectricalView, ParameterValue};
use copper_physical::footprint::{FootprintPad, footprint_geometry_digest};
use copper_physical::geometry::{Bounds, bounds, segment_in_polygon};
use copper_physical::{BoardOutline, BoardSide, CopperLayer, PlacementKeepout, Point};
use copper_units::Decimal;

use crate::board::*;
use crate::hard_macros::{bind_hard_macro, ring_covers_segment};
use crate::physicalize::physical_pin_number;
use crate::transform::transform_local;
use crate::zones::{length_nm, polygon_boundary};
use crate::{Error, Result, fail};

/// The generator identity recorded in every synthesized macro's `source` and
/// in every [`site_digest`]: a generator change invalidates tuned geometry.
pub const GENERATOR: &str = "copper antenna/v0.2";
/// The revision of the full-wave model behind [`site_digest`]: its analysis
/// sweep, excitation, modelled ground stitching and mesh rules. Bump it with
/// every change to what the model simulates for given inputs, so geometry
/// tuned under an earlier model reads as stale and must be resynthesized.
/// Revision 2: the sweep covers every band and stitching follows its top.
/// Revision 3: the model chooses every mesh line itself (no lines detected
/// from primitives, finer cells around the feed and an inverted-F's short,
/// stitching vias as wires on mesh lines) and keeps an eighth of a
/// wavelength of air before the PML.
/// Revision 4: no cell, the feed and short zones' included, is coarser than
/// the frequency limit (a fifteenth of the guided wavelength at the sweep's
/// top), which bands above about 12 GHz on FR4 reached.
pub const MODEL_REVISION: i64 = 4;
/// The `members[].reference` and `net` role names inside the generated asset.
const MEMBER: &str = "ANT";
const SHORT_MEMBER: &str = "SHORT";
const FEED_ROLE: &str = "feed";
const GROUND_ROLE: &str = "ground";
pub const SPEED_OF_LIGHT_M_PER_S: f64 = 299_792_458.0;
/// Synthesized lengths are rounded to this grid (10 µm).
pub const LENGTH_GRID_NM: i64 = 10_000;
/// The macro member's edge margin, as `macro` definitions default it.
const MEMBER_EDGE_CLEARANCE_NM: i64 = 250_000;

/// Which board axis the radiator runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// The region's frame: the radiator runs along `axis` from the region's low
/// coordinate; `inner` is the across-axis coordinate of the ground-side edge
/// and `outward` the across-axis direction toward the board perimeter.
///
/// Layouts use local coordinates `(u, v)`: `u` along the board edge from
/// `along_min`, `v` across from the ground edge toward the perimeter, so the
/// region is `0 <= u <= length()`, `0 <= v <= depth()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub axis: Axis,
    pub along_min: i64,
    pub along_max: i64,
    pub inner: i64,
    pub outward: i64,
    pub across_min: i64,
    pub across_max: i64,
}

impl Frame {
    fn point(&self, along: i64, across: i64) -> Point {
        match self.axis {
            Axis::X => Point::new(along, across),
            Axis::Y => Point::new(across, along),
        }
    }

    fn along(&self, point: Point) -> i64 {
        match self.axis {
            Axis::X => point.x_nm,
            Axis::Y => point.y_nm,
        }
    }

    fn across(&self, point: Point) -> i64 {
        match self.axis {
            Axis::X => point.y_nm,
            Axis::Y => point.x_nm,
        }
    }

    /// The board point of local `(u, v)`.
    pub fn to_board(&self, u: i64, v: i64) -> Point {
        self.point(self.along_min + u, self.inner + self.outward * v)
    }

    /// The region's extent along the board edge.
    pub fn length(&self) -> i64 {
        self.along_max - self.along_min
    }

    /// The region's extent from the ground edge to the perimeter.
    pub fn depth(&self) -> i64 {
        self.across_max - self.across_min
    }
}

/// The radiator family a declaration asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    Auto,
    Monopole,
    Meander,
    InvertedF,
    Pixel,
}

impl Topology {
    pub fn value(self) -> &'static str {
        match self {
            Topology::Auto => "auto",
            Topology::Monopole => "monopole",
            Topology::Meander => "meander",
            Topology::InvertedF => "inverted_f",
            Topology::Pixel => "pixel",
        }
    }
}

/// A concrete radiator in the site's local frame. Lengths in nanometres.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Geometry {
    /// A stub from the feed to `height`, then a run along the board edge;
    /// `length` is the conductor length from the ground edge.
    Monopole { height: i64, length: i64 },
    /// A stub to `height`, `turns` U-shaped meanders hanging `depth` below
    /// it with `pitch` between adjacent legs (centre to centre), then a
    /// straight `tail` along the board edge.
    Meander { height: i64, depth: i64, pitch: i64, turns: u32, tail: i64 },
    /// An inverted-F: the arm at `height` is `length` long from the short
    /// leg to its open end; the feed stub meets it `short` along from the
    /// leg, which runs down to the link component tying it to ground.
    InvertedF { height: i64, length: i64, short: i64 },
    /// A `columns` x `rows` grid of square `cell` tiles over the region,
    /// row-major from the ground edge, with one column centred on the feed;
    /// only tiles 4-connected to the feed tile are realized.
    Pixel { cell: i64, columns: u32, rows: u32, bits: Vec<bool> },
}

fn mm_text(nm: i64) -> String {
    let value = Decimal::from_i64(nm).div(&Decimal::from_i64(1_000_000)).expect("finite nanometres");
    format!("{}mm", value.to_fixed_string())
}

fn mm(nm: i64) -> String {
    let value = Decimal::from_i64(nm).div(&Decimal::from_i64(1_000_000)).expect("finite nanometres");
    format!("{} mm", value.to_fixed_string())
}

fn parse_length(text: &str) -> std::result::Result<i64, String> {
    let number = text.strip_suffix("mm").ok_or_else(|| format!("{} must be a length in mm", repr(text)))?;
    let value = Decimal::parse(number).map_err(|_| format!("{} is not a length", repr(text)))?;
    let nm = value.mul(&Decimal::from_i64(1_000_000)).map_err(|e| e.to_string())?;
    let integral = nm.to_integral_value(copper_units::Rounding::Down);
    if integral != nm {
        return Err(format!("{} is finer than a nanometre", repr(text)));
    }
    integral.to_i64().filter(|nm| *nm >= 0).ok_or_else(|| format!("{} is out of range", repr(text)))
}

impl Geometry {
    pub fn topology(&self) -> Topology {
        match self {
            Geometry::Monopole { .. } => Topology::Monopole,
            Geometry::Meander { .. } => Topology::Meander,
            Geometry::InvertedF { .. } => Topology::InvertedF,
            Geometry::Pixel { .. } => Topology::Pixel,
        }
    }

    /// The source form: `"monopole height=3mm length=17.46mm"`.
    pub fn to_source(&self) -> String {
        match self {
            Geometry::Monopole { height, length } => {
                format!("monopole height={} length={}", mm_text(*height), mm_text(*length))
            }
            Geometry::Meander { height, depth, pitch, turns, tail } => format!(
                "meander height={} depth={} pitch={} turns={turns} tail={}",
                mm_text(*height),
                mm_text(*depth),
                mm_text(*pitch),
                mm_text(*tail)
            ),
            Geometry::InvertedF { height, length, short } => {
                format!("inverted_f height={} length={} short={}", mm_text(*height), mm_text(*length), mm_text(*short))
            }
            Geometry::Pixel { cell, columns, rows, bits } => {
                let mut hex = String::new();
                for chunk in bits.chunks(4) {
                    let nibble = chunk.iter().enumerate().fold(0u8, |acc, (i, bit)| acc | (u8::from(*bit) << (3 - i)));
                    hex.push(char::from_digit(u32::from(nibble), 16).expect("a nibble"));
                }
                format!("pixel cell={} columns={columns} rows={rows} bits={hex}", mm_text(*cell))
            }
        }
    }

    /// Parse [`to_source`](Self::to_source)'s form.
    pub fn parse(text: &str) -> std::result::Result<Geometry, String> {
        let mut words = text.split_whitespace();
        let kind = words.next().ok_or("an empty antenna geometry")?;
        let mut fields: IndexMap<&str, &str> = IndexMap::new();
        for word in words {
            let Some((key, value)) = word.split_once('=') else {
                return Err(format!("antenna geometry field {} must be key=value", repr(word)));
            };
            if fields.insert(key, value).is_some() {
                return Err(format!("duplicate antenna geometry field {}", repr(key)));
            }
        }
        let expected: &[&str] = match kind {
            "monopole" => &["height", "length"],
            "meander" => &["height", "depth", "pitch", "turns", "tail"],
            "inverted_f" => &["height", "length", "short"],
            "pixel" => &["cell", "columns", "rows", "bits"],
            "loop" => return Err("loop geometry is not implemented".to_owned()),
            other => return Err(format!("unknown antenna geometry {}", repr(other))),
        };
        let mut keys: Vec<&str> = fields.keys().copied().collect();
        keys.sort_unstable();
        let mut wanted = expected.to_vec();
        wanted.sort_unstable();
        if keys != wanted {
            return Err(format!("{kind} geometry requires exactly {}", expected.join(", ")));
        }
        let length = |key: &str| parse_length(fields[key]);
        let count = |key: &str| -> std::result::Result<u32, String> {
            fields[key].parse::<u32>().map_err(|_| format!("{key} must be a nonnegative integer"))
        };
        Ok(match kind {
            "monopole" => Geometry::Monopole { height: length("height")?, length: length("length")? },
            "meander" => Geometry::Meander {
                height: length("height")?,
                depth: length("depth")?,
                pitch: length("pitch")?,
                turns: count("turns")?,
                tail: length("tail")?,
            },
            "inverted_f" => {
                Geometry::InvertedF { height: length("height")?, length: length("length")?, short: length("short")? }
            }
            _ => {
                let (columns, rows) = (count("columns")?, count("rows")?);
                let total = columns as usize * rows as usize;
                let hex = fields["bits"];
                if hex.len() != total.div_ceil(4) {
                    return Err(format!("pixel bits need {} hex digits for {columns} x {rows}", total.div_ceil(4)));
                }
                let mut bits = Vec::with_capacity(total);
                for c in hex.chars() {
                    let nibble =
                        c.to_digit(16).ok_or_else(|| format!("{} is not a hex digit", repr(&c.to_string())))?;
                    bits.extend((0..4).rev().map(|i| nibble >> i & 1 == 1));
                }
                if bits[total..].iter().any(|bit| *bit) {
                    return Err("pixel bits set past the last tile".to_owned());
                }
                bits.truncate(total);
                Geometry::Pixel { cell: length("cell")?, columns, rows, bits }
            }
        })
    }
}

/// Realized copper in the site's local frame: the feed track's centreline
/// from the feed pad centre, and any filled tiles `[u0, v0, u1, v1]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub path: Vec<(i64, i64)>,
    pub tiles: Vec<[i64; 4]>,
    /// Centreline length of the track from the ground edge (v = 0) on.
    pub length_nm: i64,
    /// The inverted-F short, when the geometry has one.
    pub short: Option<ShortLayout>,
}

/// The realized short of an inverted-F, in the site's local frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortLayout {
    /// The feed-net leg from the arm down to the link's feed pad centre.
    pub leg: Vec<(i64, i64)>,
    /// The link member's centre.
    pub centre: (i64, i64),
    pub feed_pad: (i64, i64),
    pub ground_pad: (i64, i64),
    /// The ground via's centre, beyond the ground pad.
    pub via: (i64, i64),
}

/// The two-pad link that ties an inverted-F arm to ground.
#[derive(Clone, Debug)]
pub struct ShortLink {
    pub reference: String,
    /// The footprint key in `PhysicalBoard.footprints`.
    pub footprint: String,
    pub feed_pad: FootprintPad,
    pub ground_pad: FootprintPad,
    /// The member rotation that puts the feed pad toward the region.
    pub rotation: i64,
    /// Pad centre offsets from the member centre, rotated, as `(along, across)`.
    pub feed_offset: (i64, i64),
    pub ground_offset: (i64, i64),
    /// The feed pad centre's `v` (negative: outside the region on the ground side).
    pub standoff: i64,
    /// The ground via centre's `v`.
    pub via_v: i64,
    pub via_size: i64,
    pub via_drill: i64,
    /// The last copper layer, which the via reaches.
    pub via_to: CopperLayer,
    /// Feed-to-leg spacing limits: the bodies must clear, the legs the clearance.
    pub short_min: i64,
    pub short_max: i64,
}

/// Everything a declaration fixes about one antenna, resolved on the board.
#[derive(Clone, Debug)]
pub struct AntennaSite {
    pub name: String,
    /// The declaration's source location.
    pub declaration: String,
    pub feed: PadReference,
    pub feed_net: String,
    pub feed_land: FootprintPad,
    /// The feed component's footprint key in `PhysicalBoard.footprints`.
    pub footprint: String,
    pub isolated: Vec<String>,
    pub ground: String,
    pub bands: Vec<Band>,
    pub topology: Topology,
    pub max_return_loss_db: f64,
    pub area: Bounds,
    pub frame: Frame,
    pub perimeter: Vec<usize>,
    pub width: i64,
    pub clearance: i64,
    pub keepout_margin: i64,
    /// The feed component's fixed pose (rotation 0, front side).
    pub pose: Point,
    /// The feed pad centre in local coordinates (`feed_v < 0`).
    pub feed_u: i64,
    pub feed_v: i64,
    pub eps_eff: f64,
    pub dielectric: String,
    pub geometry: Option<Geometry>,
    pub tuned_for: Option<String>,
    pub short: Option<ShortLink>,
}

impl AntennaSite {
    /// The copper margin to the region edges: clearance plus half a track.
    pub fn margin(&self) -> i64 {
        self.clearance + self.width / 2
    }

    /// The arithmetic centre of every band, in hertz.
    pub fn centre_hz(&self) -> f64 {
        let low = self.bands.iter().map(|b| decimal_f64(&b.low_hz)).fold(f64::INFINITY, f64::min);
        let high = self.bands.iter().map(|b| decimal_f64(&b.high_hz)).fold(f64::NEG_INFINITY, f64::max);
        (low + high) / 2.0
    }
}

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn point_json(point: Point) -> Json {
    Json::Array(vec![Json::from(point.x_nm), Json::from(point.y_nm)])
}

fn texts(items: &[&str]) -> Json {
    Json::Array(items.iter().map(|item| Json::from(*item)).collect())
}

pub fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("a decimal string")
}

fn text<'a>(parameters: &'a IndexMap<String, ParameterValue>, name: &str) -> Option<&'a str> {
    match parameters.get(name) {
        Some(ParameterValue::Text(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn length(parameters: &IndexMap<String, ParameterValue>, name: &str) -> Result<Option<i64>> {
    match parameters.get(name) {
        None => Ok(None),
        Some(value) => length_nm(value)
            .map(Some)
            .ok_or_else(|| Error::Value(format!("antenna parameter {} must be a length", repr(name)))),
    }
}

/// The rectangular region of an antenna declaration: `region` (a named
/// placement region), `polygon_mm`, or `x`/`y`/`width`/`height`.
fn region_bounds(board: &PhysicalBoard, parameters: &IndexMap<String, ParameterValue>) -> Result<Bounds> {
    let outline = if let Some(name) = text(parameters, "region") {
        let Some(region) = board.regions.iter().find(|region| region.name == name) else {
            return Err(Error::Value(format!("antenna references unknown placement region {}", repr(name))));
        };
        region.outline.clone()
    } else if let Some(polygon) = parameters.get("polygon_mm") {
        polygon_boundary(polygon, "antenna")?
    } else {
        let value = |name: &str| {
            length(parameters, name)?.ok_or_else(|| Error::Value(format!("antenna rectangle requires {}", repr(name))))
        };
        let origin = Point::new(value("x")?, value("y")?);
        BoardOutline::rectangle(value("width")?, value("height")?, Some(origin))?
    };
    if outline.circular_boundary.is_some() || outline.boundary_path.is_some() || !outline.cutouts.is_empty() {
        return crate::not_implemented("a curved or holed antenna region");
    }
    let vertices = &outline.vertices;
    let area = bounds(vertices);
    let rectangular = vertices.len() == 4
        && vertices
            .iter()
            .all(|v| (v.x_nm == area.min_x || v.x_nm == area.max_x) && (v.y_nm == area.min_y || v.y_nm == area.max_y))
        && vertices.windows(2).all(|pair| pair[0].x_nm == pair[1].x_nm || pair[0].y_nm == pair[1].y_nm);
    if !rectangular {
        return crate::not_implemented("a non-rectangular antenna region");
    }
    Ok(area)
}

/// The region's four corners, in the order the generated keepout uses:
/// edge 0 is the low-y side, then high-x, high-y and low-x.
fn corners(area: Bounds) -> [Point; 4] {
    [
        Point::new(area.min_x, area.min_y),
        Point::new(area.max_x, area.min_y),
        Point::new(area.max_x, area.max_y),
        Point::new(area.min_x, area.max_y),
    ]
}

/// Which region edges (by `corners` index) lie on the outer board perimeter.
fn perimeter_edges(host: &[Point], area: Bounds) -> Vec<usize> {
    let ring = corners(area);
    (0..4).filter(|index| ring_covers_segment(host, ring[*index], ring[(index + 1) % 4])).collect()
}

/// The radiator frame: the board edge is on one side of the run, the host
/// ground on the opposite side. Prefer the longer run when both axes qualify.
fn frame(area: Bounds, perimeter: &[usize]) -> Result<Frame> {
    let on = |edge: usize| perimeter.contains(&edge);
    let mut candidates = Vec::new();
    // Edges 0 (y = min_y) and 2 (y = max_y) bound a run along x.
    if on(0) != on(2) {
        let (perimeter_y, inner) = if on(0) { (area.min_y, area.max_y) } else { (area.max_y, area.min_y) };
        candidates.push(Frame {
            axis: Axis::X,
            along_min: area.min_x,
            along_max: area.max_x,
            inner,
            outward: (perimeter_y - inner).signum(),
            across_min: area.min_y,
            across_max: area.max_y,
        });
    }
    // Edges 1 (x = max_x) and 3 (x = min_x) bound a run along y.
    if on(1) != on(3) {
        let (perimeter_x, inner) = if on(3) { (area.min_x, area.max_x) } else { (area.max_x, area.min_x) };
        candidates.push(Frame {
            axis: Axis::Y,
            along_min: area.min_y,
            along_max: area.max_y,
            inner,
            outward: (perimeter_x - inner).signum(),
            across_min: area.min_x,
            across_max: area.max_x,
        });
    }
    candidates.into_iter().max_by_key(|frame| frame.along_max - frame.along_min).ok_or_else(|| {
        Error::Value(
            "antenna region must lie on the board edge: one edge of the rectangle on the outer board perimeter \
             with the opposite edge facing the host ground"
                .to_owned(),
        )
    })
}

/// The Hammerstad-Jensen effective permittivity of a `width_nm` trace on the
/// front copper over the stack-up's first dielectric (air above).
fn effective_permittivity(board: &PhysicalBoard, width_nm: i64) -> Result<(f64, String)> {
    let physical = &board.stackup.physical_layers;
    let Some(index) = physical.iter().position(|layer| layer.copper_layer == Some(CopperLayer::Front)) else {
        return fail("antenna synthesis requires a mechanical stackup declaring F.Cu and the dielectric under it");
    };
    let Some(dielectric) = physical.get(index + 1).filter(|layer| layer.copper_layer.is_none()) else {
        return fail("antenna synthesis requires a dielectric layer directly under F.Cu in the stackup");
    };
    let Some(er) = &dielectric.relative_permittivity else {
        return Err(Error::Value(format!(
            "antenna synthesis requires `er` on stackup dielectric {}",
            repr(&dielectric.id)
        )));
    };
    if dielectric.thickness_nm <= 0 {
        return fail("antenna synthesis requires a positive dielectric thickness under F.Cu");
    }
    let er = decimal_f64(er);
    let ratio = (12 * dielectric.thickness_nm) as f64 / width_nm as f64;
    Ok(((er + 1.0) / 2.0 + (er - 1.0) / 2.0 / (1.0 + ratio).sqrt(), dielectric.id.clone()))
}

/// A free-space quarter wave at `hertz`, shortened by `sqrt(eps_eff)`, on the length grid.
pub fn quarter_wave_nm(hertz: f64, eps_eff: f64) -> i64 {
    let metres = SPEED_OF_LIGHT_M_PER_S / (4.0 * hertz * eps_eff.sqrt());
    snap((metres * 1e9) as i64)
}

/// Round to the 10 µm synthesis grid.
pub fn snap(nm: i64) -> i64 {
    ((nm as f64 / LENGTH_GRID_NM as f64).round() as i64) * LENGTH_GRID_NM
}

fn format_hz(hertz: f64) -> String {
    if hertz >= 1e9 { format!("{:.4} GHz", hertz / 1e9) } else { format!("{:.3} MHz", hertz / 1e6) }
}

/// Resolve every `constraint antenna` of `flat` on `board`, in source order.
/// The board must not yet carry the antennas' macros.
pub fn resolve_sites(board: &PhysicalBoard, flat: &FlatElectricalView) -> Result<Vec<AntennaSite>> {
    flat.constraints
        .iter()
        .enumerate()
        .filter(|(_, constraint)| constraint.kind == ConstraintKind::Antenna)
        .map(|(index, constraint)| resolve_site(board, flat, index, constraint))
        .collect()
}

fn resolve_site(
    board: &PhysicalBoard,
    flat: &FlatElectricalView,
    index: usize,
    constraint: &Constraint,
) -> Result<AntennaSite> {
    let parameters = &constraint.parameters;
    let name = text(parameters, "name").map_or_else(|| format!("antenna:{index}"), str::to_owned);
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(&name)));
    let wrap = |error: Error| match error {
        Error::Value(message) => problem(message),
        other => other,
    };
    if board.rigid_clusters.iter().any(|cluster| cluster.name == name) {
        return Err(problem("its name is already a rigid cluster; give the antenna a unique name".to_owned()));
    }
    let topology = match text(parameters, "topology").unwrap_or("auto") {
        "auto" => Topology::Auto,
        "monopole" => Topology::Monopole,
        "meander" => Topology::Meander,
        "inverted_f" => Topology::InvertedF,
        "pixel" => Topology::Pixel,
        other => return crate::not_implemented(&format!("antenna topology {other}")),
    };
    if text(parameters, "side") == Some("back") {
        return crate::not_implemented("a back-side synthesized antenna");
    }
    let bands = parse_bands(text(parameters, "bands").unwrap_or_default()).map_err(problem)?;
    let max_return_loss_db = match parameters.get("max_return_loss_db") {
        Some(ParameterValue::Int(value)) => value.to_string().parse().unwrap_or(-10.0),
        Some(ParameterValue::Float(value)) => *value,
        _ => -10.0,
    };
    let ground = text(parameters, "ground").unwrap_or_default().to_owned();
    if !board.nets.iter().any(|net| net.name == ground) {
        return Err(problem(format!("ground net {} is unknown", repr(&ground))));
    }
    let geometry = text(parameters, "geometry").map(Geometry::parse).transpose().map_err(problem)?;
    if let (Some(geometry), true) = (&geometry, topology != Topology::Auto)
        && geometry.topology() != topology
    {
        return Err(problem(format!(
            "geometry {} contradicts topology {}",
            geometry.topology().value(),
            topology.value()
        )));
    }
    let tuned_for = text(parameters, "tuned_for").map(str::to_owned);
    if tuned_for.is_some() && geometry.is_none() {
        return Err(problem("tuned_for requires the geometry it was tuned for".to_owned()));
    }

    // The feed: one component pin, physicalized to one land on one net.
    let target = &constraint.targets[0];
    let Some((reference, pin)) = target.split_once('.') else {
        return Err(problem("the feed target must be COMPONENT.PIN".to_owned()));
    };
    let Some(component) = flat.components.iter().find(|c| c.reference == reference) else {
        return Err(problem(format!("feed component {} is unknown", repr(reference))));
    };
    let part = &flat.library[&component.part];
    let Some(pad) = physical_pin_number(component, part, &flat.devices, pin) else {
        return Err(problem(format!("feed pin {} has no physical pad", repr(target))));
    };
    let Some(placement) = board.placements.iter().find(|p| p.reference == reference) else {
        return Err(problem(format!("feed component {} has no selected footprint", repr(reference))));
    };
    let footprint = &board.footprints[&placement.footprint];
    let lands: Vec<_> = footprint.pads.iter().filter(|p| p.number == pad).collect();
    let [land] = lands.as_slice() else {
        return Err(problem(format!("feed pad {} must be exactly one physical land", repr(target))));
    };
    let feed = PadReference::new(reference, &pad);
    let Some(net) = board.nets.iter().find(|net| net.pads.contains(&feed)) else {
        return Err(problem(format!("feed pad {} is not connected to a net", repr(target))));
    };
    let mut isolated = Vec::new();
    for other in footprint.pads.iter().filter(|p| p.number != pad && p.kind.is_electrical()) {
        let other_reference = PadReference::new(reference, &other.number);
        if board.nets.iter().any(|net| net.pads.contains(&other_reference)) {
            return Err(problem(format!(
                "feed component pad {} is connected; only the feed pad may carry a net on a synthesized antenna",
                other_reference
            )));
        }
        isolated.push(other.number.clone());
    }
    isolated.sort();
    isolated.dedup();

    // The region, its frame and the dimensions.
    if board.outline.circular_boundary.is_some() || board.outline.boundary_path.is_some() {
        return crate::not_implemented("antenna synthesis on a curved board outline");
    }
    let area = region_bounds(board, parameters).map_err(wrap)?;
    let host = &board.outline.vertices;
    let ring = corners(area);
    if !(0..4).all(|i| segment_in_polygon(ring[i], ring[(i + 1) % 4], host)) {
        return Err(problem("the region extends outside the board outline".to_owned()));
    }
    let perimeter = perimeter_edges(host, area);
    let frame = frame(area, &perimeter).map_err(wrap)?;
    let rules = &board.rules;
    let clearance = rules.minimum_clearance_nm;
    let width = length(parameters, "track_width")?.unwrap_or(rules.default_track_width_nm);
    if width < rules.minimum_track_width_nm {
        return Err(problem(format!(
            "track_width {} is below the minimum track width {}",
            mm(width),
            mm(rules.minimum_track_width_nm)
        )));
    }
    let margin = clearance + width / 2;
    let keepout_margin = length(parameters, "keepout_margin")?.unwrap_or(0);
    if frame.depth() < 2 * margin + width || frame.length() < 2 * margin + width {
        return Err(problem(format!(
            "the region is narrower than one {} track with {} clearance on both sides",
            mm(width),
            mm(clearance)
        )));
    }
    let (eps_eff, dielectric) = effective_permittivity(board, width).map_err(wrap)?;

    // The feed pad sits just outside the region on the ground side; the
    // component body must also clear the placement keepout and the board edge.
    let feed_fit = standoff(&frame, footprint, land, land.position, clearance, keepout_margin);
    let base = margin.max(feed_fit.body_along_half + MEMBER_EDGE_CLEARANCE_NM);
    let mut feed_u = base;

    // The short link of an inverted-F sits between the region start and the feed.
    let short = match text(parameters, "short") {
        None if topology == Topology::InvertedF => {
            return Err(problem("topology inverted_f requires short, the two-pad link component".to_owned()));
        }
        None => None,
        Some(link_reference) => {
            let mut link =
                resolve_short_link(board, &frame, link_reference, &net.name, &ground, clearance, keepout_margin)
                    .map_err(wrap)?;
            let link_half = link.short_min;
            link.short_min = (width + clearance).max(link_half + feed_fit.body_along_half + clearance);
            link.short_min = snap(link.short_min + LENGTH_GRID_NM - 1);
            link.short_max = snap((frame.length() / 5).min(6_000_000)).max(link.short_min);
            let base = base.max(link_half + MEMBER_EDGE_CLEARANCE_NM);
            feed_u = base + link.short_max;
            Some(link)
        }
    };
    if feed_u + margin > frame.length() {
        return Err(problem("the region is too short for the feed component body".to_owned()));
    }
    let pad_gap = feed_fit.gap;
    let feed_centre = frame.to_board(feed_u, -pad_gap);
    let pose = Point::new(feed_centre.x_nm - land.position.x_nm, feed_centre.y_nm - land.position.y_nm);
    Ok(AntennaSite {
        name,
        declaration: constraint.origins.first().cloned().unwrap_or_default(),
        feed,
        feed_net: net.name.clone(),
        feed_land: (*land).clone(),
        footprint: placement.footprint.clone(),
        isolated,
        ground,
        bands,
        topology,
        max_return_loss_db,
        area,
        frame,
        perimeter,
        width,
        clearance,
        keepout_margin,
        pose,
        feed_u,
        feed_v: -pad_gap,
        eps_eff,
        dielectric,
        geometry,
        tuned_for,
        short,
    })
}

/// How far a component must stand off the region so its land `land` (centred
/// at `land_centre` in footprint coordinates) is the board clearance from the
/// region and its body clears the component keepout.
struct Standoff {
    gap: i64,
    body_along_half: i64,
}

fn standoff(
    frame: &Frame,
    footprint: &copper_physical::footprint::PhysicalFootprint,
    land: &FootprintPad,
    land_centre: Point,
    clearance: i64,
    keepout_margin: i64,
) -> Standoff {
    let pad_along_half = match frame.axis {
        Axis::X => land.size.width_nm / 2,
        Axis::Y => land.size.height_nm / 2,
    };
    let pad_across_half = match frame.axis {
        Axis::X => land.size.height_nm / 2,
        Axis::Y => land.size.width_nm / 2,
    };
    let body: Vec<Point> = if footprint.courtyard.is_empty() {
        let (w, h) = (footprint.body_size.width_nm / 2, footprint.body_size.height_nm / 2);
        vec![Point::new(-w, -h), Point::new(w, -h), Point::new(w, h), Point::new(-w, h)]
    } else {
        footprint.courtyard.clone()
    };
    let body_toward_region = body
        .iter()
        .map(|v| frame.outward * (frame.across(*v) - frame.across(land_centre)))
        .max()
        .unwrap_or(pad_across_half)
        .max(pad_across_half);
    let body_along_half =
        body.iter().map(|v| (frame.along(*v) - frame.along(land_centre)).abs()).max().unwrap_or(pad_along_half);
    let gap = (clearance + pad_across_half).max(body_toward_region + keepout_margin + clearance);
    Standoff { gap, body_along_half }
}

/// Footprint point `point` rotated by `degrees` on the front side, as frame
/// `(along, toward the region)` offsets.
fn rotated_offset(frame: &Frame, point: Point, degrees: i64) -> (i64, i64) {
    let rotated = transform_local(point, &Decimal::from_i64(degrees), BoardSide::Front);
    (frame.along(rotated), frame.outward * frame.across(rotated))
}

fn rotated_size(size: copper_physical::footprint::Size, degrees: i64) -> copper_physical::footprint::Size {
    if degrees % 180 == 0 {
        size
    } else {
        copper_physical::footprint::Size { width_nm: size.height_nm, height_nm: size.width_nm }
    }
}

/// Resolve `short`: a placed two-pad component with one land on the feed net
/// and the other on the ground net, rotated so the feed land faces the
/// region. `short_min` returns the body's half extent along the edge; the
/// caller sets the spacing limits.
fn resolve_short_link(
    board: &PhysicalBoard,
    frame: &Frame,
    reference: &str,
    feed_net: &str,
    ground: &str,
    clearance: i64,
    keepout_margin: i64,
) -> Result<ShortLink> {
    let Some(placement) = board.placements.iter().find(|p| p.reference == reference) else {
        return Err(Error::Value(format!("short component {} has no selected footprint", repr(reference))));
    };
    let footprint = &board.footprints[&placement.footprint];
    let electrical: Vec<&FootprintPad> = footprint.pads.iter().filter(|p| p.kind.is_electrical()).collect();
    let [first, second] = electrical.as_slice() else {
        return Err(Error::Value(format!(
            "short component {} must have exactly two electrical lands (a 0 ohm link or tuning inductor)",
            repr(reference)
        )));
    };
    let net_of = |pad: &FootprintPad| {
        let key = PadReference::new(reference, &pad.number);
        board.nets.iter().find(|net| net.pads.contains(&key)).map(|net| net.name.as_str())
    };
    let (feed_pad, ground_pad): (&FootprintPad, &FootprintPad) = match (net_of(first), net_of(second)) {
        (Some(a), Some(b)) if a == feed_net && b == ground => (first, second),
        (Some(a), Some(b)) if b == feed_net && a == ground => (second, first),
        _ => {
            return Err(Error::Value(format!(
                "short component {} must connect one land to the feed net {} and the other to ground {}",
                repr(reference),
                repr(feed_net),
                repr(ground)
            )));
        }
    };
    // The rotation that puts the feed land toward the region with the lands aligned across.
    let rotation = [0, 90, 180, 270]
        .into_iter()
        .map(|degrees| {
            let (fa, fx) = rotated_offset(frame, feed_pad.position, degrees);
            let (ga, gx) = rotated_offset(frame, ground_pad.position, degrees);
            (degrees, fx - gx, (fa - ga).abs())
        })
        .filter(|(_, toward, _)| *toward > 0)
        .min_by_key(|(_, toward, skew)| (*skew, -*toward))
        .map(|(degrees, _, _)| degrees)
        .ok_or_else(|| Error::Value(format!("short component {} has coincident lands", repr(reference))))?;
    let feed_offset = rotated_offset(frame, feed_pad.position, rotation);
    let ground_offset = rotated_offset(frame, ground_pad.position, rotation);
    let angle = Decimal::from_i64(rotation);
    let mut rotated = footprint.clone();
    rotated.courtyard = footprint.courtyard.iter().map(|p| transform_local(*p, &angle, BoardSide::Front)).collect();
    rotated.body_size = rotated_size(footprint.body_size, rotation);
    let mut land = feed_pad.clone();
    land.size = rotated_size(feed_pad.size, rotation);
    let fit = standoff(
        frame,
        &rotated,
        &land,
        transform_local(feed_pad.position, &angle, BoardSide::Front),
        clearance,
        keepout_margin,
    );
    let ground_size = rotated_size(ground_pad.size, rotation);
    let ground_across_half = match frame.axis {
        Axis::X => ground_size.height_nm / 2,
        Axis::Y => ground_size.width_nm / 2,
    };
    let rules = &board.rules;
    let standoff_v = -fit.gap;
    let ground_v = standoff_v - (feed_offset.1 - ground_offset.1);
    let via_v = ground_v - ground_across_half - clearance - rules.default_via_size_nm / 2;
    Ok(ShortLink {
        reference: reference.to_owned(),
        footprint: placement.footprint.clone(),
        feed_pad: feed_pad.clone(),
        ground_pad: ground_pad.clone(),
        rotation,
        feed_offset,
        ground_offset,
        standoff: standoff_v,
        via_v,
        via_size: rules.default_via_size_nm,
        via_drill: rules.default_via_drill_nm,
        via_to: *board.stackup.copper_layers.last().expect("a stackup has copper"),
        short_min: fit.body_along_half,
        short_max: 0,
    })
}

/// The first-order template: a quarter-wave monopole at the band centre,
/// scaled by the Hammerstad-Jensen effective permittivity, with its run on
/// the region's centreline.
pub fn template_geometry(site: &AntennaSite) -> Geometry {
    let length = quarter_wave_nm(site.centre_hz(), site.eps_eff);
    match &site.short {
        Some(link) => Geometry::InvertedF {
            height: snap(site.frame.depth() - site.margin() - LENGTH_GRID_NM / 2),
            length,
            short: snap((link.short_min + link.short_max) / 2),
        },
        None => Geometry::Monopole { height: site.frame.depth() / 2, length },
    }
}

/// The feed track's path and the tiles of `geometry`, checked to fit the
/// region with the board clearance and to keep its own legs apart.
pub fn layout(site: &AntennaSite, geometry: &Geometry) -> Result<Layout> {
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(&site.name)));
    let (w, clearance, margin) = (site.width, site.clearance, site.margin());
    let (span, depth) = (site.frame.length(), site.frame.depth());
    let (u0, pad_v) = (site.feed_u, site.feed_v);
    let fits_height = |height: i64| -> Result<()> {
        if height < margin || height > depth - margin {
            return Err(problem(format!(
                "height {} must stay between {} and {} (clearance plus half a track from the region edges)",
                mm(height),
                mm(margin),
                mm(depth - margin)
            )));
        }
        Ok(())
    };
    let fits_end = |u: i64, needed: i64| -> Result<()> {
        if u > span - margin {
            return Err(problem(format!(
                "the region is too short: the geometry needs {} of path but ends {} past the usable region; \
                 enlarge the region along the board edge or shorten the geometry",
                mm(needed),
                mm(u - (span - margin))
            )));
        }
        Ok(())
    };
    match geometry {
        Geometry::Monopole { height, length } => {
            let (height, length) = (*height, *length);
            if length <= 0 {
                return Err(problem("monopole length must be positive".to_owned()));
            }
            let mut path = vec![(u0, pad_v)];
            if length <= height {
                fits_height(length)?;
                path.push((u0, length));
            } else {
                fits_height(height)?;
                fits_end(u0 + (length - height), length)?;
                path.push((u0, height));
                path.push((u0 + (length - height), height));
            }
            Ok(Layout { path, tiles: Vec::new(), length_nm: length, short: None })
        }
        Geometry::InvertedF { height, length, short } => {
            let (height, length, spacing) = (*height, *length, *short);
            let Some(link) = &site.short else {
                return Err(problem("inverted_f geometry needs short, the two-pad link component".to_owned()));
            };
            fits_height(height)?;
            if spacing < link.short_min || spacing > link.short_max {
                return Err(problem(format!(
                    "inverted_f short {} must stay between {} (bodies and legs clear) and {} (the room before the feed)",
                    mm(spacing),
                    mm(link.short_min),
                    mm(link.short_max)
                )));
            }
            if length <= spacing + w {
                return Err(problem("inverted_f length must reach past the feed stub".to_owned()));
            }
            let leg_u = u0 - spacing;
            fits_end(leg_u + length, length)?;
            // Feed stub to the arm; the arm from the leg top to the open end.
            let path = vec![(u0, pad_v), (u0, height), (leg_u, height), (leg_u + length, height)];
            let feed_pad = (leg_u, link.standoff);
            let centre = (leg_u - link.feed_offset.0, link.standoff - link.feed_offset.1);
            let ground_pad = (centre.0 + link.ground_offset.0, centre.1 + link.ground_offset.1);
            let via = (ground_pad.0, link.via_v);
            // The leg runs at track width to the link land's near edge, then
            // at the land's width into it, so it stays clear of the ground land.
            let near_edge = (leg_u, link.standoff + link.feed_pad.size.width_nm.min(link.feed_pad.size.height_nm) / 2);
            let short =
                ShortLayout { leg: vec![(leg_u, height), near_edge, feed_pad], centre, feed_pad, ground_pad, via };
            Ok(Layout { path, tiles: Vec::new(), length_nm: length, short: Some(short) })
        }
        Geometry::Meander { height, depth: drop, pitch, turns, tail } => {
            let (height, drop, pitch) = (*height, *drop, *pitch);
            fits_height(height)?;
            if pitch < w + clearance {
                return Err(problem(format!(
                    "meander pitch {} leaves less than the {} clearance between legs",
                    mm(pitch),
                    mm(clearance)
                )));
            }
            if drop <= 0 || height - drop < margin {
                return Err(problem(format!(
                    "meander depth {} must be positive and keep the legs {} above the ground edge",
                    mm(drop),
                    mm(margin)
                )));
            }
            let mut path = vec![(u0, pad_v), (u0, height)];
            let mut u = u0 + pitch;
            path.push((u, height));
            for _ in 0..*turns {
                path.push((u, height - drop));
                u += pitch;
                path.push((u, height - drop));
                path.push((u, height));
                u += pitch;
                path.push((u, height));
            }
            if *tail > 0 {
                u += tail;
                path.push((u, height));
            }
            let length = height + pitch + i64::from(*turns) * (2 * drop + 2 * pitch) + tail;
            fits_end(u, length)?;
            // The meander's feed stub and its first down leg are `pitch` apart, as are all legs.
            Ok(Layout { path, tiles: Vec::new(), length_nm: length, short: None })
        }
        Geometry::Pixel { cell, columns, rows, bits } => {
            let (cell, columns, rows) = (*cell, *columns as i64, *rows as i64);
            if cell < w {
                return Err(problem(format!("pixel cell {} is narrower than the track width {}", mm(cell), mm(w))));
            }
            if bits.len() as i64 != columns * rows {
                return Err(problem("pixel bits do not match the grid".to_owned()));
            }
            // Columns: one centred on the feed, extending both ways inside the margins.
            let left = (u0 - cell / 2 - clearance) / cell;
            let first_u = u0 - cell / 2 - left * cell;
            let bottom = clearance.max(margin - w / 2);
            if first_u < clearance
                || first_u + columns * cell > span - clearance
                || bottom + rows * cell > depth - clearance
            {
                return Err(problem(format!(
                    "a {columns} x {rows} grid of {} cells does not fit the region",
                    mm(cell)
                )));
            }
            let feed_column = left;
            let on = |c: i64, r: i64| bits[(r * columns + c) as usize];
            if feed_column >= columns || !on(feed_column, 0) {
                return Err(problem("the pixel tile above the feed must be copper".to_owned()));
            }
            // Keep the 4-connected component of the feed tile.
            let mut keep = vec![false; bits.len()];
            let mut stack = vec![(feed_column, 0)];
            while let Some((c, r)) = stack.pop() {
                let index = (r * columns + c) as usize;
                if keep[index] || !on(c, r) {
                    continue;
                }
                keep[index] = true;
                for (dc, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nc, nr) = (c + dc, r + dr);
                    if (0..columns).contains(&nc) && (0..rows).contains(&nr) {
                        stack.push((nc, nr));
                    }
                }
            }
            let tiles: Vec<[i64; 4]> = (0..rows)
                .flat_map(|r| (0..columns).map(move |c| (c, r)))
                .filter(|(c, r)| keep[(r * columns + c) as usize])
                .map(|(c, r)| {
                    let (ua, va) = (first_u + c * cell, bottom + r * cell);
                    [ua, va, ua + cell, va + cell]
                })
                .collect();
            let path = vec![(u0, pad_v), (u0, bottom + cell / 2)];
            Ok(Layout { path, tiles, length_nm: bottom + cell / 2, short: None })
        }
    }
}

/// The digest of every input the full-wave model of `site` reads: outline,
/// stack-up, the ground net's zones, region, frame, track rules, feed land
/// and pose, bands, target, generator and model revision. `tuned_for` must
/// equal it.
pub fn site_digest(board: &PhysicalBoard, site: &AntennaSite) -> String {
    let points = |ring: &[Point]| Json::Array(ring.iter().map(|p| point_json(*p)).collect());
    let decimal = |value: &Option<Decimal>| value.as_ref().map_or(Json::Null, |d| Json::from(d.to_sci_string()));
    let stackup: Vec<Json> = board
        .stackup
        .physical_layers
        .iter()
        .map(|layer| {
            object(vec![
                ("id", Json::from(layer.id.as_str())),
                ("copper", layer.copper_layer.map_or(Json::Null, |l| Json::from(l.value()))),
                ("thickness_nm", Json::from(layer.thickness_nm)),
                ("er", decimal(&layer.relative_permittivity)),
                ("loss_tangent", decimal(&layer.loss_tangent)),
            ])
        })
        .collect();
    let zones: Vec<Json> = board
        .zones
        .iter()
        .filter(|zone| zone.net == site.ground)
        .map(|zone| {
            object(vec![
                ("layers", Json::Array(zone.layers.iter().map(|l| Json::from(l.value())).collect())),
                ("outer", points(&zone.outer)),
                ("holes", Json::Array(zone.holes.iter().map(|h| points(h)).collect())),
            ])
        })
        .collect();
    let land = &site.feed_land;
    let document = object(vec![
        ("generator", Json::from(GENERATOR)),
        ("model_revision", Json::from(MODEL_REVISION)),
        ("outline", points(&board.outline.vertices)),
        ("cutouts", Json::Array(board.outline.cutouts.iter().map(|c| points(&c.vertices)).collect())),
        ("stackup", Json::Array(stackup)),
        ("copper_layers", Json::Array(board.stackup.copper_layers.iter().map(|l| Json::from(l.value())).collect())),
        ("ground_zones", Json::Array(zones)),
        ("region", points(&corners(site.area))),
        ("perimeter", Json::Array(site.perimeter.iter().map(|e| Json::from(*e as i64)).collect())),
        ("track_width_nm", Json::from(site.width)),
        ("clearance_nm", Json::from(site.clearance)),
        ("feed_land_nm", Json::Array(vec![Json::from(land.size.width_nm), Json::from(land.size.height_nm)])),
        ("feed_local_nm", Json::Array(vec![Json::from(site.feed_u), Json::from(site.feed_v)])),
        ("bands", Json::Array(site.bands.iter().map(|b| Json::from(b.text.as_str())).collect())),
        ("max_return_loss_db", Json::from(format!("{}", site.max_return_loss_db))),
        (
            "short",
            site.short.as_ref().map_or(Json::Null, |link| {
                object(vec![
                    ("footprint", Json::from(board.footprints[&link.footprint].name.as_str())),
                    ("rotation", Json::from(link.rotation)),
                    (
                        "feed_offset_nm",
                        Json::Array(vec![Json::from(link.feed_offset.0), Json::from(link.feed_offset.1)]),
                    ),
                    (
                        "ground_offset_nm",
                        Json::Array(vec![Json::from(link.ground_offset.0), Json::from(link.ground_offset.1)]),
                    ),
                    (
                        "feed_land_nm",
                        Json::Array(vec![
                            Json::from(link.feed_pad.size.width_nm),
                            Json::from(link.feed_pad.size.height_nm),
                        ]),
                    ),
                    ("standoff_nm", Json::from(link.standoff)),
                    ("via_v_nm", Json::from(link.via_v)),
                    ("via_size_nm", Json::from(link.via_size)),
                    ("short_range_nm", Json::Array(vec![Json::from(link.short_min), Json::from(link.short_max)])),
                ])
            }),
        ),
    ]);
    let canonical = Dumps::compact().sort_keys(true).dumps(&document);
    let digest: String = Sha256::digest(canonical.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{digest}")
}

/// Why pixel tiles cannot be bound on this board, if they cannot: tiles are
/// owned copper polygons, and no host zone may overlap a private macro region
/// on a layer where the macro owns area copper.
pub fn tile_zone_conflict(board: &PhysicalBoard, site: &AntennaSite) -> Option<String> {
    let ring = corners(site.area);
    let only = [CopperLayer::Front];
    let zones: Vec<&str> = board
        .zones
        .iter()
        .filter(|zone| {
            zone.layers.contains(&CopperLayer::Front)
                && crate::hard_macros::zone_outlines_overlap(&only, &zone.outer, &only, &ring)
        })
        .map(|zone| zone.id.as_str())
        .collect();
    (!zones.is_empty()).then(|| {
        format!(
            "pixel tiles are owned copper polygons, so no F.Cu copper_zone may overlap or touch the antenna \
             region; draw {} with a gap around the region (polygon_mm or a placement region), which also keeps \
             ground out of it",
            zones.join(", ")
        )
    })
}

/// The provenance of a bound geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// The first-order template (no `geometry` in the source).
    Template,
    /// A source geometry with a matching `tuned_for`.
    Tuned,
    /// A source geometry without `tuned_for`.
    Manual,
}

impl Provenance {
    pub fn value(self) -> &'static str {
        match self {
            Provenance::Template => "template",
            Provenance::Tuned => "tuned",
            Provenance::Manual => "manual",
        }
    }
}

/// The geometry a site binds and its provenance; a stale `tuned_for` fails.
pub fn chosen_geometry(board: &PhysicalBoard, site: &AntennaSite) -> Result<(Geometry, Provenance)> {
    let Some(geometry) = &site.geometry else { return Ok((template_geometry(site), Provenance::Template)) };
    let Some(tuned_for) = &site.tuned_for else { return Ok((geometry.clone(), Provenance::Manual)) };
    let current = site_digest(board, site);
    if *tuned_for != current {
        return Err(Error::Value(format!(
            "antenna {}: geometry was tuned for {tuned_for} but the board now gives {current}; an input of the \
             full-wave model changed (outline, stack-up, ground zones, region, track rules, feed, bands or target). \
             Re-run `copper synthesize-antenna BOARD --antenna {}`, or remove tuned_for to keep the geometry untuned",
            repr(&site.name),
            site.name
        )));
    }
    Ok((geometry.clone(), Provenance::Tuned))
}

fn unresolved(provenance: Provenance) -> Vec<&'static str> {
    let mut items = vec![match provenance {
        Provenance::Template => {
            "First-order quarter-wave template: no field solution, no resonance or return-loss evidence"
        }
        Provenance::Tuned => {
            "Tuned in a full-wave model of the board outline, stack-up and ground zones only; components, other \
             copper, the enclosure and any cable are not in that model"
        }
        Provenance::Manual => "Source geometry without tuned_for: no resonance or return-loss evidence",
    }];
    items.extend([
        "Host ground plane, enclosure, battery and nearby metal shift resonance",
        "Matching network values and bench tuning in the final enclosure remain mandatory",
        "No regulatory (radiated emission) claim follows from this geometry",
    ]);
    items
}

/// One antenna ready to bind.
struct Synthesis {
    asset: Json,
    keepout: PlacementKeepout,
    record: Json,
    /// The short link's board pose and rotation, when the geometry has one.
    short_pose: Option<(Point, i64)>,
}

fn synthesize(board: &PhysicalBoard, site: &AntennaSite) -> Result<Synthesis> {
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(&site.name)));
    let (geometry, provenance) = chosen_geometry(board, site)?;
    let layout = match (layout(site, &geometry), provenance) {
        (Ok(layout), _) => layout,
        (Err(Error::Value(message)), Provenance::Template) => {
            let detail = message.split_once(": ").map_or(message.as_str(), |(_, rest)| rest);
            return Err(problem(format!(
                "the first-order quarter-wave monopole template at {} (effective permittivity {:.3}) does not \
                 fit; {detail}. A meander fits more length: set topology = meander and run \
                 `copper synthesize-antenna`",
                format_hz(site.centre_hz()),
                site.eps_eff
            )));
        }
        (Err(error), _) => return Err(error),
    };
    if !layout.tiles.is_empty()
        && let Some(message) = tile_zone_conflict(board, site)
    {
        return Err(problem(message));
    }
    let footprint = &board.footprints[&site.footprint];
    let pose = site.pose;
    let frame = &site.frame;
    let ring = corners(site.area);
    let local = |point: Point| Point::new(point.x_nm - pose.x_nm, point.y_nm - pose.y_nm);
    let board_point = |(u, v): (i64, i64)| frame.to_board(u, v);
    let local_ring: Vec<Json> = ring.iter().map(|point| point_json(local(*point))).collect();
    let layers: Vec<&str> = board.stackup.copper_layers.iter().map(|layer| layer.value()).collect();
    let pad = site.feed.pad.as_str();
    let pad_json = Json::Array(vec![Json::from(MEMBER), Json::from(pad)]);
    let mut points = vec![object(vec![("pad", pad_json.clone())])];
    points.extend(layout.path.iter().skip(1).map(|p| point_json(local(board_point(*p)))));
    let mut tracks = vec![object(vec![
        ("net", Json::from(FEED_ROLE)),
        ("layer", Json::from(CopperLayer::Front.value())),
        ("width_nm", Json::from(site.width)),
        ("points", Json::Array(points)),
    ])];
    let mut members = vec![object(vec![
        ("reference", Json::from(MEMBER)),
        ("footprint", Json::from(footprint.name.as_str())),
        ("footprint_digest", Json::from(footprint_geometry_digest(footprint).as_str())),
        ("center_nm", point_json(Point::new(0, 0))),
        ("rotation_degrees", Json::from("0")),
        ("edge_clearance_nm", Json::from(MEMBER_EDGE_CLEARANCE_NM)),
    ])];
    let mut pad_nets = vec![texts(&[MEMBER, pad, FEED_ROLE])];
    let mut vias = Vec::new();
    let mut plane_returns = Vec::new();
    let mut short_pose: Option<(Point, i64)> = None;
    if let (Some(link), Some(realized)) = (&site.short, &layout.short) {
        let link_footprint = &board.footprints[&link.footprint];
        let centre = board_point(realized.centre);
        let via = board_point(realized.via);
        let ground_zoned = board
            .zones
            .iter()
            .any(|zone| zone.net == site.ground && copper_physical::geometry::point_in_polygon(via, &zone.outer));
        if !ground_zoned {
            return Err(problem(format!(
                "the short's ground via at ({}, {}) must sit inside a copper_zone on {}",
                mm(via.x_nm),
                mm(via.y_nm),
                site.ground
            )));
        }
        members.push(object(vec![
            ("reference", Json::from(SHORT_MEMBER)),
            ("footprint", Json::from(link_footprint.name.as_str())),
            ("footprint_digest", Json::from(footprint_geometry_digest(link_footprint).as_str())),
            ("center_nm", point_json(local(centre))),
            ("rotation_degrees", Json::from(link.rotation.to_string())),
            ("edge_clearance_nm", Json::from(MEMBER_EDGE_CLEARANCE_NM)),
        ]));
        let feed_land = Json::Array(vec![Json::from(SHORT_MEMBER), Json::from(link.feed_pad.number.as_str())]);
        let ground_land = Json::Array(vec![Json::from(SHORT_MEMBER), Json::from(link.ground_pad.number.as_str())]);
        pad_nets.push(texts(&[SHORT_MEMBER, &link.feed_pad.number, FEED_ROLE]));
        pad_nets.push(texts(&[SHORT_MEMBER, &link.ground_pad.number, GROUND_ROLE]));
        let land_width = |pad: &FootprintPad| pad.size.width_nm.min(pad.size.height_nm).min(site.width);
        let leg: Vec<Json> = realized.leg.iter().map(|p| point_json(local(board_point(*p)))).collect();
        let near_edge = leg[leg.len() - 2].clone();
        tracks.push(object(vec![
            ("net", Json::from(FEED_ROLE)),
            ("layer", Json::from(CopperLayer::Front.value())),
            ("width_nm", Json::from(site.width)),
            ("points", Json::Array(leg[..leg.len() - 1].to_vec())),
        ]));
        tracks.push(object(vec![
            ("net", Json::from(FEED_ROLE)),
            ("layer", Json::from(CopperLayer::Front.value())),
            ("width_nm", Json::from(land_width(&link.feed_pad))),
            ("points", Json::Array(vec![near_edge, object(vec![("pad", feed_land)])])),
        ]));
        tracks.push(object(vec![
            ("net", Json::from(GROUND_ROLE)),
            ("layer", Json::from(CopperLayer::Front.value())),
            ("width_nm", Json::from(land_width(&link.ground_pad))),
            ("points", Json::Array(vec![object(vec![("pad", ground_land.clone())]), point_json(local(via))])),
        ]));
        vias.push(object(vec![
            ("net", Json::from(GROUND_ROLE)),
            ("position_nm", point_json(local(via))),
            ("size_nm", Json::from(link.via_size)),
            ("drill_nm", Json::from(link.via_drill)),
            ("from_layer", Json::from(CopperLayer::Front.value())),
            ("to_layer", Json::from(link.via_to.value())),
            ("technology", Json::Null),
            ("finish", Json::from("standard")),
        ]));
        plane_returns.push(object(vec![
            ("net", Json::from(GROUND_ROLE)),
            ("layers", Json::Array(board.stackup.copper_layers.iter().map(|l| Json::from(l.value())).collect())),
            ("pads", Json::Array(vec![ground_land])),
            ("dedicated_contacts", Json::Array(Vec::new())),
        ]));
        short_pose = Some((centre, link.rotation));
    }
    // The feed port covers every private pad on the feed net.
    let mut port_pads = vec![pad_json.clone()];
    if let Some(link) = &site.short {
        port_pads.push(Json::Array(vec![Json::from(SHORT_MEMBER), Json::from(link.feed_pad.number.as_str())]));
    }
    let polygons: Vec<Json> = layout
        .tiles
        .iter()
        .enumerate()
        .map(|(i, [u0, v0, u1, v1])| {
            let vertices = [(*u0, *v0), (*u1, *v0), (*u1, *v1), (*u0, *v1)];
            object(vec![
                ("id", Json::from(format!("{}-tile-{i}", site.name))),
                ("net", Json::from(FEED_ROLE)),
                ("layer", Json::from(CopperLayer::Front.value())),
                ("vertices", Json::Array(vertices.iter().map(|p| point_json(local(board_point(*p)))).collect())),
            ])
        })
        .collect();
    let region_id = format!("{}-region", site.name);
    let edge_id = format!("{}-edge", site.name);
    let region_row = |id: &str, block_copper: bool| {
        object(vec![
            ("id", Json::from(id)),
            ("layers", texts(&layers)),
            ("vertices", Json::Array(local_ring.clone())),
            ("block_tracks", Json::Bool(block_copper)),
            ("block_vias", Json::Bool(block_copper)),
            ("block_zones", Json::Bool(true)),
        ])
    };
    let unresolved = unresolved(provenance);
    let current = site_digest(board, site);
    let synthesis = object(vec![
        ("generator", Json::from(GENERATOR)),
        ("declaration", Json::from(site.declaration.as_str())),
        ("provenance", Json::from(provenance.value())),
        ("topology", Json::from(geometry.topology().value())),
        ("geometry", Json::from(geometry.to_source())),
        ("site_digest", Json::from(current.as_str())),
        ("tuned_for", site.tuned_for.as_deref().map_or(Json::Null, Json::from)),
        ("bands", Json::Array(site.bands.iter().map(|b| Json::from(b.text.as_str())).collect())),
        ("centre_frequency_hz", Json::from(format!("{:.0}", site.centre_hz()))),
        ("stackup_dielectric", Json::from(site.dielectric.as_str())),
        ("effective_permittivity", Json::from(format!("{:.6}", site.eps_eff))),
        ("quarter_wave_nm", Json::from(quarter_wave_nm(site.centre_hz(), site.eps_eff))),
        ("path_length_nm", Json::from(layout.length_nm)),
        ("track_width_nm", Json::from(site.width)),
        ("tiles", Json::from(layout.tiles.len() as i64)),
        ("region_nm", Json::Array(ring.iter().map(|point| point_json(*point)).collect())),
        ("perimeter_edges", Json::Array(site.perimeter.iter().map(|edge| Json::from(*edge as i64)).collect())),
    ]);
    let isolated: Vec<Json> = site.isolated.iter().map(|number| texts(&[MEMBER, number])).collect();
    let asset = object(vec![
        ("schema", Json::from("copperlib-physical-hard-macro/v0.5")),
        ("production_publishable", Json::Bool(false)),
        ("source", synthesis.clone()),
        ("anchor", Json::from(MEMBER)),
        ("members", Json::Array(members)),
        ("pad_nets", Json::Array(pad_nets)),
        ("isolated_pads", Json::Array(isolated)),
        ("tracks", Json::Array(tracks)),
        ("vias", Json::Array(vias)),
        (
            "ports",
            Json::Array(vec![object(vec![
                ("name", Json::from(FEED_ROLE)),
                ("net", Json::from(FEED_ROLE)),
                ("point", object(vec![("pad", pad_json.clone())])),
                ("layer", Json::from(CopperLayer::Front.value())),
                ("pads", Json::Array(port_pads)),
            ])]),
        ),
        ("protected_regions", Json::Array(vec![region_row(&region_id, true)])),
        ("keepouts", Json::Array(vec![region_row(&edge_id, false)])),
        ("zones", Json::Array(Vec::new())),
        ("polygons", Json::Array(polygons)),
        ("plane_returns", Json::Array(plane_returns)),
        ("width_contracts", Json::Array(Vec::new())),
        (
            "edge_openings",
            Json::Array(vec![object(vec![
                ("keepout", Json::from(edge_id.as_str())),
                ("edge_indices", Json::Array(site.perimeter.iter().map(|edge| Json::from(*edge as i64)).collect())),
            ])]),
        ),
        ("required_layers", texts(&layers)),
        ("allowed_rotations", Json::Array(vec![Json::from(0_i64)])),
        ("internal_clearance_nm", Json::from(0_i64)),
        ("unresolved", texts(&unresolved)),
    ]);
    copper_lang::macros::validate_macro_asset_data(&asset).map_err(problem)?;
    let keepout_area = site.area.expanded(site.keepout_margin);
    let keepout = PlacementKeepout::new(
        &format!("{}-components", site.name),
        BoardOutline::rectangle(
            keepout_area.max_x - keepout_area.min_x,
            keepout_area.max_y - keepout_area.min_y,
            Some(Point::new(keepout_area.min_x, keepout_area.min_y)),
        )?,
        None,
        None,
    )?;
    let mut record = match synthesis {
        Json::Object(object) => object,
        _ => Object::new(),
    };
    record.insert("name".to_owned(), Json::from(site.name.as_str()));
    record.insert("net".to_owned(), Json::from(site.feed_net.as_str()));
    record.insert("ground".to_owned(), Json::from(site.ground.as_str()));
    record.insert("feed".to_owned(), Json::from(site.feed.to_string()));
    record.insert("feed_pose_nm".to_owned(), point_json(pose));
    record.insert("path_nm".to_owned(), Json::Array(layout.path.iter().map(|p| point_json(board_point(*p))).collect()));
    record.insert("placement_keepout".to_owned(), Json::from(keepout.name.as_str()));
    record.insert("unresolved".to_owned(), texts(&unresolved));
    if let Some((centre, rotation)) = short_pose {
        record.insert("short_pose_nm".to_owned(), point_json(centre));
        record.insert("short_rotation".to_owned(), Json::from(rotation));
    }
    Ok(Synthesis { asset, keepout, record: Json::Object(record), short_pose })
}

/// Fix the feed component at its synthesized pose, reserve the region for
/// components, and bind the generated macro through the immutable binder.
/// Fix `reference` at `pose` with `rotation` on the front side, both as a
/// placement rule and in the draft placement.
fn fix_member(board: &mut PhysicalBoard, name: &str, reference: &str, pose: Point, rotation: i64) -> Result<()> {
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(name)));
    let angle = Decimal::from_i64(rotation);
    let mut rule = board
        .placement_rules
        .iter()
        .find(|rule| rule.reference == reference)
        .cloned()
        .unwrap_or_else(|| ComponentPlacementRule::new(reference));
    if rule.fixed_position.is_some_and(|position| position != pose)
        || rule.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != angle)
        || rule.side.is_some_and(|side| side != BoardSide::Front)
    {
        return Err(problem(format!(
            "component {} already has a conflicting fixed placement; the antenna fixes its pose",
            repr(reference)
        )));
    }
    rule.fixed_position = Some(pose);
    rule.fixed_rotation_degrees = Some(angle.clone());
    rule.side = Some(BoardSide::Front);
    rule.allowed_orientations = vec![angle.clone()];
    rule.edge_clearance_nm = Some(MEMBER_EDGE_CLEARANCE_NM);
    rule.region = None;
    let rule = rule.normalized()?;
    board.placement_rules.retain(|existing| existing.reference != reference);
    board.placement_rules.push(rule);
    board.placement_rules.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in &mut board.placements {
        if placement.reference == reference {
            placement.position = pose;
            placement.rotation_degrees = angle.clone();
            placement.side = BoardSide::Front;
        }
    }
    Ok(())
}

fn bind(mut board: PhysicalBoard, site: &AntennaSite, synthesis: Synthesis) -> Result<PhysicalBoard> {
    let Synthesis { asset, keepout, short_pose, .. } = synthesis;
    let (name, reference, pose) = (&site.name, &site.feed.component, site.pose);
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(name)));
    if board.keepouts.iter().any(|existing| existing.name == keepout.name) {
        return Err(problem(format!("placement keepout {} already exists", repr(&keepout.name))));
    }
    fix_member(&mut board, name, reference, pose, 0)?;
    if let (Some(link), Some((centre, rotation))) = (&site.short, short_pose) {
        fix_member(&mut board, name, &link.reference, centre, rotation)?;
    }
    board.keepouts.push(keepout);
    let raw = copper_lang::macros::canonical_macro_bytes(&asset);
    let digest: String = Sha256::digest(&raw).iter().map(|byte| format!("{byte:02x}")).collect();
    let mut members: IndexMap<String, String> = [(MEMBER.to_owned(), reference.clone())].into_iter().collect();
    let mut nets: IndexMap<String, String> = [(FEED_ROLE.to_owned(), site.feed_net.clone())].into_iter().collect();
    if let Some(link) = &site.short {
        members.insert(SHORT_MEMBER.to_owned(), link.reference.clone());
        nets.insert(GROUND_ROLE.to_owned(), site.ground.clone());
    }
    let asset_path = format!("generated:{GENERATOR}:{name}");
    let mut board = match bind_hard_macro(board, Path::new(&asset_path), &raw, &digest, name, &members, &nets) {
        Ok(board) => board,
        Err(Error::Value(message)) => return Err(problem(format!("generated macro rejected: {message}"))),
        Err(other) => return Err(other),
    };
    crate::physicalize::revalidate(&mut board)?;
    Ok(board)
}

/// `apply_antenna_constraints`: resolve, realize and bind every
/// `constraint antenna`. Records every synthesis in
/// `metadata["antenna_synthesis"]`.
pub fn apply_antenna_constraints(mut board: PhysicalBoard, flat: &FlatElectricalView) -> Result<PhysicalBoard> {
    let sites = resolve_sites(&board, flat)?;
    let mut records = Vec::new();
    for site in &sites {
        let synthesis = synthesize(&board, site)?;
        records.push(synthesis.record.clone());
        board = bind(board, site, synthesis)?;
    }
    if !records.is_empty() {
        let text = Dumps::new().sort_keys(true).dumps(&Json::Array(records));
        board.metadata.insert("antenna_synthesis".to_owned(), text);
    }
    Ok(board)
}

/// The recorded syntheses of a board, by macro name.
pub fn recorded_syntheses(board: &PhysicalBoard) -> Vec<Object> {
    match board.metadata.get("antenna_synthesis").map(|text| copper_json::parse(text)) {
        Some(Ok(Json::Array(rows))) => rows
            .into_iter()
            .filter_map(|row| match row {
                Json::Object(object) => Some(object),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A short human-readable statement of `site`'s band and target.
pub fn describe(site: &AntennaSite) -> String {
    let bands: Vec<&str> = site.bands.iter().map(|b| b.text.as_str()).collect();
    format!(
        "antenna {} on {} ({}){}, target {:.1} dB over {}, centre {}",
        repr(&site.name),
        site.feed,
        site.feed_net,
        site.short.as_ref().map_or(String::new(), |link| format!(", shorted through {}", link.reference)),
        site.max_return_loss_db,
        bands.join("; "),
        format_hz(site.centre_hz())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_wave_scales_with_permittivity() {
        // 2.44175 GHz in air: 30.69 mm; over FR4-like eps_eff 2.8: 18.34 mm.
        assert_eq!(quarter_wave_nm(2_441_750_000.0, 1.0), 30_690_000);
        assert_eq!(quarter_wave_nm(2_441_750_000.0, 2.8), 18_340_000);
    }

    #[test]
    fn frame_prefers_the_longer_run_and_rejects_interior_regions() {
        let area = Bounds { min_x: 20_000_000, min_y: 0, max_x: 40_000_000, max_y: 6_000_000 };
        let frame = frame(area, &[0]).expect("a top-edge region");
        assert_eq!(frame.axis, Axis::X);
        assert_eq!(frame.inner, 6_000_000);
        assert_eq!(frame.outward, -1);
        assert_eq!(frame.to_board(2_000_000, 1_000_000), Point::new(22_000_000, 5_000_000));
        // A corner region shares a short side too; the long run still wins.
        assert_eq!(super::frame(area, &[0, 3]).expect("a frame").axis, Axis::X);
        assert!(super::frame(area, &[]).is_err());
        assert!(super::frame(area, &[0, 2]).is_err());
    }

    #[test]
    fn geometry_source_round_trips_and_rejects_dc_shorted_kinds() {
        for text in [
            "monopole height=3mm length=17.46mm",
            "meander height=4.5mm depth=3mm pitch=1.5mm turns=4 tail=2.12mm",
            "inverted_f height=5.3mm length=17mm short=2.5mm",
            "pixel cell=1mm columns=5 rows=2 bits=a5c",
        ] {
            let geometry = Geometry::parse(text).expect(text);
            assert_eq!(geometry.to_source(), text);
        }
        assert!(Geometry::parse("monopole height=3mm").is_err());
        assert!(Geometry::parse("monopole height=3 length=4mm").is_err());
        assert!(Geometry::parse("pixel cell=1mm columns=5 rows=2 bits=a5d").is_err());
        assert!(Geometry::parse("inverted_f height=3mm").is_err());
        assert!(Geometry::parse("loop radius=3mm").unwrap_err().contains("not implemented"));
    }
}

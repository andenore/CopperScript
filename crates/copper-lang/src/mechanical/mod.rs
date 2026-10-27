//! Mechanical design lowering (`pcbir.design.lower_mechanical`).
//!
//! A `mechanical` block lowers to board geometry from `copper-physical`,
//! validated exactly as the Python dataclasses validate it. Item errors are
//! `MEC002` at the item, whole-block errors `MEC003` at the block.

mod anchors;
pub mod assembly;
mod profiles;
mod references;
pub mod stackup;

use indexmap::{IndexMap, IndexSet};

use copper_json::{Object, Value as Json};
use copper_physical::geometry::RoundedConvexShape;
use copper_physical::mechanical::{shape_in_outline, validate_mechanical_holes};
use copper_physical::{
    BoardCutout, BoardDatum, BoardEdge, BoardOutline, BoardSide, BoundarySegment, CopperKeepout, CopperLayer,
    DEFAULT_CHORD_ERROR_NM, DesignRules, MAX_COORDINATE, MechanicalHole, MechanicalSlot, PlacementKeepout, Point,
    polygon_ring,
};
use copper_units::{Decimal, Dimension, Quantity};

use crate::compiler::MechanicalProfileDefinition;
use crate::error::CopperScriptError;
use crate::pyvalue;
use crate::syntax::{Declaration, Document, Location, MechanicalItem, MechanicalItemDecl, Scalar, Value};

pub use anchors::MechanicalAttachment;
pub use assembly::{AssemblyAccess, AssemblyEnvelope, BodyOverhang, ComponentHeight};
pub use profiles::{MechanicalFeatureSource, MechanicalProfileInstance, expand_mechanical_items};
pub use references::{MechanicalReference, ReferenceEntity, reference_scene};
pub use stackup::{MechanicalStackup, lower_stackup};

/// The code of the placeholder error for rewrite gaps.
pub const NOT_IMPLEMENTED: &str = "RUST_NOT_IMPLEMENTED";

/// A lowering failure: Python's `ValueError` text, or a rewrite limit.
#[derive(Debug)]
pub(crate) enum Fail {
    Value(String),
    Rust(String),
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail::Value(message)
    }
}

impl Fail {
    pub(crate) fn error(self, code: &str, location: &Location) -> CopperScriptError {
        match self {
            Fail::Value(message) => CopperScriptError::new(code, message, location.clone()),
            Fail::Rust(message) => CopperScriptError::new(NOT_IMPLEMENTED, message, location.clone()),
        }
    }
}

pub(crate) type Lower<T> = Result<T, Fail>;

pub(crate) fn fail<T>(message: impl Into<String>) -> Lower<T> {
    Err(Fail::Value(message.into()))
}

/// `Length.of(...)` in integer nanometres; `what` names the block in messages.
pub(crate) fn nanometres(value: &Value, what: &str) -> Lower<i64> {
    let Value::Scalar(Scalar::Quantity { value, unit }) = value else {
        return fail(format!("{what} dimensions require typed lengths"));
    };
    let number = Decimal::parse(value).map_err(|failure| Fail::Rust(failure.to_string()))?;
    let quantity =
        Quantity::of(Dimension::Length, &number, unit).map_err(|failure| Fail::Value(failure.to_string()))?;
    let nm = quantity
        .base_value()
        .mul(&Decimal::from_i64(1_000_000_000))
        .map_err(|failure| Fail::Rust(failure.to_string()))?;
    if !nm.is_integral() {
        return fail(format!("{what} lengths must be exact integer nanometres"));
    }
    match nm.to_i64() {
        Some(nm) if nm.abs() <= MAX_COORDINATE => Ok(nm),
        _ => Err(Fail::Rust(format!("{what} lengths beyond 2^40 nm"))),
    }
}

/// `point(value)`: a coordinate pair of typed lengths.
pub(crate) fn coordinate(value: &Value, what: &str) -> Lower<Point> {
    match value {
        Value::Point(x, y) => {
            Ok(Point::new(nanometres(&Value::Scalar(x.clone()), what)?, nanometres(&Value::Scalar(y.clone()), what)?))
        }
        Value::Points(points) if points.len() == 2 => fail(format!("{what} dimensions require typed lengths")),
        _ => fail("expected a coordinate pair (x, y)"),
    }
}

/// `vertices(value)`: a list of coordinate pairs.
pub(crate) fn coordinates(value: &Value, what: &str) -> Lower<Vec<Point>> {
    match value {
        Value::Points(points) => {
            points.iter().map(|(x, y)| coordinate(&Value::Point(x.clone(), y.clone()), what)).collect()
        }
        Value::Point(..) => fail("expected a coordinate pair (x, y)"),
        _ => fail("expected a list of coordinate pairs"),
    }
}

/// `Enum(value)` for a text enum: Python's "is not a valid" error.
pub(crate) fn text_enum<T>(value: &Value, name: &str, parse: impl Fn(&str) -> Option<T>) -> Lower<T> {
    pyvalue::as_str(value)
        .and_then(parse)
        .ok_or_else(|| Fail::Value(format!("{} is not a valid {name}", pyvalue::repr(value))))
}

pub(crate) fn board_side(value: &Value) -> Lower<BoardSide> {
    text_enum(value, "BoardSide", BoardSide::parse)
}

/// Python's `str.strip()`.
pub(crate) fn strip(text: &str) -> &str {
    text.trim_matches(copper_json::python_isspace)
}

/// `MechanicalConnectorBinding`: a profile role bound to a component.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalConnectorBinding {
    pub role: String,
    pub reference: Value,
    pub anchor_pad: String,
    pub position: Point,
    pub rotation: Decimal,
    pub side: BoardSide,
    pub location: Location,
    pub footprint: Option<String>,
}

/// `MechanicalDesign`: the validated board mechanics.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalDesign {
    pub outline: BoardOutline,
    pub holes: Vec<MechanicalHole>,
    pub rule_overrides: IndexMap<String, i64>,
    pub connectors: Vec<MechanicalConnectorBinding>,
    pub keepouts: Vec<PlacementKeepout>,
    pub copper_keepouts: Vec<CopperKeepout>,
    pub profiles: Vec<MechanicalProfileInstance>,
    pub sources: Vec<MechanicalFeatureSource>,
    pub datums: Vec<BoardDatum>,
    pub boundary_edges: Vec<BoardEdge>,
    pub attachments: Vec<MechanicalAttachment>,
    pub body_overhangs: Vec<BodyOverhang>,
    pub component_heights: Vec<ComponentHeight>,
    pub assembly_envelopes: Vec<AssemblyEnvelope>,
    pub assembly_access: Vec<AssemblyAccess>,
    pub slots: Vec<MechanicalSlot>,
    pub references: Vec<MechanicalReference>,
    pub stackup: Option<MechanicalStackup>,
}

impl MechanicalDesign {
    /// Component references bound by connectors, attachments and assembly
    /// items, with the binding's location where it has one.
    pub fn bound_references(&self) -> Vec<(Value, Option<&Location>)> {
        let mut result: Vec<(Value, Option<&Location>)> = Vec::new();
        result.extend(self.connectors.iter().map(|c| (c.reference.clone(), Some(&c.location))));
        result.extend(
            self.attachments.iter().map(|a| (Value::Scalar(Scalar::Str(a.reference.clone())), Some(&a.location))),
        );
        result.extend(self.body_overhangs.iter().map(|o| (Value::Scalar(Scalar::Str(o.reference.clone())), None)));
        result.extend(self.component_heights.iter().map(|h| (h.reference.clone(), None)));
        result.extend(self.assembly_access.iter().map(|a| (a.reference.clone(), None)));
        result
    }

    /// `MechanicalDesign.__post_init__`: rules, references and board material.
    fn validate(&self) -> Lower<()> {
        let rules = DesignRules::with_overrides(self.rule_overrides.iter().map(|(k, v)| (k.as_str(), *v)))?;
        references::validate_reference_inventory(&self.references)?;
        let mut slot_ids: Vec<&str> = self.slots.iter().map(|slot| slot.id.as_str()).collect();
        slot_ids.sort_unstable();
        slot_ids.dedup();
        if slot_ids.len() != self.slots.len() {
            return fail("duplicate mechanical_slots identity");
        }
        validate_mechanical_holes(&self.outline, &self.holes, &self.slots, &rules)?;
        for hole in &self.holes {
            if hole.head_clearance_radius_nm != 0 {
                let shape = RoundedConvexShape::new(vec![hole.position], hole.head_clearance_radius_nm)?;
                if !shape_in_outline(&shape, &self.outline, 1)? {
                    return fail(format!(
                        "mechanical hole {} head clearance must fit board material",
                        copper_json::python_str_repr(&hole.id)
                    ));
                }
            }
        }
        Ok(())
    }
}

fn sorted_first<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    keys.into_iter().min()
}

/// Accumulated features while lowering the items of one block.
#[derive(Default)]
struct Builder<'a> {
    outline: Option<BoardOutline>,
    cutouts: Vec<BoardCutout>,
    holes: Vec<MechanicalHole>,
    rules: IndexMap<String, i64>,
    ids: IndexSet<String>,
    connectors: Vec<MechanicalConnectorBinding>,
    keepouts: Vec<PlacementKeepout>,
    copper_keepouts: Vec<CopperKeepout>,
    datum_items: Vec<&'a MechanicalItemDecl>,
    edge_items: Vec<&'a MechanicalItemDecl>,
    attachment_items: Vec<&'a MechanicalItemDecl>,
    assembly_items: Vec<&'a MechanicalItemDecl>,
    boundary_items: Vec<&'a MechanicalItemDecl>,
    slots: Vec<MechanicalSlot>,
    references: Vec<MechanicalReference>,
    path_options: Option<&'a IndexMap<String, Value>>,
}

const WHAT: &str = "mechanical";

fn length(value: &Value) -> Lower<i64> {
    nanometres(value, WHAT)
}

fn point(value: &Value) -> Lower<Point> {
    coordinate(value, WHAT)
}

impl<'a> Builder<'a> {
    fn item(&mut self, item: &'a MechanicalItemDecl) -> Lower<()> {
        let p = &item.parameters;
        if !item.name.is_empty() && !self.ids.insert(item.name.clone()) {
            return fail(format!("duplicate mechanical feature id {}", copper_json::python_str_repr(&item.name)));
        }
        let kind = item.kind.as_str();
        match kind {
            "datum" => {
                self.datum_items.push(item);
                return Ok(());
            }
            "edge" => {
                self.edge_items.push(item);
                return Ok(());
            }
            "attach" => {
                self.attachment_items.push(item);
                return Ok(());
            }
            "overhang" | "component_height" | "enclosure" | "assembly_access" => {
                self.assembly_items.push(item);
                return Ok(());
            }
            "reference" => {
                if self.references.len() >= 32 {
                    return fail("reference inventory exceeds 32 assets");
                }
                self.references.push(references::lower_reference(item)?);
                return Ok(());
            }
            "boundary" => {
                self.boundary_items.push(item);
                return Ok(());
            }
            _ => {}
        }
        if kind == "outline" && item.shape == "path" {
            if self.outline.is_some() || self.path_options.is_some() {
                return fail("exactly one board outline is allowed");
            }
            if p.keys().any(|key| key != "maximum_chord_error") {
                return fail("path outline only accepts maximum_chord_error");
            }
            self.path_options = Some(p);
            return Ok(());
        }
        let set = |names: &[&'static str]| names.to_vec();
        let (allowed, required): (Vec<&str>, Vec<&str>) = match (kind, item.shape.as_str()) {
            ("rules", _) => (DesignRules::FIELDS.to_vec(), Vec::new()),
            ("connector", _) => (
                set(&["component", "anchor_pad", "position", "rotation", "side", "footprint"]),
                set(&["component", "anchor_pad", "position", "rotation", "side"]),
            ),
            ("keepout" | "copper_keepout", shape) => {
                let mut allowed = if kind == "keepout" {
                    set(&["side", "maximum_height"])
                } else {
                    set(&["layers", "block_tracks", "block_vias", "block_pads", "block_zones", "block_footprints"])
                };
                let mut required = match shape {
                    "rectangle" => {
                        allowed.extend(["width", "height", "origin"]);
                        set(&["width", "height"])
                    }
                    "polygon" => {
                        allowed.push("vertices");
                        set(&["vertices"])
                    }
                    _ => return fail("keepouts require rectangle or polygon geometry"),
                };
                if kind == "copper_keepout" {
                    required.push("layers");
                }
                (allowed, required)
            }
            ("hole", _) => (set(&["position", "diameter", "head_clearance_radius"]), set(&["position", "diameter"])),
            ("slot", _) => (set(&["start", "end", "width"]), set(&["start", "end", "width"])),
            ("outline", "circle") => (set(&["center", "diameter", "maximum_chord_error"]), set(&["diameter"])),
            ("outline", "rectangle") => (set(&["origin", "width", "height"]), set(&["width", "height"])),
            ("outline", "rounded_rectangle") => (
                set(&["origin", "width", "height", "corner_radius", "maximum_chord_error"]),
                set(&["width", "height", "corner_radius"]),
            ),
            (_, "polygon") => (set(&["vertices"]), set(&["vertices"])),
            _ => {
                return fail(format!("unsupported {} shape {}", item.kind, copper_json::python_str_repr(&item.shape)));
            }
        };
        if let Some(key) = sorted_first(p.keys().map(String::as_str).filter(|key| !allowed.contains(key))) {
            return fail(format!("unknown mechanical property {}", copper_json::python_str_repr(key)));
        }
        if let Some(key) = sorted_first(required.iter().copied().filter(|key| !p.contains_key(*key))) {
            return fail(format!("missing mechanical property {}", copper_json::python_str_repr(key)));
        }
        match kind {
            "rules" => {
                let mut additions = IndexMap::new();
                for (name, value) in p {
                    additions.insert(format!("{name}_nm"), length(value)?);
                }
                if additions.keys().any(|key| self.rules.contains_key(key)) {
                    return fail("conflicting mechanical rule ownership");
                }
                self.rules.extend(additions);
            }
            "connector" => self.connector(item)?,
            "keepout" | "copper_keepout" => {
                let region = if item.shape == "rectangle" {
                    let (width, height) = (length(&p["width"])?, length(&p["height"])?);
                    let origin = p.get("origin").map(point).transpose()?;
                    BoardOutline::rectangle(width, height, origin)?
                } else {
                    BoardOutline::polygon(coordinates(&p["vertices"], WHAT)?)?
                };
                if kind == "keepout" {
                    let side = p.get("side").map(board_side).transpose()?;
                    let height = p.get("maximum_height").map(length).transpose()?;
                    self.keepouts.push(PlacementKeepout::new(&item.name, region, side, height)?);
                } else {
                    let Some(layers) = pyvalue::as_str(&p["layers"]) else {
                        return fail("copper keepout layers must be a comma-separated string");
                    };
                    let layers = layers
                        .split(',')
                        .map(|name| {
                            let name = strip(name);
                            CopperLayer::parse(name).ok_or_else(|| {
                                Fail::Value(format!(
                                    "{} is not a valid CopperLayer",
                                    copper_json::python_str_repr(name)
                                ))
                            })
                        })
                        .collect::<Lower<Vec<CopperLayer>>>()?;
                    let mut keepout = CopperKeepout {
                        id: item.name.clone(),
                        layers,
                        vertices: polygon_ring(&region.vertices)?,
                        block_tracks: true,
                        block_vias: true,
                        block_pads: false,
                        block_zones: true,
                        block_footprints: false,
                    };
                    for (key, value) in p.iter().filter(|(key, _)| key.starts_with("block_")) {
                        let Value::Scalar(Scalar::Bool(flag)) = value else {
                            return fail("copper keepout block properties must be booleans");
                        };
                        match key.as_str() {
                            "block_tracks" => keepout.block_tracks = *flag,
                            "block_vias" => keepout.block_vias = *flag,
                            "block_pads" => keepout.block_pads = *flag,
                            "block_zones" => keepout.block_zones = *flag,
                            _ => keepout.block_footprints = *flag,
                        }
                    }
                    keepout.validate()?;
                    self.copper_keepouts.push(keepout);
                }
            }
            "hole" => {
                let position = point(&p["position"])?;
                let diameter = length(&p["diameter"])?;
                let head = p.get("head_clearance_radius").map(length).transpose()?.unwrap_or(0);
                self.holes.push(MechanicalHole::new(&item.name, position, diameter, head)?);
            }
            "slot" => {
                let (start, end, width) = (point(&p["start"])?, point(&p["end"])?, length(&p["width"])?);
                self.slots.push(MechanicalSlot::new(&item.name, start, end, width)?);
            }
            "cutout" => self.cutouts.push(BoardCutout::new(&item.name, coordinates(&p["vertices"], WHAT)?)?),
            _ => {
                if self.outline.is_some() || self.path_options.is_some() {
                    return fail("exactly one board outline is allowed");
                }
                let chord_error = || p.get("maximum_chord_error").map(length).transpose();
                self.outline = Some(match item.shape.as_str() {
                    "circle" => {
                        let diameter = length(&p["diameter"])?;
                        let center = p.get("center").map(point).transpose()?;
                        let error = chord_error()?.unwrap_or(DEFAULT_CHORD_ERROR_NM);
                        BoardOutline::circle(diameter, center, error)?
                    }
                    "rectangle" => {
                        let (width, height) = (length(&p["width"])?, length(&p["height"])?);
                        BoardOutline::rectangle(width, height, p.get("origin").map(point).transpose()?)?
                    }
                    "rounded_rectangle" => {
                        let (width, height) = (length(&p["width"])?, length(&p["height"])?);
                        let radius = length(&p["corner_radius"])?;
                        let origin = p.get("origin").map(point).transpose()?.unwrap_or(Point::new(0, 0));
                        let error = chord_error()?.unwrap_or(DEFAULT_CHORD_ERROR_NM);
                        BoardOutline::rounded_rectangle(width, height, radius, origin, error)?
                    }
                    _ => BoardOutline::polygon(coordinates(&p["vertices"], WHAT)?)?,
                });
            }
        }
        Ok(())
    }

    fn connector(&mut self, item: &MechanicalItemDecl) -> Lower<()> {
        let p = &item.parameters;
        let anchor = &p["anchor_pad"];
        let valid_anchor = match anchor {
            Value::Scalar(Scalar::Str(text)) => !text.is_empty(),
            Value::Scalar(Scalar::Int(_)) => true,
            _ => false,
        };
        if !valid_anchor {
            return fail("connector anchor_pad must name a physical pad");
        }
        let angle = match &p["rotation"] {
            Value::Scalar(scalar @ (Scalar::Int(_) | Scalar::Float(_))) => pyvalue::decimal(scalar),
            _ => return fail("connector rotation must be numeric degrees"),
        };
        let Some(angle) = angle else { return fail("connector rotation must be finite") };
        let footprint = match p.get("footprint") {
            None => None,
            Some(Value::Scalar(Scalar::Str(text))) if !text.is_empty() => Some(text.clone()),
            Some(_) => return fail("connector footprint must be a nonempty library identifier"),
        };
        let component = &p["component"];
        if self.connectors.iter().any(|c| pyvalue::eq(&c.reference, component)) {
            return fail("multiple profile connector roles bind the same component");
        }
        let position = point(&p["position"])?;
        let rotation = angle.rem(&Decimal::from_i64(360)).map_err(|failure| Fail::Rust(failure.to_string()))?;
        let side = board_side(&p["side"])?;
        self.connectors.push(MechanicalConnectorBinding {
            role: item.name.clone(),
            reference: component.clone(),
            anchor_pad: pyvalue::text(anchor),
            position,
            rotation,
            side,
            location: item.location.clone(),
            footprint,
        });
        Ok(())
    }

    /// The whole-block checks, after every item lowered.
    fn finish(
        self,
        sources: Vec<MechanicalFeatureSource>,
        instances: Vec<MechanicalProfileInstance>,
        stackup: Option<MechanicalStackup>,
    ) -> Lower<MechanicalDesign> {
        let outline = if let Some(options) = self.path_options {
            let mut primitives = Vec::new();
            for item in &self.boundary_items {
                let p = &item.parameters;
                let required: &[&str] = match item.shape.as_str() {
                    "line" => &["start", "end"],
                    "arc" => &["start", "mid", "end"],
                    _ => &[],
                };
                if required.is_empty() || p.len() != required.len() || !required.iter().all(|key| p.contains_key(*key))
                {
                    return fail("boundary primitives require line start/end or arc start/mid/end");
                }
                primitives.push(if item.shape == "line" {
                    let (start, end) = (point(&p["start"])?, point(&p["end"])?);
                    BoundarySegment::line(&item.name, start, end)?
                } else {
                    let (start, mid, end) = (point(&p["start"])?, point(&p["mid"])?, point(&p["end"])?);
                    BoundarySegment::arc(&item.name, start, mid, end)?
                });
            }
            let error = options.get("maximum_chord_error").map(length).transpose()?.unwrap_or(DEFAULT_CHORD_ERROR_NM);
            Some(BoardOutline::from_path(copper_physical::BoardBoundaryPath::new(primitives, error)?)?)
        } else if !self.boundary_items.is_empty() {
            return fail("boundary declarations require an outline path");
        } else {
            self.outline
        };
        let Some(outline) = outline else { return fail("mechanical block requires exactly one outline") };
        let datums = anchors::resolve_datums(&self.datum_items)?;
        let mut edges = anchors::resolve_edges(&self.edge_items, &outline)?;
        if let Some(path) = &outline.boundary_path {
            for segment in &path.segments {
                if let BoundarySegment::Line { id, start, end } = segment {
                    edges.push(BoardEdge::new(id, *start, *end)?);
                }
            }
            let ids: IndexSet<&str> = edges.iter().map(|edge| edge.id.as_str()).collect();
            if ids.len() != edges.len() {
                return fail("duplicate boundary edge IDs");
            }
        }
        let attachments = anchors::resolve_attachments(&self.attachment_items, &datums, &edges, &outline)?;
        let with_cutouts = outline.with_cutouts(self.cutouts.clone())?;
        let assembly = assembly::lower_assembly(&self.assembly_items, &with_cutouts, &edges)?;
        let outline = outline.with_cutouts(self.cutouts)?;
        let design = MechanicalDesign {
            outline,
            holes: self.holes,
            rule_overrides: self.rules,
            connectors: self.connectors,
            keepouts: self.keepouts,
            copper_keepouts: self.copper_keepouts,
            profiles: instances,
            sources,
            datums,
            boundary_edges: edges,
            attachments,
            body_overhangs: assembly.overhangs,
            component_heights: assembly.heights,
            assembly_envelopes: assembly.enclosures,
            assembly_access: assembly.access,
            slots: self.slots,
            references: self.references,
            stackup,
        };
        design.validate()?;
        Ok(design)
    }
}

/// `lower_mechanical`: the board's `mechanical` block, if it has one.
pub fn lower_mechanical(
    document: &Document,
    profiles: &IndexMap<String, MechanicalProfileDefinition>,
) -> Result<Option<MechanicalDesign>, CopperScriptError> {
    let blocks: Vec<_> = document
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Mechanical(block) => Some(block),
            _ => None,
        })
        .collect();
    let Some(block) = blocks.first() else { return Ok(None) };
    if blocks.len() != 1 {
        return Err(CopperScriptError::new(
            "MEC001",
            "only one mechanical block is allowed",
            blocks[1].location.clone(),
        ));
    }
    let stackups: Vec<_> = block
        .items
        .iter()
        .filter_map(|item| match item {
            MechanicalItem::Stackup(stackup) => Some(stackup),
            _ => None,
        })
        .collect();
    if stackups.len() > 1 {
        return Err(CopperScriptError::new("MEC006", "only one stackup is allowed", stackups[1].location.clone()));
    }
    let stackup = stackups.first().map(|declaration| lower_stackup(declaration)).transpose()?;
    let items: Vec<&MechanicalItem> =
        block.items.iter().filter(|item| !matches!(item, MechanicalItem::Stackup(_))).collect();
    let (items, sources, instances) = expand_mechanical_items(&items, profiles)?;
    let mut builder = Builder::default();
    for item in &items {
        builder.item(item).map_err(|failure| failure.error("MEC002", &item.location))?;
    }
    builder.finish(sources, instances, stackup).map(Some).map_err(|failure| failure.error("MEC003", &block.location))
}

fn point_json(point: &Point) -> Json {
    Json::Object(Object::from([
        ("x_nm".to_owned(), Json::from(point.x_nm)),
        ("y_nm".to_owned(), Json::from(point.y_nm)),
    ]))
}

fn points_json(points: &[Point]) -> Json {
    Json::Array(points.iter().map(point_json).collect())
}

fn text_json(value: &str) -> Json {
    Json::from(value)
}

fn optional_text(value: Option<&str>) -> Json {
    value.map_or(Json::Null, Json::from)
}

/// A bound component as JSON; non-text references never reach serialization.
fn reference_json(value: &Value) -> Json {
    Json::from(pyvalue::text(value).as_str())
}

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn location_json(location: &Location) -> Json {
    object(vec![
        ("filename", text_json(&location.filename)),
        ("line", Json::from(location.line as i64)),
        ("column", Json::from(location.column as i64)),
    ])
}

/// The `mechanical` section of `board_to_dict(design)`.
pub fn mechanical_document(m: &MechanicalDesign) -> Json {
    let outline = &m.outline;
    let circle = outline.circular_boundary.map_or(Json::Null, |circle| {
        object(vec![
            ("center", point_json(&circle.center)),
            ("radius_nm", Json::from(circle.radius_nm)),
            ("maximum_chord_error_nm", Json::from(circle.maximum_chord_error_nm)),
        ])
    });
    let boundary_path = outline.boundary_path.as_ref().map_or(Json::Null, |path| {
        let segments = path
            .segments
            .iter()
            .map(|segment| {
                let mut entries = vec![
                    ("id", text_json(segment.id())),
                    ("kind", text_json(if matches!(segment, BoundarySegment::Arc { .. }) { "arc" } else { "line" })),
                    ("start", point_json(&segment.start())),
                    ("end", point_json(&segment.end())),
                ];
                if let BoundarySegment::Arc { mid, .. } = segment {
                    entries.push(("mid", point_json(mid)));
                }
                object(entries)
            })
            .collect();
        object(vec![
            ("maximum_chord_error_nm", Json::from(path.maximum_chord_error_nm)),
            ("segments", Json::Array(segments)),
        ])
    });
    let list = |items: Vec<Json>| Json::Array(items);
    object(vec![
        (
            "outline",
            object(vec![
                ("vertices", points_json(&outline.vertices)),
                ("circle", circle),
                (
                    "cutouts",
                    list(
                        outline
                            .cutouts
                            .iter()
                            .map(|c| object(vec![("id", text_json(&c.id)), ("vertices", points_json(&c.vertices))]))
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "holes",
            list(
                m.holes
                    .iter()
                    .map(|h| {
                        object(vec![
                            ("id", text_json(&h.id)),
                            ("position", point_json(&h.position)),
                            ("diameter_nm", Json::from(h.diameter_nm)),
                            ("head_clearance_radius_nm", Json::from(h.head_clearance_radius_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "datums",
            list(
                m.datums
                    .iter()
                    .map(|d| {
                        object(vec![
                            ("id", text_json(&d.id)),
                            ("position", point_json(&d.position)),
                            ("relative_to", optional_text(d.relative_to.as_deref())),
                            ("offset", point_json(&d.offset)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "boundary_edges",
            list(
                m.boundary_edges
                    .iter()
                    .map(|e| {
                        object(vec![
                            ("id", text_json(&e.id)),
                            ("start", point_json(&e.start)),
                            ("end", point_json(&e.end)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "attachments",
            list(
                m.attachments
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", text_json(&a.id)),
                            ("reference", text_json(&a.reference)),
                            ("target", optional_text(a.target.as_deref())),
                            ("position", point_json(&a.position)),
                            ("offset", point_json(&a.offset)),
                            ("anchor", text_json(&a.anchor)),
                            ("anchor_pad", optional_text(a.anchor_pad.as_deref())),
                            ("anchor_point", a.anchor_point.as_ref().map_or(Json::Null, point_json)),
                            ("rotation", Json::from(a.rotation.to_sci_string().as_str())),
                            ("side", text_json(a.side.value())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "rule_overrides",
            Json::Object(m.rule_overrides.iter().map(|(key, value)| (key.clone(), Json::from(*value))).collect()),
        ),
        (
            "slots",
            list(
                m.slots
                    .iter()
                    .map(|s| {
                        object(vec![
                            ("id", text_json(&s.id)),
                            ("start", point_json(&s.start)),
                            ("end", point_json(&s.end)),
                            ("width_nm", Json::from(s.width_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("references", list(m.references.iter().map(reference_scene).collect())),
        ("boundary_path", boundary_path),
        (
            "body_overhangs",
            list(
                m.body_overhangs
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", text_json(&a.id)),
                            ("reference", text_json(&a.reference)),
                            ("edge", text_json(&a.edge)),
                            ("start_nm", Json::from(a.start_nm)),
                            ("end_nm", Json::from(a.end_nm)),
                            ("distance_nm", Json::from(a.distance_nm)),
                            ("reason", text_json(&a.reason)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "component_heights",
            list(
                m.component_heights
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", text_json(&a.id)),
                            ("reference", reference_json(&a.reference)),
                            ("height_nm", Json::from(a.height_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "assembly_envelopes",
            list(
                m.assembly_envelopes
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", text_json(&a.id)),
                            ("vertices", points_json(&a.outline.vertices)),
                            ("side", text_json(a.side.value())),
                            ("maximum_height_nm", Json::from(a.maximum_height_nm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "assembly_access",
            list(
                m.assembly_access
                    .iter()
                    .map(|a| {
                        object(vec![
                            ("id", text_json(&a.id)),
                            ("reference", reference_json(&a.reference)),
                            ("vertices", points_json(&a.outline.vertices)),
                            ("side", text_json(&a.side)),
                            ("purpose", text_json(&a.purpose)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "connectors",
            list(
                m.connectors
                    .iter()
                    .map(|c| {
                        object(vec![
                            ("role", text_json(&c.role)),
                            ("reference", reference_json(&c.reference)),
                            ("anchor_pad", text_json(&c.anchor_pad)),
                            ("position", point_json(&c.position)),
                            ("rotation", Json::from(c.rotation.to_sci_string().as_str())),
                            ("side", text_json(c.side.value())),
                            ("footprint", optional_text(c.footprint.as_deref())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "keepouts",
            list(
                m.keepouts
                    .iter()
                    .map(|k| {
                        object(vec![
                            ("name", text_json(&k.name)),
                            ("vertices", points_json(&k.outline.vertices)),
                            ("side", optional_text(k.side.map(BoardSide::value))),
                            ("maximum_height_nm", k.maximum_component_height_nm.map_or(Json::Null, Json::from)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "copper_keepouts",
            list(
                m.copper_keepouts
                    .iter()
                    .map(|k| {
                        object(vec![
                            ("id", text_json(&k.id)),
                            ("layers", list(k.layers.iter().map(|layer| text_json(layer.value())).collect())),
                            ("vertices", points_json(&k.vertices)),
                            ("block_tracks", Json::Bool(k.block_tracks)),
                            ("block_vias", Json::Bool(k.block_vias)),
                            ("block_pads", Json::Bool(k.block_pads)),
                            ("block_zones", Json::Bool(k.block_zones)),
                            ("block_footprints", Json::Bool(k.block_footprints)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("provenance", provenance(m)),
    ])
}

/// `mechanical_provenance`.
fn provenance(m: &MechanicalDesign) -> Json {
    let profiles = m
        .profiles
        .iter()
        .map(|p| {
            object(vec![
                ("name", text_json(&p.name)),
                ("profile", text_json(&p.profile)),
                (
                    "bindings",
                    Json::Object(p.bindings.iter().map(|(key, value)| (key.clone(), text_json(value))).collect()),
                ),
                ("source", location_json(&p.location)),
            ])
        })
        .collect();
    let features = m
        .sources
        .iter()
        .map(|s| {
            object(vec![
                ("kind", text_json(&s.kind)),
                ("name", text_json(&s.name)),
                ("source", location_json(&s.location)),
                ("profile", optional_text(s.profile.as_deref())),
                ("instance", optional_text(s.instance.as_deref())),
                ("read_only", Json::Bool(s.profile.is_some())),
            ])
        })
        .collect();
    object(vec![("profiles", Json::Array(profiles)), ("features", Json::Array(features))])
}

//! Physical-board values of a `copperscript.state/v1` file (`pcbir.state_codec`
//! decoding into the `pcbir.physical` dataclasses).
//!
//! Python's `decode` constructs each saved dataclass from its saved fields,
//! so the field names and their defaults are the Python ones; a state value
//! this IR does not model (zone fills, via technologies) reports
//! [`Error::NotImplemented`] rather than a guess. Dataclass validation runs as
//! Python's `__post_init__` does and its `ValueError` is the message callers
//! print.

use indexmap::IndexMap;
use num_traits::ToPrimitive;

use copper_lang::mechanical::stackup::StackupLayer;
use copper_lang::silkscreen::{Artwork, ArtworkMaterial, Graphic};
use copper_physical::footprint::{
    FootprintGraphic, FootprintLayer, FootprintPad, InternalPadGroup, PadKind, PadShape, PhysicalFootprint, Size,
    ZoneConnection,
};
use copper_physical::{
    BoardBoundaryPath, BoardCutout, BoardDatum, BoardEdge, BoardOutline, BoardSide, BoundarySegment,
    CircularBoardBoundary, CopperKeepout, CopperLayer, DesignRules, MechanicalHole, MechanicalSlot, PlacementKeepout,
    Point,
};
use copper_state::StateValue;
use copper_units::Decimal;

use crate::board::*;
use crate::{Error, Result, not_implemented};

/// The saved fields of a `pcbir` dataclass, checked against the names Python
/// accepts (`{type} has no init field(s) [...]`).
pub struct Fields<'a> {
    pub type_name: &'a str,
    fields: &'a IndexMap<String, StateValue>,
}

impl<'a> Fields<'a> {
    /// The fields of a dataclass value whose `$type` is `type_name`.
    pub fn of(value: &'a StateValue, type_name: &str) -> Result<Fields<'a>> {
        match value {
            StateValue::Dataclass { name, fields } if name == type_name => Ok(Fields { type_name: name, fields }),
            StateValue::Dataclass { name, .. } => {
                not_implemented(&format!("a state value of type {name} where {type_name} was expected"))
            }
            _ => not_implemented(&format!("a state value that is not a {type_name} dataclass")),
        }
    }

    /// Python's check that every saved field is an init field of the class.
    pub fn allow(&self, names: &[&str]) -> Result<()> {
        let mut unknown: Vec<&str> =
            self.fields.keys().map(String::as_str).filter(|name| !names.contains(name)).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        unknown.sort_unstable();
        let listed: Vec<String> = unknown.iter().map(|name| copper_json::python_str_repr(name)).collect();
        Err(Error::Value(format!("{} has no init field(s) [{}]", self.type_name, listed.join(", "))))
    }

    /// A saved field; `None` when the file omits it (Python then uses the default).
    pub fn get(&self, name: &str) -> Option<&'a StateValue> {
        self.fields.get(name)
    }

    /// A field without a Python default. Python's `cls(**fields)` raises a
    /// `TypeError` (a crash, not a caught error) without it.
    pub fn required(&self, name: &str) -> Result<&'a StateValue> {
        self.fields.get(name).ok_or_else(|| {
            Error::NotImplemented(format!("a saved {} without its required field {name:?}", self.type_name))
        })
    }
}

/// `pcbir.<module>:<Name>`.
pub fn type_name(value: &StateValue) -> Option<&str> {
    match value {
        StateValue::Dataclass { name, .. } => Some(name),
        _ => None,
    }
}

pub fn integer(value: &StateValue) -> Result<i64> {
    match value {
        StateValue::Int(number) => number.to_i64().ok_or_else(|| Error::NotImplemented("an integer beyond i64".into())),
        StateValue::Bool(flag) => Ok(i64::from(*flag)),
        _ => not_implemented("a state integer that is not an int"),
    }
}

pub fn boolean(value: &StateValue) -> Result<bool> {
    match value {
        StateValue::Bool(flag) => Ok(*flag),
        StateValue::Int(number) => Ok(!num_traits::Zero::is_zero(number)),
        _ => not_implemented("a state flag that is not a bool"),
    }
}

pub fn text(value: &StateValue) -> Result<String> {
    match value {
        StateValue::Str(text) => Ok(text.clone()),
        _ => not_implemented("a state string that is not a str"),
    }
}

pub fn optional<T>(value: Option<&StateValue>, read: impl FnOnce(&StateValue) -> Result<T>) -> Result<Option<T>> {
    match value {
        None | Some(StateValue::Null) => Ok(None),
        Some(value) => read(value).map(Some),
    }
}

/// `int | None` with its Python default.
pub fn optional_integer(value: Option<&StateValue>) -> Result<Option<i64>> {
    optional(value, integer)
}

pub fn optional_text(value: Option<&StateValue>) -> Result<Option<String>> {
    optional(value, text)
}

/// `Decimal | int | float | str` fields that Python normalizes to `Decimal`.
pub fn decimal(value: &StateValue) -> Result<Decimal> {
    match value {
        StateValue::Decimal(number) => Ok(number.clone()),
        StateValue::Int(number) => Decimal::parse(&number.to_string()).map_err(|e| Error::Value(e.to_string())),
        StateValue::Str(text) => Decimal::parse(text).map_err(|e| Error::Value(e.to_string())),
        StateValue::Float(number) => {
            Decimal::parse(&copper_json::python_float_repr(*number)).map_err(|e| Error::Value(e.to_string()))
        }
        _ => not_implemented("a state decimal that is not a number"),
    }
}

pub fn items(value: &StateValue) -> Result<&[StateValue]> {
    match value {
        StateValue::Tuple(items) | StateValue::List(items) => Ok(items),
        _ => not_implemented("a state sequence that is not a tuple or list"),
    }
}

/// A `frozenset`/`set` (or sequence) in the saved order.
pub fn set_items(value: &StateValue) -> Result<&[StateValue]> {
    match value {
        StateValue::Tuple(items) | StateValue::List(items) | StateValue::FrozenSet(items) | StateValue::Set(items) => {
            Ok(items)
        }
        _ => not_implemented("a state set that is not a set"),
    }
}

pub fn sequence<T>(value: Option<&StateValue>, read: impl Fn(&StateValue) -> Result<T>) -> Result<Vec<T>> {
    match value {
        None => Ok(Vec::new()),
        Some(value) => items(value)?.iter().map(read).collect(),
    }
}

pub fn texts(value: Option<&StateValue>) -> Result<Vec<String>> {
    sequence(value, text)
}

/// A `$map` of string keys.
pub fn string_map<T>(
    value: Option<&StateValue>,
    read: impl Fn(&StateValue) -> Result<T>,
) -> Result<IndexMap<String, T>> {
    let Some(value) = value else { return Ok(IndexMap::new()) };
    let StateValue::Map { entries, .. } = value else { return not_implemented("a state mapping that is not a map") };
    let mut result = IndexMap::new();
    for (key, item) in entries {
        result.insert(text(key)?, read(item)?);
    }
    Ok(result)
}

/// An enum member by its saved value; Python: `'x' is not a valid Name`.
pub fn enumeration<T: Copy>(value: &StateValue, name: &str, parse: impl Fn(&str) -> Option<T>) -> Result<T> {
    let saved = match value {
        StateValue::Enum { value, .. } => value.as_ref(),
        other => other,
    };
    let StateValue::Str(member) = saved else { return not_implemented(&format!("a {name} that is not a string")) };
    parse(member).ok_or_else(|| Error::Value(format!("{} is not a valid {name}", copper_json::python_str_repr(member))))
}

pub fn layer(value: &StateValue) -> Result<CopperLayer> {
    enumeration(value, "CopperLayer", CopperLayer::parse)
}

pub fn side(value: &StateValue) -> Result<BoardSide> {
    enumeration(value, "BoardSide", BoardSide::parse)
}

pub fn point(value: &StateValue) -> Result<Point> {
    let fields = Fields::of(value, "pcbir.physical:Point")?;
    fields.allow(&["x_nm", "y_nm"])?;
    Ok(Point::new(integer(fields.required("x_nm")?)?, integer(fields.required("y_nm")?)?))
}

pub fn points(value: Option<&StateValue>) -> Result<Vec<Point>> {
    sequence(value, point)
}

pub fn pad_reference(value: &StateValue) -> Result<PadReference> {
    let fields = Fields::of(value, "pcbir.physical:PadReference")?;
    fields.allow(&["component", "pad"])?;
    Ok(PadReference { component: text(fields.required("component")?)?, pad: text(fields.required("pad")?)? })
}

fn size(value: &StateValue) -> Result<Size> {
    let fields = Fields::of(value, "pcbir.physical:Size")?;
    fields.allow(&["width_nm", "height_nm"])?;
    Size::new(integer(fields.required("width_nm")?)?, integer(fields.required("height_nm")?)?).map_err(Error::Value)
}

fn ring(value: &StateValue) -> Result<Vec<Point>> {
    let fields = Fields::of(value, "pcbir.physical:PolygonRing")?;
    fields.allow(&["vertices"])?;
    copper_physical::polygon_ring(&points(Some(fields.required("vertices")?))?).map_err(Error::Value)
}

/// `PolygonWithHoles`: the outer ring and its holes.
fn polygon_with_holes(value: &StateValue) -> Result<(Vec<Point>, Vec<Vec<Point>>)> {
    let fields = Fields::of(value, "pcbir.physical:PolygonWithHoles")?;
    fields.allow(&["outer", "holes"])?;
    Ok((ring(fields.required("outer")?)?, sequence(fields.get("holes"), ring)?))
}

pub fn track(value: &StateValue) -> Result<TrackSegment> {
    let fields = Fields::of(value, "pcbir.physical:TrackSegment")?;
    fields.allow(&["net", "start", "end", "width_nm", "layer"])?;
    let track = TrackSegment {
        net: text(fields.required("net")?)?,
        start: point(fields.required("start")?)?,
        end: point(fields.required("end")?)?,
        width_nm: integer(fields.required("width_nm")?)?,
        layer: layer(fields.required("layer")?)?,
    };
    if track.start == track.end {
        return Err(Error::Value("track segment cannot have zero length".to_owned()));
    }
    if track.width_nm <= 0 {
        return Err(Error::Value("track width must be positive".to_owned()));
    }
    Ok(track)
}

pub fn via(value: &StateValue) -> Result<Via> {
    let fields = Fields::of(value, "pcbir.physical:Via")?;
    fields.allow(&["net", "position", "size_nm", "drill_nm", "from_layer", "to_layer", "technology", "finish"])?;
    let via = Via {
        net: text(fields.required("net")?)?,
        position: point(fields.required("position")?)?,
        size_nm: integer(fields.required("size_nm")?)?,
        drill_nm: integer(fields.required("drill_nm")?)?,
        from_layer: fields.get("from_layer").map(layer).transpose()?.unwrap_or(CopperLayer::Front),
        to_layer: fields.get("to_layer").map(layer).transpose()?.unwrap_or(CopperLayer::Back),
        technology: optional_text(fields.get("technology"))?,
        finish: fields.get("finish").map(text).transpose()?.unwrap_or_else(|| "standard".to_owned()),
    };
    if via.size_nm <= 0 || via.drill_nm <= 0 {
        return Err(Error::Value("via size and drill must be positive".to_owned()));
    }
    if via.drill_nm >= via.size_nm {
        return Err(Error::Value("via drill must be smaller than via size".to_owned()));
    }
    if via.from_layer == via.to_layer {
        return Err(Error::Value("a via must connect distinct copper layers".to_owned()));
    }
    if !["standard", "filled-capped"].contains(&via.finish.as_str()) {
        return Err(Error::Value("unsupported via finish".to_owned()));
    }
    Ok(via)
}

fn copper_keepout(value: &StateValue) -> Result<CopperKeepout> {
    let fields = Fields::of(value, "pcbir.physical:CopperKeepout")?;
    fields.allow(&[
        "id",
        "layers",
        "outline",
        "block_tracks",
        "block_vias",
        "block_pads",
        "block_zones",
        "block_footprints",
    ])?;
    let (vertices, holes) = polygon_with_holes(fields.required("outline")?)?;
    if !holes.is_empty() {
        return not_implemented("a copper keepout outline with holes");
    }
    let flag = |name: &str, default: bool| fields.get(name).map(boolean).transpose().map(|v| v.unwrap_or(default));
    let keepout = CopperKeepout {
        id: text(fields.required("id")?)?,
        layers: sequence(Some(fields.required("layers")?), layer)?,
        vertices,
        block_tracks: flag("block_tracks", true)?,
        block_vias: flag("block_vias", true)?,
        block_pads: flag("block_pads", false)?,
        block_zones: flag("block_zones", true)?,
        block_footprints: flag("block_footprints", false)?,
    };
    keepout.validate().map_err(Error::Value)?;
    Ok(keepout)
}

fn board_outline(value: &StateValue) -> Result<BoardOutline> {
    let fields = Fields::of(value, "pcbir.physical:BoardOutline")?;
    fields.allow(&["vertices", "cutouts", "circular_boundary", "boundary_path"])?;
    let cutouts = sequence(fields.get("cutouts"), |item| {
        let fields = Fields::of(item, "pcbir.physical:BoardCutout")?;
        fields.allow(&["id", "vertices"])?;
        BoardCutout::new(&text(fields.required("id")?)?, points(Some(fields.required("vertices")?))?)
            .map_err(Error::Value)
    })?;
    let circular = optional(fields.get("circular_boundary"), |item| {
        let fields = Fields::of(item, "pcbir.physical:CircularBoardBoundary")?;
        fields.allow(&["center", "radius_nm", "maximum_chord_error_nm"])?;
        let chord = fields.get("maximum_chord_error_nm").map(integer).transpose()?;
        CircularBoardBoundary::new(
            point(fields.required("center")?)?,
            integer(fields.required("radius_nm")?)?,
            chord.unwrap_or(copper_physical::DEFAULT_CHORD_ERROR_NM),
        )
        .map_err(Error::Value)
    })?;
    let path = optional(fields.get("boundary_path"), |item| {
        let fields = Fields::of(item, "pcbir.physical:BoardBoundaryPath")?;
        fields.allow(&["segments", "maximum_chord_error_nm"])?;
        let segments = sequence(Some(fields.required("segments")?), |segment| match type_name(segment) {
            Some("pcbir.physical:BoundaryLine") => {
                let fields = Fields::of(segment, "pcbir.physical:BoundaryLine")?;
                fields.allow(&["id", "start", "end"])?;
                BoundarySegment::line(
                    &text(fields.required("id")?)?,
                    point(fields.required("start")?)?,
                    point(fields.required("end")?)?,
                )
                .map_err(Error::Value)
            }
            Some("pcbir.physical:BoundaryArc") => {
                let fields = Fields::of(segment, "pcbir.physical:BoundaryArc")?;
                fields.allow(&["id", "start", "mid", "end"])?;
                BoundarySegment::arc(
                    &text(fields.required("id")?)?,
                    point(fields.required("start")?)?,
                    point(fields.required("mid")?)?,
                    point(fields.required("end")?)?,
                )
                .map_err(Error::Value)
            }
            _ => not_implemented("a boundary path primitive that is not a line or arc"),
        })?;
        let chord = fields.get("maximum_chord_error_nm").map(integer).transpose()?;
        BoardBoundaryPath::new(segments, chord.unwrap_or(copper_physical::DEFAULT_CHORD_ERROR_NM)).map_err(Error::Value)
    })?;
    BoardOutline::new(points(Some(fields.required("vertices")?))?, cutouts, circular, path).map_err(Error::Value)
}

fn footprint_layer(value: &StateValue) -> Result<FootprintLayer> {
    enumeration(value, "FootprintLayer", |text| match text {
        "silkscreen" => Some(FootprintLayer::Silkscreen),
        "fabrication" => Some(FootprintLayer::Fabrication),
        "courtyard" => Some(FootprintLayer::Courtyard),
        "adhesive" => Some(FootprintLayer::Adhesive),
        "documentation" => Some(FootprintLayer::Documentation),
        "solder_mask" => Some(FootprintLayer::SolderMask),
        "solder_paste" => Some(FootprintLayer::SolderPaste),
        _ => None,
    })
}

fn footprint_graphic(value: &StateValue) -> Result<FootprintGraphic> {
    let filled = |fields: &Fields| fields.get("filled").map(boolean).transpose().map(|v| v.unwrap_or(false));
    let graphic = match type_name(value) {
        Some("pcbir.physical:FootprintLine") => {
            let fields = Fields::of(value, "pcbir.physical:FootprintLine")?;
            fields.allow(&["start", "end", "width_nm", "layer"])?;
            FootprintGraphic::Line {
                start: point(fields.required("start")?)?,
                end: point(fields.required("end")?)?,
                width_nm: integer(fields.required("width_nm")?)?,
                layer: footprint_layer(fields.required("layer")?)?,
            }
        }
        Some("pcbir.physical:FootprintRectangle") => {
            let fields = Fields::of(value, "pcbir.physical:FootprintRectangle")?;
            fields.allow(&["start", "end", "width_nm", "layer", "filled"])?;
            FootprintGraphic::Rectangle {
                start: point(fields.required("start")?)?,
                end: point(fields.required("end")?)?,
                width_nm: integer(fields.required("width_nm")?)?,
                layer: footprint_layer(fields.required("layer")?)?,
                filled: filled(&fields)?,
            }
        }
        Some("pcbir.physical:FootprintCircle") => {
            let fields = Fields::of(value, "pcbir.physical:FootprintCircle")?;
            fields.allow(&["center", "end", "width_nm", "layer", "filled"])?;
            FootprintGraphic::Circle {
                center: point(fields.required("center")?)?,
                end: point(fields.required("end")?)?,
                width_nm: integer(fields.required("width_nm")?)?,
                layer: footprint_layer(fields.required("layer")?)?,
                filled: filled(&fields)?,
            }
        }
        Some("pcbir.physical:FootprintArc") => {
            let fields = Fields::of(value, "pcbir.physical:FootprintArc")?;
            fields.allow(&["start", "midpoint", "end", "width_nm", "layer"])?;
            FootprintGraphic::Arc {
                start: point(fields.required("start")?)?,
                midpoint: point(fields.required("midpoint")?)?,
                end: point(fields.required("end")?)?,
                width_nm: integer(fields.required("width_nm")?)?,
                layer: footprint_layer(fields.required("layer")?)?,
            }
        }
        Some("pcbir.physical:FootprintPolygon") => {
            let fields = Fields::of(value, "pcbir.physical:FootprintPolygon")?;
            fields.allow(&["points", "width_nm", "layer", "filled"])?;
            FootprintGraphic::Polygon {
                points: points(Some(fields.required("points")?))?,
                width_nm: integer(fields.required("width_nm")?)?,
                layer: footprint_layer(fields.required("layer")?)?,
                filled: filled(&fields)?,
            }
        }
        _ => return not_implemented("a footprint graphic of an unknown kind"),
    };
    graphic.validated().map_err(Error::Value)
}

fn footprint_pad(value: &StateValue) -> Result<FootprintPad> {
    let fields = Fields::of(value, "pcbir.physical:FootprintPad")?;
    fields.allow(&[
        "number",
        "position",
        "size",
        "kind",
        "shape",
        "rotation_degrees",
        "drill",
        "roundrect_ratio_ppm",
        "has_solder_mask",
        "has_solder_paste",
        "zone_connection",
        "heatsink",
        "remove_unused_layers",
        "connector_contact",
    ])?;
    let flag = |name: &str, default: bool| fields.get(name).map(boolean).transpose().map(|v| v.unwrap_or(default));
    let pad = FootprintPad {
        number: text(fields.required("number")?)?,
        position: point(fields.required("position")?)?,
        size: size(fields.required("size")?)?,
        kind: match fields.get("kind") {
            None => PadKind::Smd,
            Some(kind) => enumeration(kind, "PadKind", |text| match text {
                "smd" => Some(PadKind::Smd),
                "aperture" => Some(PadKind::Aperture),
                "through_hole" => Some(PadKind::ThroughHole),
                "non_plated_through_hole" => Some(PadKind::NonPlatedThroughHole),
                _ => None,
            })?,
        },
        shape: match fields.get("shape") {
            None => PadShape::Rectangle,
            Some(shape) => enumeration(shape, "PadShape", |text| match text {
                "circle" => Some(PadShape::Circle),
                "oval" => Some(PadShape::Oval),
                "rectangle" => Some(PadShape::Rectangle),
                "roundrect" => Some(PadShape::Roundrect),
                _ => None,
            })?,
        },
        rotation_degrees: fields.get("rotation_degrees").map(decimal).transpose()?.unwrap_or_else(Decimal::zero),
        drill: optional(fields.get("drill"), size)?,
        roundrect_ratio_ppm: fields.get("roundrect_ratio_ppm").map(integer).transpose()?.unwrap_or(250_000),
        has_solder_mask: flag("has_solder_mask", true)?,
        has_solder_paste: flag("has_solder_paste", true)?,
        zone_connection: optional(fields.get("zone_connection"), |item| {
            enumeration(item, "ZoneConnection", |text| match text {
                "thermal" => Some(ZoneConnection::Thermal),
                "solid" => Some(ZoneConnection::Solid),
                "none" => Some(ZoneConnection::None),
                "tht_thermal" => Some(ZoneConnection::ThtThermal),
                _ => None,
            })
        })?,
        heatsink: flag("heatsink", false)?,
        remove_unused_layers: flag("remove_unused_layers", false)?,
        connector_contact: flag("connector_contact", false)?,
    };
    pad.validated().map_err(Error::Value)
}

fn footprint(value: &StateValue) -> Result<PhysicalFootprint> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalFootprint")?;
    fields.allow(&[
        "name",
        "pads",
        "body_size",
        "source_library_id",
        "graphics",
        "keepouts",
        "metadata",
        "courtyard",
        "height_nm",
        "clearance_nm",
        "exclude_from_bom",
        "exclude_from_pos_files",
        "internal_pad_groups",
    ])?;
    let flag = |name: &str| fields.get(name).map(boolean).transpose().map(|v| v.unwrap_or(false));
    let footprint = PhysicalFootprint {
        name: text(fields.required("name")?)?,
        pads: sequence(Some(fields.required("pads")?), footprint_pad)?,
        body_size: size(fields.required("body_size")?)?,
        source_library_id: optional_text(fields.get("source_library_id"))?,
        graphics: sequence(fields.get("graphics"), footprint_graphic)?,
        keepouts: sequence(fields.get("keepouts"), copper_keepout)?,
        metadata: string_map(fields.get("metadata"), text)?,
        courtyard: points(fields.get("courtyard"))?,
        height_nm: optional_integer(fields.get("height_nm"))?,
        clearance_nm: optional_integer(fields.get("clearance_nm"))?,
        exclude_from_bom: flag("exclude_from_bom")?,
        exclude_from_pos_files: flag("exclude_from_pos_files")?,
        internal_pad_groups: sequence(fields.get("internal_pad_groups"), |item| {
            let fields = Fields::of(item, "pcbir.pad_connections:InternalPadGroup")?;
            fields.allow(&["numbers"])?;
            InternalPadGroup::new(texts(Some(fields.required("numbers")?))?).map_err(Error::Value)
        })?,
    };
    footprint.validated().map_err(Error::Value)
}

fn placement(value: &StateValue) -> Result<Placement> {
    let fields = Fields::of(value, "pcbir.physical:Placement")?;
    fields.allow(&["reference", "footprint", "position", "rotation_degrees", "side", "value", "source_path"])?;
    let rotation = fields.get("rotation_degrees").map(decimal).transpose()?.unwrap_or_else(Decimal::zero);
    let mut placement = Placement::new(
        &text(fields.required("reference")?)?,
        &text(fields.required("footprint")?)?,
        point(fields.required("position")?)?,
        &rotation,
    )?;
    placement.side = fields.get("side").map(side).transpose()?.unwrap_or(BoardSide::Front);
    placement.value = fields.get("value").map(text).transpose()?.unwrap_or_default();
    placement.source_path = optional_text(fields.get("source_path"))?;
    Ok(placement)
}

fn net(value: &StateValue) -> Result<PhysicalNet> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalNet")?;
    fields.allow(&["name", "pads"])?;
    Ok(PhysicalNet {
        name: text(fields.required("name")?)?,
        pads: sequence(Some(fields.required("pads")?), pad_reference)?,
    })
}

fn stackup_layer(value: &StateValue) -> Result<StackupLayer> {
    let fields = Fields::of(value, "pcbir.physical:StackupLayer")?;
    fields.allow(&[
        "id",
        "kind",
        "thickness_nm",
        "copper_layer",
        "material",
        "relative_permittivity",
        "loss_tangent",
        "dielectric_type",
    ])?;
    let copper = enumeration(fields.required("kind")?, "StackupLayerKind", |text| match text {
        "copper" => Some(true),
        "dielectric" => Some(false),
        _ => None,
    })?;
    let copper_layer = optional(fields.get("copper_layer"), layer)?;
    // The Rust stack-up marks copper by its copper layer, as the lowering does.
    if copper != copper_layer.is_some() {
        return not_implemented("a stack-up layer whose kind disagrees with its copper layer");
    }
    Ok(StackupLayer {
        id: text(fields.required("id")?)?,
        copper_layer,
        thickness_nm: integer(fields.required("thickness_nm")?)?,
        material: optional_text(fields.get("material"))?,
        relative_permittivity: optional(fields.get("relative_permittivity"), decimal)?,
        loss_tangent: optional(fields.get("loss_tangent"), decimal)?,
        dielectric_type: optional_text(fields.get("dielectric_type"))?,
    })
}

fn stackup(value: &StateValue) -> Result<Stackup> {
    let fields = Fields::of(value, "pcbir.physical:Stackup")?;
    fields.allow(&["copper_layers", "thickness_nm", "physical_layers", "via_technologies"])?;
    if fields
        .get("via_technologies")
        .is_some_and(|value| !matches!(value, StateValue::Tuple(items) if items.is_empty()))
    {
        return not_implemented("a stack-up with via technologies");
    }
    let layers = match fields.get("copper_layers") {
        None => vec![CopperLayer::Front, CopperLayer::Back],
        Some(value) => sequence(Some(value), layer)?,
    };
    if layers.is_empty() {
        return Err(Error::Value("a stackup requires at least one copper layer".to_owned()));
    }
    let mut stackup = Stackup::new(layers);
    if let Some(thickness) = fields.get("thickness_nm") {
        stackup.thickness_nm = integer(thickness)?;
    }
    stackup.physical_layers = sequence(fields.get("physical_layers"), stackup_layer)?;
    Ok(stackup)
}

fn design_rules(value: &StateValue) -> Result<DesignRules> {
    let fields = Fields::of(value, "pcbir.physical:DesignRules")?;
    let names: Vec<String> = DesignRules::FIELDS.iter().map(|name| format!("{name}_nm")).collect();
    fields.allow(&names.iter().map(String::as_str).collect::<Vec<_>>())?;
    let mut rules = DesignRules::default();
    for name in &names {
        let Some(value) = fields.get(name) else { continue };
        match name.as_str() {
            "minimum_clearance_nm" => rules.minimum_clearance_nm = integer(value)?,
            "minimum_hole_clearance_nm" => rules.minimum_hole_clearance_nm = integer(value)?,
            "minimum_track_width_nm" => rules.minimum_track_width_nm = integer(value)?,
            "default_track_width_nm" => rules.default_track_width_nm = integer(value)?,
            "default_via_size_nm" => rules.default_via_size_nm = integer(value)?,
            "default_via_drill_nm" => rules.default_via_drill_nm = integer(value)?,
            "minimum_slot_width_nm" => rules.minimum_slot_width_nm = integer(value)?,
            "minimum_via_size_nm" => rules.minimum_via_size_nm = optional_integer(Some(value))?,
            "minimum_via_drill_nm" => rules.minimum_via_drill_nm = optional_integer(Some(value))?,
            _ => unreachable!("the rule fields are listed above"),
        }
    }
    Ok(rules)
}

fn placement_region(value: &StateValue) -> Result<PlacementRegion> {
    let fields = Fields::of(value, "pcbir.physical:PlacementRegion")?;
    fields.allow(&["name", "outline", "side"])?;
    Ok(PlacementRegion {
        name: text(fields.required("name")?)?,
        outline: board_outline(fields.required("outline")?)?,
        side: optional(fields.get("side"), side)?,
    })
}

fn placement_keepout(value: &StateValue) -> Result<PlacementKeepout> {
    let fields = Fields::of(value, "pcbir.physical:PlacementKeepout")?;
    fields.allow(&["name", "outline", "side", "maximum_component_height_nm"])?;
    PlacementKeepout::new(
        &text(fields.required("name")?)?,
        board_outline(fields.required("outline")?)?,
        optional(fields.get("side"), side)?,
        optional_integer(fields.get("maximum_component_height_nm"))?,
    )
    .map_err(Error::Value)
}

fn placement_rule(value: &StateValue) -> Result<ComponentPlacementRule> {
    let fields = Fields::of(value, "pcbir.physical:ComponentPlacementRule")?;
    fields.allow(&[
        "reference",
        "region",
        "allowed_orientations",
        "fixed_position",
        "fixed_rotation_degrees",
        "side",
        "priority",
        "edge_clearance_nm",
        "additional_courtyard_gap_nm",
    ])?;
    let mut rule = ComponentPlacementRule::new(&text(fields.required("reference")?)?);
    rule.region = optional_text(fields.get("region"))?;
    if let Some(orientations) = fields.get("allowed_orientations") {
        rule.allowed_orientations = sequence(Some(orientations), decimal)?;
    }
    rule.fixed_position = optional(fields.get("fixed_position"), point)?;
    rule.fixed_rotation_degrees = optional(fields.get("fixed_rotation_degrees"), decimal)?;
    rule.side = optional(fields.get("side"), side)?;
    rule.priority = fields.get("priority").map(integer).transpose()?.unwrap_or(0);
    rule.edge_clearance_nm = optional_integer(fields.get("edge_clearance_nm"))?;
    rule.additional_courtyard_gap_nm = optional_integer(fields.get("additional_courtyard_gap_nm"))?;
    rule.normalized()
}

fn placement_target(value: &StateValue) -> Result<PlacementTarget> {
    let fields = Fields::of(value, "pcbir.physical:PlacementTarget")?;
    fields.allow(&["reference", "pad"])?;
    Ok(PlacementTarget { reference: text(fields.required("reference")?)?, pad: optional_text(fields.get("pad"))? })
}

fn relative_rule(value: &StateValue) -> Result<RelativePlacementRule> {
    let fields = Fields::of(value, "pcbir.physical:RelativePlacementRule")?;
    fields.allow(&["kind", "targets", "distance_nm", "axis", "tolerance_nm", "weight"])?;
    Ok(RelativePlacementRule {
        kind: enumeration(fields.required("kind")?, "RelativePlacementKind", RelativePlacementKind::parse)?,
        targets: sequence(Some(fields.required("targets")?), placement_target)?,
        distance_nm: optional_integer(fields.get("distance_nm"))?,
        axis: optional(fields.get("axis"), |item| enumeration(item, "AlignmentAxis", AlignmentAxis::parse))?,
        tolerance_nm: fields.get("tolerance_nm").map(integer).transpose()?.unwrap_or(0),
        weight: fields.get("weight").map(integer).transpose()?.unwrap_or(1),
    })
}

fn placement_group(value: &StateValue) -> Result<PlacementGroup> {
    let fields = Fields::of(value, "pcbir.physical:PlacementGroup")?;
    fields.allow(&["name", "references", "anchor", "priority", "source"])?;
    PlacementGroup::new(
        &text(fields.required("name")?)?,
        &texts(Some(fields.required("references")?))?,
        optional_text(fields.get("anchor"))?.as_deref(),
        fields.get("priority").map(integer).transpose()?.unwrap_or(0),
        &fields.get("source").map(text).transpose()?.unwrap_or_else(|| "explicit".to_owned()),
    )
}

fn power_domain(value: &StateValue) -> Result<PhysicalPowerDomain> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalPowerDomain")?;
    fields.allow(&["net", "members", "sources", "source_ports"])?;
    Ok(PhysicalPowerDomain {
        net: text(fields.required("net")?)?,
        members: sequence(Some(fields.required("members")?), pad_reference)?,
        sources: sequence(fields.get("sources"), pad_reference)?,
        source_ports: texts(fields.get("source_ports"))?,
    })
}

pub fn routing_rule(value: &StateValue) -> Result<NetRoutingRule> {
    let fields = Fields::of(value, "pcbir.physical:NetRoutingRule")?;
    fields.allow(&[
        "net",
        "kind",
        "priority",
        "width_nm",
        "clearance_nm",
        "allowed_layers",
        "max_vias",
        "max_length_nm",
        "differential_partner",
        "pair_gap_nm",
        "max_skew_nm",
        "topology",
        "target_impedance_ohms",
        "maximum_uncoupled_length_nm",
        "maximum_stub_length_nm",
        "tuning_amplitude_limit_nm",
        "require_return_vias",
        "reserve_corridor",
        "return_via_net",
        "maximum_return_via_distance_nm",
        "impedance_evidence_digest",
        "return_via_policy",
        "shared_reference_layer",
        "target_single_ended_ohms",
        "impedance_tolerance_percent",
        "layer_group",
        "breakout_length_nm",
        "breakout_width_nm",
        "breakout_gap_nm",
        "breakout_clearance_nm",
        "tuning_style",
        "tuning_spacing_nm",
        "tuning_group",
    ])?;
    let flag = |name: &str| fields.get(name).map(boolean).transpose().map(|v| v.unwrap_or(false));
    let rule = NetRoutingRule {
        net: text(fields.required("net")?)?,
        kind: match fields.get("kind") {
            None => RouteKind::General,
            Some(kind) => enumeration(kind, "RouteKind", RouteKind::parse)?,
        },
        priority: fields.get("priority").map(integer).transpose()?.unwrap_or(0),
        width_nm: optional_integer(fields.get("width_nm"))?,
        clearance_nm: optional_integer(fields.get("clearance_nm"))?,
        allowed_layers: sequence(fields.get("allowed_layers"), layer)?,
        max_vias: optional_integer(fields.get("max_vias"))?,
        max_length_nm: optional_integer(fields.get("max_length_nm"))?,
        differential_partner: optional_text(fields.get("differential_partner"))?,
        pair_gap_nm: optional_integer(fields.get("pair_gap_nm"))?,
        max_skew_nm: optional_integer(fields.get("max_skew_nm"))?,
        topology: fields.get("topology").map(text).transpose()?.unwrap_or_else(|| "point_to_point".to_owned()),
        target_impedance_ohms: optional_integer(fields.get("target_impedance_ohms"))?,
        maximum_uncoupled_length_nm: optional_integer(fields.get("maximum_uncoupled_length_nm"))?,
        maximum_stub_length_nm: optional_integer(fields.get("maximum_stub_length_nm"))?,
        tuning_amplitude_limit_nm: optional_integer(fields.get("tuning_amplitude_limit_nm"))?,
        require_return_vias: flag("require_return_vias")?,
        reserve_corridor: flag("reserve_corridor")?,
        return_via_net: optional_text(fields.get("return_via_net"))?,
        maximum_return_via_distance_nm: optional_integer(fields.get("maximum_return_via_distance_nm"))?,
        impedance_evidence_digest: optional_text(fields.get("impedance_evidence_digest"))?,
        return_via_policy: match fields.get("return_via_policy") {
            None => ReturnViaPolicy::Always,
            Some(policy) => enumeration(policy, "ReturnViaPolicy", ReturnViaPolicy::parse)?,
        },
        shared_reference_layer: optional(fields.get("shared_reference_layer"), layer)?,
        target_single_ended_ohms: optional_integer(fields.get("target_single_ended_ohms"))?,
        impedance_tolerance_percent: optional(fields.get("impedance_tolerance_percent"), decimal)?,
        layer_group: optional_text(fields.get("layer_group"))?,
        breakout_length_nm: optional_integer(fields.get("breakout_length_nm"))?,
        breakout_width_nm: optional_integer(fields.get("breakout_width_nm"))?,
        breakout_gap_nm: optional_integer(fields.get("breakout_gap_nm"))?,
        breakout_clearance_nm: optional_integer(fields.get("breakout_clearance_nm"))?,
        tuning_style: match fields.get("tuning_style") {
            None => TuningStyle::Bumps,
            Some(style) => enumeration(style, "TuningStyle", TuningStyle::parse)?,
        },
        tuning_spacing_nm: optional_integer(fields.get("tuning_spacing_nm"))?,
        tuning_group: optional_text(fields.get("tuning_group"))?,
    };
    rule.validate()?;
    Ok(rule)
}

fn zone(value: &StateValue) -> Result<CopperZone> {
    let fields = Fields::of(value, "pcbir.physical:CopperZone")?;
    fields.allow(&[
        "id",
        "net",
        "layers",
        "outline",
        "priority",
        "clearance_nm",
        "minimum_width_nm",
        "pad_connection",
        "thermal",
        "island_policy",
        "minimum_island_area_nm2",
        "fill_mode",
        "reserve_routing",
        "allow_same_net_hard_macro_overlap",
    ])?;
    let (outer, holes) = polygon_with_holes(fields.required("outline")?)?;
    let (mut thermal_gap_nm, mut thermal_spoke_width_nm) = (300_000, 300_000);
    if let Some(thermal) = fields.get("thermal") {
        let thermal = Fields::of(thermal, "pcbir.physical:ThermalReliefSettings")?;
        thermal.allow(&["gap_nm", "spoke_width_nm", "spoke_count"])?;
        thermal_gap_nm = thermal.get("gap_nm").map(integer).transpose()?.unwrap_or(300_000);
        thermal_spoke_width_nm = thermal.get("spoke_width_nm").map(integer).transpose()?.unwrap_or(300_000);
        if thermal.get("spoke_count").map(integer).transpose()?.unwrap_or(4) != 4 {
            return not_implemented("a copper zone with a thermal spoke count other than four");
        }
    }
    if let Some(mode) = fields.get("fill_mode") {
        enumeration(mode, "ZoneFillMode", |text| (text == "solid").then_some(()))?;
    }
    let flag = |name: &str| fields.get(name).map(boolean).transpose().map(|v| v.unwrap_or(false));
    let zone = CopperZone {
        id: text(fields.required("id")?)?,
        net: text(fields.required("net")?)?,
        layers: sequence(Some(fields.required("layers")?), layer)?,
        outer,
        holes,
        priority: fields.get("priority").map(integer).transpose()?.unwrap_or(0),
        clearance_nm: optional_integer(fields.get("clearance_nm"))?,
        minimum_width_nm: fields.get("minimum_width_nm").map(integer).transpose()?.unwrap_or(250_000),
        pad_connection: match fields.get("pad_connection") {
            None => ZoneConnection::Thermal,
            Some(connection) => enumeration(connection, "ZoneConnection", |text| match text {
                "thermal" => Some(ZoneConnection::Thermal),
                "solid" => Some(ZoneConnection::Solid),
                "none" => Some(ZoneConnection::None),
                "tht_thermal" => Some(ZoneConnection::ThtThermal),
                _ => None,
            })?,
        },
        thermal_gap_nm,
        thermal_spoke_width_nm,
        island_policy: match fields.get("island_policy") {
            None => IslandPolicy::RemoveAll,
            Some(policy) => enumeration(policy, "IslandPolicy", IslandPolicy::parse)?,
        },
        minimum_island_area_nm2: optional_integer(fields.get("minimum_island_area_nm2"))?,
        reserve_routing: flag("reserve_routing")?,
        allow_same_net_hard_macro_overlap: flag("allow_same_net_hard_macro_overlap")?,
    };
    zone.validate()?;
    Ok(zone)
}

fn polygon(value: &StateValue) -> Result<CopperPolygon> {
    let fields = Fields::of(value, "pcbir.physical:CopperPolygon")?;
    fields.allow(&["id", "net", "layer", "outline"])?;
    Ok(CopperPolygon {
        id: text(fields.required("id")?)?,
        net: text(fields.required("net")?)?,
        layer: layer(fields.required("layer")?)?,
        vertices: ring(fields.required("outline")?)?,
    })
}

fn rigid_cluster(value: &StateValue) -> Result<RigidPlacementCluster> {
    let fields = Fields::of(value, "pcbir.physical:RigidPlacementCluster")?;
    fields.allow(&[
        "name",
        "anchor",
        "members",
        "source",
        "allowed_rotations",
        "keepouts",
        "internal_clearance_nm",
        "edge_openings",
    ])?;
    let anchor = placement_target(fields.required("anchor")?)?;
    let members = sequence(Some(fields.required("members")?), |item| {
        let fields = Fields::of(item, "pcbir.physical:RigidPlacementMember")?;
        fields.allow(&["reference", "footprint", "footprint_digest", "position", "rotation_degrees"])?;
        RigidPlacementMember::new(
            &text(fields.required("reference")?)?,
            &text(fields.required("footprint")?)?,
            &text(fields.required("footprint_digest")?)?,
            point(fields.required("position")?)?,
            &fields.get("rotation_degrees").map(decimal).transpose()?.unwrap_or_else(Decimal::zero),
        )
    })?;
    let edge_openings = sequence(fields.get("edge_openings"), |item| {
        let fields = Fields::of(item, "pcbir.physical:MacroEdgeOpening")?;
        fields.allow(&["keepout", "edge_indices"])?;
        Ok(MacroEdgeOpening {
            keepout: text(fields.required("keepout")?)?,
            edge_indices: sequence(Some(fields.required("edge_indices")?), integer)?,
        })
    })?;
    RigidPlacementCluster {
        name: text(fields.required("name")?)?,
        anchor_reference: anchor.reference,
        anchor_pad: anchor.pad,
        members,
        source: text(fields.required("source")?)?,
        allowed_rotations: match fields.get("allowed_rotations") {
            None => [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
            Some(value) => sequence(Some(value), decimal)?,
        },
        keepouts: sequence(fields.get("keepouts"), copper_keepout)?,
        internal_clearance_nm: optional_integer(fields.get("internal_clearance_nm"))?,
        edge_openings,
    }
    .normalized()
}

fn hard_macro(value: &StateValue) -> Result<PhysicalHardMacro> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalHardMacro")?;
    fields.allow(&[
        "cluster",
        "asset_sha256",
        "tracks",
        "vias",
        "ports",
        "protected_regions",
        "required_layers",
        "pad_bindings",
        "isolated_pads",
        "zones",
        "plane_returns",
        "polygons",
        "width_contracts",
    ])?;
    let hard = PhysicalHardMacro {
        cluster: text(fields.required("cluster")?)?,
        asset_sha256: text(fields.required("asset_sha256")?)?,
        tracks: sequence(Some(fields.required("tracks")?), track)?,
        vias: sequence(fields.get("vias"), via)?,
        ports: sequence(fields.get("ports"), |item| {
            let fields = Fields::of(item, "pcbir.physical:MacroPort")?;
            fields.allow(&["name", "net", "position", "layer", "pads"])?;
            Ok(MacroPort {
                name: text(fields.required("name")?)?,
                net: text(fields.required("net")?)?,
                position: point(fields.required("position")?)?,
                layer: layer(fields.required("layer")?)?,
                pads: sequence(fields.get("pads"), pad_reference)?,
            })
        })?,
        protected_regions: sequence(fields.get("protected_regions"), copper_keepout)?,
        required_layers: sequence(fields.get("required_layers"), layer)?,
        pad_bindings: sequence(fields.get("pad_bindings"), |item| {
            let fields = Fields::of(item, "pcbir.physical:MacroPadBinding")?;
            fields.allow(&["pad", "net"])?;
            Ok((pad_reference(fields.required("pad")?)?, text(fields.required("net")?)?))
        })?,
        isolated_pads: sequence(fields.get("isolated_pads"), pad_reference)?,
        zones: sequence(fields.get("zones"), zone)?,
        plane_returns: sequence(fields.get("plane_returns"), |item| {
            let fields = Fields::of(item, "pcbir.physical:MacroPlaneReturn")?;
            fields.allow(&["net", "layers", "pads", "dedicated_contacts"])?;
            Ok(MacroPlaneReturn {
                net: text(fields.required("net")?)?,
                layers: sequence(Some(fields.required("layers")?), layer)?,
                pads: sequence(Some(fields.required("pads")?), pad_reference)?,
                dedicated_contacts: sequence(fields.get("dedicated_contacts"), |contact| {
                    let [pad, position] = items(contact)? else {
                        return not_implemented("a dedicated plane contact that is not a pad/point pair");
                    };
                    Ok((pad_reference(pad)?, point(position)?))
                })?,
            })
        })?,
        polygons: sequence(fields.get("polygons"), polygon)?,
        width_contracts: sequence(fields.get("width_contracts"), |item| {
            let fields = Fields::of(item, "pcbir.physical:MacroTrackWidthContract")?;
            fields.allow(&["track_index", "minimum_width_nm", "purpose", "evidence"])?;
            let index = integer(fields.required("track_index")?)?;
            Ok(MacroTrackWidthContract {
                track_index: usize::try_from(index)
                    .map_err(|_| Error::Value("macro width contract track index is out of range".to_owned()))?,
                minimum_width_nm: integer(fields.required("minimum_width_nm")?)?,
                purpose: text(fields.required("purpose")?)?,
                evidence: text(fields.required("evidence")?)?,
            })
        })?,
    };
    hard.validate()?;
    Ok(hard)
}

fn via_in_pad_rule(value: &StateValue) -> Result<PadViaInPadRule> {
    let fields = Fields::of(value, "pcbir.physical:PadViaInPadRule")?;
    fields.allow(&["pad", "process", "rows", "columns", "pitch_nm"])?;
    Ok(PadViaInPadRule {
        pad: pad_reference(fields.required("pad")?)?,
        rows: optional_integer(fields.get("rows"))?,
        columns: optional_integer(fields.get("columns"))?,
        pitch_nm: optional_integer(fields.get("pitch_nm"))?,
    })
}

fn mechanical_hole(value: &StateValue) -> Result<MechanicalHole> {
    let fields = Fields::of(value, "pcbir.physical:MechanicalHole")?;
    fields.allow(&["id", "position", "diameter_nm", "head_clearance_radius_nm"])?;
    MechanicalHole::new(
        &text(fields.required("id")?)?,
        point(fields.required("position")?)?,
        integer(fields.required("diameter_nm")?)?,
        fields.get("head_clearance_radius_nm").map(integer).transpose()?.unwrap_or(0),
    )
    .map_err(Error::Value)
}

fn mechanical_slot(value: &StateValue) -> Result<MechanicalSlot> {
    let fields = Fields::of(value, "pcbir.physical:MechanicalSlot")?;
    fields.allow(&["id", "start", "end", "width_nm"])?;
    MechanicalSlot::new(
        &text(fields.required("id")?)?,
        point(fields.required("start")?)?,
        point(fields.required("end")?)?,
        integer(fields.required("width_nm")?)?,
    )
    .map_err(Error::Value)
}

fn datum(value: &StateValue) -> Result<BoardDatum> {
    let fields = Fields::of(value, "pcbir.physical:BoardDatum")?;
    fields.allow(&["id", "position", "relative_to", "offset"])?;
    Ok(BoardDatum {
        id: text(fields.required("id")?)?,
        position: point(fields.required("position")?)?,
        relative_to: optional_text(fields.get("relative_to"))?,
        offset: fields.get("offset").map(point).transpose()?.unwrap_or(Point::new(0, 0)),
    })
}

fn board_edge(value: &StateValue) -> Result<BoardEdge> {
    let fields = Fields::of(value, "pcbir.physical:BoardEdge")?;
    fields.allow(&["id", "start", "end"])?;
    BoardEdge::new(&text(fields.required("id")?)?, point(fields.required("start")?)?, point(fields.required("end")?)?)
        .map_err(Error::Value)
}

fn attachment(value: &StateValue) -> Result<PhysicalAttachment> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalAttachment")?;
    fields.allow(&["id", "reference", "target", "position", "offset", "anchor", "anchor_point", "rotation", "side"])?;
    Ok(PhysicalAttachment {
        id: text(fields.required("id")?)?,
        reference: text(fields.required("reference")?)?,
        target: optional_text(fields.get("target"))?,
        position: point(fields.required("position")?)?,
        offset: point(fields.required("offset")?)?,
        anchor: text(fields.required("anchor")?)?,
        anchor_point: point(fields.required("anchor_point")?)?,
        rotation: decimal(fields.required("rotation")?)?,
        side: side(fields.required("side")?)?,
    })
}

fn component_height(value: &StateValue) -> Result<ComponentHeight> {
    let fields = Fields::of(value, "pcbir.physical:ComponentHeight")?;
    fields.allow(&["id", "reference", "height_nm"])?;
    Ok(ComponentHeight {
        id: text(fields.required("id")?)?,
        reference: text(fields.required("reference")?)?,
        height_nm: integer(fields.required("height_nm")?)?,
    })
}

fn assembly_access(value: &StateValue) -> Result<AssemblyAccess> {
    let fields = Fields::of(value, "pcbir.physical:AssemblyAccess")?;
    fields.allow(&["id", "reference", "outline", "side", "purpose"])?;
    Ok(AssemblyAccess {
        id: text(fields.required("id")?)?,
        reference: text(fields.required("reference")?)?,
        outline: board_outline(fields.required("outline")?)?,
        side: text(fields.required("side")?)?,
        purpose: text(fields.required("purpose")?)?,
    })
}

fn match_group(value: &StateValue) -> Result<NetMatchGroup> {
    let fields = Fields::of(value, "pcbir.physical:NetMatchGroup")?;
    fields.allow(&["id", "nets", "max_skew_nm"])?;
    Ok(NetMatchGroup {
        id: text(fields.required("id")?)?,
        nets: texts(Some(fields.required("nets")?))?,
        max_skew_nm: integer(fields.required("max_skew_nm")?)?,
    })
}

fn hole_clearance(value: &StateValue) -> Result<ComponentHoleClearance> {
    let fields = Fields::of(value, "pcbir.physical:ComponentHoleClearance")?;
    fields.allow(&["reference", "clearance_nm", "reason"])?;
    Ok(ComponentHoleClearance {
        reference: text(fields.required("reference")?)?,
        clearance_nm: integer(fields.required("clearance_nm")?)?,
        reason: text(fields.required("reason")?)?,
    })
}

fn decoupling_link(value: &StateValue) -> Result<DecouplingLink> {
    let fields = Fields::of(value, "pcbir.physical:DecouplingLink")?;
    fields.allow(&["capacitor", "target", "return_pad", "net", "return_net"])?;
    Ok(DecouplingLink {
        capacitor: pad_reference(fields.required("capacitor")?)?,
        target: pad_reference(fields.required("target")?)?,
        return_pad: pad_reference(fields.required("return_pad")?)?,
        net: text(fields.required("net")?)?,
        return_net: text(fields.required("return_net")?)?,
    })
}

fn power_feed(value: &StateValue) -> Result<PowerFeedBoundary> {
    let fields = Fields::of(value, "pcbir.physical:PowerFeedBoundary")?;
    fields.allow(&["net", "terminal", "upstream_net", "upstream_terminal", "provenance"])?;
    Ok(PowerFeedBoundary {
        net: text(fields.required("net")?)?,
        terminal: pad_reference(fields.required("terminal")?)?,
        upstream_net: text(fields.required("upstream_net")?)?,
        upstream_terminal: pad_reference(fields.required("upstream_terminal")?)?,
        provenance: fields.get("provenance").map(text).transpose()?.unwrap_or_else(|| "dc_power_path".to_owned()),
    })
}

fn material(value: Option<&StateValue>) -> Result<ArtworkMaterial> {
    match value {
        None => Ok(ArtworkMaterial::Silkscreen),
        Some(value) => enumeration(value, "ArtworkMaterial", |text| match text {
            "silkscreen" => Some(ArtworkMaterial::Silkscreen),
            "copper" => Some(ArtworkMaterial::Copper),
            _ => None,
        }),
    }
}

fn artwork(value: &StateValue) -> Result<Artwork> {
    match type_name(value) {
        Some("pcbir.silkscreen:SilkscreenText") => {
            let fields = Fields::of(value, "pcbir.silkscreen:SilkscreenText")?;
            fields.allow(&[
                "id",
                "text",
                "position",
                "size_nm",
                "thickness_nm",
                "rotation_degrees",
                "side",
                "material",
                "exposed",
            ])?;
            Ok(Artwork::Text {
                id: text(fields.required("id")?)?,
                text: text(fields.required("text")?)?,
                position: point(fields.required("position")?)?,
                size_nm: fields.get("size_nm").map(integer).transpose()?.unwrap_or(1_000_000),
                thickness_nm: fields.get("thickness_nm").map(integer).transpose()?.unwrap_or(150_000),
                rotation_degrees: fields
                    .get("rotation_degrees")
                    .map(decimal)
                    .transpose()?
                    .unwrap_or_else(Decimal::zero),
                side: fields.get("side").map(side).transpose()?.unwrap_or(BoardSide::Front),
                material: material(fields.get("material"))?,
                exposed: fields.get("exposed").map(boolean).transpose()?.unwrap_or(false),
            })
        }
        Some("pcbir.silkscreen:SilkscreenGraphic") => {
            let fields = Fields::of(value, "pcbir.silkscreen:SilkscreenGraphic")?;
            fields.allow(&["id", "graphic", "side", "material", "exposed"])?;
            let graphic = match footprint_graphic(fields.required("graphic")?)? {
                FootprintGraphic::Line { start, end, width_nm, .. } => Graphic::Line { start, end, width_nm },
                FootprintGraphic::Rectangle { start, end, width_nm, filled, .. } => {
                    Graphic::Rectangle { start, end, width_nm, filled }
                }
                FootprintGraphic::Circle { center, end, width_nm, filled, .. } => {
                    Graphic::Circle { center, end, width_nm, filled }
                }
                FootprintGraphic::Arc { start, midpoint, end, width_nm, .. } => {
                    Graphic::Arc { start, midpoint, end, width_nm }
                }
                FootprintGraphic::Polygon { points, width_nm, filled, .. } => {
                    Graphic::Polygon { points, width_nm, filled }
                }
            };
            Ok(Artwork::Graphic {
                id: text(fields.required("id")?)?,
                graphic,
                side: fields.get("side").map(side).transpose()?.unwrap_or(BoardSide::Front),
                material: material(fields.get("material"))?,
                exposed: fields.get("exposed").map(boolean).transpose()?.unwrap_or(false),
            })
        }
        _ => not_implemented("a silkscreen artwork of an unknown kind"),
    }
}

/// `pcbir.physical:PhysicalBoard` from its saved fields.
pub fn board(value: &StateValue) -> Result<PhysicalBoard> {
    let fields = Fields::of(value, "pcbir.physical:PhysicalBoard")?;
    fields.allow(&[
        "name",
        "outline",
        "footprints",
        "placements",
        "nets",
        "stackup",
        "rules",
        "tracks",
        "vias",
        "metadata",
        "regions",
        "keepouts",
        "placement_rules",
        "relative_rules",
        "placement_groups",
        "power_domains",
        "net_routing_rules",
        "zones",
        "polygons",
        "copper_keepouts",
        "zone_fills",
        "rigid_clusters",
        "hard_macros",
        "materialized_macros",
        "via_in_pad_rules",
        "mechanical_holes",
        "datums",
        "boundary_edges",
        "attachments",
        "body_overhangs",
        "component_heights",
        "assembly_envelopes",
        "assembly_access",
        "mechanical_slots",
        "mechanical_references",
        "match_groups",
        "component_hole_clearances",
        "decoupling_links",
        "silkscreen",
        "power_feeds",
    ])?;
    for name in ["zone_fills", "body_overhangs", "assembly_envelopes", "mechanical_references"] {
        if fields.get(name).is_some_and(|value| items(value).is_ok_and(|items| !items.is_empty())) {
            return not_implemented(&format!("a saved board with {name}"));
        }
    }
    let board = PhysicalBoard {
        name: text(fields.required("name")?)?,
        outline: board_outline(fields.required("outline")?)?,
        footprints: string_map(Some(fields.required("footprints")?), footprint)?,
        placements: sequence(Some(fields.required("placements")?), placement)?,
        nets: sequence(Some(fields.required("nets")?), net)?,
        stackup: match fields.get("stackup") {
            None => Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
            Some(value) => stackup(value)?,
        },
        rules: match fields.get("rules") {
            None => DesignRules::default(),
            Some(value) => design_rules(value)?,
        },
        tracks: sequence(fields.get("tracks"), track)?,
        vias: sequence(fields.get("vias"), via)?,
        metadata: string_map(fields.get("metadata"), text)?,
        regions: sequence(fields.get("regions"), placement_region)?,
        keepouts: sequence(fields.get("keepouts"), placement_keepout)?,
        placement_rules: sequence(fields.get("placement_rules"), placement_rule)?,
        relative_rules: sequence(fields.get("relative_rules"), relative_rule)?,
        placement_groups: sequence(fields.get("placement_groups"), placement_group)?,
        power_domains: sequence(fields.get("power_domains"), power_domain)?,
        net_routing_rules: sequence(fields.get("net_routing_rules"), routing_rule)?,
        zones: sequence(fields.get("zones"), zone)?,
        polygons: sequence(fields.get("polygons"), polygon)?,
        copper_keepouts: sequence(fields.get("copper_keepouts"), copper_keepout)?,
        rigid_clusters: sequence(fields.get("rigid_clusters"), rigid_cluster)?,
        hard_macros: sequence(fields.get("hard_macros"), hard_macro)?,
        materialized_macros: texts(fields.get("materialized_macros"))?,
        via_in_pad_rules: sequence(fields.get("via_in_pad_rules"), via_in_pad_rule)?,
        mechanical_holes: sequence(fields.get("mechanical_holes"), mechanical_hole)?,
        mechanical_slots: sequence(fields.get("mechanical_slots"), mechanical_slot)?,
        datums: sequence(fields.get("datums"), datum)?,
        boundary_edges: sequence(fields.get("boundary_edges"), board_edge)?,
        attachments: sequence(fields.get("attachments"), attachment)?,
        component_heights: sequence(fields.get("component_heights"), component_height)?,
        body_overhangs: Vec::new(),
        assembly_envelopes: Vec::new(),
        assembly_access: sequence(fields.get("assembly_access"), assembly_access)?,
        match_groups: sequence(fields.get("match_groups"), match_group)?,
        component_hole_clearances: sequence(fields.get("component_hole_clearances"), hole_clearance)?,
        decoupling_links: sequence(fields.get("decoupling_links"), decoupling_link)?,
        silkscreen: sequence(fields.get("silkscreen"), artwork)?,
        power_feeds: sequence(fields.get("power_feeds"), power_feed)?,
        mechanical_references: Vec::new(),
    };
    // `PhysicalBoard.__post_init__`.
    let mut board = board;
    crate::physicalize::revalidate(&mut board)?;
    let mut ids: Vec<&str> = board.silkscreen.iter().map(Artwork::id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != board.silkscreen.len() {
        return Err(Error::Value("duplicate silkscreen feature id".to_owned()));
    }
    let mut capacitors: Vec<&str> =
        board.decoupling_links.iter().map(|link| link.capacitor.component.as_str()).collect();
    capacitors.sort_unstable();
    capacitors.dedup();
    if capacitors.len() != board.decoupling_links.len() {
        return Err(Error::Value("a decoupler requires one explicit protected pin association".to_owned()));
    }
    Ok(board)
}

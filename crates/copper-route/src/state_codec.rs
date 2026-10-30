//! Typed reading and writing of the router's state files (the typed half of
//! `pcbir.state_codec`: Python reconstructs `pcbir` dataclasses from their
//! `$type` tags; this module does the same for the types the unified router
//! saves and loads). The frozen `copperscript.state/v1` container itself is
//! `copper-state`.
//!
//! Errors follow Python's: a `StateError` or `ValueError` (unknown field,
//! bad enum value, a dataclass `__post_init__` check) is a caught error, a
//! missing required field is the `TypeError` crash of `cls(**items)`, and a
//! value whose Python behaviour would be dynamic (a float where an integer
//! belongs, a dataclass of another type) is not implemented.

use indexmap::{IndexMap, IndexSet};
use num_traits::ToPrimitive;

use copper_json::{Object, Value, python_str_repr};
use copper_lang::mechanical::assembly::{AssemblyEnvelope, BodyOverhang};
use copper_lang::mechanical::stackup::StackupLayer;
use copper_lang::silkscreen::{Artwork, ArtworkMaterial, Graphic};
use copper_pcb::board::*;
use copper_pcb::decimal_rem;
use copper_physical::footprint::{
    FootprintGraphic, FootprintLayer, FootprintPad, InternalPadGroup, PadKind, PadShape, PhysicalFootprint, Size,
    ZoneConnection,
};
use copper_physical::{
    BoardBoundaryPath, BoardCutout, BoardDatum, BoardEdge, BoardOutline, BoardSide, BoundarySegment, Bounds,
    CircularBoardBoundary, CopperKeepout, CopperLayer, DesignRules, MechanicalHole, MechanicalSlot, PlacementKeepout,
    Point,
};
use copper_state::{NdArray, StateDocument, StateError, StateValue};
use copper_units::Decimal;

use crate::congestion_feedback::CongestionMap;
use crate::detailed::{DetailedNetResult, DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus};
use crate::fine_regions::FineRegionOptions;
use crate::pin_escape::RoutingAccess;
use crate::routing::*;
use crate::routing_costs::LayerBias;
use crate::unified_error::{Error, Result, fail, not_implemented};
use crate::unified_router::UnifiedRouterOptions;

/// The dict `route_unified` saves under `save_inputs_path` and
/// `replay_unified` loads.
#[derive(Clone, Debug)]
pub struct SavedInputs {
    pub board: PhysicalBoard,
    pub global_route: GlobalRoutingResult,
    pub options: UnifiedRouterOptions,
    pub fanout_accesses: Option<Vec<(PadReference, RoutingAccess)>>,
    pub fanout_created_vias: Option<Vec<(String, Point)>>,
    pub fanout_created_tracks: Option<Vec<TrackSegment>>,
    pub only_nets: Option<IndexSet<String>>,
    pub required_accesses: Option<Vec<(PadReference, RoutingAccess)>>,
}

/// Python's `load_state(path)` on the file's text: the container checks
/// (`StateError`, naming the path as given), then the typed decode.
pub fn load_inputs(text: &str, path: &str) -> Result<SavedInputs> {
    let document = StateDocument::read(text).map_err(|error| match error {
        StateError::Json(error) => Error::Value(error.python_message(text)),
        StateError::NotAStateFile => Error::Value(format!("{path}: not a {} file", copper_state::STATE_SCHEMA)),
        StateError::DigestMismatch => Error::Value(format!("{path}: content digest mismatch")),
        other => Error::Value(other.to_string()),
    })?;
    saved_inputs(&document.value)
}

// ---------------------------------------------------------------------------
// Reading dataclasses

/// One dataclass being read: its `$type`, the class name and the file's
/// fields it did not declare.
struct Reader<'a> {
    type_name: &'a str,
    class: &'static str,
    unknown: Vec<String>,
}

impl<'a> Reader<'a> {
    /// The fields of a dataclass value of `expected` (`module:Class`).
    fn new(value: &'a StateValue, expected: &'static str) -> Result<(Self, &'a IndexMap<String, StateValue>)> {
        let class = expected.rsplit(':').next().expect("a qualified name");
        match value {
            StateValue::Dataclass { name, fields } if name == expected => {
                Ok((Reader { type_name: name, class, unknown: Vec::new() }, fields))
            }
            StateValue::Dataclass { name, .. } => {
                not_implemented(format!("a {class} field holds a {name} value, which Python would use dynamically"))
            }
            other => {
                not_implemented(format!("a {class} field holds a {}, which Python would use dynamically", kind(other)))
            }
        }
    }

    /// `StateError` for the fields the dataclass does not declare.
    fn check_unknown(&self) -> Result<()> {
        if self.unknown.is_empty() {
            return Ok(());
        }
        let mut names = self.unknown.clone();
        names.sort();
        let listed: Vec<String> = names.iter().map(|name| python_str_repr(name)).collect();
        fail(format!("{} has no init field(s) [{}]", self.type_name, listed.join(", ")))
    }

    /// The `TypeError` of `Class(**items)` for missing required fields,
    /// `(name, missing)` in parameter order.
    fn required(&self, fields: &[(&str, bool)]) -> Result<()> {
        let missing: Vec<String> = fields.iter().filter(|(_, m)| *m).map(|(n, _)| python_str_repr(n)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let names = match missing.as_slice() {
            [one] => one.clone(),
            [a, b] => format!("{a} and {b}"),
            many => format!("{}, and {}", many[..many.len() - 1].join(", "), many[many.len() - 1]),
        };
        let plural = if missing.len() == 1 { "argument" } else { "arguments" };
        Err(Error::type_error(format!(
            "{}.__init__() missing {} required positional {plural}: {names}",
            self.class,
            missing.len()
        )))
    }
}

/// Reads the fields of one dataclass value: each declared field into its
/// slot (`None` when absent), the rest as unknown.
macro_rules! dataclass {
    ($reader:ident, $value:expr, $type:literal, { $($field:literal => $slot:ident = $decode:expr;)* }) => {
        let (mut $reader, fields) = Reader::new($value, $type)?;
        $(let mut $slot = None;)*
        for (name, item) in fields {
            match name.as_str() {
                $($field => $slot = Some($decode(item)?),)*
                other => $reader.unknown.push(other.to_owned()),
            }
        }
        $reader.check_unknown()?;
    };
}

fn kind(value: &StateValue) -> &'static str {
    match value {
        StateValue::Null => "None",
        StateValue::Bool(_) => "bool",
        StateValue::Int(_) => "int",
        StateValue::Float(_) => "float",
        StateValue::Str(_) => "str",
        StateValue::List(_) => "list",
        StateValue::Dataclass { .. } => "dataclass",
        StateValue::Enum { .. } => "enum",
        StateValue::Decimal(_) => "Decimal",
        StateValue::Fraction(_) => "Fraction",
        StateValue::Tuple(_) => "tuple",
        StateValue::FrozenSet(_) => "frozenset",
        StateValue::Set(_) => "set",
        StateValue::Map { .. } => "dict",
        StateValue::NdArray(_) => "ndarray",
        StateValue::Object(_) => "dict",
    }
}

fn dynamic<T>(what: &str, value: &StateValue) -> Result<T> {
    not_implemented(format!(
        "a state file holds a {} where {what} belongs, which Python would use dynamically",
        kind(value)
    ))
}

fn int(value: &StateValue) -> Result<i64> {
    match value {
        StateValue::Int(number) => number.to_i64().ok_or_else(|| Error::not_implemented("an integer beyond 64 bits")),
        other => dynamic("an integer", other),
    }
}

fn text(value: &StateValue) -> Result<String> {
    match value {
        StateValue::Str(text) => Ok(text.clone()),
        other => dynamic("text", other),
    }
}

fn boolean(value: &StateValue) -> Result<bool> {
    match value {
        StateValue::Bool(flag) => Ok(*flag),
        other => dynamic("a boolean", other),
    }
}

fn float(value: &StateValue) -> Result<f64> {
    match value {
        StateValue::Float(number) => Ok(*number),
        StateValue::Int(number) => number
            .to_i64()
            .filter(|n| n.abs() < 1 << 53)
            .map(|n| n as f64)
            .ok_or_else(|| Error::not_implemented("a large integer in a float field")),
        other => dynamic("a float", other),
    }
}

/// `Decimal(str(value))`: the Decimal, an int, or decimal text.
fn decimal(value: &StateValue) -> Result<Decimal> {
    match value {
        StateValue::Decimal(number) => Ok(number.clone()),
        StateValue::Int(number) => Ok(Decimal::parse(&number.to_string()).expect("integer text")),
        StateValue::Str(text) => {
            Decimal::parse(text).map_err(|_| Error::not_implemented("a decimal that Python's Decimal() rejects"))
        }
        other => dynamic("a decimal", other),
    }
}

fn optional<T>(value: &StateValue, decode: impl Fn(&StateValue) -> Result<T>) -> Result<Option<T>> {
    match value {
        StateValue::Null => Ok(None),
        other => decode(other).map(Some),
    }
}

/// A `tuple` or `list` field (dataclass `__post_init__` turns lists into tuples).
fn list<T>(value: &StateValue, decode: impl Fn(&StateValue) -> Result<T>) -> Result<Vec<T>> {
    match value {
        StateValue::Tuple(items) | StateValue::List(items) => items.iter().map(decode).collect(),
        other => dynamic("a sequence", other),
    }
}

/// A `frozenset` field (also accepting a `set` or sequence as Python's
/// membership tests would).
fn set<T>(value: &StateValue, decode: impl Fn(&StateValue) -> Result<T>) -> Result<Vec<T>> {
    match value {
        StateValue::FrozenSet(items) | StateValue::Set(items) | StateValue::Tuple(items) | StateValue::List(items) => {
            items.iter().map(decode).collect()
        }
        other => dynamic("a set", other),
    }
}

/// `Class(value)` of a `str` enum: the member, or Python's ValueError.
fn enumeration<T: Copy>(value: &StateValue, expected: &str, parse: impl Fn(&str) -> Option<T>) -> Result<T> {
    let class = expected.rsplit(':').next().expect("a qualified name");
    match value {
        StateValue::Enum { name, value } if name == expected => match value.as_ref() {
            StateValue::Str(text) => {
                parse(text).ok_or_else(|| Error::Value(format!("{} is not a valid {class}", python_str_repr(text))))
            }
            other => dynamic("an enum value", other),
        },
        StateValue::Enum { name, .. } => not_implemented(format!("a {class} field holds a {name} member")),
        other => dynamic("an enum", other),
    }
}

fn copper_layer(value: &StateValue) -> Result<CopperLayer> {
    enumeration(value, "pcbir.physical:CopperLayer", CopperLayer::parse)
}

fn board_side(value: &StateValue) -> Result<BoardSide> {
    enumeration(value, "pcbir.physical:BoardSide", BoardSide::parse)
}

/// A `Mapping[str, str]` (`$map`, with or without the proxy flag).
fn text_map(value: &StateValue) -> Result<IndexMap<String, String>> {
    match value {
        StateValue::Map { entries, .. } => {
            let mut map = IndexMap::new();
            for (key, item) in entries {
                map.insert(text(key)?, text(item)?);
            }
            Ok(map)
        }
        other => dynamic("a mapping", other),
    }
}

fn point(value: &StateValue) -> Result<Point> {
    dataclass!(reader, value, "pcbir.physical:Point", {
        "x_nm" => x = int;
        "y_nm" => y = int;
    });
    reader.required(&[("x_nm", x.is_none()), ("y_nm", y.is_none())])?;
    Ok(Point::new(x.expect("required"), y.expect("required")))
}

fn size(value: &StateValue) -> Result<Size> {
    dataclass!(reader, value, "pcbir.physical:Size", {
        "width_nm" => width = int;
        "height_nm" => height = int;
    });
    reader.required(&[("width_nm", width.is_none()), ("height_nm", height.is_none())])?;
    Ok(Size::new(width.expect("required"), height.expect("required"))?)
}

fn pad_reference(value: &StateValue) -> Result<PadReference> {
    dataclass!(reader, value, "pcbir.physical:PadReference", {
        "component" => component = text;
        "pad" => pad = text;
    });
    reader.required(&[("component", component.is_none()), ("pad", pad.is_none())])?;
    Ok(PadReference { component: component.expect("required"), pad: pad.expect("required") })
}

fn points(value: &StateValue) -> Result<Vec<Point>> {
    list(value, point)
}

fn polygon_ring(value: &StateValue) -> Result<Vec<Point>> {
    dataclass!(reader, value, "pcbir.physical:PolygonRing", {
        "vertices" => vertices = points;
    });
    reader.required(&[("vertices", vertices.is_none())])?;
    Ok(vertices.expect("required"))
}

/// `PolygonWithHoles` as `(outer, holes)`.
fn polygon_with_holes(value: &StateValue) -> Result<(Vec<Point>, Vec<Vec<Point>>)> {
    dataclass!(reader, value, "pcbir.physical:PolygonWithHoles", {
        "outer" => outer = polygon_ring;
        "holes" => holes = |v| list(v, polygon_ring);
    });
    reader.required(&[("outer", outer.is_none())])?;
    Ok((outer.expect("required"), holes.unwrap_or_default()))
}

fn board_cutout(value: &StateValue) -> Result<BoardCutout> {
    dataclass!(reader, value, "pcbir.physical:BoardCutout", {
        "id" => id = text;
        "vertices" => vertices = points;
    });
    reader.required(&[("id", id.is_none()), ("vertices", vertices.is_none())])?;
    Ok(BoardCutout::new(&id.expect("required"), vertices.expect("required"))?)
}

fn circular_boundary(value: &StateValue) -> Result<CircularBoardBoundary> {
    dataclass!(reader, value, "pcbir.physical:CircularBoardBoundary", {
        "center" => center = point;
        "radius_nm" => radius = int;
        "maximum_chord_error_nm" => chord = int;
    });
    reader.required(&[("center", center.is_none()), ("radius_nm", radius.is_none())])?;
    Ok(CircularBoardBoundary::new(center.expect("required"), radius.expect("required"), chord.unwrap_or(10_000))?)
}

fn boundary_segment(value: &StateValue) -> Result<BoundarySegment> {
    if let StateValue::Dataclass { name, .. } = value
        && name == "pcbir.physical:BoundaryArc"
    {
        dataclass!(reader, value, "pcbir.physical:BoundaryArc", {
            "id" => id = text;
            "start" => start = point;
            "mid" => mid = point;
            "end" => end = point;
        });
        reader.required(&[
            ("id", id.is_none()),
            ("start", start.is_none()),
            ("mid", mid.is_none()),
            ("end", end.is_none()),
        ])?;
        return Ok(BoundarySegment::arc(
            &id.expect("required"),
            start.expect("required"),
            mid.expect("required"),
            end.expect("required"),
        )?);
    }
    dataclass!(reader, value, "pcbir.physical:BoundaryLine", {
        "id" => id = text;
        "start" => start = point;
        "end" => end = point;
    });
    reader.required(&[("id", id.is_none()), ("start", start.is_none()), ("end", end.is_none())])?;
    Ok(BoundarySegment::line(&id.expect("required"), start.expect("required"), end.expect("required"))?)
}

fn boundary_path(value: &StateValue) -> Result<BoardBoundaryPath> {
    dataclass!(reader, value, "pcbir.physical:BoardBoundaryPath", {
        "segments" => segments = |v| list(v, boundary_segment);
        "maximum_chord_error_nm" => chord = int;
    });
    reader.required(&[("segments", segments.is_none())])?;
    Ok(BoardBoundaryPath::new(segments.expect("required"), chord.unwrap_or(10_000))?)
}

fn board_outline(value: &StateValue) -> Result<BoardOutline> {
    dataclass!(reader, value, "pcbir.physical:BoardOutline", {
        "vertices" => vertices = points;
        "cutouts" => cutouts = |v| list(v, board_cutout);
        "circular_boundary" => circular = |v| optional(v, circular_boundary);
        "boundary_path" => path = |v| optional(v, boundary_path);
    });
    reader.required(&[("vertices", vertices.is_none())])?;
    Ok(BoardOutline::new(vertices.expect("required"), cutouts.unwrap_or_default(), circular.flatten(), path.flatten())?)
}

fn copper_keepout(value: &StateValue) -> Result<CopperKeepout> {
    dataclass!(reader, value, "pcbir.physical:CopperKeepout", {
        "id" => id = text;
        "layers" => layers = |v| list(v, copper_layer);
        "outline" => outline = polygon_with_holes;
        "block_tracks" => block_tracks = boolean;
        "block_vias" => block_vias = boolean;
        "block_pads" => block_pads = boolean;
        "block_zones" => block_zones = boolean;
        "block_footprints" => block_footprints = boolean;
    });
    reader.required(&[("id", id.is_none()), ("layers", layers.is_none()), ("outline", outline.is_none())])?;
    let (vertices, holes) = outline.expect("required");
    if !holes.is_empty() {
        return not_implemented("a copper keepout outline with holes");
    }
    let keepout = CopperKeepout {
        id: id.expect("required"),
        layers: layers.expect("required"),
        vertices,
        block_tracks: block_tracks.unwrap_or(true),
        block_vias: block_vias.unwrap_or(true),
        block_pads: block_pads.unwrap_or(false),
        block_zones: block_zones.unwrap_or(true),
        block_footprints: block_footprints.unwrap_or(false),
    };
    keepout.validate()?;
    Ok(keepout)
}

fn footprint_layer(value: &StateValue) -> Result<FootprintLayer> {
    enumeration(value, "pcbir.physical:FootprintLayer", FootprintLayer::parse)
}

fn footprint_graphic(value: &StateValue) -> Result<FootprintGraphic> {
    let StateValue::Dataclass { name, .. } = value else { return dynamic("a footprint graphic", value) };
    Ok(match name.as_str() {
        "pcbir.physical:FootprintLine" => {
            dataclass!(reader, value, "pcbir.physical:FootprintLine", {
                "start" => start = point; "end" => end = point; "width_nm" => width = int; "layer" => layer = footprint_layer;
            });
            reader.required(&[("start", start.is_none()), ("end", end.is_none()), ("width_nm", width.is_none()), ("layer", layer.is_none())])?;
            FootprintGraphic::Line { start: start.expect("required"), end: end.expect("required"), width_nm: width.expect("required"), layer: layer.expect("required") }
        }
        "pcbir.physical:FootprintRectangle" => {
            dataclass!(reader, value, "pcbir.physical:FootprintRectangle", {
                "start" => start = point; "end" => end = point; "width_nm" => width = int; "layer" => layer = footprint_layer; "filled" => filled = boolean;
            });
            reader.required(&[("start", start.is_none()), ("end", end.is_none()), ("width_nm", width.is_none()), ("layer", layer.is_none())])?;
            FootprintGraphic::Rectangle { start: start.expect("required"), end: end.expect("required"), width_nm: width.expect("required"), layer: layer.expect("required"), filled: filled.unwrap_or(false) }
        }
        "pcbir.physical:FootprintCircle" => {
            dataclass!(reader, value, "pcbir.physical:FootprintCircle", {
                "center" => center = point; "end" => end = point; "width_nm" => width = int; "layer" => layer = footprint_layer; "filled" => filled = boolean;
            });
            reader.required(&[("center", center.is_none()), ("end", end.is_none()), ("width_nm", width.is_none()), ("layer", layer.is_none())])?;
            FootprintGraphic::Circle { center: center.expect("required"), end: end.expect("required"), width_nm: width.expect("required"), layer: layer.expect("required"), filled: filled.unwrap_or(false) }
        }
        "pcbir.physical:FootprintArc" => {
            dataclass!(reader, value, "pcbir.physical:FootprintArc", {
                "start" => start = point; "midpoint" => midpoint = point; "end" => end = point; "width_nm" => width = int; "layer" => layer = footprint_layer;
            });
            reader.required(&[("start", start.is_none()), ("midpoint", midpoint.is_none()), ("end", end.is_none()), ("width_nm", width.is_none()), ("layer", layer.is_none())])?;
            FootprintGraphic::Arc { start: start.expect("required"), midpoint: midpoint.expect("required"), end: end.expect("required"), width_nm: width.expect("required"), layer: layer.expect("required") }
        }
        "pcbir.physical:FootprintPolygon" => {
            dataclass!(reader, value, "pcbir.physical:FootprintPolygon", {
                "points" => pts = points; "width_nm" => width = int; "layer" => layer = footprint_layer; "filled" => filled = boolean;
            });
            reader.required(&[("points", pts.is_none()), ("width_nm", width.is_none()), ("layer", layer.is_none())])?;
            FootprintGraphic::Polygon { points: pts.expect("required"), width_nm: width.expect("required"), layer: layer.expect("required"), filled: filled.unwrap_or(false) }
        }
        other => return not_implemented(format!("a footprint graphic of type {other}")),
    }
    .validated()?)
}

fn footprint_pad(value: &StateValue) -> Result<FootprintPad> {
    dataclass!(reader, value, "pcbir.physical:FootprintPad", {
        "number" => number = text;
        "position" => position = point;
        "size" => pad_size = size;
        "kind" => pad_kind = |v| enumeration(v, "pcbir.physical:PadKind", PadKind::parse);
        "shape" => shape = |v| enumeration(v, "pcbir.physical:PadShape", PadShape::parse);
        "rotation_degrees" => rotation = decimal;
        "drill" => drill = |v| optional(v, size);
        "roundrect_ratio_ppm" => ratio = int;
        "has_solder_mask" => mask = boolean;
        "has_solder_paste" => paste = boolean;
        "zone_connection" => connection = |v| optional(v, |v| enumeration(v, "pcbir.physical:ZoneConnection", ZoneConnection::parse));
        "heatsink" => heatsink = boolean;
        "remove_unused_layers" => unused = boolean;
        "connector_contact" => contact = boolean;
    });
    reader.required(&[("number", number.is_none()), ("position", position.is_none()), ("size", pad_size.is_none())])?;
    Ok(FootprintPad {
        number: number.expect("required"),
        position: position.expect("required"),
        size: pad_size.expect("required"),
        kind: pad_kind.unwrap_or(PadKind::Smd),
        shape: shape.unwrap_or(PadShape::Roundrect),
        rotation_degrees: rotation.unwrap_or_else(Decimal::zero),
        drill: drill.flatten(),
        roundrect_ratio_ppm: ratio.unwrap_or(250_000),
        has_solder_mask: mask.unwrap_or(true),
        has_solder_paste: paste.unwrap_or(true),
        zone_connection: connection.flatten(),
        heatsink: heatsink.unwrap_or(false),
        remove_unused_layers: unused.unwrap_or(false),
        connector_contact: contact.unwrap_or(false),
    }
    .validated()?)
}

fn internal_pad_group(value: &StateValue) -> Result<InternalPadGroup> {
    dataclass!(reader, value, "pcbir.pad_connections:InternalPadGroup", {
        "numbers" => numbers = |v| list(v, text);
    });
    reader.required(&[("numbers", numbers.is_none())])?;
    Ok(InternalPadGroup::new(numbers.expect("required"))?)
}

fn physical_footprint(value: &StateValue) -> Result<PhysicalFootprint> {
    dataclass!(reader, value, "pcbir.physical:PhysicalFootprint", {
        "name" => name = text;
        "pads" => pads = |v| list(v, footprint_pad);
        "body_size" => body = size;
        "source_library_id" => source = |v| optional(v, text);
        "graphics" => graphics = |v| list(v, footprint_graphic);
        "keepouts" => keepouts = |v| list(v, copper_keepout);
        "metadata" => metadata = text_map;
        "courtyard" => courtyard = points;
        "height_nm" => height = |v| optional(v, int);
        "clearance_nm" => clearance = |v| optional(v, int);
        "exclude_from_bom" => bom = boolean;
        "exclude_from_pos_files" => pos = boolean;
        "internal_pad_groups" => groups = |v| list(v, internal_pad_group);
    });
    reader.required(&[("name", name.is_none()), ("pads", pads.is_none()), ("body_size", body.is_none())])?;
    Ok(PhysicalFootprint {
        name: name.expect("required"),
        pads: pads.expect("required"),
        body_size: body.expect("required"),
        source_library_id: source.flatten(),
        graphics: graphics.unwrap_or_default(),
        keepouts: keepouts.unwrap_or_default(),
        metadata: metadata.unwrap_or_default(),
        courtyard: courtyard.unwrap_or_default(),
        height_nm: height.flatten(),
        clearance_nm: clearance.flatten(),
        exclude_from_bom: bom.unwrap_or(false),
        exclude_from_pos_files: pos.unwrap_or(false),
        internal_pad_groups: groups.unwrap_or_default(),
    }
    .validated()?)
}

fn placement(value: &StateValue) -> Result<Placement> {
    dataclass!(reader, value, "pcbir.physical:Placement", {
        "reference" => reference = text;
        "footprint" => footprint = text;
        "position" => position = point;
        "rotation_degrees" => rotation = decimal;
        "side" => side = board_side;
        "value" => part_value = text;
        "source_path" => source = |v| optional(v, text);
    });
    reader.required(&[
        ("reference", reference.is_none()),
        ("footprint", footprint.is_none()),
        ("position", position.is_none()),
    ])?;
    let reference = reference.expect("required");
    if reference.is_empty() {
        return fail("placement reference cannot be empty");
    }
    Ok(Placement {
        reference,
        footprint: footprint.expect("required"),
        position: position.expect("required"),
        rotation_degrees: decimal_rem(&rotation.unwrap_or_else(Decimal::zero), 360)?,
        side: side.unwrap_or(BoardSide::Front),
        value: part_value.unwrap_or_default(),
        source_path: source.flatten(),
    })
}

fn physical_net(value: &StateValue) -> Result<PhysicalNet> {
    dataclass!(reader, value, "pcbir.physical:PhysicalNet", {
        "name" => name = text;
        "pads" => pads = |v| list(v, pad_reference);
    });
    reader.required(&[("name", name.is_none()), ("pads", pads.is_none())])?;
    let name = name.expect("required");
    if name.is_empty() {
        return fail("physical net name cannot be empty");
    }
    Ok(PhysicalNet { name, pads: pads.expect("required") })
}

fn stackup_layer(value: &StateValue) -> Result<StackupLayer> {
    dataclass!(reader, value, "pcbir.physical:StackupLayer", {
        "id" => id = text;
        "kind" => kind = |v| enumeration(v, "pcbir.physical:StackupLayerKind", |t| match t { "copper" => Some(true), "dielectric" => Some(false), _ => None });
        "thickness_nm" => thickness = int;
        "copper_layer" => copper = |v| optional(v, copper_layer);
        "material" => material = |v| optional(v, text);
        "relative_permittivity" => permittivity = |v| optional(v, decimal);
        "loss_tangent" => tangent = |v| optional(v, decimal);
        "dielectric_type" => dielectric = |v| optional(v, text);
    });
    reader.required(&[("id", id.is_none()), ("kind", kind.is_none()), ("thickness_nm", thickness.is_none())])?;
    let (id, is_copper, thickness) = (id.expect("required"), kind.expect("required"), thickness.expect("required"));
    let copper = copper.flatten();
    if id.is_empty() || thickness <= 0 {
        return fail("stackup layers require an id and positive thickness");
    }
    if is_copper != copper.is_some() {
        return fail("only copper stackup layers name a copper layer");
    }
    let (permittivity, tangent) = (permittivity.flatten(), tangent.flatten());
    if [&permittivity, &tangent].iter().any(|value| value.as_ref().is_some_and(|v| !(*v > Decimal::zero()))) {
        return fail("dielectric properties must be positive");
    }
    let dielectric = dielectric.flatten();
    if let Some(kind) = &dielectric {
        if is_copper {
            return fail("only dielectric stackup layers have a dielectric type");
        }
        if kind != "core" && kind != "prepreg" {
            return fail("dielectric type must be core or prepreg");
        }
    }
    Ok(StackupLayer {
        id,
        copper_layer: copper,
        thickness_nm: thickness,
        material: material.flatten(),
        relative_permittivity: permittivity,
        loss_tangent: tangent,
        dielectric_type: dielectric,
    })
}

fn via_technology(value: &StateValue) -> Result<ViaTechnology> {
    dataclass!(reader, value, "pcbir.physical:ViaTechnology", {
        "id" => id = text;
        "kind" => kind = |v| enumeration(v, "pcbir.physical:ViaKind", ViaKind::parse);
        "from_layer" => from = copper_layer;
        "to_layer" => to = copper_layer;
        "minimum_drill_nm" => drill = int;
        "minimum_annular_ring_nm" => ring = int;
        "maximum_aspect_ratio" => aspect = decimal;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("kind", kind.is_none()),
        ("from_layer", from.is_none()),
        ("to_layer", to.is_none()),
        ("minimum_drill_nm", drill.is_none()),
        ("minimum_annular_ring_nm", ring.is_none()),
        ("maximum_aspect_ratio", aspect.is_none()),
    ])?;
    let technology = ViaTechnology {
        id: id.expect("required"),
        kind: kind.expect("required"),
        from_layer: from.expect("required"),
        to_layer: to.expect("required"),
        minimum_drill_nm: drill.expect("required"),
        minimum_annular_ring_nm: ring.expect("required"),
        maximum_aspect_ratio: aspect.expect("required"),
    };
    technology.validate()?;
    Ok(technology)
}

fn stackup(value: &StateValue) -> Result<Stackup> {
    dataclass!(reader, value, "pcbir.physical:Stackup", {
        "copper_layers" => layers = |v| list(v, copper_layer);
        "thickness_nm" => thickness = int;
        "physical_layers" => physical = |v| list(v, stackup_layer);
        "via_technologies" => technologies = |v| list(v, via_technology);
    });
    let stackup = Stackup {
        copper_layers: layers.unwrap_or_else(|| vec![CopperLayer::Front, CopperLayer::Back]),
        thickness_nm: thickness.unwrap_or(1_600_000),
        physical_layers: physical.unwrap_or_default(),
        via_technologies: technologies.unwrap_or_default(),
    };
    validate_stackup(&stackup)?;
    Ok(stackup)
}

/// `Stackup.__post_init__`.
fn validate_stackup(stackup: &Stackup) -> Result<()> {
    let layers = &stackup.copper_layers;
    if layers.is_empty() {
        return fail("a stackup requires at least one copper layer");
    }
    if layers.iter().collect::<IndexSet<_>>().len() != layers.len() {
        return fail("stackup copper layers must be unique");
    }
    if layers.len() > 2 {
        let inner: Vec<CopperLayer> = CopperLayer::ALL
            .iter()
            .copied()
            .filter(|l| !matches!(l, CopperLayer::Front | CopperLayer::Back))
            .take(layers.len() - 2)
            .collect();
        let expected: Vec<CopperLayer> =
            std::iter::once(CopperLayer::Front).chain(inner).chain(std::iter::once(CopperLayer::Back)).collect();
        if *layers != expected {
            return fail("multilayer stackups use contiguous F.Cu/In1.Cu.../B.Cu order");
        }
    }
    if stackup.thickness_nm <= 0 {
        return fail("board thickness must be positive");
    }
    if !stackup.physical_layers.is_empty() {
        let explicit: Vec<CopperLayer> = stackup.physical_layers.iter().filter_map(|l| l.copper_layer).collect();
        if explicit != *layers {
            return fail("physical stackup copper order must match copper_layers");
        }
        if stackup.physical_layers.iter().map(|l| l.thickness_nm).sum::<i64>() != stackup.thickness_nm {
            return fail("physical stackup thickness must equal board thickness");
        }
    }
    let ids: IndexSet<&str> = stackup.via_technologies.iter().map(|t| t.id.as_str()).collect();
    if ids.len() != stackup.via_technologies.len() {
        return fail("via technology ids must be unique");
    }
    for technology in &stackup.via_technologies {
        let (Some(low), Some(high)) = (
            layers.iter().position(|l| *l == technology.from_layer),
            layers.iter().position(|l| *l == technology.to_layer),
        ) else {
            return fail("via technology span must be in the stackup");
        };
        let (low, high) = (low.min(high), low.max(high));
        if technology.kind == ViaKind::Through && (low, high) != (0, layers.len() - 1) {
            return fail("through-via technology must span the complete stackup");
        }
        if technology.kind == ViaKind::Microvia && high - low != 1 {
            return fail("microvias may span only adjacent copper layers");
        }
    }
    Ok(())
}

fn design_rules(value: &StateValue) -> Result<DesignRules> {
    dataclass!(reader, value, "pcbir.physical:DesignRules", {
        "minimum_clearance_nm" => clearance = int;
        "minimum_hole_clearance_nm" => hole_clearance = int;
        "minimum_track_width_nm" => minimum_track = int;
        "default_track_width_nm" => default_track = int;
        "default_via_size_nm" => via_size = int;
        "default_via_drill_nm" => via_drill = int;
        "minimum_slot_width_nm" => slot = int;
        "minimum_via_size_nm" => minimum_via_size = |v| optional(v, int);
        "minimum_via_drill_nm" => minimum_via_drill = |v| optional(v, int);
    });
    let defaults = DesignRules::default();
    let rules = DesignRules {
        minimum_clearance_nm: clearance.unwrap_or(defaults.minimum_clearance_nm),
        minimum_hole_clearance_nm: hole_clearance.unwrap_or(defaults.minimum_hole_clearance_nm),
        minimum_track_width_nm: minimum_track.unwrap_or(defaults.minimum_track_width_nm),
        default_track_width_nm: default_track.unwrap_or(defaults.default_track_width_nm),
        default_via_size_nm: via_size.unwrap_or(defaults.default_via_size_nm),
        default_via_drill_nm: via_drill.unwrap_or(defaults.default_via_drill_nm),
        minimum_slot_width_nm: slot.unwrap_or(defaults.minimum_slot_width_nm),
        minimum_via_size_nm: minimum_via_size.flatten(),
        minimum_via_drill_nm: minimum_via_drill.flatten(),
    };
    rules.validate()?;
    Ok(rules)
}

fn track_segment(value: &StateValue) -> Result<TrackSegment> {
    dataclass!(reader, value, "pcbir.physical:TrackSegment", {
        "net" => net = text;
        "start" => start = point;
        "end" => end = point;
        "width_nm" => width = int;
        "layer" => layer = copper_layer;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("start", start.is_none()),
        ("end", end.is_none()),
        ("width_nm", width.is_none()),
        ("layer", layer.is_none()),
    ])?;
    let (start, end, width) = (start.expect("required"), end.expect("required"), width.expect("required"));
    if start == end {
        return fail("track segment cannot have zero length");
    }
    if width <= 0 {
        return fail("track width must be positive");
    }
    Ok(TrackSegment { net: net.expect("required"), start, end, width_nm: width, layer: layer.expect("required") })
}

fn via(value: &StateValue) -> Result<Via> {
    dataclass!(reader, value, "pcbir.physical:Via", {
        "net" => net = text;
        "position" => position = point;
        "size_nm" => size_nm = int;
        "drill_nm" => drill = int;
        "from_layer" => from = copper_layer;
        "to_layer" => to = copper_layer;
        "technology" => technology = |v| optional(v, text);
        "finish" => finish = text;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("position", position.is_none()),
        ("size_nm", size_nm.is_none()),
        ("drill_nm", drill.is_none()),
    ])?;
    let (size_nm, drill) = (size_nm.expect("required"), drill.expect("required"));
    if size_nm <= 0 || drill <= 0 {
        return fail("via size and drill must be positive");
    }
    if drill >= size_nm {
        return fail("via drill must be smaller than via size");
    }
    let (from, to) = (from.unwrap_or(CopperLayer::Front), to.unwrap_or(CopperLayer::Back));
    if from == to {
        return fail("a via must connect distinct copper layers");
    }
    let finish = finish.unwrap_or_else(|| "standard".to_owned());
    if finish != "standard" && finish != "filled-capped" {
        return fail("unsupported via finish");
    }
    Ok(Via {
        net: net.expect("required"),
        position: position.expect("required"),
        size_nm,
        drill_nm: drill,
        from_layer: from,
        to_layer: to,
        technology: technology.flatten(),
        finish,
    })
}

fn copper_zone(value: &StateValue) -> Result<CopperZone> {
    dataclass!(reader, value, "pcbir.physical:CopperZone", {
        "id" => id = text;
        "net" => net = text;
        "layers" => layers = |v| list(v, copper_layer);
        "outline" => outline = polygon_with_holes;
        "priority" => priority = int;
        "clearance_nm" => clearance = |v| optional(v, int);
        "minimum_width_nm" => minimum_width = int;
        "pad_connection" => connection = |v| enumeration(v, "pcbir.physical:ZoneConnection", ZoneConnection::parse);
        "thermal" => thermal = thermal_relief;
        "island_policy" => island = |v| enumeration(v, "pcbir.physical:IslandPolicy", IslandPolicy::parse);
        "minimum_island_area_nm2" => island_area = |v| optional(v, int);
        "fill_mode" => fill_mode = |v| enumeration(v, "pcbir.physical:ZoneFillMode", |t| (t == "solid").then_some(()));
        "reserve_routing" => reserve = boolean;
        "allow_same_net_hard_macro_overlap" => overlap = boolean;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("net", net.is_none()),
        ("layers", layers.is_none()),
        ("outline", outline.is_none()),
    ])?;
    let _ = fill_mode;
    let (outer, holes) = outline.expect("required");
    let (gap, spoke) = thermal.unwrap_or((300_000, 300_000));
    let zone = CopperZone {
        id: id.expect("required"),
        net: net.expect("required"),
        layers: layers.expect("required"),
        outer,
        holes,
        priority: priority.unwrap_or(0),
        clearance_nm: clearance.flatten(),
        minimum_width_nm: minimum_width.unwrap_or(250_000),
        pad_connection: connection.unwrap_or(ZoneConnection::Thermal),
        thermal_gap_nm: gap,
        thermal_spoke_width_nm: spoke,
        island_policy: island.unwrap_or(IslandPolicy::RemoveBelowArea),
        minimum_island_area_nm2: island_area.unwrap_or(Some(10_000_000_000_000)),
        reserve_routing: reserve.unwrap_or(false),
        allow_same_net_hard_macro_overlap: overlap.unwrap_or(false),
    };
    zone.validate()?;
    Ok(zone)
}

/// `ThermalReliefSettings` as `(gap, spoke width)`; the spoke count must be the default.
fn thermal_relief(value: &StateValue) -> Result<(i64, i64)> {
    dataclass!(reader, value, "pcbir.physical:ThermalReliefSettings", {
        "gap_nm" => gap = int;
        "spoke_width_nm" => spoke = int;
        "spoke_count" => count = int;
    });
    let (gap, spoke, count) = (gap.unwrap_or(300_000), spoke.unwrap_or(300_000), count.unwrap_or(4));
    if gap <= 0 || spoke <= 0 {
        return fail("thermal relief gap and spoke width must be positive");
    }
    if !(2..=4).contains(&count) {
        return fail("thermal relief spoke count must be 2, 3, or 4");
    }
    if count != 4 {
        return not_implemented("a thermal relief with other than four spokes");
    }
    Ok((gap, spoke))
}

fn component_placement_rule(value: &StateValue) -> Result<ComponentPlacementRule> {
    dataclass!(reader, value, "pcbir.physical:ComponentPlacementRule", {
        "reference" => reference = text;
        "region" => region = |v| optional(v, text);
        "allowed_orientations" => orientations = |v| list(v, decimal);
        "fixed_position" => position = |v| optional(v, point);
        "fixed_rotation_degrees" => rotation = |v| optional(v, decimal);
        "side" => side = |v| optional(v, board_side);
        "priority" => priority = int;
        "edge_clearance_nm" => edge = |v| optional(v, int);
        "additional_courtyard_gap_nm" => gap = |v| optional(v, int);
    });
    reader.required(&[("reference", reference.is_none())])?;
    let mut rule = ComponentPlacementRule::new(&reference.expect("required"));
    rule.region = region.flatten();
    if let Some(orientations) = orientations {
        rule.allowed_orientations = orientations;
    }
    rule.fixed_position = position.flatten();
    rule.fixed_rotation_degrees = rotation.flatten();
    rule.side = side.flatten();
    rule.priority = priority.unwrap_or(0);
    rule.edge_clearance_nm = edge.flatten();
    rule.additional_courtyard_gap_nm = gap.flatten();
    Ok(rule.normalized()?)
}

fn net_routing_rule(value: &StateValue) -> Result<NetRoutingRule> {
    dataclass!(reader, value, "pcbir.physical:NetRoutingRule", {
        "net" => net = text;
        "kind" => kind = |v| enumeration(v, "pcbir.physical:RouteKind", RouteKind::parse);
        "priority" => priority = int;
        "width_nm" => width = |v| optional(v, int);
        "clearance_nm" => clearance = |v| optional(v, int);
        "allowed_layers" => layers = |v| list(v, copper_layer);
        "max_vias" => max_vias = |v| optional(v, int);
        "max_length_nm" => max_length = |v| optional(v, int);
        "differential_partner" => partner = |v| optional(v, text);
        "pair_gap_nm" => pair_gap = |v| optional(v, int);
        "max_skew_nm" => max_skew = |v| optional(v, int);
        "topology" => topology = text;
        "target_impedance_ohms" => impedance = |v| optional(v, int);
        "maximum_uncoupled_length_nm" => uncoupled = |v| optional(v, int);
        "maximum_stub_length_nm" => stub = |v| optional(v, int);
        "tuning_amplitude_limit_nm" => amplitude = |v| optional(v, int);
        "require_return_vias" => return_vias = boolean;
        "reserve_corridor" => corridor = boolean;
        "return_via_net" => return_net = |v| optional(v, text);
        "maximum_return_via_distance_nm" => return_distance = |v| optional(v, int);
        "impedance_evidence_digest" => digest = |v| optional(v, text);
        "return_via_policy" => policy = |v| enumeration(v, "pcbir.physical:ReturnViaPolicy", ReturnViaPolicy::parse);
        "shared_reference_layer" => shared = |v| optional(v, copper_layer);
        "target_single_ended_ohms" => single = |v| optional(v, int);
        "impedance_tolerance_percent" => tolerance = |v| optional(v, decimal);
        "layer_group" => layer_group = |v| optional(v, text);
        "breakout_length_nm" => breakout_length = |v| optional(v, int);
        "breakout_width_nm" => breakout_width = |v| optional(v, int);
        "breakout_gap_nm" => breakout_gap = |v| optional(v, int);
        "breakout_clearance_nm" => breakout_clearance = |v| optional(v, int);
        "tuning_style" => style = |v| enumeration(v, "pcbir.physical:TuningStyle", TuningStyle::parse);
        "tuning_spacing_nm" => spacing = |v| optional(v, int);
        "tuning_group" => group = |v| optional(v, text);
    });
    reader.required(&[("net", net.is_none())])?;
    let rule = NetRoutingRule {
        net: net.expect("required"),
        kind: kind.unwrap_or(RouteKind::General),
        priority: priority.unwrap_or(0),
        width_nm: width.flatten(),
        clearance_nm: clearance.flatten(),
        allowed_layers: layers.unwrap_or_default(),
        max_vias: max_vias.flatten(),
        max_length_nm: max_length.flatten(),
        differential_partner: partner.flatten(),
        pair_gap_nm: pair_gap.flatten(),
        max_skew_nm: max_skew.flatten(),
        topology: topology.unwrap_or_else(|| "point_to_point".to_owned()),
        target_impedance_ohms: impedance.flatten(),
        maximum_uncoupled_length_nm: uncoupled.flatten(),
        maximum_stub_length_nm: stub.flatten(),
        tuning_amplitude_limit_nm: amplitude.flatten(),
        require_return_vias: return_vias.unwrap_or(false),
        reserve_corridor: corridor.unwrap_or(false),
        return_via_net: return_net.flatten(),
        maximum_return_via_distance_nm: return_distance.flatten(),
        impedance_evidence_digest: digest.flatten(),
        return_via_policy: policy.unwrap_or(ReturnViaPolicy::Always),
        shared_reference_layer: shared.flatten(),
        target_single_ended_ohms: single.flatten(),
        impedance_tolerance_percent: tolerance.flatten(),
        layer_group: layer_group.flatten(),
        breakout_length_nm: breakout_length.flatten(),
        breakout_width_nm: breakout_width.flatten(),
        breakout_gap_nm: breakout_gap.flatten(),
        breakout_clearance_nm: breakout_clearance.flatten(),
        tuning_style: style.unwrap_or(TuningStyle::Bumps),
        tuning_spacing_nm: spacing.flatten(),
        tuning_group: group.flatten(),
    };
    rule.validate()?;
    Ok(rule)
}

fn copper_polygon(value: &StateValue) -> Result<CopperPolygon> {
    dataclass!(reader, value, "pcbir.physical:CopperPolygon", {
        "id" => id = text;
        "net" => net = text;
        "layer" => layer = copper_layer;
        "outline" => outline = polygon_ring;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("net", net.is_none()),
        ("layer", layer.is_none()),
        ("outline", outline.is_none()),
    ])?;
    let (id, net) = (id.expect("required"), net.expect("required"));
    if id.is_empty() || net.is_empty() {
        return fail("fixed copper polygon requires an id and net");
    }
    Ok(CopperPolygon {
        id,
        net,
        layer: layer.expect("required"),
        vertices: copper_physical::mechanical::validated_ring(outline.expect("required"))?,
    })
}

fn placement_target(value: &StateValue) -> Result<PlacementTarget> {
    dataclass!(reader, value, "pcbir.physical:PlacementTarget", {
        "reference" => reference = text;
        "pad" => pad = |v| optional(v, text);
    });
    reader.required(&[("reference", reference.is_none())])?;
    Ok(PlacementTarget { reference: reference.expect("required"), pad: pad.flatten() })
}

fn placement_region(value: &StateValue) -> Result<PlacementRegion> {
    dataclass!(reader, value, "pcbir.physical:PlacementRegion", {
        "name" => name = text;
        "outline" => outline = board_outline;
        "side" => side = |v| optional(v, board_side);
    });
    reader.required(&[("name", name.is_none()), ("outline", outline.is_none())])?;
    Ok(PlacementRegion { name: name.expect("required"), outline: outline.expect("required"), side: side.flatten() })
}

fn placement_keepout(value: &StateValue) -> Result<PlacementKeepout> {
    dataclass!(reader, value, "pcbir.physical:PlacementKeepout", {
        "name" => name = text;
        "outline" => outline = board_outline;
        "side" => side = |v| optional(v, board_side);
        "maximum_component_height_nm" => height = |v| optional(v, int);
    });
    reader.required(&[("name", name.is_none()), ("outline", outline.is_none())])?;
    Ok(PlacementKeepout::new(&name.expect("required"), outline.expect("required"), side.flatten(), height.flatten())?)
}

fn relative_placement_rule(value: &StateValue) -> Result<RelativePlacementRule> {
    dataclass!(reader, value, "pcbir.physical:RelativePlacementRule", {
        "kind" => kind = |v| enumeration(v, "pcbir.physical:RelativePlacementKind", RelativePlacementKind::parse);
        "targets" => targets = |v| list(v, placement_target);
        "distance_nm" => distance = |v| optional(v, int);
        "axis" => axis = |v| optional(v, |v| enumeration(v, "pcbir.physical:AlignmentAxis", AlignmentAxis::parse));
        "tolerance_nm" => tolerance = int;
        "weight" => weight = int;
    });
    reader.required(&[("kind", kind.is_none()), ("targets", targets.is_none())])?;
    Ok(RelativePlacementRule {
        kind: kind.expect("required"),
        targets: targets.expect("required"),
        distance_nm: distance.flatten(),
        axis: axis.flatten(),
        tolerance_nm: tolerance.unwrap_or(0),
        weight: weight.unwrap_or(1),
    })
}

fn placement_group(value: &StateValue) -> Result<PlacementGroup> {
    dataclass!(reader, value, "pcbir.physical:PlacementGroup", {
        "name" => name = text;
        "references" => references = |v| list(v, text);
        "anchor" => anchor = |v| optional(v, text);
        "priority" => priority = int;
        "source" => source = text;
    });
    reader.required(&[("name", name.is_none()), ("references", references.is_none())])?;
    Ok(PlacementGroup::new(
        &name.expect("required"),
        &references.expect("required"),
        anchor.flatten().as_deref(),
        priority.unwrap_or(0),
        &source.unwrap_or_else(|| "explicit".to_owned()),
    )?)
}

fn power_domain(value: &StateValue) -> Result<PhysicalPowerDomain> {
    dataclass!(reader, value, "pcbir.physical:PhysicalPowerDomain", {
        "net" => net = text;
        "members" => members = |v| list(v, pad_reference);
        "sources" => sources = |v| list(v, pad_reference);
        "source_ports" => ports = |v| list(v, text);
    });
    reader.required(&[("net", net.is_none()), ("members", members.is_none())])?;
    let domain = PhysicalPowerDomain {
        net: net.expect("required"),
        members: members.expect("required"),
        sources: sources.unwrap_or_default(),
        source_ports: ports.unwrap_or_default(),
    };
    if domain.net.is_empty() || domain.members.is_empty() {
        return fail("physical power domain requires a net and members");
    }
    let unique = |pads: &[PadReference]| pads.iter().collect::<IndexSet<_>>().len() == pads.len();
    if !unique(&domain.members) || !unique(&domain.sources) {
        return fail("physical power domain terminals must be unique");
    }
    if !domain.sources.iter().all(|s| domain.members.contains(s)) {
        return fail("physical power domain sources must be members");
    }
    Ok(domain)
}

fn power_feed(value: &StateValue) -> Result<PowerFeedBoundary> {
    dataclass!(reader, value, "pcbir.physical:PowerFeedBoundary", {
        "net" => net = text;
        "terminal" => terminal = pad_reference;
        "upstream_net" => upstream = text;
        "upstream_terminal" => upstream_terminal = pad_reference;
        "provenance" => provenance = text;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("terminal", terminal.is_none()),
        ("upstream_net", upstream.is_none()),
        ("upstream_terminal", upstream_terminal.is_none()),
    ])?;
    Ok(PowerFeedBoundary {
        net: net.expect("required"),
        terminal: terminal.expect("required"),
        upstream_net: upstream.expect("required"),
        upstream_terminal: upstream_terminal.expect("required"),
        provenance: provenance.unwrap_or_else(|| "dc_power_path".to_owned()),
    })
}

fn decoupling_link(value: &StateValue) -> Result<DecouplingLink> {
    dataclass!(reader, value, "pcbir.physical:DecouplingLink", {
        "capacitor" => capacitor = pad_reference;
        "target" => target = pad_reference;
        "return_pad" => return_pad = pad_reference;
        "net" => net = text;
        "return_net" => return_net = text;
    });
    reader.required(&[
        ("capacitor", capacitor.is_none()),
        ("target", target.is_none()),
        ("return_pad", return_pad.is_none()),
        ("net", net.is_none()),
        ("return_net", return_net.is_none()),
    ])?;
    Ok(DecouplingLink {
        capacitor: capacitor.expect("required"),
        target: target.expect("required"),
        return_pad: return_pad.expect("required"),
        net: net.expect("required"),
        return_net: return_net.expect("required"),
    })
}

fn net_match_group(value: &StateValue) -> Result<NetMatchGroup> {
    dataclass!(reader, value, "pcbir.physical:NetMatchGroup", {
        "id" => id = text;
        "nets" => nets = |v| list(v, text);
        "max_skew_nm" => skew = int;
    });
    reader.required(&[("id", id.is_none()), ("nets", nets.is_none()), ("max_skew_nm", skew.is_none())])?;
    Ok(NetMatchGroup { id: id.expect("required"), nets: nets.expect("required"), max_skew_nm: skew.expect("required") })
}

fn component_hole_clearance(value: &StateValue) -> Result<ComponentHoleClearance> {
    dataclass!(reader, value, "pcbir.physical:ComponentHoleClearance", {
        "reference" => reference = text;
        "clearance_nm" => clearance = int;
        "reason" => reason = text;
    });
    reader.required(&[
        ("reference", reference.is_none()),
        ("clearance_nm", clearance.is_none()),
        ("reason", reason.is_none()),
    ])?;
    Ok(ComponentHoleClearance {
        reference: reference.expect("required"),
        clearance_nm: clearance.expect("required"),
        reason: reason.expect("required"),
    })
}

fn via_in_pad_rule(value: &StateValue) -> Result<PadViaInPadRule> {
    dataclass!(reader, value, "pcbir.physical:PadViaInPadRule", {
        "pad" => pad = pad_reference;
        "process" => process = |v: &StateValue| Ok::<StateValue, Error>(v.clone());
        "rows" => rows = |v: &StateValue| Ok::<StateValue, Error>(v.clone());
        "columns" => columns = |v: &StateValue| Ok::<StateValue, Error>(v.clone());
        "pitch_nm" => pitch = |v| optional(v, int);
    });
    reader.required(&[("pad", pad.is_none())])?;
    // `PadViaInPadRule.__post_init__`.
    if process.is_some_and(|p| !matches!(p, StateValue::Str(ref text) if text == "filled-capped")) {
        return fail("via-in-pad supports only the filled-capped process");
    }
    let given = |count: &Option<StateValue>| count.as_ref().is_some_and(|v| !matches!(v, StateValue::Null));
    if given(&rows) != given(&columns) {
        return fail("via-in-pad array rows and columns must be given together");
    }
    let (rows, columns) = if given(&rows) {
        let message = "via-in-pad array rows and columns must be positive integers";
        let count = |v: &Option<StateValue>| strict_int(v.as_ref().expect("given"), message);
        let (rows, columns) = (count(&rows)?, count(&columns)?);
        if rows <= 0 || columns <= 0 {
            return fail(message);
        }
        (Some(rows), Some(columns))
    } else {
        (None, None)
    };
    let pitch = pitch.flatten();
    if pitch.is_some_and(|pitch| rows.is_none() || pitch <= 0) {
        return fail("via-in-pad array pitch must be positive and requires rows and columns");
    }
    Ok(PadViaInPadRule { pad: pad.expect("required"), rows, columns, pitch_nm: pitch })
}

fn mechanical_hole(value: &StateValue) -> Result<MechanicalHole> {
    dataclass!(reader, value, "pcbir.physical:MechanicalHole", {
        "id" => id = text;
        "position" => position = point;
        "diameter_nm" => diameter = int;
        "head_clearance_radius_nm" => head = int;
    });
    reader.required(&[("id", id.is_none()), ("position", position.is_none()), ("diameter_nm", diameter.is_none())])?;
    Ok(MechanicalHole::new(
        &id.expect("required"),
        position.expect("required"),
        diameter.expect("required"),
        head.unwrap_or(0),
    )?)
}

fn mechanical_slot(value: &StateValue) -> Result<MechanicalSlot> {
    dataclass!(reader, value, "pcbir.physical:MechanicalSlot", {
        "id" => id = text;
        "start" => start = point;
        "end" => end = point;
        "width_nm" => width = int;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("start", start.is_none()),
        ("end", end.is_none()),
        ("width_nm", width.is_none()),
    ])?;
    Ok(MechanicalSlot::new(
        &id.expect("required"),
        start.expect("required"),
        end.expect("required"),
        width.expect("required"),
    )?)
}

fn board_datum(value: &StateValue) -> Result<BoardDatum> {
    dataclass!(reader, value, "pcbir.physical:BoardDatum", {
        "id" => id = text;
        "position" => position = point;
        "relative_to" => relative = |v| optional(v, text);
        "offset" => offset = point;
    });
    reader.required(&[("id", id.is_none()), ("position", position.is_none())])?;
    Ok(BoardDatum {
        id: id.expect("required"),
        position: position.expect("required"),
        relative_to: relative.flatten(),
        offset: offset.unwrap_or(Point::new(0, 0)),
    })
}

fn board_edge(value: &StateValue) -> Result<BoardEdge> {
    dataclass!(reader, value, "pcbir.physical:BoardEdge", {
        "id" => id = text;
        "start" => start = point;
        "end" => end = point;
    });
    reader.required(&[("id", id.is_none()), ("start", start.is_none()), ("end", end.is_none())])?;
    Ok(BoardEdge::new(&id.expect("required"), start.expect("required"), end.expect("required"))?)
}

fn attachment(value: &StateValue) -> Result<PhysicalAttachment> {
    dataclass!(reader, value, "pcbir.physical:PhysicalAttachment", {
        "id" => id = text;
        "reference" => reference = text;
        "target" => target = |v| optional(v, text);
        "position" => position = point;
        "offset" => offset = point;
        "anchor" => anchor = text;
        "anchor_point" => anchor_point = point;
        "rotation" => rotation = decimal;
        "side" => side = board_side;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("reference", reference.is_none()),
        ("target", target.is_none()),
        ("position", position.is_none()),
        ("offset", offset.is_none()),
        ("anchor", anchor.is_none()),
        ("anchor_point", anchor_point.is_none()),
        ("rotation", rotation.is_none()),
        ("side", side.is_none()),
    ])?;
    Ok(PhysicalAttachment {
        id: id.expect("required"),
        reference: reference.expect("required"),
        target: target.expect("required"),
        position: position.expect("required"),
        offset: offset.expect("required"),
        anchor: anchor.expect("required"),
        anchor_point: anchor_point.expect("required"),
        rotation: rotation.expect("required"),
        side: side.expect("required"),
    })
}

fn body_overhang(value: &StateValue) -> Result<BodyOverhang> {
    dataclass!(reader, value, "pcbir.physical:BodyOverhang", {
        "id" => id = text;
        "reference" => reference = text;
        "edge" => edge = text;
        "start_nm" => start = int;
        "end_nm" => end = int;
        "distance_nm" => distance = int;
        "reason" => reason = text;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("reference", reference.is_none()),
        ("edge", edge.is_none()),
        ("start_nm", start.is_none()),
        ("end_nm", end.is_none()),
        ("distance_nm", distance.is_none()),
        ("reason", reason.is_none()),
    ])?;
    Ok(BodyOverhang {
        id: id.expect("required"),
        reference: reference.expect("required"),
        edge: edge.expect("required"),
        start_nm: start.expect("required"),
        end_nm: end.expect("required"),
        distance_nm: distance.expect("required"),
        reason: reason.expect("required"),
    })
}

fn component_height(value: &StateValue) -> Result<ComponentHeight> {
    dataclass!(reader, value, "pcbir.physical:ComponentHeight", {
        "id" => id = text;
        "reference" => reference = text;
        "height_nm" => height = int;
    });
    reader.required(&[("id", id.is_none()), ("reference", reference.is_none()), ("height_nm", height.is_none())])?;
    Ok(ComponentHeight {
        id: id.expect("required"),
        reference: reference.expect("required"),
        height_nm: height.expect("required"),
    })
}

fn assembly_envelope(value: &StateValue) -> Result<AssemblyEnvelope> {
    dataclass!(reader, value, "pcbir.physical:AssemblyEnvelope", {
        "id" => id = text;
        "outline" => outline = board_outline;
        "side" => side = board_side;
        "maximum_height_nm" => height = int;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("outline", outline.is_none()),
        ("side", side.is_none()),
        ("maximum_height_nm", height.is_none()),
    ])?;
    Ok(AssemblyEnvelope {
        id: id.expect("required"),
        outline: outline.expect("required"),
        side: side.expect("required"),
        maximum_height_nm: height.expect("required"),
    })
}

fn assembly_access(value: &StateValue) -> Result<AssemblyAccess> {
    dataclass!(reader, value, "pcbir.physical:AssemblyAccess", {
        "id" => id = text;
        "reference" => reference = text;
        "outline" => outline = board_outline;
        "side" => side = text;
        "purpose" => purpose = text;
    });
    reader.required(&[
        ("id", id.is_none()),
        ("reference", reference.is_none()),
        ("outline", outline.is_none()),
        ("side", side.is_none()),
        ("purpose", purpose.is_none()),
    ])?;
    Ok(AssemblyAccess {
        id: id.expect("required"),
        reference: reference.expect("required"),
        outline: outline.expect("required"),
        side: side.expect("required"),
        purpose: purpose.expect("required"),
    })
}

fn artwork_material(value: &StateValue) -> Result<ArtworkMaterial> {
    enumeration(value, "pcbir.silkscreen:ArtworkMaterial", |t| match t {
        "silkscreen" => Some(ArtworkMaterial::Silkscreen),
        "copper" => Some(ArtworkMaterial::Copper),
        _ => None,
    })
}

fn silkscreen(value: &StateValue) -> Result<Artwork> {
    let StateValue::Dataclass { name, .. } = value else { return dynamic("silkscreen artwork", value) };
    if name == "pcbir.silkscreen:SilkscreenText" {
        dataclass!(reader, value, "pcbir.silkscreen:SilkscreenText", {
            "id" => id = text;
            "text" => body = text;
            "position" => position = point;
            "size_nm" => size_nm = int;
            "thickness_nm" => thickness = int;
            "rotation_degrees" => rotation = decimal;
            "side" => side = board_side;
            "material" => material = artwork_material;
            "exposed" => exposed = boolean;
        });
        reader.required(&[("id", id.is_none()), ("text", body.is_none()), ("position", position.is_none())])?;
        return Ok(Artwork::Text {
            id: id.expect("required"),
            text: body.expect("required"),
            position: position.expect("required"),
            size_nm: size_nm.unwrap_or(1_000_000),
            thickness_nm: thickness.unwrap_or(150_000),
            rotation_degrees: rotation.unwrap_or_else(Decimal::zero),
            side: side.unwrap_or(BoardSide::Front),
            material: material.unwrap_or(ArtworkMaterial::Silkscreen),
            exposed: exposed.unwrap_or(false),
        });
    }
    dataclass!(reader, value, "pcbir.silkscreen:SilkscreenGraphic", {
        "id" => id = text;
        "graphic" => graphic = footprint_graphic;
        "side" => side = board_side;
        "material" => material = artwork_material;
        "exposed" => exposed = boolean;
    });
    reader.required(&[("id", id.is_none()), ("graphic", graphic.is_none())])?;
    let graphic = graphic.expect("required");
    if graphic.layer() != FootprintLayer::Silkscreen {
        return fail("board silkscreen graphics must use the silkscreen layer");
    }
    let graphic = match graphic {
        FootprintGraphic::Line { start, end, width_nm, .. } => Graphic::Line { start, end, width_nm },
        FootprintGraphic::Rectangle { start, end, width_nm, filled, .. } => {
            Graphic::Rectangle { start, end, width_nm, filled }
        }
        FootprintGraphic::Circle { center, end, width_nm, filled, .. } => {
            Graphic::Circle { center, end, width_nm, filled }
        }
        FootprintGraphic::Arc { start, midpoint, end, width_nm, .. } => Graphic::Arc { start, midpoint, end, width_nm },
        FootprintGraphic::Polygon { points, width_nm, filled, .. } => Graphic::Polygon { points, width_nm, filled },
    };
    Ok(Artwork::Graphic {
        id: id.expect("required"),
        graphic,
        side: side.unwrap_or(BoardSide::Front),
        material: material.unwrap_or(ArtworkMaterial::Silkscreen),
        exposed: exposed.unwrap_or(false),
    })
}

fn rigid_member(value: &StateValue) -> Result<RigidPlacementMember> {
    dataclass!(reader, value, "pcbir.physical:RigidPlacementMember", {
        "reference" => reference = text;
        "footprint" => footprint = text;
        "footprint_digest" => digest = text;
        "position" => position = point;
        "rotation_degrees" => rotation = decimal;
    });
    reader.required(&[
        ("reference", reference.is_none()),
        ("footprint", footprint.is_none()),
        ("footprint_digest", digest.is_none()),
        ("position", position.is_none()),
    ])?;
    Ok(RigidPlacementMember::new(
        &reference.expect("required"),
        &footprint.expect("required"),
        &digest.expect("required"),
        position.expect("required"),
        &rotation.unwrap_or_else(Decimal::zero),
    )?)
}

fn edge_opening(value: &StateValue) -> Result<MacroEdgeOpening> {
    dataclass!(reader, value, "pcbir.physical:MacroEdgeOpening", {
        "keepout" => keepout = text;
        "edge_indices" => indices = |v| list(v, int);
    });
    reader.required(&[("keepout", keepout.is_none()), ("edge_indices", indices.is_none())])?;
    Ok(MacroEdgeOpening { keepout: keepout.expect("required"), edge_indices: indices.expect("required") })
}

fn rigid_cluster(value: &StateValue) -> Result<RigidPlacementCluster> {
    dataclass!(reader, value, "pcbir.physical:RigidPlacementCluster", {
        "name" => name = text;
        "anchor" => anchor = placement_target;
        "members" => members = |v| list(v, rigid_member);
        "source" => source = text;
        "allowed_rotations" => rotations = |v| list(v, decimal);
        "keepouts" => keepouts = |v| list(v, copper_keepout);
        "internal_clearance_nm" => clearance = |v| optional(v, int);
        "edge_openings" => openings = |v| list(v, edge_opening);
    });
    reader.required(&[
        ("name", name.is_none()),
        ("anchor", anchor.is_none()),
        ("members", members.is_none()),
        ("source", source.is_none()),
    ])?;
    let anchor = anchor.expect("required");
    Ok(RigidPlacementCluster {
        name: name.expect("required"),
        anchor_reference: anchor.reference,
        anchor_pad: anchor.pad,
        members: members.expect("required"),
        source: source.expect("required"),
        allowed_rotations: rotations
            .unwrap_or_else(|| [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect()),
        keepouts: keepouts.unwrap_or_default(),
        internal_clearance_nm: clearance.flatten(),
        edge_openings: openings.unwrap_or_default(),
    }
    .normalized()?)
}

fn macro_port(value: &StateValue) -> Result<MacroPort> {
    dataclass!(reader, value, "pcbir.physical:MacroPort", {
        "name" => name = text;
        "net" => net = text;
        "position" => position = point;
        "layer" => layer = copper_layer;
        "pads" => pads = |v| list(v, pad_reference);
    });
    reader.required(&[
        ("name", name.is_none()),
        ("net", net.is_none()),
        ("position", position.is_none()),
        ("layer", layer.is_none()),
    ])?;
    let (name, net) = (name.expect("required"), net.expect("required"));
    if name.is_empty() || net.is_empty() {
        return fail("macro port requires a name and net");
    }
    Ok(MacroPort {
        name,
        net,
        position: position.expect("required"),
        layer: layer.expect("required"),
        pads: pads.unwrap_or_default(),
    })
}

fn pad_binding(value: &StateValue) -> Result<(PadReference, String)> {
    dataclass!(reader, value, "pcbir.physical:MacroPadBinding", {
        "pad" => pad = pad_reference;
        "net" => net = text;
    });
    reader.required(&[("pad", pad.is_none()), ("net", net.is_none())])?;
    Ok((pad.expect("required"), net.expect("required")))
}

fn plane_return(value: &StateValue) -> Result<MacroPlaneReturn> {
    dataclass!(reader, value, "pcbir.physical:MacroPlaneReturn", {
        "net" => net = text;
        "layers" => layers = |v| list(v, copper_layer);
        "pads" => pads = |v| list(v, pad_reference);
        "dedicated_contacts" => contacts = |v| list(v, |v| match v {
            StateValue::Tuple(items) | StateValue::List(items) if items.len() == 2 => Ok((pad_reference(&items[0])?, point(&items[1])?)),
            other => dynamic("a (pad, point) pair", other),
        });
    });
    reader.required(&[("net", net.is_none()), ("layers", layers.is_none()), ("pads", pads.is_none())])?;
    Ok(MacroPlaneReturn {
        net: net.expect("required"),
        layers: layers.expect("required"),
        pads: pads.expect("required"),
        dedicated_contacts: contacts.unwrap_or_default(),
    })
}

fn width_contract(value: &StateValue) -> Result<MacroTrackWidthContract> {
    dataclass!(reader, value, "pcbir.physical:MacroTrackWidthContract", {
        "track_index" => index = int;
        "minimum_width_nm" => width = int;
        "purpose" => purpose = text;
        "evidence" => evidence = text;
    });
    reader.required(&[
        ("track_index", index.is_none()),
        ("minimum_width_nm", width.is_none()),
        ("purpose", purpose.is_none()),
        ("evidence", evidence.is_none()),
    ])?;
    let index = index.expect("required");
    if index < 0 {
        return fail("macro width contract track index is out of range");
    }
    Ok(MacroTrackWidthContract {
        track_index: index as usize,
        minimum_width_nm: width.expect("required"),
        purpose: purpose.expect("required"),
        evidence: evidence.expect("required"),
    })
}

fn hard_macro(value: &StateValue) -> Result<PhysicalHardMacro> {
    dataclass!(reader, value, "pcbir.physical:PhysicalHardMacro", {
        "cluster" => cluster = text;
        "asset_sha256" => asset = text;
        "tracks" => tracks = |v| list(v, track_segment);
        "vias" => vias = |v| list(v, via);
        "ports" => ports = |v| list(v, macro_port);
        "protected_regions" => regions = |v| list(v, copper_keepout);
        "required_layers" => layers = |v| list(v, copper_layer);
        "pad_bindings" => bindings = |v| list(v, pad_binding);
        "isolated_pads" => isolated = |v| list(v, pad_reference);
        "zones" => zones = |v| list(v, copper_zone);
        "plane_returns" => returns = |v| list(v, plane_return);
        "polygons" => polygons = |v| list(v, copper_polygon);
        "width_contracts" => contracts = |v| list(v, width_contract);
    });
    reader.required(&[
        ("cluster", cluster.is_none()),
        ("asset_sha256", asset.is_none()),
        ("tracks", tracks.is_none()),
    ])?;
    let item = PhysicalHardMacro {
        cluster: cluster.expect("required"),
        asset_sha256: asset.expect("required"),
        tracks: tracks.expect("required"),
        vias: vias.unwrap_or_default(),
        ports: ports.unwrap_or_default(),
        protected_regions: regions.unwrap_or_default(),
        required_layers: layers.unwrap_or_default(),
        pad_bindings: bindings.unwrap_or_default(),
        isolated_pads: isolated.unwrap_or_default(),
        zones: zones.unwrap_or_default(),
        plane_returns: returns.unwrap_or_default(),
        polygons: polygons.unwrap_or_default(),
        width_contracts: contracts.unwrap_or_default(),
    };
    item.validate()?;
    Ok(item)
}

/// `PhysicalBoard(**fields)`: the dataclass and the checks of its
/// `__post_init__` and `_validate_references` the rewrite covers.
pub fn physical_board(value: &StateValue) -> Result<PhysicalBoard> {
    dataclass!(reader, value, "pcbir.physical:PhysicalBoard", {
        "name" => name = text;
        "outline" => outline = board_outline;
        "footprints" => footprints = |v: &StateValue| match v {
            StateValue::Map { entries, .. } => {
                let mut map = IndexMap::new();
                for (key, item) in entries {
                    map.insert(text(key)?, physical_footprint(item)?);
                }
                Ok(map)
            }
            other => dynamic("a footprint mapping", other),
        };
        "placements" => placements = |v| list(v, placement);
        "nets" => nets = |v| list(v, physical_net);
        "stackup" => board_stackup = stackup;
        "rules" => rules = design_rules;
        "tracks" => tracks = |v| list(v, track_segment);
        "vias" => vias = |v| list(v, via);
        "metadata" => metadata = text_map;
        "regions" => regions = |v| list(v, placement_region);
        "keepouts" => keepouts = |v| list(v, placement_keepout);
        "placement_rules" => placement_rules = |v| list(v, component_placement_rule);
        "relative_rules" => relative_rules = |v| list(v, relative_placement_rule);
        "placement_groups" => groups = |v| list(v, placement_group);
        "power_domains" => domains = |v| list(v, power_domain);
        "net_routing_rules" => routing_rules = |v| list(v, net_routing_rule);
        "zones" => zones = |v| list(v, copper_zone);
        "polygons" => polygons = |v| list(v, copper_polygon);
        "copper_keepouts" => copper_keepouts = |v| list(v, copper_keepout);
        "zone_fills" => fills = |v| list(v, |v| not_implemented::<()>(format!("a zone fill result ({})", kind(v))));
        "rigid_clusters" => clusters = |v| list(v, rigid_cluster);
        "hard_macros" => macros = |v| list(v, hard_macro);
        "materialized_macros" => materialized = |v| list(v, text);
        "via_in_pad_rules" => via_in_pad = |v| list(v, via_in_pad_rule);
        "mechanical_holes" => holes = |v| list(v, mechanical_hole);
        "datums" => datums = |v| list(v, board_datum);
        "boundary_edges" => edges = |v| list(v, board_edge);
        "attachments" => attachments = |v| list(v, attachment);
        "body_overhangs" => overhangs = |v| list(v, body_overhang);
        "component_heights" => heights = |v| list(v, component_height);
        "assembly_envelopes" => envelopes = |v| list(v, assembly_envelope);
        "assembly_access" => access = |v| list(v, assembly_access);
        "mechanical_slots" => slots = |v| list(v, mechanical_slot);
        "mechanical_references" => references = |v| list(v, |v| not_implemented::<()>(format!("a mechanical reference ({})", kind(v))));
        "match_groups" => match_groups = |v| list(v, net_match_group);
        "component_hole_clearances" => hole_clearances = |v| list(v, component_hole_clearance);
        "decoupling_links" => links = |v| list(v, decoupling_link);
        "silkscreen" => artwork = |v| list(v, silkscreen);
        "power_feeds" => feeds = |v| list(v, power_feed);
    });
    reader.required(&[
        ("name", name.is_none()),
        ("outline", outline.is_none()),
        ("footprints", footprints.is_none()),
        ("placements", placements.is_none()),
        ("nets", nets.is_none()),
    ])?;
    let _ = (fills, references);
    let board = PhysicalBoard {
        name: name.expect("required"),
        outline: outline.expect("required"),
        footprints: footprints.expect("required"),
        placements: placements.expect("required"),
        nets: nets.expect("required"),
        stackup: board_stackup.unwrap_or_else(|| Stackup::new(vec![CopperLayer::Front, CopperLayer::Back])),
        rules: rules.unwrap_or_default(),
        tracks: tracks.unwrap_or_default(),
        vias: vias.unwrap_or_default(),
        metadata: metadata.unwrap_or_default(),
        regions: regions.unwrap_or_default(),
        keepouts: keepouts.unwrap_or_default(),
        placement_rules: placement_rules.unwrap_or_default(),
        relative_rules: relative_rules.unwrap_or_default(),
        placement_groups: groups.unwrap_or_default(),
        power_domains: domains.unwrap_or_default(),
        net_routing_rules: routing_rules.unwrap_or_default(),
        zones: zones.unwrap_or_default(),
        polygons: polygons.unwrap_or_default(),
        copper_keepouts: copper_keepouts.unwrap_or_default(),
        rigid_clusters: clusters.unwrap_or_default(),
        hard_macros: macros.unwrap_or_default(),
        materialized_macros: materialized.unwrap_or_default(),
        via_in_pad_rules: via_in_pad.unwrap_or_default(),
        mechanical_holes: holes.unwrap_or_default(),
        mechanical_slots: slots.unwrap_or_default(),
        datums: datums.unwrap_or_default(),
        boundary_edges: edges.unwrap_or_default(),
        attachments: attachments.unwrap_or_default(),
        component_heights: heights.unwrap_or_default(),
        body_overhangs: overhangs.unwrap_or_default(),
        assembly_envelopes: envelopes.unwrap_or_default(),
        assembly_access: access.unwrap_or_default(),
        match_groups: match_groups.unwrap_or_default(),
        component_hole_clearances: hole_clearances.unwrap_or_default(),
        decoupling_links: links.unwrap_or_default(),
        silkscreen: artwork.unwrap_or_default(),
        power_feeds: feeds.unwrap_or_default(),
        mechanical_references: Vec::new(),
    };
    validate_board(&board)?;
    Ok(board)
}

/// The checks of `PhysicalBoard.__post_init__` and `_validate_references`
/// that a saved board can fail; a board whose remaining checks Python would
/// run (datum chains, overhang geometry, via-in-pad arrays) is handled by
/// the rewrite only in its common cases.
fn validate_board(board: &PhysicalBoard) -> Result<()> {
    let capacitors: IndexSet<&str> = board.decoupling_links.iter().map(|l| l.capacitor.component.as_str()).collect();
    if capacitors.len() != board.decoupling_links.len() {
        return fail("a decoupler requires one explicit protected pin association");
    }
    let net_members: IndexMap<&str, IndexSet<&PadReference>> =
        board.nets.iter().map(|n| (n.name.as_str(), n.pads.iter().collect())).collect();
    for feed in &board.power_feeds {
        if feed.net == feed.upstream_net
            || feed.provenance != "dc_power_path"
            || !net_members.get(feed.net.as_str()).is_some_and(|pads| pads.contains(&feed.terminal))
            || !net_members.get(feed.upstream_net.as_str()).is_some_and(|pads| pads.contains(&feed.upstream_terminal))
            || feed.terminal.component != feed.upstream_terminal.component
        {
            return fail("invalid passive power feed boundary");
        }
    }
    let unique_ids = |field: &str, ids: Vec<&str>| -> Result<()> {
        if ids.iter().collect::<IndexSet<_>>().len() != ids.len() {
            return fail(format!("duplicate {field} identity"));
        }
        Ok(())
    };
    unique_ids("datums", board.datums.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("boundary_edges", board.boundary_edges.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("attachments", board.attachments.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("body_overhangs", board.body_overhangs.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("component_heights", board.component_heights.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("assembly_envelopes", board.assembly_envelopes.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("assembly_access", board.assembly_access.iter().map(|d| d.id.as_str()).collect())?;
    unique_ids("mechanical_slots", board.mechanical_slots.iter().map(|d| d.id.as_str()).collect())?;
    copper_physical::mechanical::validate_mechanical_holes(
        &board.outline,
        &board.mechanical_holes,
        &board.mechanical_slots,
        &board.rules,
    )?;
    let references: Vec<&str> = board.placements.iter().map(|p| p.reference.as_str()).collect();
    if references.iter().collect::<IndexSet<_>>().len() != references.len() {
        return fail("physical placement references must be unique");
    }
    if !board.datums.is_empty()
        || !board.boundary_edges.is_empty()
        || !board.attachments.is_empty()
        || !board.body_overhangs.is_empty()
    {
        return not_implemented("a board with datums, named edges, attachments or body overhangs");
    }
    for feature in board
        .component_heights
        .iter()
        .map(|f| f.reference.as_str())
        .chain(board.assembly_access.iter().map(|f| f.reference.as_str()))
    {
        if !references.contains(&feature) {
            return fail("component_heights requires unique known component owners");
        }
    }
    let net_names: Vec<&str> = board.nets.iter().map(|n| n.name.as_str()).collect();
    if net_names.iter().collect::<IndexSet<_>>().len() != net_names.len() {
        return fail("physical net names must be unique");
    }
    let zone_ids: Vec<&str> = board.zones.iter().map(|z| z.id.as_str()).collect();
    if zone_ids.iter().collect::<IndexSet<_>>().len() != zone_ids.len() {
        return fail("copper zone ids must be unique");
    }
    if board.polygons.iter().map(|p| p.id.as_str()).collect::<IndexSet<_>>().len() != board.polygons.len() {
        return fail("fixed copper polygon ids must be unique");
    }
    let keepout_ids: Vec<&str> = board.copper_keepouts.iter().map(|k| k.id.as_str()).collect();
    if keepout_ids.iter().collect::<IndexSet<_>>().len() != keepout_ids.len() {
        return fail("copper keepout ids must be unique");
    }
    let layers = &board.stackup.copper_layers;
    for zone in &board.zones {
        if !net_names.contains(&zone.net.as_str()) {
            return fail(format!(
                "copper zone {} references unknown net {}",
                python_str_repr(&zone.id),
                python_str_repr(&zone.net)
            ));
        }
        if !zone.layers.iter().all(|l| layers.contains(l)) {
            return fail(format!("copper zone {} references a layer outside the stackup", python_str_repr(&zone.id)));
        }
    }
    for polygon in &board.polygons {
        if !net_names.contains(&polygon.net.as_str()) || !layers.contains(&polygon.layer) {
            return fail(format!("fixed copper polygon {} has an unknown net or layer", python_str_repr(&polygon.id)));
        }
    }
    copper_pcb::zones::validate_routing_reservations(&board.zones)?;
    for keepout in &board.copper_keepouts {
        if !keepout.layers.iter().all(|l| layers.contains(l)) {
            return fail(format!(
                "copper keepout {} references a layer outside the stackup",
                python_str_repr(&keepout.id)
            ));
        }
    }
    let mut pads_by_component: IndexMap<&str, IndexMap<&str, &FootprintPad>> = IndexMap::new();
    for placement in &board.placements {
        let Some(footprint) = board.footprints.get(&placement.footprint) else {
            return fail(format!(
                "placement {} uses unknown footprint {}",
                python_str_repr(&placement.reference),
                python_str_repr(&placement.footprint)
            ));
        };
        pads_by_component.insert(&placement.reference, footprint.pads.iter().map(|p| (p.number.as_str(), p)).collect());
    }
    let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            let Some(pads) = pads_by_component.get(pad.component.as_str()) else {
                return fail(format!(
                    "net {} references unknown placement {}",
                    python_str_repr(&net.name),
                    python_str_repr(&pad.component)
                ));
            };
            let Some(physical) = pads.get(pad.pad.as_str()) else {
                return fail(format!(
                    "net {} references unknown pad {}.{}",
                    python_str_repr(&net.name),
                    pad.component,
                    pad.pad
                ));
            };
            if matches!(physical.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
                return fail(format!(
                    "non-electrical pad {}.{} cannot belong to an electrical net",
                    pad.component, pad.pad
                ));
            }
            if let Some(previous) = assigned.get(pad) {
                return fail(format!(
                    "pad {}.{} belongs to both {} and {}",
                    pad.component,
                    pad.pad,
                    python_str_repr(previous),
                    python_str_repr(&net.name)
                ));
            }
            assigned.insert(pad, &net.name);
        }
    }
    for link in &board.decoupling_links {
        if assigned.get(&link.capacitor).copied() != Some(link.net.as_str())
            || assigned.get(&link.target).copied() != Some(link.net.as_str())
            || assigned.get(&link.return_pad).copied() != Some(link.return_net.as_str())
        {
            return fail("decoupling association must match actual pad nets");
        }
    }
    for placement in &board.placements {
        for group in &board.footprints[&placement.footprint].internal_pad_groups {
            let nets: IndexSet<Option<&str>> = group
                .numbers
                .iter()
                .map(|n| assigned.get(&PadReference::new(&placement.reference, n)).copied())
                .collect();
            let known = nets.iter().filter(|n| n.is_some()).count();
            if known > 1 {
                return fail("internally connected pads belong to different nets");
            }
            if nets.contains(&None) && nets.len() > 1 {
                return fail("all numbered pads in an internal group must share a net");
            }
        }
    }
    let mut permitted: IndexSet<&PadReference> = IndexSet::new();
    for rule in &board.via_in_pad_rules {
        if !permitted.insert(&rule.pad) {
            return fail("multiple via-in-pad permissions for one pad");
        }
        if assigned.get(&rule.pad).copied() != Some("GND") {
            return fail("via-in-pad permission requires a connected GND pad");
        }
        let placement = board.placements.iter().find(|p| p.reference == rule.pad.component).expect("placed pad");
        let lands: Vec<_> =
            board.footprints[&placement.footprint].pads.iter().filter(|p| p.number == rule.pad.pad).collect();
        if lands.iter().any(|p| p.kind != PadKind::Smd) {
            return fail("via-in-pad permission requires SMD lands");
        }
        if board.metadata.get("fabrication_profile").map(String::as_str) != Some("jlcpcb-six-layer")
            || board.stackup.copper_layers.len() != 6
        {
            return fail("filled-capped via-in-pad requires the JLCPCB six-layer profile");
        }
        if !board
            .zones
            .iter()
            .any(|z| z.net == "GND" && z.layers.iter().any(|l| !matches!(l, CopperLayer::Front | CopperLayer::Back)))
        {
            return fail("via-in-pad permission requires a declared inner GND zone");
        }
        if rule.rows.is_some() {
            copper_pcb::pad_via_arrays::validate_array_fit(board, rule, &lands)?;
        }
    }
    for track in &board.tracks {
        if !net_names.contains(&track.net.as_str()) {
            return fail(format!("track references unknown net {}", python_str_repr(&track.net)));
        }
        if !layers.contains(&track.layer) {
            return fail(format!("track uses unavailable layer {}", python_str_repr(track.layer.value())));
        }
    }
    let technologies: IndexMap<&str, &ViaTechnology> =
        board.stackup.via_technologies.iter().map(|t| (t.id.as_str(), t)).collect();
    for via in &board.vias {
        if !net_names.contains(&via.net.as_str()) {
            return fail(format!("via references unknown net {}", python_str_repr(&via.net)));
        }
        if !layers.contains(&via.from_layer) || !layers.contains(&via.to_layer) {
            return fail("via uses a layer not present in the stackup");
        }
        if !technologies.is_empty() && via.technology.is_none() {
            return fail("vias must select a technology when the stackup defines a catalog");
        }
        if let Some(name) = &via.technology {
            let Some(technology) = technologies.get(name.as_str()) else {
                return fail(format!("via references unknown technology {}", python_str_repr(name)));
            };
            let span: IndexSet<CopperLayer> = [via.from_layer, via.to_layer].into_iter().collect();
            let declared: IndexSet<CopperLayer> = [technology.from_layer, technology.to_layer].into_iter().collect();
            if span != declared {
                return fail("via span does not match its selected technology");
            }
            if via.drill_nm < technology.minimum_drill_nm {
                return fail("via drill is below its technology limit");
            }
            if (via.size_nm - via.drill_nm).div_euclid(2) < technology.minimum_annular_ring_nm {
                return fail("via annular ring is below its technology limit");
            }
            let Some(depth) = crate::routing_vias::via_span_depth(&board.stackup, via.from_layer, via.to_layer) else {
                return not_implemented("a via layer outside the physical stack-up (Python crashes)");
            };
            if crate::routing_vias::exceeds_aspect_ratio(depth, via.drill_nm, &technology.maximum_aspect_ratio) {
                return fail("via aspect ratio exceeds its technology limit");
            }
        }
    }
    let region_names: Vec<&str> = board.regions.iter().map(|r| r.name.as_str()).collect();
    if region_names.iter().collect::<IndexSet<_>>().len() != region_names.len() {
        return not_implemented("duplicate placement region names (Python's later checks)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The router's own types

fn grid_node(value: &StateValue) -> Result<GridNode> {
    dataclass!(reader, value, "pcbir.routing:GridNode", {
        "layer_index" => layer = int;
        "x_index" => x = int;
        "y_index" => y = int;
    });
    reader.required(&[("layer_index", layer.is_none()), ("x_index", x.is_none()), ("y_index", y.is_none())])?;
    Ok(GridNode { layer_index: layer.expect("required"), x_index: x.expect("required"), y_index: y.expect("required") })
}

fn pin_access(value: &StateValue) -> Result<PinAccess> {
    dataclass!(reader, value, "pcbir.routing:PinAccess", {
        "pad" => pad = pad_reference;
        "pad_position" => pad_position = point;
        "node" => node = grid_node;
        "access_position" => access = point;
        "layer" => layer = copper_layer;
        "tracks" => tracks = |v| list(v, track_segment);
        "via" => via_ = |v| optional(v, via);
        "region_only" => region = boolean;
        "retained_via" => retained = |v| optional(v, via);
    });
    reader.required(&[
        ("pad", pad.is_none()),
        ("pad_position", pad_position.is_none()),
        ("node", node.is_none()),
        ("access_position", access.is_none()),
        ("layer", layer.is_none()),
    ])?;
    Ok(PinAccess {
        pad: pad.expect("required"),
        pad_position: pad_position.expect("required"),
        node: node.expect("required"),
        access_position: access.expect("required"),
        layer: layer.expect("required"),
        tracks: tracks.unwrap_or_default(),
        via: via_.flatten(),
        region_only: region.unwrap_or(false),
        retained_via: retained.flatten(),
    })
}

fn route_segment(value: &StateValue) -> Result<GlobalRouteSegment> {
    dataclass!(reader, value, "pcbir.routing:GlobalRouteSegment", {
        "net" => net = text;
        "layer" => layer = copper_layer;
        "start" => start = point;
        "end" => end = point;
        "guide_half_width_nm" => half = int;
        "resource_id" => resource = text;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("layer", layer.is_none()),
        ("start", start.is_none()),
        ("end", end.is_none()),
        ("guide_half_width_nm", half.is_none()),
        ("resource_id", resource.is_none()),
    ])?;
    Ok(GlobalRouteSegment {
        net: net.expect("required"),
        layer: layer.expect("required"),
        start: start.expect("required"),
        end: end.expect("required"),
        guide_half_width_nm: half.expect("required"),
        resource_id: resource.expect("required"),
    })
}

fn via_proposal(value: &StateValue) -> Result<GlobalViaProposal> {
    dataclass!(reader, value, "pcbir.routing:GlobalViaProposal", {
        "net" => net = text;
        "position" => position = point;
        "from_layer" => from = copper_layer;
        "to_layer" => to = copper_layer;
        "resource_id" => resource = text;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("position", position.is_none()),
        ("from_layer", from.is_none()),
        ("to_layer", to.is_none()),
        ("resource_id", resource.is_none()),
    ])?;
    Ok(GlobalViaProposal {
        net: net.expect("required"),
        position: position.expect("required"),
        from_layer: from.expect("required"),
        to_layer: to.expect("required"),
        resource_id: resource.expect("required"),
    })
}

fn net_route(value: &StateValue) -> Result<GlobalNetRoute> {
    dataclass!(reader, value, "pcbir.routing:GlobalNetRoute", {
        "net" => net = text;
        "connected" => connected = boolean;
        "accesses" => accesses = |v| list(v, pin_access);
        "segments" => segments = |v| list(v, route_segment);
        "vias" => vias = |v| list(v, via_proposal);
        "length_nm" => length = int;
        "diagnostics" => diagnostics = |v| list(v, text);
        "deferred_to_zone" => deferred = boolean;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("connected", connected.is_none()),
        ("accesses", accesses.is_none()),
        ("segments", segments.is_none()),
        ("vias", vias.is_none()),
        ("length_nm", length.is_none()),
    ])?;
    Ok(GlobalNetRoute {
        net: net.expect("required"),
        connected: connected.expect("required"),
        accesses: accesses.expect("required"),
        segments: segments.expect("required"),
        vias: vias.expect("required"),
        length_nm: length.expect("required"),
        diagnostics: diagnostics.unwrap_or_default(),
        deferred_to_zone: deferred.unwrap_or(false),
    })
}

fn hotspot(value: &StateValue) -> Result<RoutingHotspot> {
    dataclass!(reader, value, "pcbir.routing:RoutingHotspot", {
        "resource_id" => resource = text;
        "layer" => layer = |v| optional(v, copper_layer);
        "start" => start = point;
        "end" => end = point;
        "capacity" => capacity = int;
        "usage" => usage = int;
        "overflow" => overflow = int;
        "contributors" => contributors = |v| list(v, text);
    });
    reader.required(&[
        ("resource_id", resource.is_none()),
        ("layer", layer.is_none()),
        ("start", start.is_none()),
        ("end", end.is_none()),
        ("capacity", capacity.is_none()),
        ("usage", usage.is_none()),
        ("overflow", overflow.is_none()),
        ("contributors", contributors.is_none()),
    ])?;
    Ok(RoutingHotspot {
        resource_id: resource.expect("required"),
        layer: layer.expect("required"),
        start: start.expect("required"),
        end: end.expect("required"),
        capacity: capacity.expect("required"),
        usage: usage.expect("required"),
        overflow: overflow.expect("required"),
        contributors: contributors.expect("required"),
    })
}

fn routing_metrics(value: &StateValue) -> Result<GlobalRoutingMetrics> {
    dataclass!(reader, value, "pcbir.routing:GlobalRoutingMetrics", {
        "routed_net_count" => routed = int;
        "unrouted_net_count" => unrouted = int;
        "total_overflow" => total_overflow = int;
        "maximum_overflow" => maximum_overflow = int;
        "overfull_resource_count" => overfull = int;
        "maximum_utilization_ppm" => utilization = int;
        "total_length_nm" => length = int;
        "proposed_via_count" => vias = int;
        "iterations" => iterations = int;
        "region_only_access_count" => region_only = int;
        "deferred_net_count" => deferred = int;
    });
    reader.required(&[
        ("routed_net_count", routed.is_none()),
        ("unrouted_net_count", unrouted.is_none()),
        ("total_overflow", total_overflow.is_none()),
        ("maximum_overflow", maximum_overflow.is_none()),
        ("overfull_resource_count", overfull.is_none()),
        ("maximum_utilization_ppm", utilization.is_none()),
        ("total_length_nm", length.is_none()),
        ("proposed_via_count", vias.is_none()),
        ("iterations", iterations.is_none()),
    ])?;
    Ok(GlobalRoutingMetrics {
        routed_net_count: routed.expect("required"),
        unrouted_net_count: unrouted.expect("required"),
        total_overflow: total_overflow.expect("required"),
        maximum_overflow: maximum_overflow.expect("required"),
        overfull_resource_count: overfull.expect("required"),
        maximum_utilization_ppm: utilization.expect("required"),
        total_length_nm: length.expect("required"),
        proposed_via_count: vias.expect("required"),
        iterations: iterations.expect("required"),
        region_only_access_count: region_only.unwrap_or(0),
        deferred_net_count: deferred.unwrap_or(0),
    })
}

pub fn global_routing_result(value: &StateValue) -> Result<GlobalRoutingResult> {
    dataclass!(reader, value, "pcbir.routing:GlobalRoutingResult", {
        "status" => status = |v| enumeration(v, "pcbir.routing:GlobalRoutingStatus", |text| match text {
            "success" => Some(GlobalRoutingStatus::Success),
            "overflow" => Some(GlobalRoutingStatus::Overflow),
            "unreachable" => Some(GlobalRoutingStatus::Unreachable),
            _ => None,
        });
        "routes" => routes = |v| list(v, net_route);
        "hotspots" => hotspots = |v| list(v, hotspot);
        "metrics" => metrics = routing_metrics;
        "placement_fingerprint" => placement = text;
        "routing_fingerprint" => routing = text;
        "fixed_prefix_digest" => prefix = |v| optional(v, text);
    });
    reader.required(&[
        ("status", status.is_none()),
        ("routes", routes.is_none()),
        ("hotspots", hotspots.is_none()),
        ("metrics", metrics.is_none()),
        ("placement_fingerprint", placement.is_none()),
        ("routing_fingerprint", routing.is_none()),
    ])?;
    Ok(GlobalRoutingResult {
        status: status.expect("required"),
        routes: routes.expect("required"),
        hotspots: hotspots.expect("required"),
        metrics: metrics.expect("required"),
        placement_fingerprint: placement.expect("required"),
        routing_fingerprint: routing.expect("required"),
        fixed_prefix_digest: prefix.flatten(),
    })
}

fn bounds(value: &StateValue) -> Result<Bounds> {
    dataclass!(reader, value, "pcbir.geometry:Bounds", {
        "min_x" => min_x = int;
        "min_y" => min_y = int;
        "max_x" => max_x = int;
        "max_y" => max_y = int;
    });
    reader.required(&[
        ("min_x", min_x.is_none()),
        ("min_y", min_y.is_none()),
        ("max_x", max_x.is_none()),
        ("max_y", max_y.is_none()),
    ])?;
    Ok(Bounds {
        min_x: min_x.expect("required"),
        min_y: min_y.expect("required"),
        max_x: max_x.expect("required"),
        max_y: max_y.expect("required"),
    })
}

fn layer_bias(value: &StateValue) -> Result<LayerBias> {
    dataclass!(reader, value, "pcbir.routing_costs:LayerBias", {
        "net" => net = text;
        "layer" => layer = copper_layer;
        "area" => area = bounds;
        "rate" => rate = int;
    });
    reader.required(&[
        ("net", net.is_none()),
        ("layer", layer.is_none()),
        ("area", area.is_none()),
        ("rate", rate.is_none()),
    ])?;
    Ok(LayerBias {
        net: net.expect("required"),
        layer: layer.expect("required"),
        area: area.expect("required"),
        rate: rate.expect("required"),
    })
}

fn fine_region_options(value: &StateValue) -> Result<FineRegionOptions> {
    dataclass!(reader, value, "pcbir.fine_regions:FineRegionOptions", {
        "pad_pitch_threshold_nm" => threshold = int;
        "margin_nm" => margin = int;
        "maximum_factor" => factor = int;
        "merge_gap_nm" => gap = int;
        "minimum_area_nm2" => area = int;
        "minimum_pads" => pads = int;
        "enabled" => enabled = boolean;
        "dense_packages" => dense = boolean;
    });
    let defaults = FineRegionOptions::default();
    FineRegionOptions {
        pad_pitch_threshold_nm: threshold.unwrap_or(defaults.pad_pitch_threshold_nm),
        margin_nm: margin.unwrap_or(defaults.margin_nm),
        maximum_factor: factor.unwrap_or(defaults.maximum_factor),
        merge_gap_nm: gap.unwrap_or(defaults.merge_gap_nm),
        minimum_area_nm2: area.unwrap_or(defaults.minimum_area_nm2),
        minimum_pads: pads.unwrap_or(defaults.minimum_pads),
        enabled: enabled.unwrap_or(true),
        dense_packages: dense.unwrap_or(false),
    }
    .validated()
}

/// A field whose `__post_init__` check requires `type(value) is int`.
fn strict_int(value: &StateValue, message: &str) -> Result<i64> {
    match value {
        StateValue::Int(number) => number.to_i64().ok_or_else(|| Error::not_implemented("an integer beyond 64 bits")),
        _ => fail(message),
    }
}

pub fn unified_router_options(value: &StateValue) -> Result<UnifiedRouterOptions> {
    let defaults = UnifiedRouterOptions::default();
    dataclass!(reader, value, "pcbir.unified_router:UnifiedRouterOptions", {
        "pitch_nm" => pitch_nm = |v| optional(v, int);
        "maximum_iterations" => maximum_iterations = int;
        "stall_iterations" => stall_iterations = int;
        "present_factor" => present_factor = float;
        "present_growth" => present_growth = float;
        "history_increment" => history_increment = float;
        "via_cost_mm" => via_cost_mm = float;
        "wrong_way_cost" => wrong_way_cost = float;
        "outer_layer_cost" => outer_layer_cost = float;
        "attach_radius_nm" => attach_radius_nm = int;
        "neck_radius_nm" => neck_radius_nm = int;
        "attach_candidates" => attach_candidates = int;
        "window_margin_nm" => window_margin_nm = int;
        "present_limit" => present_limit = float;
        "window_growth_limit" => window_growth_limit = int;
        "maximum_expansions" => maximum_expansions = int;
        "commit_rounds" => commit_rounds = int;
        "pour_recheck" => pour_recheck = boolean;
        "buses_first" => buses_first = boolean;
        "bus_length_budget" => bus_length_budget = boolean;
        "bus_skew_tuning" => bus_skew_tuning = boolean;
        "length_priority" => length_priority = boolean;
        "full_reroute_every" => full_reroute_every = int;
        "rejection_marker_cost" => rejection_marker_cost = float;
        "route_smoothing" => route_smoothing = boolean;
        "ball_field_cost" => ball_field_cost = float;
        "reuse_plane_vias" => reuse_plane_vias = boolean;
        "ball_field_via_sites" => ball_field_via_sites = boolean;
        "ball_field_band" => ball_field_band = int;
        "ball_field_exact" => ball_field_exact = boolean;
        "ball_field_exact_reach_nm" => ball_field_exact_reach_nm = |v| optional(v, int);
        "exit_reservation_cost" => exit_reservation_cost = float;
        "bus_bundles" => bus_bundles = boolean;
        "bga_fanout" => bga_fanout = boolean;
        "raster_margin_nm" => raster_margin_nm = |v| optional(v, int);
        "time_budget_s" => time_budget_s = |v| optional(v, float);
        "grid_window_nm" => grid_window_nm = |v| optional(v, |v| {
            let message = "grid_window_nm requires four ordered integer nanometre coordinates";
            let items = list(v, |item| strict_int(item, message))?;
            if items.len() != 4 {
                return fail(message);
            }
            Ok((items[0], items[1], items[2], items[3]))
        });
        "critical_pair_state_limit" => critical_pair_state_limit = |v| optional(v, |v| strict_int(v, "critical pair state limit must be a positive integer or None"));
        "critical_pair_state_escalation" => critical_pair_state_escalation = |v| strict_int(v, "critical pair state escalation must be an integer of at least 1");
        "power_islands" => power_islands = |v| optional(v, text);
        "power_islands_ball_grid_only" => power_islands_ball_grid_only = boolean;
        "plane_fill_width" => plane_fill_width = boolean;
        "implicit_neck_nm" => implicit_neck_nm = int;
        "local_escape_radius_nm" => local_escape_radius_nm = int;
        "local_escape_step_nm" => local_escape_step_nm = int;
        "save_inputs_path" => save_inputs_path = |v| optional(v, text);
        "local_escape_limit" => local_escape_limit = int;
        "pre_route_decouplers" => pre_route_decouplers = boolean;
        "direct" => direct = boolean;
        "escapes_first" => escapes_first = boolean;
        "escape_net_order" => escape_net_order = |v| list(v, text);
        "escape_layer_biases" => escape_layer_biases = |v| list(v, layer_bias);
        "retain_escape_copper" => retain_escape_copper = boolean;
        "planned_escapes_as_copper" => planned_escapes_as_copper = boolean;
        "share_commit_index" => share_commit_index = boolean;
        "failed_retry_every" => failed_retry_every = int;
        "failed_retry_limit" => failed_retry_limit = int;
        "failed_retry_before_exit" => failed_retry_before_exit = boolean;
        "outward_axis_threshold_nm" => outward_axis_threshold_nm = int;
        "attach_extension_steps_nm" => attach_extension_steps_nm = |v| list(v, |item| strict_int(item, "attach_extension_steps_nm requires positive integer nanometre steps"));
        "attach_extension_reach_nm" => attach_extension_reach_nm = int;
        "local_escape_detour_nm" => local_escape_detour_nm = int;
        "local_escape_states" => local_escape_states = int;
        "open_flood_limit" => open_flood_limit = int;
        "open_space_area_nm2" => open_space_area_nm2 = int;
        "plane_piece_area_nm2" => plane_piece_area_nm2 = int;
        "plane_piece_fraction" => plane_piece_fraction = float;
        "repair_neighbour_reach_nm" => repair_neighbour_reach_nm = int;
        "via_budget_factor" => via_budget_factor = float;
        "via_overrun_factor" => via_overrun_factor = float;
        "bend_cost" => bend_cost = float;
        "guide_cost" => guide_cost = float;
        "guide_margin_nm" => guide_margin_nm = int;
        "congestion_tile_nm" => congestion_tile_nm = |v| strict_int(v, "congestion_tile_nm must be a positive integer of at least one pitch");
        "export_congestion" => export_congestion = boolean;
        "fine_regions" => fine_regions = text;
        "fine_region_options" => fine_region_options_ = |v| optional(v, fine_region_options);
    });
    UnifiedRouterOptions {
        pitch_nm: pitch_nm.flatten(),
        maximum_iterations: maximum_iterations.unwrap_or(defaults.maximum_iterations),
        stall_iterations: stall_iterations.unwrap_or(defaults.stall_iterations),
        present_factor: present_factor.unwrap_or(defaults.present_factor),
        present_growth: present_growth.unwrap_or(defaults.present_growth),
        history_increment: history_increment.unwrap_or(defaults.history_increment),
        via_cost_mm: via_cost_mm.unwrap_or(defaults.via_cost_mm),
        wrong_way_cost: wrong_way_cost.unwrap_or(defaults.wrong_way_cost),
        outer_layer_cost: outer_layer_cost.unwrap_or(defaults.outer_layer_cost),
        attach_radius_nm: attach_radius_nm.unwrap_or(defaults.attach_radius_nm),
        neck_radius_nm: neck_radius_nm.unwrap_or(defaults.neck_radius_nm),
        attach_candidates: attach_candidates.unwrap_or(defaults.attach_candidates),
        window_margin_nm: window_margin_nm.unwrap_or(defaults.window_margin_nm),
        present_limit: present_limit.unwrap_or(defaults.present_limit),
        window_growth_limit: window_growth_limit.unwrap_or(defaults.window_growth_limit),
        maximum_expansions: maximum_expansions.unwrap_or(defaults.maximum_expansions),
        commit_rounds: commit_rounds.unwrap_or(defaults.commit_rounds),
        pour_recheck: pour_recheck.unwrap_or(defaults.pour_recheck),
        buses_first: buses_first.unwrap_or(defaults.buses_first),
        bus_length_budget: bus_length_budget.unwrap_or(defaults.bus_length_budget),
        bus_skew_tuning: bus_skew_tuning.unwrap_or(defaults.bus_skew_tuning),
        length_priority: length_priority.unwrap_or(defaults.length_priority),
        full_reroute_every: full_reroute_every.unwrap_or(defaults.full_reroute_every),
        rejection_marker_cost: rejection_marker_cost.unwrap_or(defaults.rejection_marker_cost),
        route_smoothing: route_smoothing.unwrap_or(defaults.route_smoothing),
        ball_field_cost: ball_field_cost.unwrap_or(defaults.ball_field_cost),
        reuse_plane_vias: reuse_plane_vias.unwrap_or(defaults.reuse_plane_vias),
        ball_field_via_sites: ball_field_via_sites.unwrap_or(defaults.ball_field_via_sites),
        ball_field_band: ball_field_band.unwrap_or(defaults.ball_field_band),
        ball_field_exact: ball_field_exact.unwrap_or(defaults.ball_field_exact),
        ball_field_exact_reach_nm: ball_field_exact_reach_nm.flatten(),
        exit_reservation_cost: exit_reservation_cost.unwrap_or(defaults.exit_reservation_cost),
        bus_bundles: bus_bundles.unwrap_or(defaults.bus_bundles),
        bga_fanout: bga_fanout.unwrap_or(defaults.bga_fanout),
        raster_margin_nm: raster_margin_nm.flatten(),
        time_budget_s: time_budget_s.flatten(),
        grid_window_nm: grid_window_nm.flatten(),
        critical_pair_state_limit: critical_pair_state_limit.flatten(),
        critical_pair_state_escalation: critical_pair_state_escalation
            .unwrap_or(defaults.critical_pair_state_escalation),
        power_islands: power_islands.flatten(),
        power_islands_ball_grid_only: power_islands_ball_grid_only.unwrap_or(defaults.power_islands_ball_grid_only),
        plane_fill_width: plane_fill_width.unwrap_or(defaults.plane_fill_width),
        implicit_neck_nm: implicit_neck_nm.unwrap_or(defaults.implicit_neck_nm),
        local_escape_radius_nm: local_escape_radius_nm.unwrap_or(defaults.local_escape_radius_nm),
        local_escape_step_nm: local_escape_step_nm.unwrap_or(defaults.local_escape_step_nm),
        save_inputs_path: save_inputs_path.flatten(),
        local_escape_limit: local_escape_limit.unwrap_or(defaults.local_escape_limit),
        pre_route_decouplers: pre_route_decouplers.unwrap_or(defaults.pre_route_decouplers),
        direct: direct.unwrap_or(defaults.direct),
        escapes_first: escapes_first.unwrap_or(defaults.escapes_first),
        escape_net_order: escape_net_order.unwrap_or_default(),
        escape_layer_biases: escape_layer_biases.unwrap_or_default(),
        retain_escape_copper: retain_escape_copper.unwrap_or(defaults.retain_escape_copper),
        planned_escapes_as_copper: planned_escapes_as_copper.unwrap_or(defaults.planned_escapes_as_copper),
        share_commit_index: share_commit_index.unwrap_or(defaults.share_commit_index),
        failed_retry_every: failed_retry_every.unwrap_or(defaults.failed_retry_every),
        failed_retry_limit: failed_retry_limit.unwrap_or(defaults.failed_retry_limit),
        failed_retry_before_exit: failed_retry_before_exit.unwrap_or(defaults.failed_retry_before_exit),
        outward_axis_threshold_nm: outward_axis_threshold_nm.unwrap_or(defaults.outward_axis_threshold_nm),
        attach_extension_steps_nm: attach_extension_steps_nm.unwrap_or(defaults.attach_extension_steps_nm),
        attach_extension_reach_nm: attach_extension_reach_nm.unwrap_or(defaults.attach_extension_reach_nm),
        local_escape_detour_nm: local_escape_detour_nm.unwrap_or(defaults.local_escape_detour_nm),
        local_escape_states: local_escape_states.unwrap_or(defaults.local_escape_states),
        open_flood_limit: open_flood_limit.unwrap_or(defaults.open_flood_limit),
        open_space_area_nm2: open_space_area_nm2.unwrap_or(defaults.open_space_area_nm2),
        plane_piece_area_nm2: plane_piece_area_nm2.unwrap_or(defaults.plane_piece_area_nm2),
        plane_piece_fraction: plane_piece_fraction.unwrap_or(defaults.plane_piece_fraction),
        repair_neighbour_reach_nm: repair_neighbour_reach_nm.unwrap_or(defaults.repair_neighbour_reach_nm),
        via_budget_factor: via_budget_factor.unwrap_or(defaults.via_budget_factor),
        via_overrun_factor: via_overrun_factor.unwrap_or(defaults.via_overrun_factor),
        bend_cost: bend_cost.unwrap_or(defaults.bend_cost),
        guide_cost: guide_cost.unwrap_or(defaults.guide_cost),
        guide_margin_nm: guide_margin_nm.unwrap_or(defaults.guide_margin_nm),
        congestion_tile_nm: congestion_tile_nm.unwrap_or(defaults.congestion_tile_nm),
        export_congestion: export_congestion.unwrap_or(defaults.export_congestion),
        fine_regions: fine_regions.unwrap_or(defaults.fine_regions),
        fine_region_options: fine_region_options_.flatten(),
    }
    .validated()
}

fn routing_access(value: &StateValue) -> Result<RoutingAccess> {
    dataclass!(reader, value, "pcbir.pin_escape:RoutingAccess", {
        "position" => position = point;
        "layer" => layer = copper_layer;
        "launch_position" => launch = point;
        "path" => path = |v| list(v, track_segment);
    });
    reader.required(&[
        ("position", position.is_none()),
        ("layer", layer.is_none()),
        ("launch_position", launch.is_none()),
        ("path", path.is_none()),
    ])?;
    RoutingAccess::new(
        position.expect("required"),
        layer.expect("required"),
        launch.expect("required"),
        path.expect("required"),
    )
    .map_err(Error::from)
}

/// A `dict[PadReference, RoutingAccess]`.
fn accesses(value: &StateValue) -> Result<Vec<(PadReference, RoutingAccess)>> {
    match value {
        StateValue::Map { entries, .. } => {
            entries.iter().map(|(key, item)| Ok((pad_reference(key)?, routing_access(item)?))).collect()
        }
        other => dynamic("an access mapping", other),
    }
}

/// The saved dict: every key the router reads, in the file's order.
fn saved_inputs(value: &StateValue) -> Result<SavedInputs> {
    let StateValue::Map { entries, .. } = value else { return dynamic("the saved inputs dict", value) };
    let mut map: IndexMap<String, &StateValue> = IndexMap::new();
    for (key, item) in entries {
        map.insert(text(key)?, item);
    }
    let lookup = |key: &str| -> Result<&StateValue> {
        map.get(key).copied().ok_or_else(|| Error::Crash("KeyError".to_owned(), python_str_repr(key)))
    };
    let mut board = None;
    let mut global_route = None;
    let mut options = None;
    let mut fanout_accesses = None;
    let mut fanout_created_vias = None;
    let mut fanout_created_tracks = None;
    let mut only_nets = None;
    let mut required_accesses = None;
    // Python decodes the whole dict before replay reads it, so every value
    // is checked in file order.
    for (key, item) in &map {
        match key.as_str() {
            "board" => board = Some(physical_board(item)?),
            "global_route" => global_route = Some(global_routing_result(item)?),
            "options" => options = Some(unified_router_options(item)?),
            "fanout_accesses" => fanout_accesses = Some(optional(item, accesses)?),
            "fanout_created_vias" => {
                fanout_created_vias = Some(optional(item, |v| {
                    set(v, |item| match item {
                        StateValue::Tuple(parts) | StateValue::List(parts) if parts.len() == 2 => {
                            Ok((text(&parts[0])?, point(&parts[1])?))
                        }
                        other => dynamic("a (net, point) pair", other),
                    })
                })?)
            }
            "fanout_created_tracks" => fanout_created_tracks = Some(optional(item, |v| list(v, track_segment))?),
            "only_nets" => only_nets = Some(optional(item, |v| set(v, text).map(|names| names.into_iter().collect()))?),
            "required_accesses" => required_accesses = Some(optional(item, accesses)?),
            _ => {}
        }
    }
    lookup("options")?;
    lookup("board")?;
    lookup("global_route")?;
    lookup("fanout_accesses")?;
    lookup("fanout_created_vias")?;
    lookup("fanout_created_tracks")?;
    lookup("only_nets")?;
    Ok(SavedInputs {
        board: board.expect("looked up"),
        global_route: global_route.expect("looked up"),
        options: options.expect("looked up"),
        fanout_accesses: fanout_accesses.expect("looked up"),
        fanout_created_vias: fanout_created_vias.expect("looked up"),
        fanout_created_tracks: fanout_created_tracks.expect("looked up"),
        only_nets: only_nets.expect("looked up"),
        required_accesses: required_accesses.flatten(),
    })
}

// ---------------------------------------------------------------------------
// Writing the result

fn dc(name: &str, fields: Vec<(&str, StateValue)>) -> StateValue {
    StateValue::Dataclass {
        name: name.to_owned(),
        fields: fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
    }
}

fn en(name: &str, value: &str) -> StateValue {
    StateValue::Enum { name: name.to_owned(), value: Box::new(StateValue::Str(value.to_owned())) }
}

fn s(text: &str) -> StateValue {
    StateValue::Str(text.to_owned())
}

fn i(value: i64) -> StateValue {
    StateValue::Int(value.into())
}

fn b(flag: bool) -> StateValue {
    StateValue::Bool(flag)
}

fn opt<T>(value: &Option<T>, encode: impl Fn(&T) -> StateValue) -> StateValue {
    value.as_ref().map_or(StateValue::Null, encode)
}

fn opt_int(value: Option<i64>) -> StateValue {
    value.map_or(StateValue::Null, i)
}

fn opt_text(value: &Option<String>) -> StateValue {
    value.as_deref().map_or(StateValue::Null, s)
}

fn tuple<T>(items: &[T], encode: impl Fn(&T) -> StateValue) -> StateValue {
    StateValue::Tuple(items.iter().map(encode).collect())
}

fn dec(value: &Decimal) -> StateValue {
    StateValue::Decimal(value.clone())
}

fn e_layer(layer: &CopperLayer) -> StateValue {
    en("pcbir.physical:CopperLayer", layer.value())
}

fn e_side(side: &BoardSide) -> StateValue {
    en("pcbir.physical:BoardSide", side.value())
}

fn e_point(point: &Point) -> StateValue {
    dc("pcbir.physical:Point", vec![("x_nm", i(point.x_nm)), ("y_nm", i(point.y_nm))])
}

fn e_size(size: &Size) -> StateValue {
    dc("pcbir.physical:Size", vec![("width_nm", i(size.width_nm)), ("height_nm", i(size.height_nm))])
}

fn e_pad_reference(pad: &PadReference) -> StateValue {
    dc("pcbir.physical:PadReference", vec![("component", s(&pad.component)), ("pad", s(&pad.pad))])
}

fn e_ring(vertices: &[Point]) -> StateValue {
    dc("pcbir.physical:PolygonRing", vec![("vertices", tuple(vertices, e_point))])
}

fn e_polygon(outer: &[Point], holes: &[Vec<Point>]) -> StateValue {
    dc("pcbir.physical:PolygonWithHoles", vec![("outer", e_ring(outer)), ("holes", tuple(holes, |h| e_ring(h)))])
}

fn e_outline(outline: &BoardOutline) -> StateValue {
    dc(
        "pcbir.physical:BoardOutline",
        vec![
            ("vertices", tuple(&outline.vertices, e_point)),
            (
                "cutouts",
                tuple(&outline.cutouts, |c| {
                    dc("pcbir.physical:BoardCutout", vec![("id", s(&c.id)), ("vertices", tuple(&c.vertices, e_point))])
                }),
            ),
            (
                "circular_boundary",
                opt(&outline.circular_boundary, |c| {
                    dc(
                        "pcbir.physical:CircularBoardBoundary",
                        vec![
                            ("center", e_point(&c.center)),
                            ("radius_nm", i(c.radius_nm)),
                            ("maximum_chord_error_nm", i(c.maximum_chord_error_nm)),
                        ],
                    )
                }),
            ),
            (
                "boundary_path",
                opt(&outline.boundary_path, |path| {
                    dc(
                        "pcbir.physical:BoardBoundaryPath",
                        vec![
                            (
                                "segments",
                                tuple(&path.segments, |segment| match segment {
                                    BoundarySegment::Line { id, start, end } => dc(
                                        "pcbir.physical:BoundaryLine",
                                        vec![("id", s(id)), ("start", e_point(start)), ("end", e_point(end))],
                                    ),
                                    BoundarySegment::Arc { id, start, mid, end } => dc(
                                        "pcbir.physical:BoundaryArc",
                                        vec![
                                            ("id", s(id)),
                                            ("start", e_point(start)),
                                            ("mid", e_point(mid)),
                                            ("end", e_point(end)),
                                        ],
                                    ),
                                }),
                            ),
                            ("maximum_chord_error_nm", i(path.maximum_chord_error_nm)),
                        ],
                    )
                }),
            ),
        ],
    )
}

fn e_keepout(keepout: &CopperKeepout) -> StateValue {
    dc(
        "pcbir.physical:CopperKeepout",
        vec![
            ("id", s(&keepout.id)),
            ("layers", tuple(&keepout.layers, e_layer)),
            ("outline", e_polygon(&keepout.vertices, &[])),
            ("block_tracks", b(keepout.block_tracks)),
            ("block_vias", b(keepout.block_vias)),
            ("block_pads", b(keepout.block_pads)),
            ("block_zones", b(keepout.block_zones)),
            ("block_footprints", b(keepout.block_footprints)),
        ],
    )
}

fn e_footprint_layer(layer: FootprintLayer) -> StateValue {
    en("pcbir.physical:FootprintLayer", layer.value())
}

fn e_graphic(graphic: &FootprintGraphic) -> StateValue {
    match graphic {
        FootprintGraphic::Line { start, end, width_nm, layer } => dc(
            "pcbir.physical:FootprintLine",
            vec![
                ("start", e_point(start)),
                ("end", e_point(end)),
                ("width_nm", i(*width_nm)),
                ("layer", e_footprint_layer(*layer)),
            ],
        ),
        FootprintGraphic::Rectangle { start, end, width_nm, layer, filled } => dc(
            "pcbir.physical:FootprintRectangle",
            vec![
                ("start", e_point(start)),
                ("end", e_point(end)),
                ("width_nm", i(*width_nm)),
                ("layer", e_footprint_layer(*layer)),
                ("filled", b(*filled)),
            ],
        ),
        FootprintGraphic::Circle { center, end, width_nm, layer, filled } => dc(
            "pcbir.physical:FootprintCircle",
            vec![
                ("center", e_point(center)),
                ("end", e_point(end)),
                ("width_nm", i(*width_nm)),
                ("layer", e_footprint_layer(*layer)),
                ("filled", b(*filled)),
            ],
        ),
        FootprintGraphic::Arc { start, midpoint, end, width_nm, layer } => dc(
            "pcbir.physical:FootprintArc",
            vec![
                ("start", e_point(start)),
                ("midpoint", e_point(midpoint)),
                ("end", e_point(end)),
                ("width_nm", i(*width_nm)),
                ("layer", e_footprint_layer(*layer)),
            ],
        ),
        FootprintGraphic::Polygon { points, width_nm, layer, filled } => dc(
            "pcbir.physical:FootprintPolygon",
            vec![
                ("points", tuple(points, e_point)),
                ("width_nm", i(*width_nm)),
                ("layer", e_footprint_layer(*layer)),
                ("filled", b(*filled)),
            ],
        ),
    }
}

fn e_text_map(map: &IndexMap<String, String>, proxy: bool) -> StateValue {
    StateValue::Map { entries: map.iter().map(|(k, v)| (s(k), s(v))).collect(), proxy }
}

fn e_footprint(footprint: &PhysicalFootprint) -> StateValue {
    dc(
        "pcbir.physical:PhysicalFootprint",
        vec![
            ("name", s(&footprint.name)),
            (
                "pads",
                tuple(&footprint.pads, |pad| {
                    dc(
                        "pcbir.physical:FootprintPad",
                        vec![
                            ("number", s(&pad.number)),
                            ("position", e_point(&pad.position)),
                            ("size", e_size(&pad.size)),
                            ("kind", en("pcbir.physical:PadKind", pad.kind.value())),
                            ("shape", en("pcbir.physical:PadShape", pad.shape.value())),
                            ("rotation_degrees", dec(&pad.rotation_degrees)),
                            ("drill", opt(&pad.drill, e_size)),
                            ("roundrect_ratio_ppm", i(pad.roundrect_ratio_ppm)),
                            ("has_solder_mask", b(pad.has_solder_mask)),
                            ("has_solder_paste", b(pad.has_solder_paste)),
                            (
                                "zone_connection",
                                opt(&pad.zone_connection, |c| en("pcbir.physical:ZoneConnection", c.value())),
                            ),
                            ("heatsink", b(pad.heatsink)),
                            ("remove_unused_layers", b(pad.remove_unused_layers)),
                            ("connector_contact", b(pad.connector_contact)),
                        ],
                    )
                }),
            ),
            ("body_size", e_size(&footprint.body_size)),
            ("source_library_id", opt_text(&footprint.source_library_id)),
            ("graphics", tuple(&footprint.graphics, e_graphic)),
            ("keepouts", tuple(&footprint.keepouts, e_keepout)),
            ("metadata", e_text_map(&footprint.metadata, true)),
            ("courtyard", tuple(&footprint.courtyard, e_point)),
            ("height_nm", opt_int(footprint.height_nm)),
            ("clearance_nm", opt_int(footprint.clearance_nm)),
            ("exclude_from_bom", b(footprint.exclude_from_bom)),
            ("exclude_from_pos_files", b(footprint.exclude_from_pos_files)),
            (
                "internal_pad_groups",
                tuple(&footprint.internal_pad_groups, |group| {
                    dc("pcbir.pad_connections:InternalPadGroup", vec![("numbers", tuple(&group.numbers, |n| s(n)))])
                }),
            ),
        ],
    )
}

fn e_track(track: &TrackSegment) -> StateValue {
    dc(
        "pcbir.physical:TrackSegment",
        vec![
            ("net", s(&track.net)),
            ("start", e_point(&track.start)),
            ("end", e_point(&track.end)),
            ("width_nm", i(track.width_nm)),
            ("layer", e_layer(&track.layer)),
        ],
    )
}

fn e_via(via: &Via) -> StateValue {
    dc(
        "pcbir.physical:Via",
        vec![
            ("net", s(&via.net)),
            ("position", e_point(&via.position)),
            ("size_nm", i(via.size_nm)),
            ("drill_nm", i(via.drill_nm)),
            ("from_layer", e_layer(&via.from_layer)),
            ("to_layer", e_layer(&via.to_layer)),
            ("technology", opt_text(&via.technology)),
            ("finish", s(&via.finish)),
        ],
    )
}

fn e_zone(zone: &CopperZone) -> StateValue {
    dc(
        "pcbir.physical:CopperZone",
        vec![
            ("id", s(&zone.id)),
            ("net", s(&zone.net)),
            ("layers", tuple(&zone.layers, e_layer)),
            ("outline", e_polygon(&zone.outer, &zone.holes)),
            ("priority", i(zone.priority)),
            ("clearance_nm", opt_int(zone.clearance_nm)),
            ("minimum_width_nm", i(zone.minimum_width_nm)),
            ("pad_connection", en("pcbir.physical:ZoneConnection", zone.pad_connection.value())),
            (
                "thermal",
                dc(
                    "pcbir.physical:ThermalReliefSettings",
                    vec![
                        ("gap_nm", i(zone.thermal_gap_nm)),
                        ("spoke_width_nm", i(zone.thermal_spoke_width_nm)),
                        ("spoke_count", i(4)),
                    ],
                ),
            ),
            ("island_policy", en("pcbir.physical:IslandPolicy", zone.island_policy.value())),
            ("minimum_island_area_nm2", opt_int(zone.minimum_island_area_nm2)),
            ("fill_mode", en("pcbir.physical:ZoneFillMode", "solid")),
            ("reserve_routing", b(zone.reserve_routing)),
            ("allow_same_net_hard_macro_overlap", b(zone.allow_same_net_hard_macro_overlap)),
        ],
    )
}

fn e_polygon_copper(polygon: &CopperPolygon) -> StateValue {
    dc(
        "pcbir.physical:CopperPolygon",
        vec![
            ("id", s(&polygon.id)),
            ("net", s(&polygon.net)),
            ("layer", e_layer(&polygon.layer)),
            ("outline", e_ring(&polygon.vertices)),
        ],
    )
}

fn e_target(target: &PlacementTarget) -> StateValue {
    dc("pcbir.physical:PlacementTarget", vec![("reference", s(&target.reference)), ("pad", opt_text(&target.pad))])
}

fn e_rule(rule: &NetRoutingRule) -> StateValue {
    dc(
        "pcbir.physical:NetRoutingRule",
        vec![
            ("net", s(&rule.net)),
            ("kind", en("pcbir.physical:RouteKind", rule.kind.value())),
            ("priority", i(rule.priority)),
            ("width_nm", opt_int(rule.width_nm)),
            ("clearance_nm", opt_int(rule.clearance_nm)),
            ("allowed_layers", tuple(&rule.allowed_layers, e_layer)),
            ("max_vias", opt_int(rule.max_vias)),
            ("max_length_nm", opt_int(rule.max_length_nm)),
            ("differential_partner", opt_text(&rule.differential_partner)),
            ("pair_gap_nm", opt_int(rule.pair_gap_nm)),
            ("max_skew_nm", opt_int(rule.max_skew_nm)),
            ("topology", s(&rule.topology)),
            ("target_impedance_ohms", opt_int(rule.target_impedance_ohms)),
            ("maximum_uncoupled_length_nm", opt_int(rule.maximum_uncoupled_length_nm)),
            ("maximum_stub_length_nm", opt_int(rule.maximum_stub_length_nm)),
            ("tuning_amplitude_limit_nm", opt_int(rule.tuning_amplitude_limit_nm)),
            ("require_return_vias", b(rule.require_return_vias)),
            ("reserve_corridor", b(rule.reserve_corridor)),
            ("return_via_net", opt_text(&rule.return_via_net)),
            ("maximum_return_via_distance_nm", opt_int(rule.maximum_return_via_distance_nm)),
            ("impedance_evidence_digest", opt_text(&rule.impedance_evidence_digest)),
            ("return_via_policy", en("pcbir.physical:ReturnViaPolicy", rule.return_via_policy.value())),
            ("shared_reference_layer", opt(&rule.shared_reference_layer, e_layer)),
            ("target_single_ended_ohms", opt_int(rule.target_single_ended_ohms)),
            ("impedance_tolerance_percent", opt(&rule.impedance_tolerance_percent, dec)),
            ("layer_group", opt_text(&rule.layer_group)),
            ("breakout_length_nm", opt_int(rule.breakout_length_nm)),
            ("breakout_width_nm", opt_int(rule.breakout_width_nm)),
            ("breakout_gap_nm", opt_int(rule.breakout_gap_nm)),
            ("breakout_clearance_nm", opt_int(rule.breakout_clearance_nm)),
            ("tuning_style", en("pcbir.physical:TuningStyle", rule.tuning_style.value())),
            ("tuning_spacing_nm", opt_int(rule.tuning_spacing_nm)),
            ("tuning_group", opt_text(&rule.tuning_group)),
        ],
    )
}

fn e_artwork(artwork: &Artwork) -> StateValue {
    let material = |m: &ArtworkMaterial| en("pcbir.silkscreen:ArtworkMaterial", m.value());
    match artwork {
        Artwork::Text { id, text, position, size_nm, thickness_nm, rotation_degrees, side, material: m, exposed } => {
            dc(
                "pcbir.silkscreen:SilkscreenText",
                vec![
                    ("id", s(id)),
                    ("text", s(text)),
                    ("position", e_point(position)),
                    ("size_nm", i(*size_nm)),
                    ("thickness_nm", i(*thickness_nm)),
                    ("rotation_degrees", dec(rotation_degrees)),
                    ("side", e_side(side)),
                    ("material", material(m)),
                    ("exposed", b(*exposed)),
                ],
            )
        }
        Artwork::Graphic { id, graphic, side, material: m, exposed } => {
            let layer = FootprintLayer::Silkscreen;
            let graphic = match graphic {
                Graphic::Line { start, end, width_nm } => {
                    FootprintGraphic::Line { start: *start, end: *end, width_nm: *width_nm, layer }
                }
                Graphic::Rectangle { start, end, width_nm, filled } => FootprintGraphic::Rectangle {
                    start: *start,
                    end: *end,
                    width_nm: *width_nm,
                    layer,
                    filled: *filled,
                },
                Graphic::Circle { center, end, width_nm, filled } => {
                    FootprintGraphic::Circle { center: *center, end: *end, width_nm: *width_nm, layer, filled: *filled }
                }
                Graphic::Arc { start, midpoint, end, width_nm } => {
                    FootprintGraphic::Arc { start: *start, midpoint: *midpoint, end: *end, width_nm: *width_nm, layer }
                }
                Graphic::Polygon { points, width_nm, filled } => {
                    FootprintGraphic::Polygon { points: points.clone(), width_nm: *width_nm, layer, filled: *filled }
                }
            };
            dc(
                "pcbir.silkscreen:SilkscreenGraphic",
                vec![
                    ("id", s(id)),
                    ("graphic", e_graphic(&graphic)),
                    ("side", e_side(side)),
                    ("material", material(m)),
                    ("exposed", b(*exposed)),
                ],
            )
        }
    }
}

fn e_mechanical_reference(reference: &copper_lang::mechanical::MechanicalReference) -> StateValue {
    dc(
        "pcbir.mechanical_references:MechanicalReference",
        vec![
            ("id", s(&reference.id)),
            ("file", s(&reference.file)),
            ("sha256", s(&reference.sha256)),
            ("units", s(&reference.units)),
            ("frame", s(&reference.frame)),
            ("position", e_point(&reference.position)),
            ("rotation_degrees", dec(&reference.rotation_degrees)),
            ("mirror_x", b(reference.mirror_x)),
            ("side", s(&reference.side)),
            ("purpose", s(&reference.purpose)),
            (
                "entities",
                tuple(&reference.entities, |entity| {
                    dc(
                        "pcbir.mechanical_references:ReferenceEntity",
                        vec![
                            ("kind", s(entity.kind)),
                            ("points", tuple(&entity.points, e_point)),
                            ("radius_nm", i(entity.radius_nm)),
                            ("start_degrees", dec(&entity.start_degrees)),
                            ("sweep_degrees", dec(&entity.sweep_degrees)),
                            ("closed", b(entity.closed)),
                        ],
                    )
                }),
            ),
            ("asset_path", s(&reference.asset_path)),
        ],
    )
}

/// `encode(board)`: the `PhysicalBoard` dataclass.
pub fn encode_board(board: &PhysicalBoard) -> StateValue {
    dc(
        "pcbir.physical:PhysicalBoard",
        vec![
            ("name", s(&board.name)),
            ("outline", e_outline(&board.outline)),
            (
                "footprints",
                StateValue::Map {
                    entries: board.footprints.iter().map(|(k, v)| (s(k), e_footprint(v))).collect(),
                    proxy: true,
                },
            ),
            (
                "placements",
                tuple(&board.placements, |p| {
                    dc(
                        "pcbir.physical:Placement",
                        vec![
                            ("reference", s(&p.reference)),
                            ("footprint", s(&p.footprint)),
                            ("position", e_point(&p.position)),
                            ("rotation_degrees", dec(&p.rotation_degrees)),
                            ("side", e_side(&p.side)),
                            ("value", s(&p.value)),
                            ("source_path", opt_text(&p.source_path)),
                        ],
                    )
                }),
            ),
            (
                "nets",
                tuple(&board.nets, |n| {
                    dc(
                        "pcbir.physical:PhysicalNet",
                        vec![("name", s(&n.name)), ("pads", tuple(&n.pads, e_pad_reference))],
                    )
                }),
            ),
            (
                "stackup",
                dc(
                    "pcbir.physical:Stackup",
                    vec![
                        ("copper_layers", tuple(&board.stackup.copper_layers, e_layer)),
                        ("thickness_nm", i(board.stackup.thickness_nm)),
                        (
                            "physical_layers",
                            tuple(&board.stackup.physical_layers, |layer| {
                                dc(
                                    "pcbir.physical:StackupLayer",
                                    vec![
                                        ("id", s(&layer.id)),
                                        (
                                            "kind",
                                            en(
                                                "pcbir.physical:StackupLayerKind",
                                                if layer.copper_layer.is_some() { "copper" } else { "dielectric" },
                                            ),
                                        ),
                                        ("thickness_nm", i(layer.thickness_nm)),
                                        ("copper_layer", opt(&layer.copper_layer, e_layer)),
                                        ("material", opt_text(&layer.material)),
                                        ("relative_permittivity", opt(&layer.relative_permittivity, dec)),
                                        ("loss_tangent", opt(&layer.loss_tangent, dec)),
                                        ("dielectric_type", opt_text(&layer.dielectric_type)),
                                    ],
                                )
                            }),
                        ),
                        (
                            "via_technologies",
                            tuple(&board.stackup.via_technologies, |t| {
                                dc(
                                    "pcbir.physical:ViaTechnology",
                                    vec![
                                        ("id", s(&t.id)),
                                        ("kind", en("pcbir.physical:ViaKind", t.kind.value())),
                                        ("from_layer", e_layer(&t.from_layer)),
                                        ("to_layer", e_layer(&t.to_layer)),
                                        ("minimum_drill_nm", i(t.minimum_drill_nm)),
                                        ("minimum_annular_ring_nm", i(t.minimum_annular_ring_nm)),
                                        ("maximum_aspect_ratio", dec(&t.maximum_aspect_ratio)),
                                    ],
                                )
                            }),
                        ),
                    ],
                ),
            ),
            (
                "rules",
                dc(
                    "pcbir.physical:DesignRules",
                    vec![
                        ("minimum_clearance_nm", i(board.rules.minimum_clearance_nm)),
                        ("minimum_hole_clearance_nm", i(board.rules.minimum_hole_clearance_nm)),
                        ("minimum_track_width_nm", i(board.rules.minimum_track_width_nm)),
                        ("default_track_width_nm", i(board.rules.default_track_width_nm)),
                        ("default_via_size_nm", i(board.rules.default_via_size_nm)),
                        ("default_via_drill_nm", i(board.rules.default_via_drill_nm)),
                        ("minimum_slot_width_nm", i(board.rules.minimum_slot_width_nm)),
                        ("minimum_via_size_nm", opt_int(board.rules.minimum_via_size_nm)),
                        ("minimum_via_drill_nm", opt_int(board.rules.minimum_via_drill_nm)),
                    ],
                ),
            ),
            ("tracks", tuple(&board.tracks, e_track)),
            ("vias", tuple(&board.vias, e_via)),
            ("metadata", e_text_map(&board.metadata, true)),
            (
                "regions",
                tuple(&board.regions, |r| {
                    dc(
                        "pcbir.physical:PlacementRegion",
                        vec![("name", s(&r.name)), ("outline", e_outline(&r.outline)), ("side", opt(&r.side, e_side))],
                    )
                }),
            ),
            (
                "keepouts",
                tuple(&board.keepouts, |k| {
                    dc(
                        "pcbir.physical:PlacementKeepout",
                        vec![
                            ("name", s(&k.name)),
                            ("outline", e_outline(&k.outline)),
                            ("side", opt(&k.side, e_side)),
                            ("maximum_component_height_nm", opt_int(k.maximum_component_height_nm)),
                        ],
                    )
                }),
            ),
            (
                "placement_rules",
                tuple(&board.placement_rules, |r| {
                    dc(
                        "pcbir.physical:ComponentPlacementRule",
                        vec![
                            ("reference", s(&r.reference)),
                            ("region", opt_text(&r.region)),
                            ("allowed_orientations", tuple(&r.allowed_orientations, dec)),
                            ("fixed_position", opt(&r.fixed_position, e_point)),
                            ("fixed_rotation_degrees", opt(&r.fixed_rotation_degrees, dec)),
                            ("side", opt(&r.side, e_side)),
                            ("priority", i(r.priority)),
                            ("edge_clearance_nm", opt_int(r.edge_clearance_nm)),
                            ("additional_courtyard_gap_nm", opt_int(r.additional_courtyard_gap_nm)),
                        ],
                    )
                }),
            ),
            (
                "relative_rules",
                tuple(&board.relative_rules, |r| {
                    dc(
                        "pcbir.physical:RelativePlacementRule",
                        vec![
                            ("kind", en("pcbir.physical:RelativePlacementKind", r.kind.value())),
                            ("targets", tuple(&r.targets, e_target)),
                            ("distance_nm", opt_int(r.distance_nm)),
                            ("axis", opt(&r.axis, |a| en("pcbir.physical:AlignmentAxis", a.value()))),
                            ("tolerance_nm", i(r.tolerance_nm)),
                            ("weight", i(r.weight)),
                        ],
                    )
                }),
            ),
            (
                "placement_groups",
                tuple(&board.placement_groups, |g| {
                    dc(
                        "pcbir.physical:PlacementGroup",
                        vec![
                            ("name", s(&g.name)),
                            ("references", tuple(&g.references, |r| s(r))),
                            ("anchor", opt_text(&g.anchor)),
                            ("priority", i(g.priority)),
                            ("source", s(&g.source)),
                        ],
                    )
                }),
            ),
            (
                "power_domains",
                tuple(&board.power_domains, |d| {
                    dc(
                        "pcbir.physical:PhysicalPowerDomain",
                        vec![
                            ("net", s(&d.net)),
                            ("members", tuple(&d.members, e_pad_reference)),
                            ("sources", tuple(&d.sources, e_pad_reference)),
                            ("source_ports", tuple(&d.source_ports, |p| s(p))),
                        ],
                    )
                }),
            ),
            ("net_routing_rules", tuple(&board.net_routing_rules, e_rule)),
            ("zones", tuple(&board.zones, e_zone)),
            ("polygons", tuple(&board.polygons, e_polygon_copper)),
            ("copper_keepouts", tuple(&board.copper_keepouts, e_keepout)),
            ("zone_fills", StateValue::Tuple(Vec::new())),
            (
                "rigid_clusters",
                tuple(&board.rigid_clusters, |c| {
                    dc(
                        "pcbir.physical:RigidPlacementCluster",
                        vec![
                            ("name", s(&c.name)),
                            (
                                "anchor",
                                e_target(&PlacementTarget {
                                    reference: c.anchor_reference.clone(),
                                    pad: c.anchor_pad.clone(),
                                }),
                            ),
                            (
                                "members",
                                tuple(&c.members, |m| {
                                    dc(
                                        "pcbir.physical:RigidPlacementMember",
                                        vec![
                                            ("reference", s(&m.reference)),
                                            ("footprint", s(&m.footprint)),
                                            ("footprint_digest", s(&m.footprint_digest)),
                                            ("position", e_point(&m.position)),
                                            ("rotation_degrees", dec(&m.rotation_degrees)),
                                        ],
                                    )
                                }),
                            ),
                            ("source", s(&c.source)),
                            ("allowed_rotations", tuple(&c.allowed_rotations, dec)),
                            ("keepouts", tuple(&c.keepouts, e_keepout)),
                            ("internal_clearance_nm", opt_int(c.internal_clearance_nm)),
                            (
                                "edge_openings",
                                tuple(&c.edge_openings, |o| {
                                    dc(
                                        "pcbir.physical:MacroEdgeOpening",
                                        vec![
                                            ("keepout", s(&o.keepout)),
                                            ("edge_indices", tuple(&o.edge_indices, |x| i(*x))),
                                        ],
                                    )
                                }),
                            ),
                        ],
                    )
                }),
            ),
            (
                "hard_macros",
                tuple(&board.hard_macros, |m| {
                    dc(
                        "pcbir.physical:PhysicalHardMacro",
                        vec![
                            ("cluster", s(&m.cluster)),
                            ("asset_sha256", s(&m.asset_sha256)),
                            ("tracks", tuple(&m.tracks, e_track)),
                            ("vias", tuple(&m.vias, e_via)),
                            (
                                "ports",
                                tuple(&m.ports, |p| {
                                    dc(
                                        "pcbir.physical:MacroPort",
                                        vec![
                                            ("name", s(&p.name)),
                                            ("net", s(&p.net)),
                                            ("position", e_point(&p.position)),
                                            ("layer", e_layer(&p.layer)),
                                            ("pads", tuple(&p.pads, e_pad_reference)),
                                        ],
                                    )
                                }),
                            ),
                            ("protected_regions", tuple(&m.protected_regions, e_keepout)),
                            ("required_layers", tuple(&m.required_layers, e_layer)),
                            (
                                "pad_bindings",
                                tuple(&m.pad_bindings, |(pad, net)| {
                                    dc(
                                        "pcbir.physical:MacroPadBinding",
                                        vec![("pad", e_pad_reference(pad)), ("net", s(net))],
                                    )
                                }),
                            ),
                            ("isolated_pads", tuple(&m.isolated_pads, e_pad_reference)),
                            ("zones", tuple(&m.zones, e_zone)),
                            (
                                "plane_returns",
                                tuple(&m.plane_returns, |r| {
                                    dc(
                                        "pcbir.physical:MacroPlaneReturn",
                                        vec![
                                            ("net", s(&r.net)),
                                            ("layers", tuple(&r.layers, e_layer)),
                                            ("pads", tuple(&r.pads, e_pad_reference)),
                                            (
                                                "dedicated_contacts",
                                                tuple(&r.dedicated_contacts, |(pad, point)| {
                                                    StateValue::Tuple(vec![e_pad_reference(pad), e_point(point)])
                                                }),
                                            ),
                                        ],
                                    )
                                }),
                            ),
                            ("polygons", tuple(&m.polygons, e_polygon_copper)),
                            (
                                "width_contracts",
                                tuple(&m.width_contracts, |c| {
                                    dc(
                                        "pcbir.physical:MacroTrackWidthContract",
                                        vec![
                                            ("track_index", i(c.track_index as i64)),
                                            ("minimum_width_nm", i(c.minimum_width_nm)),
                                            ("purpose", s(&c.purpose)),
                                            ("evidence", s(&c.evidence)),
                                        ],
                                    )
                                }),
                            ),
                        ],
                    )
                }),
            ),
            ("materialized_macros", tuple(&board.materialized_macros, |m| s(m))),
            (
                "via_in_pad_rules",
                tuple(&board.via_in_pad_rules, |r| {
                    dc(
                        "pcbir.physical:PadViaInPadRule",
                        vec![
                            ("pad", e_pad_reference(&r.pad)),
                            ("process", s("filled-capped")),
                            ("rows", opt_int(r.rows)),
                            ("columns", opt_int(r.columns)),
                            ("pitch_nm", opt_int(r.pitch_nm)),
                        ],
                    )
                }),
            ),
            (
                "mechanical_holes",
                tuple(&board.mechanical_holes, |h| {
                    dc(
                        "pcbir.physical:MechanicalHole",
                        vec![
                            ("id", s(&h.id)),
                            ("position", e_point(&h.position)),
                            ("diameter_nm", i(h.diameter_nm)),
                            ("head_clearance_radius_nm", i(h.head_clearance_radius_nm)),
                        ],
                    )
                }),
            ),
            (
                "datums",
                tuple(&board.datums, |d| {
                    dc(
                        "pcbir.physical:BoardDatum",
                        vec![
                            ("id", s(&d.id)),
                            ("position", e_point(&d.position)),
                            ("relative_to", opt_text(&d.relative_to)),
                            ("offset", e_point(&d.offset)),
                        ],
                    )
                }),
            ),
            (
                "boundary_edges",
                tuple(&board.boundary_edges, |e| {
                    dc(
                        "pcbir.physical:BoardEdge",
                        vec![("id", s(&e.id)), ("start", e_point(&e.start)), ("end", e_point(&e.end))],
                    )
                }),
            ),
            (
                "attachments",
                tuple(&board.attachments, |a| {
                    dc(
                        "pcbir.physical:PhysicalAttachment",
                        vec![
                            ("id", s(&a.id)),
                            ("reference", s(&a.reference)),
                            ("target", opt_text(&a.target)),
                            ("position", e_point(&a.position)),
                            ("offset", e_point(&a.offset)),
                            ("anchor", s(&a.anchor)),
                            ("anchor_point", e_point(&a.anchor_point)),
                            ("rotation", dec(&a.rotation)),
                            ("side", e_side(&a.side)),
                        ],
                    )
                }),
            ),
            (
                "body_overhangs",
                tuple(&board.body_overhangs, |o| {
                    dc(
                        "pcbir.physical:BodyOverhang",
                        vec![
                            ("id", s(&o.id)),
                            ("reference", s(&o.reference)),
                            ("edge", s(&o.edge)),
                            ("start_nm", i(o.start_nm)),
                            ("end_nm", i(o.end_nm)),
                            ("distance_nm", i(o.distance_nm)),
                            ("reason", s(&o.reason)),
                        ],
                    )
                }),
            ),
            (
                "component_heights",
                tuple(&board.component_heights, |h| {
                    dc(
                        "pcbir.physical:ComponentHeight",
                        vec![("id", s(&h.id)), ("reference", s(&h.reference)), ("height_nm", i(h.height_nm))],
                    )
                }),
            ),
            (
                "assembly_envelopes",
                tuple(&board.assembly_envelopes, |e| {
                    dc(
                        "pcbir.physical:AssemblyEnvelope",
                        vec![
                            ("id", s(&e.id)),
                            ("outline", e_outline(&e.outline)),
                            ("side", e_side(&e.side)),
                            ("maximum_height_nm", i(e.maximum_height_nm)),
                        ],
                    )
                }),
            ),
            (
                "assembly_access",
                tuple(&board.assembly_access, |a| {
                    dc(
                        "pcbir.physical:AssemblyAccess",
                        vec![
                            ("id", s(&a.id)),
                            ("reference", s(&a.reference)),
                            ("outline", e_outline(&a.outline)),
                            ("side", s(&a.side)),
                            ("purpose", s(&a.purpose)),
                        ],
                    )
                }),
            ),
            (
                "mechanical_slots",
                tuple(&board.mechanical_slots, |slot| {
                    dc(
                        "pcbir.physical:MechanicalSlot",
                        vec![
                            ("id", s(&slot.id)),
                            ("start", e_point(&slot.start)),
                            ("end", e_point(&slot.end)),
                            ("width_nm", i(slot.width_nm)),
                        ],
                    )
                }),
            ),
            ("mechanical_references", tuple(&board.mechanical_references, e_mechanical_reference)),
            (
                "match_groups",
                tuple(&board.match_groups, |g| {
                    dc(
                        "pcbir.physical:NetMatchGroup",
                        vec![("id", s(&g.id)), ("nets", tuple(&g.nets, |n| s(n))), ("max_skew_nm", i(g.max_skew_nm))],
                    )
                }),
            ),
            (
                "component_hole_clearances",
                tuple(&board.component_hole_clearances, |c| {
                    dc(
                        "pcbir.physical:ComponentHoleClearance",
                        vec![
                            ("reference", s(&c.reference)),
                            ("clearance_nm", i(c.clearance_nm)),
                            ("reason", s(&c.reason)),
                        ],
                    )
                }),
            ),
            (
                "decoupling_links",
                tuple(&board.decoupling_links, |l| {
                    dc(
                        "pcbir.physical:DecouplingLink",
                        vec![
                            ("capacitor", e_pad_reference(&l.capacitor)),
                            ("target", e_pad_reference(&l.target)),
                            ("return_pad", e_pad_reference(&l.return_pad)),
                            ("net", s(&l.net)),
                            ("return_net", s(&l.return_net)),
                        ],
                    )
                }),
            ),
            ("silkscreen", tuple(&board.silkscreen, e_artwork)),
            (
                "power_feeds",
                tuple(&board.power_feeds, |f| {
                    dc(
                        "pcbir.physical:PowerFeedBoundary",
                        vec![
                            ("net", s(&f.net)),
                            ("terminal", e_pad_reference(&f.terminal)),
                            ("upstream_net", s(&f.upstream_net)),
                            ("upstream_terminal", e_pad_reference(&f.upstream_terminal)),
                            ("provenance", s(&f.provenance)),
                        ],
                    )
                }),
            ),
        ],
    )
}

/// `np.float64` rows as a little-endian C-order `$ndarray`.
fn e_array(rows: &[Vec<f64>], nx: usize) -> StateValue {
    let mut data = Vec::with_capacity(rows.len() * nx * 8);
    for row in rows {
        for value in row {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    StateValue::NdArray(NdArray { dtype: "<f8".to_owned(), shape: vec![rows.len() as u64, nx as u64], data })
}

fn e_congestion(map: &CongestionMap) -> StateValue {
    dc(
        "pcbir.congestion_feedback:CongestionMap",
        vec![
            ("tile_nm", i(map.tile_nm)),
            ("x0_nm", i(map.x0_nm)),
            ("y0_nm", i(map.y0_nm)),
            ("nx", i(map.nx as i64)),
            ("ny", i(map.ny as i64)),
            ("demand", e_array(&map.demand, map.nx)),
            ("capacity", e_array(&map.capacity, map.nx)),
            ("source", s(&map.source)),
            (
                "failed_points",
                tuple(&map.failed_points, |(net, point)| StateValue::Tuple(vec![s(net), e_point(point)])),
            ),
        ],
    )
}

/// `encode(result)`: the `DetailedRoutingResult` the replay saves with
/// `--save-result`.
pub fn encode_result(result: &DetailedRoutingResult) -> StateValue {
    dc(
        "pcbir.detailed:DetailedRoutingResult",
        vec![
            ("status", en("pcbir.detailed:DetailedRoutingStatus", result.status.value())),
            ("board", encode_board(&result.board)),
            (
                "nets",
                tuple(&result.nets, |net| {
                    dc(
                        "pcbir.detailed:DetailedNetResult",
                        vec![
                            ("net", s(&net.net)),
                            ("connected", b(net.connected)),
                            ("track_count", i(net.track_count)),
                            ("via_count", i(net.via_count)),
                            ("length_nm", i(net.length_nm)),
                            ("guide_deviation_count", i(net.guide_deviation_count)),
                            ("diagnostics", tuple(&net.diagnostics, |d| s(d))),
                            ("orthogonal_mode_used", b(net.orthogonal_mode_used)),
                            ("search_policy", StateValue::Null),
                            ("resumed_branch_count", i(net.resumed_branch_count)),
                            ("search_budget_exhausted", b(net.search_budget_exhausted)),
                            ("retry_heuristic_weight_percent", opt_int(net.retry_heuristic_weight_percent)),
                            ("warnings", tuple(&net.warnings, |w| s(w))),
                            ("search_stats", StateValue::Null),
                        ],
                    )
                }),
            ),
            (
                "metrics",
                dc(
                    "pcbir.detailed:DetailedRoutingMetrics",
                    vec![
                        ("routed_net_count", i(result.metrics.routed_net_count)),
                        ("unrouted_net_count", i(result.metrics.unrouted_net_count)),
                        ("conflict_resource_count", i(result.metrics.conflict_resource_count)),
                        ("total_conflict_overflow", i(result.metrics.total_conflict_overflow)),
                        ("track_count", i(result.metrics.track_count)),
                        ("via_count", i(result.metrics.via_count)),
                        ("total_length_nm", i(result.metrics.total_length_nm)),
                        ("passes", i(result.metrics.passes)),
                    ],
                ),
            ),
            ("locked_track_count", i(result.locked_track_count)),
            ("locked_via_count", i(result.locked_via_count)),
            ("global_routing_fingerprint", s(&result.global_routing_fingerprint)),
            ("routing_fingerprint", s(&result.routing_fingerprint)),
            ("repair_trials", StateValue::Tuple(Vec::new())),
            ("search_work", StateValue::Null),
            ("congestion", opt(&result.congestion, e_congestion)),
        ],
    )
}

/// `save_state(path, result, note=...)`: the file text.
pub fn result_state_text(result: &DetailedRoutingResult) -> String {
    let metadata = Object::from([(
        "note".to_owned(),
        Value::from("Scoped unified replay result; not source/native admission or board qualification"),
    )]);
    StateDocument::new("pcbir.detailed:DetailedRoutingResult", metadata, encode_result(result)).to_text()
}

/// Whether the status enum round-trips (keeps the status type in use).
pub fn status_value(status: DetailedRoutingStatus) -> &'static str {
    status.value()
}

#[allow(dead_code)]
fn unused(_: &DetailedNetResult, _: &DetailedRoutingMetrics) {}

// ---------------------------------------------------------------------------
// Writing the router's inputs (`save_inputs_path`)

fn f(value: f64) -> StateValue {
    StateValue::Float(value)
}

fn e_grid_node(node: &GridNode) -> StateValue {
    dc(
        "pcbir.routing:GridNode",
        vec![("layer_index", i(node.layer_index)), ("x_index", i(node.x_index)), ("y_index", i(node.y_index))],
    )
}

fn e_pin_access(access: &PinAccess) -> StateValue {
    dc(
        "pcbir.routing:PinAccess",
        vec![
            ("pad", e_pad_reference(&access.pad)),
            ("pad_position", e_point(&access.pad_position)),
            ("node", e_grid_node(&access.node)),
            ("access_position", e_point(&access.access_position)),
            ("layer", e_layer(&access.layer)),
            ("tracks", tuple(&access.tracks, e_track)),
            ("via", opt(&access.via, e_via)),
            ("region_only", b(access.region_only)),
            ("retained_via", opt(&access.retained_via, e_via)),
        ],
    )
}

fn e_net_route(route: &GlobalNetRoute) -> StateValue {
    dc(
        "pcbir.routing:GlobalNetRoute",
        vec![
            ("net", s(&route.net)),
            ("connected", b(route.connected)),
            ("accesses", tuple(&route.accesses, e_pin_access)),
            (
                "segments",
                tuple(&route.segments, |segment| {
                    dc(
                        "pcbir.routing:GlobalRouteSegment",
                        vec![
                            ("net", s(&segment.net)),
                            ("layer", e_layer(&segment.layer)),
                            ("start", e_point(&segment.start)),
                            ("end", e_point(&segment.end)),
                            ("guide_half_width_nm", i(segment.guide_half_width_nm)),
                            ("resource_id", s(&segment.resource_id)),
                        ],
                    )
                }),
            ),
            (
                "vias",
                tuple(&route.vias, |via| {
                    dc(
                        "pcbir.routing:GlobalViaProposal",
                        vec![
                            ("net", s(&via.net)),
                            ("position", e_point(&via.position)),
                            ("from_layer", e_layer(&via.from_layer)),
                            ("to_layer", e_layer(&via.to_layer)),
                            ("resource_id", s(&via.resource_id)),
                        ],
                    )
                }),
            ),
            ("length_nm", i(route.length_nm)),
            ("diagnostics", tuple(&route.diagnostics, |d| s(d))),
            ("deferred_to_zone", b(route.deferred_to_zone)),
        ],
    )
}

fn e_hotspot(hotspot: &RoutingHotspot) -> StateValue {
    dc(
        "pcbir.routing:RoutingHotspot",
        vec![
            ("resource_id", s(&hotspot.resource_id)),
            ("layer", opt(&hotspot.layer, e_layer)),
            ("start", e_point(&hotspot.start)),
            ("end", e_point(&hotspot.end)),
            ("capacity", i(hotspot.capacity)),
            ("usage", i(hotspot.usage)),
            ("overflow", i(hotspot.overflow)),
            ("contributors", tuple(&hotspot.contributors, |c| s(c))),
        ],
    )
}

/// `encode(global_route)`: the `GlobalRoutingResult` dataclass.
pub fn encode_global_route(route: &GlobalRoutingResult) -> StateValue {
    let metrics = &route.metrics;
    dc(
        "pcbir.routing:GlobalRoutingResult",
        vec![
            ("status", en("pcbir.routing:GlobalRoutingStatus", route.status.value())),
            ("routes", tuple(&route.routes, e_net_route)),
            ("hotspots", tuple(&route.hotspots, e_hotspot)),
            (
                "metrics",
                dc(
                    "pcbir.routing:GlobalRoutingMetrics",
                    vec![
                        ("routed_net_count", i(metrics.routed_net_count)),
                        ("unrouted_net_count", i(metrics.unrouted_net_count)),
                        ("total_overflow", i(metrics.total_overflow)),
                        ("maximum_overflow", i(metrics.maximum_overflow)),
                        ("overfull_resource_count", i(metrics.overfull_resource_count)),
                        ("maximum_utilization_ppm", i(metrics.maximum_utilization_ppm)),
                        ("total_length_nm", i(metrics.total_length_nm)),
                        ("proposed_via_count", i(metrics.proposed_via_count)),
                        ("iterations", i(metrics.iterations)),
                        ("region_only_access_count", i(metrics.region_only_access_count)),
                        ("deferred_net_count", i(metrics.deferred_net_count)),
                    ],
                ),
            ),
            ("placement_fingerprint", s(&route.placement_fingerprint)),
            ("routing_fingerprint", s(&route.routing_fingerprint)),
            ("fixed_prefix_digest", opt_text(&route.fixed_prefix_digest)),
        ],
    )
}

fn e_bounds(bounds: &Bounds) -> StateValue {
    dc(
        "pcbir.geometry:Bounds",
        vec![
            ("min_x", i(bounds.min_x)),
            ("min_y", i(bounds.min_y)),
            ("max_x", i(bounds.max_x)),
            ("max_y", i(bounds.max_y)),
        ],
    )
}

/// `encode(options)`: the `UnifiedRouterOptions` dataclass, every field.
pub fn encode_unified_options(o: &UnifiedRouterOptions) -> StateValue {
    let fine = |options: &FineRegionOptions| {
        dc(
            "pcbir.fine_regions:FineRegionOptions",
            vec![
                ("pad_pitch_threshold_nm", i(options.pad_pitch_threshold_nm)),
                ("margin_nm", i(options.margin_nm)),
                ("maximum_factor", i(options.maximum_factor)),
                ("merge_gap_nm", i(options.merge_gap_nm)),
                ("minimum_area_nm2", i(options.minimum_area_nm2)),
                ("minimum_pads", i(options.minimum_pads)),
                ("enabled", b(options.enabled)),
                ("dense_packages", b(options.dense_packages)),
            ],
        )
    };
    let bias = |bias: &LayerBias| {
        dc(
            "pcbir.routing_costs:LayerBias",
            vec![
                ("net", s(&bias.net)),
                ("layer", e_layer(&bias.layer)),
                ("area", e_bounds(&bias.area)),
                ("rate", i(bias.rate)),
            ],
        )
    };
    dc(
        "pcbir.unified_router:UnifiedRouterOptions",
        vec![
            ("pitch_nm", opt_int(o.pitch_nm)),
            ("maximum_iterations", i(o.maximum_iterations)),
            ("stall_iterations", i(o.stall_iterations)),
            ("present_factor", f(o.present_factor)),
            ("present_growth", f(o.present_growth)),
            ("history_increment", f(o.history_increment)),
            ("via_cost_mm", f(o.via_cost_mm)),
            ("wrong_way_cost", f(o.wrong_way_cost)),
            ("outer_layer_cost", f(o.outer_layer_cost)),
            ("attach_radius_nm", i(o.attach_radius_nm)),
            ("neck_radius_nm", i(o.neck_radius_nm)),
            ("attach_candidates", i(o.attach_candidates)),
            ("window_margin_nm", i(o.window_margin_nm)),
            ("present_limit", f(o.present_limit)),
            ("window_growth_limit", i(o.window_growth_limit)),
            ("maximum_expansions", i(o.maximum_expansions)),
            ("commit_rounds", i(o.commit_rounds)),
            ("pour_recheck", b(o.pour_recheck)),
            ("buses_first", b(o.buses_first)),
            ("bus_length_budget", b(o.bus_length_budget)),
            ("bus_skew_tuning", b(o.bus_skew_tuning)),
            ("length_priority", b(o.length_priority)),
            ("full_reroute_every", i(o.full_reroute_every)),
            ("rejection_marker_cost", f(o.rejection_marker_cost)),
            ("route_smoothing", b(o.route_smoothing)),
            ("ball_field_cost", f(o.ball_field_cost)),
            ("reuse_plane_vias", b(o.reuse_plane_vias)),
            ("ball_field_via_sites", b(o.ball_field_via_sites)),
            ("ball_field_band", i(o.ball_field_band)),
            ("ball_field_exact", b(o.ball_field_exact)),
            ("ball_field_exact_reach_nm", opt_int(o.ball_field_exact_reach_nm)),
            ("exit_reservation_cost", f(o.exit_reservation_cost)),
            ("bus_bundles", b(o.bus_bundles)),
            ("bga_fanout", b(o.bga_fanout)),
            ("raster_margin_nm", opt_int(o.raster_margin_nm)),
            ("time_budget_s", o.time_budget_s.map_or(StateValue::Null, f)),
            (
                "grid_window_nm",
                o.grid_window_nm
                    .map_or(StateValue::Null, |(x0, y0, x1, y1)| StateValue::Tuple(vec![i(x0), i(y0), i(x1), i(y1)])),
            ),
            ("critical_pair_state_limit", opt_int(o.critical_pair_state_limit)),
            ("critical_pair_state_escalation", i(o.critical_pair_state_escalation)),
            ("power_islands", opt_text(&o.power_islands)),
            ("power_islands_ball_grid_only", b(o.power_islands_ball_grid_only)),
            ("plane_fill_width", b(o.plane_fill_width)),
            ("implicit_neck_nm", i(o.implicit_neck_nm)),
            ("local_escape_radius_nm", i(o.local_escape_radius_nm)),
            ("local_escape_step_nm", i(o.local_escape_step_nm)),
            ("save_inputs_path", opt_text(&o.save_inputs_path)),
            ("local_escape_limit", i(o.local_escape_limit)),
            ("pre_route_decouplers", b(o.pre_route_decouplers)),
            ("direct", b(o.direct)),
            ("escapes_first", b(o.escapes_first)),
            ("escape_net_order", tuple(&o.escape_net_order, |n| s(n))),
            ("escape_layer_biases", tuple(&o.escape_layer_biases, bias)),
            ("retain_escape_copper", b(o.retain_escape_copper)),
            ("planned_escapes_as_copper", b(o.planned_escapes_as_copper)),
            ("share_commit_index", b(o.share_commit_index)),
            ("failed_retry_every", i(o.failed_retry_every)),
            ("failed_retry_limit", i(o.failed_retry_limit)),
            ("failed_retry_before_exit", b(o.failed_retry_before_exit)),
            ("outward_axis_threshold_nm", i(o.outward_axis_threshold_nm)),
            ("attach_extension_steps_nm", tuple(&o.attach_extension_steps_nm, |n| i(*n))),
            ("attach_extension_reach_nm", i(o.attach_extension_reach_nm)),
            ("local_escape_detour_nm", i(o.local_escape_detour_nm)),
            ("local_escape_states", i(o.local_escape_states)),
            ("open_flood_limit", i(o.open_flood_limit)),
            ("open_space_area_nm2", i(o.open_space_area_nm2)),
            ("plane_piece_area_nm2", i(o.plane_piece_area_nm2)),
            ("plane_piece_fraction", f(o.plane_piece_fraction)),
            ("repair_neighbour_reach_nm", i(o.repair_neighbour_reach_nm)),
            ("via_budget_factor", f(o.via_budget_factor)),
            ("via_overrun_factor", f(o.via_overrun_factor)),
            ("bend_cost", f(o.bend_cost)),
            ("guide_cost", f(o.guide_cost)),
            ("guide_margin_nm", i(o.guide_margin_nm)),
            ("congestion_tile_nm", i(o.congestion_tile_nm)),
            ("export_congestion", b(o.export_congestion)),
            ("fine_regions", s(&o.fine_regions)),
            ("fine_region_options", opt(&o.fine_region_options, fine)),
        ],
    )
}

/// `save_state(save_inputs_path + ".bga.json", {...}, note="bga_fanout output")`:
/// the ball-grid fanout copper beside the router inputs, so a replay can take
/// it out and fan out again.
pub fn bga_sidecar_text(
    tracks: &[TrackSegment],
    vias: &[Via],
    array_flow: bool,
    escape_plan_options: Option<&crate::escape_plan::EscapePlanOptions>,
) -> String {
    let options = escape_plan_options.map_or(StateValue::Null, |o| {
        dc(
            "pcbir.escape_plan:EscapePlanOptions",
            vec![
                ("cluster_pair_minimum_nets", i(o.cluster_pair_minimum_nets)),
                ("maximum_pair_nets", i(o.maximum_pair_nets)),
                ("field_spacing_factor", i(o.field_spacing_factor)),
                ("field_spacing_minimum_nm", i(o.field_spacing_minimum_nm)),
                ("two_end_order", b(o.two_end_order)),
                ("corridor_margin_nm", i(o.corridor_margin_nm)),
                ("layer_bias_rate", i(o.layer_bias_rate)),
                ("array_flow", b(o.array_flow)),
                ("bus_layers", b(o.bus_layers)),
                ("via_lattice", b(o.via_lattice)),
                ("lattice_band_rings", i(o.lattice_band_rings)),
            ],
        )
    });
    let entries = vec![
        (s("tracks"), tuple(tracks, e_track)),
        (s("vias"), tuple(vias, e_via)),
        (s("array_flow"), b(array_flow)),
        (s("escape_plan_options"), options),
    ];
    let metadata = Object::from([("note".to_owned(), Value::from("bga_fanout output"))]);
    StateDocument::new("builtins:dict", metadata, StateValue::Map { entries, proxy: false }).to_text()
}

/// `save_state(options.save_inputs_path, {...}, note="route_unified inputs")`:
/// the file text of the router's inputs. Only the inputs the pipeline passes
/// are written; saved fanout accesses or a net selection report not
/// implemented.
pub fn inputs_state_text(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &UnifiedRouterOptions,
    only_nets: Option<&IndexSet<String>>,
    has_accesses: bool,
) -> Result<String> {
    if has_accesses {
        return not_implemented("unified router save_inputs_path with fanout or required accesses");
    }
    let mut saved = options.clone();
    saved.save_inputs_path = None;
    let only = only_nets.map_or(StateValue::Null, |names| StateValue::FrozenSet(names.iter().map(|n| s(n)).collect()));
    let entries = vec![
        (s("board"), encode_board(board)),
        (s("global_route"), encode_global_route(global_route)),
        (s("options"), encode_unified_options(&saved)),
        (s("fanout_accesses"), StateValue::Null),
        (s("fanout_created_vias"), StateValue::Null),
        (s("fanout_created_tracks"), StateValue::Null),
        (s("only_nets"), only),
        (s("required_accesses"), StateValue::Null),
    ];
    let metadata = Object::from([("note".to_owned(), Value::from("route_unified inputs"))]);
    Ok(StateDocument::new("builtins:dict", metadata, StateValue::Map { entries, proxy: false }).to_text())
}

// ---------------------------------------------------------------------------
// Writing route-board checkpoints

fn e_frozenset_of<T>(items: impl IntoIterator<Item = T>, encode: impl Fn(T) -> StateValue) -> StateValue {
    StateValue::FrozenSet(items.into_iter().map(encode).collect())
}

/// `encode(options)`: the `PlacementPlannerOptions` dataclass.
pub fn encode_planner_options(o: &copper_place::PlannerOptions) -> StateValue {
    dc(
        "pcbir.placement:PlacementPlannerOptions",
        vec![
            ("grid_step_nm", i(o.grid_step_nm)),
            ("edge_clearance_nm", i(o.edge_clearance_nm)),
            ("component_clearance_nm", i(o.component_clearance_nm)),
            ("congestion_bin_nm", i(o.congestion_bin_nm)),
            ("analytical_iterations", i(o.analytical_iterations)),
            ("satellite_seeding", b(o.satellite_seeding)),
            ("auto_pair_corridors", b(o.auto_pair_corridors)),
            ("analytical_step_mm", f(o.analytical_step_mm)),
            ("analytical_momentum", f(o.analytical_momentum)),
            ("refinement_passes", i(o.refinement_passes)),
            ("refinement_radius_steps", i(o.refinement_radius_steps)),
            ("candidate_count", i(o.candidate_count)),
            ("legalization_candidates", i(o.legalization_candidates)),
            ("exact_repair_limit", i(o.exact_repair_limit)),
            ("exact_repair_candidates", i(o.exact_repair_candidates)),
            ("fixed_references", e_frozenset_of(&o.fixed_references, |r| s(r))),
            ("escape_margin_nm", i(o.escape_margin_nm)),
            ("escape_transit_lanes", i(o.escape_transit_lanes)),
            ("escape_spacing_passes", i(o.escape_spacing_passes)),
            ("escape_max_movement_nm", i(o.escape_max_movement_nm)),
            ("power_domain_weight", f(o.power_domain_weight)),
            ("support_pattern_trials", i(o.support_pattern_trials)),
            ("package_support_trials", i(o.package_support_trials)),
            ("decoupling_placement_trials", i(o.decoupling_placement_trials)),
            ("seed_count", opt_int(o.seed_count)),
        ],
    )
}

/// `encode(options)`: the `GlobalRouterOptions` dataclass.
pub fn encode_global_options(o: &GlobalRouterOptions) -> StateValue {
    dc(
        "pcbir.routing:GlobalRouterOptions",
        vec![
            ("tile_size_nm", i(o.tile_size_nm)),
            ("maximum_iterations", i(o.maximum_iterations)),
            ("present_penalty", i(o.present_penalty)),
            ("present_penalty_growth", i(o.present_penalty_growth)),
            ("history_penalty", i(o.history_penalty)),
            ("via_cost", i(o.via_cost)),
            ("bend_cost", i(o.bend_cost)),
            ("layer_preference_cost", i(o.layer_preference_cost)),
            ("direction_preference_cost", i(o.direction_preference_cost)),
            ("guide_half_width_tiles", i(o.guide_half_width_tiles)),
            ("layer_assignment_passes", i(o.layer_assignment_passes)),
            ("local_demand_cost", i(o.local_demand_cost)),
            ("local_demand_pitch_nm", i(o.local_demand_pitch_nm)),
            ("pin_access_candidates", i(o.pin_access_candidates)),
            ("escape_radius_nm", i(o.escape_radius_nm)),
            ("escape_step_nm", i(o.escape_step_nm)),
            ("critical_escape_step_nm", opt_int(o.critical_escape_step_nm)),
        ],
    )
}

/// `encode(options)`: the `PlacementRoutingFeedbackOptions` dataclass.
pub fn encode_feedback_options(o: &crate::routeflow::PlacementRoutingFeedbackOptions) -> StateValue {
    dc(
        "pcbir.routeflow:PlacementRoutingFeedbackOptions",
        vec![
            ("maximum_iterations", i(o.maximum_iterations)),
            ("initial_movement_nm", i(o.initial_movement_nm)),
            ("minimum_movement_nm", i(o.minimum_movement_nm)),
            ("maximum_trials_per_iteration", i(o.maximum_trials_per_iteration)),
            ("stagnation_limit", i(o.stagnation_limit)),
            ("preferred_candidate_id", opt_text(&o.preferred_candidate_id)),
        ],
    )
}

/// `encode(result)`: the `PlacementRoutingResult` a placement checkpoint holds
/// (no package patterns: support-pattern trials are not rewritten).
pub fn encode_placement_result(result: &crate::routeflow::PlacementRoutingResult) -> StateValue {
    dc(
        "pcbir.routeflow:PlacementRoutingResult",
        vec![
            ("status", en("pcbir.routeflow:FeedbackStatus", result.status.value())),
            ("board", encode_board(&result.board)),
            ("global_route", encode_global_route(&result.global_route)),
            ("placement_candidate", s(&result.placement_candidate)),
            (
                "iterations",
                tuple(&result.iterations, |item| {
                    dc(
                        "pcbir.routeflow:FeedbackIteration",
                        vec![
                            ("iteration", i(item.iteration)),
                            ("accepted", b(item.accepted)),
                            ("movement_nm", i(item.movement_nm)),
                            ("trials", i(item.trials)),
                            ("unrouted_net_count", i(item.unrouted_net_count)),
                            ("total_overflow", i(item.total_overflow)),
                            ("maximum_overflow", i(item.maximum_overflow)),
                            ("routing_fingerprint", s(&item.routing_fingerprint)),
                        ],
                    )
                }),
            ),
            ("accepted_moves", i(result.accepted_moves)),
            ("full_route_certified", b(result.full_route_certified)),
            ("package_patterns", StateValue::Tuple(Vec::new())),
        ],
    )
}

/// `encode(options)`: the `PlaneStitchOptions` dataclass.
pub fn encode_plane_stitch_options(o: &crate::plane::PlaneStitchOptions) -> StateValue {
    let pads = |pads: &Option<std::collections::BTreeSet<PadReference>>| {
        opt(pads, |pads| e_frozenset_of(pads, e_pad_reference))
    };
    dc(
        "pcbir.plane:PlaneStitchOptions",
        vec![
            ("step_nm", i(o.step_nm)),
            ("maximum_radius_nm", i(o.maximum_radius_nm)),
            ("maximum_contact_radius_nm", i(o.maximum_contact_radius_nm)),
            ("maximum_detour_nm", i(o.maximum_detour_nm)),
            ("maze_step_nm", i(o.maze_step_nm)),
            ("maze_state_budget", i(o.maze_state_budget)),
            ("candidate_bias", opt_text(&o.candidate_bias)),
            ("escape_width_nm", opt_int(o.escape_width_nm)),
            ("only_pads", pads(&o.only_pads)),
            ("preferred_ground_pads", pads(&o.preferred_ground_pads)),
            ("ground_via_in_pad", b(o.ground_via_in_pad)),
            ("include_surface_zones", b(o.include_surface_zones)),
            ("require_explicit", b(o.require_explicit)),
            ("keepouts", tuple(&o.keepouts, e_keepout)),
        ],
    )
}

fn e_decoupling_link(link: &DecouplingLink) -> StateValue {
    dc(
        "pcbir.physical:DecouplingLink",
        vec![
            ("capacitor", e_pad_reference(&link.capacitor)),
            ("target", e_pad_reference(&link.target)),
            ("return_pad", e_pad_reference(&link.return_pad)),
            ("net", s(&link.net)),
            ("return_net", s(&link.return_net)),
        ],
    )
}

fn e_copper_path(path: &crate::decoupling_paths::DecouplingCopperPath) -> StateValue {
    use crate::decoupling_paths::{DecouplingPathNode, PathEdge};
    let node = |n: &DecouplingPathNode| {
        dc(
            "pcbir.decoupling_paths:DecouplingPathNode",
            vec![("position", e_point(&n.position)), ("layer", e_layer(&n.layer))],
        )
    };
    dc(
        "pcbir.decoupling_paths:DecouplingCopperPath",
        vec![
            ("start", e_pad_reference(&path.start)),
            ("end", e_pad_reference(&path.end)),
            ("net", s(&path.net)),
            (
                "steps",
                tuple(&path.steps, |step| {
                    let edge = match &step.edge {
                        PathEdge::Track(track) => e_track(track),
                        PathEdge::Via(via) => e_via(via),
                    };
                    dc(
                        "pcbir.decoupling_paths:DecouplingPathStep",
                        vec![("edge", edge), ("start", node(&step.start)), ("end", node(&step.end))],
                    )
                }),
            ),
        ],
    )
}

fn e_witness(witness: &crate::decoupling_paths::DecouplingCrossLayerWitness) -> StateValue {
    dc(
        "pcbir.decoupling_paths:DecouplingCrossLayerWitness",
        vec![
            ("link", e_decoupling_link(&witness.link)),
            ("supply_path", e_copper_path(&witness.supply_path)),
            ("return_target", e_pad_reference(&witness.return_target)),
            ("return_path", e_copper_path(&witness.return_path)),
            ("context_digest", s(&witness.context_digest)),
            ("board_digest", s(&witness.board_digest)),
            ("qualification", s(&witness.qualification)),
        ],
    )
}

/// `encode(reserved)`: a bypass-only `FanoutResult` (no accesses, pending
/// pads or fanout analysis), as the critical stage receives it.
pub fn encode_bypass_reservation(reserved: &crate::bypass_reservations::BypassReservation) -> StateValue {
    dc(
        "pcbir.fanout:FanoutResult",
        vec![
            ("board", encode_board(&reserved.board)),
            ("accesses", StateValue::Map { entries: Vec::new(), proxy: false }),
            ("pending_pads", StateValue::Tuple(Vec::new())),
            ("added_track_count", i(reserved.added_track_count)),
            ("added_via_count", i(reserved.added_via_count)),
            ("created_vias", tuple(&reserved.created_vias, e_via)),
            ("pin_analysis", StateValue::Tuple(Vec::new())),
            ("created_tracks", tuple(&reserved.created_tracks, e_track)),
            ("assignment", StateValue::Null),
            ("boundary_accesses", StateValue::Null),
            ("surface_accesses", StateValue::Null),
            ("decoupling_pending", StateValue::Tuple(Vec::new())),
            ("decoupling_tracks", tuple(&reserved.decoupling_tracks, e_track)),
            ("decoupling_vias", tuple(&reserved.decoupling_vias, e_via)),
            ("decoupling_witnesses", tuple(&reserved.decoupling_witnesses, e_witness)),
            ("decoupling_pending_links", StateValue::Tuple(Vec::new())),
            ("late_bypass", StateValue::Null),
        ],
    )
}

/// The dict the critical stage saves as `critical-inputs` (and
/// `critical-retry-inputs`): the board and global route it starts from, the
/// held bypass reservation and the return-plane stitch options.
pub fn critical_inputs_state_text(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    reserved_accesses: Option<&crate::bypass_reservations::BypassReservation>,
    plane_stitch_options: Option<&crate::plane::PlaneStitchOptions>,
) -> String {
    let entries = vec![
        (s("board"), encode_board(board)),
        (s("global_route"), encode_global_route(global_route)),
        (s("reserved_accesses"), reserved_accesses.map_or(StateValue::Null, encode_bypass_reservation)),
        (s("plane_stitch_options"), plane_stitch_options.map_or(StateValue::Null, encode_plane_stitch_options)),
    ];
    StateDocument::new("builtins:dict", Object::new(), StateValue::Map { entries, proxy: false }).to_text()
}

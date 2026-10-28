//! Resolved footprint geometry (`physical.FootprintPad`, `PhysicalFootprint`,
//! the footprint graphics and `pad_connections.InternalPadGroup`).

use indexmap::{IndexMap, IndexSet};

use copper_units::Decimal;

use crate::board::{CopperKeepout, Result};
use crate::geometry::Point;

/// `Size`: a positive width and height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width_nm: i64,
    pub height_nm: i64,
}

impl Size {
    pub fn new(width_nm: i64, height_nm: i64) -> Result<Self> {
        if width_nm <= 0 || height_nm <= 0 {
            return Err("physical sizes must be positive".to_owned());
        }
        Ok(Size { width_nm, height_nm })
    }
}

macro_rules! text_enum {
    ($name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<$name> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

text_enum!(PadKind { Smd = "smd", Aperture = "aperture", ThroughHole = "through_hole", NonPlatedThroughHole = "non_plated_through_hole" });
text_enum!(PadShape { Circle = "circle", Oval = "oval", Rectangle = "rectangle", Roundrect = "roundrect" });
text_enum!(ZoneConnection { Thermal = "thermal", Solid = "solid", None = "none", ThtThermal = "tht_thermal" });
text_enum!(FootprintLayer {
    Silkscreen = "silkscreen", Fabrication = "fabrication", Courtyard = "courtyard", Adhesive = "adhesive",
    Documentation = "documentation", SolderMask = "solder_mask", SolderPaste = "solder_paste",
});

impl PadKind {
    /// Apertures and NPTHs carry no electrical connection.
    pub fn is_electrical(self) -> bool {
        !matches!(self, PadKind::Aperture | PadKind::NonPlatedThroughHole)
    }
}

/// `FootprintPad`.
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintPad {
    pub number: String,
    pub position: Point,
    pub size: Size,
    pub kind: PadKind,
    pub shape: PadShape,
    pub rotation_degrees: Decimal,
    pub drill: Option<Size>,
    pub roundrect_ratio_ppm: i64,
    pub has_solder_mask: bool,
    pub has_solder_paste: bool,
    pub zone_connection: Option<ZoneConnection>,
    pub heatsink: bool,
    pub remove_unused_layers: bool,
    pub connector_contact: bool,
}

impl FootprintPad {
    /// `FootprintPad.__post_init__`: normalizes the rotation and validates.
    pub fn validated(mut self) -> Result<Self> {
        if self.number.is_empty() && self.kind.is_electrical() {
            return Err("electrical footprint pad number cannot be empty".to_owned());
        }
        self.rotation_degrees = self.rotation_degrees.rem(&Decimal::from_i64(360)).map_err(|e| e.to_string())?;
        if matches!(self.kind, PadKind::Smd | PadKind::Aperture) && self.drill.is_some() {
            return Err("SMD and aperture pads cannot have a drill".to_owned());
        }
        if self.connector_contact && (self.kind != PadKind::Smd || self.has_solder_paste) {
            return Err("connector contacts must be non-pasted surface pads".to_owned());
        }
        if matches!(self.kind, PadKind::ThroughHole | PadKind::NonPlatedThroughHole) {
            let Some(drill) = self.drill else { return Err("through-hole pads require a drill".to_owned()) };
            if self.kind == PadKind::ThroughHole
                && (drill.width_nm >= self.size.width_nm || drill.height_nm >= self.size.height_nm)
            {
                return Err("pad drill must be smaller than pad size".to_owned());
            }
            if self.kind == PadKind::NonPlatedThroughHole
                && (drill.width_nm > self.size.width_nm || drill.height_nm > self.size.height_nm)
            {
                return Err("non-plated drill cannot exceed its hole envelope".to_owned());
            }
        }
        if !(0..=500_000).contains(&self.roundrect_ratio_ppm) {
            return Err("roundrect pad ratio must be between 0 and 0.5".to_owned());
        }
        Ok(self)
    }
}

/// A placement-relative non-copper drawing (`FootprintLine` ... `FootprintPolygon`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FootprintGraphic {
    Line { start: Point, end: Point, width_nm: i64, layer: FootprintLayer },
    Rectangle { start: Point, end: Point, width_nm: i64, layer: FootprintLayer, filled: bool },
    Circle { center: Point, end: Point, width_nm: i64, layer: FootprintLayer, filled: bool },
    Arc { start: Point, midpoint: Point, end: Point, width_nm: i64, layer: FootprintLayer },
    Polygon { points: Vec<Point>, width_nm: i64, layer: FootprintLayer, filled: bool },
}

impl FootprintGraphic {
    pub fn layer(&self) -> FootprintLayer {
        match self {
            FootprintGraphic::Line { layer, .. }
            | FootprintGraphic::Rectangle { layer, .. }
            | FootprintGraphic::Circle { layer, .. }
            | FootprintGraphic::Arc { layer, .. }
            | FootprintGraphic::Polygon { layer, .. } => *layer,
        }
    }

    pub fn width_nm(&self) -> i64 {
        match self {
            FootprintGraphic::Line { width_nm, .. }
            | FootprintGraphic::Rectangle { width_nm, .. }
            | FootprintGraphic::Circle { width_nm, .. }
            | FootprintGraphic::Arc { width_nm, .. }
            | FootprintGraphic::Polygon { width_nm, .. } => *width_nm,
        }
    }

    /// The dataclass checks of each graphic type.
    pub fn validated(self) -> Result<Self> {
        let (name, negative) = match &self {
            FootprintGraphic::Line { start, end, width_nm, .. } => {
                if start == end {
                    return Err("footprint line cannot have zero length".to_owned());
                }
                ("line", *width_nm < 0)
            }
            FootprintGraphic::Rectangle { start, end, width_nm, .. } => {
                if start == end {
                    return Err("footprint rectangle cannot have zero size".to_owned());
                }
                ("rectangle", *width_nm < 0)
            }
            FootprintGraphic::Circle { center, end, width_nm, .. } => {
                if center == end {
                    return Err("footprint circle must have a positive radius".to_owned());
                }
                ("circle", *width_nm < 0)
            }
            FootprintGraphic::Arc { start, midpoint, end, width_nm, .. } => {
                if start == midpoint || midpoint == end || start == end {
                    return Err("footprint arc requires three distinct points".to_owned());
                }
                ("arc", *width_nm < 0)
            }
            FootprintGraphic::Polygon { points, width_nm, .. } => {
                if points.len() < 3 {
                    return Err("footprint polygon requires at least three points".to_owned());
                }
                ("polygon", *width_nm < 0)
            }
        };
        if negative {
            return Err(format!("footprint {name} width cannot be negative"));
        }
        Ok(self)
    }
}

/// `InternalPadGroup`: pad numbers joined inside the component.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InternalPadGroup {
    pub numbers: Vec<String>,
}

impl InternalPadGroup {
    pub fn new(numbers: Vec<String>) -> Result<Self> {
        let invalid =
            |n: &String| n.is_empty() || n.chars().any(|c| copper_json::python_isspace(c) || c == ',' || c == ';');
        if numbers.is_empty() || numbers.iter().any(invalid) {
            return Err("internal pad groups require nonempty pad numbers".to_owned());
        }
        let unique: IndexSet<&String> = numbers.iter().collect();
        if unique.len() != numbers.len() {
            return Err("internal pad group repeats a pad number".to_owned());
        }
        let mut numbers = numbers;
        numbers.sort();
        Ok(InternalPadGroup { numbers })
    }
}

/// `validate_internal_pad_groups`: known, disjoint groups in sorted order.
pub fn validate_internal_pad_groups(
    groups: Vec<InternalPadGroup>,
    known: &IndexSet<String>,
) -> Result<Vec<InternalPadGroup>> {
    let mut seen: IndexSet<&String> = IndexSet::new();
    for group in &groups {
        if group.numbers.iter().any(|number| !known.contains(number)) {
            return Err("internal pad group references unknown pad number".to_owned());
        }
        if group.numbers.iter().any(|number| seen.contains(number)) {
            return Err("internal pad groups overlap".to_owned());
        }
        seen.extend(&group.numbers);
    }
    let mut groups = groups;
    groups.sort();
    Ok(groups)
}

/// `merge_internal_pad_groups`: the union of overlapping groups.
pub fn merge_internal_pad_groups(groups: &[InternalPadGroup]) -> Result<Vec<InternalPadGroup>> {
    let mut merged: Vec<IndexSet<String>> = Vec::new();
    for group in groups {
        let mut current: IndexSet<String> = group.numbers.iter().cloned().collect();
        let mut untouched = Vec::new();
        for existing in merged {
            if existing.iter().any(|number| current.contains(number)) {
                current.extend(existing);
            } else {
                untouched.push(existing);
            }
        }
        untouched.push(current);
        merged = untouched;
    }
    let mut result = merged
        .into_iter()
        .map(|numbers| InternalPadGroup::new(numbers.into_iter().collect()))
        .collect::<Result<Vec<_>>>()?;
    result.sort();
    Ok(result)
}

/// `PhysicalFootprint`.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalFootprint {
    pub name: String,
    pub pads: Vec<FootprintPad>,
    pub body_size: Size,
    pub source_library_id: Option<String>,
    pub graphics: Vec<FootprintGraphic>,
    pub keepouts: Vec<CopperKeepout>,
    pub metadata: IndexMap<String, String>,
    pub courtyard: Vec<Point>,
    pub height_nm: Option<i64>,
    pub clearance_nm: Option<i64>,
    pub exclude_from_bom: bool,
    pub exclude_from_pos_files: bool,
    pub internal_pad_groups: Vec<InternalPadGroup>,
}

impl PhysicalFootprint {
    /// `PhysicalFootprint.__post_init__`.
    pub fn validated(mut self) -> Result<Self> {
        let electrical: Vec<&FootprintPad> = self.pads.iter().filter(|pad| pad.kind.is_electrical()).collect();
        let known: IndexSet<String> = electrical.iter().map(|pad| pad.number.clone()).collect();
        self.internal_pad_groups = validate_internal_pad_groups(std::mem::take(&mut self.internal_pad_groups), &known)?;
        for group in &self.internal_pad_groups {
            if electrical.iter().filter(|pad| group.numbers.contains(&pad.number)).count() < 2 {
                return Err("internal pad group requires at least two electrical lands".to_owned());
            }
        }
        if self.name.is_empty() {
            return Err("footprint name cannot be empty".to_owned());
        }
        if !self.courtyard.is_empty() && self.courtyard.len() < 3 {
            return Err("a footprint courtyard requires at least three points".to_owned());
        }
        if self.height_nm.is_some_and(|height| height <= 0) {
            return Err("footprint height must be positive".to_owned());
        }
        if self.clearance_nm.is_some_and(|clearance| clearance < 0) {
            return Err("footprint clearance cannot be negative".to_owned());
        }
        let ids: IndexSet<&str> = self.keepouts.iter().map(|keepout| keepout.id.as_str()).collect();
        if ids.len() != self.keepouts.len() {
            return Err("footprint keepout ids must be unique".to_owned());
        }
        Ok(self)
    }
}

fn json_point(point: &Point) -> copper_json::Value {
    use copper_json::Value;
    Value::Object(
        [("x_nm".to_owned(), Value::from(point.x_nm)), ("y_nm".to_owned(), Value::from(point.y_nm))]
            .into_iter()
            .collect(),
    )
}

fn json_size(size: &Size) -> copper_json::Value {
    use copper_json::Value;
    Value::Object(
        [("width_nm".to_owned(), Value::from(size.width_nm)), ("height_nm".to_owned(), Value::from(size.height_nm))]
            .into_iter()
            .collect(),
    )
}

fn json_object(entries: Vec<(&str, copper_json::Value)>) -> copper_json::Value {
    copper_json::Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// The dataclass document of a footprint (`clusters._footprint_geometry_digest`'s `document`).
fn footprint_document(footprint: &PhysicalFootprint) -> copper_json::Value {
    use copper_json::Value;
    let text = |value: &str| Value::from(value);
    let optional_int = |value: Option<i64>| value.map_or(Value::Null, Value::from);
    let pads = footprint
        .pads
        .iter()
        .map(|pad| {
            json_object(vec![
                ("number", text(&pad.number)),
                ("position", json_point(&pad.position)),
                ("size", json_size(&pad.size)),
                ("kind", text(pad.kind.value())),
                ("shape", text(pad.shape.value())),
                ("rotation_degrees", Value::from(pad.rotation_degrees.to_sci_string().as_str())),
                ("drill", pad.drill.as_ref().map_or(Value::Null, json_size)),
                ("roundrect_ratio_ppm", Value::from(pad.roundrect_ratio_ppm)),
                ("has_solder_mask", Value::Bool(pad.has_solder_mask)),
                ("has_solder_paste", Value::Bool(pad.has_solder_paste)),
                ("zone_connection", pad.zone_connection.map_or(Value::Null, |zone| text(zone.value()))),
                ("heatsink", Value::Bool(pad.heatsink)),
                ("remove_unused_layers", Value::Bool(pad.remove_unused_layers)),
                ("connector_contact", Value::Bool(pad.connector_contact)),
            ])
        })
        .collect();
    let points = |points: &[Point]| Value::Array(points.iter().map(json_point).collect());
    let graphics = footprint
        .graphics
        .iter()
        .map(|graphic| match graphic {
            FootprintGraphic::Line { start, end, width_nm, layer } => json_object(vec![
                ("start", json_point(start)),
                ("end", json_point(end)),
                ("width_nm", Value::from(*width_nm)),
                ("layer", text(layer.value())),
            ]),
            FootprintGraphic::Rectangle { start, end, width_nm, layer, filled } => json_object(vec![
                ("start", json_point(start)),
                ("end", json_point(end)),
                ("width_nm", Value::from(*width_nm)),
                ("layer", text(layer.value())),
                ("filled", Value::Bool(*filled)),
            ]),
            FootprintGraphic::Circle { center, end, width_nm, layer, filled } => json_object(vec![
                ("center", json_point(center)),
                ("end", json_point(end)),
                ("width_nm", Value::from(*width_nm)),
                ("layer", text(layer.value())),
                ("filled", Value::Bool(*filled)),
            ]),
            FootprintGraphic::Arc { start, midpoint, end, width_nm, layer } => json_object(vec![
                ("start", json_point(start)),
                ("midpoint", json_point(midpoint)),
                ("end", json_point(end)),
                ("width_nm", Value::from(*width_nm)),
                ("layer", text(layer.value())),
            ]),
            FootprintGraphic::Polygon { points: vertices, width_nm, layer, filled } => json_object(vec![
                ("points", points(vertices)),
                ("width_nm", Value::from(*width_nm)),
                ("layer", text(layer.value())),
                ("filled", Value::Bool(*filled)),
            ]),
        })
        .collect();
    let keepouts = footprint
        .keepouts
        .iter()
        .map(|keepout| {
            json_object(vec![
                ("id", text(&keepout.id)),
                ("layers", Value::Array(keepout.layers.iter().map(|layer| text(layer.value())).collect())),
                (
                    "outline",
                    json_object(vec![
                        ("outer", json_object(vec![("vertices", points(&keepout.vertices))])),
                        ("holes", Value::Array(Vec::new())),
                    ]),
                ),
                ("block_tracks", Value::Bool(keepout.block_tracks)),
                ("block_vias", Value::Bool(keepout.block_vias)),
                ("block_pads", Value::Bool(keepout.block_pads)),
                ("block_zones", Value::Bool(keepout.block_zones)),
                ("block_footprints", Value::Bool(keepout.block_footprints)),
            ])
        })
        .collect();
    let groups = footprint
        .internal_pad_groups
        .iter()
        .map(|group| {
            json_object(vec![("numbers", Value::Array(group.numbers.iter().map(|number| text(number)).collect()))])
        })
        .collect();
    json_object(vec![
        ("name", text(&footprint.name)),
        ("pads", Value::Array(pads)),
        ("body_size", json_size(&footprint.body_size)),
        ("source_library_id", footprint.source_library_id.as_deref().map_or(Value::Null, text)),
        ("graphics", Value::Array(graphics)),
        ("keepouts", Value::Array(keepouts)),
        ("metadata", Value::Object(footprint.metadata.iter().map(|(key, value)| (key.clone(), text(value))).collect())),
        ("courtyard", points(&footprint.courtyard)),
        ("height_nm", optional_int(footprint.height_nm)),
        ("clearance_nm", optional_int(footprint.clearance_nm)),
        ("exclude_from_bom", Value::Bool(footprint.exclude_from_bom)),
        ("exclude_from_pos_files", Value::Bool(footprint.exclude_from_pos_files)),
        ("internal_pad_groups", Value::Array(groups)),
    ])
}

fn footprint_digest(footprint: &PhysicalFootprint, portable: bool) -> String {
    use sha2::{Digest, Sha256};
    let copper_json::Value::Object(mut content) = footprint_document(footprint) else { unreachable!() };
    if footprint.internal_pad_groups.is_empty() {
        content.shift_remove("internal_pad_groups");
    }
    let mut keys = vec![
        "source_path",
        "resolution",
        "managed_reference",
        "source_asset",
        "module_path",
        "module_version",
        "module_checksum",
    ];
    if portable {
        keys.push("source_sha256");
    }
    if let Some(copper_json::Value::Object(metadata)) = content.get_mut("metadata") {
        for key in keys {
            metadata.shift_remove(key);
        }
    }
    let text = copper_json::Dumps::compact().sort_keys(true).dumps(&copper_json::Value::Object(content));
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `footprint_geometry_digest`: identity independent of source line endings and paths.
pub fn footprint_geometry_digest(footprint: &PhysicalFootprint) -> String {
    footprint_digest(footprint, true)
}

/// `legacy_footprint_geometry_digest`: the digest of older locked macro assets.
pub fn legacy_footprint_geometry_digest(footprint: &PhysicalFootprint) -> String {
    footprint_digest(footprint, false)
}

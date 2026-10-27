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

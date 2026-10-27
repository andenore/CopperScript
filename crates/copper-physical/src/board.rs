//! Board geometry records (`pcbir.physical`) with their dataclass validation.

use crate::geometry::Point;
use crate::mechanical::{circle_query_ring, validate_cutouts, validated_ring};
use crate::pyfloat::format_g;

pub type Result<T> = std::result::Result<T, String>;

macro_rules! text_enum {
    ($name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<$name> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

text_enum!(BoardSide { Front = "front", Back = "back" });

text_enum!(CopperLayer {
    Front = "F.Cu", In1 = "In1.Cu", In2 = "In2.Cu", In3 = "In3.Cu", In4 = "In4.Cu", In5 = "In5.Cu",
    In6 = "In6.Cu", In7 = "In7.Cu", In8 = "In8.Cu", In9 = "In9.Cu", In10 = "In10.Cu", In11 = "In11.Cu",
    In12 = "In12.Cu", In13 = "In13.Cu", In14 = "In14.Cu", In15 = "In15.Cu", In16 = "In16.Cu",
    In17 = "In17.Cu", In18 = "In18.Cu", In19 = "In19.Cu", In20 = "In20.Cu", In21 = "In21.Cu",
    In22 = "In22.Cu", In23 = "In23.Cu", In24 = "In24.Cu", In25 = "In25.Cu", In26 = "In26.Cu",
    In27 = "In27.Cu", In28 = "In28.Cu", In29 = "In29.Cu", In30 = "In30.Cu", Back = "B.Cu",
});

/// `nm_from_mm("0.01")`: the default curve chord error.
pub const DEFAULT_CHORD_ERROR_NM: i64 = 10_000;

/// `BoardCutout`: a named polygon of removed substrate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCutout {
    pub id: String,
    pub vertices: Vec<Point>,
}

impl BoardCutout {
    pub fn new(id: &str, vertices: Vec<Point>) -> Result<Self> {
        if id.trim().is_empty() {
            return Err("board cutout requires an id".to_owned());
        }
        Ok(BoardCutout { id: id.to_owned(), vertices: validated_ring(vertices)? })
    }
}

/// `BoardDatum`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardDatum {
    pub id: String,
    pub position: Point,
    pub relative_to: Option<String>,
    pub offset: Point,
}

/// `BoardEdge`: a named straight board boundary segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardEdge {
    pub id: String,
    pub start: Point,
    pub end: Point,
}

impl BoardEdge {
    pub fn new(id: &str, start: Point, end: Point) -> Result<Self> {
        if id.is_empty() || start == end {
            return Err("named edge requires an id and distinct endpoints".to_owned());
        }
        Ok(BoardEdge { id: id.to_owned(), start, end })
    }
}

/// `MechanicalHole`: a board-owned round NPTH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MechanicalHole {
    pub id: String,
    pub position: Point,
    pub diameter_nm: i64,
    pub head_clearance_radius_nm: i64,
}

impl MechanicalHole {
    pub fn new(id: &str, position: Point, diameter_nm: i64, head_clearance_radius_nm: i64) -> Result<Self> {
        if id.trim().is_empty() || diameter_nm <= 0 {
            return Err("mechanical hole requires an id and positive integer diameter".to_owned());
        }
        if head_clearance_radius_nm < 0 {
            return Err("mechanical head clearance radius must be a nonnegative integer".to_owned());
        }
        if head_clearance_radius_nm != 0 && 2 * head_clearance_radius_nm < diameter_nm {
            return Err("mechanical head clearance cannot be smaller than the hole".to_owned());
        }
        Ok(MechanicalHole { id: id.to_owned(), position, diameter_nm, head_clearance_radius_nm })
    }
}

/// `MechanicalSlot`: a non-plated capsule between tool-centre endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MechanicalSlot {
    pub id: String,
    pub start: Point,
    pub end: Point,
    pub width_nm: i64,
}

impl MechanicalSlot {
    pub fn new(id: &str, start: Point, end: Point, width_nm: i64) -> Result<Self> {
        if id.trim().is_empty() || start == end {
            return Err("routed slot requires an id and distinct typed endpoints".to_owned());
        }
        if width_nm <= 0 {
            return Err("routed slot width must be a positive integer length".to_owned());
        }
        Ok(MechanicalSlot { id: id.to_owned(), start, end, width_nm })
    }
}

/// `BoundaryLine` or `BoundaryArc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundarySegment {
    Line { id: String, start: Point, end: Point },
    Arc { id: String, start: Point, mid: Point, end: Point },
}

impl BoundarySegment {
    pub fn line(id: &str, start: Point, end: Point) -> Result<Self> {
        if id.trim().is_empty() || start == end {
            return Err("boundary line requires an id and distinct typed endpoints".to_owned());
        }
        Ok(BoundarySegment::Line { id: id.to_owned(), start, end })
    }

    pub fn arc(id: &str, start: Point, mid: Point, end: Point) -> Result<Self> {
        if id.trim().is_empty() || start == mid || mid == end || start == end {
            return Err("boundary arc requires an id and three distinct typed points".to_owned());
        }
        Ok(BoundarySegment::Arc { id: id.to_owned(), start, mid, end })
    }

    pub fn id(&self) -> &str {
        match self {
            BoundarySegment::Line { id, .. } | BoundarySegment::Arc { id, .. } => id,
        }
    }

    pub fn start(&self) -> Point {
        match self {
            BoundarySegment::Line { start, .. } | BoundarySegment::Arc { start, .. } => *start,
        }
    }

    pub fn end(&self) -> Point {
        match self {
            BoundarySegment::Line { end, .. } | BoundarySegment::Arc { end, .. } => *end,
        }
    }
}

/// `BoardBoundaryPath`: a closed, exactly connected line/arc path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardBoundaryPath {
    pub segments: Vec<BoundarySegment>,
    pub maximum_chord_error_nm: i64,
}

impl BoardBoundaryPath {
    pub fn new(segments: Vec<BoundarySegment>, maximum_chord_error_nm: i64) -> Result<Self> {
        if !(2..=256).contains(&segments.len()) {
            return Err("boundary path requires 2\u{2013}256 line/arc primitives".to_owned());
        }
        let mut ids: Vec<&str> = segments.iter().map(BoundarySegment::id).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != segments.len() {
            return Err("boundary primitive IDs must be unique".to_owned());
        }
        if maximum_chord_error_nm <= 2 {
            return Err("boundary path chord error must exceed 2 nm".to_owned());
        }
        let count = segments.len();
        if (0..count).any(|i| segments[i].end() != segments[(i + 1) % count].start()) {
            return Err("boundary path must be exactly connected and closed; no healing".to_owned());
        }
        Ok(BoardBoundaryPath { segments, maximum_chord_error_nm })
    }
}

/// `CircularBoardBoundary`: an authoritative circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CircularBoardBoundary {
    pub center: Point,
    pub radius_nm: i64,
    pub maximum_chord_error_nm: i64,
}

impl CircularBoardBoundary {
    pub fn new(center: Point, radius_nm: i64, maximum_chord_error_nm: i64) -> Result<Self> {
        if radius_nm <= 0 {
            return Err("circular board radius must be a positive integer".to_owned());
        }
        if !(2 < maximum_chord_error_nm && maximum_chord_error_nm < radius_nm) {
            return Err("circular board chord error must be >2 nm and smaller than the radius".to_owned());
        }
        Ok(CircularBoardBoundary { center, radius_nm, maximum_chord_error_nm })
    }
}

/// `BoardOutline`: the board boundary with its conservative query ring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardOutline {
    pub vertices: Vec<Point>,
    pub cutouts: Vec<BoardCutout>,
    pub circular_boundary: Option<CircularBoardBoundary>,
    pub boundary_path: Option<BoardBoundaryPath>,
}

impl BoardOutline {
    /// `BoardOutline(vertices, cutouts, circular_boundary, boundary_path)`.
    pub fn new(
        vertices: Vec<Point>,
        cutouts: Vec<BoardCutout>,
        circular_boundary: Option<CircularBoardBoundary>,
        boundary_path: Option<BoardBoundaryPath>,
    ) -> Result<Self> {
        let vertices = validated_ring(vertices)?;
        if let Some(circle) = &circular_boundary
            && vertices != circle_query_ring(circle)?
        {
            return Err("circular board query ring must match its authoritative circle".to_owned());
        }
        if let Some(path) = &boundary_path
            && (circular_boundary.is_some() || vertices != crate::curves::path_query_ring(path)?)
        {
            return Err("board query ring must match its authoritative line/arc path".to_owned());
        }
        let outline = BoardOutline { vertices, cutouts, circular_boundary, boundary_path };
        validate_cutouts(&outline)?;
        Ok(outline)
    }

    pub fn polygon(vertices: Vec<Point>) -> Result<Self> {
        BoardOutline::new(vertices, Vec::new(), None, None)
    }

    /// `dataclasses.replace(outline, cutouts=...)`.
    pub fn with_cutouts(&self, cutouts: Vec<BoardCutout>) -> Result<Self> {
        BoardOutline::new(self.vertices.clone(), cutouts, self.circular_boundary, self.boundary_path.clone())
    }

    pub fn from_path(path: BoardBoundaryPath) -> Result<Self> {
        let ring = crate::curves::path_query_ring(&path)?;
        BoardOutline::new(ring, Vec::new(), None, Some(path))
    }

    pub fn rounded_rectangle(
        width_nm: i64,
        height_nm: i64,
        corner_radius_nm: i64,
        origin: Point,
        maximum_chord_error_nm: i64,
    ) -> Result<Self> {
        let path = crate::curves::rounded_rectangle_path(
            origin,
            width_nm,
            height_nm,
            corner_radius_nm,
            maximum_chord_error_nm,
        )?;
        BoardOutline::from_path(path)
    }

    pub fn circle(diameter_nm: i64, center: Option<Point>, maximum_chord_error_nm: i64) -> Result<Self> {
        if diameter_nm <= 0 || diameter_nm % 2 != 0 {
            return Err("circular board diameter must be positive and an even number of nanometres".to_owned());
        }
        let radius = diameter_nm / 2;
        let circle =
            CircularBoardBoundary::new(center.unwrap_or(Point::new(radius, radius)), radius, maximum_chord_error_nm)?;
        let ring = circle_query_ring(&circle)?;
        BoardOutline::new(ring, Vec::new(), Some(circle), None)
    }

    pub fn rectangle(width_nm: i64, height_nm: i64, origin: Option<Point>) -> Result<Self> {
        let start = origin.unwrap_or(Point::new(0, 0));
        if width_nm <= 0 || height_nm <= 0 {
            return Err("board dimensions must be positive".to_owned());
        }
        BoardOutline::polygon(vec![
            start,
            Point::new(start.x_nm + width_nm, start.y_nm),
            Point::new(start.x_nm + width_nm, start.y_nm + height_nm),
            Point::new(start.x_nm, start.y_nm + height_nm),
        ])
    }
}

/// `PolygonRing`: at least three distinct vertices with nonzero area.
pub fn polygon_ring(vertices: &[Point]) -> Result<Vec<Point>> {
    let mut vertices = vertices.to_vec();
    if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
        vertices.pop();
    }
    let mut distinct = vertices.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if vertices.len() < 3 || distinct.len() < 3 {
        return Err("a polygon ring requires three distinct vertices".to_owned());
    }
    if crate::mechanical::signed_area_twice(&vertices) == 0 {
        return Err("a polygon ring cannot have zero area".to_owned());
    }
    Ok(vertices)
}

/// `CopperKeepout` with a hole-free outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopperKeepout {
    pub id: String,
    pub layers: Vec<CopperLayer>,
    pub vertices: Vec<Point>,
    pub block_tracks: bool,
    pub block_vias: bool,
    pub block_pads: bool,
    pub block_zones: bool,
    pub block_footprints: bool,
}

impl CopperKeepout {
    pub fn validate(&self) -> Result<()> {
        let mut layers = self.layers.clone();
        layers.sort_unstable();
        layers.dedup();
        if self.id.is_empty() || self.layers.is_empty() || layers.len() != self.layers.len() {
            return Err("a copper keepout requires an id and unique layers".to_owned());
        }
        if !(self.block_tracks || self.block_vias || self.block_pads || self.block_zones || self.block_footprints) {
            return Err("a copper keepout must block at least one object type".to_owned());
        }
        Ok(())
    }
}

/// `PlacementKeepout`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementKeepout {
    pub name: String,
    pub outline: BoardOutline,
    pub side: Option<BoardSide>,
    pub maximum_component_height_nm: Option<i64>,
}

impl PlacementKeepout {
    pub fn new(
        name: &str,
        outline: BoardOutline,
        side: Option<BoardSide>,
        maximum_component_height_nm: Option<i64>,
    ) -> Result<Self> {
        if name.is_empty() {
            return Err("placement keepout name cannot be empty".to_owned());
        }
        if !outline.cutouts.is_empty() {
            return Err("placement keepout outlines do not support cutouts".to_owned());
        }
        if outline.circular_boundary.is_some() {
            return Err("placement keepout outlines do not support curved boundaries".to_owned());
        }
        if maximum_component_height_nm.is_some_and(|height| height < 0) {
            return Err("keepout maximum component height cannot be negative".to_owned());
        }
        Ok(PlacementKeepout { name: name.to_owned(), outline, side, maximum_component_height_nm })
    }
}

/// `DesignRules`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesignRules {
    pub minimum_clearance_nm: i64,
    pub minimum_hole_clearance_nm: i64,
    pub minimum_track_width_nm: i64,
    pub default_track_width_nm: i64,
    pub default_via_size_nm: i64,
    pub default_via_drill_nm: i64,
    pub minimum_slot_width_nm: i64,
    pub minimum_via_size_nm: Option<i64>,
    pub minimum_via_drill_nm: Option<i64>,
}

impl Default for DesignRules {
    fn default() -> Self {
        DesignRules {
            minimum_clearance_nm: 200_000,
            minimum_hole_clearance_nm: 250_000,
            minimum_track_width_nm: 200_000,
            default_track_width_nm: 250_000,
            default_via_size_nm: 800_000,
            default_via_drill_nm: 400_000,
            minimum_slot_width_nm: 1_000_000,
            minimum_via_size_nm: None,
            minimum_via_drill_nm: None,
        }
    }
}

impl DesignRules {
    /// The rule field names, without their `_nm` suffix, in declaration order.
    pub const FIELDS: [&'static str; 9] = [
        "minimum_clearance",
        "minimum_hole_clearance",
        "minimum_track_width",
        "default_track_width",
        "default_via_size",
        "default_via_drill",
        "minimum_slot_width",
        "minimum_via_size",
        "minimum_via_drill",
    ];

    /// `DesignRules(**overrides)` for `(field_nm, value)` pairs.
    pub fn with_overrides<'a>(overrides: impl IntoIterator<Item = (&'a str, i64)>) -> Result<Self> {
        let mut rules = DesignRules::default();
        for (name, value) in overrides {
            match name {
                "minimum_clearance_nm" => rules.minimum_clearance_nm = value,
                "minimum_hole_clearance_nm" => rules.minimum_hole_clearance_nm = value,
                "minimum_track_width_nm" => rules.minimum_track_width_nm = value,
                "default_track_width_nm" => rules.default_track_width_nm = value,
                "default_via_size_nm" => rules.default_via_size_nm = value,
                "default_via_drill_nm" => rules.default_via_drill_nm = value,
                "minimum_slot_width_nm" => rules.minimum_slot_width_nm = value,
                "minimum_via_size_nm" => rules.minimum_via_size_nm = Some(value),
                "minimum_via_drill_nm" => rules.minimum_via_drill_nm = Some(value),
                other => return Err(format!("DesignRules.__init__() got an unexpected keyword argument '{other}'")),
            }
        }
        rules.validate()?;
        Ok(rules)
    }

    pub fn validate(&self) -> Result<()> {
        let values = [
            self.minimum_clearance_nm,
            self.minimum_hole_clearance_nm,
            self.minimum_track_width_nm,
            self.default_track_width_nm,
            self.default_via_size_nm,
            self.default_via_drill_nm,
            self.minimum_slot_width_nm,
        ];
        if values.iter().any(|&value| value <= 0) {
            return Err("physical design rules must be positive".to_owned());
        }
        if self.minimum_track_width_nm > self.default_track_width_nm {
            return Err("minimum track width cannot exceed default track width".to_owned());
        }
        if self.default_via_drill_nm >= self.default_via_size_nm {
            return Err("default via drill must be smaller than via size".to_owned());
        }
        if [self.minimum_via_size_nm, self.minimum_via_drill_nm].iter().any(|value| value.is_some_and(|v| v <= 0)) {
            return Err("physical design rules must be positive".to_owned());
        }
        let mm = |value: i64| format_g(value as f64 / 1_000_000.0);
        if let Some(minimum) = self.minimum_via_size_nm
            && self.default_via_size_nm < minimum
        {
            return Err(format!(
                "default via size {} mm is below the minimum via size {} mm",
                mm(self.default_via_size_nm),
                mm(minimum)
            ));
        }
        if let Some(minimum) = self.minimum_via_drill_nm
            && self.default_via_drill_nm < minimum
        {
            return Err(format!(
                "default via drill {} mm is below the minimum via drill {} mm",
                mm(self.default_via_drill_nm),
                mm(minimum)
            ));
        }
        Ok(())
    }
}

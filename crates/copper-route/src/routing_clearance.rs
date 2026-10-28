//! Incremental exact-copper clearance queries for the detailed router
//! (`pcbir.routing_clearance`).
//!
//! The grid guides a search; it is not a substitute for checking the copper
//! that will actually be written. This index uses the same placed-pad shapes
//! and integer geometry predicates as physical DRC, with a coarse spatial
//! hash only as a broad phase.

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_drc::breakout::BreakoutRegions;
use copper_drc::legality::shape_in_board;
use copper_json::{Dumps, Value as Json};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, TrackSegment, Via};
use copper_pcb::hard_macros::macro_reservations;
use copper_pcb::keepouts::resolved_copper_keepouts;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::zones::ZoneRoutingReservations;
use copper_pcb::{Error, Result};

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{Bounds, shapes_clear};
use copper_physical::{BoardOutline, BoardSide, BoundarySegment, CopperLayer, Point, RoundedConvexShape};

use crate::surface_path::track_inside_board;
use crate::terminal_preferences::TerminalPreferences;

/// `_CopperObject`: one indexed copper shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopperObject {
    pub net: String,
    pub layers: Vec<CopperLayer>,
    pub shape: RoundedConvexShape,
    pub clearance_nm: i64,
    pub locked: bool,
    pub is_pad: bool,
    pub pad_reference: Option<PadReference>,
    /// Terminal land whose breakout region holds this copper (plan R1).
    pub breakout_land: Option<String>,
    pub occurrence: String,
}

impl CopperObject {
    fn new(net: &str, layers: Vec<CopperLayer>, shape: RoundedConvexShape) -> Self {
        CopperObject {
            net: net.to_owned(),
            layers,
            shape,
            clearance_nm: 0,
            locked: true,
            is_pad: false,
            pad_reference: None,
            breakout_land: None,
            occurrence: String::new(),
        }
    }
}

/// `_KeepoutObject`: a static region that blocks tracks and/or vias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepoutObject {
    pub layers: Vec<CopperLayer>,
    pub shape: RoundedConvexShape,
    pub block_tracks: bool,
    pub block_vias: bool,
    pub has_holes: bool,
    pub clearance_nm: i64,
}

/// `_DrilledHole`: a plated or non-plated drill centre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrilledHole {
    pub net: String,
    pub position: Point,
    pub radius_nm: i64,
    pub locked: bool,
    pub reusable: bool,
    pub span: Option<(CopperLayer, CopperLayer)>,
    pub occurrence: String,
}

/// An `additions()` entry: a track or via with its lock flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Copper {
    Track(TrackSegment),
    Via(Via),
}

impl Copper {
    pub fn net(&self) -> &str {
        match self {
            Copper::Track(track) => &track.net,
            Copper::Via(via) => &via.net,
        }
    }
}

/// `_Addition`: one `additions()` entry and the index slots it created.
#[derive(Clone, Debug)]
struct Addition {
    serial: u64,
    entry: (Copper, bool),
    kind: &'static str,
    /// Default labels name the log position and follow it on removal.
    default_occurrence: bool,
    objects: Vec<usize>,
    hole: Option<usize>,
}

/// `RoutingBlocker`: snapshot-bound exact blocker evidence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutingBlocker {
    pub identity: String,
    pub reason: String,
    pub net: Option<String>,
    pub immutable: bool,
    pub layers: Vec<String>,
    pub positions_nm: Vec<(i64, i64)>,
    /// Snapshot-local occurrence, distinct even for geometrically equal objects.
    pub occurrence: String,
}

fn j_int(value: i64) -> Json {
    Json::Int(value.into())
}

fn j_str(text: &str) -> Json {
    Json::Str(text.to_owned())
}

fn j_point(point: Point) -> Json {
    Json::Object(IndexMap::from([("x_nm".to_owned(), j_int(point.x_nm)), ("y_nm".to_owned(), j_int(point.y_nm))]))
}

fn j_shape(shape: &RoundedConvexShape) -> Json {
    let bounds = shape.bounds;
    Json::Object(IndexMap::from([
        ("spine".to_owned(), Json::Array(shape.spine.iter().map(|p| j_point(*p)).collect())),
        ("radius_nm".to_owned(), j_int(shape.radius_nm)),
        (
            "_bounds".to_owned(),
            Json::Object(IndexMap::from([
                ("min_x".to_owned(), j_int(bounds.min_x)),
                ("min_y".to_owned(), j_int(bounds.min_y)),
                ("max_x".to_owned(), j_int(bounds.max_x)),
                ("max_y".to_owned(), j_int(bounds.max_y)),
            ])),
        ),
    ]))
}

fn j_layers(layers: &[CopperLayer]) -> Json {
    Json::Array(layers.iter().map(|l| j_str(l.value())).collect())
}

/// A frozenset's canonical form: members sorted by their compact JSON text.
fn j_layer_set(layers: &[CopperLayer]) -> Json {
    let mut items: Vec<(String, Json)> =
        layers.iter().map(|l| (Dumps::compact().sort_keys(true).dumps(&j_str(l.value())), j_str(l.value()))).collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    Json::Array(items.into_iter().map(|(_, value)| value).collect())
}

/// `_geometry_identity`: sha256 of the canonical JSON of a record.
fn geometry_identity(value: &Json) -> String {
    let text = Dumps::compact().sort_keys(true).dumps(value);
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn keepout_identity(region: &KeepoutObject) -> String {
    geometry_identity(&Json::Object(IndexMap::from([
        ("layers".to_owned(), j_layer_set(&region.layers)),
        ("shape".to_owned(), j_shape(&region.shape)),
        ("block_tracks".to_owned(), Json::Bool(region.block_tracks)),
        ("block_vias".to_owned(), Json::Bool(region.block_vias)),
        ("has_holes".to_owned(), Json::Bool(region.has_holes)),
        ("clearance_nm".to_owned(), j_int(region.clearance_nm)),
    ])))
}

fn copper_identity(item: &CopperObject) -> String {
    geometry_identity(&Json::Object(IndexMap::from([
        ("net".to_owned(), j_str(&item.net)),
        ("layers".to_owned(), j_layers(&item.layers)),
        ("shape".to_owned(), j_shape(&item.shape)),
        ("clearance_nm".to_owned(), j_int(item.clearance_nm)),
        ("locked".to_owned(), Json::Bool(item.locked)),
        ("is_pad".to_owned(), Json::Bool(item.is_pad)),
        (
            "pad_reference".to_owned(),
            match &item.pad_reference {
                None => Json::Null,
                Some(pad) => Json::Object(IndexMap::from([
                    ("component".to_owned(), j_str(&pad.component)),
                    ("pad".to_owned(), j_str(&pad.pad)),
                ])),
            },
        ),
        ("breakout_land".to_owned(), item.breakout_land.as_deref().map_or(Json::Null, j_str)),
        ("occurrence".to_owned(), j_str(&item.occurrence)),
    ])))
}

fn hole_identity(hole: &DrilledHole) -> String {
    geometry_identity(&Json::Object(IndexMap::from([
        ("net".to_owned(), j_str(&hole.net)),
        ("position".to_owned(), j_point(hole.position)),
        ("radius_nm".to_owned(), j_int(hole.radius_nm)),
        ("locked".to_owned(), Json::Bool(hole.locked)),
        ("reusable".to_owned(), Json::Bool(hole.reusable)),
        ("span".to_owned(), hole.span.map_or(Json::Null, |(a, b)| j_layers(&[a, b]))),
        ("occurrence".to_owned(), j_str(&hole.occurrence)),
    ])))
}

fn outline_identity(outline: &BoardOutline) -> String {
    let segment = |segment: &BoundarySegment| match segment {
        BoundarySegment::Line { id, start, end } => Json::Object(IndexMap::from([
            ("id".to_owned(), j_str(id)),
            ("start".to_owned(), j_point(*start)),
            ("end".to_owned(), j_point(*end)),
        ])),
        BoundarySegment::Arc { id, start, mid, end } => Json::Object(IndexMap::from([
            ("id".to_owned(), j_str(id)),
            ("start".to_owned(), j_point(*start)),
            ("mid".to_owned(), j_point(*mid)),
            ("end".to_owned(), j_point(*end)),
        ])),
    };
    geometry_identity(&Json::Object(IndexMap::from([
        ("vertices".to_owned(), Json::Array(outline.vertices.iter().map(|p| j_point(*p)).collect())),
        (
            "cutouts".to_owned(),
            Json::Array(
                outline
                    .cutouts
                    .iter()
                    .map(|c| {
                        Json::Object(IndexMap::from([
                            ("id".to_owned(), j_str(&c.id)),
                            ("vertices".to_owned(), Json::Array(c.vertices.iter().map(|p| j_point(*p)).collect())),
                        ]))
                    })
                    .collect(),
            ),
        ),
        (
            "circular_boundary".to_owned(),
            outline.circular_boundary.as_ref().map_or(Json::Null, |circle| {
                Json::Object(IndexMap::from([
                    ("center".to_owned(), j_point(circle.center)),
                    ("radius_nm".to_owned(), j_int(circle.radius_nm)),
                    ("maximum_chord_error_nm".to_owned(), j_int(circle.maximum_chord_error_nm)),
                ]))
            }),
        ),
        (
            "boundary_path".to_owned(),
            outline.boundary_path.as_ref().map_or(Json::Null, |path| {
                Json::Object(IndexMap::from([
                    ("segments".to_owned(), Json::Array(path.segments.iter().map(segment).collect())),
                    ("maximum_chord_error_nm".to_owned(), j_int(path.maximum_chord_error_nm)),
                ]))
            }),
        ),
    ])))
}

type BinKey = (CopperLayer, i64, i64);

/// `RoutingClearanceIndex`: reserve copper; same-net tracks may join, but vias must avoid pads.
pub struct RoutingClearanceIndex<'b> {
    pub board: &'b PhysicalBoard,
    pub terminal_preferences: TerminalPreferences,
    pub zone_reservations: ZoneRoutingReservations,
    pub bin_size_nm: i64,
    pub rules: IndexMap<String, NetRoutingRule>,
    /// Breakout regions relax width, gap and clearance next to terminal lands.
    pub breakout: BreakoutRegions,
    /// `pair_search._PairAccessIndex`: local pair paths emit exactly the
    /// differential profile's width pieces and only octilinear legs.
    /// PENDING: owned by rust-m4-critical (a subclass in Python).
    pub pair_access: bool,
    macro_regions: Vec<(Vec<CopperLayer>, RoundedConvexShape)>,
    // Bins hold stable slot ids; `remove` leaves `None` in a slot until compaction.
    slots: Vec<Option<CopperObject>>,
    hole_slots: Vec<Option<DrilledHole>>,
    dead_slots: usize,
    dead_holes: usize,
    // Drill centres binned by position (layer independent, as drills are).
    hole_bins: IndexMap<(i64, i64), Vec<usize>>,
    // Never decreases, so it bounds every live drill for the broad phase.
    max_hole_radius_nm: i64,
    // Ordered insertions; with `board` they determine the whole index.
    additions: Vec<(Copper, bool)>,
    records: Vec<Addition>,
    next_serial: u64,
    // Unlocked additions per net, in insertion order (`remove` owners).
    owned: IndexMap<String, Vec<u64>>,
    bins: IndexMap<BinKey, Vec<usize>>,
    keepouts: Vec<KeepoutObject>,
    static_regions: Vec<KeepoutObject>,
    static_layer_bounds: IndexMap<CopperLayer, Bounds>,
    static_reach_nm: i64,
    static_bins: IndexMap<BinKey, Vec<usize>>,
    static_large: IndexMap<CopperLayer, Vec<usize>>,
    static_by_layer: IndexMap<CopperLayer, Vec<usize>>,
    static_hole_track_layers: IndexSet<CopperLayer>,
    static_hole_via_layers: IndexSet<CopperLayer>,
    max_clearance_nm: i64,
    net_clearance_nm: IndexMap<String, i64>,
    breakout_active: bool,
    spacing_bound_nm: i64,
}

const STATIC_BIN_LIMIT: i64 = 512;
// Tombstones tolerated before `remove` compacts slot ids (amortized).
const COMPACT_MIN_DEAD: usize = 4096;

/// Python's `range(a // size, b // size + 1)` for a closed box.
fn cells(low: i64, high: i64, size: i64) -> std::ops::RangeInclusive<i64> {
    low.div_euclid(size)..=high.div_euclid(size)
}

fn range_len(range: &std::ops::RangeInclusive<i64>) -> i64 {
    (range.end() - range.start() + 1).max(0)
}

fn shape(spine: Vec<Point>, radius_nm: i64) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(spine, radius_nm)?)
}

impl<'b> RoutingClearanceIndex<'b> {
    pub fn new(board: &'b PhysicalBoard) -> Result<Self> {
        Self::with_bin_size(board, crate::nm_from_mm(2), &IndexSet::new())
    }

    /// `RoutingClearanceIndex(board, bin_size_nm, _deferred_components=...)`.
    pub fn with_bin_size(
        board: &'b PhysicalBoard,
        bin_size_nm: i64,
        deferred_components: &IndexSet<String>,
    ) -> Result<Self> {
        if bin_size_nm <= 0 {
            return fail("clearance bin size must be positive");
        }
        let rules: IndexMap<String, NetRoutingRule> =
            board.net_routing_rules.iter().map(|r| (r.net.clone(), r.clone())).collect();
        let breakout = BreakoutRegions::new(board);
        let mut macro_regions = Vec::new();
        for region in macro_reservations(board)? {
            macro_regions.push((region.layers.clone(), shape(region.vertices.clone(), 0)?));
        }
        let mut keepouts = Vec::new();
        for item in resolved_copper_keepouts(board)? {
            keepouts.push(KeepoutObject {
                layers: item.layers.clone(),
                shape: shape(item.vertices.clone(), 0)?,
                block_tracks: item.block_tracks,
                block_vias: item.block_vias,
                has_holes: false,
                clearance_nm: 0,
            });
        }
        let mut max_clearance_nm = board.rules.minimum_clearance_nm.max(board.rules.minimum_hole_clearance_nm);
        for rule in rules.values() {
            max_clearance_nm = max_clearance_nm.max(rule.clearance_nm.unwrap_or(0));
            // A pair with breakout properties is spaced by its gap, not clearance.
            if rule.breakout_length_nm.is_some() {
                max_clearance_nm = max_clearance_nm.max(rule.pair_gap_nm.unwrap_or(0));
            }
        }
        for footprint in board.footprints.values() {
            max_clearance_nm = max_clearance_nm.max(footprint.clearance_nm.unwrap_or(0));
        }
        let minimum = board.rules.minimum_clearance_nm;
        let net_clearance_nm =
            rules.iter().map(|(net, rule)| (net.clone(), minimum.max(rule.clearance_nm.unwrap_or(0)))).collect();
        let mut spacing_bound_nm = rules.values().map(|rule| rule.pair_gap_nm.unwrap_or(0)).max().unwrap_or(0);
        spacing_bound_nm = spacing_bound_nm.max(max_clearance_nm);
        let mut index = RoutingClearanceIndex {
            board,
            terminal_preferences: TerminalPreferences::new(board),
            zone_reservations: ZoneRoutingReservations::new(board),
            bin_size_nm,
            breakout_active: breakout.any(),
            rules,
            breakout,
            pair_access: false,
            macro_regions,
            slots: Vec::new(),
            hole_slots: Vec::new(),
            dead_slots: 0,
            dead_holes: 0,
            hole_bins: IndexMap::new(),
            max_hole_radius_nm: 0,
            additions: Vec::new(),
            records: Vec::new(),
            next_serial: 0,
            owned: IndexMap::new(),
            bins: IndexMap::new(),
            keepouts,
            static_regions: Vec::new(),
            static_layer_bounds: IndexMap::new(),
            static_reach_nm: 1,
            static_bins: IndexMap::new(),
            static_large: IndexMap::new(),
            static_by_layer: IndexMap::new(),
            static_hole_track_layers: IndexSet::new(),
            static_hole_via_layers: IndexSet::new(),
            max_clearance_nm,
            net_clearance_nm,
            spacing_bound_nm,
        };
        index.index_static_regions();
        let (objects, holes) = index.pad_template()?;
        if !deferred_components.is_empty() {
            // Private support-placement proposal view. The caller must restore
            // every land and validate the complete new pose before adoption.
            let poses: IndexMap<&str, _> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
            let known = deferred_components.iter().all(|reference| poses.contains_key(reference.as_str()));
            let smd_only = known
                && deferred_components.iter().all(|reference| {
                    board.footprints[&poses[reference.as_str()].footprint]
                        .pads
                        .iter()
                        .all(|pad| pad.kind == PadKind::Smd)
                });
            if !smd_only {
                return fail("deferred placement view requires known SMD-only components");
            }
            for object in objects {
                let deferred = object
                    .pad_reference
                    .as_ref()
                    .is_some_and(|reference| deferred_components.contains(&reference.component));
                if !deferred {
                    index.add(object);
                }
            }
        } else {
            for object in objects {
                index.add_without_breakout(object);
            }
        }
        for hole in holes {
            index.add_hole(hole);
        }
        for (_, hole) in copper_drc::non_plated_holes(board) {
            let mut object = CopperObject::new("<non-plated-hole>", board.stackup.copper_layers.clone(), hole);
            object.clearance_nm = board.rules.minimum_hole_clearance_nm;
            index.add(object);
        }
        for (ordinal, track) in board.tracks.iter().enumerate() {
            index.add_track(track.clone(), true, Some(format!("track:{ordinal}")))?;
        }
        for (ordinal, via) in board.vias.iter().enumerate() {
            index.add_via(via.clone(), true, Some(format!("via:{ordinal}")))?;
        }
        Ok(index)
    }

    /// `_pad_index_template`: the immutable pad broad phase (objects and drilled holes).
    fn pad_template(&self) -> Result<(Vec<CopperObject>, Vec<DrilledHole>)> {
        let board = self.board;
        let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
        for net in &board.nets {
            for pad in &net.pads {
                assigned.insert(pad, &net.name);
            }
        }
        let lengths: IndexMap<&str, i64> = board
            .net_routing_rules
            .iter()
            .filter_map(|rule| rule.breakout_length_nm.map(|length| (rule.net.as_str(), length)))
            .collect();
        let mut placements: Vec<_> = board.placements.iter().collect();
        placements.sort_by(|a, b| a.reference.cmp(&b.reference));
        let (mut objects, mut holes) = (Vec::new(), Vec::new());
        for pose in placements {
            let footprint = &board.footprints[&pose.footprint];
            for (pad_index, pad) in footprint.pads.iter().enumerate() {
                if matches!(pad.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
                    continue;
                }
                let position = transformed_local_point(pose, pad.position);
                let pad_shape = placed_pad_shape(position, pad, pose);
                let reference = PadReference::new(&pose.reference, &pad.number);
                let net = match assigned.get(&reference) {
                    Some(net) => (*net).to_owned(),
                    None => format!("<unconnected:{}.{}:{pad_index}>", pose.reference, pad.number),
                };
                let layers = if pad.kind == PadKind::Smd {
                    vec![if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
                } else {
                    board.stackup.copper_layers.clone()
                };
                let occurrence = format!("pad:{}.{}:{pad_index}", pose.reference, pad.number);
                if pad.kind == PadKind::ThroughHole
                    && let Some(drill) = pad.drill
                {
                    holes.push(DrilledHole {
                        net: net.clone(),
                        position,
                        radius_nm: drill.width_nm.max(drill.height_nm).div_euclid(2),
                        locked: true,
                        reusable: false,
                        span: None,
                        occurrence: occurrence.clone(),
                    });
                }
                let breakout_land = if assigned.contains_key(&reference) && lengths.contains_key(net.as_str()) {
                    self.breakout.region(&net, &pad_shape.spine)
                } else {
                    None
                };
                objects.push(CopperObject {
                    net,
                    layers,
                    shape: pad_shape,
                    clearance_nm: footprint.clearance_nm.unwrap_or(0),
                    locked: true,
                    is_pad: true,
                    pad_reference: Some(reference),
                    breakout_land,
                    occurrence,
                });
            }
        }
        Ok((objects, holes))
    }

    pub fn can_track(&self, net: &str, start: Point, end: Point, width_nm: i64, layer: CopperLayer) -> Result<bool> {
        let shape = shape(vec![start, end], width_nm.div_euclid(2))?;
        Ok(self.zone_reservations.can_area(net, &shape, layer)
            && self.keepout_clear(&shape, &[layer], false)?
            && self.clear(net, &shape, &[layer], 0))
    }

    /// `route_pieces`: prefer the requested width; use permitted neckdowns only if needed.
    ///
    /// This selects copper, not permission to commit it: callers still check
    /// every returned piece.
    pub fn route_pieces(
        &self,
        net: &str,
        start: Point,
        end: Point,
        width_nm: i64,
        layer: CopperLayer,
    ) -> Result<Vec<TrackSegment>> {
        if self.pair_access {
            let tracks = vec![TrackSegment { net: net.to_owned(), start, end, width_nm, layer }];
            return Ok(self.breakout.split_tracks(&tracks, Some(width_nm)));
        }
        if !self.breakout.ordinary.contains(net) {
            let tracks = vec![TrackSegment { net: net.to_owned(), start, end, width_nm, layer }];
            // Exact local/global terminal access uses the authored critical
            // breakout widths too. Ordinary nets retain preferred-width trials.
            return Ok(if self.breakout.declared.contains(net) {
                self.breakout.split_tracks(&tracks, Some(width_nm))
            } else {
                tracks
            });
        }
        let fits = |first: Point, second: Point| -> Result<bool> {
            Ok(self.can_track(net, first, second, width_nm, layer)?
                && track_inside_board(self.board, first, second, width_nm)?)
        };
        if fits(start, end)? {
            return Ok(vec![TrackSegment { net: net.to_owned(), start, end, width_nm, layer }]);
        }
        let mut result = Vec::new();
        for piece in self.breakout.ordinary_pieces(net, start, end, width_nm, layer) {
            if piece.width_nm < width_nm && fits(piece.start, piece.end)? {
                result.push(TrackSegment { width_nm, ..piece });
            } else {
                result.push(piece);
            }
        }
        Ok(result)
    }

    /// `can_route`: `can_track` for each piece ordinary routing emits.
    pub fn can_route(&self, net: &str, start: Point, end: Point, width_nm: i64, layer: CopperLayer) -> Result<bool> {
        if self.pair_access {
            let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
            if dx != 0 && dy != 0 && dx.abs() != dy.abs() {
                return Ok(false);
            }
            for piece in self.route_pieces(net, start, end, width_nm, layer)? {
                if !self.can_track(net, piece.start, piece.end, piece.width_nm, layer)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if !self.breakout.declared.contains(net) {
            return self.can_track(net, start, end, width_nm, layer);
        }
        for piece in self.route_pieces(net, start, end, width_nm, layer)? {
            if !self.can_track(net, piece.start, piece.end, piece.width_nm, layer)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `can_area`: like `can_track` for any swept shape, such as a whole tuning bump.
    pub fn can_area(&self, net: &str, shape: &RoundedConvexShape, layer: CopperLayer) -> Result<bool> {
        Ok(self.zone_reservations.can_area(net, shape, layer)
            && self.keepout_clear(shape, &[layer], false)?
            && self.clear(net, shape, &[layer], 0))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn can_via(
        &self,
        net: &str,
        position: Point,
        size_nm: i64,
        from_layer: CopperLayer,
        to_layer: CopperLayer,
        drill_nm: Option<i64>,
        allow_pad_overlap: bool,
        allowed_pad: Option<&PadReference>,
    ) -> Result<bool> {
        let layers = self.via_layers(from_layer, to_layer);
        let annulus = shape(vec![position], size_nm.div_euclid(2))?;
        if !self.keepout_clear(&annulus, &layers, true)? {
            return Ok(false);
        }
        let pads_clear = match allowed_pad {
            Some(pad) => self.pad_copper_clear(&annulus, &layers, Some(pad)),
            None => allow_pad_overlap || self.pad_copper_clear(&annulus, &layers, None),
        };
        if !pads_clear || !self.clear(net, &annulus, &layers, 0) {
            return Ok(false);
        }
        // All plated drills must clear foreign copper, not only filled/capped contacts.
        let drill = drill_nm.unwrap_or(self.board.rules.default_via_drill_nm);
        let drill_shape = shape(vec![position], drill.div_euclid(2))?;
        Ok(self.clear(net, &drill_shape, &layers, self.board.rules.minimum_hole_clearance_nm)
            && self.hole_clear(net, position, drill.div_euclid(2), (from_layer, to_layer)))
    }

    /// `candidate_vias_clear`: a net's whole tentative via set coexists.
    pub fn candidate_vias_clear(&self, vias: &[Via]) -> Result<bool> {
        Ok(self.candidate_via_conflict(vias, false)?.is_none())
    }

    /// `candidate_via_conflict`: the first tentative via that cannot coexist with its peers.
    pub fn candidate_via_conflict(&self, vias: &[Via], allow_movable_conflicts: bool) -> Result<Option<Via>> {
        for (index, via) in vias.iter().enumerate() {
            let legal = if allow_movable_conflicts {
                !self.blocking_via_nets(via)?.1
            } else {
                self.can_via(
                    &via.net,
                    via.position,
                    via.size_nm,
                    via.from_layer,
                    via.to_layer,
                    Some(via.drill_nm),
                    false,
                    None,
                )?
            };
            if !legal {
                return Ok(Some(via.clone()));
            }
            for earlier in &vias[..index] {
                let dx = i128::from(via.position.x_nm - earlier.position.x_nm);
                let dy = i128::from(via.position.y_nm - earlier.position.y_nm);
                let distance_squared = dx * dx + dy * dy;
                let hole = self.board.rules.minimum_hole_clearance_nm;
                let mut required = via.drill_nm.div_euclid(2) + earlier.drill_nm.div_euclid(2) + hole;
                let via_layers = self.via_layers(via.from_layer, via.to_layer);
                let earlier_layers = self.via_layers(earlier.from_layer, earlier.to_layer);
                if via.net != earlier.net && via_layers.iter().any(|layer| earlier_layers.contains(layer)) {
                    let earlier_shape = shape(vec![earlier.position], earlier.size_nm.div_euclid(2))?;
                    let mut earlier_copper =
                        CopperObject::new(&earlier.net, earlier_layers.clone(), earlier_shape.clone());
                    earlier_copper.breakout_land = self.breakout_land(&earlier.net, &earlier_shape);
                    let via_shape = shape(vec![via.position], via.size_nm.div_euclid(2))?;
                    let clearance = self.rule_clearance(
                        &via.net,
                        self.breakout_land(&via.net, &via_shape).as_deref(),
                        &earlier_copper,
                    );
                    required = required
                        .max(via.drill_nm.div_euclid(2) + earlier.size_nm.div_euclid(2) + hole)
                        .max(earlier.drill_nm.div_euclid(2) + via.size_nm.div_euclid(2) + hole)
                        .max(via.size_nm.div_euclid(2) + earlier.size_nm.div_euclid(2) + clearance);
                }
                if distance_squared < i128::from(required) * i128::from(required) {
                    return Ok(Some(via.clone()));
                }
            }
        }
        Ok(None)
    }

    /// `pad_copper_clear`: no pad contact, independent of net (including the full annulus).
    pub fn pad_copper_clear(
        &self,
        shape: &RoundedConvexShape,
        layers: &[CopperLayer],
        allowed_pad: Option<&PadReference>,
    ) -> bool {
        // Include same-net and unassigned pads, and the complete annulus rather
        // than only the drill/center. A one-nanometre separation rejects contact too.
        self.overlapping_objects(shape, layers).iter().all(|other| {
            !other.is_pad
                || allowed_pad.is_some_and(|pad| other.pad_reference.as_ref() == Some(pad))
                || shapes_clear(shape, &other.shape, 1)
        })
    }

    fn hole_clear(&self, net: &str, position: Point, radius_nm: i64, span: (CopperLayer, CopperLayer)) -> bool {
        let hole_clearance = self.board.rules.minimum_hole_clearance_nm;
        let reach = radius_nm + self.max_hole_radius_nm + hole_clearance;
        for hole in self.nearby_holes(position, reach) {
            let dx = i128::from(position.x_nm - hole.position.x_nm);
            let dy = i128::from(position.y_nm - hole.position.y_nm);
            let distance_squared = dx * dx + dy * dy;
            if distance_squared == 0 && hole.net == net && hole.reusable && hole.span == Some(span) {
                continue; // Reuse an existing same-net through via, not a new drill.
            }
            let required = i128::from(radius_nm + hole.radius_nm + hole_clearance);
            if distance_squared < required * required {
                return false;
            }
        }
        true
    }

    fn keepout_clear(&self, shape: &RoundedConvexShape, layers: &[CopperLayer], for_via: bool) -> Result<bool> {
        // Every outline, plain polygons included: the detailed grid spans the
        // outline's bounds inclusively, so edge nodes would otherwise admit
        // copper the exported copper-to-edge clearance rejects.
        if !shape_in_board(
            self.board,
            shape,
            self.board.rules.minimum_clearance_nm,
            self.board.rules.minimum_hole_clearance_nm,
        )? {
            return Ok(false); // Always locked, including speculative soft rip-up.
        }
        if self.static_regions.is_empty() {
            return Ok(true);
        }
        // Unsupported keepout holes fail closed even far from the query.
        let blocked_layers = if for_via { &self.static_hole_via_layers } else { &self.static_hole_track_layers };
        if layers.iter().any(|layer| blocked_layers.contains(layer)) {
            return Ok(false);
        }
        // Quick reject: nothing static on these layers reaches the query.
        let reach = self.static_reach_nm;
        let bounds = shape.bounds;
        if !layers.iter().any(|layer| {
            self.static_layer_bounds.get(layer).is_some_and(|known| bounds.intersects_expanded(*known, reach))
        }) {
            return Ok(true);
        }
        for keepout in self.overlapping_static_regions(shape, layers) {
            if !(if for_via { keepout.block_vias } else { keepout.block_tracks }) {
                continue;
            }
            let area =
                if keepout.clearance_nm != 0 { shape.bounds.expanded(keepout.clearance_nm) } else { shape.bounds };
            if !area.intersects(keepout.shape.bounds) {
                continue;
            }
            if !shapes_clear(shape, &keepout.shape, keepout.clearance_nm) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `_index_static_regions`: a snapshot-local broad phase, separate from mutable copper.
    fn index_static_regions(&mut self) {
        let mut regions: Vec<KeepoutObject> = self
            .macro_regions
            .iter()
            .map(|(layers, shape)| KeepoutObject {
                layers: layers.clone(),
                shape: shape.clone(),
                block_tracks: true,
                block_vias: true,
                has_holes: false,
                clearance_nm: 1,
            })
            .collect();
        regions.extend(self.keepouts.iter().filter(|region| region.block_tracks || region.block_vias).cloned());
        let mut bins: IndexMap<BinKey, Vec<usize>> = IndexMap::new();
        let mut large: IndexMap<CopperLayer, Vec<usize>> = IndexMap::new();
        let mut by_layer: IndexMap<CopperLayer, Vec<usize>> = IndexMap::new();
        let mut hole_tracks = IndexSet::new();
        let mut hole_vias = IndexSet::new();
        let mut layer_bounds: IndexMap<CopperLayer, Bounds> = IndexMap::new();
        let mut reach = 1; // macro reservations reject exact contact (one nm margin)
        for (identity, region) in regions.iter().enumerate() {
            let area = region.shape.bounds;
            let (xs, ys) = self.static_cells(area);
            reach = reach.max(region.clearance_nm + 1);
            for layer in &region.layers {
                by_layer.entry(*layer).or_default().push(identity);
                let merged = match layer_bounds.get(layer) {
                    None => area,
                    Some(known) => Bounds {
                        min_x: known.min_x.min(area.min_x),
                        min_y: known.min_y.min(area.min_y),
                        max_x: known.max_x.max(area.max_x),
                        max_y: known.max_y.max(area.max_y),
                    },
                };
                layer_bounds.insert(*layer, merged);
                if region.has_holes {
                    if region.block_tracks {
                        hole_tracks.insert(*layer);
                    }
                    if region.block_vias {
                        hole_vias.insert(*layer);
                    }
                }
                if range_len(&xs) * range_len(&ys) > STATIC_BIN_LIMIT {
                    large.entry(*layer).or_default().push(identity);
                } else {
                    for x in xs.clone() {
                        for y in ys.clone() {
                            bins.entry((*layer, x, y)).or_default().push(identity);
                        }
                    }
                }
            }
        }
        self.static_regions = regions;
        self.static_layer_bounds = layer_bounds;
        self.static_reach_nm = reach;
        self.static_bins = bins;
        self.static_large = large;
        self.static_by_layer = by_layer;
        self.static_hole_track_layers = hole_tracks;
        self.static_hole_via_layers = hole_vias;
    }

    fn static_cells(&self, area: Bounds) -> (std::ops::RangeInclusive<i64>, std::ops::RangeInclusive<i64>) {
        (cells(area.min_x, area.max_x, self.bin_size_nm), cells(area.min_y, area.max_y, self.bin_size_nm))
    }

    fn overlapping_static_regions(&self, shape: &RoundedConvexShape, layers: &[CopperLayer]) -> Vec<&KeepoutObject> {
        // Macro access reservations reject even exact contact (one nm margin).
        // Expand the QUERY so an adjacent-bin contact cannot be lost.
        if !layers.iter().any(|layer| self.static_by_layer.contains_key(layer)) {
            return Vec::new();
        }
        let (xs, ys) = self.static_cells(shape.bounds.expanded(1));
        let mut identities: IndexSet<usize> = IndexSet::new();
        for layer in layers {
            if !self.static_by_layer.contains_key(layer) {
                continue;
            }
            if range_len(&xs) * range_len(&ys) > STATIC_BIN_LIMIT {
                identities.extend(self.static_by_layer.get(layer).into_iter().flatten().copied());
            } else {
                identities.extend(self.static_large.get(layer).into_iter().flatten().copied());
                for x in xs.clone() {
                    for y in ys.clone() {
                        identities.extend(self.static_bins.get(&(*layer, x, y)).into_iter().flatten().copied());
                    }
                }
            }
        }
        let mut sorted: Vec<usize> = identities.into_iter().collect();
        sorted.sort_unstable();
        sorted.into_iter().map(|identity| &self.static_regions[identity]).collect()
    }

    /// `additions`: every added track/via with its lock flag, in insertion order.
    pub fn additions(&self) -> &[(Copper, bool)] {
        &self.additions
    }

    /// Live indexed copper in insertion order.
    /// Resolved copper keepouts (`_keepouts`; read by the unified router's rasters).
    pub fn keepouts(&self) -> &[KeepoutObject] {
        &self.keepouts
    }

    /// Hard-macro copper regions (`_macro_regions`).
    pub fn macro_regions(&self) -> &[(Vec<CopperLayer>, RoundedConvexShape)] {
        &self.macro_regions
    }

    pub fn objects(&self) -> Vec<&CopperObject> {
        self.slots.iter().flatten().collect()
    }

    /// Live drilled holes in insertion order.
    pub fn holes(&self) -> Vec<&DrilledHole> {
        self.hole_slots.iter().flatten().collect()
    }

    /// `owned_count`: unlocked additions of `net` that `remove` would delete.
    pub fn owned_count(&self, net: &str) -> usize {
        self.owned.get(net).map_or(0, Vec::len)
    }

    /// `remove`: delete every unlocked track and via of net `owner`; return how many.
    ///
    /// Afterwards every query answers exactly as a fresh index of `board` with
    /// the remaining `additions()` re-added in order, including default
    /// occurrence labels, which follow their log position.
    pub fn remove(&mut self, owner: &str) -> usize {
        let Some(serials) = self.owned.shift_remove(owner).filter(|serials| !serials.is_empty()) else { return 0 };
        let doomed: IndexSet<u64> = serials.iter().copied().collect();
        let records = std::mem::take(&mut self.records);
        for record in records.iter().filter(|record| doomed.contains(&record.serial)) {
            for slot in &record.objects {
                self.drop_object(*slot);
            }
            if let Some(hole) = record.hole {
                self.drop_hole(hole);
            }
        }
        let first = records.iter().position(|record| doomed.contains(&record.serial)).expect("owned record");
        let kept: Vec<Addition> = records.into_iter().filter(|record| !doomed.contains(&record.serial)).collect();
        for (position, record) in kept.iter().enumerate().skip(first) {
            if record.default_occurrence {
                let occurrence = format!("addition:{position}:{}", record.kind);
                self.relabel(record, &occurrence);
            }
        }
        self.additions = kept.iter().map(|record| record.entry.clone()).collect();
        self.records = kept;
        if self.dead_slots + self.dead_holes > COMPACT_MIN_DEAD && 2 * self.dead_slots >= self.slots.len() {
            self.compact();
        }
        serials.len()
    }

    fn record(
        &mut self,
        entry: (Copper, bool),
        kind: &'static str,
        default: bool,
        objects: Vec<usize>,
        hole: Option<usize>,
    ) {
        let serial = self.next_serial;
        self.next_serial += 1;
        if !entry.1 {
            self.owned.entry(entry.0.net().to_owned()).or_default().push(serial);
        }
        self.records.push(Addition { serial, entry, kind, default_occurrence: default, objects, hole });
    }

    fn relabel(&mut self, record: &Addition, occurrence: &str) {
        for slot in &record.objects {
            if let Some(item) = &mut self.slots[*slot]
                && item.occurrence != occurrence
            {
                item.occurrence = occurrence.to_owned();
            }
        }
        if let Some(hole) = record.hole
            && let Some(item) = &mut self.hole_slots[hole]
            && item.occurrence != occurrence
        {
            item.occurrence = occurrence.to_owned();
        }
    }

    /// Undo exactly the bin insertions `add` made for `slot`.
    fn drop_object(&mut self, slot: usize) {
        let item = self.slots[slot].take().expect("live slot");
        let bounds = item.shape.bounds;
        for layer in &item.layers {
            for x in cells(bounds.min_x, bounds.max_x, self.bin_size_nm) {
                for y in cells(bounds.min_y, bounds.max_y, self.bin_size_nm) {
                    let key = (*layer, x, y);
                    let ids = self.bins.get_mut(&key).expect("binned slot");
                    let position = ids.iter().position(|id| *id == slot).expect("binned slot");
                    ids.remove(position);
                    if ids.is_empty() {
                        self.bins.shift_remove(&key);
                    }
                }
            }
        }
        self.dead_slots += 1;
    }

    fn add_hole(&mut self, hole: DrilledHole) -> usize {
        let slot = self.hole_slots.len();
        let key = (hole.position.x_nm.div_euclid(self.bin_size_nm), hole.position.y_nm.div_euclid(self.bin_size_nm));
        self.hole_bins.entry(key).or_default().push(slot);
        self.max_hole_radius_nm = self.max_hole_radius_nm.max(hole.radius_nm);
        self.hole_slots.push(Some(hole));
        slot
    }

    fn drop_hole(&mut self, slot: usize) {
        let hole = self.hole_slots[slot].take().expect("live hole");
        let key = (hole.position.x_nm.div_euclid(self.bin_size_nm), hole.position.y_nm.div_euclid(self.bin_size_nm));
        let ids = self.hole_bins.get_mut(&key).expect("binned hole");
        let position = ids.iter().position(|id| *id == slot).expect("binned hole");
        ids.remove(position);
        if ids.is_empty() {
            self.hole_bins.shift_remove(&key);
        }
        self.dead_holes += 1;
    }

    /// Live drills centred within `reach_nm` per axis of `position` (broad phase).
    fn nearby_holes(&self, position: Point, reach_nm: i64) -> Vec<&DrilledHole> {
        if reach_nm < 0 {
            return Vec::new();
        }
        let size = self.bin_size_nm;
        let xs = cells(position.x_nm - reach_nm, position.x_nm + reach_nm, size);
        let ys = cells(position.y_nm - reach_nm, position.y_nm + reach_nm, size);
        if range_len(&xs) * range_len(&ys) > self.hole_bins.len() as i64 {
            return self.holes();
        }
        let mut found = Vec::new();
        for x in xs {
            for y in ys.clone() {
                if let Some(ids) = self.hole_bins.get(&(x, y)) {
                    found.extend(ids.iter().map(|slot| self.hole_slots[*slot].as_ref().expect("live hole")));
                }
            }
        }
        found
    }

    /// Renumber live slots densely; relative (query) order is unchanged.
    fn compact(&mut self) {
        let mut remap: IndexMap<usize, usize> = IndexMap::new();
        let mut live = Vec::new();
        for (old, item) in std::mem::take(&mut self.slots).into_iter().enumerate() {
            if let Some(item) = item {
                remap.insert(old, live.len());
                live.push(Some(item));
            }
        }
        let mut hole_remap: IndexMap<usize, usize> = IndexMap::new();
        let mut holes = Vec::new();
        for (old, hole) in std::mem::take(&mut self.hole_slots).into_iter().enumerate() {
            if let Some(hole) = hole {
                hole_remap.insert(old, holes.len());
                holes.push(Some(hole));
            }
        }
        self.slots = live;
        self.hole_slots = holes;
        self.dead_slots = 0;
        self.dead_holes = 0;
        for ids in self.bins.values_mut() {
            for slot in ids.iter_mut() {
                *slot = remap[&*slot];
            }
        }
        for ids in self.hole_bins.values_mut() {
            for slot in ids.iter_mut() {
                *slot = hole_remap[&*slot];
            }
        }
        for record in &mut self.records {
            for slot in record.objects.iter_mut() {
                *slot = remap[&*slot];
            }
            if let Some(hole) = record.hole.as_mut() {
                *hole = hole_remap[&*hole];
            }
        }
    }

    pub fn add_track(&mut self, track: TrackSegment, locked: bool, occurrence: Option<String>) -> Result<()> {
        let default = occurrence.as_deref().is_none_or(str::is_empty);
        let occurrence = occurrence
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("addition:{}:track", self.additions.len()));
        let entry = (Copper::Track(track.clone()), locked);
        self.additions.push(entry.clone());
        let mut object = CopperObject::new(
            &track.net,
            vec![track.layer],
            shape(vec![track.start, track.end], track.width_nm.div_euclid(2))?,
        );
        object.locked = locked;
        object.occurrence = occurrence;
        let slot = self.add(object);
        self.record(entry, "track", default, vec![slot], None);
        Ok(())
    }

    pub fn add_via(&mut self, via: Via, locked: bool, occurrence: Option<String>) -> Result<()> {
        let default = occurrence.as_deref().is_none_or(str::is_empty);
        let occurrence = occurrence
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("addition:{}:via", self.additions.len()));
        let entry = (Copper::Via(via.clone()), locked);
        self.additions.push(entry.clone());
        let hole = self.add_hole(DrilledHole {
            net: via.net.clone(),
            position: via.position,
            radius_nm: via.drill_nm.div_euclid(2),
            locked,
            reusable: true,
            span: Some((via.from_layer, via.to_layer)),
            occurrence: occurrence.clone(),
        });
        let layers = self.via_layers(via.from_layer, via.to_layer);
        let mut annulus =
            CopperObject::new(&via.net, layers.clone(), shape(vec![via.position], via.size_nm.div_euclid(2))?);
        annulus.locked = locked;
        annulus.occurrence = occurrence.clone();
        let annulus = self.add(annulus);
        let mut drill = CopperObject::new(&via.net, layers, shape(vec![via.position], via.drill_nm.div_euclid(2))?);
        drill.clearance_nm = self.board.rules.minimum_hole_clearance_nm;
        drill.locked = locked;
        drill.occurrence = occurrence;
        let drill = self.add(drill);
        self.record(entry, "via", default, vec![annulus, drill], Some(hole));
        Ok(())
    }

    /// `blocking_track_nets`: movable blocker nets and whether immutable geometry blocks a track.
    pub fn blocking_track_nets(&self, track: &TrackSegment) -> Result<(IndexSet<String>, bool)> {
        let shape = shape(vec![track.start, track.end], track.width_nm.div_euclid(2))?;
        self.blockers(&track.net, &shape, &[track.layer], false, 0)
    }

    /// `track_blocker_records`: snapshot-bound exact blocker evidence; does not relax the query.
    pub fn track_blocker_records(&self, track: &TrackSegment) -> Result<Vec<RoutingBlocker>> {
        let shape = shape(vec![track.start, track.end], track.width_nm.div_euclid(2))?;
        self.blocker_records(&track.net, &shape, &[track.layer], false, 0)
    }

    fn blocker_records(
        &self,
        net: &str,
        shape: &RoundedConvexShape,
        layers: &[CopperLayer],
        for_via: bool,
        clearance_floor_nm: i64,
    ) -> Result<Vec<RoutingBlocker>> {
        let mut records = Vec::new();
        let layer_values = |layers: &[CopperLayer]| layers.iter().map(|l| l.value().to_owned()).collect::<Vec<_>>();
        let positions = |points: &[Point]| points.iter().map(|p| (p.x_nm, p.y_nm)).collect::<Vec<_>>();
        if !for_via {
            for layer in layers {
                for zone in self.zone_reservations.blocking_zones(net, shape, *layer) {
                    records.push(RoutingBlocker {
                        identity: format!("zone:{}", zone.id),
                        reason: "source routing reservation".to_owned(),
                        net: Some(zone.net.clone()),
                        immutable: true,
                        layers: layer_values(&[*layer]),
                        positions_nm: positions(&zone.outer),
                        occurrence: String::new(),
                    });
                }
            }
        }
        for region in &self.static_regions {
            if layers.iter().any(|layer| region.layers.contains(layer))
                && (if for_via { region.block_vias } else { region.block_tracks })
                && (region.has_holes || !shapes_clear(shape, &region.shape, region.clearance_nm))
            {
                let mut sorted = region.layers.clone();
                sorted.sort_by_key(|layer| layer.value());
                records.push(RoutingBlocker {
                    identity: format!("region:{}", keepout_identity(region)),
                    reason: "keepout or immutable macro region".to_owned(),
                    net: None,
                    immutable: true,
                    layers: layer_values(&sorted),
                    positions_nm: positions(&region.shape.spine),
                    occurrence: String::new(),
                });
            }
        }
        let land = self.breakout_land(net, shape);
        for other in self.overlapping_objects(shape, layers) {
            if other.net == net {
                continue;
            }
            let required =
                self.rule_clearance(net, land.as_deref(), other).max(other.clearance_nm).max(clearance_floor_nm);
            if !shapes_clear(shape, &other.shape, required) {
                let prefix = match &other.pad_reference {
                    Some(pad) => format!("pad:{}.{}:", pad.component, pad.pad),
                    None => "copper:".to_owned(),
                };
                records.push(RoutingBlocker {
                    identity: prefix + &copper_identity(other),
                    reason: if other.is_pad { "pad clearance" } else { "copper clearance" }.to_owned(),
                    net: Some(other.net.clone()),
                    immutable: other.locked,
                    layers: layer_values(&other.layers),
                    positions_nm: positions(&other.shape.spine),
                    occurrence: other.occurrence.clone(),
                });
            }
        }
        if !self.keepout_clear(shape, layers, for_via)? && !records.iter().any(|item| item.net.is_none()) {
            records.push(RoutingBlocker {
                identity: format!("board-material:{}", outline_identity(&self.board.outline)),
                reason: "board material, hole or unsupported keepout geometry".to_owned(),
                net: None,
                immutable: true,
                layers: layer_values(layers),
                positions_nm: positions(&shape.spine),
                occurrence: String::new(),
            });
        }
        Ok(sorted_records(records))
    }

    pub fn via_blocker_records(&self, via: &Via) -> Result<Vec<RoutingBlocker>> {
        let layers = self.via_layers(via.from_layer, via.to_layer);
        let annulus = shape(vec![via.position], via.size_nm.div_euclid(2))?;
        let mut records = self.blocker_records(&via.net, &annulus, &layers, true, 0)?;
        let drill = shape(vec![via.position], via.drill_nm.div_euclid(2))?;
        records.extend(self.blocker_records(
            &via.net,
            &drill,
            &layers,
            true,
            self.board.rules.minimum_hole_clearance_nm,
        )?);
        let layer_values: Vec<String> = layers.iter().map(|l| l.value().to_owned()).collect();
        for other in self.overlapping_objects(&annulus, &layers) {
            if other.is_pad && !shapes_clear(&annulus, &other.shape, 1) {
                records.push(RoutingBlocker {
                    identity: format!("pad-contact:{}", copper_identity(other)),
                    reason: "via annulus must avoid pad copper".to_owned(),
                    net: Some(other.net.clone()),
                    immutable: true,
                    layers: other.layers.iter().map(|l| l.value().to_owned()).collect(),
                    positions_nm: other.shape.spine.iter().map(|p| (p.x_nm, p.y_nm)).collect(),
                    occurrence: other.occurrence.clone(),
                });
            }
        }
        let hole_clearance = self.board.rules.minimum_hole_clearance_nm;
        for hole in
            self.nearby_holes(via.position, via.drill_nm.div_euclid(2) + self.max_hole_radius_nm + hole_clearance)
        {
            let dx = i128::from(via.position.x_nm - hole.position.x_nm);
            let dy = i128::from(via.position.y_nm - hole.position.y_nm);
            let distance = dx * dx + dy * dy;
            if distance == 0
                && hole.net == via.net
                && hole.reusable
                && hole.span == Some((via.from_layer, via.to_layer))
            {
                continue;
            }
            let required = i128::from(via.drill_nm.div_euclid(2) + hole.radius_nm + hole_clearance);
            if distance < required * required {
                records.push(RoutingBlocker {
                    identity: format!("drill:{}", hole_identity(hole)),
                    reason: "drill spacing".to_owned(),
                    net: Some(hole.net.clone()),
                    immutable: hole.locked || hole.net == via.net,
                    layers: layer_values.clone(),
                    positions_nm: vec![(hole.position.x_nm, hole.position.y_nm)],
                    occurrence: hole.occurrence.clone(),
                });
            }
        }
        Ok(sorted_records(records))
    }

    /// `via_obstacle_bounds`: bounded nearby frontier proposals, not a clearance approximation.
    #[allow(clippy::too_many_arguments)]
    pub fn via_obstacle_bounds(
        &self,
        net: &str,
        position: Point,
        size_nm: i64,
        drill_nm: i64,
        from_layer: CopperLayer,
        to_layer: CopperLayer,
        radius_nm: i64,
        maximum_obstacles: usize,
    ) -> Result<Vec<Bounds>> {
        let layers = self.via_layers(from_layer, to_layer);
        let annulus = shape(vec![position], size_nm.div_euclid(2))?;
        let land = self.breakout_land(net, &annulus);
        let hole_clearance = self.board.rules.minimum_hole_clearance_nm;
        let mut boundaries: IndexSet<(i64, i64, i64, i64)> = IndexSet::new();
        let key = |b: Bounds| (b.min_x, b.min_y, b.max_x, b.max_y);
        for other in self.nearby_copper(&annulus, &layers, radius_nm) {
            if other.net == net && !other.is_pad {
                continue;
            }
            let margin = if other.net == net {
                size_nm.div_euclid(2) + 1
            } else {
                (size_nm.div_euclid(2) + self.rule_clearance(net, land.as_deref(), other).max(other.clearance_nm))
                    .max(drill_nm.div_euclid(2) + hole_clearance)
            };
            boundaries.insert(key(other.shape.bounds.expanded(margin)));
        }
        for hole in self.nearby_holes(position, radius_nm + size_nm) {
            if (hole.position.x_nm - position.x_nm).abs() <= radius_nm + size_nm
                && (hole.position.y_nm - position.y_nm).abs() <= radius_nm + size_nm
            {
                let margin = hole.radius_nm + drill_nm.div_euclid(2) + hole_clearance;
                boundaries.insert(key(shape(vec![hole.position], margin)?.bounds));
            }
        }
        for region in
            self.overlapping_static_regions(&shape(vec![position], radius_nm + size_nm.div_euclid(2))?, &layers)
        {
            if region.block_vias {
                boundaries.insert(key(region.shape.bounds.expanded(size_nm.div_euclid(2) + region.clearance_nm + 1)));
            }
        }
        let mut sorted: Vec<(i64, i64, i64, i64)> = boundaries.into_iter().collect();
        let distance = |b: &(i64, i64, i64, i64)| -> i128 {
            let dx = i128::from((b.0 - position.x_nm).max(0).max(position.x_nm - b.2));
            let dy = i128::from((b.1 - position.y_nm).max(0).max(position.y_nm - b.3));
            dx * dx + dy * dy
        };
        sorted.sort_by(|a, b| distance(a).cmp(&distance(b)).then_with(|| a.cmp(b)));
        sorted.truncate(maximum_obstacles);
        Ok(sorted.into_iter().map(|(min_x, min_y, max_x, max_y)| Bounds { min_x, min_y, max_x, max_y }).collect())
    }

    /// `route_blockers`: `blocking_track_nets` over the pieces of `route_pieces`.
    pub fn route_blockers(
        &self,
        net: &str,
        start: Point,
        end: Point,
        width_nm: i64,
        layer: CopperLayer,
    ) -> Result<(IndexSet<String>, bool)> {
        if !self.breakout.ordinary.contains(net) {
            return self.blocking_track_nets(&TrackSegment { net: net.to_owned(), start, end, width_nm, layer });
        }
        let mut movable = IndexSet::new();
        let mut locked = false;
        for piece in self.route_pieces(net, start, end, width_nm, layer)? {
            let (names, fixed) = self.blocking_track_nets(&piece)?;
            movable.extend(names);
            locked |= fixed;
        }
        Ok((movable, locked))
    }

    /// `blocking_via_nets`: movable blocker nets and whether immutable geometry blocks a via.
    pub fn blocking_via_nets(&self, via: &Via) -> Result<(IndexSet<String>, bool)> {
        let layers = self.via_layers(via.from_layer, via.to_layer);
        let annulus = shape(vec![via.position], via.size_nm.div_euclid(2))?;
        let (mut movable, mut locked) = self.blockers(&via.net, &annulus, &layers, true, 0)?;
        let drill = shape(vec![via.position], via.drill_nm.div_euclid(2))?;
        let (drill_movable, drill_locked) =
            self.blockers(&via.net, &drill, &layers, true, self.board.rules.minimum_hole_clearance_nm)?;
        movable.extend(drill_movable);
        locked |= drill_locked;
        locked = locked || !self.pad_copper_clear(&annulus, &layers, None);
        let hole_clearance = self.board.rules.minimum_hole_clearance_nm;
        for hole in
            self.nearby_holes(via.position, via.drill_nm.div_euclid(2) + self.max_hole_radius_nm + hole_clearance)
        {
            let dx = i128::from(via.position.x_nm - hole.position.x_nm);
            let dy = i128::from(via.position.y_nm - hole.position.y_nm);
            let distance_squared = dx * dx + dy * dy;
            if distance_squared == 0
                && hole.net == via.net
                && hole.reusable
                && hole.span == Some((via.from_layer, via.to_layer))
            {
                continue;
            }
            let required = i128::from(via.drill_nm.div_euclid(2) + hole.radius_nm + hole_clearance);
            if distance_squared < required * required {
                if hole.locked || hole.net == via.net {
                    locked = true;
                } else {
                    movable.insert(hole.net.clone());
                }
            }
        }
        Ok((movable, locked))
    }

    fn blockers(
        &self,
        net: &str,
        shape: &RoundedConvexShape,
        layers: &[CopperLayer],
        for_via: bool,
        clearance_floor_nm: i64,
    ) -> Result<(IndexSet<String>, bool)> {
        if !for_via && layers.iter().any(|layer| !self.zone_reservations.can_area(net, shape, *layer)) {
            return Ok((IndexSet::new(), true));
        }
        if !self.keepout_clear(shape, layers, for_via)? {
            return Ok((IndexSet::new(), true));
        }
        let mut movable = IndexSet::new();
        let mut locked = false;
        let land = self.breakout_land(net, shape);
        for other in self.overlapping_objects(shape, layers) {
            if other.net == net {
                continue;
            }
            let clearance =
                self.rule_clearance(net, land.as_deref(), other).max(other.clearance_nm).max(clearance_floor_nm);
            if shapes_clear(shape, &other.shape, clearance) {
                continue;
            }
            if other.locked {
                locked = true;
            } else {
                movable.insert(other.net.clone());
            }
        }
        Ok((movable, locked))
    }

    /// `_via_layers`: the stackup layers a via spans, in stackup order.
    pub fn via_layers(&self, from_layer: CopperLayer, to_layer: CopperLayer) -> Vec<CopperLayer> {
        let layers = &self.board.stackup.copper_layers;
        let (a, b) = (crate::layer_index(layers, from_layer), crate::layer_index(layers, to_layer));
        layers[a.min(b)..=a.max(b)].to_vec()
    }

    fn add(&mut self, mut item: CopperObject) -> usize {
        if self.breakout.declared.contains(&item.net) {
            item.breakout_land = self.breakout.region(&item.net, &item.shape.spine);
        }
        self.add_without_breakout(item)
    }

    /// `_add` for template copper, whose breakout land is already resolved.
    fn add_without_breakout(&mut self, item: CopperObject) -> usize {
        let identity = self.slots.len();
        let bounds = item.shape.bounds;
        for layer in &item.layers {
            for x in cells(bounds.min_x, bounds.max_x, self.bin_size_nm) {
                for y in cells(bounds.min_y, bounds.max_y, self.bin_size_nm) {
                    self.bins.entry((*layer, x, y)).or_default().push(identity);
                }
            }
        }
        self.slots.push(Some(item));
        identity
    }

    fn clear(&self, net: &str, shape: &RoundedConvexShape, layers: &[CopperLayer], clearance_floor_nm: i64) -> bool {
        let land = self.breakout_land(net, shape);
        let bounds = shape.bounds;
        let (qx0, qy0, qx1, qy1) = (bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y);
        let reach = self.spacing_bound_nm.max(clearance_floor_nm);
        let minimum = self.board.rules.minimum_clearance_nm;
        let own = self.net_clearance_nm.get(net).copied().unwrap_or(minimum);
        // The broad phase of `overlapping_objects` (nearby_copper), inlined: the
        // same bins and objects, each object once. The answer is a conjunction,
        // so it does not depend on the visiting order.
        let distance = self.max_clearance_nm;
        let xs = cells(qx0 - distance, qx1 + distance, self.bin_size_nm);
        let ys = cells(qy0 - distance, qy1 + distance, self.bin_size_nm);
        let mut seen: IndexSet<usize> = IndexSet::new();
        for layer in layers {
            for x in xs.clone() {
                for y in ys.clone() {
                    let Some(ids) = self.bins.get(&(*layer, x, y)) else { continue };
                    for identity in ids {
                        if !seen.insert(*identity) {
                            continue;
                        }
                        let other = self.slots[*identity].as_ref().expect("live slot");
                        if other.net == net {
                            continue;
                        }
                        // Every clearance below is at most `margin`: a box miss
                        // at that distance is a miss for the exact test.
                        let other_box = other.shape.bounds;
                        let margin = if reach >= other.clearance_nm { reach } else { other.clearance_nm };
                        if qx1 + margin < other_box.min_x
                            || other_box.max_x < qx0 - margin
                            || qy1 + margin < other_box.min_y
                            || other_box.max_y < qy0 - margin
                        {
                            continue;
                        }
                        let mut clearance = if self.breakout_active {
                            self.rule_clearance(net, land.as_deref(), other)
                        } else {
                            own.max(self.net_clearance_nm.get(&other.net).copied().unwrap_or(0))
                        };
                        clearance = clearance.max(clearance_floor_nm).max(other.clearance_nm);
                        if !shapes_clear(shape, &other.shape, clearance) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// The breakout region holding a queried shape's centreline, if any.
    fn breakout_land(&self, net: &str, shape: &RoundedConvexShape) -> Option<String> {
        if !self.breakout.declared.contains(net) {
            return None;
        }
        self.breakout.region(net, &shape.spine)
    }

    /// Net-rule spacing between queried copper and `other`, region-aware.
    fn rule_clearance(&self, net: &str, land: Option<&str>, other: &CopperObject) -> i64 {
        if self.breakout_active {
            return self.breakout.spacing_nm(net, land, &other.net, other.breakout_land.as_deref());
        }
        let own = self.rules.get(net).and_then(|rule| rule.clearance_nm).unwrap_or(0);
        let theirs = self.rules.get(&other.net).and_then(|rule| rule.clearance_nm).unwrap_or(0);
        self.board.rules.minimum_clearance_nm.max(own).max(theirs)
    }

    pub fn overlapping_objects(&self, shape: &RoundedConvexShape, layers: &[CopperLayer]) -> Vec<&CopperObject> {
        self.nearby_copper(shape, layers, self.max_clearance_nm)
    }

    /// `nearby_copper`: indexed copper on `layers` in the bins within `distance_nm` of `shape`.
    ///
    /// The broad phase of every clearance query: pads, tracks, vias, via
    /// drills and non-plated holes, each once, in a deterministic order.
    pub fn nearby_copper(
        &self,
        shape: &RoundedConvexShape,
        layers: &[CopperLayer],
        distance_nm: i64,
    ) -> Vec<&CopperObject> {
        let area = shape.bounds.expanded(distance_nm);
        let mut seen: IndexSet<usize> = IndexSet::new();
        let mut result = Vec::new();
        for layer in layers {
            for x in cells(area.min_x, area.max_x, self.bin_size_nm) {
                for y in cells(area.min_y, area.max_y, self.bin_size_nm) {
                    for identity in self.bins.get(&(*layer, x, y)).into_iter().flatten() {
                        if seen.insert(*identity) {
                            result.push(self.slots[*identity].as_ref().expect("live slot"));
                        }
                    }
                }
            }
        }
        result
    }
}

/// `sorted(set(records), key=(identity, occurrence, reason))`.
fn sorted_records(records: Vec<RoutingBlocker>) -> Vec<RoutingBlocker> {
    let mut unique: Vec<RoutingBlocker> = Vec::new();
    for record in records {
        if !unique.contains(&record) {
            unique.push(record);
        }
    }
    unique.sort_by(|a, b| {
        (&a.identity, &a.occurrence, &a.reason).cmp(&(&b.identity, &b.occurrence, &b.reason)).then_with(|| a.cmp(b))
    });
    unique
}

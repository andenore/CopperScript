//! `constraint antenna(COMPONENT.PIN) { ... }`: a synthesized trace antenna.
//!
//! The declaration is lowered to a generated one-member hard macro in the
//! `copperlib-physical-hard-macro/v0.5` contract and bound through the same
//! immutable binder as a library macro: the feed component is the anchor, the
//! radiator is locked owner copper, the antenna region is a private region on
//! every copper layer with a zone-blocking keepout opened to the board
//! perimeter, and the feed pad is the macro's one external port. Other
//! components are kept out of the region by a placement keepout.
//!
//! Only the template tier is implemented: a first-order quarter-wave monopole
//! scaled by the declared stack-up's effective permittivity (Hammerstad-Jensen
//! microstrip estimate, as the critical router's impedance screen). There is no
//! field solution. Resonance, matching, radiation, enclosure effects and
//! regulatory compliance remain unqualified; the generated macro says so in
//! `unresolved`. See `docs/pcb-antenna-synthesis-research.md`.

use std::path::Path;

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use copper_json::{Dumps, Object, Value as Json, python_str_repr as repr};
use copper_lang::antenna::{Band, parse_bands};
use copper_lang::model::{Constraint, ConstraintKind, FlatElectricalView, ParameterValue};
use copper_physical::footprint::footprint_geometry_digest;
use copper_physical::geometry::{Bounds, bounds, segment_in_polygon};
use copper_physical::{BoardOutline, BoardSide, CopperLayer, PlacementKeepout, Point};
use copper_units::Decimal;

use crate::board::*;
use crate::hard_macros::{bind_hard_macro, ring_covers_segment};
use crate::physicalize::{Selected, physical_pin_number};
use crate::zones::{length_nm, polygon_boundary};
use crate::{Error, Result, fail};

/// The generator identity recorded in every synthesized macro's `source`.
pub const GENERATOR: &str = "copper antenna/v0.1";
/// The `members[].reference` and `net` role names inside the generated asset.
const MEMBER: &str = "ANT";
const FEED_ROLE: &str = "feed";
const SPEED_OF_LIGHT_M_PER_S: f64 = 299_792_458.0;
/// Synthesized lengths are rounded to this grid (10 µm).
const LENGTH_GRID_NM: i64 = 10_000;
/// The macro member's edge margin, as `macro` definitions default it.
const MEMBER_EDGE_CLEARANCE_NM: i64 = 250_000;

/// Which board axis the radiator runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// The region's frame: the radiator runs along `axis` from the region's low
/// coordinate; `inner` is the across-axis coordinate of the ground-side edge
/// and `outward` the across-axis direction toward the board perimeter.
#[derive(Clone, Copy, Debug)]
struct Frame {
    axis: Axis,
    along_min: i64,
    along_max: i64,
    inner: i64,
    outward: i64,
    across_min: i64,
    across_max: i64,
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

fn mm(nm: i64) -> String {
    let value = Decimal::from_i64(nm).div(&Decimal::from_i64(1_000_000)).expect("finite nanometres");
    format!("{} mm", value.to_fixed_string())
}

fn decimal_f64(value: &Decimal) -> f64 {
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
fn quarter_wave_nm(hertz: f64, eps_eff: f64) -> i64 {
    let metres = SPEED_OF_LIGHT_M_PER_S / (4.0 * hertz * eps_eff.sqrt());
    let nm = (metres * 1e9 / LENGTH_GRID_NM as f64).round() * LENGTH_GRID_NM as f64;
    nm as i64
}

/// One synthesized antenna, ready to bind.
struct Synthesis {
    name: String,
    reference: String,
    net: String,
    pose: Point,
    asset: Json,
    keepout: PlacementKeepout,
    record: Json,
}

fn synthesize(
    board: &PhysicalBoard,
    flat: &FlatElectricalView,
    components: &IndexMap<&str, &Selected>,
    index: usize,
    constraint: &Constraint,
) -> Result<Synthesis> {
    let parameters = &constraint.parameters;
    let name = text(parameters, "name").map_or_else(|| format!("antenna:{index}"), str::to_owned);
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(&name)));
    if board.rigid_clusters.iter().any(|cluster| cluster.name == name) {
        return Err(problem("its name is already a rigid cluster; give the antenna a unique name".to_owned()));
    }
    for (parameter, implemented) in [("topology", &["auto", "monopole"][..]), ("solver", &["template"][..])] {
        if let Some(value) = text(parameters, parameter)
            && !implemented.contains(&value)
        {
            return crate::not_implemented(&format!("antenna {parameter} {value}"));
        }
    }
    if text(parameters, "side") == Some("back") {
        return crate::not_implemented("a back-side synthesized antenna");
    }
    let bands = parse_bands(text(parameters, "bands").unwrap_or_default()).map_err(problem)?;
    let band: &Band = match bands.as_slice() {
        [band] => band,
        _ => return crate::not_implemented("multi-band antenna synthesis"),
    };
    let ground = text(parameters, "ground").unwrap_or_default();
    if !board.nets.iter().any(|net| net.name == ground) {
        return Err(problem(format!("ground net {} is unknown", repr(ground))));
    }

    // The feed: one component pin, physicalized to one land on one net.
    let target = &constraint.targets[0];
    let Some((reference, pin)) = target.split_once('.') else {
        return Err(problem("the feed target must be COMPONENT.PIN".to_owned()));
    };
    let Some(item) = components.get(reference) else {
        return Err(problem(format!("feed component {} has no selected footprint", repr(reference))));
    };
    let Some(pad) = physical_pin_number(item.component, item.part, &flat.devices, pin) else {
        return Err(problem(format!("feed pin {} has no physical pad", repr(target))));
    };
    let Some(placement) = board.placements.iter().find(|p| p.reference == reference) else {
        return Err(problem(format!("feed component {} is not placed", repr(reference))));
    };
    let footprint = &board.footprints[&placement.footprint];
    let lands: Vec<_> = footprint.pads.iter().filter(|p| p.number == pad).collect();
    let [land] = lands.as_slice() else {
        return Err(problem(format!("feed pad {} must be exactly one physical land", repr(target))));
    };
    let feed_pad = PadReference::new(reference, &pad);
    let Some(net) = board.nets.iter().find(|net| net.pads.contains(&feed_pad)) else {
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
    let area = region_bounds(board, parameters).map_err(|error| match error {
        Error::Value(message) => problem(message),
        other => other,
    })?;
    let host = &board.outline.vertices;
    let ring = corners(area);
    if !(0..4).all(|i| segment_in_polygon(ring[i], ring[(i + 1) % 4], host)) {
        return Err(problem("the region extends outside the board outline".to_owned()));
    }
    let perimeter = perimeter_edges(host, area);
    let frame = frame(area, &perimeter).map_err(|error| match error {
        Error::Value(message) => problem(message),
        other => other,
    })?;
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
    let across_extent = frame.across_max - frame.across_min;
    let along_extent = frame.along_max - frame.along_min;
    if across_extent < 2 * margin + width || along_extent < 2 * margin + width {
        return Err(problem(format!(
            "the region is narrower than one {} track with {} clearance on both sides",
            mm(width),
            mm(clearance)
        )));
    }
    let (eps_eff, dielectric) = effective_permittivity(board, width).map_err(|error| match error {
        Error::Value(message) => problem(message),
        other => other,
    })?;
    let centre_hz = band.centre_hz();
    let target_length = quarter_wave_nm(decimal_f64(&centre_hz), eps_eff);

    // The feed pad sits just outside the region on the ground side; the
    // component body must also clear the placement keepout and the board edge.
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
        .map(|v| frame.outward * (frame.across(*v) - frame.across(land.position)))
        .max()
        .unwrap_or(pad_across_half)
        .max(pad_across_half);
    let body_along_half =
        body.iter().map(|v| (frame.along(*v) - frame.along(land.position)).abs()).max().unwrap_or(pad_along_half);
    let feed_along = frame.along_min + margin.max(body_along_half + MEMBER_EDGE_CLEARANCE_NM);
    if feed_along + margin > frame.along_max {
        return Err(problem("the region is too short for the feed component body".to_owned()));
    }
    let pad_gap = (clearance + pad_across_half).max(body_toward_region + keepout_margin + clearance);
    let feed_centre = frame.point(feed_along, frame.inner - frame.outward * pad_gap);
    let pose = Point::new(feed_centre.x_nm - land.position.x_nm, feed_centre.y_nm - land.position.y_nm);

    // The radiator: a stub from the ground edge to the region centreline, then
    // a run along the board edge, totalling one scaled quarter wave.
    let stub = across_extent / 2;
    let centreline = frame.inner + frame.outward * stub;
    let run_limit = frame.along_max - margin;
    let available = stub + (run_limit - feed_along);
    if target_length > available {
        return Err(problem(format!(
            "the region is too short for a quarter-wave monopole at {} (effective permittivity {eps_eff:.3}): the \
             template needs {} of path but the region offers {}; enlarge the region along the board edge",
            format_hz(&centre_hz),
            mm(target_length),
            mm(available)
        )));
    }
    let mut path = vec![feed_centre];
    if target_length <= stub {
        path.push(frame.point(feed_along, frame.inner + frame.outward * target_length));
    } else {
        path.push(frame.point(feed_along, centreline));
        path.push(frame.point(feed_along + (target_length - stub), centreline));
    }
    let local = |point: Point| Point::new(point.x_nm - pose.x_nm, point.y_nm - pose.y_nm);
    let local_ring: Vec<Json> = ring.iter().map(|point| point_json(local(*point))).collect();
    let layers: Vec<&str> = board.stackup.copper_layers.iter().map(|layer| layer.value()).collect();
    let pad_json = Json::Array(vec![Json::from(MEMBER), Json::from(pad.as_str())]);
    let mut points = vec![object(vec![("pad", pad_json.clone())])];
    points.extend(path.iter().skip(1).map(|point| point_json(local(*point))));
    let region_id = format!("{name}-region");
    let edge_id = format!("{name}-edge");
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
    let unresolved = [
        "First-order quarter-wave monopole template: no field solution, no resonance or return-loss evidence",
        "Host ground plane, enclosure, battery and nearby metal are not modelled; they shift resonance",
        "Matching network values and bench tuning in the final enclosure remain mandatory",
        "No regulatory (radiated emission) claim follows from this geometry",
    ];
    let synthesis = object(vec![
        ("generator", Json::from(GENERATOR)),
        ("declaration", Json::from(constraint.origins.first().map_or("", String::as_str))),
        ("topology", Json::from("monopole")),
        ("band", Json::from(band.text.as_str())),
        ("centre_frequency_hz", Json::from(centre_hz.to_fixed_string().as_str())),
        ("stackup_dielectric", Json::from(dielectric.as_str())),
        ("effective_permittivity", Json::from(format!("{eps_eff:.6}").as_str())),
        ("quarter_wave_nm", Json::from(target_length)),
        ("path_length_nm", Json::from(target_length)),
        ("track_width_nm", Json::from(width)),
        ("region_nm", Json::Array(ring.iter().map(|point| point_json(*point)).collect())),
        ("perimeter_edges", Json::Array(perimeter.iter().map(|edge| Json::from(*edge as i64)).collect())),
    ]);
    let asset = object(vec![
        ("schema", Json::from("copperlib-physical-hard-macro/v0.5")),
        ("production_publishable", Json::Bool(false)),
        ("source", synthesis.clone()),
        ("anchor", Json::from(MEMBER)),
        (
            "members",
            Json::Array(vec![object(vec![
                ("reference", Json::from(MEMBER)),
                ("footprint", Json::from(footprint.name.as_str())),
                ("footprint_digest", Json::from(footprint_geometry_digest(footprint).as_str())),
                ("center_nm", point_json(Point::new(0, 0))),
                ("rotation_degrees", Json::from("0")),
                ("edge_clearance_nm", Json::from(MEMBER_EDGE_CLEARANCE_NM)),
            ])]),
        ),
        ("pad_nets", Json::Array(vec![texts(&[MEMBER, pad.as_str(), FEED_ROLE])])),
        ("isolated_pads", Json::Array(isolated.iter().map(|number| texts(&[MEMBER, number])).collect())),
        (
            "tracks",
            Json::Array(vec![object(vec![
                ("net", Json::from(FEED_ROLE)),
                ("layer", Json::from(CopperLayer::Front.value())),
                ("width_nm", Json::from(width)),
                ("points", Json::Array(points)),
            ])]),
        ),
        ("vias", Json::Array(Vec::new())),
        (
            "ports",
            Json::Array(vec![object(vec![
                ("name", Json::from(FEED_ROLE)),
                ("net", Json::from(FEED_ROLE)),
                ("point", object(vec![("pad", pad_json.clone())])),
                ("layer", Json::from(CopperLayer::Front.value())),
                ("pads", Json::Array(vec![pad_json])),
            ])]),
        ),
        ("protected_regions", Json::Array(vec![region_row(&region_id, true)])),
        ("keepouts", Json::Array(vec![region_row(&edge_id, false)])),
        ("zones", Json::Array(Vec::new())),
        ("polygons", Json::Array(Vec::new())),
        ("plane_returns", Json::Array(Vec::new())),
        ("width_contracts", Json::Array(Vec::new())),
        (
            "edge_openings",
            Json::Array(vec![object(vec![
                ("keepout", Json::from(edge_id.as_str())),
                ("edge_indices", Json::Array(perimeter.iter().map(|edge| Json::from(*edge as i64)).collect())),
            ])]),
        ),
        ("required_layers", texts(&layers)),
        ("allowed_rotations", Json::Array(vec![Json::from(0_i64)])),
        ("internal_clearance_nm", Json::from(0_i64)),
        ("unresolved", texts(&unresolved)),
    ]);
    copper_lang::macros::validate_macro_asset_data(&asset).map_err(problem)?;
    let keepout_area = area.expanded(keepout_margin);
    let keepout = PlacementKeepout::new(
        &format!("{name}-components"),
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
    record.insert("name".to_owned(), Json::from(name.as_str()));
    record.insert("net".to_owned(), Json::from(net.name.as_str()));
    record.insert("ground".to_owned(), Json::from(ground));
    record.insert("feed".to_owned(), Json::from(target.as_str()));
    record.insert("feed_pose_nm".to_owned(), point_json(pose));
    record.insert("path_nm".to_owned(), Json::Array(path.iter().map(|point| point_json(*point)).collect()));
    record.insert("placement_keepout".to_owned(), Json::from(keepout.name.as_str()));
    record.insert("unresolved".to_owned(), texts(&unresolved));
    Ok(Synthesis {
        name,
        reference: reference.to_owned(),
        net: net.name.clone(),
        pose,
        asset,
        keepout,
        record: Json::Object(record),
    })
}

fn format_hz(hertz: &Decimal) -> String {
    let value = decimal_f64(hertz);
    if value >= 1e9 { format!("{:.4} GHz", value / 1e9) } else { format!("{:.3} MHz", value / 1e6) }
}

/// Fix the feed component at its synthesized pose, reserve the region for
/// components, and bind the generated macro through the immutable binder.
fn bind(mut board: PhysicalBoard, synthesis: Synthesis) -> Result<PhysicalBoard> {
    let Synthesis { name, reference, net, pose, asset, keepout, .. } = synthesis;
    let problem = |message: String| Error::Value(format!("antenna {}: {message}", repr(&name)));
    if board.keepouts.iter().any(|existing| existing.name == keepout.name) {
        return Err(problem(format!("placement keepout {} already exists", repr(&keepout.name))));
    }
    let zero = Decimal::zero();
    let mut rule = board
        .placement_rules
        .iter()
        .find(|rule| rule.reference == reference)
        .cloned()
        .unwrap_or_else(|| ComponentPlacementRule::new(&reference));
    if rule.fixed_position.is_some_and(|position| position != pose)
        || rule.fixed_rotation_degrees.as_ref().is_some_and(|rotation| *rotation != zero)
        || rule.side.is_some_and(|side| side != BoardSide::Front)
    {
        return Err(problem(format!(
            "feed component {} already has a conflicting fixed placement; the antenna fixes its pose",
            repr(&reference)
        )));
    }
    rule.fixed_position = Some(pose);
    rule.fixed_rotation_degrees = Some(zero.clone());
    rule.side = Some(BoardSide::Front);
    rule.allowed_orientations = vec![zero.clone()];
    rule.edge_clearance_nm = Some(MEMBER_EDGE_CLEARANCE_NM);
    rule.region = None;
    let rule = rule.normalized()?;
    board.placement_rules.retain(|existing| existing.reference != reference);
    board.placement_rules.push(rule);
    board.placement_rules.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in &mut board.placements {
        if placement.reference == reference {
            placement.position = pose;
            placement.rotation_degrees = zero.clone();
            placement.side = BoardSide::Front;
        }
    }
    board.keepouts.push(keepout);
    let raw = copper_lang::macros::canonical_macro_bytes(&asset);
    let digest: String = Sha256::digest(&raw).iter().map(|byte| format!("{byte:02x}")).collect();
    let members: IndexMap<String, String> = [(MEMBER.to_owned(), reference.clone())].into_iter().collect();
    let nets: IndexMap<String, String> = [(FEED_ROLE.to_owned(), net)].into_iter().collect();
    let asset_path = format!("generated:{GENERATOR}:{name}");
    let mut board = match bind_hard_macro(board, Path::new(&asset_path), &raw, &digest, &name, &members, &nets) {
        Ok(board) => board,
        Err(Error::Value(message)) => return Err(problem(format!("generated macro rejected: {message}"))),
        Err(other) => return Err(other),
    };
    crate::physicalize::revalidate(&mut board)?;
    Ok(board)
}

/// `apply_antenna_constraints`: synthesize and bind every `constraint antenna`.
/// Records every synthesis in `metadata["antenna_synthesis"]`.
pub(crate) fn apply_antenna_constraints(
    mut board: PhysicalBoard,
    flat: &FlatElectricalView,
    components: &IndexMap<&str, &Selected>,
) -> Result<PhysicalBoard> {
    let mut records = Vec::new();
    for (index, constraint) in flat.constraints.iter().enumerate() {
        if constraint.kind != ConstraintKind::Antenna {
            continue;
        }
        let synthesis = synthesize(&board, flat, components, index, constraint)?;
        records.push(synthesis.record.clone());
        board = bind(board, synthesis)?;
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
        // A corner region shares a short side too; the long run still wins.
        let frame = frame_or_panic(area, &[0, 3]);
        assert_eq!(frame.axis, Axis::X);
        assert!(super::frame(area, &[]).is_err());
        assert!(super::frame(area, &[0, 2]).is_err());
    }

    fn frame_or_panic(area: Bounds, perimeter: &[usize]) -> Frame {
        frame(area, perimeter).expect("a frame")
    }
}

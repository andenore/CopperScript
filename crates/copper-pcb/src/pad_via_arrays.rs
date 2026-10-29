//! Required via-in-pad arrays (`pcbir.pad_via_arrays`): their geometry, the
//! placement-independent fit check, the presence check routing stages
//! require, and the report record. Committing an array needs the routing
//! clearance index and lives in `copper-route`'s module of the same name.

use indexmap::IndexMap;

use copper_json::Value;
use copper_physical::Point;
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::geometry::circle_inside_shape;
use copper_units::Decimal;

use crate::board::{PadReference, PadViaInPadRule, PhysicalBoard, Placement, Via};
use crate::transform::{decimal_f64, placed_pad_shape, rotate_offset, transformed_local_point};
use crate::{Error, Result};

/// Today's single fallback via size; the process qualification is unchanged.
pub const ARRAY_VIA_SIZE_NM: i64 = 300_000;
pub const ARRAY_VIA_DRILL_NM: i64 = 200_000;
/// Default pitches are rounded down to this grid for readable coordinates.
const PITCH_GRID_NM: i64 = 10_000;
/// Python builds every site eagerly; beyond this it runs out of memory.
const MAXIMUM_SITES: i64 = 1_000_000;

/// `array_rules`: permissions that require an array, in deterministic pad order.
pub fn array_rules(board: &PhysicalBoard) -> Vec<&PadViaInPadRule> {
    let mut rules: Vec<&PadViaInPadRule> = board.via_in_pad_rules.iter().filter(|rule| rule.rows.is_some()).collect();
    rules.sort_by(|a, b| a.pad.cmp(&b.pad));
    rules
}

fn shape_of(rule: &PadViaInPadRule) -> (i64, i64) {
    (rule.rows.unwrap_or(0), rule.columns.unwrap_or(0))
}

/// `rows * columns`, which every site list and report count holds.
pub fn site_count(rule: &PadViaInPadRule) -> Result<i64> {
    let (rows, columns) = shape_of(rule);
    match rows.checked_mul(columns) {
        Some(count) if count <= MAXIMUM_SITES => Ok(count),
        _ => crate::not_implemented("a via-in-pad array beyond a million sites (Python runs out of memory)"),
    }
}

/// `array_pitch_nm`: explicit pitch, else the square pitch centring the vias in equal cells.
pub fn array_pitch_nm(pad: &FootprintPad, rule: &PadViaInPadRule) -> i64 {
    if let Some(pitch) = rule.pitch_nm {
        return pitch;
    }
    let (rows, columns) = shape_of(rule);
    let cells: Vec<i64> = [(pad.size.width_nm, columns), (pad.size.height_nm, rows)]
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(extent, count)| extent.div_euclid(count))
        .collect();
    cells.iter().min().map_or(0, |cell| cell.div_euclid(PITCH_GRID_NM) * PITCH_GRID_NM)
}

/// `_offsets`: (row, column, dx, dy) in the land's own axes, row-major from -y, -x.
fn offsets(pad: &FootprintPad, rule: &PadViaInPadRule) -> Result<Vec<(i64, i64, i64, i64)>> {
    site_count(rule)?;
    let pitch = i128::from(array_pitch_nm(pad, rule));
    let (rows, columns) = shape_of(rule);
    let offset = |index: i64, count: i64| {
        i64::try_from((2 * i128::from(index) - i128::from(count) + 1) * pitch).map(|doubled| doubled.div_euclid(2))
    };
    let mut result = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let (Ok(dx), Ok(dy)) = (offset(column, columns), offset(row, rows)) else {
                return crate::not_implemented("a via-in-pad array offset beyond 2^63");
            };
            result.push((row + 1, column + 1, dx, dy));
        }
    }
    Ok(result)
}

/// `validate_array_fit`: placement-independent checks: one land, drill spacing and containment.
pub fn validate_array_fit(board: &PhysicalBoard, rule: &PadViaInPadRule, lands: &[&FootprintPad]) -> Result<()> {
    let name = format!("{}.{}", rule.pad.component, rule.pad.pad);
    let [pad] = lands else {
        return Err(Error::Value(format!("via_in_pad array {name} requires exactly one SMD land")));
    };
    let pitch = array_pitch_nm(pad, rule);
    let spacing = ARRAY_VIA_DRILL_NM + board.rules.minimum_hole_clearance_nm;
    let (rows, columns) = shape_of(rule);
    // Python multiplies unbounded integers; any overflowing product is above one.
    if rows.checked_mul(columns).is_none_or(|count| count > 1) && pitch < spacing {
        return Err(Error::Value(format!(
            "via_in_pad array {name} pitch {} mm is below the {} mm drill spacing",
            mm(pitch),
            mm(spacing)
        )));
    }
    let mut unrotated = (*pad).clone();
    unrotated.rotation_degrees = Decimal::zero();
    let land = placed_pad_shape(Point::new(0, 0), &unrotated, &unrotated_placement());
    for (row, column, dx, dy) in offsets(pad, rule)? {
        if !circle_inside_shape(Point::new(dx, dy), ARRAY_VIA_SIZE_NM / 2, &land) {
            return Err(Error::Value(format!(
                "via_in_pad array {name} site row {row} column {column} does not fit inside the land at pitch {} mm",
                mm(pitch)
            )));
        }
    }
    Ok(())
}

/// `_UNROTATED`: `placed_pad_shape` reads only its placement's rotation.
fn unrotated_placement() -> Placement {
    Placement {
        reference: String::new(),
        footprint: String::new(),
        position: Point::new(0, 0),
        rotation_degrees: Decimal::zero(),
        side: copper_physical::BoardSide::Front,
        value: String::new(),
        source_path: None,
    }
}

/// `_land`: the owner placement and its SMD land (`StopIteration` crashes Python).
pub fn land<'a>(board: &'a PhysicalBoard, reference: &PadReference) -> Result<(&'a Placement, &'a FootprintPad)> {
    let crash = || Error::NotImplemented("a via-in-pad array without its land (Python crashes)".to_owned());
    let placement = board.placements.iter().find(|item| item.reference == reference.component).ok_or_else(crash)?;
    let pad = board.footprints[&placement.footprint]
        .pads
        .iter()
        .find(|item| item.number == reference.pad && item.kind == PadKind::Smd)
        .ok_or_else(crash)?;
    Ok((placement, pad))
}

/// `_sites` (`array_sites`): board positions, rotated with the land exactly as
/// DRC shapes it; `placement` is an accepted pose of the declared owner.
pub fn array_sites(
    board: &PhysicalBoard,
    rule: &PadViaInPadRule,
    placement: Option<&Placement>,
) -> Result<Vec<(i64, i64, Point)>> {
    let (source_pose, pad) = land(board, &rule.pad)?;
    let placement = placement.unwrap_or(source_pose);
    if placement.reference != source_pose.reference || placement.footprint != source_pose.footprint {
        return Err(Error::Value("array geometry requires its declared owner and footprint".to_owned()));
    }
    let centre = transformed_local_point(placement, pad.position);
    let sum = placement.rotation_degrees.add(&pad.rotation_degrees).map_err(|e| Error::Value(e.to_string()))?;
    let angle = -decimal_f64(&sum);
    Ok(offsets(pad, rule)?
        .into_iter()
        .map(|(row, column, dx, dy)| (row, column, rotate_offset(centre, dx, dy, angle)))
        .collect())
}

/// The array via at one site.
pub fn array_via(board: &PhysicalBoard, net: &str, site: Point) -> Via {
    let layers = &board.stackup.copper_layers;
    Via {
        net: net.to_owned(),
        position: site,
        size_nm: ARRAY_VIA_SIZE_NM,
        drill_nm: ARRAY_VIA_DRILL_NM,
        from_layer: layers[0],
        to_layer: layers[layers.len() - 1],
        technology: None,
        finish: "filled-capped".to_owned(),
    }
}

/// `{pad: net.name ...}`: the last net listing a pad wins.
pub fn net_by_pad(board: &PhysicalBoard) -> IndexMap<&PadReference, &str> {
    let mut nets = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            nets.insert(pad, net.name.as_str());
        }
    }
    nets
}

/// The owner pad's net (`KeyError` crashes Python).
pub fn owner_net<'a>(nets: &IndexMap<&PadReference, &'a str>, rule: &PadViaInPadRule) -> Result<&'a str> {
    match nets.get(&rule.pad) {
        Some(net) => Ok(net),
        None => crate::not_implemented("a via-in-pad array on an unconnected pad (Python crashes)"),
    }
}

/// `via_in_pad_array_vias`: required array geometry, unchecked; `placements`
/// optionally holds accepted owner poses only. A supplied mapping may be
/// partial: unaccepted owners create no obstacles.
pub fn via_in_pad_array_vias(
    board: &PhysicalBoard,
    placements: Option<&IndexMap<String, Placement>>,
) -> Result<Vec<Via>> {
    let rules = array_rules(board);
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let nets = net_by_pad(board);
    let mut vias = Vec::new();
    for rule in rules {
        let pose = match placements {
            None => None,
            Some(poses) => match poses.get(&rule.pad.component) {
                Some(pose) => Some(pose),
                None => continue,
            },
        };
        let net = owner_net(&nets, rule)?;
        for (_, _, site) in array_sites(board, rule, pose)? {
            vias.push(array_via(board, net, site));
        }
    }
    Ok(vias)
}

/// `Counter(required) - Counter(present)` is empty: every required via is present.
pub fn multiset_contains(present: &[Via], required: &[Via]) -> bool {
    let mut remaining: Vec<(&Via, i64)> = Vec::new();
    for via in present {
        match remaining.iter_mut().find(|(item, _)| *item == via) {
            Some((_, count)) => *count += 1,
            None => remaining.push((via, 1)),
        }
    }
    required.iter().all(|via| match remaining.iter_mut().find(|(item, _)| *item == via) {
        Some((_, count)) if *count > 0 => {
            *count -= 1;
            true
        }
        _ => false,
    })
}

/// `via_in_pad_arrays_present`: every required array via is on the board (as a multiset).
pub fn via_in_pad_arrays_present(board: &PhysicalBoard) -> Result<bool> {
    Ok(multiset_contains(&board.vias, &via_in_pad_array_vias(board, None)?))
}

/// `require_via_in_pad_arrays`: reject routing that would run before a declared array is fixed copper.
pub fn require_via_in_pad_arrays(board: &PhysicalBoard, stage: &str) -> Result<()> {
    if !array_rules(board).is_empty() && !via_in_pad_arrays_present(board)? {
        return Err(Error::Value(format!("place required via-in-pad arrays before {stage}")));
    }
    Ok(())
}

/// `via_in_pad_array_report`: per-pad array records for the route and preflight reports.
pub fn via_in_pad_array_report(board: &PhysicalBoard) -> Result<Vec<Value>> {
    let vias = via_in_pad_array_vias(board, None)?;
    let mut next = vias.iter();
    let mut records = Vec::new();
    for rule in array_rules(board) {
        let (_, pad) = land(board, &rule.pad)?;
        let (rows, columns) = shape_of(rule);
        let count = site_count(rule)?;
        let array: Vec<&Via> = (0..count).filter_map(|_| next.next()).collect();
        let point = |p: Point| Value::Array(vec![Value::from(p.x_nm), Value::from(p.y_nm)]);
        records.push(Value::Object(
            [
                ("pad", Value::from(format!("{}.{}", rule.pad.component, rule.pad.pad))),
                ("rows", Value::from(rows)),
                ("columns", Value::from(columns)),
                ("count", Value::from(count)),
                ("pitch_nm", if count > 1 { Value::from(array_pitch_nm(pad, rule)) } else { Value::Null }),
                ("explicit_pitch", Value::Bool(rule.pitch_nm.is_some())),
                ("diameter_nm", Value::from(ARRAY_VIA_SIZE_NM)),
                ("drill_nm", Value::from(ARRAY_VIA_DRILL_NM)),
                ("finish", Value::from("filled-capped")),
                ("placed", Value::Bool(array.iter().all(|via| board.vias.contains(via)))),
                ("positions_nm", Value::Array(array.iter().map(|via| point(via.position)).collect())),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        ));
    }
    Ok(records)
}

/// `_mm`: `str(Decimal(value_nm) / Decimal(1_000_000))`.
pub fn mm(value_nm: i64) -> String {
    crate::kicad_pcb::mm(value_nm)
}

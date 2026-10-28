//! Required via-in-pad arrays (`pcbir.pad_via_arrays`): their geometry, the
//! presence check routing stages require, and the report record.
//! Materialization (`materialize_via_in_pad_arrays`) is not ported here.

use indexmap::IndexMap;

use copper_json::Value;
use copper_pcb::board::{PadReference, PadViaInPadRule, PhysicalBoard, Placement, Via};
use copper_pcb::transform::{decimal_f64, rotate_offset, transformed_local_point};
use copper_physical::Point;
use copper_physical::footprint::{FootprintPad, PadKind};

use crate::{Error, Result};

pub const ARRAY_VIA_SIZE_NM: i64 = 300_000;
pub const ARRAY_VIA_DRILL_NM: i64 = 200_000;
const PITCH_GRID_NM: i64 = 10_000;

/// `array_rules`: permissions that require an array, in deterministic pad order.
pub fn array_rules(board: &PhysicalBoard) -> Vec<&PadViaInPadRule> {
    let mut rules: Vec<&PadViaInPadRule> = board.via_in_pad_rules.iter().filter(|rule| rule.rows.is_some()).collect();
    rules.sort_by(|a, b| a.pad.cmp(&b.pad));
    rules
}

fn shape_of(rule: &PadViaInPadRule) -> (i64, i64) {
    (rule.rows.unwrap_or(0), rule.columns.unwrap_or(0))
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
fn offsets(pad: &FootprintPad, rule: &PadViaInPadRule) -> Vec<(i64, i64, i64, i64)> {
    let pitch = array_pitch_nm(pad, rule);
    let (rows, columns) = shape_of(rule);
    let mut result = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            result.push((
                row + 1,
                column + 1,
                ((2 * column - columns + 1) * pitch).div_euclid(2),
                ((2 * row - rows + 1) * pitch).div_euclid(2),
            ));
        }
    }
    result
}

/// `_land`: the owner placement and its SMD land (`StopIteration` crashes Python).
fn land<'a>(board: &'a PhysicalBoard, reference: &PadReference) -> Result<(&'a Placement, &'a FootprintPad)> {
    let crash = || Error::NotImplemented("a via-in-pad array without its land (Python crashes)".to_owned());
    let placement = board.placements.iter().find(|item| item.reference == reference.component).ok_or_else(crash)?;
    let pad = board.footprints[&placement.footprint]
        .pads
        .iter()
        .find(|item| item.number == reference.pad && item.kind == PadKind::Smd)
        .ok_or_else(crash)?;
    Ok((placement, pad))
}

/// `_sites`: board positions, rotated with the land exactly as DRC shapes it.
fn sites(board: &PhysicalBoard, rule: &PadViaInPadRule) -> Result<Vec<(i64, i64, Point)>> {
    let (placement, pad) = land(board, &rule.pad)?;
    let centre = transformed_local_point(placement, pad.position);
    let sum = placement.rotation_degrees.add(&pad.rotation_degrees).map_err(|e| Error::Value(e.to_string()))?;
    let angle = -decimal_f64(&sum);
    Ok(offsets(pad, rule)
        .into_iter()
        .map(|(row, column, dx, dy)| (row, column, rotate_offset(centre, dx, dy, angle)))
        .collect())
}

/// `via_in_pad_array_vias`: required array geometry, unchecked.
pub fn via_in_pad_array_vias(board: &PhysicalBoard) -> Result<Vec<Via>> {
    let rules = array_rules(board);
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let layers = &board.stackup.copper_layers;
    let mut net_by_pad: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_by_pad.insert(pad, net.name.as_str());
        }
    }
    let mut vias = Vec::new();
    for rule in rules {
        let Some(net) = net_by_pad.get(&rule.pad) else {
            return copper_pcb::not_implemented("a via-in-pad array on an unconnected pad (Python crashes)");
        };
        for (_, _, site) in sites(board, rule)? {
            vias.push(Via {
                net: (*net).to_owned(),
                position: site,
                size_nm: ARRAY_VIA_SIZE_NM,
                drill_nm: ARRAY_VIA_DRILL_NM,
                from_layer: layers[0],
                to_layer: layers[layers.len() - 1],
                technology: None,
                finish: "filled-capped".to_owned(),
            });
        }
    }
    Ok(vias)
}

/// `via_in_pad_arrays_present`: every required array via is on the board (as a multiset).
pub fn via_in_pad_arrays_present(board: &PhysicalBoard) -> Result<bool> {
    let mut present: IndexMap<&Via, i64> = IndexMap::new();
    for via in &board.vias {
        *present.entry(via).or_insert(0) += 1;
    }
    for via in via_in_pad_array_vias(board)? {
        match present.iter_mut().find(|(item, _)| ***item == via) {
            Some((_, count)) if *count > 0 => *count -= 1,
            _ => return Ok(false),
        }
    }
    Ok(true)
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
    let vias = via_in_pad_array_vias(board)?;
    let mut next = vias.iter();
    let mut records = Vec::new();
    for rule in array_rules(board) {
        let (_, pad) = land(board, &rule.pad)?;
        let (rows, columns) = shape_of(rule);
        let count = rows * columns;
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

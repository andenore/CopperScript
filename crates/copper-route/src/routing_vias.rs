//! The physical via span shared by routing stages (`pcbir.routing_vias`).
//!
//! Without an explicit technology table the only safe default is a through-via
//! spanning the complete stackup. A guide's layer transition is electrical
//! intent, not proof that blind or buried fabrication is available.

use copper_pcb::board::{PhysicalBoard, Stackup, ViaKind};
use copper_physical::CopperLayer;
use copper_units::Decimal;

/// `physical._via_span_depth`: the board thickness, or with a physical
/// stack-up the layers from the first copper layer to the second.
/// `None` where Python's position lookup raises `KeyError`.
pub fn via_span_depth(stackup: &Stackup, first: CopperLayer, second: CopperLayer) -> Option<i64> {
    if stackup.physical_layers.is_empty() {
        return Some(stackup.thickness_nm);
    }
    let position = |layer: CopperLayer| {
        stackup.physical_layers.iter().enumerate().rfind(|(_, item)| item.copper_layer == Some(layer))
    };
    let (low, high) = (position(first)?.0, position(second)?.0);
    let (low, high) = (low.min(high), low.max(high));
    Some(stackup.physical_layers[low..=high].iter().map(|item| item.thickness_nm).sum())
}

/// `Decimal(depth) / Decimal(drill) > maximum_aspect_ratio`.
pub fn exceeds_aspect_ratio(depth: i64, drill: i64, maximum: &Decimal) -> bool {
    match Decimal::from_i64(depth).div(&Decimal::from_i64(drill)) {
        Ok(ratio) => ratio > *maximum,
        Err(_) => true,
    }
}

/// `physical.select_via_technology`: the first legal technology of the
/// span in profile order; `Ok(None)` without a catalog, `Err` (Python's
/// `ValueError` message) when none is legal.
pub fn select_via_technology(
    stackup: &Stackup,
    from_layer: CopperLayer,
    to_layer: CopperLayer,
    size_nm: i64,
    drill_nm: i64,
) -> Result<Option<String>, String> {
    if stackup.via_technologies.is_empty() {
        return Ok(None);
    }
    let Some(depth) = via_span_depth(stackup, from_layer, to_layer) else {
        return Err("a via layer outside the physical stack-up".to_owned());
    };
    let span = |a: CopperLayer, b: CopperLayer| if a == b { vec![a] } else { vec![a, b] };
    let wanted = span(from_layer, to_layer);
    for technology in &stackup.via_technologies {
        let declared = span(technology.from_layer, technology.to_layer);
        if wanted.len() != declared.len() || !wanted.iter().all(|layer| declared.contains(layer)) {
            continue;
        }
        if drill_nm < technology.minimum_drill_nm
            || (size_nm - drill_nm).div_euclid(2) < technology.minimum_annular_ring_nm
            || exceeds_aspect_ratio(depth, drill_nm, &technology.maximum_aspect_ratio)
        {
            continue;
        }
        return Ok(Some(technology.id.clone()));
    }
    Err(format!(
        "no legal via technology for {} to {} with size/drill {size_nm}/{drill_nm} nm",
        from_layer.value(),
        to_layer.value()
    ))
}

/// `physical_via_span`: `(from, to, technology)` of the via a layer
/// transition needs: the transition's own span, else a through technology
/// of the outer span; `None` when no declared technology serves it.
pub fn physical_via_span(
    board: &PhysicalBoard,
    first: CopperLayer,
    second: CopperLayer,
) -> Option<(CopperLayer, CopperLayer, Option<String>)> {
    let stackup = &board.stackup;
    let layers = &stackup.copper_layers;
    let (outer_first, outer_last) = (*layers.first()?, *layers.last()?);
    if stackup.via_technologies.is_empty() {
        return Some((outer_first, outer_last, None));
    }
    for (start, end) in [(first, second), (outer_first, outer_last)] {
        let Ok(Some(id)) = select_via_technology(
            stackup,
            start,
            end,
            board.rules.default_via_size_nm,
            board.rules.default_via_drill_nm,
        ) else {
            continue;
        };
        let technology = stackup.via_technologies.iter().find(|item| item.id == id).expect("a selected technology");
        if (start, end) != (first, second) && technology.kind != ViaKind::Through {
            continue;
        }
        let index = |layer: CopperLayer| layers.iter().position(|item| *item == layer);
        let mut ordered = [start, end];
        ordered.sort_by_key(|layer| index(*layer));
        return Some((ordered[0], ordered[1], Some(id)));
    }
    None
}

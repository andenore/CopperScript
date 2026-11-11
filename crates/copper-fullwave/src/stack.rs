//! The board's stack-up as heights: copper layers on interfaces (copper
//! thickness is neglected, like the antenna model) and dielectric slabs.
//!
//! Extraction follows `copper_antenna::model::stack`; the difference is the
//! fallback for a board without a declared stack-up, which the antenna
//! model refuses: the copper layers are then spread evenly over the board
//! thickness (as `copper_drc::signal_integrity::copper_layer_depths_nm` does
//! for every EMI estimate) with FR-4 between them, relative permittivity 4.5
//! and loss tangent 0.02.

use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;
use copper_units::Decimal;

use crate::geom::nm_to_mm;
use crate::spec::{Error, Result};

/// Relative permittivity assumed without a declared stack-up (the same as
/// `emi-check`'s `DEFAULT_ER`).
pub const DEFAULT_ER: f64 = 4.5;
/// Loss tangent assumed without a declared one.
pub const DEFAULT_LOSS_TANGENT: f64 = 0.02;

/// One dielectric slab, `top > bottom`, in millimetres (`F.Cu` at 0).
#[derive(Clone, Debug, PartialEq)]
pub struct Slab {
    pub id: String,
    pub top: f64,
    pub bottom: f64,
    pub er: f64,
    pub loss_tangent: f64,
}

/// Copper-layer heights and dielectric slabs, top down.
#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    pub copper: Vec<(CopperLayer, f64)>,
    pub slabs: Vec<Slab>,
    /// Whether the board declares a mechanical stack-up (otherwise assumed).
    pub declared: bool,
}

impl Stack {
    pub fn z(&self, layer: CopperLayer) -> Option<f64> {
        self.copper.iter().find(|(l, _)| *l == layer).map(|(_, z)| *z)
    }

    pub fn max_er(&self) -> f64 {
        self.slabs.iter().map(|s| s.er).fold(1.0, f64::max)
    }

    pub fn bottom(&self) -> f64 {
        let slabs = self.slabs.iter().map(|s| s.bottom);
        let copper = self.copper.iter().map(|c| c.1);
        slabs.chain(copper).fold(0.0, f64::min)
    }

    /// The thinnest dielectric, mm.
    pub fn thinnest(&self) -> f64 {
        self.slabs.iter().map(|s| s.top - s.bottom).fold(f64::INFINITY, f64::min)
    }
}

fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().unwrap_or(f64::NAN)
}

/// The board's stack-up as heights.
pub fn stack(board: &PhysicalBoard) -> Result<Stack> {
    let layers = &board.stackup.physical_layers;
    if layers.is_empty() {
        return assumed(board);
    }
    let (mut z, mut copper, mut slabs) = (0.0_f64, Vec::new(), Vec::new());
    for layer in layers {
        match layer.copper_layer {
            Some(copper_layer) => copper.push((copper_layer, z)),
            None => {
                let Some(er) = &layer.relative_permittivity else {
                    return Err(Error::Invalid(format!("stackup dielectric {} declares no er", layer.id)));
                };
                let er = decimal_f64(er);
                let loss_tangent = layer.loss_tangent.as_ref().map_or(0.0, decimal_f64);
                let bottom = z - nm_to_mm(layer.thickness_nm);
                slabs.push(Slab { id: layer.id.clone(), top: z, bottom, er, loss_tangent });
                z = bottom;
            }
        }
    }
    if copper.is_empty() {
        return Err(Error::Invalid("the stack-up holds no copper layer".to_owned()));
    }
    // The first copper layer is the reference of z = 0; a dielectric above it
    // (solder mask, say) is above z = 0 and is dropped.
    let top = copper[0].1;
    let copper: Vec<(CopperLayer, f64)> = copper.into_iter().map(|(l, h)| (l, h - top)).collect();
    let slabs: Vec<Slab> = slabs
        .into_iter()
        .filter(|s| s.top <= top + 1e-9)
        .map(|s| Slab { top: s.top - top, bottom: s.bottom - top, ..s })
        .collect();
    Ok(Stack { copper, slabs, declared: true })
}

fn assumed(board: &PhysicalBoard) -> Result<Stack> {
    let layers = &board.stackup.copper_layers;
    if layers.len() < 2 {
        return Err(Error::Invalid(
            "a board with fewer than two copper layers and no stack-up cannot be modelled".to_owned(),
        ));
    }
    let thickness = nm_to_mm(board.stackup.thickness_nm.max(1));
    let spans = (layers.len() - 1) as f64;
    let copper: Vec<(CopperLayer, f64)> =
        layers.iter().enumerate().map(|(i, l)| (*l, -thickness * i as f64 / spans)).collect();
    let slabs = copper
        .windows(2)
        .enumerate()
        .map(|(i, w)| Slab {
            id: format!("assumed-{}", i + 1),
            top: w[0].1,
            bottom: w[1].1,
            er: DEFAULT_ER,
            loss_tangent: DEFAULT_LOSS_TANGENT,
        })
        .collect();
    Ok(Stack { copper, slabs, declared: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_pcb::board::Stackup;

    #[test]
    fn an_undeclared_stackup_spreads_the_layers_over_the_thickness() {
        let mut board = crate::testing::bare_board();
        board.stackup = Stackup::new(vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back]);
        let s = stack(&board).expect("stack");
        assert!(!s.declared);
        assert_eq!(s.copper.len(), 4);
        assert!((s.z(CopperLayer::In1).unwrap() + 1.6 / 3.0).abs() < 1e-9);
        assert!((s.bottom() + 1.6).abs() < 1e-9);
        assert_eq!(s.slabs.len(), 3);
    }
}

//! Escape planning (`pcbir.escape_plan`).
// PENDING: owned by rust-m4-global (only the options route-board builds).

/// `EscapePlanOptions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapePlanOptions {
    /// Two components sharing this many signal nets are planned as a pair.
    pub cluster_pair_minimum_nets: i64,
    /// Pairs with more shared nets are reported without an order analysis.
    pub maximum_pair_nets: i64,
    /// Pads of one component closer than this many times their median
    /// neighbour distance, or than the minimum, form one pad field.
    pub field_spacing_factor: i64,
    pub field_spacing_minimum_nm: i64,
    /// Two-end planning (E3): plan crossing bundles' layers and route order.
    pub two_end_order: bool,
    /// The corridor of a bundle is its pins' box grown by this much.
    pub corridor_margin_nm: i64,
    /// Extra cost per millimetre for a moved net off its planned layer in the
    /// corridor, in routing cost units.
    pub layer_bias_rate: i64,
    /// Array escape (E2): plan surface escapes of ball grids by min-cost flow.
    pub array_flow: bool,
    /// Bus layers (E5): spread a crossing bundle's moved nets over successive layers.
    pub bus_layers: bool,
    /// Via lattice (E7): one via site per ball grid ball that needs one.
    pub via_lattice: bool,
    pub lattice_band_rings: i64,
}

impl Default for EscapePlanOptions {
    fn default() -> Self {
        EscapePlanOptions {
            cluster_pair_minimum_nets: 2,
            maximum_pair_nets: 256,
            field_spacing_factor: 3,
            field_spacing_minimum_nm: 3_000_000,
            two_end_order: false,
            corridor_margin_nm: 1_000_000,
            layer_bias_rate: 20,
            array_flow: false,
            bus_layers: false,
            via_lattice: false,
            lattice_band_rings: 1,
        }
    }
}

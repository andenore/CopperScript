//! What is asked of the translator: the window, the nets, the ports, the
//! bridges and the fidelity.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use copper_physical::CopperLayer;

/// Why a model could not be built or run.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The request is inconsistent with the board.
    Invalid(String),
    /// The model would cost more than the budget allows; nothing was run.
    Budget(String),
    /// The solver failed.
    Solver(String),
    /// The case is outside what this slice models.
    NotSupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(m) | Error::Budget(m) | Error::Solver(m) | Error::NotSupported(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// How the copper of a net is modelled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetTreatment {
    /// Perfect conductor, zero thickness (the default).
    Pec,
    /// The solver's `ConductingSheet`: conductivity 5.8e7 S/m, the layer's thickness
    /// (35 um when the stack-up declares none). Slower to build; adds the
    /// skin-effect resistance.
    Conducting,
    /// Left out of the model.
    Omit,
}

/// Mesh density of the copper region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fidelity {
    /// Two cells across the narrowest feature, two cells across each dielectric.
    Coarse,
    /// Four cells across the narrowest feature, three across each dielectric.
    Fine,
}

impl Fidelity {
    pub fn name(self) -> &'static str {
        match self {
            Fidelity::Coarse => "coarse",
            Fidelity::Fine => "fine",
        }
    }
}

/// One end of a port or bridge: a placed pad, or an explicit rectangle on a layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Terminal {
    /// Pad `pad` of `component`, on the layer the placed footprint puts it.
    Pad { component: String, pad: String },
    /// A rectangle `[x0, y0, x1, y1]` (nm) of copper on `layer`.
    Rect { layer: CopperLayer, rect: [i64; 4] },
}

impl Terminal {
    pub fn pad(component: &str, pad: &str) -> Terminal {
        Terminal::Pad { component: component.to_owned(), pad: pad.to_owned() }
    }

    pub fn rect(layer: CopperLayer, rect: [i64; 4]) -> Terminal {
        Terminal::Rect { layer, rect }
    }

    pub fn label(&self) -> String {
        match self {
            Terminal::Pad { component, pad } => format!("{component}.{pad}"),
            Terminal::Rect { layer, rect } => {
                format!("{}[{},{},{},{}]", layer.value(), rect[0], rect[1], rect[2], rect[3])
            }
        }
    }
}

/// Where a lumped port sits.
#[derive(Clone, Debug, PartialEq)]
pub enum PortKind {
    /// In the plane of the terminals' layer, across the gap between two
    /// terminals on that layer (like a soldered source across a cut).
    Gap { from: Terminal, to: Terminal },
    /// From the terminal's copper down to the copper plane on `plane`,
    /// below it (the terminal's footprint is the port's cross-section).
    ToPlane { top: Terminal, plane: CopperLayer },
}

/// A lumped port with a reference impedance.
#[derive(Clone, Debug, PartialEq)]
pub struct PortSpec {
    pub name: String,
    pub kind: PortKind,
    /// Reference impedance and termination resistance, ohm.
    pub z_ref: f64,
}

impl PortSpec {
    pub fn gap(name: &str, from: Terminal, to: Terminal) -> PortSpec {
        PortSpec { name: name.to_owned(), kind: PortKind::Gap { from, to }, z_ref: 50.0 }
    }

    pub fn to_plane(name: &str, top: Terminal, plane: CopperLayer) -> PortSpec {
        PortSpec { name: name.to_owned(), kind: PortKind::ToPlane { top, plane }, z_ref: 50.0 }
    }

    pub fn with_z_ref(mut self, z_ref: f64) -> PortSpec {
        self.z_ref = z_ref;
        self
    }
}

/// A PEC strap shorting two terminals on the same layer: the hull of the two
/// terminals' outlines (a capacitor or a diode replaced by copper for an
/// inductance extraction).
#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    pub name: String,
    pub a: Terminal,
    pub b: Terminal,
}

/// Copper added where the board has none: a straight trace between two
/// points, for the legs of a placed (unrouted) loop.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtraTrace {
    pub name: String,
    pub net: String,
    pub layer: CopperLayer,
    pub points: Vec<[i64; 2]>,
    pub width_nm: i64,
}

/// Limits on what a run may cost. Both are checked before the time loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Largest accepted `cells x steps`, in cell updates (the expected number
    /// of steps, not the cap).
    pub max_cell_updates: f64,
    /// Largest accepted estimated run time, seconds, at `assumed_rate`.
    pub max_seconds: f64,
    /// Cell updates per second used for the estimate. 150 M/s is the
    /// low end of the rates measured on the 6-core laptop in
    /// `docs/antenna-synthesis-performance.md` (135 to 350 M/s).
    pub assumed_rate: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { max_cell_updates: 4.0e11, max_seconds: 1800.0, assumed_rate: 1.5e8 }
    }
}

/// The region-of-interest request.
#[derive(Clone, Debug)]
pub struct RegionSpec {
    /// The window `[x0, y0, x1, y1]` in board nanometres. Copper and
    /// dielectric outside `window + margin` are not modelled.
    pub window_nm: [i64; 4],
    /// Added to the window on all four sides.
    pub margin_nm: i64,
    /// Nets whose copper is modelled; `None` means every net.
    pub nets: Option<BTreeSet<String>>,
    /// Per-net treatment; nets absent from the map use `default_treatment`.
    pub net_treatment: BTreeMap<String, NetTreatment>,
    pub default_treatment: NetTreatment,
    pub ports: Vec<PortSpec>,
    pub bridges: Vec<Bridge>,
    pub extra_traces: Vec<ExtraTrace>,
    /// The analysis band `(low, high)` in hertz. The excitation covers it.
    pub band_hz: (f64, f64),
    pub fidelity: Fidelity,
    /// Override of the copper-region cell, nm (otherwise derived from the
    /// narrowest copper feature and the fidelity).
    pub cell_nm: Option<i64>,
    /// Air between the modelled window and the PML, nm (default: the larger
    /// of 8 mm and half the window's longer side).
    pub air_margin_nm: Option<i64>,
    /// Continue plane-like copper and the dielectric that touch the window edge
    /// through the air into the PML. Off: they end at the window edge
    /// (a limitation, flagged `planes_truncated`).
    pub extend_planes: bool,
    /// Fraction of the copper cell by which every outward-facing copper edge
    /// (pads, tracks, polygons, ports' terminals) is moved into the metal before
    /// meshing: the 1/3 rule. A PEC edge on a mesh line behaves as if the metal
    /// reached about half a cell further (its singular field fills the dual
    /// cell), which made the microstrip of the validation 9 to 19 % low in
    /// impedance; moving the edge a third of a cell inward removes most of it.
    /// 0 puts edges on the mesh lines as drawn.
    pub edge_inset: f64,
    /// Include the pads of components on the included nets.
    pub include_pads: bool,
    /// Include copper zones (as their outline minus holes; fills are unknown).
    pub include_zones: bool,
    /// Stop when the field energy has decayed this far (dB, negative).
    pub decay_db: f64,
    /// Simulated-time cap, ns; `None` derives it from the window.
    pub max_time_ns: Option<f64>,
    /// Solver threads; 0 picks one per physical core.
    pub threads: usize,
    pub budget: Budget,
}

impl RegionSpec {
    pub fn new(window_nm: [i64; 4]) -> RegionSpec {
        RegionSpec {
            window_nm,
            margin_nm: 0,
            nets: None,
            net_treatment: BTreeMap::new(),
            default_treatment: NetTreatment::Pec,
            ports: Vec::new(),
            bridges: Vec::new(),
            extra_traces: Vec::new(),
            band_hz: (100e6, 1e9),
            fidelity: Fidelity::Coarse,
            cell_nm: None,
            air_margin_nm: None,
            extend_planes: false,
            edge_inset: 1.0 / 3.0,
            include_pads: true,
            include_zones: true,
            decay_db: -35.0,
            max_time_ns: None,
            threads: 0,
            budget: Budget::default(),
        }
    }

    pub fn treatment(&self, net: &str) -> NetTreatment {
        self.net_treatment.get(net).copied().unwrap_or(self.default_treatment)
    }

    pub fn includes(&self, net: &str) -> bool {
        self.nets.as_ref().is_none_or(|set| set.contains(net)) && self.treatment(net) != NetTreatment::Omit
    }

    /// The window grown by the margin.
    pub fn region_nm(&self) -> [i64; 4] {
        let w = self.window_nm;
        [w[0] - self.margin_nm, w[1] - self.margin_nm, w[2] + self.margin_nm, w[3] + self.margin_nm]
    }
}

//! A canary for the pinned solver: a free-space strip dipole must show the
//! textbook ~73 ohm input resistance at resonance, a few percent below
//! c/2L. Upgrading `ems` must keep this passing before any synthesis result
//! is trusted. About two seconds in release builds.

use ems::csx::mesh::{MeshBuilder, MeshOptions};
use ems::csx::primitives::BoxShape;
use ems::csx::{ContinuousStructure, Primitive, Property};
use ems::prelude::*;

#[test]
fn a_strip_dipole_resonates_near_73_ohm() {
    let mut model = ContinuousStructure::new();
    model.grid.delta_unit = 1e-3;
    let metal = model.add_property(Property::metal("dipole"));
    let (half, w, gap) = (29.0, 1.0, 1.0);
    for (x0, x1) in [(-half, -gap / 2.0), (gap / 2.0, half)] {
        model
            .add_primitive(metal, Primitive::new(BoxShape::new([x0, -w / 2.0, 0.0], [x1, w / 2.0, 0.0])))
            .expect("arm");
    }
    let port = LumpedPort::new(1, 73.0, [-gap / 2.0, -w / 2.0, 0.0], [gap / 2.0, w / 2.0, 0.0], Axis::X)
        .excite(1.0)
        .add_to(&mut model)
        .expect("port");
    let fine = |a: f64, b: f64| (0..=((b - a) / 0.5).round() as usize).map(move |i| a + 0.5 * i as f64);
    MeshBuilder::new(MeshOptions::new(3.0).with_ratio(1.3).with_margin(45.0).with_min_res([0.25; 3]))
        .with_lines(0, fine(-30.0, 30.0))
        .with_lines(1, fine(-2.0, 2.0))
        .with_lines(2, fine(-1.0, 1.0))
        .apply(&mut model)
        .expect("mesh");
    let fdtd = Fdtd::new()
        .waveform(Waveform::gaussian(2.5e9, 1.0e9))
        .boundaries(Boundaries::all(Boundary::pml(8)))
        .end_criteria(EndCriteria::energy_decay_db(-40.0))
        .threads(std::thread::available_parallelism().map_or(4, |n| n.get()));
    let mut sim = Simulation::new(&model, &fdtd).expect("simulation");
    let results = sim.run().expect("run");
    let freqs: Vec<f64> = (0..=100).map(|i| 2.2e9 + f64::from(i) * 4e6).collect();
    let z = port.analyze(&results, &freqs).expect("analysis").z_in();
    let i = (1..z.len()).find(|i| z[i - 1].im <= 0.0 && z[*i].im > 0.0).expect("a series resonance");
    let resonance = freqs[i];
    let c_over_2l = 299_792_458.0 / (2.0 * 0.058);
    assert!((0.90..0.97).contains(&(resonance / c_over_2l)), "resonance {resonance} Hz");
    assert!((65.0..82.0).contains(&z[i].re), "R {} ohm at {resonance} Hz", z[i].re);
}

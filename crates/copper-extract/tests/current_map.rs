//! The current distribution of a solved extraction (`Extraction::current_map`): the numbers behind the picture of
//! `copper emi-check --extract --plot-current`, checked against what a microstrip over a plane must do.

use copper_emi::spectrum::Complex;
use copper_extract::extract::extract;
use copper_extract::layout::{Hint, Layout, ViaGeom};
use copper_extract::mesh::MeshSpec;

const MM: i64 = 1_000_000;
const T: f64 = 35e-6;

/// A 0.5 mm trace of net `SIG` on F.Cu along `x` from -10 to 10 mm, an `In1.Cu` plane of net `GND` 0.2 mm below it,
/// the port between the start of the trace and the plane under it, and `vias` joining the far end of the trace to
/// the plane (a short between the nets, as the wall of a test line).
fn microstrip(vias: &[(i64, i64)]) -> Layout {
    let (hx, hy) = (14 * MM, 6 * MM);
    let mut layout = Layout::new([-hx, -hy, hx, hy]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let plane = layout.add_layer("In1.Cu", -200e-6, T);
    layout.add_rect(top, "SIG", [-10 * MM, -250_000, 10 * MM, 250_000]);
    layout.add_rect(plane, "GND", [-hx, -hy, hx, hy]);
    layout.layers[plane].hints.push(Hint {
        rect: [-12 * MM, -2 * MM, 12 * MM, 2 * MM],
        cell_x_nm: 0,
        cell_y_nm: 250_000,
    });
    let plus = layout.add_terminal_rect("trace start", top, "SIG", [-10 * MM, -250_000, -9_500_000, 250_000]);
    let minus = layout.add_terminal_rect("plane under the start", plane, "GND", [-10 * MM, -500_000, -9 * MM, 500_000]);
    layout.add_port("p", plus, minus);
    for (k, &(x, y)) in vias.iter().enumerate() {
        layout.vias.push(ViaGeom {
            name: format!("v{k}"),
            x,
            y,
            net: "SIG".to_owned(),
            first_layer: top,
            last_layer: plane,
            drill_nm: 300_000,
            plating_nm: 25_000,
            layer_nets: vec![(plane, "GND".to_owned())],
        });
    }
    layout
}

fn spec() -> MeshSpec {
    MeshSpec { cell_nm: 250_000, coarse_nm: 1_000_000, coarsen: false, max_filaments: 8000, ..MeshSpec::default() }
}

#[test]
fn the_return_current_of_a_microstrip_runs_under_the_trace_and_the_vias_share_the_ampere() {
    let layout = microstrip(&[(10 * MM - 400_000, -150_000), (10 * MM - 400_000, 150_000)]);
    let x = extract(&layout, &spec(), &[0.0, 1e6, 1e8], true).expect("extract");
    assert!(x.mesh.stats.vias_unattached.is_empty(), "{:?}", x.mesh.stats.vias_unattached);
    let map = x.current_map(&layout, 2, 0).expect("a current map");
    assert_eq!(map.frequency_hz, 1e8);
    assert_eq!(map.layers.len(), 2);
    assert_eq!((map.port.plus.0, map.port.minus.0), (0, 1));

    // At the middle of the line the trace carries the whole port current along +x and the plane returns it.
    let cut = |layer: usize, keep: &dyn Fn(f64) -> bool| -> Complex {
        map.layers[layer]
            .bars
            .iter()
            .filter(|b| {
                b.along_x
                    && b.centre_mm[0] - b.length_mm / 2.0 <= 0.0
                    && b.centre_mm[0] + b.length_mm / 2.0 > 0.0
                    && keep(b.centre_mm[1])
            })
            .fold(Complex::ZERO, |a, b| a + b.current)
    };
    let trace = cut(0, &|_| true);
    let plane = cut(1, &|_| true);
    println!("at x = 0: trace {trace:?}, plane {plane:?}");
    assert!((trace.re - 1.0).abs() < 0.02 && trace.im.abs() < 0.05, "trace {trace:?}");
    assert!((plane.re + 1.0).abs() < 0.05, "the plane returns the ampere against the trace: {plane:?}");

    // Concentrated under the trace: for an infinite plane a distance h below, the fraction within +-d of the trace is
    // 2/pi atan(d/h) (87 % at d = 1 mm, h = 0.2 mm); the finite plane and the mesh cost a little of it.
    let near = cut(1, &|y| y.abs() <= 1.0);
    let fraction = near.re / plane.re;
    println!("plane current within 1 mm of the trace: {:.1} % (infinite plane 87.4 %)", fraction * 100.0);
    assert!(fraction > 0.80, "{fraction}");
    let far = cut(1, &|y| y.abs() > 3.0);
    println!("plane current more than 3 mm from the trace: {:.1} % (infinite plane 4.2 %)", 100.0 * far.re / plane.re);
    assert!(far.re.abs() < 0.06 * plane.re.abs(), "little current more than 3 mm away: {far:?}");

    // The vias: one transition, two vias at mirror positions, each about half, and together the whole ampere.
    let transitions = map.transitions();
    assert_eq!(transitions.len(), 1, "{transitions:?}");
    let t = &transitions[0];
    assert_eq!((t.net.as_str(), t.lower, t.upper), ("SIG", 1, 0));
    assert_eq!(t.vias.len(), 2);
    assert!((t.total.abs() - 1.0).abs() < 0.02, "the vias carry the port current: {:?}", t.total);
    assert!((t.vias.iter().map(|v| v.2).sum::<f64>() - 1.0).abs() < 1e-12);
    for v in &t.vias {
        assert!((v.2 - 0.5).abs() < 0.05, "mirror-symmetric vias share equally: {:?}", t.vias);
    }
    // The same numbers as the existing via-share reading of the extraction.
    let shares = x.via_shares(2, 0, &[0, 1]);
    for s in &shares {
        let from_map = t.vias.iter().find(|v| map.vias[v.0].name == format!("v{}", s.via)).expect("the via");
        assert!((from_map.2 - s.share).abs() < 1e-12);
    }
}

#[test]
fn a_via_farther_from_the_return_path_carries_less_and_the_shares_still_add_up() {
    let layout = microstrip(&[(10 * MM - 400_000, 0), (10 * MM - 400_000, 3 * MM / 2)]);
    let x = extract(&layout, &spec(), &[1e8], true).expect("extract");
    let map = x.current_map(&layout, 0, 0).expect("a current map");
    let t = &map.transitions()[0];
    assert_eq!(map.vias[t.vias[0].0].name, "v0", "the via on the trace's axis is the strongest: {t:?}");
    assert!(t.vias[0].2 > 0.6 && t.vias[1].2 < 0.4);
    assert!((t.vias.iter().map(|v| v.2).sum::<f64>() - 1.0).abs() < 1e-12);
}

#[test]
fn the_map_is_repeatable_to_the_bit_and_needs_the_branch_currents() {
    let layout = microstrip(&[(9 * MM, 0)]);
    let a = extract(&layout, &spec(), &[1e7], true).expect("extract").current_map(&layout, 0, 0).expect("map");
    let b = extract(&layout, &spec(), &[1e7], true).expect("extract").current_map(&layout, 0, 0).expect("map");
    assert_eq!(a, b);
    let without = extract(&layout, &spec(), &[1e7], false).expect("extract");
    assert!(without.current_map(&layout, 0, 0).is_none());
    let kept = extract(&layout, &spec(), &[1e7], true).expect("extract");
    assert!(kept.current_map(&layout, 0, 5).is_none(), "no such port");
    assert!(kept.current_map(&layout, 3, 0).is_none(), "no such frequency");
}

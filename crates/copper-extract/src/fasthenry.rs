//! A FastHenry input deck of a mesh, for comparing the extraction with an independent solver.
//!
//! FastHenry is a validation reference, never a dependency, and none is installed on the machine this was written
//! on: the deck has been checked for structure only (the unit test below), not run through FastHenry. One segment
//! per filament (`E` lines, width and height in micrometres), the terminals and shorts as `.equiv` node lists,
//! and one `.external` per port. FastHenry's own filament subdivision (`nhinc`, `nwinc`) is left at its default of
//! one, so the deck is the same model as the one solved here, not a refinement of it.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::mesh::Mesh;
use crate::partial::Axis;

/// The deck of `mesh` for FastHenry, analysed at `frequencies` (Hz).
pub fn deck(mesh: &Mesh, title: &str, frequencies: &[f64]) -> String {
    let um = 1e6;
    let mut position: BTreeMap<usize, [f64; 3]> = BTreeMap::new();
    for b in &mesh.network.branches {
        let c = b.bar.centre;
        let h = b.bar.length / 2.0;
        let (a, z) = match b.bar.axis {
            Axis::X => ([c[0] - h, c[1], c[2]], [c[0] + h, c[1], c[2]]),
            Axis::Y => ([c[0], c[1] - h, c[2]], [c[0], c[1] + h, c[2]]),
            Axis::Z => ([c[0], c[1], c[2] - h], [c[0], c[1], c[2] + h]),
        };
        position.entry(b.a).or_insert(a);
        position.entry(b.b).or_insert(z);
    }
    let mut text = String::new();
    let _ = writeln!(text, "* {title}");
    let _ = writeln!(text, ".units um");
    // rho in ohm um: 1.72e-8 ohm m = 1.72e-2 ohm um.
    let _ = writeln!(text, ".default rho={}", copper_emi::inductance::COPPER_RESISTIVITY * um);
    for (node, p) in &position {
        let _ = writeln!(text, "N{node} x={:.4} y={:.4} z={:.4}", p[0] * um, p[1] * um, p[2] * um);
    }
    for (k, b) in mesh.network.branches.iter().enumerate() {
        let (w, h) = match b.bar.axis {
            Axis::Z => (2.0 * b.bar.radius * um, 2.0 * b.bar.radius * um),
            _ => (b.bar.width * um, b.bar.thickness * um),
        };
        let _ = writeln!(text, "E{k} N{} N{} w={w:.4} h={h:.4}", b.a, b.b);
    }
    let mut sets: Vec<&Vec<usize>> = mesh.network.shorts.iter().collect();
    for p in &mesh.network.ports {
        sets.push(&p.plus);
        sets.push(&p.minus);
    }
    for set in sets.into_iter().filter(|s| s.len() > 1) {
        let _ = writeln!(text, ".equiv {}", set.iter().map(|n| format!("N{n}")).collect::<Vec<_>>().join(" "));
    }
    for p in &mesh.network.ports {
        if let (Some(a), Some(b)) = (p.plus.first(), p.minus.first()) {
            let _ = writeln!(text, ".external N{a} N{b}");
        }
    }
    if let (Some(lo), Some(hi)) = (frequencies.first(), frequencies.last()) {
        let ndec = if frequencies.len() > 1 {
            ((frequencies.len() - 1) as f64 / (hi / lo).log10().max(1e-9)).max(1.0)
        } else {
            1.0
        };
        let _ = writeln!(text, ".freq fmin={lo} fmax={hi} ndec={ndec:.3}");
    }
    let _ = writeln!(text, ".end");
    text
}

/// The `(frequency, R, X)` triples of a FastHenry `Zc.mat` listing of a one-port analysis.
pub fn parse_zc(listing: &str) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let mut frequency = None;
    for line in listing.lines() {
        if let Some(rest) = line.split("frequency =").nth(1) {
            frequency = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok());
        } else if let Some(f) = frequency {
            // "  1.2e-2 +  3.4e-2j"
            let cleaned = line.replace('j', " ").replace('+', " + ");
            let parts: Vec<&str> = cleaned.split_whitespace().collect();
            if parts.len() >= 3
                && parts[1] == "+"
                && let (Ok(r), Ok(x)) = (parts[0].parse::<f64>(), parts[2].parse::<f64>())
            {
                out.push((f, r, x));
                frequency = None;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;
    use crate::mesh::{MeshSpec, mesh};

    #[test]
    fn the_deck_has_one_segment_per_filament_and_an_external_port() {
        let mut layout = Layout::new([0, 0, 4_000_000, 1_000_000]);
        let layer = layout.add_layer("F.Cu", 0.0, 35e-6);
        layout.add_rect(layer, "N", [0, 0, 4_000_000, 1_000_000]);
        let a = layout.add_terminal_rect("a", layer, "N", [-1000, 0, 1000, 1_000_000]);
        let b = layout.add_terminal_rect("b", layer, "N", [3_999_000, 0, 4_001_000, 1_000_000]);
        layout.add_port("p", a, b);
        let m =
            mesh(&layout, &MeshSpec { cell_nm: 500_000, coarse_nm: 1_000_000, coarsen: false, ..MeshSpec::default() })
                .expect("mesh");
        let text = deck(&m, "test", &[1e6, 1e8]);
        assert_eq!(text.lines().filter(|l| l.starts_with('E')).count(), m.network.branches.len());
        assert_eq!(text.lines().filter(|l| l.starts_with(".external")).count(), 1);
        assert!(text.contains(".units um") && text.contains(".freq fmin=1000000") && text.trim_end().ends_with(".end"));
        assert_eq!(
            parse_zc("Impedance matrix for frequency = 1e+06 1 x 1\n   0.0102 +   0.0455j\n"),
            vec![(1e6, 0.0102, 0.0455)]
        );
    }

    /// Runs FastHenry on the deck of a straight bar when a `fasthenry` binary is on the PATH and compares the
    /// inductance. Not run by default and not verified: no FastHenry is installed where this was written.
    #[test]
    #[ignore = "needs a fasthenry binary on the PATH (none is installed where this was written)"]
    fn fasthenry_agrees_on_a_straight_bar() {
        let mut layout = Layout::new([0, 0, 10_000_000, 1_000_000]);
        let layer = layout.add_layer("F.Cu", 0.0, 35e-6);
        layout.add_rect(layer, "N", [0, 0, 10_000_000, 1_000_000]);
        let a = layout.add_terminal_rect("a", layer, "N", [-1000, 0, 1000, 1_000_000]);
        let b = layout.add_terminal_rect("b", layer, "N", [9_999_000, 0, 10_001_000, 1_000_000]);
        layout.add_port("p", a, b);
        let spec = MeshSpec { cell_nm: 250_000, coarse_nm: 2_000_000, coarsen: false, ..MeshSpec::default() };
        let x = crate::extract::extract(&layout, &spec, &[1e6], false).expect("extract");
        let text = deck(&x.mesh, "bar", &[1e6]);
        let dir = std::env::temp_dir().join(format!("copper-extract-fasthenry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("bar.inp"), text).expect("deck");
        let run = std::process::Command::new("fasthenry").arg("bar.inp").current_dir(&dir).output();
        let Ok(run) = run else {
            println!("no fasthenry on the PATH: skipped");
            return;
        };
        assert!(run.status.success());
        let listing = std::fs::read_to_string(dir.join("Zc.mat")).expect("Zc.mat");
        let z = parse_zc(&listing);
        let (f, _r, xi) = z[0];
        let l_fh = xi / (2.0 * std::f64::consts::PI * f);
        let l_here = x.inductance(0, 0, 0);
        assert!((l_fh / l_here - 1.0).abs() < 0.05, "{l_fh} vs {l_here}");
    }
}

//! Prototype: line-to-line coupling (crosstalk) from the region full-wave model, against coupled-line theory.
//!
//! Two parallel microstrips (1 mm wide, 0.5 mm over a plane, er 4.3, about 50 ohm, 20 mm long) with a lumped
//! 50 ohm port at each end: port 1 and 2 are the near and far end of the aggressor, 3 and 4 the near and far end of
//! the victim. One run excites port 1 and gives the column S11, S21 (through), S31 (near-end crosstalk, NEXT) and
//! S41 (far-end crosstalk, FEXT).
//!
//! **Reference.** The even and odd modes of the pair from the 2-D cross-section solver
//! (`copper_extract::capacitance`, within 0.3 % of closed forms on single and coupled strips, see
//! `tests/capacitance.rs`): `Z_m = 1 / (c sqrt(C_m C0_m))` and `er_eff,m = C_m / C0_m` from the capacitance with and
//! without the dielectric. Each mode is a uniform line between 50 ohm terminations; for a symmetric pair the
//! four-port column is `S11 = (S11e + S11o) / 2`, `S21 = (S21e + S21o) / 2`, `S31 = (S11e - S11o) / 2`,
//! `S41 = (S21e - S21o) / 2` (exact for TEM-like modes; microstrip is quasi-TEM below a few GHz at this height).
//! The reference length is the distance between the centres of the two port terminals.
//!
//! **What this measures.** Whether the full-wave tier can answer "how much does this line couple to that one":
//! the worst |error| of |S31| and |S41| in dB over 0.5 to 3 GHz (36 points), for a tight pair and a weak one.
//! Measured (4 threads, release, cell 0.25 mm unless said):
//!
//! | Case | Cells, steps, run | NEXT peak, full wave / reference | NEXT worst error | FEXT worst error |
//! | --- | --- | --- | --- | --- |
//! | gap 1 mm (2 h) | 0.82 M, 4,351, 16 s | -24.12 / -24.66 dB | 0.96 dB | 0.59 dB |
//! | gap 3 mm (6 h) | 0.82 M, 4,351, 18 s | -36.55 / -38.13 dB | 2.70 dB (at 3 GHz) | 1.00 dB |
//! | gap 3 mm, decay -50 dB instead of -35 | same steps (the record had decayed) | identical | 2.70 dB | 1.00 dB |
//! | gap 3 mm, cell 0.167 mm | 2.06 M, 6,551, 67 s | -36.51 dB | 2.74 dB | 1.01 dB |
//! | gap 3 mm, 8 mm of plane around the window | 1.86 M, 4,351, 39 s | -36.89 dB | 2.28 dB | 0.81 dB |
//! | gap 3 mm, planes continued into the PML | 0.82 M, 4,351, 15 s | -37.10 dB | 2.02 dB | 0.66 dB |
//! | gap 5 mm (10 h) | 0.92 M, 4,351, 18 s | -42.94 / -45.69 dB | 4.30 dB (0.89 at 0.5 GHz) | 1.43 dB |
//! | gap 5 mm, planes continued | 0.92 M, 4,351, 16 s | -43.91 dB | 3.18 dB (0.08 at 0.5 GHz) | 0.95 dB |
//! | gap 8 mm (16 h) | 1.02 M, 4,351, 20 s | -48.75 / -53.59 dB | 6.62 dB (1.65 at 0.5 GHz) | 2.10 dB |
//! | gap 8 mm, planes continued | 1.02 M, 4,351, 21 s | -50.39 dB | 4.92 dB (0.39 at 0.5 GHz) | 1.55 dB |
//!
//! So the error is not the record length (a deeper decay threshold ran the same steps) and not the mesh (a finer
//! cell changes it by 0.04 dB). Part of it is the truncated plane (the reference's plane is infinite); the rest
//! grows with frequency and is most likely the direct coupling of the two vertical port elements 4 mm apart at
//! each end, which the reference does not have. The full wave reads high in every case, which is the conservative
//! side for screening. Below about -40 dB a path that the TEM reference lacks dominates at the top of the band:
//! at 0.5 GHz the weakest pair still agrees within 0.4 dB (planes continued), at 3 GHz it reads 5 dB high. Whether
//! that path is the port elements (a modelling artefact) or the radiation of the open line ends and port wires
//! (physics the reference leaves out) is not yet separated; a line-length sweep would separate them. Until then,
//! a coupling below -40 dB at GHz frequencies is an upper bound, not an estimate, and coupling runs should continue
//! the planes into the PML (`extend_planes`), which removed 1 to 2 dB in every case.
//!
//! The full-wave runs take 15 s to a minute, so they are `#[ignore]`d: `cargo test --release -p copper-extract
//! --test coupled_microstrip -- --ignored --nocapture`. `CPL_GAP_UM`, `CPL_MARGIN_MM`, `CPL_EXTEND` and
//! `CPL_CELL` reproduce the study rows. Screening evidence, like the rest of the tier.

use copper_extract::capacitance::{Rect, SPEED_OF_LIGHT, Section, SectionMesh, Slab, Wall};
use copper_fullwave::testing::*;
use copper_fullwave::{Fidelity, PortColumn, PortSpec, RegionModel, RegionSpec, Terminal, db};
use copper_physical::CopperLayer;
use ems::Complex64;

const MM: i64 = 1_000_000;
const Z_REF: f64 = 50.0;
/// The lumped port's terminal: the last 0.5 mm of each end.
const STUB: i64 = 500_000;

/// The pair: strip width, gap between the strips, height over the plane, length (nm) and substrate permittivity.
#[derive(Clone, Copy, Debug)]
struct Pair {
    width: i64,
    gap: i64,
    height: i64,
    length: i64,
    er: f64,
}

impl Pair {
    fn with_gap(gap: i64) -> Pair {
        Pair { width: MM, gap, height: 500_000, length: 20 * MM, er: 4.3 }
    }

    /// The centre lines of the two strips (board nm).
    fn centres(&self) -> (i64, i64) {
        let y = 50 * MM;
        (y, y + self.width + self.gap)
    }
}

/// The modal parameters of a symmetric pair: `(Z_even, er_eff_even, Z_odd, er_eff_odd)`.
fn modes(pair: &Pair) -> (f64, f64, f64, f64) {
    let (w, s, h) = (pair.width as f64 * 1e-9, pair.gap as f64 * 1e-9, pair.height as f64 * 1e-9);
    let half = 40.0 * h + w + s / 2.0;
    let strip = |x0: f64, x1: f64, z: f64, group: usize| Rect { x0, x1, z0: z, z1: z, group };
    let section = Section {
        conductors: vec![
            strip(-s / 2.0 - w, -s / 2.0, h, 0),
            strip(s / 2.0, s / 2.0 + w, h, 1),
            strip(-half, half, 0.0, 2),
        ],
        slabs: vec![Slab { bottom: 0.0, top: h, er: pair.er }],
        bounds: [-half, 0.0, half, 40.0 * h],
        sides: Wall::Grounded,
        ends: Wall::Grounded,
        mesh: SectionMesh::default(),
    };
    let c = section.capacitance_matrix().expect("dielectric section");
    let c0 = section.in_vacuum().capacitance_matrix().expect("vacuum section");
    // Maxwell matrices: the off-diagonal entries are negative.
    let (ce, co) = (c[0][0] + c[0][1], c[0][0] - c[0][1]);
    let (c0e, c0o) = (c0[0][0] + c0[0][1], c0[0][0] - c0[0][1]);
    let z = |cm: f64, c0m: f64| 1.0 / (SPEED_OF_LIGHT * (cm * c0m).sqrt());
    (z(ce, c0e), ce / c0e, z(co, c0o), co / c0o)
}

/// `(S11, S21)` of a uniform line of impedance `z` and electrical length `theta` between `Z_REF` terminations.
fn line(z: f64, theta: f64) -> (Complex64, Complex64) {
    let g = Complex64::new((z - Z_REF) / (z + Z_REF), 0.0);
    let p = Complex64::from_polar(1.0, -theta);
    let den = Complex64::ONE - g * g * p * p;
    (g * (Complex64::ONE - p * p) / den, p * (Complex64::ONE - g * g) / den)
}

/// The reference column `[S11, S21, S31, S41]` at `f` Hz, from the pair's [`modes`].
fn reference(pair: &Pair, modes: (f64, f64, f64, f64), f: f64) -> [Complex64; 4] {
    let (ze, ere, zo, ero) = modes;
    let length = (pair.length - STUB) as f64 * 1e-9;
    let k0 = 2.0 * std::f64::consts::PI * f / SPEED_OF_LIGHT;
    let (s11e, s21e) = line(ze, k0 * ere.sqrt() * length);
    let (s11o, s21o) = line(zo, k0 * ero.sqrt() * length);
    let h = Complex64::new(0.5, 0.0);
    [h * (s11e + s11o), h * (s21e + s21o), h * (s11e - s11o), h * (s21e - s21o)]
}

/// The full-wave model of the pair: ports 1/2 at the near/far end of the aggressor, 3/4 of the victim.
fn model(pair: &Pair, cell: i64, decay_db: f64) -> RegionModel {
    let mut board = bare_board();
    let er = format!("{}", pair.er);
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, pair.height, &er, "0.0"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let (x0, x1) = (20 * MM, 20 * MM + pair.length);
    let (ya, yb) = pair.centres();
    let w = pair.width;
    for (net, y) in [("AGG", ya), ("VIC", yb)] {
        board.tracks.push(track(net, (x0 + w / 2, y), (x1 - w / 2, y), w, CopperLayer::Front));
    }
    board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
    let end = |a: i64, b: i64, y: i64| Terminal::rect(CopperLayer::Front, [a, y - w / 2, b, y + w / 2]);
    let half = (w / 2 + 4 * pair.height).max(3 * MM);
    let mut spec = RegionSpec::new([x0, ya - half, x1, yb + half]);
    // `CPL_MARGIN_MM` and `CPL_EXTEND` (studies): more modelled plane around the pair, or planes continued into
    // the PML.
    spec.margin_nm = std::env::var("CPL_MARGIN_MM").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(2) * MM;
    spec.extend_planes = std::env::var_os("CPL_EXTEND").is_some();
    spec.band_hz = (0.5e9, 3.0e9);
    spec.fidelity = Fidelity::Coarse;
    spec.cell_nm = Some(cell);
    spec.decay_db = decay_db;
    let port = |name: &str, a: i64, b: i64, y: i64| {
        PortSpec::to_plane(name, end(a, b, y), CopperLayer::Back).with_z_ref(Z_REF)
    };
    spec.ports = vec![
        port("agg_near", x0, x0 + STUB, ya),
        port("agg_far", x1 - STUB, x1, ya),
        port("vic_near", x0, x0 + STUB, yb),
        port("vic_far", x1 - STUB, x1, yb),
    ];
    RegionModel::build(&board, &spec).expect("model")
}

/// What one case measured, in dB over the band: the largest |error| of NEXT and FEXT, and the peaks.
struct Comparison {
    next_error_db: f64,
    fext_error_db: f64,
    next_peak_db: (f64, f64),
    fext_peak_db: (f64, f64),
}

fn compare(pair: &Pair, column: &PortColumn) -> Comparison {
    let (ze, ere, zo, ero) = modes(pair);
    println!(
        "  modes: even {ze:.2} ohm er_eff {ere:.3}, odd {zo:.2} ohm er_eff {ero:.3}; coupling factor (Ze-Zo)/(Ze+Zo) {:.2} dB",
        db(Complex64::new((ze - zo) / (ze + zo), 0.0))
    );
    println!("  f GHz | S21 fw / ref | NEXT fw / ref | FEXT fw / ref  (dB)");
    let (mut next_error, mut fext_error) = (0.0_f64, 0.0_f64);
    let (mut next_peak, mut fext_peak) =
        ((f64::NEG_INFINITY, f64::NEG_INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
    for (k, f) in column.freqs.iter().enumerate() {
        let r = reference(pair, (ze, ere, zo, ero), *f);
        let (s21, s31, s41) = (db(column.s[1][k]), db(column.s[2][k]), db(column.s[3][k]));
        let (r21, r31, r41) = (db(r[1]), db(r[2]), db(r[3]));
        if k % 5 == 0 {
            println!("  {:5.2} | {s21:6.2} / {r21:6.2} | {s31:7.2} / {r31:7.2} | {s41:7.2} / {r41:7.2}", f / 1e9);
        }
        next_error = next_error.max((s31 - r31).abs());
        fext_error = fext_error.max((s41 - r41).abs());
        next_peak = (next_peak.0.max(s31), next_peak.1.max(r31));
        fext_peak = (fext_peak.0.max(s41), fext_peak.1.max(r41));
    }
    Comparison {
        next_error_db: next_error,
        fext_error_db: fext_error,
        next_peak_db: next_peak,
        fext_peak_db: fext_peak,
    }
}

fn run(name: &str, pair: Pair, cell: i64, decay_db: f64) -> Comparison {
    let model = model(&pair, cell, decay_db);
    let t = std::time::Instant::now();
    let column = model.solve_port(0).expect("solve");
    println!(
        "{name}: gap {:.2} mm, cell {:.3} mm, decay {decay_db} dB: {} cells, {} steps, converged {}, {:.0} s",
        pair.gap as f64 * 1e-6,
        cell as f64 * 1e-6,
        model.mesh.cells,
        column.stats.steps,
        column.stats.converged,
        t.elapsed().as_secs_f64()
    );
    let c = compare(&pair, &column);
    println!(
        "  NEXT peak {:.2} dB (ref {:.2}), worst error {:.2} dB; FEXT peak {:.2} dB (ref {:.2}), worst error {:.2} dB",
        c.next_peak_db.0, c.next_peak_db.1, c.next_error_db, c.fext_peak_db.0, c.fext_peak_db.1, c.fext_error_db
    );
    c
}

/// The reference itself: reciprocal and lossless, so `|S11|^2 + |S21|^2 + |S31|^2 + |S41|^2 = 1`, and the near-end
/// coupling peaks at a quarter wave at `(Ze - Zo) / (Ze + Zo)` (for a matched pair).
#[test]
fn the_coupled_line_reference_is_lossless_and_peaks_at_the_coupling_factor() {
    let pair = Pair::with_gap(MM);
    let m = modes(&pair);
    let (ze, ere, zo, ero) = m;
    assert!(ze > zo && ere > ero, "even {ze} {ere}, odd {zo} {ero}");
    // The even and odd impedances bracket the single line's ~50 ohm.
    assert!(ze > 50.0 && zo < 50.0 && (ze * zo).sqrt() > 40.0 && (ze * zo).sqrt() < 60.0);
    let mut peak = 0.0_f64;
    for k in 0..=300 {
        let f = 0.01e9 * k as f64 + 0.1e9;
        let s = reference(&pair, m, f);
        let power: f64 = s.iter().map(|x| x.norm_sqr()).sum();
        assert!((power - 1.0).abs() < 1e-12, "{f}: {power}");
        peak = peak.max(s[2].norm());
    }
    let factor = (ze - zo) / (ze + zo);
    assert!((peak / factor - 1.0).abs() < 0.1, "NEXT peak {peak} against coupling factor {factor}");
}

/// A tight pair (gap 1 mm = 2 h): NEXT about -25 dB.
#[test]
#[ignore = "full-wave run, about 20 s"]
fn tight_pair_crosstalk_matches_coupled_line_theory() {
    let c = run("tight pair", Pair::with_gap(MM), 250_000, -35.0);
    assert!(c.next_error_db < 1.5, "NEXT error {} dB", c.next_error_db);
    assert!(c.fext_error_db < 1.0, "FEXT error {} dB", c.fext_error_db);
    assert!((c.next_peak_db.0 - c.next_peak_db.1).abs() < 1.0);
}

/// A weak pair (gap 3 mm = 6 h): NEXT about -38 dB, where the error is larger (see the module table).
#[test]
#[ignore = "full-wave run, about 20 s"]
fn weak_pair_crosstalk_matches_coupled_line_theory_within_three_db() {
    let cell = std::env::var("CPL_CELL").ok().and_then(|v| v.parse().ok()).unwrap_or(250_000);
    let gap = std::env::var("CPL_GAP_UM").ok().and_then(|v| v.parse::<i64>().ok()).map_or(3 * MM, |um| um * 1000);
    let c = run("weak pair", Pair::with_gap(gap), cell, -35.0);
    assert!(c.next_error_db < 3.0, "NEXT error {} dB", c.next_error_db);
    assert!(c.fext_error_db < 1.5, "FEXT error {} dB", c.fext_error_db);
    // Conservative: the full wave does not under-read the coupling.
    assert!(c.next_peak_db.0 > c.next_peak_db.1 - 0.5);
}

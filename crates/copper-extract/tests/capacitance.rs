//! The 2-D cross-section solver of `copper_extract::capacitance` against closed forms (zero-thickness strips unless
//! said otherwise), at the mesh the board extraction uses ([`SectionMesh::default`]).
//!
//! | Case | Closed form | Its own accuracy | Measured | Bound here |
//! | --- | --- | --- | --- | --- |
//! | parallel plates, two slabs in series, open side walls | `w / (t1/(e0 er1) + t2/(e0 er2))` | exact | 1.5e-13 | 1e-9 |
//! | wide strip over a plane, homogeneous, `w/h` 10, 30, 100 | Hammerstad, vacuum impedance | 0.03 % | +0.20 to +0.24 % | 0.5 % |
//! | microstrip on `er` 4.3, air above, `w/h` 0.25 to 10 | Hammerstad and Jensen | 0.2 % | +0.12 to +0.26 % | 0.5 % |
//! | edge-coupled strips between two planes, even and odd mode | Cohn (1955) | exact | +0.10 to +0.20 % | 0.5 % |
//! | the same, mutual capacitance `(C_odd - C_even) / 2` | Cohn (1955) | exact | +0.29 to +0.61 % | 1 % |
//! | two coplanar strips, no ground, `s/w` 0.4 to 2 | conformal mapping, `eps K(k')/K(k)` | exact | +0.18 to +0.20 % | 0.5 % |
//!
//! The solver overestimates by these amounts (the edge cells' first-order error is extrapolated away; what is left
//! is the grading's). A mutual capacitance is a difference of two larger numbers, so a weak one (strips a plane
//! spacing apart) carries the larger relative error. Copper thickness and solder mask are not in the closed forms:
//! `thickness_and_mask_raise_a_microstrip` prints what the solver gives for them. The measured errors are printed
//! with `--nocapture`.

use copper_extract::capacitance::{EPSILON_0, Rect, Section, SectionMesh, Slab, Wall, closed_form};

fn rel(a: f64, b: f64) -> f64 {
    (a - b) / b
}

fn strip(x0: f64, x1: f64, z: f64, group: usize) -> Rect {
    Rect { x0, x1, z0: z, z1: z, group }
}

/// A strip of width `w` at height `h` over a plane (group 1 at z = 0, as wide as the box), dielectric `er` up to
/// `slab_top` (air above), box `half` wide each side and `top` high.
fn microstrip(w: f64, h: f64, er: f64, slab_top: f64, half: f64, top: f64) -> Section {
    Section {
        conductors: vec![strip(-w / 2.0, w / 2.0, h, 0), Rect { x0: -half, x1: half, z0: 0.0, z1: 0.0, group: 1 }],
        slabs: vec![Slab { bottom: 0.0, top: slab_top, er }],
        bounds: [-half, 0.0, half, top],
        sides: Wall::Grounded,
        ends: Wall::Grounded,
        mesh: SectionMesh::default(),
    }
}

#[test]
fn parallel_plates_in_layered_dielectric_are_exact() {
    let (w, t1, t2, er1, er2) = (2e-3, 0.2e-3, 1.0e-3, 4.3, 4.6);
    let section = Section {
        conductors: vec![strip(0.0, w, t1 + t2, 0), strip(0.0, w, 0.0, 1)],
        slabs: vec![Slab { bottom: 0.0, top: t2, er: er2 }, Slab { bottom: t2, top: t1 + t2, er: er1 }],
        bounds: [0.0, 0.0, w, t1 + t2],
        sides: Wall::Open,
        ends: Wall::Open,
        mesh: SectionMesh::default(),
    };
    let c = section.capacitance_matrix().unwrap();
    let expected = w / (t1 / (EPSILON_0 * er1) + t2 / (EPSILON_0 * er2));
    println!("parallel plates: {:.6e} F/m against {expected:.6e} ({:.2e})", c[0][0], rel(c[0][0], expected));
    assert!(rel(c[0][0], expected).abs() < 1e-9);
    assert!(rel(-c[0][1], expected).abs() < 1e-9);
}

#[test]
fn a_wide_strip_over_a_plane_matches_hammerstad_and_shows_its_fringe() {
    let h = 0.2e-3;
    for u in [10.0, 30.0, 100.0] {
        let w = u * h;
        let section = microstrip(w, h, 4.3, 40.0 * h + w, w / 2.0 + 40.0 * h, 40.0 * h + w);
        let c = section.solve(&[1.0, 0.0]).unwrap().groups[0];
        let exact = closed_form::strip_over_plane_capacitance(u, 4.3);
        let plate = EPSILON_0 * 4.3 * u;
        println!(
            "wide strip w/h {u}: {:.5e} F/m, Hammerstad {exact:.5e} ({:+.3} %), plate eps w/h {plate:.5e} (fringe {:.1} %)",
            c,
            100.0 * rel(c, exact),
            100.0 * rel(exact, plate)
        );
        assert!(rel(c, exact).abs() < 0.005, "w/h {u}: {}", rel(c, exact));
    }
}

#[test]
fn microstrip_on_a_substrate_matches_hammerstad_and_jensen() {
    let h = 0.2e-3;
    for u in [0.25, 0.5, 1.0, 2.0, 5.0, 10.0] {
        let w = u * h;
        let section = microstrip(w, h, 4.3, h, 40.0 * h + w, 40.0 * h);
        let c = section.solve(&[1.0, 0.0]).unwrap().groups[0];
        let exact = closed_form::microstrip_capacitance(u, 4.3);
        println!("microstrip w/h {u}: {c:.5e} F/m, Hammerstad-Jensen {exact:.5e} ({:+.3} %)", 100.0 * rel(c, exact));
        assert!(rel(c, exact).abs() < 0.005, "w/h {u}: {}", rel(c, exact));
    }
}

#[test]
fn edge_coupled_strips_between_planes_match_cohn() {
    let b = 0.5e-3;
    for (w, s) in [(0.1e-3, 0.1e-3), (0.2e-3, 0.2e-3), (0.15e-3, 0.5e-3)] {
        let half = 10.0 * b + w + s;
        let section = Section {
            conductors: vec![
                strip(-s / 2.0 - w, -s / 2.0, b / 2.0, 0),
                strip(s / 2.0, s / 2.0 + w, b / 2.0, 1),
                Rect { x0: -half, x1: half, z0: 0.0, z1: 0.0, group: 2 },
                Rect { x0: -half, x1: half, z0: b, z1: b, group: 2 },
            ],
            slabs: vec![Slab { bottom: 0.0, top: b, er: 4.3 }],
            bounds: [-half, 0.0, half, b],
            sides: Wall::Grounded,
            ends: Wall::Grounded,
            mesh: SectionMesh::default(),
        };
        let c = section.capacitance_matrix().unwrap();
        let (even, odd) = closed_form::coupled_stripline(w, s, b, 4.3);
        let (c_even, c_odd) = (c[0][0] + c[0][1], c[0][0] - c[0][1]);
        let (mutual, mutual_exact) = (-c[0][1], (odd - even) / 2.0);
        println!(
            "coupled stripline w {w:e} s {s:e}: even {:+.3} %, odd {:+.3} %, mutual {mutual:.4e} against {mutual_exact:.4e} ({:+.3} %), symmetry {:.1e}",
            100.0 * rel(c_even, even),
            100.0 * rel(c_odd, odd),
            100.0 * rel(mutual, mutual_exact),
            rel(c[0][1], c[1][0])
        );
        assert!(rel(c_even, even).abs() < 0.005);
        assert!(rel(c_odd, odd).abs() < 0.005);
        assert!(rel(mutual, mutual_exact).abs() < 0.01);
        assert!(rel(c[0][1], c[1][0]).abs() < 1e-9);
    }
}

#[test]
fn coplanar_strips_match_the_conformal_map() {
    for (w, s) in [(0.2e-3, 0.1e-3), (0.2e-3, 0.4e-3), (0.5e-3, 0.2e-3)] {
        let half = 60.0 * (2.0 * w + s);
        let section = Section {
            conductors: vec![strip(-s / 2.0 - w, -s / 2.0, 0.0, 0), strip(s / 2.0, s / 2.0 + w, 0.0, 1)],
            slabs: vec![],
            bounds: [-half, -half, half, half],
            sides: Wall::Grounded,
            ends: Wall::Grounded,
            mesh: SectionMesh::default(),
        };
        let q = section.solve(&[0.5, -0.5]).unwrap();
        let c = q.groups[0];
        let exact = closed_form::coplanar_strips(w, s, 1.0);
        println!("coplanar strips w {w:e} s {s:e}: {c:.5e} F/m, exact {exact:.5e} ({:+.3} %)", 100.0 * rel(c, exact));
        assert!(rel(c, exact).abs() < 0.005);
    }
}

#[test]
fn thickness_and_mask_raise_a_microstrip() {
    // A 0.25 mm trace 0.2 mm over its plane on er 4.3: zero thickness, 35 um copper, and 35 um copper under a
    // 20 um mask of er 3.5 (as a slab over the board and the trace: the mask between traces is thinner in reality).
    let (w, h, t) = (0.25e-3, 0.2e-3, 35e-6);
    let zero = microstrip(w, h, 4.3, h, 40.0 * h + w, 40.0 * h);
    let mut thick = zero.clone();
    thick.conductors[0] = Rect { x0: -w / 2.0, x1: w / 2.0, z0: h, z1: h + t, group: 0 };
    let mut masked = thick.clone();
    masked.slabs.push(Slab { bottom: h, top: h + t + 20e-6, er: 3.5 });
    let c = |s: &Section| s.solve(&[1.0, 0.0]).unwrap().groups[0];
    let (c0, c1, c2) = (c(&zero), c(&thick), c(&masked));
    println!(
        "microstrip w 0.25 mm h 0.2 mm: zero thickness {c0:.4e} F/m, 35 um {c1:.4e} ({:+.2} %), with 20 um mask {c2:.4e} ({:+.2} %)",
        100.0 * rel(c1, c0),
        100.0 * rel(c2, c0)
    );
    assert!(c1 > c0 && c2 > c1);
}

/// The coarser mesh of the board extraction (`ParasiticOptions::default().mesh`) on the same cases: the largest
/// error of a capacitance per unit length and of a mutual one.
fn errors_with(mesh: SectionMesh) -> (f64, f64) {
    let (mut own, mut mutual) = (0.0f64, 0.0f64);
    let h = 0.2e-3;
    for u in [0.25, 1.0, 5.0] {
        let mut section = microstrip(u * h, h, 4.3, h, 40.0 * h + u * h, 40.0 * h);
        section.mesh = mesh;
        let c = section.solve(&[1.0, 0.0]).unwrap().groups[0];
        own = own.max(rel(c, closed_form::microstrip_capacitance(u, 4.3)).abs());
    }
    let b = 0.5e-3;
    for (w, s) in [(0.1e-3, 0.1e-3), (0.15e-3, 0.5e-3)] {
        let half = 10.0 * b + w + s;
        let section = Section {
            conductors: vec![
                strip(-s / 2.0 - w, -s / 2.0, b / 2.0, 0),
                strip(s / 2.0, s / 2.0 + w, b / 2.0, 1),
                Rect { x0: -half, x1: half, z0: 0.0, z1: 0.0, group: 2 },
                Rect { x0: -half, x1: half, z0: b, z1: b, group: 2 },
            ],
            slabs: vec![Slab { bottom: 0.0, top: b, er: 4.3 }],
            bounds: [-half, 0.0, half, b],
            sides: Wall::Grounded,
            ends: Wall::Grounded,
            mesh,
        };
        let c = section.capacitance_matrix().unwrap();
        let (even, odd) = closed_form::coupled_stripline(w, s, b, 4.3);
        own = own.max(rel(c[0][0] + c[0][1], even).abs()).max(rel(c[0][0] - c[0][1], odd).abs());
        mutual = mutual.max(rel(-c[0][1], (odd - even) / 2.0).abs());
    }
    (own, mutual)
}

#[test]
fn the_board_mesh_is_coarser_and_says_how_much() {
    let board = copper_extract::parasitics::ParasiticOptions::default().mesh;
    let (own, mutual) = errors_with(board);
    let (own_default, mutual_default) = errors_with(SectionMesh::default());
    println!(
        "board mesh: largest error {:.3} % (capacitance), {:.3} % (mutual); default mesh {:.3} %, {:.3} %",
        100.0 * own,
        100.0 * mutual,
        100.0 * own_default,
        100.0 * mutual_default
    );
    assert!(own < 0.01 && mutual < 0.03, "{own} {mutual}");
}

#[test]
fn the_closed_forms_agree_with_each_other_in_their_limits() {
    // Cohn's coupled pair far apart is the single centred stripline, sech form: C = 4 eps K(k')/K(k), k = sech(pi w / 2b).
    let (w, b) = (0.2e-3, 0.5e-3);
    let (even, odd) = closed_form::coupled_stripline(w, 20e-3, b, 1.0);
    let x = std::f64::consts::PI * w / (2.0 * b);
    let (k, kc) = (1.0 / x.cosh(), x.tanh());
    let single = 4.0 * EPSILON_0 * closed_form::k_ratio(k, kc);
    assert!(rel(even, single).abs() < 1e-9 && rel(odd, single).abs() < 1e-9);
    // Hammerstad's homogeneous strip and Hammerstad-Jensen with er = 1 are one formula.
    assert!(
        rel(closed_form::microstrip_capacitance(1.0, 1.0), closed_form::strip_over_plane_capacitance(1.0, 1.0)).abs()
            < 1e-12
    );
}

#[test]
#[ignore]
fn study_mesh_convergence() {
    let h = 0.2e-3;
    for (extrapolate, coarse, box_h) in [(false, 0.05, 40.0), (true, 0.05, 40.0)] {
        for fraction in [1.0 / 20.0, 1.0 / 40.0, 1.0 / 80.0, 1.0 / 160.0] {
            for growth in [1.3, 1.25, 1.2, 1.15, 1.1] {
                let mesh = SectionMesh {
                    fine_fraction: fraction,
                    fine_min: 1e-10,
                    fine_max: 1.0,
                    growth,
                    coarse_fraction: coarse,
                    extrapolate,
                };
                let mut section = microstrip(h, h, 4.3, h, box_h * h + h, box_h * h);
                section.mesh = mesh;
                let c = section.solve(&[1.0, 0.0]).unwrap().groups[0];
                let exact = closed_form::microstrip_capacitance(1.0, 4.3);
                println!(
                    "extrapolate {extrapolate} box {box_h} coarse {coarse} fraction 1/{:.0} growth {growth}: grid {:?} error {:+.4} %",
                    1.0 / fraction,
                    section.grid_size(),
                    100.0 * rel(c, exact)
                );
            }
        }
    }
}

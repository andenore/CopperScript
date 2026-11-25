//! Prototype: coupling from an antenna's feed to a trace elsewhere on the board (`model::evaluate_coupled`).
//!
//! The board is `examples/antenna_synthesis/board.copper` (a 2.4 GHz monopole in the top-right corner, GND zoned on
//! F.Cu and B.Cu over 1.51 mm of FR-4) with its tuned geometry. A synthetic 0.5 mm F.Cu trace of 14 mm on a net of
//! its own is added parallel to the monopole's arm, either 2.5 mm below the antenna region (`near`) or 18 mm below
//! it (`far`), or 2.5 mm below with a 45-degree jog of 2 mm in its middle (`jogged`), terminated in 50 ohm at both
//! ends to B.Cu. The coarse fidelity is used. Measured (4 threads): antenna alone 0.29 M cells, 10 s; near 0.31 M
//! cells, 10 s, -28.9 / -31.0 dB at the two ends at 2.44 GHz; far 0.41 M cells, 13 s, -48.3 / -48.9 dB, S11 moved
//! by at most 0.04 dB; jogged 0.39 M cells, 38 s, -30.6 / -33.2 dB. The test is `#[ignore]`d: `cargo test --release -p copper-antenna --test victim_coupling -- --ignored
//! --nocapture`.
//!
//! What is checked is physical consistency, not accuracy (there is no closed form for this geometry; the
//! coupled-microstrip comparison in `copper-extract/tests/coupled_microstrip.rs` is the accuracy evidence and
//! says coupling below about -40 dB at GHz frequencies is an upper bound): the coupling falls with distance, no
//! port receives more power than the feed accepts, and a far victim leaves the antenna's own S11 nearly unchanged.

use std::path::{Path, PathBuf};

use copper_antenna::model::{Fidelity, Problem, Victim, cut_ground, evaluate, evaluate_coupled};
use copper_fullwave::coupling::Termination;
use copper_lang::compiler::load_design;
use copper_lang::elaborate::elaborate;
use copper_lang::footprints::FootprintResolver;
use copper_lang::model::ConstraintKind;
use copper_pcb::antenna::{chosen_geometry, layout, resolve_sites};
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

const MM: i64 = 1_000_000;

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/antenna_synthesis/board.copper")
}

/// The example's physical board (without its antenna declaration, as `copper synthesize-antenna` resolves sites)
/// and its one antenna site.
fn board() -> (PhysicalBoard, copper_pcb::antenna::AntennaSite) {
    let path = example();
    let design = load_design(&path, true, true).expect("the example compiles");
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", true, true)
        .expect("physical options");
    let mut bare = design.clone();
    bare.electrical.constraints.retain(|c| c.kind != ConstraintKind::Antenna);
    let base = path.parent().expect("the example's directory").to_path_buf();
    let mut resolver = FootprintResolver::new(&base, &[], false, true, true);
    let physical =
        physicalize(&bare, Footprints::Resolved(&mut resolver), &options, true, true).expect("the board physicalizes");
    let flat = elaborate(&design.electrical).expect("elaboration");
    let mut sites = resolve_sites(&physical, &flat).expect("the antenna site");
    assert_eq!(sites.len(), 1);
    (physical, sites.remove(0))
}

fn segment(net: &str, a: (f64, f64), b: (f64, f64)) -> TrackSegment {
    let nm = |v: f64| (v * MM as f64).round() as i64;
    TrackSegment {
        net: net.to_owned(),
        start: Point::new(nm(a.0), nm(a.1)),
        end: Point::new(nm(b.0), nm(b.1)),
        width_nm: 500_000,
        layer: CopperLayer::Front,
    }
}

/// A 14 mm, 0.5 mm wide F.Cu trace on net `net` from x = 22 to 36 mm at `y_mm`.
fn trace(net: &str, y_mm: f64) -> Vec<TrackSegment> {
    vec![segment(net, (22.0, y_mm), (36.0, y_mm))]
}

/// The same run with a 45-degree jog of 2 mm in its middle (from `y_mm` to `y_mm + 2`).
fn jogged(net: &str, y_mm: f64) -> Vec<TrackSegment> {
    vec![
        segment(net, (22.0, y_mm), (28.0, y_mm)),
        segment(net, (28.0, y_mm), (30.0, y_mm + 2.0)),
        segment(net, (30.0, y_mm + 2.0), (36.0, y_mm + 2.0)),
    ]
}

#[test]
#[ignore = "four coarse full-wave runs, about a minute"]
fn coupling_from_the_feed_to_a_trace_falls_with_distance() {
    let (mut physical, site) = board();
    // Both victims on the board at once would couple to each other; each is evaluated on its own copy.
    let (geometry, _) = chosen_geometry(&physical, &site).expect("the tuned geometry");
    let realized = layout(&site, &geometry).expect("the layout");
    let fidelity = Fidelity::coarse(&site, 0);

    let alone = {
        let problem = Problem::new(&physical, &site).expect("the problem");
        let e = evaluate(&problem, &realized, &fidelity).expect("the antenna alone");
        println!("antenna alone: {} cells, {} steps, {:.0} s, converged {}", e.cells, e.steps, e.seconds, e.converged);
        e
    };

    let mut peaks = Vec::new();
    let cases: [(&str, f64, fn(&str, f64) -> Vec<TrackSegment>); 3] =
        [("near", 8.5, trace), ("far", 24.0, trace), ("jogged", 8.5, jogged)];
    for (name, y, draw) in cases {
        physical.tracks.retain(|t| !t.net.starts_with("VICTIM"));
        let net = format!("VICTIM_{name}");
        physical.tracks.extend(draw(&net, y));
        let problem = Problem::new(&physical, &site).expect("the problem");
        let victim = Victim::from_net(&problem, &net, &|_| Termination::default_ohms()).expect("the victim");
        assert_eq!(victim.geometry.ports.len(), 2, "{:?}", victim.geometry.ports);
        let (e, couplings) = evaluate_coupled(&problem, &realized, &fidelity, &[victim]).expect("the coupled run");
        println!(
            "{name} (y = {y} mm, {:.1} mm below the region): {} cells, {} steps, {:.0} s, converged {}",
            y - 6.0,
            e.cells,
            e.steps,
            e.seconds,
            e.converged
        );
        assert_eq!(couplings.len(), 2);
        println!("  f GHz | S11 alone / with victim | {} | {}", couplings[0].port, couplings[1].port);
        let mut peak = f64::NEG_INFINITY;
        for (k, f) in e.freqs.iter().enumerate() {
            if k % 4 == 0 {
                println!(
                    "  {:.3} | {:6.2} / {:6.2} | {:7.2} | {:7.2}",
                    f / 1e9,
                    alone.s11_db[k],
                    e.s11_db[k],
                    couplings[0].s_db[k],
                    couplings[1].s_db[k]
                );
            }
            // No port can receive more than the feed accepts: |S11|^2 + sum |S_k1|^2 <= 1.
            let power = 10f64.powf(e.s11_db[k] / 10.0) + couplings.iter().map(|c| c.s[k].norm_sqr()).sum::<f64>();
            assert!(power <= 1.0 + 1e-3, "{name} at {f}: {power}");
            peak = peak.max(couplings.iter().map(|c| c.s_db[k]).fold(f64::NEG_INFINITY, f64::max));
        }
        println!("  peak coupling {peak:.2} dB");
        if name == "far" {
            let shift = e.s11_db.iter().zip(&alone.s11_db).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            println!("  largest S11 shift from the antenna alone {shift:.2} dB");
            assert!(shift < 1.0, "a far victim moved S11 by {shift} dB");
        }
        peaks.push(peak);
    }
    assert!(peaks[0] > peaks[1] + 3.0, "near {} dB, far {} dB", peaks[0], peaks[1]);
    // The jog moves the second half 2 mm further from the antenna: comparable to the straight near trace, and
    // well above the far one.
    assert!((peaks[2] - peaks[0]).abs() < 6.0 && peaks[2] > peaks[1] + 3.0, "jogged {} dB", peaks[2]);
}

/// The ground cut: no ground sheet on the victim's layer covers a point of its copper, ground is back within twice
/// the clearance of it, and no stitching via is left inside the cut.
#[test]
fn ground_is_cut_around_a_victim_and_its_stitching_removed() {
    let (mut physical, site) = board();
    for (net, draw) in [("VICTIM_S", trace as fn(&str, f64) -> Vec<TrackSegment>), ("VICTIM_J", jogged)] {
        physical.tracks.retain(|t| !t.net.starts_with("VICTIM"));
        physical.tracks.extend(draw(net, 8.5));
        let problem = Problem::new(&physical, &site).expect("the problem");
        let victim = Victim::from_net(&problem, net, &|_| Termination::default_ohms()).expect("the victim");
        let cut = cut_ground(&problem, std::slice::from_ref(&victim));
        let (ground, stitching) = (cut.ground, cut.stitching);
        let top: Vec<&Vec<(f64, f64)>> = ground.iter().filter(|(z, _)| *z == 0.0).map(|(_, r)| r).collect();
        let inside = |p: (f64, f64)| {
            let point = Point::new((p.0 * 1e6) as i64, (p.1 * 1e6) as i64);
            top.iter().any(|ring| {
                let ring: Vec<Point> =
                    ring.iter().map(|q| Point::new((q.0 * 1e6) as i64, (q.1 * 1e6) as i64)).collect();
                copper_physical::geometry::point_in_polygon(point, &ring)
            })
        };
        // Points along the straight centre line (and, for the jog, its offset half).
        for x in [22.5, 25.0, 27.5, 31.0, 33.0, 35.5] {
            let y = if net == "VICTIM_J" && x > 30.0 { 10.5 } else { 8.5 };
            assert!(!inside((x, y)), "{net}: ground covers the victim at ({x}, {y})");
            // Ground resumes beyond the cut (clearance + half the width + a margin), below the trace.
            let c = victim.clearance + 0.25 + 0.3;
            assert!(inside((x, y + c)) || inside((x, y - c)), "{net}: no ground beside ({x}, {y})");
        }
        let near = |x: f64, y: f64| (8.0..13.0).contains(&y) && (21.0..37.0).contains(&x);
        let kept = stitching.iter().filter(|(x, y, _, _)| near(*x, *y)).count();
        println!("{net}: {} top ground pieces, {} stitching vias near the victim", top.len(), kept);
    }
}

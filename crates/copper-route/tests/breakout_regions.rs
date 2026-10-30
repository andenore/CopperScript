//! Breakout width, gap and clearance apply only near terminal lands (D-PHY
//! plan R1), ported from `tests/test_breakout_regions.py`. The
//! `_route_single`/`_route_pair` + `_validate_candidate` cases are in
//! `src/test_support_planes.rs`.

mod support_planes;

use std::collections::BTreeSet;

use copper_drc::breakout::BreakoutRegions;
use copper_drc::{Policy, Report};
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_physical::geometry::{RoundedConvexShape, shape_distance_squared};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::critical::{CriticalOptions, CriticalRoutingResult, critical_net_document, route_critical_nets};
use copper_route::drc_report::fingerprint;
use copper_route::pair_search::{legal, tracks as lane_tracks};
use num_traits::ToPrimitive;
use support_planes::breakout::{Row, test_point};
use support_planes::*;

const FRONT: CopperLayer = CopperLayer::Front;

fn route(board: &PhysicalBoard) -> CriticalRoutingResult {
    let options = CriticalOptions { pair_state_limit: Some(20_000), ..CriticalOptions::default() };
    route_critical_nets(board.clone(), &global(board), &options, None).expect("critical")
}

fn drc(board: &PhysicalBoard) -> Report {
    copper_drc::run_physical_drc(board, Policy::default()).expect("drc")
}

/// `_test_point_spacing`: least edge-to-edge distance from the tracks to the test point.
fn test_point_spacing(tracks: &[TrackSegment]) -> f64 {
    let pad = RoundedConvexShape::new(vec![test_point()], nm(0.25)).unwrap();
    tracks
        .iter()
        .map(|track| {
            let spine = RoundedConvexShape::new(vec![track.start, track.end], 0).unwrap();
            shape_distance_squared(&spine, &pad).to_f64().unwrap().sqrt()
                - track.width_nm as f64 / 2.0
                - nm(0.25) as f64
        })
        .fold(f64::INFINITY, f64::min)
}

/// `_codes`: finding codes, of findings on any of `nets` when given.
fn codes(report: &Report, nets: &[&str]) -> BTreeSet<String> {
    report
        .findings
        .iter()
        .filter(|finding| nets.is_empty() || finding.nets.iter().any(|net| nets.contains(&net.as_str())))
        .map(|finding| finding.code.clone())
        .collect()
}

fn none_of(found: &BTreeSet<String>, unwanted: &[&str]) -> bool {
    !unwanted.iter().any(|code| found.contains(*code))
}

fn with_copper(board: &PhysicalBoard, tracks: Vec<TrackSegment>, vias: Vec<Via>) -> PhysicalBoard {
    let mut board = board.clone();
    board.tracks = tracks;
    board.vias = vias;
    board
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn land(name: &str) -> Option<String> {
    Some(name.to_owned())
}

// --- the region ------------------------------------------------------------

#[test]
fn the_region_is_each_terminal_land_swept_by_the_breakout_length() {
    let regions = BreakoutRegions::new(&Row::default().build());
    assert!(regions.any());
    assert_eq!(
        regions.declared.iter().cloned().collect::<BTreeSet<_>>(),
        ["LANE_N", "LANE_P"].map(String::from).into()
    );
    // U1.3 spans x 9.1..9.7 mm; 1 mm beyond its edge is the region boundary.
    assert_eq!(regions.region("LANE_P", &[mm(9.4, 9.75), mm(10.7, 9.75)]), land("pad:U1.3:2"));
    assert_eq!(regions.region("LANE_P", &[Point::new(nm(10.7) + 1, nm(9.75))]), None);
    // Plan-view distance from the land outline, not from its centre.
    assert_eq!(regions.region("LANE_P", &[mm(9.4, 10.85)]), land("pad:U1.3:2"));
    assert_eq!(regions.region("LANE_P", &[mm(9.4, 10.9)]), None);
    assert_eq!(regions.region("AUX", &[mm(9.4, 9.75)]), None);
    // A crossing segment is cut at the last integer step inside the region.
    let pieces = regions.split("LANE_P", mm(9.4, 9.75), mm(15.0, 9.75));
    assert_eq!(pieces, [(mm(9.4, 9.75), mm(10.7, 9.75), land("pad:U1.3:2")), (mm(10.7, 9.75), mm(15.0, 9.75), None)]);
    assert_eq!(regions.split("LANE_P", pieces[0].0, pieces[0].1), [pieces[0].clone()]);
    assert_eq!(regions.split("LANE_P", pieces[1].0, pieces[1].1), [pieces[1].clone()]);
    // Both regions of the net: three pieces; a 45-degree segment is cut too.
    let across: Vec<(i64, i64, Option<String>)> = regions
        .split("LANE_P", mm(9.4, 9.75), mm(20.6, 9.75))
        .into_iter()
        .map(|(a, b, land)| (a.x_nm, b.x_nm, land))
        .collect();
    assert_eq!(
        across,
        [(nm(9.4), nm(10.7), land("pad:U1.3:2")), (nm(10.7), nm(19.3), None), (nm(19.3), nm(20.6), land("pad:J1.3:2"))]
    );
    let diagonal = regions.split("LANE_P", mm(9.7, 9.75), mm(11.7, 11.75));
    assert!(diagonal.len() == 2 && diagonal[0].2 == land("pad:U1.3:2") && diagonal[1].2.is_none(), "{diagonal:?}");
    // Any other direction is not cut; it is outside unless wholly inside.
    let oblique = regions.split("LANE_P", mm(9.7, 9.75), mm(11.7, 10.75));
    assert_eq!(oblique, [(mm(9.7, 9.75), mm(11.7, 10.75), None)]);
    assert!(!BreakoutRegions::new(&Row { breakout: None, ..Row::default() }.build()).any());
}

#[test]
fn the_routers_split_tracks_and_carry_the_breakout_width_inside() {
    let regions = BreakoutRegions::new(&Row { neck: true, ..Row::default() }.build());
    let lane = track("LANE_P", mm(9.4, 9.75), mm(15.0, 9.75), nm(0.15), FRONT);
    let pieces = regions.split_tracks(std::slice::from_ref(&lane), Some(nm(0.15)));
    assert_eq!(pieces.len(), 2);
    let (inside, outside) = (&pieces[0], &pieces[1]);
    assert_eq!((inside.end, inside.width_nm), (mm(10.7, 9.75), nm(0.12)));
    assert_eq!((outside.start, outside.width_nm), (mm(10.7, 9.75), nm(0.15)));
    // Idempotent, and a necked piece moved outside gets the profile width back.
    assert_eq!(regions.split_tracks(&pieces, Some(nm(0.15))), pieces);
    let necked = TrackSegment { width_nm: nm(0.12), ..outside.clone() };
    assert_eq!(regions.split_tracks(&[necked], Some(nm(0.15))), std::slice::from_ref(outside));
    let foreign = TrackSegment { net: "AUX".to_owned(), ..lane };
    assert_eq!(regions.split_tracks(std::slice::from_ref(&foreign), None), [foreign]);
}

// --- routing ---------------------------------------------------------------

const HARD: [&str; 4] = ["DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH", "DRC-OPEN-NET"];

#[test]
fn a_pair_escapes_the_pin_field_only_with_breakout_values() {
    let routed = route(&Row::default().build());
    let pair = &routed.nets[0];
    assert!(pair.connected, "{:?}", pair.diagnostics);
    assert!(pair.strategy.starts_with("joint_pair"), "{}", pair.strategy);
    // The pin field passes at the breakout clearance; the channel keeps the
    // full 0.5 mm from the test point by detouring around it.
    assert!(test_point_spacing(&routed.locked_tracks) >= nm(0.5) as f64 - 1e-6);
    let report = drc(&routed.board);
    assert!(none_of(&codes(&report, &[]), &HARD), "{:?}", codes(&report, &[]));
    let lands: BTreeSet<(String, String)> = report
        .breakout_relaxations
        .iter()
        .flat_map(|item| item.regions.iter().map(|(net, land, _)| (net.clone(), land.clone())))
        .collect();
    let expected: BTreeSet<(String, String)> =
        [("LANE_P", "pad:U1.3:2"), ("LANE_P", "pad:J1.3:2"), ("LANE_N", "pad:U1.4:3"), ("LANE_N", "pad:J1.4:3")]
            .into_iter()
            .map(|(net, land)| (net.to_owned(), land.to_owned()))
            .collect();
    assert_eq!(lands, expected);
    // Clearance 0.5 mm everywhere: the pair cannot leave the pin field.
    assert!(!route(&Row { breakout: None, ..Row::default() }.build()).nets[0].connected);
    // A region too short to clear the neighbouring lands does not help either.
    assert!(!route(&Row { breakout: Some((0.2, 0.15)), ..Row::default() }.build()).nets[0].connected);
    // Clearance 0.15 mm everywhere routes straight past the test point,
    // 0.3 mm away, violating the intended channel clearance.
    let loose = route(&Row { clearance: 0.15, breakout: None, ..Row::default() }.build());
    assert!(loose.nets[0].connected && loose.nets[0].strategy == "aligned_pair", "{}", loose.nets[0].strategy);
    assert_eq!(test_point_spacing(&loose.locked_tracks).round() as i64, nm(0.3));
}

#[test]
fn the_routed_pair_necks_down_only_inside_the_region() {
    let board = Row { neck: true, ..Row::default() }.build();
    let routed = route(&board);
    assert!(routed.nets[0].connected, "{:?}", routed.nets[0].diagnostics);
    let regions = BreakoutRegions::new(&board);
    let widths: BTreeSet<(bool, i64)> = routed
        .locked_tracks
        .iter()
        .map(|t| (regions.region(&t.net, &[t.start, t.end]).is_some(), t.width_nm))
        .collect();
    assert_eq!(widths, [(true, nm(0.12)), (false, nm(0.15))].into());
    let report = drc(&routed.board);
    assert!(none_of(&codes(&report, &[]), &HARD), "{:?}", codes(&report, &[]));
    let necked: Vec<_> = report.breakout_relaxations.iter().filter(|item| item.check == "track_width").collect();
    assert!(!necked.is_empty());
    for item in necked {
        assert_eq!((item.required_nm, item.relaxed_nm, item.measured_nm), (nm(0.15), nm(0.12), Some(nm(0.12))));
    }
}

#[test]
fn pair_search_spacing_is_region_aware() {
    let board = Row { neck: true, obstacle: false, ..Row::default() }.build();
    let mut index = RoutingClearanceIndex::new(&board).unwrap();
    let (width, layer) = (nm(0.15), FRONT);
    let clearance = board.rules.minimum_clearance_nm;
    let lanes = |index: &RoutingClearanceIndex<'_>, x0: f64, x1: f64, half_pitch: f64| {
        let offset = nm(half_pitch);
        let spine = [mm(x0, 10.0), mm(x1, 10.0)];
        let shifted = |dy: i64| spine.iter().map(|p| Point::new(p.x_nm, p.y_nm + dy)).collect::<Vec<_>>();
        (
            lane_tracks("LANE_P", &shifted(-offset), width, layer, Some(&index.breakout)),
            lane_tracks("LANE_N", &shifted(offset), width, layer, Some(&index.breakout)),
        )
    };
    // Lanes 0.35 mm apart centre to centre. Next to the lands they neck to
    // 0.12 mm; their 0.23 mm gap needs the 0.2 mm breakout gap...
    let near = lanes(&index, 10.0, 10.6, 0.175);
    assert!(near.0.iter().chain(&near.1).all(|t| t.width_nm == nm(0.12)));
    assert!(legal(&board, &index, &near.0, &near.1, clearance).unwrap());
    // ...but in the channel, at 0.15 mm, their 0.2 mm gap is below the pair gap.
    let far = lanes(&index, 12.0, 12.6, 0.175);
    assert!(!legal(&board, &index, &far.0, &far.1, clearance).unwrap());
    let wide = lanes(&index, 12.0, 12.6, 0.2);
    assert!(legal(&board, &index, &wide.0, &wide.1, clearance).unwrap());
    // A foreign track 0.2 mm from a lane: legal only inside the region.
    assert!(index.can_track("LANE_P", mm(10.0, 9.5), mm(10.6, 9.5), width, layer).unwrap());
    index.add_track(track("AUX", mm(10.0, 9.15), mm(13.0, 9.15), width, layer), false, None).unwrap();
    assert!(index.can_track("LANE_P", mm(10.0, 9.5), mm(10.6, 9.5), width, layer).unwrap());
    assert!(!index.can_track("LANE_P", mm(11.0, 9.5), mm(11.6, 9.5), width, layer).unwrap());
}

// --- physical DRC ----------------------------------------------------------

/// `_lane_and_via`: a LANE_P track from its U1 land with an AUX via 0.2 mm from its edge.
fn lane_and_via(end_x_nm: i64, split: bool) -> (Vec<TrackSegment>, Vec<Via>) {
    let (start, boundary) = (mm(9.4, 9.75), mm(10.7, 9.75));
    let end = Point::new(end_x_nm, boundary.y_nm);
    let width = nm(0.15);
    let tracks = if split {
        vec![track("LANE_P", start, boundary, width, FRONT), track("LANE_P", boundary, end, width, FRONT)]
    } else {
        vec![track("LANE_P", start, end, width, FRONT)]
    };
    let tracks = tracks.into_iter().filter(|t| t.start != t.end).collect();
    (tracks, vec![via("AUX", mm(10.1, 9.325), nm(0.3), nm(0.15))])
}

fn clearance_or_short(report: &Report) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|f| f.code == "DRC-CLEARANCE" || f.code == "DRC-SHORT")
        .map(|f| f.message.clone())
        .collect()
}

#[test]
fn drc_relaxes_clearance_only_inside_the_region_and_records_why() {
    let board = Row { obstacle: false, ..Row::default() }.build();
    let edge = nm(10.7);
    let (tracks, vias) = lane_and_via(edge, false);
    let inside_board = with_copper(&board, tracks, vias);
    let inside = drc(&inside_board);
    assert_eq!(clearance_or_short(&inside), Vec::<String>::new());
    let relaxed: Vec<_> =
        inside.breakout_relaxations.iter().filter(|item| item.objects == strings(&["track:0", "via:0"])).collect();
    assert_eq!(relaxed.len(), 1);
    let relaxation = relaxed[0];
    assert_eq!(
        (relaxation.check.as_str(), &relaxation.nets, &relaxation.layers),
        ("clearance", &strings(&["AUX", "LANE_P"]), &strings(&["F.Cu"]))
    );
    assert_eq!(
        (relaxation.required_nm, relaxation.relaxed_nm, relaxation.measured_nm),
        (nm(0.5), nm(0.15), Some(nm(0.2)))
    );
    assert_eq!(relaxation.regions, [("LANE_P".to_owned(), "pad:U1.3:2".to_owned(), nm(1.0))]);
    let document = copper_route::drc_report::run_physical_drc(&inside_board, Policy::default()).unwrap().to_json();
    assert!(document.contains("\"breakout_relaxations\"") && document.contains("\"land\": \"pad:U1.3:2\""));
    assert!(inside.coverage.iter().any(|item| item.check == "breakout_regions"));
    // The same copper reaching 1 nm past the boundary is outside the region.
    let (tracks, vias) = lane_and_via(edge + 1, false);
    let outside = drc(&with_copper(&board, tracks, vias));
    let found: Vec<_> = outside.findings.iter().filter(|f| f.objects == strings(&["track:0", "via:0"])).collect();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].code.as_str(), &found[0].nets), ("DRC-CLEARANCE", &strings(&["AUX", "LANE_P"])));
    // The lands next to it now need the full 0.5 mm as well.
    assert!(
        outside.findings.iter().any(|f| f.code == "DRC-CLEARANCE" && f.objects == strings(&["track:0", "pad:U1.2:1"]))
    );
    // Cut at the boundary, the part next to the land is inside again.
    let (tracks, vias) = lane_and_via(edge + 1, true);
    assert_eq!(clearance_or_short(&drc(&with_copper(&board, tracks, vias))), Vec::<String>::new());
}

#[test]
fn drc_accepts_a_neck_down_inside_the_region_and_rejects_it_outside() {
    let board = Row { neck: true, obstacle: false, ..Row::default() }.build();
    let (narrow, width) = (nm(0.12), nm(0.15));
    let neck = track("LANE_P", mm(9.4, 9.75), mm(10.2, 9.75), narrow, FRONT);
    let report = drc(&with_copper(&board, vec![neck.clone()], Vec::new()));
    assert!(!codes(&report, &[]).contains("DRC-TRACK-WIDTH"));
    let relaxed: Vec<_> = report
        .breakout_relaxations
        .iter()
        .filter(|item| item.check == "track_width")
        .map(|item| (item.required_nm, item.relaxed_nm, item.regions.clone()))
        .collect();
    assert_eq!(relaxed, [(width, narrow, vec![("LANE_P".to_owned(), "pad:U1.3:2".to_owned(), nm(1.0))])]);
    let long_neck = TrackSegment { end: mm(10.8, 9.75), ..neck };
    let long = drc(&with_copper(&board, vec![long_neck], Vec::new()));
    let found: Vec<_> = long.findings.iter().filter(|f| f.code == "DRC-TRACK-WIDTH").collect();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].required_nm, found[0].measured_nm), (Some(width), Some(narrow)));

    // Pair members 0.2 mm apart: the breakout gap inside, the pair gap outside.
    let pair = |x0: f64, x1: f64| {
        vec![
            track("LANE_P", mm(x0, 9.825), mm(x1, 9.825), width, FRONT),
            track("LANE_N", mm(x0, 10.175), mm(x1, 10.175), width, FRONT),
        ]
    };
    let near = drc(&with_copper(&board, pair(10.0, 10.6), Vec::new()));
    assert_eq!(clearance_or_short(&near), Vec::<String>::new());
    let gaps: Vec<_> =
        near.breakout_relaxations.iter().filter(|item| item.objects == strings(&["track:0", "track:1"])).collect();
    assert_eq!(gaps.len(), 1);
    let gap = gaps[0];
    assert_eq!(
        (gap.check.as_str(), gap.required_nm, gap.relaxed_nm, gap.measured_nm),
        ("pair_gap", nm(0.25), nm(0.2), Some(nm(0.2)))
    );
    assert_eq!(
        gap.regions,
        [
            ("LANE_N".to_owned(), "pad:U1.4:3".to_owned(), nm(1.0)),
            ("LANE_P".to_owned(), "pad:U1.3:2".to_owned(), nm(1.0))
        ]
    );
    let far = drc(&with_copper(&board, pair(12.0, 12.6), Vec::new()));
    let found: Vec<_> = far.findings.iter().filter(|f| f.objects == strings(&["track:0", "track:1"])).collect();
    assert_eq!(found.len(), 1);
    assert!(found[0].code == "DRC-CLEARANCE" && found[0].message.contains("pair gap"), "{}", found[0].message);
    // Centre distance: width + pair gap.
    assert_eq!(found[0].required_nm, Some(nm(0.4)));
    // The coupled pair gap itself is legal in the channel, below the clearance.
    let coupled = vec![
        track("LANE_P", mm(12.0, 9.8), mm(12.6, 9.8), width, FRONT),
        track("LANE_N", mm(12.0, 10.2), mm(12.6, 10.2), width, FRONT),
    ];
    assert_eq!(clearance_or_short(&drc(&with_copper(&board, coupled, Vec::new()))), Vec::<String>::new());
}

// --- regression and determinism ---------------------------------------------

#[test]
fn nets_without_breakout_properties_are_unaffected() {
    let plain = Row { clearance: 0.15, breakout: None, ..Row::default() }.build();
    let routed = route(&plain).board;
    let report = drc(&routed);
    assert!(report.breakout_relaxations.is_empty());
    let document = copper_route::drc_report::run_physical_drc(&routed, Policy::default()).unwrap().to_json();
    assert!(!document.contains("breakout"));
    assert!(!report.coverage.iter().any(|item| item.check == "breakout_regions"));
    // A pair without breakout properties keeps the plain clearance rule
    // between its members, beside a pair that has them.
    let width = nm(0.15);
    let aux = |name: &str, partner: &str| NetRoutingRule {
        kind: RouteKind::Differential,
        differential_partner: Some(partner.to_owned()),
        width_nm: Some(width),
        pair_gap_nm: Some(nm(0.25)),
        clearance_nm: Some(nm(0.5)),
        ..rule(name)
    };
    let copper = vec![
        track("AUX_P", mm(12.0, 3.0), mm(14.0, 3.0), width, FRONT),
        track("AUX_N", mm(12.0, 3.4), mm(14.0, 3.4), width, FRONT),
        track("LANE_P", mm(9.4, 9.75), mm(10.2, 9.75), width, FRONT),
    ];
    let aux_findings = |mut board: PhysicalBoard| -> Vec<String> {
        board.nets.extend([net("AUX_P", &[]), net("AUX_N", &[])]);
        board.net_routing_rules.extend([aux("AUX_P", "AUX_N"), aux("AUX_N", "AUX_P")]);
        board.tracks = copper.clone();
        drc(&board)
            .findings
            .iter()
            .filter(|f| {
                f.nets.iter().any(|net| net == "AUX_P" || net == "AUX_N")
                    && (f.code == "DRC-CLEARANCE" || f.code == "DRC-SHORT")
            })
            .map(fingerprint)
            .collect()
    };
    let with_breakout = aux_findings(Row { obstacle: false, ..Row::default() }.build());
    assert_eq!(with_breakout, aux_findings(Row { obstacle: false, breakout: None, ..Row::default() }.build()));
    // The 0.25 mm members still violate their own 0.5 mm clearance.
    assert_eq!(with_breakout.len(), 1);
    assert!(!RoutingClearanceIndex::new(&plain).unwrap().breakout.any());
}

#[test]
fn breakout_routing_and_drc_are_deterministic() {
    let board = Row { neck: true, ..Row::default() }.build();
    let (first, second) = (route(&board), route(&board));
    assert_eq!(first.board, second.board);
    assert_eq!(first.locked_tracks, second.locked_tracks);
    let docs = |result: &CriticalRoutingResult| result.nets.iter().map(critical_net_document).collect::<Vec<_>>();
    assert_eq!(docs(&first), docs(&second));
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert_eq!(first.routing_fingerprint, second.routing_fingerprint);
    let reports = [&first.board, &second.board]
        .map(|board| copper_route::drc_report::run_physical_drc(board, Policy::default()).unwrap());
    assert_eq!(reports[0].to_json(), reports[1].to_json());
    assert_eq!(reports[0].report.breakout_relaxations, reports[1].report.breakout_relaxations);
}

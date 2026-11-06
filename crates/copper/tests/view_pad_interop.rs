//! A back-side pad must occupy the same copper in saved-state and native views.
use copper_json::Value;
use copper_lang::compiler::compile_design_source;
use copper_pcb::board::TrackSegment;
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_physical::footprint::{PadShape, Size};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_report::scene::{Scene, ScenePad, scene_from_kicad_text, scene_from_physical};
use copper_report::view::{ViewRequest, net_colors, svg};
use copper_units::Decimal;

#[test]
fn rotated_back_pad_matches_exported_scene_and_stays_in_its_layer_panel() {
    let design = compile_design_source(
        "board ViewPadInterop {
            use library \"tiny\";
            component R1: RESISTOR { footprint = \"0402\"; }
            net SIGNAL { R1.1; R1.2; }
        }",
        "view-pad-interop.copper",
        false,
        true,
    )
    .unwrap();
    let options = PhysicalOptions::new(40.0, 30.0, 2, "generic").unwrap();
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, true).unwrap();
    let pose = &mut board.placements[0];
    pose.position = Point::new(20_000_000, 15_000_000);
    pose.rotation_degrees = Decimal::from_i64(37);
    pose.side = BoardSide::Back;
    let footprint_name = pose.footprint.clone();
    let pad = board.footprints.get_mut(&footprint_name).unwrap().pads.iter_mut().find(|p| p.number == "1").unwrap();
    // Nonzero local offset catches an extra mirror; non-square copper at a
    // non-cardinal local angle distinguishes placement-pad from placement+pad.
    pad.position = Point::new(1_250_000, -700_000);
    pad.shape = PadShape::Rectangle;
    pad.size = Size::new(2_000_000, 1_000_000).unwrap();
    pad.rotation_degrees = Decimal::from_i64(30);
    pad.roundrect_ratio_ppm = 0;
    board.tracks.push(TrackSegment {
        net: "SIGNAL".to_owned(),
        start: Point::new(5_000_000, 15_000_000),
        end: Point::new(35_125_000, 15_000_000),
        width_nm: 200_000,
        layer: CopperLayer::Front,
    });

    let physical = scene_from_physical(&copper_route::state_codec::encode_board(&board)).unwrap();
    let manifest = copper_pcb::kicad_pcb::generate(&board).unwrap();
    let pcb = manifest.artifacts.iter().find(|file| file.name.ends_with(".kicad_pcb")).unwrap();
    let native = scene_from_kicad_text(&pcb.content, &pcb.name).unwrap();
    fn target(scene: &Scene) -> &ScenePad {
        scene.pads.iter().find(|pad| pad.reference == "R1" && pad.pad == "1").unwrap()
    }
    let (ours, exported) = (target(&physical), target(&native));
    assert_eq!(ours.layers, vec!["B.Cu"]);
    assert_eq!(ours.layers, exported.layers);
    // Integer-nanometre physical transforms round the centre and offsets;
    // the native reader retains floating-point coordinates from the export.
    let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) <= 0.000002;
    assert!(close(ours.center, exported.center), "{:?} != {:?}", ours.center, exported.center);
    assert_eq!(ours.polygon.len(), 4);
    assert_eq!(exported.polygon.len(), 4);
    for corner in &ours.polygon {
        assert!(
            exported.polygon.iter().any(|other| close(*corner, *other)),
            "{:?} != {:?}",
            ours.polygon,
            exported.polygon
        );
    }

    let request = ViewRequest {
        window: [15.0, 10.0, 25.0, 20.0],
        layers: vec!["F.Cu".to_owned(), "B.Cu".to_owned()],
        highlight: vec![Value::from("SIGNAL")],
        markers: Vec::new(),
    };
    let colors = net_colors(&request.highlight).unwrap();
    for scene in [&physical, &native] {
        let drawing = svg(scene, &request, &colors, None);
        let panels: Vec<_> = drawing
            .split("<svg class=\"layer-viewport\"")
            .skip(1)
            .map(|tail| tail.split_once("</svg>").unwrap().0)
            .collect();
        assert_eq!(panels.len(), 2);
        assert!(!panels[0].contains("<polygon"), "back-side pads leaked into the front panel");
        assert_eq!(panels[1].matches("<polygon").count(), scene.pads.len());
        let front_track = "x1=\"5.0\" y1=\"15.0\" x2=\"35.125\"";
        assert!(panels[0].contains(front_track));
        assert!(!panels[1].contains(front_track), "front track leaked into the back panel");
    }
}

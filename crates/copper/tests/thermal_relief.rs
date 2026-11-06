//! Copper zones join same-net pads through thermal reliefs by default; the
//! relief gap and spoke width are configurable, and footprint pad
//! `zone_connect` overrides keep KiCad's numbering.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A two-land footprint: pad 1 overrides the zone connection with KiCad
/// `zone_connect` `code`, pad 2 inherits the zone's setting.
fn lands(code: u8) -> String {
    format!(
        r#"(footprint "Thermal_{code}"
  (version 20240108)
  (generator "CopperScript-test")
  (layer "F.Cu")
  (attr smd)
  (fp_rect (start -2 -1) (end 2 1) (stroke (width 0.05) (type default))
    (fill none) (layer "F.CrtYd"))
  (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu" "F.Paste" "F.Mask") (zone_connect {code}))
  (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu" "F.Paste" "F.Mask"))
)
"#
    )
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

/// A board whose GND pads, one footprint per `zone_connect` code, all sit in
/// one `F.Cu` GND zone declared with `zone`.
fn project(name: &str, zone: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("thermal-relief").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    for code in 0..4 {
        std::fs::write(directory.join(format!("Thermal_{code}.kicad_mod")), lands(code)).unwrap();
    }
    let source = directory.join("board.copper");
    std::fs::write(
        &source,
        format!(
            r#"board ThermalBoard {{
    use library "tiny";
    component C0: CAPACITOR {{ value = 1uF; footprint = "Thermal_0.kicad_mod"; }}
    component C1: CAPACITOR {{ value = 1uF; footprint = "Thermal_1.kicad_mod"; }}
    component C2: CAPACITOR {{ value = 1uF; footprint = "Thermal_2.kicad_mod"; }}
    component C3: CAPACITOR {{ value = 1uF; footprint = "Thermal_3.kicad_mod"; }}
    net GND {{ C0.1; C0.2; C1.1; C1.2; C2.1; C2.2; C3.1; C3.2; }}
    constraint fixed_placement(C0) {{ x = 16mm; y = 15mm; rotation = 0; }}
    constraint fixed_placement(C1) {{ x = 22mm; y = 15mm; rotation = 0; }}
    constraint fixed_placement(C2) {{ x = 28mm; y = 15mm; rotation = 0; }}
    constraint fixed_placement(C3) {{ x = 34mm; y = 15mm; rotation = 0; }}
    constraint copper_zone(GND) {{ layers = "F.Cu"; {zone} }}
}}
"#
        ),
    )
    .unwrap();
    source
}

fn copper(arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs")
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

/// Export `zone` to KiCad; the PCB text, or the failure output.
fn export(name: &str, zone: &str) -> Result<(PathBuf, String), String> {
    let source = project(name, zone);
    let pcb = source.with_file_name("board.kicad_pcb");
    let output = copper(&[
        "export-kicad-pcb".as_ref(),
        source.as_os_str(),
        "--width-mm".as_ref(),
        "50".as_ref(),
        "--height-mm".as_ref(),
        "30".as_ref(),
        "-o".as_ref(),
        pcb.as_os_str(),
    ]);
    if !output.status.success() {
        return Err(text(&output));
    }
    let board = std::fs::read_to_string(&pcb).unwrap();
    Ok((pcb, board))
}

/// The zone block's connection and fill settings lines.
fn zone_settings(board: &str) -> (String, String) {
    let line = |prefix: &str| {
        board
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix}"))
            .to_owned()
    };
    (line("(connect_pads"), line("(fill yes"))
}

#[test]
fn zones_default_to_thermal_reliefs() {
    let (_, board) = export("default", "").unwrap();
    let (connect, fill) = zone_settings(&board);
    assert!(connect.starts_with("(connect_pads (clearance"), "{connect}");
    assert!(fill.starts_with("(fill yes (thermal_gap 0.3) (thermal_bridge_width 0.3)"), "{fill}");
}

#[test]
fn relief_gap_and_spoke_width_are_configurable() {
    let (_, board) = export("custom", "thermal_gap = 0.25mm; thermal_spoke_width = 0.4mm;").unwrap();
    let (_, fill) = zone_settings(&board);
    assert!(fill.starts_with("(fill yes (thermal_gap 0.25) (thermal_bridge_width 0.4)"), "{fill}");

    // The default spoke width never falls below the zone's minimum width.
    let (_, board) = export("wide", "minimum_width = 0.45mm;").unwrap();
    let (_, fill) = zone_settings(&board);
    assert!(fill.starts_with("(fill yes (thermal_gap 0.3) (thermal_bridge_width 0.45)"), "{fill}");

    let (_, board) = export("solid", "pad_connection = solid;").unwrap();
    assert!(zone_settings(&board).0.starts_with("(connect_pads yes"), "{board}");
}

#[test]
fn unfillable_reliefs_are_rejected() {
    let narrow = export("narrow", "minimum_width = 0.3mm; thermal_spoke_width = 0.2mm;").unwrap_err();
    assert!(narrow.contains("thermal_spoke_width must be at least the zone minimum_width"), "{narrow}");
    let zero = export("zero", "thermal_gap = 0mm;").unwrap_err();
    assert!(zero.contains("thermal_gap and thermal_spoke_width must be positive"), "{zero}");
}

#[test]
fn pad_zone_connect_keeps_kicad_numbering() {
    let (_, board) = export("pads", "").unwrap();
    let codes: Vec<&str> = board
        .lines()
        .filter_map(|line| line.trim().strip_prefix("(zone_connect "))
        .map(|c| c.trim_end_matches(')'))
        .collect();
    assert_eq!(codes, ["0", "1", "2", "3"], "{board}");
}

#[test]
fn native_fill_follows_each_pad_connection() {
    let (pcb, _) = export("fill", "").unwrap();
    let filled = pcb.with_file_name("filled.kicad_pcb");
    let output = copper(&["fill".as_ref(), pcb.as_os_str(), "-o".as_ref(), filled.as_os_str()]);
    assert!(output.status.success(), "{}", text(&output));
    let points = fill_points(&std::fs::read_to_string(&filled).unwrap());
    assert!(!points.is_empty(), "no fill");
    let near = |x: f64, y: f64| points.iter().any(|&(px, py)| (px - x).abs() < 0.06 && (py - y).abs() < 0.06);
    let untouched = |x: f64| !points.iter().any(|&(px, py)| (px - x).abs() < 0.8 && (py - 15.0).abs() < 0.8);
    // 1 mm lands at y = 15 mm. Thermal pads (C0.2 at x = 17 mm inherits the
    // zone's thermal; C1.1 at x = 21 mm sets it) keep a 0.3 mm gap, bridged by
    // 0.3 mm spokes on their axes.
    for x in [17.0, 21.0] {
        assert!(near(x - 0.8, 14.5) && near(x + 0.8, 15.5), "no 0.3 mm relief gap at x = {x}: {points:?}");
        assert!(near(x - 0.15, 15.8) && near(x + 0.15, 15.8), "no 0.3 mm spoke at x = {x}: {points:?}");
    }
    // `zone_connect 0` (C0.1 at x = 15 mm) only keeps the 0.2 mm clearance.
    assert!(near(14.3, 15.54) && !near(15.0 - 0.15, 15.8), "C0.1 is not isolated: {points:?}");
    // Solid (C2.1) and through-hole thermal on an SMD land (C3.1) flood directly.
    assert!(untouched(27.0) && untouched(33.0), "a solid land has a relief: {points:?}");
}

/// Every `(xy X Y)` vertex of the board's filled polygons, in millimetres.
fn fill_points(board: &str) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    for block in board.split("(filled_polygon").skip(1) {
        let block = block.split("(filled_polygon").next().unwrap();
        for vertex in block.split("(xy ").skip(1) {
            let mut numbers = vertex.split([' ', ')']).filter(|s| !s.is_empty());
            if let (Some(x), Some(y)) = (numbers.next(), numbers.next())
                && let (Ok(x), Ok(y)) = (x.parse(), y.parse())
            {
                points.push((x, y));
            }
        }
    }
    points
}

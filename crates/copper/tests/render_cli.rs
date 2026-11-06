//! `copper render` draws a native plot as SVG: routed, filled and plotted
//! from the antenna example, without KiCad.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper")).args(arguments).current_dir(workspace()).output().expect("copper runs")
}

fn passed(output: &Output) -> String {
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "copper failed:\n{text}");
    text
}

#[test]
fn renders_a_native_plot_with_exposed_copper_and_reports_missing_layers() {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-cli");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = |name: &str| directory.join(name).to_string_lossy().into_owned();
    passed(&copper(&[
        "route-board",
        "examples/antenna_synthesis/board.copper",
        "--locked",
        "--offline",
        "-o",
        &path("board.kicad_pcb"),
        "--report",
        &path("route.json"),
    ]));
    passed(&copper(&["fill", &path("board.kicad_pcb"), "-o", &path("filled.kicad_pcb")]));
    passed(&copper(&["plot", &path("filled.kicad_pcb"), "-o", &path("plot")]));

    let text = passed(&copper(&[
        "render",
        &path("plot"),
        "-o",
        &path("top.svg"),
        "--layer",
        "F.Cu",
        "--layer",
        "F.Cu&F.Mask:#d9a441:0.9",
        "--layer",
        "Edge.Cuts",
        "--background",
        "#203028",
        "--title",
        "Antenna probe",
    ]));
    assert!(text.contains("Rendered 3 layer plate(s)"), "{text}");
    let svg = std::fs::read_to_string(path("top.svg")).unwrap();
    // The 40 x 30 mm profile, its 0.05 mm outline stroke and the default
    // 1 mm margin, Y pointing down.
    assert!(
        svg.starts_with(r##"<svg xmlns="http://www.w3.org/2000/svg" width="42.05mm" height="32.05mm" viewBox="-1.025 -1.025 42.05 32.05">"##),
        "{}",
        &svg[..200]
    );
    assert!(svg.contains("<title>Antenna probe</title>"));
    assert_eq!(svg.matches("<mask ").count(), 3, "F.Cu, F.Mask and Edge.Cuts masks");
    assert!(svg.contains(r##"<g data-layer="F.Cu" opacity="0.9"><g mask="url(#m1)">"##));
    assert!(svg.contains(r##"fill="#203028""##));
    // The plotted zone fill and the 1 mm antenna arm are in the copper mask.
    assert!(svg.contains(r##"stroke="#fff" stroke-width="1""##));

    let missing = copper(&["render", &path("plot"), "-o", &path("inner.svg"), "--layer", "In2.Cu"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("RENDER ERROR: no plotted In2.Cu layer"));
    let bad = copper(&["render", &path("plot"), "-o", &path("bad.svg"), "--layer", "F.Cu:copper"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("colour must be #rgb or #rrggbb"));
}

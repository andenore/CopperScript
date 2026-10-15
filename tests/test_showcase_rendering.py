"""Native figure transformations must preserve exact package and route geometry."""
import importlib.util
from pathlib import Path
import xml.etree.ElementTree as ET

import pytest

spec = importlib.util.spec_from_file_location("showcase_rendering", Path(__file__).resolve().parents[1] / "scripts/render_board_showcase.py")
renderer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(renderer)


BOARD = '''(kicad_pcb
  (layers (0 "F.Cu" signal) (2 "B.Cu" signal))
  (footprint "QFN (native)" (layer "F.Cu") (at 10 20 90)
    (property "Reference" "U1 (keep pose)" (layer "F.Fab"))
    (property "Value" "quoted \\"value\\"" (layer "F.Fab") (hide yes))
    (fp_line (start 0 0) (end 1 1) (layer "F.Fab"))
    (pad "1" smd roundrect (at 1.25 2) (size 0.4 0.6) (layers "F.Cu" "F.Mask")))
  (segment (start 1 2) (end 3 4) (width 0.2) (layer "B.Cu"))
  (via (at 3 4) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu"))
  (zone (net 1) (layer "F.Cu") (filled_polygon (pts (xy 1 1) (xy 5 5))))
)'''


def test_omit_zone_preserves_native_pads_poses_and_routes():
    before = renderer.native_geometry(BOARD)
    after = renderer.omit(BOARD, lambda tag, _: tag == "zone")
    assert renderer.native_geometry(after) == before
    assert before["pads"] == 1 and before["tracks_and_vias"] == 2
    assert '(at 10 20 90)' in after
    assert '(zone' not in after
    assert [tag for tag, *_ in renderer.nodes(BOARD)] == ["layers", "footprint", "segment", "via", "zone"]


def test_hide_labels_keeps_native_package_and_route_geometry():
    hidden = renderer.hide_package_labels(BOARD)
    assert renderer.native_geometry(hidden) == renderer.native_geometry(BOARD)
    assert '(layer "F.Fab") (hide yes))' in hidden
    assert hidden.count('(hide yes)') == 2
    assert '(fp_line' in hidden


def test_pad_overlay_highlights_land_but_keeps_drill_and_bounds(tmp_path):
    base, pads = tmp_path / "base.svg", tmp_path / "pads.svg"
    base.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 8"><g id="copper"/></svg>')
    pads.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 8"><g><rect width="1" height="2" style="fill:#C83434"/><circle r="0.3" style="fill:#FFFFFF"/></g></svg>')
    renderer.highlight_pads(base, pads)
    tree = ET.parse(base)
    assert tree.getroot().get("viewBox") == "0 0 10 8"
    assert tree.find('.//{*}rect').get("style") == "fill:" + renderer.PAD_COLOR
    assert tree.find('.//{*}circle').get("style") == "fill:#FFFFFF"
    pads.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 8"/>')
    with pytest.raises(ValueError, match="different bounds"):
        renderer.highlight_pads(base, pads)

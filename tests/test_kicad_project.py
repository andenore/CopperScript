"""Self-contained, deterministic KiCad library export and independent checks."""

from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import BoardOutline, KiCadPcbBackend, PhysicalBoard, Placement, Point
from pcbir.backends.base import Artifact
from pcbir.backends.kicad_project import kicad_export_digest, write_kicad_project
from pcbir.importers.kicad_mod import parse_kicad_mod
from pcbir.physical import BoardSide, CopperLayer


def fixture_board() -> PhysicalBoard:
    fp = parse_kicad_mod('''(footprint "Vendor:Socket" (layer "F.Cu")
      (pad "1" smd roundrect (at -1 0 30) (size 0.8 0.5)
        (roundrect_rratio 0.25) (layers "F.Cu" "F.Paste" "F.Mask"))
      (pad "2" thru_hole oval (at 1 0) (size 1 1.2) (drill oval 0.4 0.6)
        (layers "*.Cu" "*.Mask"))
      (fp_rect (start -2 -2) (end 2 2) (stroke (width 0.05) (type default))
        (fill none) (layer "F.CrtYd"))
      (zone (layer "F.Cu")
        (keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed)
          (copperpour not_allowed) (footprints allowed))
        (polygon (pts (xy 0 1) (xy 1 1) (xy 1 2) (xy 0 2)))))''').footprint
    return PhysicalBoard("LocalLibrary", BoardOutline.rectangle(40, 30),
        {fp.name: fp}, (
            Placement("J1", fp.name, Point.mm(10, 10), rotation_degrees=45),
            Placement("J2", fp.name, Point.mm(25, 10), rotation_degrees=135, side=BoardSide.BACK),
        ), ())


def test_library_is_unique_canonical_and_never_mutates_ir():
    board = fixture_board()
    before = repr(board)
    manifest = KiCadPcbBackend().generate(board)
    assert manifest == KiCadPcbBackend().generate(board)
    assert repr(board) == before
    libraries = [item for item in manifest.artifacts if item.name.endswith(".kicad_mod")]
    assert len(libraries) == 1  # one asset, two front/back/rotated instances
    canonical = parse_kicad_mod(libraries[0].content).footprint
    original = next(iter(board.footprints.values()))
    assert canonical.pads == original.pads
    assert canonical.graphics == original.graphics
    assert canonical.keepouts[0].outline == original.keepouts[0].outline
    assert canonical.keepouts[0].layers == (CopperLayer.FRONT,)
    assert "(net " not in libraries[0].content.split("(pad", 1)[-1].split("(zone", 1)[0]
    assert "${KIPRJMOD}/CopperScript.pretty" in manifest.artifacts[2].content
    name = Path(libraries[0].name).stem
    assert manifest.artifacts[0].content.count(f'(footprint "CopperScript:{name}"') == 2
    renamed = KiCadPcbBackend().generate(replace(board, name="AnotherBoard"))
    assert renamed.artifacts[3:] == manifest.artifacts[3:]


def test_content_addressing_handles_namespace_sanitization_and_geometry_changes():
    board = fixture_board()
    original = next(iter(board.footprints.values()))
    other = replace(original, name="Other:Socket")
    collision = replace(original, name="Vendor/Socket")
    board = replace(board, footprints={fp.name: fp for fp in (original, other, collision)},
        placements=(*board.placements, Placement("J3", other.name, Point.mm(10, 20)),
                    Placement("J4", collision.name, Point.mm(25, 20))))
    names = [item.name for item in KiCadPcbBackend().generate(board).artifacts if item.name.endswith(".kicad_mod")]
    assert len(set(name.casefold() for name in names)) == 3
    changed = replace(original, clearance_nm=100000)
    changed_board = replace(board, footprints={**board.footprints, original.name: changed})
    changed_names = [item.name for item in KiCadPcbBackend().generate(changed_board).artifacts if item.name.endswith(".kicad_mod")]
    assert set(names) != set(changed_names)
    # Moving a local source cache must not rename the exported asset.
    assert KiCadPcbBackend().generate(replace(board, footprints={**board.footprints,
        original.name: replace(original, metadata={**original.metadata, "source_path": "new/cache"})})).artifacts == KiCadPcbBackend().generate(board).artifacts


def test_writer_creates_complete_project_and_repeats_with_custom_stem(tmp_path):
    manifest = KiCadPcbBackend().generate(fixture_board())
    output = tmp_path / "nested folder" / "chosen.kicad_pcb"
    paths = write_kicad_project(manifest, output)
    assert len(paths) == len(manifest.artifacts)
    before = {path: path.read_bytes() for path in paths}
    assert write_kicad_project(manifest, output) == paths
    assert {path: path.read_bytes() for path in paths} == before
    assert json.loads(output.with_suffix(".kicad_pro").read_text())["meta"]["filename"] == "chosen.kicad_pro"
    assert (output.parent / "fp-lib-table").is_file()


def process_board():
    from pcbir import PhysicalNet, Stackup, Via
    return replace(fixture_board(), nets=(PhysicalNet("GND", ()),),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                        CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)),
        metadata={"fabrication_profile": "jlcpcb-six-layer"},
        vias=(Via("GND", Point.mm(20, 20), 300000, 200000, finish="filled-capped"),
              Via("GND", Point.mm(30, 20), 800000, 400000)))


def test_via_process_evidence_is_deterministic_content_bound_and_follows_output_stem(tmp_path):
    board = process_board()
    manifest = KiCadPcbBackend().generate(board)
    assert manifest == KiCadPcbBackend().generate(board)
    process = next(a for a in manifest.artifacts if a.name.endswith(".via-process.json"))
    data = json.loads(process.content)
    assert data == {
        "schema": "copperscript-via-process/v0.1", "generated_by": "CopperScript",
        "fabrication_profile": "jlcpcb-six-layer", "fabrication_ready": False,
        "vias": [{"net": "GND", "position_nm": [20000000, 20000000],
                  "diameter_nm": 300000, "drill_nm": 200000,
                  "layers": ["F.Cu", "B.Cu"], "finish": "filled-capped", "technology": None}],
    }
    assert any(".via-process.json" in warning and "native PCB alone" in warning for warning in manifest.warnings)
    changed = replace(manifest, artifacts=tuple(replace(a, content=a.content + "\n")
                                              if a is process else a for a in manifest.artifacts))
    assert kicad_export_digest(changed) != kicad_export_digest(manifest)
    output = tmp_path / "chosen.kicad_pcb"
    paths = write_kicad_project(manifest, output)
    destination = output.with_suffix(".via-process.json")
    assert destination in paths and destination.read_text() == process.content
    assert not (tmp_path / process.name).exists()
    assert write_kicad_project(manifest, output) == paths
    plain = replace(board, vias=tuple(replace(via, finish="standard") for via in board.vias))
    write_kicad_project(KiCadPcbBackend().generate(plain), output)
    assert not destination.exists()


@pytest.mark.parametrize("unrelated", ["user instructions", '{"schema": "copperscript-via-process/v0.1"}',
                                      '{"generated_by": "CopperScript", "schema": "other-format"}'])
def test_writer_preserves_unrelated_process_sidecars_and_rejects_conflicts_atomically(tmp_path, unrelated):
    output = tmp_path / "chosen.kicad_pcb"
    process = output.with_suffix(".via-process.json")
    process.write_text(unrelated)
    write_kicad_project(KiCadPcbBackend().generate(fixture_board()), output)
    assert process.read_text() == unrelated
    before = {p: p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()}
    with pytest.raises(FileExistsError, match="unrelated via-process"):
        write_kicad_project(KiCadPcbBackend().generate(process_board()), output)
    assert {p: p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()} == before


def test_writer_preserves_unrelated_library_table_before_writing_anything(tmp_path):
    table = tmp_path / "fp-lib-table"
    table.write_text("user-maintained table")
    with pytest.raises(FileExistsError, match="unrelated fp-lib-table"):
        write_kicad_project(KiCadPcbBackend().generate(fixture_board()), tmp_path / "board.kicad_pcb")
    assert table.read_text() == "user-maintained table"
    assert list(tmp_path.iterdir()) == [table]


@pytest.mark.parametrize("name", ["../escape.kicad_mod", "/escape.kicad_mod", "C:/escape.kicad_mod", "..\\escape.kicad_mod"])
def test_writer_rejects_unsafe_manifest_paths_before_writing(tmp_path, name):
    manifest = KiCadPcbBackend().generate(fixture_board())
    malicious = replace(manifest, artifacts=(*manifest.artifacts, Artifact(name, "text/plain", "unsafe")))
    with pytest.raises(ValueError, match="unsafe generated artifact"):
        write_kicad_project(malicious, tmp_path / "board.kicad_pcb")
    assert not list(tmp_path.iterdir())


def test_verification_digest_covers_library_table_and_footprints():
    manifest = KiCadPcbBackend().generate(fixture_board())
    digest = kicad_export_digest(manifest)
    for index in range(2, len(manifest.artifacts)):
        artifacts = list(manifest.artifacts)
        artifacts[index] = replace(artifacts[index], content=artifacts[index].content + "\n")
        assert kicad_export_digest(replace(manifest, artifacts=tuple(artifacts))) != digest


def test_writer_rejects_duplicate_destinations_before_writing(tmp_path):
    manifest = KiCadPcbBackend().generate(fixture_board())
    duplicate = replace(manifest, artifacts=(*manifest.artifacts, manifest.artifacts[-1]))
    with pytest.raises(ValueError, match="destinations collide"):
        write_kicad_project(duplicate, tmp_path / "board.kicad_pcb")
    assert not list(tmp_path.iterdir())


def test_cli_reports_colliding_output_paths_without_partial_project(tmp_path, capsys):
    from pcbir.cli import main

    root = Path(__file__).resolve().parents[1]
    assert main(["export-kicad-pcb", str(root / "examples/resolved_footprint_board/board.copper"),
                 "-o", str(tmp_path / "wrong.kicad_pro")]) == 2
    assert "OUTPUT ERROR: generated artifact destinations collide" in capsys.readouterr().out
    assert not list(tmp_path.iterdir())


def test_writer_rejects_library_symlink_escaping_output_directory(tmp_path):
    root, outside = tmp_path / "project", tmp_path / "outside"
    root.mkdir()
    outside.mkdir()
    link = root / "CopperScript.pretty"
    try:
        link.symlink_to(outside, target_is_directory=True)
    except OSError:
        pytest.skip("creating symlinks requires platform permissions")
    with pytest.raises(ValueError, match="escapes output directory"):
        write_kicad_project(KiCadPcbBackend().generate(fixture_board()), root / "board.kicad_pcb")
    assert list(root.iterdir()) == [link]
    assert not list(outside.iterdir())


@pytest.mark.parametrize("with_graphics", [True, False])
def test_installed_kicad_resolves_relocated_project_without_library_warnings(tmp_path, with_graphics):
    cli = shutil.which("kicad-cli")
    installed = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli and not installed.is_file():
        pytest.skip("independent library check requires KiCad CLI")
    cli = cli or str(installed)
    output = tmp_path / "original" / "chosen.kicad_pcb"
    board = fixture_board()
    if not with_graphics:
        board = replace(board, footprints={name: replace(fp, graphics=()) for name, fp in board.footprints.items()})
    write_kicad_project(KiCadPcbBackend().generate(board), output)
    relocated = tmp_path / "relocated with spaces"
    shutil.copytree(output.parent, relocated)
    report = relocated / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--format", "json", "--output", str(report),
                             str(relocated / output.name)], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    findings = json.loads(report.read_text())["violations"]
    assert not [item for item in findings if item["type"].startswith("lib_")], findings


@pytest.mark.parametrize("side,rotation", [(BoardSide.FRONT, 0), (BoardSide.FRONT, 45), (BoardSide.BACK, 90)])
def test_installed_kicad_enforces_transformed_footprint_keepouts(tmp_path, side, rotation):
    from pcbir import PhysicalNet, TrackSegment, nm_from_mm
    from pcbir.placement import transformed_local_point

    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("independent keepout check requires KiCad CLI")
    source = fixture_board()
    pose = replace(source.placements[0], rotation_degrees=rotation, side=side)
    layer = CopperLayer.FRONT if side is BoardSide.FRONT else CopperLayer.BACK
    start, end = (transformed_local_point(pose, point) for point in (Point.mm(-0.5, 1.5), Point.mm(1.5, 1.5)))
    board = replace(source, placements=(pose,), nets=(PhysicalNet("PROBE", ()),),
        tracks=(TrackSegment("PROBE", start, end, nm_from_mm("0.2"), layer),))
    output = tmp_path / "probe.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), output)
    report = tmp_path / "drc.json"
    result = subprocess.run([cli, "pcb", "drc", "--format", "json", "--output", str(report), str(output)],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    findings = json.loads(report.read_text())["violations"]
    assert any(item["type"] == "items_not_allowed" for item in findings), findings
    assert not any(item["type"].startswith("lib_") for item in findings), findings


def test_native_kicad_pad_positions_and_angles_match_front_back_physical_ir(tmp_path):
    from pcbir.placement import transformed_pad_position

    python = Path("C:/Program Files/KiCad/10.0/bin/python.exe")
    if not python.is_file():
        pytest.skip("optional direct geometry comparison requires KiCad's Python runtime")
    board = fixture_board()
    output = tmp_path / "geometry.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), output)
    script = """import json, pcbnew, sys
b = pcbnew.LoadBoard(sys.argv[1])
print(json.dumps({f.GetReference(): {p.GetNumber(): [p.GetPosition().x, p.GetPosition().y,
    p.GetOrientationDegrees() % 180] for p in f.Pads()} for f in b.GetFootprints()}))
"""
    result = subprocess.run([str(python), "-c", script, str(output)], capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout + result.stderr
    actual = json.loads(result.stdout)
    for pose in board.placements:
        for pad in board.footprints[pose.footprint].pads:
            expected = transformed_pad_position(board, pose, pad.number)
            x, y, angle = actual[pose.reference][pad.number]
            assert abs(x - expected.x_nm) <= 1 and abs(y - expected.y_nm) <= 1
            local_angle = pad.rotation_degrees if pose.side is BoardSide.FRONT else -pad.rotation_degrees
            expected_angle = float((pose.rotation_degrees + local_angle + 360) % 180)
            assert angle == pytest.approx(expected_angle)

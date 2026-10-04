import csv
import json
from pathlib import Path
from zipfile import ZipFile

import pytest

from pcbir.cli import main
from pcbir.manufacturing import CommandResult
from pcbir.manufacturing_files import (
    ManufacturingFilesError, export_manufacturing_files, write_jlcpcb_cpl,
)


def pcb_fixture(tmp_path):
    pcb = tmp_path / "round.kicad_pcb"
    pcb.write_text('(kicad_pcb (layers (0 "F.Cu" signal) (2 "B.Cu" signal)) '
                   '(gr_circle (center 25 25) (end 50 25) (layer "Edge.Cuts")))')
    pcb.with_suffix(".kicad_pro").write_text('{"rules":"retained"}')
    return pcb


def fake_kicad(command, cwd):
    if command[1:] == ("version",):
        return CommandResult(0, "10.0.6\n")
    output = Path(command[command.index("-o") + 1])
    if command[1:3] == ("pcb", "drc"):
        output.write_text('{"violations":[],"unconnected_items":[]}')
    elif command[3] == "gerbers":
        for layer in command[command.index("--layers") + 1].split(","):
            (output / f"round-{layer.replace('.', '_')}.gbr").write_text("artwork")
    elif command[3] == "drill":
        assert command[command.index("--drill-origin") + 1] == "absolute"
        (output / "round-PTH.drl").write_text("drills")
    elif command[3] == "ipcd356":
        output.write_text("netlist")
    elif command[3] == "pos":
        assert "--bottom-negate-x" not in command
        output.write_text('Ref,Val,Package,PosX,PosY,Rot,Side\n'
                          'R1,10k,R,1.5,-2.0,45,top\nBT1,cell,B,29,-25,180,bottom\n')
    else:
        return CommandResult(2, stderr="unexpected command")
    return CommandResult(0)


def export(pcb, output, **kwargs):
    return export_manufacturing_files(pcb, output, kicad_cli=Path("kicad-cli"),
                                      skip_independent_cam=True, **kwargs)


def test_circle_exports_native_files_and_truthful_manifest(tmp_path):
    pcb = pcb_fixture(tmp_path)
    original = pcb.read_bytes()
    output = export(pcb, tmp_path / "manufacturing", runner=fake_kicad)
    doc = json.loads((output / "manifest.json").read_text())
    assert doc["independent_cam"] == "skipped_by_request"
    assert doc["qualified_release"] is False
    assert not doc["assembly_bom_included"]
    assert pcb.read_bytes() == original
    assert (output / pcb.name).read_bytes() == original
    assert (output / "round.kicad_pro").read_text() == '{"rules":"retained"}'
    with ZipFile(output / "gerbers-drill.zip") as archive:
        assert len(archive.namelist()) == 10
        assert all(name.startswith(("gerbers/", "drill/")) for name in archive.namelist())
    assert "manifest.json" in (output / "SHA256SUMS").read_text()
    with ZipFile(output / "manufacturing-package.zip") as archive:
        assert "SHA256SUMS" in archive.namelist()
        assert "gerbers-drill.zip" in archive.namelist()
        assert "manufacturing-package.zip" not in archive.namelist()
    assert not list(tmp_path.glob(".manufacturing-*"))


def test_bom_and_both_sides_cpl_preserve_native_conventions(tmp_path):
    pcb = pcb_fixture(tmp_path)
    bom = tmp_path / "bom.csv"
    bom.write_text('Comment,Designator,Footprint,LCSC Part #\n10k,R1,R,C123\ncell,BT1,B,C456\n')
    output = export(pcb, tmp_path / "manufacturing", runner=fake_kicad, bom=bom)
    with (output / "cpl.csv").open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    assert rows[0] == {"Designator":"BT1", "Mid X":"29", "Mid Y":"-25", "Rotation":"180", "Layer":"Bottom"}
    assert rows[1]["Rotation"] == "45"
    assert json.loads((output / "manifest.json").read_text())["assembly_bom_included"]


@pytest.mark.parametrize("report", [
    {"violations":[{}],"unconnected_items":[]},
    {"violations":[],"unconnected_items":[{}]},
    {"violations":[]},
    {"violations":[],"unconnected_items":None},
])
def test_failed_or_incomplete_native_drc_does_not_publish(tmp_path, report):
    pcb = pcb_fixture(tmp_path)
    def run(command, cwd):
        if command[1:3] == ("pcb", "drc"):
            Path(command[command.index("-o")+1]).write_text(json.dumps(report))
            return CommandResult(0)
        return fake_kicad(command, cwd)
    with pytest.raises(ManufacturingFilesError, match="zero violations"):
        export(pcb, tmp_path / "manufacturing", runner=run)
    assert not (tmp_path / "manufacturing").exists()
    assert not list(tmp_path.glob(".manufacturing-*"))


def test_tool_failure_does_not_publish(tmp_path):
    pcb = pcb_fixture(tmp_path)
    def run(command, cwd):
        if command[1:3] == ("pcb", "drc"):
            return CommandResult(5, stderr="clearance violation")
        return fake_kicad(command, cwd)
    with pytest.raises(RuntimeError, match="clearance violation"):
        export(pcb, tmp_path / "manufacturing", runner=run)
    assert not (tmp_path / "manufacturing").exists()


def test_requires_acknowledgement_and_matching_project(tmp_path):
    pcb = pcb_fixture(tmp_path)
    with pytest.raises(ManufacturingFilesError, match="acknowledgement"):
        export_manufacturing_files(pcb, tmp_path / "out", kicad_cli=Path("unused"))
    pcb.with_suffix(".kicad_pro").unlink()
    with pytest.raises(ManufacturingFilesError, match="matching .kicad_pro"):
        export(pcb, tmp_path / "out", runner=fake_kicad)


def test_never_overwrites_existing_outputs(tmp_path):
    pcb = pcb_fixture(tmp_path)
    output = tmp_path / "out"
    output.mkdir()
    with pytest.raises(FileExistsError):
        export(pcb, output, runner=fake_kicad)


def test_explicit_replacement_keeps_previous_outputs(tmp_path):
    pcb = pcb_fixture(tmp_path)
    output = export(pcb, tmp_path / "out", runner=fake_kicad)
    (output / "user-note.txt").write_text("retain me")
    export(pcb, output, runner=fake_kicad, replace_existing=True)
    backups = list(tmp_path.glob(".out-previous-*"))
    assert len(backups) == 1
    assert (backups[0] / "user-note.txt").read_text() == "retain me"
    assert not (output / "user-note.txt").exists()


def test_replacement_refuses_arbitrary_directory(tmp_path):
    pcb = pcb_fixture(tmp_path)
    output = tmp_path / "out"
    output.mkdir()
    with pytest.raises(ManufacturingFilesError, match="replacement requires"):
        export(pcb, output, runner=fake_kicad, replace_existing=True)


@pytest.mark.parametrize("row,error", [
    ("R1,1,-2,0,sideways", "unknown placement side"),
    ("R1,NaN,-2,0,top", "invalid placement number"),
    ("R1,1,-2,0,top\nR1,1,-2,0,top", "duplicate position"),
    ("OTHER,1,-2,0,top", "missing from position"),
])
def test_cpl_rejects_invalid_positions(tmp_path, row, error):
    native = tmp_path / "pos.csv"
    native.write_text("Ref,PosX,PosY,Rot,Side\n" + row + "\n")
    with pytest.raises(ManufacturingFilesError, match=error):
        write_jlcpcb_cpl(native, tmp_path / "cpl.csv", references={"R1"})


def test_cli_dispatches_export(tmp_path, monkeypatch, capsys):
    pcb = pcb_fixture(tmp_path)
    monkeypatch.setattr("pcbir.manufacturing_files._subprocess_runner", fake_kicad)
    assert main(["export-manufacturing", str(pcb), "--skip-independent-cam", "-o", str(tmp_path / "out")]) == 0
    assert "independent CAM skipped" in capsys.readouterr().out

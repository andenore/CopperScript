from __future__ import annotations

from dataclasses import replace
from pathlib import Path
import json

import pytest

from pcbir import (
    BoardOutline,
    CommandResult,
    CopperLayer,
    FootprintPad,
    ManufacturingProfile,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    TrackSegment,
    build_manufacturing_release,
    nm_from_mm,
    run_physical_drc,
    verify_cam_directory,
    FabricationAssemblyProfile,
    ProcessCapability,
    run_process_drc,
)


def _board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "Manufacturing",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
        tracks=(TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6), nm_from_mm("0.25"), CopperLayer.FRONT),),
        metadata={"detailed_routing": "complete"},
    )


def _fake_kicad(command: tuple[str, ...], cwd: Path) -> CommandResult:
    if command[1:] == ("--version",):
        return CommandResult(0, "10.0.6\n")
    if command[1:3] == ("pcb", "drc"):
        output = Path(command[command.index("--output") + 1])
        output.write_text('{"violations": []}\n', encoding="utf-8")
        return CommandResult(0)
    if command[1:4] == ("pcb", "export", "gerbers"):
        output = Path(command[command.index("--output") + 1])
        layers = command[command.index("--layers") + 1].split(",")
        functions = {
            "F.Cu": "Copper,L1,Top",
            "B.Cu": "Copper,L2,Bot",
            "F.Mask": "Soldermask,Top",
            "B.Mask": "Soldermask,Bot",
            "F.Silkscreen": "Legend,Top",
            "B.Silkscreen": "Legend,Bot",
            "Edge.Cuts": "Profile,NP",
        }
        for index, layer in enumerate(layers):
            (output / f"Manufacturing-{layer.replace('.', '_')}.gbr").write_text(
                "G04 CopperScript test*\n"
                "%FSLAX46Y46*%\n"
                "%MOMM*%\n"
                f"%TF.FileFunction,{functions[layer]}*%\n"
                "%TF.FilePolarity,Positive*%\n"
                "M02*\n",
                encoding="ascii",
            )
        return CommandResult(0)
    if command[1:4] == ("pcb", "export", "drill"):
        output = Path(command[command.index("--output") + 1])
        (output / "Manufacturing-PTH.drl").write_text(
            "M48\nMETRIC,TZ\n%\nM30\n", encoding="ascii"
        )
        return CommandResult(0)
    if command[1:4] == ("pcb", "export", "ipcd356"):
        output = Path(command[command.index("--output") + 1])
        output.write_text("P  JOB Manufacturing\n999\n", encoding="ascii")
        return CommandResult(0)
    return CommandResult(2, stderr=f"unexpected command: {command}")


def test_release_is_gated_verified_manifested_and_atomic(tmp_path: Path) -> None:
    board = _board()
    signoff = run_physical_drc(board).token
    output = tmp_path / "release"

    release = build_manufacturing_release(
        board,
        signoff,
        output,
        kicad_cli=Path("kicad-cli"),
        runner=_fake_kicad,
    )

    assert release.directory == output
    assert release.cam_report.passed
    assert release.manifest.is_file()
    assert release.checksums.is_file()
    document = json.loads(release.manifest.read_text(encoding="utf-8"))
    assert document["board_digest"] == signoff.board_digest
    assert document["signoff_token_digest"] == signoff.token_digest
    assert any(item["path"].endswith(".d356") for item in document["artifacts"])
    assert not any(item.name.startswith(".release-") for item in tmp_path.iterdir())


def test_release_rejects_stale_signoff_before_running_tools(tmp_path: Path) -> None:
    board = _board()
    signoff = run_physical_drc(board).token
    changed = replace(
        board,
        tracks=(replace(board.tracks[0], width_nm=nm_from_mm("0.3")),),
    )

    with pytest.raises(ValueError, match="does not match"):
        build_manufacturing_release(
            changed,
            signoff,
            tmp_path / "release",
            kicad_cli=Path("kicad-cli"),
            runner=_fake_kicad,
        )

    assert not (tmp_path / "release").exists()


def test_independent_cam_parser_rejects_corrupt_artwork(tmp_path: Path) -> None:
    profile = ManufacturingProfile(
        gerber_layers=("F.Cu",),
        require_ipcd356=False,
        required_file_function_prefixes=(),
    )
    gerbers = tmp_path / "gerbers"
    drills = tmp_path / "drill"
    gerbers.mkdir()
    drills.mkdir()
    (gerbers / "bad.gbr").write_text("%FSLAX46Y46*%\n%MOMM*%\nM02*\n", encoding="ascii")
    (drills / "board.drl").write_text("M48\nMETRIC,TZ\n%\nM30\n", encoding="ascii")

    report = verify_cam_directory(tmp_path, profile)

    assert not report.passed
    assert {item.code for item in report.findings} >= {
        "CAM-GERBER-X2",
        "CAM-GERBER-POLARITY",
    }


def test_release_profile_can_require_separate_process_gates(tmp_path: Path) -> None:
    board = _board()
    signoff = run_physical_drc(board).token
    profile = ManufacturingProfile(require_process_drc=True)
    with pytest.raises(ValueError, match="requires fabrication"):
        build_manufacturing_release(board, signoff, tmp_path / "release",
                                    kicad_cli=Path("kicad-cli"), profile=profile,
                                    runner=_fake_kicad)

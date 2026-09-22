from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest

from pcbir import (
    CopperKeepout,
    CopperLayer,
    CopperZone,
    IslandPolicy,
    KiCadPcbBackend,
    ManufacturingProfile,
    Point,
    PolygonRing,
    PolygonWithHoles,
    ThermalReliefSettings,
    ZoneConnection,
    build_manufacturing_release,
    run_physical_drc,
)

from test_manufacturing import _board, _fake_kicad


def _polygon(x1: int, y1: int, x2: int, y2: int) -> PolygonWithHoles:
    return PolygonWithHoles(
        PolygonRing(
            (
                Point.mm(x1, y1),
                Point.mm(x2, y1),
                Point.mm(x2, y2),
                Point.mm(x1, y2),
            )
        )
    )


def _zone_board():
    return replace(
        _board(),
        zones=(
            CopperZone(
                "signal-pour",
                "SIGNAL",
                (CopperLayer.FRONT,),
                _polygon(1, 1, 19, 11),
                priority=2,
                pad_connection=ZoneConnection.SOLID,
                thermal=ThermalReliefSettings(),
                island_policy=IslandPolicy.KEEP_ALL,
                minimum_island_area_nm2=None,
            ),
        ),
        copper_keepouts=(
            CopperKeepout(
                "antenna-clearance",
                (CopperLayer.FRONT, CopperLayer.BACK),
                _polygon(8, 4, 12, 8),
            ),
        ),
    )


def test_zone_intent_is_typed_validated_and_deterministic() -> None:
    board = _zone_board()

    first = KiCadPcbBackend().generate(board).artifacts[0].content
    second = KiCadPcbBackend().generate(board).artifacts[0].content

    assert first == second
    assert '(name "signal-pour")' in first
    assert "(connect_pads yes (clearance 0.2))" in first
    assert "(island_removal_mode 1)" in first
    assert '(name "antenna-clearance")' in first
    assert "(copperpour not_allowed)" in first


def test_zone_references_and_polygon_holes_fail_safely() -> None:
    board = _board()
    with pytest.raises(ValueError, match="unknown net"):
        replace(
            board,
            zones=(
                CopperZone(
                    "bad", "MISSING", (CopperLayer.FRONT,), _polygon(1, 1, 2, 2)
                ),
            ),
        )

    holed = PolygonWithHoles(
        _polygon(1, 1, 10, 10).outer,
        (_polygon(3, 3, 4, 4).outer,),
    )
    with pytest.raises(ValueError, match="does not yet support holes"):
        KiCadPcbBackend().generate(
            replace(
                board,
                zones=(
                    CopperZone(
                        "holed", "SIGNAL", (CopperLayer.FRONT,), holed
                    ),
                ),
            )
        )


def test_manufacturing_refills_and_saves_zones_with_pinned_kicad(
    tmp_path: Path,
) -> None:
    board = _zone_board()
    commands: list[tuple[str, ...]] = []

    def runner(command: tuple[str, ...], cwd: Path):
        commands.append(command)
        return _fake_kicad(command, cwd)

    release = build_manufacturing_release(
        board,
        run_physical_drc(board).token,
        tmp_path / "release",
        kicad_cli=Path("kicad-cli"),
        profile=ManufacturingProfile(),
        runner=runner,
    )

    drc = next(command for command in commands if command[1:3] == ("pcb", "drc"))
    assert "--refill-zones" in drc
    assert "--save-board" in drc
    assert release.cam_report.passed

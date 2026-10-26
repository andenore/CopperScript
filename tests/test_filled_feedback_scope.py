"""Known non-zone opens must not be mislabeled as unknown native objects."""
from pathlib import Path
import shutil

import pytest

from pcbir import (
    BoardOutline,
    CopperLayer,
    CopperZone,
    PadReference,
    PhysicalBoard,
    PhysicalNet,
    Placement,
    Point,
)
from pcbir.fanout import FanoutOptions
from pcbir.filled_feedback import improve_filled_copper
from pcbir.flow import run_routing_pipeline
from pcbir.physical import BoardSide, IslandPolicy, ZoneConnection
from pcbir.placement import PlacementPlannerOptions
from pcbir.routeflow import PlacementRoutingFeedbackOptions
from test_autonomous_routing_quality import pipeline, rectangle, terminal


@pytest.mark.parametrize("renamed", (False, True))
@pytest.mark.parametrize("with_zone", (False, True))
def test_real_native_ordinary_opens_report_non_zone_routing_scope(renamed, with_zone):
    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        pytest.skip("real KiCad required for native scope evidence")
    signal = "CONTROL_57" if renamed else "SIGNAL"
    ground = "RETURN_23" if renamed else "GND"
    refs = ("HOST_8", "SENSOR_6") if renamed else ("J1", "J2")
    land = terminal()
    poses = tuple(Placement(ref, land.name, Point.mm(x, 9))
                  for ref, x in zip(refs, (3, 17)))
    nets = (PhysicalNet(signal, tuple(PadReference(ref, "1") for ref in refs)),)
    zones = ()
    if with_zone:
        returns = ("RETURN_A", "RETURN_B")
        poses = (*poses, *(Placement(ref, land.name, Point.mm(x, 3))
                          for ref, x in zip(returns, (3, 17))))
        nets = (*nets, PhysicalNet(ground, tuple(PadReference(ref, "1") for ref in returns)))
        zones = (CopperZone("return-fill", ground, (CopperLayer.FRONT,),
                 rectangle(1, 1, 19, 11), pad_connection=ZoneConnection.SOLID,
                 island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None),)
    board = PhysicalBoard("native-scope", BoardOutline.rectangle(20, 12),
                          {land.name: land}, poses, nets, zones=zones)
    # Actual bounded search fails: neither routing nor connected flags are mocked.
    initial = run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,
            analytical_iterations=0, refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in poses)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        fanout_options=FanoutOptions())
    assert initial.package_access.ready
    assert not next(item for item in initial.detailed.nets if item.net == signal).connected
    result = improve_filled_copper(initial, kicad_cli=cli)
    assert result.board == initial.board and result.pipeline == initial and not result.trials
    assert result.verification.matches(result.board) and not result.verification.passed
    opens = [finding for finding in result.verification.native_findings
             if finding.category == "unconnected"]
    assert opens and len(opens) == result.verification.unconnected_count
    assert {item.net for finding in opens for item in finding.items} == {signal}
    assert all(item.kind != "unknown" for finding in opens for item in finding.items)
    assert result.stop_reason == "no_eligible_zone_opens"
    assert result.diagnostic and "routing" in result.diagnostic.lower()


@pytest.mark.parametrize("renamed", (False, True))
def test_known_opposite_surface_zone_opens_are_not_unknown_objects(renamed):
    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        pytest.skip("real KiCad required for native scope evidence")
    net = "RAIL_53" if renamed else "SUPPLY"
    refs = ("HOST_A", "LOAD_B") if renamed else ("J1", "J2")
    land = terminal()
    board = PhysicalBoard("two-surface-scope", BoardOutline.rectangle(20, 12),
        {land.name: land},
        (Placement(refs[0], land.name, Point.mm(3, 6)),
         Placement(refs[1], land.name, Point.mm(17, 6), side=BoardSide.BACK)),
        (PhysicalNet(net, tuple(PadReference(ref, "1") for ref in refs)),),
        zones=(CopperZone("two-surface-fill", net, (CopperLayer.FRONT, CopperLayer.BACK),
            rectangle(1, 1, 19, 11), pad_connection=ZoneConnection.SOLID,
            island_policy=IslandPolicy.REMOVE_ALL, minimum_island_area_nm2=None),))
    initial = pipeline(board)
    assert initial.package_access.ready
    result = improve_filled_copper(initial, kicad_cli=cli)
    assert result.board == initial.board and result.pipeline == initial and not result.trials
    assert not result.verification.passed and result.verification.unconnected_count
    assert result.verification.matches(result.board)
    assert all(item.kind != "unknown" and item.net == net
               for finding in result.verification.native_findings
               if finding.category == "unconnected" for item in finding.items)
    assert result.stop_reason == "no_eligible_surface_bridge"
    assert result.diagnostic and "same-surface" in result.diagnostic
    assert not result.board.vias  # No unsupported layer transition is invented.

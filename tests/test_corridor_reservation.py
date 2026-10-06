"""R4: corridor-aware placement for ``reserve_corridor`` differential pairs."""

from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import subprocess
import sys

import pytest

from pcbir import (
    BoardOutline,
    ComponentPlacementRule,
    FootprintPad,
    GateStatus,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementPlannerOptions,
    PlacementPlanningError,
    Size,
    compile_source,
    placement_solution_is_legal,
    plan_placement,
    prototype_physicalize,
)
from pcbir.drc import physical_board_digest
from pcbir.physical import BoardSide, NetRoutingRule, Point, RouteKind, nm_from_mm
from pcbir.placement import (
    _placement_polygon,
    _polygons_too_close,
    placement_rejection_reasons,
    reserved_corridors,
)


ROOT = Path(__file__).parents[1]
OPTIONS = PlacementPlannerOptions(candidate_count=1)


def _board(
    *,
    reserve: bool = True,
    fix_connector: bool = True,
    fixed_capacitor: Point | None = None,
    pair_kind: RouteKind = RouteKind.DIFFERENTIAL,
    profile: bool = True,
) -> PhysicalBoard:
    """A fixed chip and connector joined by DP/DN, plus a movable capacitor.

    The capacitor connects to a chip pin and a connector pin beside the pair,
    so wire length pulls it into the pair's corridor unless one is reserved.
    """

    chip = PhysicalFootprint("test/chip", (
        FootprintPad("1", Point.mm(1.4, -0.25), Size.mm(0.6, 0.25)),
        FootprintPad("2", Point.mm(1.4, 0.25), Size.mm(0.6, 0.25)),
        FootprintPad("3", Point.mm(1.4, 1.0), Size.mm(0.6, 0.25)),
    ), Size.mm(3, 3))
    connector = PhysicalFootprint("test/connector", (
        FootprintPad("1", Point.mm(-1.4, -0.25), Size.mm(0.6, 0.25)),
        FootprintPad("2", Point.mm(-1.4, 0.25), Size.mm(0.6, 0.25)),
        FootprintPad("3", Point.mm(-1.4, 1.0), Size.mm(0.6, 0.25)),
    ), Size.mm(3, 3))
    capacitor = PhysicalFootprint("test/capacitor", (
        FootprintPad("1", Point.mm(-0.5, 0), Size.mm(0.5, 0.5)),
        FootprintPad("2", Point.mm(0.5, 0), Size.mm(0.5, 0.5)),
    ), Size.mm(1.6, 0.8))
    rules = [ComponentPlacementRule("U1", fixed_position=Point.mm(8, 10),
                                    fixed_rotation_degrees=0, side=BoardSide.FRONT)]
    if fix_connector:
        rules.append(ComponentPlacementRule("J1", fixed_position=Point.mm(24, 10),
                                            fixed_rotation_degrees=0, side=BoardSide.FRONT))
    if fixed_capacitor is not None:
        rules.append(ComponentPlacementRule("C1", fixed_position=fixed_capacitor,
                                            fixed_rotation_degrees=0))
    if profile:
        widths = {"width_nm": nm_from_mm("0.15"), "clearance_nm": nm_from_mm("0.25"),
                  "pair_gap_nm": nm_from_mm("0.15")}
    else:
        widths = {}
    return PhysicalBoard(
        "CorridorReservation",
        BoardOutline.rectangle(32, 20),
        {item.name: item for item in (chip, connector, capacitor)},
        (
            Placement("U1", chip.name, Point.mm(8, 10)),
            Placement("J1", connector.name, Point.mm(24, 10)),
            Placement("C1", capacitor.name, Point.mm(16, 4)),
        ),
        (
            PhysicalNet("DP", (PadReference("U1", "1"), PadReference("J1", "1"))),
            PhysicalNet("DN", (PadReference("U1", "2"), PadReference("J1", "2"))),
            PhysicalNet("A", (PadReference("U1", "3"), PadReference("C1", "1"))),
            PhysicalNet("B", (PadReference("J1", "3"), PadReference("C1", "2"))),
        ),
        placement_rules=tuple(rules),
        net_routing_rules=(
            # Only one member requests the reservation; it covers the pair.
            NetRoutingRule("DN", pair_kind, differential_partner="DP", **widths),
            NetRoutingRule("DP", pair_kind, differential_partner="DN",
                           reserve_corridor=reserve, **widths),
        ),
    )


def _overlaps_corridor(board: PhysicalBoard, placement: Placement, corridor_board: PhysicalBoard) -> bool:
    corridor = reserved_corridors(corridor_board).corridors[0]
    return _polygons_too_close(
        _placement_polygon(board, placement), corridor.keepout.outline.vertices, 0
    )


def _pose(board: PhysicalBoard, reference: str) -> Placement:
    return next(item for item in board.placements if item.reference == reference)


def test_reserve_corridor_lowers_from_source_and_defaults_off() -> None:
    template = '''
        board Routed {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "0402"; }}
            component R2: RESISTOR {{ footprint = "0402"; }}
            net USB_DP {{ R1.1; R2.1; }}
            net USB_DM {{ R1.2; R2.2; }}
            constraint routing(USB_DP) {{
                kind = differential; partner = USB_DM; pair_gap = 0.2mm; {extra}
            }}
        }}
    '''
    reserved = prototype_physicalize(compile_source(template.format(extra="reserve_corridor = true;")))
    default = prototype_physicalize(compile_source(template.format(extra="")))

    assert reserved.net_routing_rules[0].reserve_corridor is True
    assert default.net_routing_rules[0].reserve_corridor is False
    assert physical_board_digest(reserved) != physical_board_digest(default)


@pytest.mark.parametrize(("body", "message"), [
    ("reserve_corridor = true;", "requires a differential partner"),
    ("kind = differential; partner = USB_DM; pair_gap = 0.2mm; reserve_corridor = 1;",
     "must be boolean"),
])
def test_reserve_corridor_rejects_invalid_source(body: str, message: str) -> None:
    electrical = compile_source(f'''
        board Routed {{
            use library "tiny";
            component R1: RESISTOR {{ footprint = "0402"; }}
            component R2: RESISTOR {{ footprint = "0402"; }}
            net USB_DP {{ R1.1; R2.1; }}
            net USB_DM {{ R1.2; R2.2; }}
            constraint routing(USB_DP) {{ {body} }}
        }}
    ''')
    with pytest.raises(ValueError, match=message):
        prototype_physicalize(electrical)


def test_corridor_covers_terminal_lands_expanded_by_pair_margin() -> None:
    reservation = reserved_corridors(_board())

    assert reservation.skipped == ()
    (corridor,) = reservation.corridors
    assert corridor.nets == ("DN", "DP")
    assert corridor.name == "reserved-corridor:DN/DP"
    assert corridor.side is BoardSide.FRONT
    assert corridor.terminal_references == ("J1", "U1")
    # width 0.15 + pair gap 0.15 + rule clearance 0.25 (above the 0.2 board minimum).
    assert corridor.margin_nm == nm_from_mm("0.55")
    xs = [point.x_nm for point in corridor.keepout.outline.vertices]
    ys = [point.y_nm for point in corridor.keepout.outline.vertices]
    # U1 lands start at x = 9.1 mm, J1 lands end at x = 22.9 mm, and the pair
    # spans y = 9.625 .. 10.375 mm; the corridor adds the margin on every side.
    assert (min(xs), max(xs)) == (nm_from_mm("8.55"), nm_from_mm("23.45"))
    assert (min(ys), max(ys)) == (nm_from_mm("9.075"), nm_from_mm("10.925"))


def test_corridor_margin_falls_back_to_board_rules() -> None:
    board = _board(pair_kind=RouteKind.GENERAL, profile=False)
    (corridor,) = reserved_corridors(board).corridors

    assert corridor.margin_nm == (
        board.rules.default_track_width_nm
        + board.rules.minimum_clearance_nm
        + board.rules.minimum_clearance_nm
    )


def test_corridor_spans_both_sides_when_terminals_are_on_different_sides() -> None:
    board = _board()
    board = replace(
        board,
        placements=tuple(replace(item, side=BoardSide.BACK) if item.reference == "J1" else item
                         for item in board.placements),
        placement_rules=tuple(replace(rule, side=BoardSide.BACK) if rule.reference == "J1" else rule
                              for rule in board.placement_rules),
    )
    (corridor,) = reserved_corridors(board).corridors

    assert corridor.side is None
    placements = {item.reference: item for item in board.placements}
    assert placement_solution_is_legal(board, placements, OPTIONS)
    for side in BoardSide:
        inside = {**placements, "C1": replace(placements["C1"], position=Point.mm(16, 10), side=side)}
        assert not placement_solution_is_legal(board, inside, OPTIONS)


def test_movable_capacitor_is_placed_outside_the_reserved_corridor() -> None:
    board = _board()
    plan = plan_placement(board, OPTIONS)
    capacitor = _pose(plan.board, "C1")

    assert not _overlaps_corridor(plan.board, capacitor, board)
    assert placement_solution_is_legal(
        board, {item.reference: item for item in plan.board.placements}, OPTIONS
    )
    codes = [finding.code for finding in plan.report.findings]
    assert "RESERVED_CORRIDORS" in codes
    assert "CORRIDOR_NOT_RESERVED" not in codes


def test_without_reserve_corridor_the_capacitor_lands_in_the_corridor() -> None:
    plan = plan_placement(_board(reserve=False), OPTIONS)

    assert reserved_corridors(_board(reserve=False)).corridors == ()
    # The same placement would block the pair: a decoupling capacitor in the breakout.
    assert _overlaps_corridor(plan.board, _pose(plan.board, "C1"), _board())
    assert "reserved_corridors" in json.loads(plan.report.to_json())


def test_corridor_is_enforced_by_shared_legality_and_named_in_rejections() -> None:
    board = _board()
    placements = {item.reference: item for item in board.placements}
    inside = {**placements, "C1": replace(placements["C1"], position=Point.mm(16, 10))}

    assert placement_solution_is_legal(board, placements, OPTIONS)
    assert not placement_solution_is_legal(board, inside, OPTIONS)
    assert any("reserved-corridor:DN/DP" in reason
               for reason in placement_rejection_reasons(board, inside, OPTIONS))
    # The corridor is side-specific, like a hand-drawn keepout.
    flipped = {**inside, "C1": replace(inside["C1"], side=BoardSide.BACK)}
    assert placement_solution_is_legal(board, flipped, OPTIONS)
    # Without the property, the same pose is legal.
    unreserved = _board(reserve=False)
    assert placement_solution_is_legal(unreserved, inside, OPTIONS)


def test_fixed_non_terminal_component_in_corridor_is_an_error() -> None:
    board = _board(fixed_capacitor=Point.mm(16, 10))

    with pytest.raises(
        PlacementPlanningError,
        match=r"reserved corridor 'reserved-corridor:DN/DP' for differential pair DN/DP "
              r"overlaps fixed component 'C1'",
    ):
        plan_placement(board, OPTIONS)


def test_session_locked_component_in_corridor_is_an_error() -> None:
    board = _board()
    board = replace(board, placements=tuple(
        replace(item, position=Point.mm(16, 10)) if item.reference == "C1" else item
        for item in board.placements
    ))

    with pytest.raises(PlacementPlanningError, match="overlaps fixed component 'C1'"):
        plan_placement(board, replace(OPTIONS, fixed_references=frozenset({"C1"})))


def test_corridor_is_skipped_with_reason_when_a_terminal_is_movable() -> None:
    board = _board(fix_connector=False)
    reservation = reserved_corridors(board)

    assert reservation.corridors == ()
    (skipped,) = reservation.skipped
    assert skipped.nets == ("DN", "DP")
    assert "J1" in skipped.reason and "not fixed" in skipped.reason
    placements = {item.reference: item for item in board.placements}
    inside = {**placements, "C1": replace(placements["C1"], position=Point.mm(16, 10))}
    assert placement_solution_is_legal(board, inside, OPTIONS)

    plan = plan_placement(board, OPTIONS)
    findings = [item for item in plan.report.findings if item.code == "CORRIDOR_NOT_RESERVED"]
    assert len(findings) == 1 and findings[0].status is GateStatus.WARNING
    assert findings[0].subject == "DN/DP"
    assert plan.report.gates[0].status is GateStatus.WARNING
    document = json.loads(plan.report.to_json())
    assert document["reserved_corridors"] == []
    assert document["skipped_corridors"] == [{"nets": ["DN", "DP"], "reason": skipped.reason}]


def test_corridor_report_is_deterministic_and_reviewable() -> None:
    first = plan_placement(_board(), OPTIONS).report.to_json()
    second = plan_placement(_board(), OPTIONS).report.to_json()

    assert first == second
    (corridor,) = json.loads(first)["reserved_corridors"]
    assert corridor["name"] == "reserved-corridor:DN/DP"
    assert corridor["nets"] == ["DN", "DP"]
    assert corridor["side"] == "front"
    assert corridor["margin_nm"] == nm_from_mm("0.55")
    assert corridor["terminal_components"] == ["J1", "U1"]
    assert len(corridor["polygon_nm"]) >= 4
    assert json.loads(first)["skipped_corridors"] == []


def test_plan_layout_cli_reports_reserved_corridor(tmp_path: Path) -> None:
    source = tmp_path / "board.copper"
    source.write_text('''
board CorridorCli {
    use library "tiny";
    component R1: RESISTOR { footprint = "0402"; }
    component R2: RESISTOR { footprint = "0402"; }
    component R3: RESISTOR { footprint = "0402"; }
    component C1: CAPACITOR { footprint = "0402"; }
    net DP { R1.1; R2.1; }
    net DN { R1.2; R2.2; }
    net SENSE_A { R3.1; C1.1; }
    net SENSE_B { R3.2; C1.2; }
    constraint fixed_placement(R1) { x = 40mm; y = 40mm; rotation = 90; side = front; }
    constraint fixed_placement(R2) { x = 60mm; y = 40mm; rotation = 90; side = front; }
    constraint fixed_placement(R3) { x = 50mm; y = 44mm; rotation = 0; side = front; }
    constraint routing(DP) {
        kind = differential;
        partner = DN;
        width = 0.15mm;
        pair_gap = 0.15mm;
        clearance = 0.15mm;
        reserve_corridor = true;
    }
}
''', encoding="utf-8")
    report = tmp_path / "layout.json"
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "plan-layout", str(source),
         "-o", str(tmp_path / "planned.kicad_pcb"), "--report", str(report),
         "--allow-proxy-footprints", "--candidates", "1"],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    document = json.loads(report.read_text(encoding="utf-8"))
    (corridor,) = document["reserved_corridors"]
    assert corridor["nets"] == ["DN", "DP"]
    assert corridor["terminal_components"] == ["R1", "R2"]
    assert corridor["side"] == "front"
    # 0.15 width + 0.15 gap + 0.2 board minimum clearance.
    assert corridor["margin_nm"] == nm_from_mm("0.5")
    assert any(item["code"] == "RESERVED_CORRIDORS" for item in document["findings"])

"""Physical nearest-choice trap: a scarce exit must survive an easy neighbor."""
from dataclasses import replace
import pytest
import pcbir.fanout as fanout
from pcbir.drc import run_physical_drc
from pcbir.physical import (BoardOutline, CopperLayer, CopperKeepout, CopperZone, FootprintPad,
    NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, PolygonRing, PolygonWithHoles, RouteKind, Size, nm_from_mm)


def board():
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, 1), Size.mm(.3, .3)),
    ), Size.mm(2, 3))
    terminal = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    return PhysicalBoard("trap", BoardOutline.rectangle(14, 14),
        {package.name: package, terminal.name: terminal}, (
            Placement("U", package.name, Point.mm(5, 7)),
            Placement("JA", terminal.name, Point.mm(11, 6)),
            Placement("JB", terminal.name, Point.mm(11, 8)),
        ), (PhysicalNet("A", (PadReference("U", "1"), PadReference("JA", "1"))),
            PhysicalNet("B", (PadReference("U", "2"), PadReference("JB", "1")))))


def options(**kwargs):
    return fanout.FanoutOptions(minimum_component_pads=2,
                               maximum_neighbor_distance_nm=nm_from_mm(3), **kwargs)


def candidates(position, center, options):
    return (Point.mm(3, 8), Point.mm(3, 6)) if position.y_nm == nm_from_mm(6) else (Point.mm(3, 8),)


def test_low_slack_exit_survives_earlier_easy_neighbor(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout, "_candidates", candidates)
    greedy = fanout.route_fanout(base, options(constrained_pins_first=False))
    selected = fanout.route_fanout(base, options())
    assert greedy.pending_pads == (PadReference("U", "2"),)
    assert selected.pending_pads == ()
    assert selected.accesses[PadReference("U", "2")] == Point.mm(3, 8)
    assert selected.accesses[PadReference("U", "1")] == Point.mm(3, 6)
    assert [(p.pad.pad, p.legal_candidate_count, p.selected_candidate_index)
            for p in selected.pin_analysis] == [("2", 1, 0), ("1", 2, 1)]
    assert greedy.pin_analysis[-1].diagnostic == "all initial candidates blocked by selected escapes"
    assert selected == fanout.route_fanout(base, options())
    assert selected.board.placements == base.placements
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-DRILL-SPACING"}
                   for f in run_physical_drc(selected.board).findings)


def test_subset_preserves_input_copper_and_excludes_unselected_pins(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout, "_candidates", candidates)
    first = fanout.route_fanout(base, options(), only_nets=frozenset({"B"}))
    second = fanout.route_fanout(first.board, options(), only_nets=frozenset({"A"}))
    assert not second.pending_pads
    assert second.board.tracks[:len(first.board.tracks)] == first.board.tracks
    assert second.board.vias[:len(first.board.vias)] == first.board.vias
    assert set(second.accesses) == {PadReference("U", "1")}
    empty = fanout.route_fanout(second.board, options(), only_nets=frozenset())
    assert empty.board is second.board and not empty.accesses and not empty.created_vias
    with pytest.raises(ValueError, match="unknown fanout nets"):
        fanout.route_fanout(base, options(), only_nets=frozenset({"missing"}))


def test_locked_input_exit_retained_when_domain_is_empty(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout, "_candidates", candidates)
    first = fanout.route_fanout(base, options(), only_nets=frozenset({"A"}))
    second = fanout.route_fanout(first.board, options(), only_nets=frozenset({"B"}))
    assert second.pending_pads == (PadReference("U", "2"),)
    assert second.pin_analysis[0].legal_candidate_count == 0
    assert second.pin_analysis[0].diagnostic == "no legal candidate against immutable input"
    assert second.board is first.board and not second.created_vias


def test_critical_and_zone_nets_never_enter_candidate_domains(monkeypatch):
    base = board()
    base = replace(base, net_routing_rules=(NetRoutingRule("A", kind=RouteKind.RF_FEED),),
        zones=(CopperZone("ground", "B", (CopperLayer.BACK,),
                          PolygonWithHoles(PolygonRing(base.outline.vertices))),))
    monkeypatch.setattr(fanout, "_candidates", lambda *args: (_ for _ in ()).throw(AssertionError("protected pin escaped")))
    result = fanout.route_fanout(base, options())
    assert result.board is base and not result.accesses and not result.pending_pads


def test_failed_final_native_gate_rolls_back_all_candidate_copper(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout, "_candidates", candidates)
    from types import SimpleNamespace
    monkeypatch.setattr(fanout, "run_physical_drc", lambda b: SimpleNamespace(
        findings=(SimpleNamespace(code="DRC-CLEARANCE"),) if b.tracks else ()))
    result = fanout.route_fanout(base, options())
    assert result.board is base and not result.accesses and not result.created_vias
    assert set(result.pending_pads) == {PadReference("U", "1"), PadReference("U", "2")}
    assert all(p.selected_candidate_index is None and "rejected" in p.diagnostic
               for p in result.pin_analysis)


def test_real_candidate_generation_with_two_physical_via_windows():
    base = board()
    package = replace(base.footprints["two"], pads=(
        replace(base.footprints["two"].pads[0], position=Point.mm(-1, -1)),
        replace(base.footprints["two"].pads[1], position=Point.mm(-3, 1)),
    ))
    # Rectangle tiling avoids the router's deliberately conservative handling
    # of keepouts with holes; only two physical windows accept a whole via.
    xs, ys = (0, 2.3, 3.7, 4.3, 5.7, 14), (0, 3.3, 4.7, 5.3, 6.7, 14)
    keepouts = tuple(CopperKeepout(f"cell-{i}-{j}", base.stackup.copper_layers,
        PolygonWithHoles(PolygonRing((Point.mm(x, y), Point.mm(xx, y),
                                     Point.mm(xx, yy), Point.mm(x, yy)))),
        block_tracks=False, block_vias=True, block_zones=False)
        for i, (x, xx) in enumerate(zip(xs, xs[1:]))
        for j, (y, yy) in enumerate(zip(ys, ys[1:]))
        if (i, j) not in {(1, 3), (3, 1)})
    base = replace(base, footprints={**base.footprints, "two": package},
        placements=(replace(base.placements[0], position=Point.mm(6, 7)), *base.placements[1:]),
        copper_keepouts=keepouts)
    settings = fanout.FanoutOptions(minimum_component_pads=2,
                                    maximum_neighbor_distance_nm=nm_from_mm(5))
    greedy = fanout.route_fanout(base, replace(settings, constrained_pins_first=False))
    selected = fanout.route_fanout(base, settings)
    assert greedy.pending_pads == (PadReference("U", "2"),)
    assert selected.pending_pads == ()
    assert selected.accesses[PadReference("U", "2")] == Point.mm(3, 6)
    assert selected.accesses[PadReference("U", "1")] == Point.mm(5, 4)
    assert not any(f.code == "DRC-COPPER-KEEPOUT" for f in run_physical_drc(selected.board).findings)


def test_pin_analysis_is_observational_not_geometry(monkeypatch):
    base = board()
    monkeypatch.setattr(fanout, "_candidates", candidates)
    measured = fanout.route_fanout(base, options())
    monkeypatch.setattr(fanout, "FanoutPinAnalysis", lambda *args: None)
    unmeasured = fanout.route_fanout(base, options())
    assert measured.board == unmeasured.board
    assert measured.accesses == unmeasured.accesses
    assert measured.created_vias == unmeasured.created_vias
    assert measured.pending_pads == unmeasured.pending_pads

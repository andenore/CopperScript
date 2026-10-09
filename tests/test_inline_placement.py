from dataclasses import replace
from decimal import Decimal

from pcbir import (
    BoardOutline, FootprintPad, NetRoutingRule, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, PlacementPlannerOptions, Point, RouteKind, Size,
    PadReference, nm_from_mm,
)
from pcbir.placement import (
    _detailed_refine, _fast_score, _inline_pair_candidates,
    placement_solution_is_legal,
)


def _board(*, paired: bool = True) -> PhysicalBoard:
    terminal = PhysicalFootprint("terminal", (
        FootprintPad("D", Point.mm(0, -0.3), Size.mm(0.4, 0.4)),
        FootprintPad("M", Point.mm(0, 0.3), Size.mm(0.4, 0.4)),
    ), Size.mm(2, 2))
    filter_fp = PhysicalFootprint("filter", (
        FootprintPad("1", Point.mm(-0.275, -0.655), Size.mm(0.3, 0.3)),
        FootprintPad("2", Point.mm(-0.275, 0.655), Size.mm(0.3, 0.3)),
        FootprintPad("3", Point.mm(0.275, 0.655), Size.mm(0.3, 0.3)),
        FootprintPad("4", Point.mm(0.275, -0.655), Size.mm(0.3, 0.3)),
    ), Size.mm(1.36, 2.4))
    obstacle = PhysicalFootprint("obstacle", (), Size.mm(24, 5))
    names = (("LEFT_DP", "A", "D", "F", "2"),
             ("LEFT_DM", "A", "M", "F", "3"),
             ("RIGHT_DP", "B", "D", "F", "1"),
             ("RIGHT_DM", "B", "M", "F", "4"))
    nets = tuple(PhysicalNet(name, (PadReference(a, apad), PadReference(f, fpad)))
                 for name, a, apad, f, fpad in names)
    rules = tuple(NetRoutingRule(name, kind=RouteKind.DIFFERENTIAL,
                                 differential_partner=("LEFT_DM" if name == "LEFT_DP" else
                                                       "LEFT_DP" if name == "LEFT_DM" else
                                                       "RIGHT_DM" if name == "RIGHT_DP" else "RIGHT_DP"),
                                 pair_gap_nm=nm_from_mm("0.2")) for name, *_ in names) if paired else ()
    return PhysicalBoard("InlinePair", BoardOutline.rectangle(40, 35),
                         {f.name: f for f in (terminal, filter_fp, obstacle)},
                         (Placement("A", "terminal", Point.mm(6, 15)),
                          Placement("B", "terminal", Point.mm(34, 15)),
                          Placement("F", "filter", Point.mm(20, 27)),
                          Placement("O", "obstacle", Point.mm(20, 18))),
                         nets, net_routing_rules=rules)


def test_inline_pair_refinement_reaches_corridor_beyond_local_radius() -> None:
    board = _board()
    original = {p.reference: p for p in board.placements}
    options = PlacementPlannerOptions(refinement_passes=1, fixed_references={"A", "B", "O"})
    proposals = _inline_pair_candidates(board, original, "F", options)
    assert any(p.position.y_nm <= nm_from_mm(13)
               and p.rotation_degrees == Decimal(270) for p in proposals)

    refined, *_ = _detailed_refine(board, original, options, seed=0)
    assert refined["F"].position.y_nm <= nm_from_mm(13)
    assert abs(refined["F"].position.y_nm - original["F"].position.y_nm) > nm_from_mm(2)
    assert placement_solution_is_legal(board, refined, options)
    assert _fast_score(board, refined) < _fast_score(board, original)


def test_inline_proposals_require_explicit_two_pair_topology() -> None:
    board = _board(paired=False)
    poses = {p.reference: p for p in board.placements}
    assert _inline_pair_candidates(board, poses, "F", PlacementPlannerOptions()) == ()
    assert _inline_pair_candidates(replace(board, net_routing_rules=()), poses,
                                   "A", PlacementPlannerOptions()) == ()

    board = _board()
    mixed_rules = tuple(replace(rule, kind=RouteKind.GENERAL)
                        if rule.net == "LEFT_DM" else rule
                        for rule in board.net_routing_rules)
    assert _inline_pair_candidates(replace(board, net_routing_rules=mixed_rules),
                                   poses, "F", PlacementPlannerOptions()) == ()

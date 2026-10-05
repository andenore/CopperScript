from dataclasses import replace
from types import SimpleNamespace

from pcbir import (BoardOutline, FootprintPad, PhysicalBoard, PhysicalFootprint,
                   Placement, PadReference, Point, Size)
from pcbir.critical import _pair_prefers_via_escape


def setup(perimeter=False):
    pads = (FootprintPad("1", Point.mm(3 if perimeter else 0, 0), Size.mm(.3, .3)),
            FootprintPad("2", Point.mm(3 if perimeter else .5, -.5), Size.mm(.3, .3)),
            FootprintPad("3", Point.mm(-3, -3), Size.mm(.3, .3)),
            FootprintPad("4", Point.mm(3, 3), Size.mm(.3, .3)))
    fp = PhysicalFootprint("package", pads, Size.mm(7, 7))
    board = PhysicalBoard("order", BoardOutline.rectangle(20, 20), {fp.name: fp},
                          (Placement("U", fp.name, Point.mm(10, 10)),), ())
    return board, [SimpleNamespace(accesses=(SimpleNamespace(pad=PadReference("U", n)),)) for n in ("1", "2")]


def test_internal_pair_prefers_vias_but_perimeter_pair_retains_surface_first():
    board, guides = setup()
    assert _pair_prefers_via_escape(board, *guides)
    board, guides = setup(perimeter=True)
    assert not _pair_prefers_via_escape(board, *guides)


def test_duplicate_land_on_perimeter_blocks_internal_classification():
    board, guides = setup()
    fp = board.footprints["package"]
    fp = replace(fp, pads=(*fp.pads, FootprintPad("1", Point.mm(3, 0), Size.mm(.3, .3))))
    board = replace(board, footprints={fp.name: fp})
    assert not _pair_prefers_via_escape(board, *guides)


def test_pose_does_not_change_local_internal_land_classification():
    board, guides = setup()
    board = replace(board, placements=(replace(board.placements[0], rotation_degrees=45),))
    assert _pair_prefers_via_escape(board, *guides)


def test_ordering_is_in_report_and_progress_telemetry():
    import json
    from pcbir.critical import CriticalNetResult, CriticalRoutingResult, CriticalRoutingStatus
    from pcbir.progress import critical_progress
    board, _ = setup()
    net = CriticalNetResult(("P", "N"), True, 0, 0, (), 0,
                            pair_search_order="via_first_internal_lands",
                            pair_state_limit=6000, pair_budget_exhausted=False)
    result = CriticalRoutingResult(CriticalRoutingStatus.SUCCESS, board, (net,), (), (), "g", "r")
    assert json.loads(result.to_json())["nets"][0]["pair_search_order"] == "via_first_internal_lands"
    seen = []
    critical_progress(lambda phase, event, details: seen.append(details))("finished", net.nets, net)
    assert seen[0]["pair_search_order"] == "via_first_internal_lands"
    assert seen[0]["pair_state_limit"] == 6000 and not seen[0]["pair_budget_exhausted"]
    assert json.loads(result.to_json())["nets"][0]["pair_state_limit"] == 6000

"""Every separate land of a pad number is routed unless the part declares it internally joined."""
import json

from pcbir.bench import route_board_file
from pcbir.detailed import _separate_lands
from pcbir.importers.kicad_pcb import load_kicad_pcb, parse_kicad_pcb
from pcbir.physical import nm_from_mm

BOARD = """(kicad_pcb (version 20260206) (generator "test")
  (layers (0 "F.Cu" signal) (2 "B.Cu" signal) (25 "Edge.Cuts" user))
  (net 0 "") (net 1 "SHIELD")
  (footprint "LUGS" (layer "F.Cu") (at 10 10)
    (property "Reference" "J1" (at 0 0 0) (layer "F.SilkS"))
    {jumpers}
    (pad "1" thru_hole circle (at 0 0) (size 1.8 1.8) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SHIELD"))
    (pad "1" thru_hole circle (at 12 0) (size 1.8 1.8) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SHIELD"))
    (pad "1" smd rect (at 0 3) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "SHIELD"))
    (pad "1" smd rect (at 0.9 3) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "SHIELD")))
  (footprint "TP" (layer "F.Cu") (at 16 22)
    (property "Reference" "TP1" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd circle (at 0 0) (size 1.5 1.5) (layers "F.Cu" "F.Mask") (net 1 "SHIELD")))
  (gr_rect (start 0 0) (end 32 30) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
)"""


def _board(jumpers=""):
    return BOARD.replace("{jumpers}", jumpers)


def test_separate_lands_skip_the_first_land_and_touching_lands():
    board = parse_kicad_pcb(_board(), source="lands").board
    placement = next(p for p in board.placements if p.reference == "J1")
    lands = _separate_lands(placement, board.footprints[placement.footprint], "1")
    # Four lands: the second through-hole lug and one piece for the two touching surface lands.
    positions = sorted((point.x_nm, point.y_nm) for _, point in lands)
    assert positions == [(nm_from_mm(10), nm_from_mm(13)), (nm_from_mm(22), nm_from_mm(10))]


def _route(tmp_path, text):
    path = tmp_path / "board.kicad_pcb"
    path.write_text(text)
    record = route_board_file(path, tmp_path / "out")
    routed = (tmp_path / "out" / "routed.kicad_pcb").read_text()
    return record, routed


def test_every_separate_land_is_routed(tmp_path):
    record, routed = _route(tmp_path, _board())
    assert record["status"] == "routed" and record["unrouted_nets"] == []
    # The far lug at (22, 10) carries copper to the rest of the pin.
    assert "(end 22 10)" in routed or "(start 22 10)" in routed


def test_declared_internal_group_is_not_routed_between_its_lands(tmp_path):
    record, routed = _route(tmp_path, _board('(duplicate_pad_numbers_are_jumpers no) (jumper_pad_groups ("1"))'))
    assert record["status"] == "routed" and record["unrouted_nets"] == []
    imported = load_kicad_pcb(tmp_path / "board.kicad_pcb").board
    assert next(f for f in imported.footprints.values() if f.internal_pad_groups).internal_pad_groups
    assert "(end 22 10)" not in routed and "(start 22 10)" not in routed

"""Surface pads straddling the board edge are routed from their copper inside the board (D3 plan 3.6)."""
from pcbir.bench import route_board_file
from pcbir.importers.kicad_pcb import parse_kicad_pcb
from pcbir.physical import Size, nm_from_mm
from pcbir.placement import transformed_pad_position

# Two 5 x 10 mm wire pads overhang an 8 mm tall, 22 mm wide board on three sides
# (D3-A board 0034, NavigationThing backlight), joined by one net.
BOARD = """(kicad_pcb (version 20260206) (generator "test")
  (layers (0 "F.Cu" signal) (2 "B.Cu" signal) (25 "Edge.Cuts" user))
  (net 0 "") (net 1 "SIG")
  (footprint "PAD" (layer "F.Cu") (at -0.5 4 {rotation})
    (property "Reference" "P1" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd {shape} (at 0 0 {rotation}) (size 5 10) (layers "F.Cu" "F.Mask") (net 1 "SIG")))
  (footprint "PAD" (layer "F.Cu") (at 22.5 4)
    (property "Reference" "P2" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd rect (at 0 0) (size 5 10) (layers "F.Cu" "F.Mask") (net 1 "SIG")))
  (footprint "TP" (layer "F.Cu") (at 11 4)
    (property "Reference" "TP1" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "SIG")))
  (gr_rect (start 0 0) (end 22 8) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
)"""


def _board(shape="rect", rotation=180):
    return BOARD.replace("{shape}", shape).replace("{rotation}", str(rotation))


def _pad(board, reference):
    placement = next(p for p in board.placements if p.reference == reference)
    pad = board.footprints[placement.footprint].pads[0]
    return placement, pad, transformed_pad_position(board, placement, "1")


def test_straddling_pads_are_clipped_to_the_board():
    imported = parse_kicad_pcb(_board(), source="edge")
    board = imported.board
    for reference, centre_x in (("P1", nm_from_mm(1)), ("P2", nm_from_mm(21))):
        placement, pad, centre = _pad(board, reference)
        assert (centre.x_nm, centre.y_nm) == (centre_x, nm_from_mm(4))
        assert pad.size == Size(nm_from_mm(2), nm_from_mm(8))
        assert placement.footprint.endswith(f"~clipped~{reference}")
    # The test point inside the board shares the original footprint name space untouched.
    _, pad, centre = _pad(board, "TP1")
    assert pad.size == Size(nm_from_mm(1), nm_from_mm(1)) and centre.x_nm == nm_from_mm(11)
    assert any(item.startswith("pad clipped to the board outline: 2") for item in imported.approximations)


def test_rotated_pad_is_clipped_in_its_own_frame():
    # At 90 degrees the 5 x 10 pad lies 10 wide and 5 tall (x -5.5..4.5): 4.5 mm of it is inside.
    board = parse_kicad_pcb(_board(rotation=90), source="edge").board
    _, pad, centre = _pad(board, "P1")
    assert (centre.x_nm, centre.y_nm) == (nm_from_mm("2.25"), nm_from_mm(4))
    # Board-frame 4.5 x 5 is 5 x 4.5 in the pad's own (rotated) frame.
    assert pad.size == Size(nm_from_mm(5), nm_from_mm("4.5"))


def test_round_pads_are_left_as_they_are():
    board = parse_kicad_pcb(_board(shape="circle"), source="edge").board
    _, pad, centre = _pad(board, "P1")
    assert centre.x_nm == nm_from_mm("-0.5") and pad.size == Size(nm_from_mm(5), nm_from_mm(10))


def test_straddling_pads_route(tmp_path):
    path = tmp_path / "board.kicad_pcb"
    path.write_text(_board())
    record = route_board_file(path, tmp_path / "out")
    assert record["status"] == "routed" and record["unrouted_nets"] == []
    # The written board keeps the original footprints.
    assert "(size 5 10)" in (tmp_path / "out" / "routed.kicad_pcb").read_text()

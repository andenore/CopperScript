from pathlib import Path
import re
import uuid

from pcbir import (
    BoardOutline,
    CopperLayer,
    FootprintPad,
    KiCadPcbBackend,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    TrackSegment,
    Via,
    compile_file,
    nm_from_mm,
    prototype_physicalize,
)


ROOT = Path(__file__).parents[1]


def test_kicad_pcb_backend_emits_deterministic_board_geometry() -> None:
    board = _routed_board()
    backend = KiCadPcbBackend()

    first = backend.generate(board)
    second = backend.generate(board)

    assert first == second
    assert first.backend == "kicad-pcb"
    assert first.target_version == "8.0"
    assert first.warnings == ()
    assert first.artifacts[0].name == "BackendTest.kicad_pcb"
    pcb = first.artifacts[0].content
    assert pcb.startswith("(kicad_pcb\n  (version 20240108)")
    assert '(net 1 "GND")' in pcb
    assert '(net 2 "VIN")' in pcb
    assert '(footprint "test/two_pin"' in pcb
    assert '(pad "1" smd roundrect' in pcb
    assert '(segment' in pcb
    assert '(via' in pcb
    assert '(38 "B.Mask" user)' in pcb
    assert '(39 "F.Mask" user)' in pcb
    assert pcb.count('(layer "Edge.Cuts")') == 4
    assert _parentheses_are_balanced(pcb)

    identifiers = re.findall(r'\(uuid "([0-9a-f-]+)"\)', pcb)
    assert identifiers
    assert len(identifiers) == len(set(identifiers))
    assert all(uuid.UUID(identifier).version == 4 for identifier in identifiers)


def test_kicad_pcb_backend_marks_proxy_board_as_non_fabrication_ready() -> None:
    electrical = compile_file(ROOT / "examples" / "valid_board.copper")
    physical = prototype_physicalize(electrical)

    manifest = KiCadPcbBackend().generate(physical)

    assert len(manifest.warnings) == 3
    assert "proxy footprints" in manifest.warnings[0]
    assert "draft placement" in manifest.warnings[1]
    assert "no routed tracks" in manifest.warnings[2]
    assert '(property "Reference" "U2"' in manifest.artifacts[0].content
    assert '(net 4 "V3V3")' in manifest.artifacts[0].content


def test_rotated_footprint_exports_pad_angles_in_board_coordinates() -> None:
    from dataclasses import replace

    board = _routed_board()
    placement = replace(board.placements[0], rotation_degrees=90)
    rotated = replace(board, placements=(placement, board.placements[1]))

    pcb = KiCadPcbBackend().generate(rotated).artifacts[0].content

    assert '(at -1 0 90)' in pcb
    assert '(at 1 0 90)' in pcb


def _routed_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        name="test/two_pin",
        pads=(
            FootprintPad("1", Point.mm(-1, 0), Size.mm(1, 1)),
            FootprintPad("2", Point.mm(1, 0), Size.mm(1, 1)),
        ),
        body_size=Size.mm(1.6, 0.8),
    )
    return PhysicalBoard(
        name="BackendTest",
        outline=BoardOutline.rectangle(40, 30),
        footprints={footprint.name: footprint},
        placements=(
            Placement("R1", footprint.name, Point.mm(10, 10), value="1 kohm"),
            Placement("R2", footprint.name, Point.mm(20, 10), value="2 kohm"),
        ),
        nets=(
            PhysicalNet(
                "VIN", (PadReference("R1", "1"), PadReference("R2", "1"))
            ),
            PhysicalNet(
                "GND", (PadReference("R1", "2"), PadReference("R2", "2"))
            ),
        ),
        tracks=(
            TrackSegment(
                "VIN",
                Point.mm(9, 10),
                Point.mm(19, 10),
                nm_from_mm("0.25"),
                CopperLayer.FRONT,
            ),
            TrackSegment(
                "GND",
                Point.mm(11, 10),
                Point.mm(15, 10),
                nm_from_mm("0.25"),
                CopperLayer.FRONT,
            ),
            TrackSegment(
                "GND",
                Point.mm(15, 10),
                Point.mm(21, 10),
                nm_from_mm("0.25"),
                CopperLayer.BACK,
            ),
        ),
        vias=(
            Via(
                "GND",
                Point.mm(15, 10),
                nm_from_mm("0.8"),
                nm_from_mm("0.4"),
            ),
        ),
    )


def _parentheses_are_balanced(text: str) -> bool:
    depth = 0
    quoted = False
    escaped = False
    for character in text:
        if escaped:
            escaped = False
        elif character == "\\" and quoted:
            escaped = True
        elif character == '"':
            quoted = not quoted
        elif not quoted and character == "(":
            depth += 1
        elif not quoted and character == ")":
            depth -= 1
            if depth < 0:
                return False
    return depth == 0 and not quoted

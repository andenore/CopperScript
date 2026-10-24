"""Required source arrays are reusable path edges, never new via permissions."""
from dataclasses import replace
from collections import Counter
import pytest
from test_decoupling_crosslayer_paths import fixture
from pcbir.physical import (CopperLayer, CopperZone, PolygonRing, PolygonWithHoles,
    Point, Size, PadReference, PadViaInPadRule, TrackSegment)
from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
from pcbir.decoupling_paths import (propose_crosslayer_decoupling,
    verify_crosslayer_decoupling, validate_crosslayer_witness)


def array_fixture(rows):
    board, link = fixture()
    old_return = link.return_net
    link = replace(link, return_net="GND")
    chip = board.footprints["chip"]
    chip = replace(chip, pads=tuple(replace(p, size=Size.mm(2.6, 2.6))
        if p.number == "G" else p for p in chip.pads))
    board = replace(board, footprints={**board.footprints, "chip": chip},
        metadata={"fabrication_profile": "jlcpcb-six-layer"},
        nets=tuple(replace(n, name="GND") if n.name == old_return else n for n in board.nets),
        net_routing_rules=tuple(replace(r, net="GND", max_vias=rows*rows)
            if r.net == old_return else r for r in board.net_routing_rules),
        decoupling_links=(link,),
        zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,),
            PolygonWithHoles(PolygonRing((Point.mm(1,1), Point.mm(24,1),
                Point.mm(24,19), Point.mm(1,19))))),),
        via_in_pad_rules=(PadViaInPadRule(PadReference(link.target.component,"G"),
            rows=rows, columns=rows),))
    return materialize_via_in_pad_arrays(board), link


@pytest.mark.parametrize("rows", (1, 3))
def test_bypass_reuses_required_array_without_extra_return_via(rows):
    board, link = array_fixture(rows)
    result = propose_crosslayer_decoupling(board, link)
    assert result.witness is not None, result.reason
    assert result.witness.return_path.vias
    assert all(v.finish == "filled-capped" for v in result.witness.return_path.vias)
    assert not any(v.net == "GND" for v in result.created_vias)
    assert not Counter(board.vias) - Counter(result.board.vias)
    assert all(a is b for a,b in zip(board.vias,result.board.vias))
    assert validate_crosslayer_witness(result.board,result.witness) == ""
    again = propose_crosslayer_decoupling(result.board,link)
    assert again.board is result.board and not again.created_tracks and not again.created_vias


@pytest.mark.parametrize("fault", ("missing", "undeclared", "moved", "foreign_track"))
def test_source_array_path_requires_declaration_occurrences_and_fresh_clearance(fault):
    board, link = array_fixture(1)
    result = propose_crosslayer_decoupling(board,link)
    assert result.witness
    array = board.vias[0]
    if fault == "missing":
        changed = replace(result.board,vias=tuple(v for v in result.board.vias if v is not array))
    elif fault == "undeclared":
        changed = replace(result.board,via_in_pad_rules=())
    elif fault == "moved":
        changed = replace(result.board,vias=tuple(replace(v,position=Point.mm(9.1,10))
            if v is array else v for v in result.board.vias))
    else:
        changed = replace(result.board,tracks=(*result.board.tracks,
            TrackSegment(link.net,Point.mm(8.5,10),Point.mm(9.5,10),150000,CopperLayer.BACK)))
    assert validate_crosslayer_witness(changed,result.witness)
    assert verify_crosslayer_decoupling(changed,link).witness is None

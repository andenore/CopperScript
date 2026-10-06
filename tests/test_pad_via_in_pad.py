"""Pad-scoped permissions never enable via-in-pad on neighboring lands."""
from dataclasses import replace

import pytest

from pcbir import compile_source, prototype_physicalize, PrototypePhysicalOptions, Size
from pcbir.physical import PadReference, PadViaInPadRule, Stackup, CopperLayer
from pcbir.plane import stitch_zone_pads
from test_plane_stitch import _plane_board


def qualified_board():
    base = _plane_board()
    fp = replace(base.footprints["test/one-pad"], pads=(
        replace(base.footprints["test/one-pad"].pads[0], size=Size.mm(8,8)),))
    return replace(base, footprints={fp.name: fp},
        stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,CopperLayer.INTERNAL_2,
                        CopperLayer.INTERNAL_3,CopperLayer.INTERNAL_4,CopperLayer.BACK)),
        metadata={"fabrication_profile":"jlcpcb-six-layer"})


def test_only_explicit_ground_pad_gets_filled_capped_via():
    base = qualified_board()
    assert set(stitch_zone_pads(base).pending_pads) == {PadReference("J1","1"),PadReference("J2","1")}
    board = replace(base, via_in_pad_rules=(PadViaInPadRule(PadReference("J1","1")),))
    result = stitch_zone_pads(board)
    assert result.stitched_pads == (PadReference("J1","1"),)
    assert result.pending_pads == (PadReference("J2","1"),)
    assert len(result.board.vias) == 1
    assert result.board.vias[0].finish == "filled-capped"
    assert result.board.vias[0].position == board.placements[0].position
    again = stitch_zone_pads(result.board)
    assert len(again.board.vias) == 1


@pytest.mark.parametrize("pad",[PadReference("MISSING","1"),PadReference("J1","9")])
def test_missing_unconnected_pad_is_rejected(pad):
    with pytest.raises(ValueError,match="connected GND"):
        replace(qualified_board(),via_in_pad_rules=(PadViaInPadRule(pad),))


def test_permission_rejects_wrong_process_profile_and_duplicate():
    with pytest.raises(ValueError,match="filled-capped"):
        PadViaInPadRule(PadReference("J1","1"),"open")
    rule = PadViaInPadRule(PadReference("J1","1"))
    with pytest.raises(ValueError,match="six-layer"):
        replace(qualified_board(),metadata={},via_in_pad_rules=(rule,))
    with pytest.raises(ValueError,match="multiple"):
        replace(qualified_board(),via_in_pad_rules=(rule,rule))


def test_permission_does_not_allow_contact_with_another_same_net_pad():
    from pcbir.physical import Placement, Point
    board = qualified_board()
    large = board.footprints["test/one-pad"]
    tiny = replace(large,name="tiny",pads=(replace(large.pads[0],size=Size.mm(.2,.2)),))
    board = replace(board,footprints={**board.footprints,"tiny":tiny},
        placements=(board.placements[0],Placement("J2","tiny",Point.mm(3.2,6))),
        via_in_pad_rules=(PadViaInPadRule(PadReference("J1","1")),))
    result = stitch_zone_pads(board)
    assert PadReference("J1","1") in result.pending_pads
    assert not any(v.finish=="filled-capped" for v in result.board.vias)


def source(target="BT1.2",process="filled-capped",extra=""):
    return '''board Test { use library "tiny";
        component BT1: CAPACITOR;
        net GND { BT1.2; }
        constraint copper_zone(GND) { layers = "In1.Cu"; }
        constraint via_in_pad('''+target+''') { process = "'''+process+'''"; '''+extra+''' }
    }'''


def test_source_constraint_lowers_semantic_pin_to_typed_physical_permission():
    board = prototype_physicalize(compile_source(source()),
        PrototypePhysicalOptions(copper_layers=6,fabrication_profile="jlcpcb-six-layer"))
    assert board.via_in_pad_rules == (PadViaInPadRule(PadReference("BT1","2")),)


@pytest.mark.parametrize("target,process,extra",[
    ("BT1","filled-capped",""),("NOPE.2","filled-capped",""),
    ("BT1.1","filled-capped",""),("BT1.2","open",""),
    ("BT1.2","filled-capped","typo = 1;"),
])
def test_invalid_source_permission_fails_closed(target,process,extra):
    with pytest.raises(ValueError):
        prototype_physicalize(compile_source(source(target,process,extra)),
            PrototypePhysicalOptions(copper_layers=6,fabrication_profile="jlcpcb-six-layer"))


def test_editor_projection_digest_keeps_via_in_pad_profile(tmp_path):
    from pcbir.drc import physical_board_digest
    from pcbir.editor.overlay import projection_digest, write_intent
    board = replace(qualified_board(), via_in_pad_rules=(PadViaInPadRule(PadReference("J1", "1")),))
    # Stripping all metadata would fail the via-in-pad profile validation.
    assert projection_digest(board) == projection_digest(replace(board, metadata={**board.metadata, "run": "x"}))
    source = tmp_path / "board.copper"
    source.write_text("board B {}\n")
    write_intent(source, board, stitch_zone_pads(board).board, tmp_path / "board.editor-intent.json")
    # Boards without permissions keep the metadata-free digest they had before.
    plain = qualified_board()
    assert projection_digest(plain) == physical_board_digest(
        replace(plain, tracks=(), vias=(), zone_fills=(), materialized_macros=(), metadata={}))

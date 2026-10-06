"""Declared board stack-up (D-PHY plan L1)."""

from decimal import Decimal

import pytest

from pcbir import (
    CopperLayer,
    KiCadPcbBackend,
    PrototypePhysicalOptions,
    StackupLayerKind,
    compile_design_source,
    compile_source,
    nm_from_mm,
    physical_board_digest,
    prototype_physicalize,
)
from pcbir.editor.source import SourceSnapshot, declaration_index, mechanical_patch
from pcbir.syntax import CopperScriptError


SIX_LAYER = """
    stackup {
        copper F.Cu { thickness = 0.035mm; }
        dielectric P1 { thickness = 0.1mm; er = 4.1; loss_tangent = 0.02; material = "3313"; type = prepreg; }
        copper In1.Cu { thickness = 0.0175mm; }
        dielectric C1 { thickness = 0.55mm; er = 4.6; type = core; }
        copper In2.Cu { thickness = 0.0175mm; }
        dielectric P2 { thickness = 0.1mm; er = 4.1; }
        copper In3.Cu { thickness = 0.0175mm; }
        dielectric C2 { thickness = 0.55mm; er = 4.6; }
        copper In4.Cu { thickness = 0.0175mm; }
        dielectric P3 { thickness = 0.1mm; er = 4.1; material = "3313"; }
        copper B.Cu { thickness = 0.035mm; }
    }
"""

FOUR_LAYER = """
    stackup {
        copper F.Cu { thickness = 0.035mm; }
        dielectric P1 { thickness = 0.2mm; er = 4.3; }
        copper In1.Cu { thickness = 0.0175mm; }
        dielectric C1 { thickness = 1.065mm; er = 4.6; }
        copper In2.Cu { thickness = 0.0175mm; }
        dielectric P2 { thickness = 0.2mm; er = 4.3; }
        copper B.Cu { thickness = 0.035mm; }
    }
"""


def source(stackup: str, extra: str = "") -> str:
    return f"""board Stack {{
        mechanical {{
            outline rectangle {{ width = 30mm; height = 20mm; }}
            {stackup}
            {extra}
        }}
    }}"""


def test_six_layer_stackup_lowers_to_physical_layers() -> None:
    design = compile_design_source(source(SIX_LAYER), "stack.copper")
    declared = design.mechanical.stackup
    assert declared.copper_layers == (
        CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
        CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)
    board = prototype_physicalize(design, PrototypePhysicalOptions(copper_layers=6))
    layers = board.stackup.physical_layers
    assert len(layers) == 11
    assert [layer.kind for layer in layers] == [
        StackupLayerKind.COPPER if index % 2 == 0 else StackupLayerKind.DIELECTRIC
        for index in range(11)]
    assert board.stackup.thickness_nm == sum(layer.thickness_nm for layer in layers) == nm_from_mm("1.54")
    prepreg = layers[1]
    assert (prepreg.id, prepreg.thickness_nm, prepreg.relative_permittivity, prepreg.loss_tangent,
            prepreg.material, prepreg.dielectric_type) == (
        "P1", nm_from_mm("0.1"), Decimal("4.1"), Decimal("0.02"), "3313", "prepreg")
    assert layers[3].dielectric_type == "core" and layers[5].dielectric_type is None
    assert layers[2].copper_layer is CopperLayer.INTERNAL_1
    assert layers[2].thickness_nm == nm_from_mm("0.0175")


def test_stackup_is_digest_bound_and_absent_by_default() -> None:
    declared = prototype_physicalize(compile_design_source(source(SIX_LAYER)),
                                     PrototypePhysicalOptions(copper_layers=6))
    plain = prototype_physicalize(compile_design_source(source("")),
                                  PrototypePhysicalOptions(copper_layers=6))
    assert plain.stackup.physical_layers == ()
    assert plain.stackup.thickness_nm == nm_from_mm("1.6")
    assert physical_board_digest(declared) != physical_board_digest(plain)


def test_selected_layer_count_must_match_the_declared_copper_layers() -> None:
    design = compile_design_source(source(SIX_LAYER), "stack.copper")
    with pytest.raises(ValueError, match=r"stack.copper:\d+:\d+: stackup declares 6 copper layers.*"
                                         r"selected stack has 4.*--layers 6"):
        prototype_physicalize(design, PrototypePhysicalOptions(copper_layers=4))
    with pytest.raises(ValueError, match="declares 6 copper layers"):
        prototype_physicalize(design)


@pytest.mark.parametrize("body, code, message", [
    ("stackup { }", "MEC006", "at least F.Cu and B.Cu"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } copper B.Cu { thickness = 0.035mm; } }",
     "MEC006", "alternate copper and dielectric"),
    ("stackup { dielectric D { thickness = 1mm; er = 4; } copper F.Cu { thickness = 0.035mm; } }",
     "MEC006", "alternate copper and dielectric"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; } }",
     "MEC006", "end with a copper layer"),
    ("stackup { copper Top.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "unknown copper layer 'Top.Cu'"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; }"
     " copper In2.Cu { thickness = 0.035mm; } dielectric E { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "F.Cu, In1.Cu, In2.Cu ... B.Cu in order"),
    ("stackup { copper B.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; }"
     " copper F.Cu { thickness = 0.035mm; } }", "MEC006", "in order from top to bottom"),
    ("stackup { copper F.Cu { thickness = -0.035mm; } dielectric D { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "thickness must be positive"),
    ("stackup { copper F.Cu { thickness = 0.035; } dielectric D { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "typed length"),
    ("stackup { copper F.Cu { thickness = 0.035V; } dielectric D { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "must be a length"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 0; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "er must be positive"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 0.5; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "er must be at least 1"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4mm; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "unitless number"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "requires 'er'"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; loss_tangent = -1; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "loss_tangent must be positive"),
    ("stackup { copper F.Cu { thickness = 0.035mm; er = 4; } dielectric D { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "unknown copper stackup property 'er'"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; dk = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "unknown dielectric stackup property 'dk'"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; type = laminate; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "type must be core or prepreg"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric D { thickness = 1mm; er = 4; material = \"\"; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "material must be a nonempty name"),
    ("stackup { copper F.Cu { thickness = 0.035mm; } dielectric F.Cu { thickness = 1mm; er = 4; }"
     " copper B.Cu { thickness = 0.035mm; } }", "MEC006", "duplicate stackup layer 'F.Cu'"),
    ("stackup { copper F.Cu { thickness = 0.035mm; thickness = 0.035mm; } }", "PAR004", "duplicate property"),
    ("stackup { metal F.Cu { thickness = 0.035mm; } }", "PAR014", "unknown stackup layer kind 'metal'"),
    (FOUR_LAYER + FOUR_LAYER, "MEC006", "only one stackup"),
])
def test_invalid_stackups_fail_with_source_locations(body: str, code: str, message: str) -> None:
    with pytest.raises(CopperScriptError, match=message) as error:
        compile_source(source(body), "invalid.copper")
    assert error.value.code == code
    assert error.value.location.filename == "invalid.copper"
    assert error.value.location.line > 1


def test_stackup_is_board_owned_not_a_profile_feature() -> None:
    with pytest.raises(CopperScriptError, match="unknown mechanical declaration 'stackup'"):
        compile_source("board_profile P { stackup { } }")


def test_kicad_export_writes_the_declared_stackup() -> None:
    board = prototype_physicalize(compile_design_source(source(SIX_LAYER)),
                                  PrototypePhysicalOptions(copper_layers=6))
    text = KiCadPcbBackend().generate(board).artifacts[0].content
    assert "(thickness 1.54)" in text
    assert '(layer "F.Cu" (type "copper") (thickness 0.035))' in text
    assert ('(layer "dielectric 1" (type "prepreg") (thickness 0.1) (material "3313") '
            '(epsilon_r 4.1) (loss_tangent 0.02))') in text
    assert '(layer "dielectric 2" (type "core") (thickness 0.55) (epsilon_r 4.6))' in text
    assert '(layer "In4.Cu" (type "copper") (thickness 0.0175))' in text
    assert text.index("(stackup") < text.index("(pad_to_mask_clearance 0)")


def test_kicad_export_defaults_undeclared_dielectric_construction_like_kicad() -> None:
    board = prototype_physicalize(compile_design_source(source(FOUR_LAYER)),
                                  PrototypePhysicalOptions(copper_layers=4))
    text = KiCadPcbBackend().generate(board).artifacts[0].content
    assert '(layer "dielectric 1" (type "prepreg")' in text
    assert '(layer "dielectric 2" (type "core")' in text
    assert '(layer "dielectric 3" (type "prepreg")' in text
    plain = prototype_physicalize(compile_design_source(source("")))
    assert "(stackup" not in KiCadPcbBackend().generate(plain).artifacts[0].content


def test_source_editor_indexes_and_patches_around_a_stackup() -> None:
    snapshot = SourceSnapshot(source(FOUR_LAYER).encode())
    index = declaration_index(snapshot)
    assert index.children
    patch = mechanical_patch(snapshot, kind="hole", name="H1",
                             parameters={"position": "(5mm, 5mm)", "diameter": "3mm"})
    candidate = compile_design_source(SourceSnapshot(patch.apply(snapshot)).text)
    assert candidate.mechanical.holes[0].id == "H1"
    assert len(candidate.mechanical.stackup.layers) == 7

from dataclasses import replace
from decimal import Decimal

import pytest

from pcbir import (
    BoardOutline, CopperLayer, KiCadPcbBackend, PhysicalBoard, PhysicalNet,
    Point, Stackup, StackupLayer, StackupLayerKind, Via, ViaKind,
    ViaTechnology, nm_from_mm,
)


def _four_layer_stackup() -> Stackup:
    copper = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    layers = (
        StackupLayer("L1", StackupLayerKind.COPPER, nm_from_mm("0.035"), copper[0]),
        StackupLayer("D1", StackupLayerKind.DIELECTRIC, nm_from_mm("0.1"), material="FR-4", relative_permittivity=Decimal("4.2"), loss_tangent=Decimal("0.02")),
        StackupLayer("L2", StackupLayerKind.COPPER, nm_from_mm("0.035"), copper[1]),
        StackupLayer("D2", StackupLayerKind.DIELECTRIC, nm_from_mm("0.66"), material="FR-4", relative_permittivity=Decimal("4.2"), loss_tangent=Decimal("0.02")),
        StackupLayer("L3", StackupLayerKind.COPPER, nm_from_mm("0.035"), copper[2]),
        StackupLayer("D3", StackupLayerKind.DIELECTRIC, nm_from_mm("0.7"), material="FR-4", relative_permittivity=Decimal("4.2"), loss_tangent=Decimal("0.02")),
        StackupLayer("L4", StackupLayerKind.COPPER, nm_from_mm("0.035"), copper[3]),
    )
    return Stackup(copper, nm_from_mm("1.6"), layers, (
        ViaTechnology("TH", ViaKind.THROUGH, copper[0], copper[3], nm_from_mm("0.3"), nm_from_mm("0.15"), Decimal("6")),
        ViaTechnology("UV1", ViaKind.MICROVIA, copper[0], copper[1], nm_from_mm("0.1"), nm_from_mm("0.075"), Decimal("2")),
    ))


def test_stackup_via_catalog_validates_span_ring_and_aspect_ratio() -> None:
    stackup = _four_layer_stackup()
    board = PhysicalBoard("FourLayer", BoardOutline.rectangle(20, 10), {}, (), (PhysicalNet("N", ()),),
                          stackup=stackup,
                          vias=(Via("N", Point.mm(5, 5), nm_from_mm("0.7"), nm_from_mm("0.4"), CopperLayer.FRONT, CopperLayer.BACK, "TH"),))
    assert board.vias[0].technology == "TH"
    with pytest.raises(ValueError, match="annular ring"):
        replace(board, vias=(replace(board.vias[0], size_nm=nm_from_mm("0.5")),))


def test_kicad_backend_maps_multilayer_stack_and_microvia() -> None:
    stackup = _four_layer_stackup()
    board = PhysicalBoard("FourLayer", BoardOutline.rectangle(20, 10), {}, (), (PhysicalNet("N", ()),),
                          stackup=stackup,
                          vias=(Via("N", Point.mm(5, 5), nm_from_mm("0.3"), nm_from_mm("0.1"), CopperLayer.FRONT, CopperLayer.INTERNAL_1, "UV1"),))
    text = KiCadPcbBackend().generate(board).artifacts[0].content
    assert '(1 "In1.Cu" signal)' in text
    assert '(2 "In2.Cu" signal)' in text
    assert "(via micro" in text

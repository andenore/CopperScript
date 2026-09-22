from decimal import Decimal

from pcbir import (
    AnalysisStatus, BoardOutline, FabricationAssemblyProfile, FootprintPad,
    FootprintLayer, FootprintLine, PadKind,
    PhysicalBoard, PhysicalFootprint, Placement, Point, ProcessCapability,
    ProcessGateStatus, Size, dc_trace_resistance, nm_from_mm, run_process_drc,
)


def _cap(value: str) -> ProcessCapability:
    return ProcessCapability(nm_from_mm(value), "fabricator capability sheet", "2026-01")


def test_process_gates_are_separate_and_profile_provenance_is_required() -> None:
    footprint = PhysicalFootprint("qfn", (FootprintPad("1", Point(0, 0), Size.mm("0.2", "0.2")),),
                                  Size.mm(2, 2), courtyard=(Point.mm(-1, -1), Point.mm(1, -1), Point.mm(1, 1), Point.mm(-1, 1)),
                                  height_nm=nm_from_mm("0.8"))
    board = PhysicalBoard("Process", BoardOutline.rectangle(10, 10), {"qfn": footprint},
                          (Placement("U1", "qfn", Point.mm(5, 5)),), ())
    profile = FabricationAssemblyProfile("fab", _cap("0.2"), _cap("0.1"), _cap("0.12"),
                                         ProcessCapability(600_000, "IPC-7525", "B"), _cap("2"))
    report = run_process_drc(board, profile)
    assert report.fabrication is ProcessGateStatus.PASS
    assert report.stencil is ProcessGateStatus.FAIL
    assert report.assembly is ProcessGateStatus.PASS


def test_engineering_results_state_scope_grade_and_validity() -> None:
    result = dc_trace_resistance(nm_from_mm(100), nm_from_mm("0.25"), nm_from_mm("0.035"),
                                 maximum_ohms=Decimal("0.3"))
    assert result.status is AnalysisStatus.PASS
    assert result.claim_scope and result.validity and result.value is not None


def test_artwork_and_slot_checks_use_profile_geometry() -> None:
    footprint = PhysicalFootprint(
        "connector",
        (FootprintPad("1", Point(0, 0), Size.mm("1", "1"), kind=PadKind.THROUGH_HOLE,
                      drill=Size.mm("0.15", "0.5")),), Size.mm(2, 2),
        graphics=(FootprintLine(Point.mm("-0.5", 0), Point.mm("0.5", 0),
                                nm_from_mm("0.15"), FootprintLayer.SILKSCREEN),),
        courtyard=(Point.mm(-1, -1), Point.mm(1, -1), Point.mm(1, 1), Point.mm(-1, 1)),
    )
    board = PhysicalBoard("Artwork", BoardOutline.rectangle(10, 10), {footprint.name: footprint},
                          (Placement("J1", footprint.name, Point.mm(5, 5)),), ())
    profile = FabricationAssemblyProfile(
        "fab", _cap("0.1"), _cap("0.1"), _cap("0.12"),
        ProcessCapability(500_000, "IPC-7525", "B"),
        minimum_silkscreen_clearance_nm=_cap("0.2"),
        minimum_slot_width_nm=_cap("0.2"),
    )
    report = run_process_drc(board, profile)
    codes = {item.code for item in report.findings}
    assert {"FAB-SLOT-MIN", "FAB-SILK-MASK"} <= codes

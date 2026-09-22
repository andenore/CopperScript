from decimal import Decimal

from pcbir import (
    AnalysisStatus, BoardOutline, CopperLayer, FabricationAssemblyProfile, FootprintPad,
    FootprintLayer, FootprintLine, PadKind,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, ProcessCapability,
    ProcessGateStatus, Size, dc_net_voltage_drop, dc_trace_resistance,
    external_solver_result, microstrip_impedance, nm_from_mm, propagation_delay,
    run_process_drc, thermal_screen, TrackSegment,
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


def test_si_dc_thermal_and_external_results_are_evidence_graded() -> None:
    impedance = microstrip_impedance(nm_from_mm("0.3"), nm_from_mm("0.035"),
                                     nm_from_mm("0.18"), Decimal("4.1"),
                                     target_ohms=Decimal("50"), tolerance_ohms=Decimal("30"))
    assert impedance.value is not None and impedance.evidence_grade.value == "screening"
    delay = propagation_delay(nm_from_mm(100), Decimal("3.2"))
    assert delay.value is not None and delay.unit == "s"
    board = PhysicalBoard("DC", BoardOutline.rectangle(20, 10), {}, (),
                          (PhysicalNet("PWR", ()),),
                          tracks=(TrackSegment(
                              "PWR", Point.mm(2, 5), Point.mm(18, 5),
                              nm_from_mm("0.5"), CopperLayer.FRONT),))
    drop = dc_net_voltage_drop(board, "PWR", Decimal("1"), nm_from_mm("0.035"),
                               nm_from_mm("0.02"), maximum_volts=Decimal("0.1"))
    assert drop.value is not None
    thermal = thermal_screen(Decimal("1.5"), Decimal("20"), Decimal("25"),
                             Decimal("85"), model_source="component datasheet")
    assert thermal.status is AnalysisStatus.PASS
    external = external_solver_result("impedance", Decimal("49.8"), "ohm", b"report",
                                      tool="openEMS", version="0.0.35", passed=True,
                                      claim_scope="routed USB pair")
    assert external.evidence_digest is not None and len(external.evidence_digest) == 64

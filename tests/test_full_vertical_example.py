from pathlib import Path

from pcbir import check, compile_file
from pcbir.elaborate import elaborate
from pcbir.power import analyze_power_states


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples" / "full_vertical_board.copper"


def test_full_vertical_example_compiles_and_passes_erc() -> None:
    board = compile_file(EXAMPLE)

    assert board.name == "FullVerticalTracker"
    assert check(board) == []
    assert analyze_power_states(board) == []
    assert {instance.ref for instance in board.module_instances} == {"PWR"}


def test_full_vertical_example_exercises_required_subsystems() -> None:
    board = compile_file(EXAMPLE)
    flat = elaborate(board)
    parts = {component.part for component in flat.components}
    configurations = {
        (selection.component, selection.peripheral)
        for selection in flat.peripheral_selections
    }

    assert {
        "vertical.STM32G0C1RET6",
        "vertical.TCAN334G",
        "vertical.EG800G_EU",
        "vertical.NRF52832_QFAA",
        "vertical.LIS2DW12",
        "vertical.MAX_M10S_00B",
    } <= parts
    assert {
        ("U_MCU", "USART1"),
        ("U_MCU", "USART2"),
        ("U_MCU", "USART3"),
        ("U_MCU", "USART6"),
        ("U_MCU", "I2C2"),
        ("U_MCU", "FDCAN1"),
        ("U_MCU", "USB_FS"),
        ("U_NRF", "UARTE0"),
    } <= configurations
    assert {"NORMAL", "LOGIC_ONLY"} == {state.name for state in board.power_states}

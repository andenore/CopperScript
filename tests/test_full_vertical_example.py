from pathlib import Path

from pcbir import ConnectionPolicy, check, compile_file
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
    assert {
        dependency.import_path for dependency in board.dependencies
    } == {"github.com/andenore/CopperLib/packages/full_vertical"}


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


def test_nordic_qfaa_package_has_all_footprint_pad_numbers() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.NRF52832_QFAA"]
    assert {pin.number for pin in part.pins.values()} == {
        str(number) for number in range(1, 50)
    }
    assert part.pins["NC_44"].connection_policy is ConnectionPolicy.DO_NOT_CONNECT
    assert all(pin.bonds for pin in part.pins.values() if pin.name != "NC_44")

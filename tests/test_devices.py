from pathlib import Path

from pcbir import check, compile_file, compile_source
from pcbir.serializer import board_to_dict


ROOT = Path(__file__).parents[1]


def test_explicit_mcu_mux_selection_is_typed_and_serialized() -> None:
    board = compile_file(ROOT / "examples" / "valid_board.copper")

    assert check(board) == []
    part = board.library["stm32.STM32G0B1CBT6"]
    assert part.device == "stm32.STM32G0B1"
    device = board.devices[part.device]
    assert device.peripherals["I2C1"].signals["SDA"].required

    selection = board.peripheral_selections[0]
    assert selection.component == "U2"
    assert selection.peripheral == "I2C1"
    assert selection.signals["SDA"].pin == "PB7"
    assert selection.signals["SDA"].selector == "AF6"

    serialized = board_to_dict(board)
    assert serialized["peripheral_selections"][0]["signals"]["SCL"]["selector"] == "AF6"


def test_mcu_erc_reports_mux_and_resource_conflicts() -> None:
    source = """board InvalidMuxBoard {
    import stm32 "github.com/copperscript/examples/stm32";

    component U1: stm32.STM32G0B1CBT6;

    configure U1.I2C1 as SENSOR_BUS {
        SDA = PB7;
        SCL = PB6;
    }
    configure U1.I2C1 as BAD_BUS {
        SDA = PA2;
    }
    configure U1.USART2 as UART_DEFAULT {
        TX = PA2;
        RX = PA3;
    }
    configure U1.USART2 as UART_REMAP {
        TX = PB6;
        RX = PB7;
    }

    net VDD { U1.VDD; }
    net GND { U1.VSS; }
    supply VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}
"""
    board = compile_source(source, str(ROOT / "tests" / "invalid_mux_board.copper"))
    codes = {diagnostic.code for diagnostic in check(board)}

    assert {
        "MISSING_PERIPHERAL_SIGNAL",
        "INVALID_MUX_OPTION",
        "PERIPHERAL_CONFLICT",
        "PIN_MUX_CONFLICT",
        "MUX_RESOURCE_CONFLICT",
    } <= codes

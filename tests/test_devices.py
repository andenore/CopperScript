from pathlib import Path

from pcbir import Severity, analyze_power_states, check, compile_file, compile_source
from pcbir.serializer import board_to_dict


ROOT = Path(__file__).parents[1]


def test_explicit_mcu_mux_selection_is_typed_and_serialized() -> None:
    board = compile_file(ROOT / "examples" / "valid_board.copper")

    assert check(board) == []
    part = board.library["stm32.STM32G0B1CBT6"]
    assert part.device == "stm32.STM32G0B1"
    device = board.devices[part.device]
    assert tuple(bond.pad for bond in part.pins["PB6"].bonds) == ("PB6",)
    assert device.pads["PB6"].power_domain == "VDDIO1"
    assert device.source is not None
    assert device.source.revision == "illustrative-v0.1"
    assert part.source is None
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


def test_firmware_managed_mux_sharing_waives_only_sharing_conflicts() -> None:
    source = """board FirmwareManagedMux {
    import stm32 "github.com/copperscript/examples/stm32";
    component U1: stm32.STM32G0B1CBT6;

    configure U1.I2C1 as I2C_MODE {
        usage = firmware_managed;
        SDA = PB7;
        SCL = PB6;
    }
    configure U1.USART2 as UART_DEFAULT {
        usage = firmware_managed;
        TX = PA2;
        RX = PA3;
    }
    configure U1.USART2 as UART_REMAP {
        usage = firmware_managed;
        TX = PB6;
        RX = PB7;
    }

    net VDD { U1.VDD; }
    net GND { U1.VSS; }
    supply VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}
"""
    board = compile_source(source, str(ROOT / "tests" / "managed_mux.copper"))
    codes = {diagnostic.code for diagnostic in check(board)}

    assert "PIN_MUX_CONFLICT" not in codes
    assert "PERIPHERAL_CONFLICT" not in codes
    assert "MUX_RESOURCE_CONFLICT" not in codes
    assert "INVALID_MUX_OPTION" not in codes


def test_firmware_managed_selection_does_not_override_an_exclusive_selection() -> None:
    source = """board MixedMuxUsage {
    import stm32 "github.com/copperscript/examples/stm32";
    component U1: stm32.STM32G0B1CBT6;

    configure U1.I2C1 as EXCLUSIVE_MODE {
        SDA = PB7;
        SCL = PB6;
    }
    configure U1.USART2 as EXCLUSIVE_UART {
        TX = PA2;
        RX = PA3;
    }
    configure U1.USART2 as MANAGED_UART {
        usage = firmware_managed;
        TX = PB6;
        RX = PB7;
    }

    net VDD { U1.VDD; }
    net GND { U1.VSS; }
    supply VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}
"""
    board = compile_source(source, str(ROOT / "tests" / "mixed_mux_usage.copper"))
    codes = {diagnostic.code for diagnostic in check(board)}

    assert "PIN_MUX_CONFLICT" in codes
    assert "PERIPHERAL_CONFLICT" in codes
    assert "MUX_RESOURCE_CONFLICT" in codes


def test_power_state_analysis_warns_about_driving_an_unpowered_domain() -> None:
    source = """board SequencedBoard {
    use library "tiny";
    import stm32 "github.com/copperscript/examples/stm32";

    component PS1: VOLTAGE_SOURCE;
    component U1: stm32.STM32G0B1CBT6;

    net SIGNAL { PS1.OUT; U1.PB6; }
    net MCU_VDD { U1.VDD; }
    net GND { PS1.GND; U1.VSS; }

    supply SIGNAL { voltage = 3.3V; source = PS1.OUT; }
    supply MCU_VDD { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }

    power_state UNSAFE {
        SIGNAL = on;
        MCU_VDD = off;
        GND = on;
    }
}
"""
    board = compile_source(source, str(ROOT / "tests" / "power_sequence.copper"))
    diagnostics = analyze_power_states(board)

    warning = next(item for item in diagnostics if item.code == "POSSIBLE_BACKPOWER")
    assert warning.severity is Severity.WARNING
    assert warning.subject == "UNSAFE:U1.PB6"

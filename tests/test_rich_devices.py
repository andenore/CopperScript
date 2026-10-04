from dataclasses import replace
from pathlib import Path

from pcbir import Endpoint, KiCadSchematicBackend, check, compile_file, compile_source
from pcbir.serializer import board_to_dict


ROOT = Path(__file__).parents[1]
SHOWCASE = ROOT / "examples/device_model_showcase/board.copper"


def test_cross_vendor_showcase_compiles_and_serializes_rich_semantics() -> None:
    board = compile_file(SHOWCASE)

    assert check(board) == []
    assert board.library["parts.NRF52840_QIAA"].category == "semiconductor.mcu"
    assert board.library["parts.AD4134BCPZ"].category == "converter.adc"
    assert board.library["parts.OPA2197ID"].category == "amplifier.opamp"
    assert board.library["parts.ICE40UP5K_SG48"].category == "programmable_logic.fpga"
    assert board.library["parts.ISO6721D"].category == "interface.digital_isolator"

    nrf = board.devices["parts.NRF52840"]
    assert nrf.route_rules[0].pad_set == "GPIO_PSEL"
    assert board.peripheral_selections[0].signals["TX"].selector == "0"

    opamp = board.devices["parts.OPA2197_DIE"]
    assert set(opamp.units) == {"A", "B", "POWER"}
    assert opamp.units["POWER"].shared

    adc = board.devices["parts.AD4134_DIE"]
    assert adc.signal_groups["AIN0"].members == {
        "positive": "AIN0P",
        "negative": "AIN0N",
    }

    serialized = board_to_dict(board)
    serialized_devices = {item["name"]: item for item in serialized["devices"]}
    assert serialized_devices["parts.FX10_USB"]["mode_groups"][0]["choices"] == [
        "LVCMOS",
        "LVDS",
    ]


def test_semantic_unit_terminal_and_physical_pin_share_canonical_identity() -> None:
    board = compile_file(SHOWCASE)
    extra = replace(
        board.nets[-1],
        name="ALIAS_CONFLICT",
        endpoints=(Endpoint("OP", "OUTA"),),
    )
    diagnostics = check(replace(board, nets=(*board.nets, extra)))

    assert "PIN_ON_MULTIPLE_NETS" in {item.code for item in diagnostics}


def test_package_policies_and_differential_groups_are_checked() -> None:
    board = compile_file(SHOWCASE)
    nets = []
    for net in board.nets:
        endpoints = tuple(
            endpoint
            for endpoint in net.endpoints
            if endpoint not in {Endpoint("ADC", "EPAD"), Endpoint("FX", "P0D7N")}
        )
        nets.append(replace(net, endpoints=endpoints))
    nets.append(replace(board.nets[0], name="BAD_DNC", endpoints=(Endpoint("ADC", "DNC1"),)))

    codes = {item.code for item in check(replace(board, nets=tuple(nets)))}
    assert {"DO_NOT_CONNECT", "REQUIRED_PIN_UNCONNECTED", "INCOMPLETE_DIFFERENTIAL_PAIR"} <= codes


def test_invalid_mode_and_parametric_route_are_rejected() -> None:
    board = compile_file(SHOWCASE)
    components = tuple(
        replace(component, modes={"PORT0": "NOT_A_MODE"})
        if component.ref == "FX"
        else component
        for component in board.components
    )
    assert "UNKNOWN_MODE_CHOICE" in {
        item.code for item in check(replace(board, components=components))
    }

    invalid_route = """board InvalidRoute {
        import parts "github.com/copperscript/examples/compatibility";
        component U1: parts.NRF52840_QIAA;
        configure U1.UARTE0 as BAD { TX = VDD; RX = P0_01; }
        net VDD { U1.VDD; }
        net GND { U1.VSS; }
        supply VDD { voltage = 3.3V; external = true; }
        supply GND { voltage = 0V; external = true; }
    }
    """
    compiled = compile_source(invalid_route, str(ROOT / "tests" / "invalid_route.copper"))
    assert "INVALID_MUX_OPTION" in {item.code for item in check(compiled)}


def test_power_domain_operating_range_is_checked() -> None:
    board = compile_file(SHOWCASE)
    supplies = tuple(
        replace(supply, voltage=board.supplies[0].voltage)
        if supply.name == "V5"
        else supply
        for supply in board.supplies
    )

    assert "POWER_DOMAIN_VOLTAGE_LOW" in {
        item.code for item in check(replace(board, supplies=supplies))
    }


def test_schematic_backend_resolves_semantic_unit_terminals() -> None:
    board = compile_file(SHOWCASE)
    artifact = KiCadSchematicBackend().generate(board).artifacts[0]

    assert "OP_A_OUT" in artifact.content
    assert "ISO_OUT" in artifact.content

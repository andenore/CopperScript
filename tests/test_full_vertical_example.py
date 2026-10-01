from pathlib import Path

from pcbir import ConnectionPolicy, PrototypePhysicalOptions, check, compile_file, prototype_physicalize
from pcbir.elaborate import elaborate
from pcbir.power import analyze_power_states
from pcbir.physical import CopperLayer, RouteKind


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples" / "full_vertical_board.copper"


def test_full_vertical_explicit_critical_profiles_lower_without_claiming_qualification() -> None:
    board = compile_file(EXAMPLE)
    physical = prototype_physicalize(board, PrototypePhysicalOptions(copper_layers=6))
    profiles = {rule.net: rule for rule in physical.net_routing_rules}
    assert set(profiles) == {
        "USB_DP_MCU", "USB_DM_MCU", "USB_DP_MODEM", "USB_DM_MODEM",
        "CELL_RF", "GNSS_RF", "NRF_RF_RAW", "NRF_RF_ANT",
    }
    for suffix in ("MCU", "MODEM"):
        positive, negative = profiles[f"USB_DP_{suffix}"], profiles[f"USB_DM_{suffix}"]
        assert positive.kind is negative.kind is RouteKind.DIFFERENTIAL
        assert positive.differential_partner == negative.net
        assert negative.differential_partner == positive.net
        assert positive.width_nm == negative.width_nm
        assert positive.pair_gap_nm == negative.pair_gap_nm
        assert positive.target_impedance_ohms == negative.target_impedance_ohms == 90
    for name in ("CELL_RF", "GNSS_RF"):
        assert profiles[name].kind is RouteKind.RF_FEED
        assert profiles[name].target_impedance_ohms == 50
    assert profiles["NRF_RF_RAW"].target_impedance_ohms is None
    assert profiles["NRF_RF_RAW"].topology == "tree"
    assert profiles["NRF_RF_ANT"].kind is RouteKind.RF_FEED
    assert profiles["NRF_RF_ANT"].topology == "point_to_point"
    assert all(rule.allowed_layers == (CopperLayer.FRONT,) and rule.max_vias == 0
               for rule in profiles.values())
    assert all(rule.impedance_evidence_digest is None and rule.max_length_nm is None
               and rule.max_skew_nm is None for rule in profiles.values())


def test_full_vertical_example_compiles_and_passes_erc() -> None:
    board = compile_file(EXAMPLE)

    assert board.name == "FullVerticalTracker"
    assert check(board) == []
    assert analyze_power_states(board) == []
    assert {instance.ref for instance in board.module_instances} == {"PWR"}
    assert {
        dependency.import_path for dependency in board.dependencies
    } == {"github.com/andenore/CopperLib/packages/full_vertical"}


def test_full_vertical_declares_unfilled_inner_ground_plane() -> None:
    board = compile_file(EXAMPLE)
    physical = prototype_physicalize(board, PrototypePhysicalOptions(copper_layers=4))
    assert len(physical.zones) == 1
    assert physical.zones[0].net == "GND"
    assert tuple(layer.value for layer in physical.zones[0].layers) == ("In1.Cu",)
    assert physical.zone_fills == ()


def test_swd_headers_use_keyed_smd_cortex_pinout() -> None:
    board = compile_file(EXAMPLE)
    header = board.library["vertical.SWD_HEADER"]
    assert header.footprints == ("Connector_Debug:FTSH-105-01-L-DV-007-K",)
    assert {pin.number for pin in header.pins.values()} == {
        "1", "2", "3", "4", "5", "6", "8", "9", "10"
    }
    assert header.pins["GND_DETECT"].number == "9"
    ground = next(net for net in board.nets if net.name == "GND")
    assert {(endpoint.component, endpoint.pin) for endpoint in ground.endpoints} >= {
        ("J_SWD_MCU", "GND_DETECT"), ("J_SWD_NRF", "GND_DETECT")
    }


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


def test_bluetooth_antenna_keeps_nc_anchor_isolated() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.JOHANSON_2450AT18A0100001E"]
    assert part.manufacturer == "Johanson Technology"
    assert part.pins["NC"].number == "2"
    assert part.pins["NC"].connection_policy is ConnectionPolicy.DO_NOT_CONNECT
    assert not any(endpoint.component == "ANT_BT" and endpoint.pin == "NC"
                   for net in board.nets for endpoint in net.endpoints)


def test_nordic_matching_shunt_is_on_chip_side_of_series_inductor() -> None:
    board = compile_file(EXAMPLE)
    nets = {net.name: {(ep.component, ep.pin) for ep in net.endpoints} for net in board.nets}
    assert nets["NRF_RF_RAW"] == {("U_NRF", "ANT"), ("C_BT_MATCH", "1"), ("L_BT_MATCH", "1")}
    assert nets["NRF_RF_ANT"] == {("L_BT_MATCH", "2"), ("ANT_BT", "FEED")}
    assert ("C_BT_MATCH", "2") in nets["GND"]


def test_stm32g0c1re_standard_lqfp64_bonds_are_complete() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.STM32G0C1RET6"]
    assert {pin.number for pin in part.pins.values()} == {
        str(number) for number in range(1, 65)
    }
    assert all(pin.bonds for pin in part.pins.values())
    assert {pin.name for pin in part.pins.values() if pin.number in {"6", "7", "8", "9"}} == {
        "VBAT", "VREF_PLUS", "VDD", "VSS"
    }


def test_gct_nano_sim_socket_uses_c7_for_io_and_connects_shell() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.SIM8060_6_0_14_00_A"]
    assert part.footprints == ("Connector_Card:nanoSIM_GCT_SIM8060-6-0-14-00",)
    assert part.pins["VPP"].number == "6"
    assert part.pins["IO"].number == "7"
    assert part.pins["SHIELD"].number == "SH"
    ground = next(net for net in board.nets if net.name == "GND")
    assert any(
        endpoint.component == "J_SIM" and endpoint.pin == "SHIELD"
        for endpoint in ground.endpoints
    )


def test_usb_choke_uses_coilcraft_winding_pairs_and_land_pattern() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.COILCRAFT_0603USB_601MLC"]
    assert part.footprints == (
        "Inductor_SMD:L_CommonModeChoke_Coilcraft_0603USB",
    )
    assert {
        name: pin.number for name, pin in part.pins.items()
    } == {"DP_IN": "1", "DM_IN": "2", "DM_OUT": "3", "DP_OUT": "4"}


def test_usb_c_power_entry_detects_3a_source_and_defaults_modem_off() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["vertical.GCT_USB4135_GF_A"]
    assert part.footprints == (
        "Connector_USB:USB_C_Receptacle_GCT_USB4135-GF-A_6P_TopMnt_Horizontal",
    )
    assert {name: pin.number for name, pin in part.pins.items()} == {
        "CC1": "A5", "VBUS_A": "A9", "GND_A": "A12",
        "CC2": "B5", "VBUS_B": "B9", "GND_B": "B12", "SHIELD": "SH",
    }
    nets = {
        net.name: {(endpoint.component, endpoint.pin) for endpoint in net.endpoints}
        for net in board.nets
    }
    flat_nets = {
        net.name: {(endpoint.component, endpoint.pin) for endpoint in net.endpoints}
        for net in elaborate(board).nets
    }
    cc = board.library["vertical.TUSB320LAI"]
    assert cc.footprints == ("Package_DFN_QFN:Texas_X2QFN-12_1.6x1.6mm_P0.4mm",)
    assert {pin.number for pin in cc.pins.values()} == {str(number) for number in range(1, 13)}
    assert cc.pins["ADDR"].connection_policy is ConnectionPolicy.DO_NOT_CONNECT
    assert {("J_POWER", "VBUS_A"), ("J_POWER", "VBUS_B")} <= nets["V5"]
    assert nets["USB_C_CC1"] == {("J_POWER", "CC1"), ("U_CC", "CC1")}
    assert nets["USB_C_CC2"] == {("J_POWER", "CC2"), ("U_CC", "CC2")}
    assert {("U_CC", "PORT"), ("U_CC", "EN_N")} <= nets["GND"]
    assert {("U_CC", "VDD"), ("R_CC_OUT1", "1"), ("R_CC_OUT2", "1")} <= nets["V3V3"]
    assert nets["USB_C_CURRENT_1"] == {
        ("U_CC", "OUT1"), ("R_CC_OUT1", "2"), ("U_MCU", "PC8")
    }
    assert nets["USB_C_CURRENT_2"] == {
        ("U_CC", "OUT2"), ("R_CC_OUT2", "2"), ("U_MCU", "PC9")
    }
    assert {("U_MCU", "PC10"), ("PWR/U_MODEM", "EN"),
            ("PWR/R_MODEM_EN_PD", "1")} <= flat_nets["MODEM_EN"]
    assert ("PWR/R_MODEM_EN_PD", "2") in flat_nets["GND"]
    assert ("PWR/U_MODEM", "EN") not in flat_nets["V5"]
    assert next(supply for supply in board.supplies if supply.name == "V5").externally_driven


def test_modem_rail_uses_real_buck_power_stage() -> None:
    board = compile_file(EXAMPLE)
    flat = elaborate(board)
    nets = {
        net.name: {(endpoint.component, endpoint.pin) for endpoint in net.endpoints}
        for net in flat.nets
    }
    buck = board.library["vertical.TPS62130ARGTR"]
    inductor = board.library["vertical.XAL4020_222MEC"]
    assert buck.footprints == (
        "Package_DFN_QFN:VQFN-16-1EP_3x3mm_P0.5mm_EP1.68x1.68mm",
    )
    assert {pin.number for pin in buck.pins.values()} == {
        str(number) for number in range(1, 18)
    }
    assert inductor.footprints == ("Inductor_SMD:L_Coilcraft_XAL4020-XXX",)
    assert {("PWR/U_MODEM", "SW_1"), ("PWR/U_MODEM", "SW_2"),
            ("PWR/U_MODEM", "SW_3"), ("PWR/L_MODEM", "A")} <= nets["PWR/MODEM_SW"]
    assert {("PWR/L_MODEM", "B"), ("PWR/U_MODEM", "VOS"),
            ("PWR/R_MODEM_FB_TOP", "1")} <= nets["V3V8"]
    assert {("PWR/U_MODEM", "FB"), ("PWR/R_MODEM_FB_TOP", "2"),
            ("PWR/R_MODEM_FB_BOT", "1")} <= nets["PWR/MODEM_FB"]
    assert {("PWR/U_MODEM", "AGND"), ("PWR/U_MODEM", "PGND_1"),
            ("PWR/U_MODEM", "PGND_2"), ("PWR/U_MODEM", "EP"),
            ("PWR/R_MODEM_FB_BOT", "2")} <= nets["GND"]

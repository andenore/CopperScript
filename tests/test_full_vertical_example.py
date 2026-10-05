from dataclasses import replace
from decimal import Decimal
from pathlib import Path

import pytest

from pcbir import ConnectionPolicy, PrototypePhysicalOptions, check, compile_file, prototype_physicalize
from pcbir.elaborate import elaborate
from pcbir.power import analyze_power_states
from pcbir.physical import BoardSide, CopperLayer, Point, RouteKind, nm_from_mm


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/full_vertical/board.copper"

FIXED_FLOORPLAN = {
    "J_POWER": ("12", "74", 0),
    "J_CAN": ("25", "72", 0),
    "U_MODEM": ("20", "20", 90),
    "J_CELL": ("13.4", "6", 90),
    "J_SIM": ("10", "44", 270),
    "U_NRF": ("86", "12", 0),
    "ANT_BT": ("95.7", "12.508", 0),
    "U_GNSS": ("86", "67", 0),
}


def test_full_vertical_mechanical_intent_lowers_to_hard_physical_rules() -> None:
    physical = prototype_physicalize(compile_file(EXAMPLE), PrototypePhysicalOptions(copper_layers=6))
    fixed = {rule.reference: rule for rule in physical.placement_rules
             if rule.fixed_position is not None}
    assert fixed.keys() == FIXED_FLOORPLAN.keys()
    for ref, (x, y, rotation) in FIXED_FLOORPLAN.items():
        assert fixed[ref].fixed_position == Point.mm(x, y)
        assert fixed[ref].fixed_rotation_degrees == Decimal(rotation)
        assert fixed[ref].side is BoardSide.FRONT
    assert any(rule.targets[0].reference == "J_GNSS"
               and rule.targets[1].reference == "U_GNSS"
               and rule.targets[1].pad == "11"
               and rule.distance_nm == nm_from_mm(5)
               for rule in physical.relative_rules)


def test_fixed_floorplan_is_legal_on_installed_footprints_and_preserves_rf_macro() -> None:
    from pcbir import FootprintResolver, PhysicalBoard, resolved_physicalize
    from pcbir.clusters import cluster_placements
    from pcbir.placement import placement_solution_is_legal, transformed_footprint_polygon, transformed_pad_position
    from pcbir.placement_templates import apply_placement_templates

    physical = resolved_physicalize(compile_file(EXAMPLE, locked=True, offline=True),
        FootprintResolver(EXAMPLE.parent, (), locked=True, offline=True),
        PrototypePhysicalOptions(copper_layers=6, fabrication_profile="jlcpcb-six-layer"))
    physical = apply_placement_templates(physical, EXAMPLE.parent / "placement_templates.json")
    refs = set(FIXED_FLOORPLAN) | {"C_BT_MATCH", "L_BT_MATCH"}
    poses = {pose.reference: pose for pose in physical.placements if pose.reference in refs}
    for rule in physical.placement_rules:
        if rule.fixed_position is not None:
            poses[rule.reference] = replace(poses[rule.reference], position=rule.fixed_position,
                rotation_degrees=rule.fixed_rotation_degrees, side=rule.side)
    poses.update(cluster_placements(physical, physical.rigid_clusters[0], poses["U_NRF"]))
    # This bounded mechanical fixture checks the locked parts plus the RF macro;
    # complete-board placement is checked separately through plan-layout.
    fixture = PhysicalBoard("FixedFloorplan", physical.outline, physical.footprints,
        tuple(poses.values()), (), placement_rules=tuple(rule for rule in physical.placement_rules
            if rule.reference in refs), rigid_clusters=physical.rigid_clusters)
    assert placement_solution_is_legal(fixture, poses)
    modem_rf = transformed_pad_position(fixture, poses["U_MODEM"], "35")
    modem = transformed_footprint_polygon(fixture, poses["U_MODEM"])
    cellular = transformed_footprint_polygon(fixture, poses["J_CELL"])
    assert modem_rf.y_nm < poses["U_MODEM"].position.y_nm
    assert max(point.y_nm for point in cellular) < min(point.y_nm for point in modem)
    assert transformed_pad_position(fixture, poses["J_CELL"], "1").y_nm > poses["J_CELL"].position.y_nm
    antenna_feed = transformed_pad_position(fixture, poses["ANT_BT"], "1")
    assert poses["L_BT_MATCH"].position.x_nm < antenna_feed.x_nm < poses["ANT_BT"].position.x_nm
    antenna = transformed_footprint_polygon(fixture, poses["ANT_BT"])
    assert max(point.x_nm for point in antenna) == nm_from_mm(98)
    # Hard locks cannot be violated by later optimization/feedback.
    for ref in FIXED_FLOORPLAN:
        changed = {**poses, ref: replace(poses[ref], position=Point(
            poses[ref].position.x_nm + nm_from_mm(1), poses[ref].position.y_nm))}
        assert not placement_solution_is_legal(fixture, changed)


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
        for rule in (positive,negative):
            assert rule.allowed_layers == (CopperLayer.FRONT,CopperLayer.INTERNAL_2)
            assert rule.max_vias == 2
            assert rule.require_return_vias and rule.return_via_net == 'GND'
            assert rule.maximum_return_via_distance_nm == nm_from_mm(2)
    for name in ("CELL_RF", "GNSS_RF"):
        assert profiles[name].kind is RouteKind.RF_FEED
        assert profiles[name].target_impedance_ohms == 50
    assert profiles["NRF_RF_RAW"].target_impedance_ohms is None
    assert profiles["NRF_RF_RAW"].topology == "tree"
    assert profiles["NRF_RF_ANT"].kind is RouteKind.RF_FEED
    assert profiles["NRF_RF_ANT"].topology == "point_to_point"
    assert all(rule.allowed_layers == (CopperLayer.FRONT,) and rule.max_vias == 0
               for name,rule in profiles.items() if not name.startswith('USB_'))
    assert all(rule.impedance_evidence_digest is None and rule.max_length_nm is None
               and rule.max_skew_nm is None for rule in profiles.values())


def test_full_vertical_example_compiles_and_passes_erc() -> None:
    board = compile_file(EXAMPLE)

    assert board.name == "FullVerticalTracker"
    assert check(board) == []
    assert analyze_power_states(board) == []
    assert {instance.ref for instance in board.module_instances} == {"PWR"}
    dependencies = {dependency.import_path for dependency in board.dependencies}
    assert "github.com/andenore/CopperLib/packages/parts/nordic/nrf52832" in dependencies
    assert "github.com/andenore/CopperLib/packages/parts/johanson/2450at18a0100001e" in dependencies
    assert "github.com/copperscript/examples/vertical_support" in dependencies
    assert not any(path.endswith("/packages/full_vertical") for path in dependencies)


def test_full_vertical_declares_unfilled_inner_ground_plane() -> None:
    board = compile_file(EXAMPLE)
    physical = prototype_physicalize(board, PrototypePhysicalOptions(copper_layers=4))
    assert len(physical.zones) == 1
    assert physical.zones[0].net == "GND"
    assert tuple(layer.value for layer in physical.zones[0].layers) == ("In1.Cu",)
    assert physical.zone_fills == ()


def test_swd_headers_use_keyed_smd_cortex_pinout() -> None:
    board = compile_file(EXAMPLE)
    header = board.library["swd.SWD_HEADER"]
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
        "st_mcu.STM32G0C1RET6",
        "ti_can.TCAN334G",
        "quectel.EG800G_EU",
        "nordic.NRF52832_QFAA",
        "st_accel.LIS2DW12",
        "ublox.MAX_M10S_00B",
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
    part = board.library["nordic.NRF52832_QFAA"]
    assert {pin.number for pin in part.pins.values()} == {
        str(number) for number in range(1, 50)
    }
    assert part.pins["NC_44"].connection_policy is ConnectionPolicy.DO_NOT_CONNECT
    assert all(pin.bonds for pin in part.pins.values() if pin.name != "NC_44")


def test_bluetooth_antenna_keeps_nc_anchor_isolated() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["johanson.JOHANSON_2450AT18A0100001E"]
    assert part.manufacturer == "Johanson Technology"
    assert part.pins["NC"].number == "2"
    assert part.pins["NC"].connection_policy is ConnectionPolicy.DO_NOT_CONNECT
    assert not any(endpoint.component == "ANT_BT" and endpoint.pin == "NC"
                   for net in board.nets for endpoint in net.endpoints)


def test_nordic_matching_shunt_is_on_chip_side_of_series_inductor() -> None:
    board = compile_file(EXAMPLE)
    nets = {net.name: {(ep.component, ep.pin) for ep in net.endpoints} for net in board.nets}
    assert nets["NRF_RF_RAW"] == {("U_NRF", "ANT"), ("C_BT_MATCH", "2"), ("L_BT_MATCH", "1")}
    assert nets["NRF_RF_ANT"] == {("L_BT_MATCH", "2"), ("ANT_BT", "FEED")}
    assert ("C_BT_MATCH", "1") in nets["GND"]


def test_stm32g0c1re_standard_lqfp64_bonds_are_complete() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["st_mcu.STM32G0C1RET6"]
    assert {pin.number for pin in part.pins.values()} == {
        str(number) for number in range(1, 65)
    }
    assert all(pin.bonds for pin in part.pins.values())
    assert {pin.name for pin in part.pins.values() if pin.number in {"6", "7", "8", "9"}} == {
        "VBAT", "VREF_PLUS", "VDD", "VSS"
    }


def test_gct_nano_sim_socket_uses_c7_for_io_and_connects_shell() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["gct_sim.SIM8060_6_0_14_00_A"]
    assert part.footprints == ("github.com/andenore/CopperLib/packages/parts/gct/sim8060/footprints/Connector_Card.pretty/nanoSIM_GCT_SIM8060-6-0-14-00.kicad_mod",)
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
    part = board.library["coilcraft.COILCRAFT_0603USB_601MLC"]
    assert part.footprints == (
        "Inductor_SMD:L_CommonModeChoke_Coilcraft_0603USB",
    )
    assert {
        name: pin.number for name, pin in part.pins.items()
    } == {"DP_IN": "1", "DM_IN": "4", "DM_OUT": "3", "DP_OUT": "2"}
    # Each lane passes through one winding; the MCU pair shares the dotted side.
    windings = {frozenset({"1", "2"}), frozenset({"4", "3"})}
    assert {frozenset({part.pins[f"{lane}_IN"].number,
                      part.pins[f"{lane}_OUT"].number}) for lane in ("DP", "DM")} == windings
    physical = prototype_physicalize(board, PrototypePhysicalOptions(copper_layers=6))
    choke_pads = {
        net.name: {pad.pad for pad in net.pads if pad.component == "FL_USB"}
        for net in physical.nets if any(pad.component == "FL_USB" for pad in net.pads)
    }
    assert choke_pads == {
        "USB_DP_MCU": {"1"}, "USB_DM_MCU": {"4"},
        "USB_DP_MODEM": {"2"}, "USB_DM_MODEM": {"3"},
    }


def test_usb_c_power_entry_detects_3a_source_and_defaults_modem_off() -> None:
    board = compile_file(EXAMPLE)
    part = board.library["gct_usb.GCT_USB4135_GF_A"]
    assert part.footprints == (
        "github.com/andenore/CopperLib/packages/parts/gct/usb4135/footprints/Connector_USB.pretty/USB_C_Receptacle_GCT_USB4135-GF-A_6P_TopMnt_Horizontal.kicad_mod",
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
    cc = board.library["tusb.TUSB320LAI"]
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
    buck = board.library["vertical_support.ti.TPS62130ARGTR"]
    inductor = board.library["vertical_support.coilcraft.XAL4020_222MEC"]
    assert buck.footprints == (
        "Package_DFN_QFN:VQFN-16-1EP_3x3mm_P0.5mm_EP1.68x1.68mm",
    )
    assert {pin.number for pin in buck.pins.values()} == {
        str(number) for number in range(1, 18)
    }
    assert inductor.footprints == ("github.com/andenore/CopperLib/packages/parts/coilcraft/xal4020/footprints/Inductor_SMD.pretty/L_Coilcraft_XAL4020-XXX.kicad_mod",)
    assert {("PWR/U_MODEM", "SW_1"), ("PWR/U_MODEM", "SW_2"),
            ("PWR/U_MODEM", "SW_3"), ("PWR/L_MODEM", "A")} <= nets["PWR/MODEM_SW"]
    assert {("PWR/L_MODEM", "B"), ("PWR/U_MODEM", "VOS"),
            ("PWR/R_MODEM_FB_TOP", "1")} <= nets["V3V8"]
    assert {("PWR/U_MODEM", "FB"), ("PWR/R_MODEM_FB_TOP", "2"),
            ("PWR/R_MODEM_FB_BOT", "1")} <= nets["PWR/MODEM_FB"]
    assert {("PWR/U_MODEM", "AGND"), ("PWR/U_MODEM", "PGND_1"),
            ("PWR/U_MODEM", "PGND_2"), ("PWR/U_MODEM", "EP"),
            ("PWR/R_MODEM_FB_BOT", "2")} <= nets["GND"]

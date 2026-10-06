"""Mode-group defaults apply to bonds, pads, mux options and signal groups."""

from pathlib import Path

from pcbir import (
    Severity,
    analyze_power_states,
    check,
    compile_source,
    has_errors,
)
from pcbir.backends.kicad import _physical_pin_name
from pcbir.elaborate import elaborate
from pcbir.modes import active_bonded_pads, active_device_pads, effective_modes
from pcbir.physicalize import _physical_pin_number
from pcbir.pin_resolution import resolve_package_pin


# A camera-bridge style device whose lane pins bond to D-PHY pads by default
# and to C-PHY trio pads when the component selects ``PHY=CPHY``.
DEVICE = """device LANE_BRIDGE_DIE {
    vendor = "Example";
    mode_group PHY { choices = "DPHY,CPHY"; default = DPHY; }
    power_domain VDDIO { supply_pads = "VDD"; }
    pad VDD { domains = "power"; directions = "input"; }
    pad GND { domains = "ground"; directions = "input"; }
    pad DA0P {
        domains = "digital"; directions = "input"; traits = "differential_positive";
        power_domain = VDDIO; unpowered = clamped; when = "PHY=DPHY";
    }
    pad DA0N {
        domains = "digital"; directions = "input"; traits = "differential_negative";
        power_domain = VDDIO; unpowered = clamped; when = "PHY=DPHY";
    }
    pad TA0A { domains = "digital"; directions = "input"; traits = "trio"; when = "PHY=CPHY"; }
    pad TA0B { domains = "digital"; directions = "input"; traits = "trio"; when = "PHY=CPHY"; }
    pad TA0C { domains = "digital"; directions = "input"; traits = "trio"; when = "PHY=CPHY"; }
    pad GPIO0 { domains = "digital"; directions = "bidirectional"; drive_modes = "push_pull"; }
    peripheral SYNC: gpio { signal OUT: output; }
    mux GPIO0: SYNC.OUT { selector = "AF1"; when = "PHY=DPHY"; }
    unit LANE0: phy.lane { POS = DA0P; NEG = DA0N; }
    group DA0: differential_pair { positive = DA0P; negative = DA0N; when = "PHY=DPHY"; }
}
"""

PART = """part LANE_BRIDGE {
    category = "interface.serdes";
    footprint = "QFN-6";
    device = LANE_BRIDGE_DIE;
    pin VDD { number = "1"; bond = VDD; }
    pin GND { number = "2"; bond = GND; }
    pin L0 { number = "3"; bond = "DA0P@PHY=DPHY,TA0A@PHY=CPHY"; }
    pin L1 { number = "4"; bond = "DA0N@PHY=DPHY,TA0B@PHY=CPHY"; }
    pin L2 { number = "5"; bond = "TA0C@PHY=CPHY"; connection = optional; }
    pin G0 { number = "6"; bond = GPIO0; }
}
"""


def compile_bridge(tmp_path: Path, modes: str = "", body: str = ""):
    root = tmp_path / f"fixture-{len(tuple(tmp_path.iterdir()))}"
    root.mkdir()
    (root / "copper.mod").write_text(
        "module mode-default-test\n"
        "require github.com/test/bridge v1.0.0\n"
        "replace github.com/test/bridge => .\n",
        encoding="utf-8",
    )
    (root / "bridge_device.copper").write_text(DEVICE, encoding="utf-8")
    (root / "bridge.copper").write_text(PART, encoding="utf-8")
    selection = f' {{ modes = "{modes}"; }}' if modes else ";"
    return compile_source(
        f"""board ModeDefaults {{
            use library "tiny";
            import bridge "github.com/test/bridge";
            component U1: bridge.LANE_BRIDGE{selection}
            net VDD {{ U1.VDD; }}
            net GND {{ U1.GND; }}
            supply VDD {{ voltage = 1.8V; external = true; }}
            supply GND {{ voltage = 0V; external = true; }}
            {body}
        }}""",
        str(root / "board.copper"),
    )


def _codes(diagnostics) -> set[str]:
    return {item.code for item in diagnostics}


def test_default_mode_activates_bonds_and_pads_without_explicit_selection(tmp_path) -> None:
    board = compile_bridge(tmp_path, body="net LANE_P { U1.L0; } net LANE_N { U1.L1; }")

    diagnostics = check(board)

    assert diagnostics == []
    component = board.components[0]
    assert component.modes == {}
    device = board.devices["bridge.LANE_BRIDGE_DIE"]
    part = board.library["bridge.LANE_BRIDGE"]
    assert effective_modes(component, device) == {"PHY": "DPHY"}
    assert active_bonded_pads(component, part.pins["L0"], device) == ("DA0P",)
    assert [pad.name for pad in active_device_pads(component, part.pins["L1"], device)] == ["DA0N"]
    assert active_bonded_pads(component, part.pins["L2"], device) == ()


def test_explicit_selection_overrides_the_default_bonding(tmp_path) -> None:
    board = compile_bridge(
        tmp_path,
        modes="PHY=CPHY",
        body="net TRIO_A { U1.L0; } net TRIO_B { U1.L1; } net TRIO_C { U1.L2; }",
    )

    assert check(board) == []
    component = board.components[0]
    device = board.devices["bridge.LANE_BRIDGE_DIE"]
    part = board.library["bridge.LANE_BRIDGE"]
    assert active_bonded_pads(component, part.pins["L0"], device) == ("TA0A",)
    assert active_bonded_pads(component, part.pins["L2"], device) == ("TA0C",)


def test_pin_bonded_only_in_a_non_default_mode_stays_unmodeled(tmp_path) -> None:
    board = compile_bridge(tmp_path, body="net TRIO_C { U1.L2; }")

    assert "UNMODELED_PIN" in _codes(check(board))


def test_differential_group_conditioned_on_the_default_mode_applies(tmp_path) -> None:
    half_pair = "net LANE_P { U1.L0; }"

    default_codes = _codes(check(compile_bridge(tmp_path, body=half_pair)))
    selected_codes = _codes(check(compile_bridge(tmp_path, modes="PHY=CPHY", body=half_pair)))

    assert "INCOMPLETE_DIFFERENTIAL_PAIR" in default_codes
    assert "UNMODELED_PIN" not in default_codes
    assert "INCOMPLETE_DIFFERENTIAL_PAIR" not in selected_codes


def test_mux_option_conditioned_on_the_default_mode_is_available(tmp_path) -> None:
    configure = "configure U1.SYNC as FRAME_SYNC { OUT = G0; } net FSYNC { U1.G0; }"

    default_board = compile_bridge(tmp_path, body=configure)
    cphy_board = compile_bridge(tmp_path, modes="PHY=CPHY", body=configure)

    assert check(default_board) == []
    assert default_board.peripheral_selections[0].signals["OUT"].selector == "AF1"
    assert "INVALID_MUX_OPTION" in _codes(check(cphy_board))


def test_unit_terminals_resolve_through_default_bonds_in_every_backend(tmp_path) -> None:
    board = compile_bridge(
        tmp_path, body="net LANE_P { U1.LANE0.POS; } net LANE_N { U1.LANE0.NEG; }"
    )

    assert check(board) == []
    flat = elaborate(board)
    component = flat.components[0]
    part = flat.library[component.part]
    assert resolve_package_pin(flat, component, "LANE0.POS").name == "L0"
    assert _physical_pin_number(component, part, flat.devices, "LANE0.NEG") == "4"
    assert _physical_pin_name(component, part, flat.devices, "LANE0.POS") == "L0"

    cphy = elaborate(compile_bridge(tmp_path, modes="PHY=CPHY"))
    cphy_component = cphy.components[0]
    assert _physical_pin_number(cphy_component, part, cphy.devices, "LANE0.NEG") is None


def test_power_analysis_sees_pads_bonded_by_the_default_mode(tmp_path) -> None:
    body = """component PS1: VOLTAGE_SOURCE;
        net DRIVE { PS1.OUT; U1.L0; }
        supply DRIVE { voltage = 1.8V; source = PS1.OUT; }
        power_state IO_OFF { DRIVE = on; VDD = off; GND = on; }"""

    default_warnings = analyze_power_states(compile_bridge(tmp_path, body=body))
    cphy_warnings = analyze_power_states(compile_bridge(tmp_path, modes="PHY=CPHY", body=body))

    backpower = [item for item in default_warnings if item.code == "POSSIBLE_BACKPOWER"]
    assert [item.subject for item in backpower] == ["IO_OFF:U1.L0"]
    assert backpower[0].severity is Severity.WARNING
    assert "POSSIBLE_BACKPOWER" not in _codes(cphy_warnings)


def test_mode_group_without_default_still_requires_a_selection(tmp_path) -> None:
    board = compile_bridge(tmp_path, body="net LANE_P { U1.L0; } net LANE_N { U1.L1; }")
    device = board.devices["bridge.LANE_BRIDGE_DIE"]
    group = device.mode_groups["PHY"]
    from dataclasses import replace

    no_default = replace(device, mode_groups={"PHY": replace(group, default=None)})
    stripped = replace(board, devices={**board.devices, device.name: no_default})

    diagnostics = check(stripped)
    assert {"MODE_NOT_SELECTED", "UNMODELED_PIN"} <= _codes(diagnostics)
    assert has_errors(diagnostics)

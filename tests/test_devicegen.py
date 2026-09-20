from dataclasses import replace
from pathlib import Path

from pcbir.devicegen import (
    build_work_packet,
    check_generated,
    load_bundle,
    render_bundle,
    summarize_bundle,
    validate_bundle,
)


ROOT = Path(__file__).parents[1]
BUNDLE = ROOT / "device-data" / "stm32g0b1"


def test_device_bundle_is_valid_and_generated_sources_are_current() -> None:
    bundle = load_bundle(BUNDLE)

    assert validate_bundle(bundle) == ()
    assert check_generated(bundle, ROOT / "examples" / "packages" / "stm32") == ()
    assert set(render_bundle(bundle)) == {
        "stm32g0b1_device.copper",
        "stm32g0b1cbt6.copper",
    }
    assert summarize_bundle(bundle) == {
        "schema": "copperscript-device-bundle/v0.1",
        "device": "STM32G0B1",
        "pads": 6,
        "peripheral_signals": 5,
        "mux_options": 6,
        "parts": {"STM32G0B1CBT6": 6},
        "unknown_cells": 0,
        "validation_errors": 0,
    }


def test_work_packet_contains_only_requested_rows_and_legend() -> None:
    bundle = load_bundle(BUNDLE)

    packet = build_work_packet(bundle, "mux", match="I2C1")
    pin_packet = build_work_packet(
        bundle, "pins", part_name="STM32G0B1CBT6", match="PB6"
    )

    assert '"rows":2' in packet
    assert "PB7,I2C1,SDA,AF6" in packet
    assert "USART2" not in packet
    assert '"domains"' in pin_packet
    assert "PB6,45,PB6" in pin_packet


def test_unknown_marker_blocks_generation_and_is_counted() -> None:
    bundle = load_bundle(BUNDLE)
    first_pad = dict(bundle.pads[0])
    first_pad["domains"] = "?"
    incomplete = replace(bundle, pads=(first_pad, *bundle.pads[1:]))

    errors = validate_bundle(incomplete)
    summary = summarize_bundle(incomplete)
    packet = build_work_packet(incomplete, "pads", missing_only=True)

    assert any("unresolved fields: domains" in error for error in errors)
    assert summary["unknown_cells"] == 1
    assert '"rows":1' in packet
    assert "VDD,?" in packet


def test_bundle_validation_catches_cross_reference_errors() -> None:
    bundle = load_bundle(BUNDLE)
    first_mux = dict(bundle.mux[0])
    first_mux["pad"] = "NOT_A_PAD"
    invalid = replace(bundle, mux=(first_mux, *bundle.mux[1:]))

    assert any("unknown pad 'NOT_A_PAD'" in error for error in validate_bundle(invalid))


def test_bundle_renders_units_groups_modes_and_parametric_routes() -> None:
    bundle = load_bundle(BUNDLE)
    manifest = dict(bundle.manifest)
    device = dict(bundle.device)
    device.update(
        {
            "mode_groups": [{"name": "IO_MODE", "choices": ["GPIO", "ALT"], "default": "GPIO"}],
            "units": [{"name": "A", "kind": "std.buffer", "terminals": {"IN": "PA2", "OUT": "PA3"}}],
            "signal_groups": [{"name": "PAIR", "kind": "differential_pair", "members": {"positive": "PB6", "negative": "PB7"}}],
            "pad_sets": [{"name": "GPIO", "pads": ["PA2", "PA3"]}],
            "route_rules": [{"peripheral": "USART2", "signal": "TX", "pad_set": "GPIO", "selector": "pad_name"}],
        }
    )
    manifest["device"] = device
    rich = replace(bundle, manifest=manifest)

    assert validate_bundle(rich) == ()
    rendered = render_bundle(rich)["stm32g0b1_device.copper"]
    assert "unit A: std.buffer" in rendered
    assert "group PAIR: differential_pair" in rendered
    assert "mode_group IO_MODE" in rendered
    assert "route USART2.TX" in rendered

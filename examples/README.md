# CopperScript examples

Each standalone example owns a directory so its source, helper code, local
assets and generated reference inputs stay together. The reusable component
packages under `packages/` are separate from these board and language examples.

| Example | Entry point |
| --- | --- |
| Copper hard-macro definition | `macro_definition/board.copper` |
| Device model showcase | `device_model_showcase/board.copper` |
| Full vertical integration board | `full_vertical/board.copper` |
| FX10-like USB bridge, 121-ball 0.8 mm BGA (routing benchmark) | `fx10_bridge/board.copper` |
| Hierarchical board | `hierarchical_board/board.copper` |
| Invalid diagnostics fixture | `invalid_board/board.copper` |
| Mechanical anchors | `mechanical_anchors/board.copper` |
| Mechanical curves | `mechanical_curves/board.copper` |
| Mechanical editor demo | `mechanical_editor_demo/board.copper` |
| Mechanical outline | `mechanical_outline/board.copper` |
| Mechanical reference assets | `mechanical_reference/board.copper` |
| Mechanical routing probe | `mechanical_probe/mechanical_example.py` |
| nRF52832 coin-cell board | `nrf52_coin_cell/board.copper` |
| nRF antenna hard-macro probe | `nrf_antenna_macro/board.copper` |
| TPS62130A regulator hard-macro probe | `tps62130a_macro/board.copper` |
| Resolved local footprint | `resolved_footprint_board/board.copper` |
| Valid compiler fixture | `valid_board/board.copper` |
| Analog front end and vendor-model simulations | [Source, commands and current evidence](simulation/analog_frontend/README.md) |

The CM4 carrier, managed-footprint, and simulation examples already use
multi-file directories and retain their existing folder names.

For a complete LED-ring board project, use
[CopperLedRing](https://github.com/andenore/CopperLedRing). It owns the current
board source, exact component selections, dependency pins and routing evidence.
Follow that project's build instructions and qualification status; its recorded
routing pass does not imply fabrication readiness.

Physical macro examples declare their bindings in `.copper` with ordinary asset
imports. Versions and hashes use the existing dependency workflow; helper scripts
place/materialize those declarations without separately loading macro scenes.

## Deprecated fixtures

`round_led_ring/board.copper` and its optional Python wrapper are deprecated in
favor of CopperLedRing. They remain for compiler regression tests and existing
commands, not as a second maintained LED-ring design or a routing benchmark.
See the [legacy fixture notes](../docs/round-led-ring.md).

# CopperScript examples

Each standalone example owns a directory so its source, helper code, local
assets and generated reference inputs stay together. The reusable component
packages under `packages/` are separate from these board and language examples.

| Example | Entry point |
| --- | --- |
| Device model showcase | `device_model_showcase/board.copper` |
| Full vertical integration board | `full_vertical/board.copper` |
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
| Resolved local footprint | `resolved_footprint_board/board.copper` |
| Round LED ring | `round_led_ring/board.copper` |
| Valid compiler fixture | `valid_board/board.copper` |

The CM4 carrier, managed-footprint, and simulation examples already use
multi-file directories and retain their existing folder names.

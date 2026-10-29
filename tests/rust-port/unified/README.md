# Unified-router replay states

Inputs of the `replay-unified*` corpus suites: the states
`route-board --unified-save-inputs` writes before the unified router runs.
`generate.py` regenerates them with the Python CLI (the oracle); every run is
deterministic and writes the same bytes. Placement-feedback trial states
(`STATE.trial-N`) are deleted; the corpus keeps the selected placement's state.

    python tests/rust-port/unified/generate.py            # every state and the variants
    python tests/rust-port/unified/generate.py zones      # one board

Each board runs, from a scratch directory, with `PYTHONUTF8=1 PYTHONHASHSEED=0`:

    python -m copperscript route-board BOARD ARGS -o SCRATCH/routed.kicad_pcb \
        --report SCRATCH/report.json --unified-save-inputs tests/rust-port/unified/NAME.state.json

| Name | Board | Arguments |
|---|---|---|
| `valid_board` | `examples/valid_board/board.copper` | `--locked --offline --allow-proxy-footprints` |
| `routing` | `tests/rust-port/export-kicad-pcb/routing.copper` | `--locked --offline --allow-proxy-footprints` |
| `placement` | `tests/rust-port/export-kicad-pcb/placement.copper` | `--locked --offline --allow-proxy-footprints` |
| `six_layer` | `tests/rust-port/export-kicad-pcb/six_layer.copper` | `--locked --offline --allow-proxy-footprints --layers 6 --fab-profile jlcpcb-six-layer` |
| `zones` | `tests/rust-port/export-kicad-pcb/zones.copper` | `--locked --offline --allow-proxy-footprints --layers 4` |
| `layer_groups` | `tests/rust-port/si-check/layer_groups.copper` | `--locked --offline --allow-proxy-footprints --layers 6 --fab-profile jlcpcb-six-layer` (a length-match group of ordinary nets: the bus phase) |
| `dphy` | `tests/rust-port/si-check/dphy.copper` | `--locked --offline --allow-proxy-footprints --layers 6 --fab-profile jlcpcb-six-layer` |
| `round_led_ring` | `examples/round_led_ring/board.copper` | `--locked --layers 2 --fab-profile generic --candidates 1 --feedback-iterations 1 --router-iterations 5 --fanout --plane-contact-radius-mm 5` (the ladder's arguments; resolves footprints online) |
| `nrf52` | `examples/nrf52_coin_cell/board.copper` | `--locked --offline --layers 6 --fab-profile jlcpcb-six-layer --width-mm 50 --height-mm 40` |

`routing`, `six_layer`, `zones`, `round_led_ring` and `nrf52` leave nets
unrouted (route-board exits 1); their states are complete inputs either way.
The nRF52 board with proxy footprints fails before routing (macro footprint
identity mismatch), so it resolves its footprints from the lock.

`malformed/` holds variants of `valid_board.state.json` that each break one
rule (schema, digest, JSON syntax, an unknown or invalid option or board
field, a missing key); their content digests are recomputed unless the digest
is the point. `variants/` holds valid variants (`fine_regions="auto"`, a saved
two-net selection) that Python routes.

No state sets a time budget: the replay suites compare the routed result
state, the report's `unrouted` map, the exit status and stderr, never the
`seconds` and `PROGRESS` timing in stdout.

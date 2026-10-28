# M4 critical-replay fixtures

Inputs of the `replay-critical*` comparison suites (see
`scripts/compare_corpus.json`). `copper replay-critical` replays the critical
routing stage from saved inputs; the Rust router cannot produce them yet, so
they are produced once by the Python CLI and committed. Both implementations
then read the same files.

## Checkpoints

`NAME.state.json` is the `critical-inputs.state.json` that

```sh
python -m copperscript route-board BOARD OPTIONS --routing-profile quick --checkpoint-dir DIR
```

writes, run from the repository root with `PYTHONHASHSEED=0 PYTHONUTF8=1`.
The rest of the routing run is not needed: `generate.py` stops it as soon as
the checkpoint is complete.

| Checkpoint | Board | Options |
|---|---|---|
| `routing` | `tests/rust-port/export-kicad-pcb/routing.copper` | `--allow-proxy-footprints --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `si-dphy` | `tests/rust-port/si-check/dphy.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `si-four-layer` | `tests/rust-port/si-check/four_layer.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 4 --fab-profile jlcpcb-four-layer` |
| `si-layer-groups` | `tests/rust-port/si-check/layer_groups.copper` | as `si-dphy` |
| `si-no-stackup` | `tests/rust-port/si-check/no_stackup.copper` | as `si-dphy` |
| `si-reference-planes` | `tests/rust-port/si-check/reference_planes.copper` | as `si-dphy` |
| `si-single-ended` | `tests/rust-port/si-check/single_ended.copper` | as `si-dphy` |
| `si-two-layer` | `tests/rust-port/si-check/two_layer.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 2` |
| `nrf-antenna-macro` | `examples/nrf_antenna_macro/board.copper` | `--locked --offline --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `full-vertical` | `examples/full_vertical/board.copper` | `--locked --no-check --layers 6 --fab-profile jlcpcb-six-layer` (resolves footprints online; proxies do not fit its macros) |

The trivial no-critical-net checkpoint of `examples/valid_board` (and its
placement checkpoint, which `replay-critical` also accepts) is the M5 report
fixture `tests/rust-port/reports/states/`.

## Derived inputs

`bad/` holds inputs for the error paths and `derived/` valid inputs for
result paths no checkpoint reaches. Each is a small edit of a real state,
re-digested so that it reaches the check it is about:

| File | Edit | Reaches |
|---|---|---|
| `bad/not-a-state.json` | another schema | not a state file |
| `bad/invalid-json.state.json`, `bad/empty.state.json`, `bad/not-utf8.state.json` | truncated, empty, not UTF-8 | the JSON and decode errors |
| `bad/digest-mismatch.state.json` | one digest digit | content digest mismatch |
| `bad/foreign-type.state.json` | an `os:system` value | the codec's type allow-list |
| `bad/plain-number.state.json`, `bad/no-global-route.state.json` | a number; the dict without `global_route` | missing board/global route |
| `bad/unknown-board-field.state.json` | `PhysicalBoard.bogus` | unknown init field |
| `bad/bad-guide-layer.state.json` | a guide segment on `X.Cu` | an enum `ValueError` |
| `bad/stale-macro-guides.state.json` | nRF guides with another placement fingerprint | stale hard-macro guides |
| `bad/reserved-accesses.state.json` | a `FanoutResult` without fields | Python's crash (Rust: not implemented) |
| `derived/asymmetric-pair.state.json` | `routing`'s `D_P` partner set to `GND` | the asymmetric pair result (`--only D_N`) |

Regenerate with

```sh
python tests/rust-port/critical/generate.py            # every checkpoint (minutes per board)
python tests/rust-port/critical/generate.py routing    # one checkpoint
python tests/rust-port/critical/generate.py --derived  # bad/ and derived/ from the committed checkpoints
```

Changed inputs invalidate the comparison: regenerate the derived inputs after
any checkpoint.

## Comparing

The report and stdout carry wall time (`seconds`, and
`local_preparation.elapsed_seconds` of each group), so the result suites
compare the exit status, stderr, the KiCad project files and the report
without those keys (the harness `json` option's `without` form), never
stdout. The error and usage suites print no wall time and compare stdout too.

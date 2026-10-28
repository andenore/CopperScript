# M6 `editor-overlay` fixtures

Build runs for the `editor-overlay` and `editor-overlay-options` comparison
suites (`scripts/compare_corpus.json`). `editor-overlay` reads a routed
build run (`run.json`, `board.kicad_pcb`, `board.editor-intent.json` and the
optional `kicad-drc.json`) and probes the saved board with KiCad's Python.
There is no Rust router yet and no KiCad in the harness, so the runs are
produced once by the Python CLI and committed, and the probe is stood in for.

Regenerate everything with

```sh
python tests/rust-port/overlay/generate.py
```

which routes each board with `route-board --routing-profile quick --locked
--offline --allow-proxy-footprints` (`PYTHONHASHSEED=0 PYTHONUTF8=1`), keeps
the draft board and the editor intent the compiler writes beside it, and
writes a `run.json` with the digests `pcbir.build` records (the real runner
needs `kicad-cli`). The harness copies a run into the case's output directory
first (the corpus `stage` field), because the command binds the overlay into
the sibling `run.json`; both implementations then start from identical copies
and the rewritten manifest is compared like any other output.

| Run | Board | What it covers |
|---|---|---|
| `flat` | `boards/flat.copper` | two front parts, one routing layer, no vias: the success path |
| `rear` | `boards/rear.copper` | rear-side parts: the native orientation differs by 180 degrees |
| `demo` | `examples/mechanical_editor_demo` | a partial route (exit 1 build) with holes and a cutout |
| `valid` | `examples/valid_board` | vias: Python compares the probe's `via_type` field against an intent without one, so every board with vias is refused as "native PCB copper does not match" (reproduced as is) |
| `verified`, `remaining` | `flat` | `kicad-drc.json` bound by digest, with no and with one remaining connection |
| `filled` | `flat` | the board plus a filled polygon the probe reads as native copper |
| `drc-unbound`, `drc-units` | `flat` | evidence present but unbound (stale report digest, inch units): no credit |
| `intent-schema`, `unbound`, `pcb-changed`, `pose-mismatch`, `copper-mismatch` | `flat` | one broken binding each |
| `duplicate-key`, `nonfinite`, `bad-json`, `missing-intent`, `manifest-list` | `flat` | malformed inputs (`manifest-list` is an uncaught Python `AttributeError`) |

`kicad-python/` holds the probe stand-ins the suites pass as
`--kicad-python`: `fake-kicad-python.sh` runs `native_probe_stub.py`, a reader
of the saved `.kicad_pcb` text that prints what the pcbnew probe prints
(layers, footprint poses, segments, vias, `filled_polygon` rings);
`failing-kicad-python.sh` exits 2 like a Python without `pcbnew`;
`garbled-kicad-python.sh` prints no JSON. The stand-in takes the board as its
last argument, because Python runs `python native_overlay.py BOARD` while the
binary runs `python -c SCRIPT BOARD`. On Windows the shell stand-ins cannot
execute and both implementations report `[WinError 193]`.

These are comparison inputs, not routing or qualification evidence; the
`kicad-drc.json` files here were written by hand, no native DRC ran.

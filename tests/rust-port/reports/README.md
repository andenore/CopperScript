# M5 report fixtures

Run artefacts for the M5 comparison suites (`explain`, `issues`, `view`,
`summarize`; see `scripts/compare_corpus.json`). `issues`, `view` and
`summarize` read what `route-board` and `plan-layout` wrote; the Rust router
does not exist yet, so these inputs are produced once by the Python CLI and
committed. Both implementations then read the same files.

Regenerate everything with

```sh
python tests/rust-port/reports/generate.py
```

which runs, from the repository root with `PYTHONHASHSEED=0 PYTHONUTF8=1`
(each into `build/report-fixtures/NAME`, keeping the files listed):

| Fixture | Command (`python -m copperscript ... --locked --offline --allow-proxy-footprints`) | Kept |
|---|---|---|
| `runs/valid-pass` | `route-board examples/valid_board/board.copper --routing-profile quick` | report, board |
| `runs/valid-clearance` | `route-board examples/valid_board/board.copper --routing-profile quick --width-mm 40 --height-mm 30` | report, board |
| `runs/valid-stopped` | `route-board examples/valid_board/board.copper --routing-profile quick --stop-after placement` | report, board |
| `runs/hierarchical` | `route-board examples/hierarchical_board/board.copper --routing-profile quick` | report, board |
| `runs/outline` | `route-board examples/mechanical_outline/board.copper --routing-profile quick` | report, board |
| `runs/margin-error` | `route-board examples/valid_board/board.copper --routing-profile quick --width-mm 6 --height-mm 6` | error record |
| `runs/ring-error` | `route-board examples/round_led_ring/board.copper --routing-profile quick` | error record, its diagnostic state |
| `runs/layout` | `plan-layout examples/valid_board/board.copper` | layout report, board |
| `states/*.state.json` | `route-board examples/valid_board/board.copper --routing-profile quick --checkpoint-dir DIR` | final board, placement checkpoint, critical inputs |

Every command adds `--report build/report-fixtures/NAME/<report>.json -o
build/report-fixtures/NAME/board.kicad_pcb`.

The small boards never reach several report branches (native findings,
global and critical failures, length budgets, escape findings, plane
estimates, replay summaries, layout gates, requirement failures, late bypass
passes). `generate.py` adds reports for them next to `runs/valid-clearance`,
derived from its real report and located on its real pads and tracks:
`native.json` (with `native/filled.kicad_pcb`, the board plus a filled GND
zone, bound by digest), `closed.json` and `unbound.json` (a closed and an
unbound native verification), `failing.json` and `replay.json`. `runs/renamed`
holds a report found by its schema rather than its name. `boards/` holds a
rotated copy of a run's board for reference overlays and `scene.kicad_pcb`, a
handwritten board with every construct the board reader handles. `bad/` holds
reports that `summarize` and `issues` refuse.

These are working artefacts for comparing implementations, not routing or
qualification evidence.

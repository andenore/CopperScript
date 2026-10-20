# Multi-board routing benchmark

One board cannot judge a router change: on full-vertical a small change in
search order reroutes the whole board, so via counts and track length move
by 10–20% in either direction for reasons unrelated to the change. The
benchmark routes many third-party boards, each with its placement held
fixed, and scores them with KiCad rather than with CopperScript's own DRC.
It is measurement, never signoff.

## Boards

The board set is PCBWorld's D3 benchmark (LG AI Research, BSD-3 code),
built from the PCBench corpus (MIT, 1,182 open hardware boards). The
default tier is D3-A: the 99 easy test boards named in PCBWorld's
`configs/datasets/d3.json`. Pins for both repositories, the KiCad series,
the tier, the shard count and the per-board time limit are in
`.github/routing-benchmark.json`.

PCBench ships KiCad 5 boards. The workflow prepares them with PCBWorld's
own chain under stock KiCad 9: `convert_v9.py` (re-save in KiCad 9 format,
creating the `.kicad_pro`), `drc_fix_v9.py` (relax checks KiCad 9 added or
tightened, keep boards whose original routing passes), and `make_guide.py`
(fold each net's original track width into its net class). The board
under test is `processed_v9_guide_v3.kicad_pcb`, the file D3 names. The
difficulty sort is skipped: boards keep their D3 names directly.

## Routing and scoring

`python -m pcbir.bench route BOARD --out DIR` imports the board
([KiCad board import](kicad-board-import.md)), routes it through
`run_routing_pipeline` with the imported placement as `initial_placement`,
and writes the copper back into the original board text: the top-level
`segment`, `arc` and `via` items are removed and CopperScript's tracks and
vias are appended, using the file's own net numbers and copper layer names.
Every other byte of the file is unchanged. The `.kicad_pro` is copied
alongside.

`python -m pcbir.bench score DIR --pcbnew-python PY` first refills the
board's zones through KiCad's `pcbnew` module (zones still hold the original
routing's fill), as PCBWorld's own preparation does, then runs stock
`kicad-cli pcb drc --format json --all-track-errors` on the written board
under its own project rules. PCBWorld's definitions apply:

| Measure | Meaning |
|---|---|
| Clean pass | Zero error-level violations and zero unconnected items |
| KiCad errors | Error-level violations, also counted by type |
| Unconnected | KiCad's unconnected items after zone refill |
| Net completion | Routable nets CopperScript reports connected |

A board that fails to import, crashes, or exceeds its time limit (status
`incomplete`) still counts against the clean pass rate.

`python -m pcbir.bench reference BOARD --out DIR` stages a board with its
original routing untouched; scoring the references checks the scorer,
since `drc_fix_v9.py` kept only boards whose original routing passes.

## CI

`.github/workflows/routing-benchmark.yml` runs on manual dispatch (with an
optional board limit) and on pushes to `claude/**` that touch the benchmark
or the importer. It prepares the boards once, routes them across matrix
shards, then scores every board and its reference and publishes the
aggregate and per-board tables in the run summary and as a notice
annotation. Artifacts keep each routed board, its KiCad DRC report and
its result record.

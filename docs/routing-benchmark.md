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
optional board limit) and on pushes to `claude/**` that touch `pcbir/` or the
benchmark files. It prepares the boards once, routes them across matrix
shards, then scores every board and its reference and publishes the
aggregate and per-board tables in the run summary and as a notice
annotation. Artifacts keep each routed board, its KiCad DRC report and
its result record.

## Board-by-board comparison

`.github/routing-benchmark-baseline.json` holds each board's committed
outcome: status, clean pass, unrouted nets, KiCad errors, unconnected items,
vias and length. Every run ends with
`python -m pcbir.bench compare .github/routing-benchmark-baseline.json RESULTS`.
It lists the boards that became clean or stopped being clean, and changes in
unrouted nets, errors and unconnected items on boards that are still unclean.
For boards clean in both runs, it gives total vias and length. **The job
fails if a board that was clean in the baseline is no longer clean.** A change
that improves the benchmark refreshes the baseline in the same pull request.
Each run publishes its own record as the "Benchmark baseline record"
annotations (`python -m pcbir.bench baseline RESULTS` writes the JSON).

The workflow runs on pushes to `claude/**` branches that touch `pcbir/` or
the benchmark files, so router changes are measured too, and on manual
dispatch.

## Baseline (2026-10-20)

D3-A, 99 boards, commit 07f51f6, stock KiCad 9.0.9 on `ubuntu-24.04`:

| Measure | CopperScript | Original routing (scorer check) |
|---|---:|---:|
| Clean pass | 79 / 99 (0.798) | 99 / 99 |
| Mean net completion | 0.909 | |
| Mean KiCad error-level violations | 0.081 | 0 |
| Mean unconnected items | 1.31 | 0 |
| Mean vias | 3.40 | |
| Mean track length | 156.3 mm | |
| Mean seconds per board | 31.8 (max 971) | |

For orientation only, PCBWorld reports clean pass 0.80 for Freerouting,
0.86 for its PPO agent and 0.74 for KiCadRoutingTools on D3-A. Those
numbers come from PCBWorld's own engine build and scorer, not this workflow,
so they are not a like-for-like comparison.

The 20 failures, by first cause:

| Cause | Boards |
|---|---:|
| Package-access preflight blocks area routing (pin access on small boards with few global tiles) | 6 |
| One net left unrouted without a located diagnostic | 8 |
| Detailed search cannot reach a terminal | 4 |
| KiCad clearance or short violations CopperScript's own DRC does not report | 2 |

Fixing the importer before this baseline raised the clean pass from 0.697:
surface custom pads now use their copper's own bounding box, and pad-only
spacing findings on imported placements no longer block package access (see
[KiCad board import](kicad-board-import.md)).

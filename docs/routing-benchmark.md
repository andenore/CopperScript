# Multi-board routing benchmark

One board cannot judge a router change: on full-vertical a small change in
search order reroutes the whole board, so via counts and track length move
by 10–20% in either direction for reasons unrelated to the change. The
benchmark routes many third-party boards, each with its placement held
fixed, and scores them with KiCad rather than with CopperScript's own DRC.
It is measurement, never signoff.

## Boards

The board set is PCBWorld's D3 benchmark (LG AI Research, BSD-3 code),
built from the PCBench corpus (MIT, 1,182 open hardware boards). Two tiers
run: D3-A, the 99 easy test boards, and D3-B, the 10 medium test boards
(PCBWorld's `easy` and `medium` test splits in `configs/datasets/d3.json`).
Pins for both repositories, the KiCad series, the tiers, the shard count and
the per-board time limit are in `.github/routing-benchmark.json`. The
prepare job writes `tiers.json` (board → tier) next to the boards;
`summarize --tiers` and `compare --tiers` report each tier separately.

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
It gives each tier's clean count, lists the boards that became clean or
stopped being clean, and changes in
unrouted nets, errors and unconnected items on boards that are still unclean.
For boards clean in both runs, it gives total vias and length. **The job
fails if a board that was clean in the baseline is no longer clean.** A change
that improves the benchmark refreshes the baseline in the same pull request.
Each run publishes its own record as the "Benchmark baseline record"
annotations. To refresh the baseline from a run, save those annotations'
text to a file and run
`python -m pcbir.bench baseline FILE --from-record --source "run ID on COMMIT"`;
from local results, `python -m pcbir.bench baseline RESULTS` writes the
same JSON.

The workflow runs on pushes to `claude/**` branches that touch `pcbir/` or
the benchmark files, so router changes are measured too, and on manual
dispatch.

## Router choice

`router` in `.github/routing-benchmark.json` names the ordinary router the
benchmark measures (`legacy` or `unified`, as `route-board --router`); a
manual dispatch may override it with the `router` input. The committed
baseline belongs to the pinned router, so a run of the other router fails the
lost-board check by design. The per-board record also carries routable nets
and routing seconds. To compare two runs on every recorded measure, save
both runs' "Benchmark baseline record" annotations (or use two local
`baseline.json` files) and run
`python -m pcbir.bench versus FIRST SECOND --names legacy unified`. It reports
clean boards, fully routed boards, net completion, unrouted nets, KiCad errors,
unconnected items, vias, length and bends on boards clean in both, bends per 100 mm, routing time
(total, median, slowest) and failed runs, plus per-board better/equal/worse
counts and the boards clean with only one of the two.

## Local runs

`python -m pcbir.bench run BOARDS --out RESULTS --router unified --workers 6
--kicad-cli KICAD_CLI --pcbnew-python PY` routes and scores every prepared
board in `BOARDS` (`BOARDS/<id>/processed_v9_guide_v3.kicad_pcb` with its
`.kicad_pro`, the layout of the CI's `bench-boards` artifact: download it
from any benchmark run). Each board routes in its own process under
`--board-seconds` (default 1200, as CI). `RESULTS` must be new or empty; the
run writes per-board records, `summary.md`, `baseline.json` and `record.txt`.
`--only` (board ids or number prefixes) and `--limit` select a subset; `--set OPTION=VALUE` (repeatable) overrides a `UnifiedRouterOptions` field for `--router unified`, so an option is measured before it becomes a default. On
Windows, KiCad's own interpreter (`KiCad/10.0/bin/python.exe`) imports
`pcbnew`. Local scores use the installed KiCad, so compare a local run with
another local run, not with the KiCad 9 CI baseline.

## Failure causes

Every routed record carries the placed board's escape diagnostics and a cause
class for each unrouted net (see [escape planning](escape-planning.md)).
`python -m pcbir.bench causes RESULTS` tabulates unrouted nets per primary
cause for every unclean board, and marks boards that fail only KiCad's check;
the run summary includes it.

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

## Progress

| Change | Clean boards | Notes |
|---|---:|---|
| Baseline (2026-10-20) | 79 | |
| Plan 3.1: copper text as an obstacle | 81 | 0011 and 0026 clean; no KiCad error-level violations left on any board |
| Plan 3.2: wide net classes neck down at narrower lands | 82 | 0078 clean; full-vertical identical |
| Plan 3.4: every separate land of a pad number is routed unless declared internally joined | 89 | 0013, 0045, 0067, 0069, 0085, 0089 and 0090 clean; full-vertical identical |
| Routing refactor merged (PR #15) | 91 | 0068 and 0099 clean; one inaccessible pin no longer blocks a whole board |

# Routing performance and optimization assessment

## Package-access via search

The preferred plane-contact search stops considering farther via rings once
their minimum possible pad-to-via distance exceeds the best legal choice already
found. The existing 2 mm search cap and exact copper clearance checks remain.
This preserves candidate selection because a route cannot be shorter than its
pad-to-via displacement.

On the full-vertical `candidate-00` package-access preflight with both hard
macros, the same inputs and unprofiled process took 248.3 s with the previous
search and 121.7 s with this pruning. The three plane-stitch passes changed from
67.5/69.2/69.9 s to 21.4/21.3/33.8 s. Both results were ready with 113 plane
contacts, the same critical-route fingerprint, and the same native board and
report digests. These are single-run measurements, not a full-route timing or
manufacturing signoff claim. Ignored diagnostic reports are in
`build/package-access-timing-{unpruned,pruned}-20261009.json`.

## Profiling future runs

Package preflight now negotiates ordinary escape patterns after a bounded
initial pair-search tier (default 6,000 aggregate expanded states per pair/pass),
before committing to historical full-budget searches. If incomplete, it retries
the original pattern at full budget. `--package-initial-pair-states 0` provides
the comparison/disable mode; `--package-pattern-trials 0` also disables staging.
Pass these through the generic build wrapper after `--`. Reports include
`package_access.search_tiers` with all actual probe/revalidation work and the
selected tier. A limited failure is not evidence that no legal route exists.
The read-only run summary includes `critical_expanded_states` across completed
groups, including discarded pattern/probe work, plus tier/fallback details. Do
not add tier totals to the group totals again. The comparison helper allows the
staging knob as an explicit intervention, while preserving other budget,
quality, provenance and uninstrumented-timing checks.
See [the plan and acceptance criteria](routing-performance.md).

The complete workflow now enables standard-library `cProfile` function profiling
and streamed phase events by default; no additional dependency is required:

```powershell
make EXAMPLE=full-vertical route
```

Each fresh ignored `build/full-vertical/runs/<UTC-run-id>/` contains:

- `routing.prof`: complete Python call statistics, including callers/callees.
- `profile-summary.txt` and `profile-summary.json`: top 50 functions by self time
  and cumulative time, plus call counts.
- `phase-timings.json`: paired start/finish events, inclusive elapsed spans,
  trial decisions, malformed-event count and unfinished phases.
- `routing.log`: the original events and diagnostics.
- `run.json`: actual launch and routing commands, Python/platform/CPU information,
  Git commit and tracked-diff hash, input hashes, profile status and router exit code.

Direct `python -m copperscript route-board ...` remains unprofiled. To profile
another module invocation, preserving failure exit codes:

```powershell
New-Item -ItemType Directory -Force build/profile | Out-Null
uv run --no-sync python -m pcbir.profiling --output build/profile/routing.prof `
  --module copperscript -- route-board examples/valid_board/board.copper `
  --allow-proxy-footprints --report build/profile/route-report.json `
  -o build/profile/board.kicad_pcb
```

The proxy-footprint invocation above is only a small diagnostic example, not the
full-board or manufacturing workflow. Existing profile files can be inspected
with `uv run --no-sync python -m pstats <path-to-routing.prof>`.

Profiling is observational, not signoff evidence. The router's exit 1 remains
exit 1, even if profile collection succeeds. This wrapper intentionally does not
use `python -m cProfile`: its utility catches `SystemExit`. A forcibly terminated
process may leave no profile; the manifest reports `missing`, not success.
Normal exceptions and in-process interrupts attempt to save partial statistics;
they do not establish a completed routing result. Profiling/report failures are
recorded separately and cannot replace the router's exit status.

Function profiling adds overhead, potentially substantial in a function-call-heavy
maze search. Cumulative times include callees and must not be summed; phase spans
also overlap when trials contain whole pipelines. The profile covers the routing
Python process, not KiCad's internal CPU work; subprocess wait time and the
independent-KiCad phase are visible, but native KiCad internals are not attributed.
See the [Python profiling documentation](https://docs.python.org/3.13/library/profile.html).

For uninstrumented wall-time comparisons, keep progress events but disable
function instrumentation:

```powershell
make EXAMPLE=full-vertical route PROFILE=none
```

Inspect a run (including an unfinished run) without modifying its artifacts:

```powershell
uv run --no-sync python -m pcbir.routing_benchmark summarize "build/full-vertical/runs/<run-id>"
```

For the O1b comparison, first complete the function-profile run, then run these
uninstrumented variants sequentially on the same otherwise idle machine. Repeat
in alternating order before drawing a wall-time conclusion:

```powershell
make EXAMPLE=full-vertical route PROFILE=none EXTRA_ROUTE_ARGS=--no-incremental-placement-repair
make EXAMPLE=full-vertical route PROFILE=none
uv run --no-sync python -m pcbir.routing_benchmark compare "build/full-vertical/runs/<baseline-id>" "build/full-vertical/runs/<candidate-id>"
```

`routing_benchmark` checks named failed signal/critical nets, pending ground,
duplicate-land and fanout pads, native hard finding categories/affected nets,
required native-check coverage, overflow and independent KiCad findings. It
reports selected-route length/via/layer metrics rather than trading them away
implicitly. A comparison with new failures, missing final data, changed recorded
inputs/tools/settings or either run function-profiled produces **no timing ratio**.
Exit 0 means eligible for timing review, 1 means a quality regression, and 2 means
incomparable/incomplete data or an input error; none means manufacturing signoff.
The standalone `summarize` command returns 0 for valid observations, even if a run
is still active, and clearly marks its completion state.

The two explicit repair switches (`--no-incremental-placement-repair` and
`--zone-dependency-expansions`) are interventions under test and may differ. Other
settings, including search budgets, must match. Runtime provenance does not hash
all footprint files or capture machine load, so an eligible ratio still requires
external-library verification and repeated measurements. This is a review aid,
not automatic optimization acceptance. Structured progress provides completed
full/incremental trial and subset/probe counts, including rejected work; a legacy
plain log uses a clearly labelled final-report lower bound instead of inventing
zero work. General-area expanded maze states and peak memory are not yet recorded
by this tool; paired critical searches do report their expanded states.

### Per-net area progress

With `--progress`, detailed routing emits `detailed_pass` and `detailed_net`
start/finish events. A net starts **before grid construction**, so an unfinished
event identifies the active net even if grid preparation or a repair is slow.
Pass events include ordinary-net counts and provisional failed-net names. Net
events include pass/ordinal (where applicable), pad count, requested pitch and
search budget, layer/direction preferences, and final tentative connectivity,
track/via counts and diagnostics. These events do not change the IR, selected
copper, fingerprints, JSON reports or exports.

Repair stages are `failed_first`, `soft_merge`, `soft_ripup`, `evicted_net`
(with its repair owner), and `final_retry`; a normal pass uses `pass`.
`detailed_repair` and `detailed_neutral_fallback` events state whether their
candidate was selected. The same observer is forwarded through placement trials
and local zone dependency reroutes. There are no per-expanded-state callbacks.
The requested budget is not an aggregate net budget: branch searches, guide
expansions and bounded pitch refinements can perform several searches.

Every finished net event also carries a process-local `search_identity`.
Within one repair stage, a search repeated with identical inputs is answered
from a bounded memo (`search_reuse: "hit"`, with the original `reused_search_seconds`); a
`detailed_search_reuse` summary follows `detailed_repair`. The attempt is the
identical earlier result, so copper, reports and fingerprints do not change.
`--search-reuse-entries 0` disables it for comparison runs; see
[exact repair-search reuse](routing-performance.md).

Follow a running build in PowerShell:

```powershell
Get-Content "build/full-vertical/runs/<run-id>/routing.log" -Tail 20 -Wait
```

The read-only summary reports `detailed_net_attempts`,
`failed_detailed_attempts`, `detailed_attempt_stages` and
`reused_detailed_attempts`, including tentative and rejected work. Counts are
**not** unique accepted nets or DRC closure.
Older logs without completed per-net events have unknown (`null`) attempt
counts. Interrupted attempts remain in `work.unfinished`, without an invented
duration or success; a forcibly killed Windows process may also leave the
original manifest unfinalized. Check process liveness as well as its status.
Inclusive phase durations overlap and are not percentages or an ETA.

Compare repeated runs with identical source/lock, actual footprint files, options,
KiCad/Python versions and machine load. The recorded hashes do not fingerprint
every footprint or external tool; record CopperLib revision and KiCad version
alongside a benchmark. Compare connectivity, exact geometry/fingerprints, DRC,
pending escapes, wire length and via counts as well as time. Report wall time and
peak memory; distinguish cold and warm runs. Do not infer speedups by comparing
instrumented and uninstrumented times. An out-of-process sampler such as
[py-spy](https://github.com/benfred/py-spy) is a useful lower-overhead cross-check,
but is not a required dependency or enabled by the shared build workflow.

## Where parallelism is unsafe or unlikely to help first

Per-net threads are not a straightforward speedup: ordinary searches share mutable
usage, congestion history, rip-up decisions and committed copper. Independent
searches on the same snapshot can produce mutually conflicting routes, requiring
conflict detection, deterministic reconciliation and rerouting. Spatial partitions
likewise require boundary/halo ownership and cross-partition repair. Treat these as
algorithm changes, not a `ThreadPoolExecutor` wrapper.

The usual GIL-enabled CPython build does not run CPU-bound Python bytecode in
parallel through ordinary threads. Processes can bypass that limitation, but need
picklable inputs/results and an importable entry point; Windows spawn costs and
memory copies matter. See [Python's process-executor documentation](https://docs.python.org/3.13/library/concurrent.futures.html#processpoolexecutor).
Parallel rendering/logging or independent CAM checks may improve convenience, but
the historical few seconds of KiCad verification do not justify making them the
first optimization target. No parallel router or relaxed correctness gates have
been enabled by this profiling change.



## Exact search reuse and acceptance

Repair-stage memoization is bounded by `--search-reuse-entries` (default 128;
0 disables). Reuse requires identical board/obstacle geometry, keepouts, rules,
guides, anchors, congestion, options and ordered copper state. Any blocking
geometry change needs a fresh search, including previously failed searches;
budget exhaustion is not a reusable proof of impossibility on another board.
Separate board objects conservatively receive separate caches. Grid material
queries may reuse immutable board/axes data but keep fresh mutable query state.

Incremental placement repairs invalidate incident nets, moved-pad/keepout/contact
collisions and derived guide/access evidence. Unsupported ownership or excessive
dependency cones fall back to a full pipeline. No failed transaction publishes
tentative copper. Static geometry caches need complete snapshot/invalidation keys.

Compare connectivity, exact DRC, critical/package constraints, deterministic
results and final-copper quality alongside uninstrumented time and work counts.
Profiling alone must not change artifact bytes. Routing changes may select
different legal geometry; fewer states or a shorter runtime cannot excuse
newly failed routes. Open work is in [optimization backlog](routing-optimization-todo.md).

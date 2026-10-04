# Routing performance and optimization assessment

## Profiling future runs

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
zero work. Expanded maze states and peak memory are not yet recorded by this tool.

Compare repeated runs with identical source/lock, actual footprint files, options,
KiCad/Python versions and machine load. The recorded hashes do not fingerprint
every footprint or external tool; record CopperLib revision and KiCad version
alongside a benchmark. Compare connectivity, exact geometry/fingerprints, DRC,
pending escapes, wire length and via counts as well as time. Report wall time and
peak memory; distinguish cold and warm runs. Do not infer speedups by comparing
instrumented and uninstrumented times. An out-of-process sampler such as
[py-spy](https://github.com/benfred/py-spy) is a useful lower-overhead cross-check,
but is not a required dependency or enabled by the shared build workflow.

## Evidence available on 2026-10-02

The latest full run (`20261002T080102482942Z`, routing code `31e515e`) took
**3,985.613 seconds / 66.4 minutes**. Its report contains three full ground-feedback
evaluations after the initial routing pipeline: accepted current-placement
escape, rejected current-placement escape, and accepted `C_CC` 270-degree rotation.
Thus four complete pipeline evaluations were used for a small number of remaining
ground contacts. This establishes repeated work, not a measured breakdown of the
66 minutes: that run predates continuous profiling and streamed phase events.

An earlier pass-13 phase-timed run, using an earlier pipeline order, measured:

| Stage | Seconds |
| --- | ---: |
| Placement/global | 121.067 |
| Critical routing | 87.962 |
| Fanout | 3.720 |
| Detailed routing | 1,247.980 |
| Initial physical closure/native DRC | 9.193 |
| Ground feedback, including local repairs | 117.225 |
| Independent KiCad verification | 3.640 |

Total elapsed was 1,603.499 seconds; detailed routing accounted for about **78%**.
The stages shown do not account for every setup/finalization second. This is
historical evidence, not a current-run profile or a prediction of future timings.
The retained pass-13 timing file lives in the task's review outputs; the latest
full run's manifest/report live in ignored `build/` and are not portable repository
fixtures.

The earlier R9 15-second/734-sample snapshot attributed 27.4% of inclusive samples
to via checks, 13.1% to track checks and 12.8% to neighbor generation. These categories
overlap and cover one routing interval, not the whole board. Search-local full-span
via caching and exact rectangular-outline fast paths have already been implemented;
they must not be counted again as proposed new optimizations.

## Recommended order

1. **Reduce the number of whole-board evaluations.** Extend the existing local
   repair in `escape_feedback.py` to a dependency-bounded repair for small placement
   moves/rotations: invalidate moved-package exits, incident nets and interfering
   copper; expand that affected set if repair collides with additional nets.
   Preserve unrelated copper byte-for-byte and locked critical-group ownership;
   validate native connectivity/DRC and the final filled KiCad board. Recompute
   congestion/clearance state for the affected transaction and fall back to a full
   pipeline if dependency bounds or validation fail. Reserve required ground access
   alongside package exits where appropriate, so ordinary routing cannot consume
   the last escape. Local rip-up and early contact screening already exist; improve
   those mechanisms rather than adding another duplicate repair controller.

2. **Reduce exact-check/allocation work in measured hot paths.** Audit
   `detailed._search_once`, `_Grid.point` and `RoutingClearanceIndex`. Candidate
   targets are repeated `Point`/rounded-shape allocation, immutable pad/keepout
   geometry reconstruction, and broad-phase bucket tuning. The clearance index
   already has spatial hashing and exact narrow-phase checks; searches already
   cache track/via legality, heuristics and obstacle queries. Cache static geometry
   separately from changing routed copper. Any reuse must include placement/grid,
   net width/clearance, via physical span and copper revision; otherwise it can
   create false legal routes. Net-specific grid coordinates mean grids cannot be
   blindly shared. Optimize only where full profiles show significant self time
   or excessive calls, retaining tangency, rotated-pad and all-layer-via tests.

3. **Reduce maze states, not just time per state.** Tighten adaptive guide corridors
   and coarse-to-fine search around the existing progressive guides; expand them
   on failure. Precheck feasible ports/layer reachability before expensive repair
   searches. Evaluate multi-terminal tree-target indexing where target sets are
   large; cached minimum-distance heuristics still scan targets on first use.
   Measure expanded states, guide expansions and budget failures. Retain the
   wide/neutral fallback and exact final geometry checks. Do not claim a speedup
   by shrinking budgets until a formerly routable board simply fails faster.

4. **Parallelize independent candidate evaluation with processes.** Independent
   placement/global candidates and separately routed trial boards are natural
   units. Workers need isolated immutable inputs and their own copper/congestion
   state and KiCad temporary directories. Rank results in original deterministic
   candidate order, not completion order; preserve sequential acceptance semantics
   and discard/recompute speculative candidates based on an obsolete incumbent.
   Cap workers and total speculative work/memory. The latest ground trials depended
   on the previous accepted board, so they cannot all be dispatched unchanged.
   Trial-board serialization is nontrivial (`MappingProxyType` metadata); use a
   tested portable snapshot or reconstruct locked inputs in each worker.

5. **Consider native hot kernels after profiling the above.** Compact search-state
   storage and a compiled geometry/neighbor kernel may help if Python call/allocation
   overhead remains dominant. Keep the existing Python implementation as an oracle;
   prove integer-overflow, exact-clearance and deterministic tie-break behavior
   before replacing it. This is higher implementation risk than avoiding reruns.

These are ranked code-informed recommendations, not implemented router changes or
measured speedup claims for the full board. Implementation status and the detailed
sequence are tracked in the [optimization todos](routing-optimization-todo.md).

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

## Work list and acceptance gates

- [x] Default function profiles, raw call graph and bounded readable/JSON summaries.
- [x] Phase/trial elapsed spans with unfinished-phase accounting.
- [x] Provenance, explicit unprofiled mode and truthful routing exit status.
- [x] Real-module failure and routed-artifact byte-identity regression tests.
- [ ] Capture a complete current-board function profile and uninstrumented baseline.
- [x] Extend affected-net transactional repair; compare full-pipeline count on retained fixtures.
- [x] Add read-only, regression/provenance-aware run summaries and comparison gates.
- [ ] Finish profiled current-board routing and repeated uninstrumented O1b comparisons.
- [ ] Optimize confirmed allocation/geometry hot spots with exact-predicate regressions.
- [ ] Evaluate adaptive search changes against expanded-state and routability metrics.
- [ ] Prototype bounded deterministic process-level candidate evaluation; measure
  IPC, peak memory, speculative waste and worker-count scaling.
- [ ] Consider native kernels only if remaining profile evidence warrants them.

Accept an optimization only if connectivity and hard DRC do not regress, package
and critical constraints remain satisfied, repeated results are deterministic, and
uninstrumented measurements show improvement. Changed routing algorithms may
produce different legal geometry; profiling alone must not change artifact bytes.
The latest board still has non-library dangling-copper warnings and signoff work;
performance telemetry does not remove those remaining gates.

## First optimization: bounded local dependencies

O1a is implemented in `escape_feedback.py`. The existing local ground repair first
displaces the exact ordinary-net blockers of a prospective escape. If rerouting
that subset fails, it can now make a temporary probe without other ordinary area
copper, identify further blockers with the exact clearance index, and retry the
expanded net set with all unrelated copper restored. The probe itself is never
committed. Critical copper, fanout and accepted ground contacts remain fixed.
The existing hard-DRC/connectivity comparisons and full-reroute fallback remain.

The default allows two dependency expansions and respects the existing total local
blocker limit. `--zone-dependency-expansions 0` disables just the new expansion,
not the original local repair. The complete workflow exposes the same option:

```powershell
make EXAMPLE=full-vertical route PROFILE=none EXTRA_ROUTE_ARGS="--zone-dependency-expansions 0"
make EXAMPLE=full-vertical route PROFILE=none EXTRA_ROUTE_ARGS="--zone-dependency-expansions 2"
```

Compare on the same inputs and machine load, recording both accepted geometry and
rejected work. Progress adds inclusive `zone_subset_search` spans with `kind`
(`transaction` or `probe_only`), affected nets, expansion count, failed nets and
overflow. Reports identify `local_dependency`, `incremental_placement`, `full_pipeline` or `early_screen`
strategies; accepted local attempts record repaired nets and expansion rounds.
Failed local searches are visible in progress/profile data, not fabricated as
accepted report attempts. Process parallelism and full-board before/after
measurements are still pending.

## Incremental placement repair

O1b adds conservative placement transactions in `incremental_placement.py`.
For a legal move or allowed rotation of a noncritical, noncrowded component,
invalidate its incident ordinary nets. Rebuild affected zone-net contact trees
(there is not yet pad-level ownership for shared ground contacts), retaining
critical return vias. Exact clearance checks discover further ordinary nets
blocked by moved pads, footprint keepouts or the new early contacts. Repair that
bounded dependency cone with unrelated copper restored and critical copper fixed.

Global guides are recomputed from the new clean placement, not relabelled from
old coordinates. Critical/access stages are rebased on their retained actual
copper; obsolete fanout domain statistics and historical placement trials are
cleared. Whole-board land closure, hard DRC and signal-connectivity nonregression
still gate acceptance, which requires fewer pending ground contacts. Independent
KiCad refill/DRC is still performed on final exported geometry; no fill/signoff
evidence is reused across placements.

Moved critical endpoints, rigid macros, crowded package-access owners, missing
ownership evidence, illegal placement, failed fresh global routing and excessive
or unsuccessful repair cones take the original full-pipeline fallback. This first
scope avoids rebuilding unrelated ordinary maze routes, not all global/DRC work.
To compare against full placement trials while retaining local same-placement
repair, run:

```powershell
make EXAMPLE=full-vertical route PROFILE=none EXTRA_ROUTE_ARGS=--no-incremental-placement-repair
make EXAMPLE=full-vertical route PROFILE=none
```

Progress records `zone_incremental_trial`, `zone_moved_global` and fallback reasons
in `zone_incremental_guard`. Report trials record `changed_references`, `repair_nets`,
`rebuilt_zone_nets` and dependency expansions. Regression fixtures exercise real
incident-net routing, unchanged critical geometry, a 45-degree rotation and access
rebasing. The same controlled improving move needs zero full-pipeline evaluations
with incremental repair and one without it. No full-board wall-time speedup has
yet been measured; O1d benchmarking remains open.

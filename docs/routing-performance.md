# Routing performance and optimization assessment

## Check the deadline before each repair alternative (2026-10-23)

The unified router now checks its existing deadline before each unseeded or
seeded repair attempt. Previously the outer round alone checked time: an
unseeded attempt could use the remaining budget and still launch a full escape
seeder afterward. It retains the best completed attempt before reporting the
skipped alternative. This does not interrupt a running search, geometry check
or seeder, and is not a hard wall-time guarantee.

A real-search regression expires a controlled clock as the unseeded repair
ends. Before the fix a third negotiation/seed starts; afterward exactly two
negotiations run, the seeder is never called, the connected fixed net stays
connected and the unresolved net remains explicit. The 32-case unified,
coupled-order and bypass-flow set passes. Ladder233 retains all 35 signals,
zero exact geometry errors, 40 vias and 346.483 mm; 45.9 s versus unchanged
control228's 71.6 s flags no regression, without a speedup claim.

## Native filled-polygon parity index (2026-10-23)

The native slit-bridge decoder indexes nonhorizontal edges by their exact
half-open Y intervals before evaluating polygonized faces. It retains original
oriented edges and the original even/odd crossing arithmetic. No geometry is
healed, rounded, buffered, or cached; representation rejection and linework
checks remain unchanged. The median lower endpoint gives the interval tree a
finite, balanced construction even when coordinates repeat.

`bench/native_fill.py` measures the public decoder, using `pcbir.workbench`
to load native PCB fills. On the identical 15,005-vertex synthetic walk,
controls223 took 7.57 s before and 1.65 s after. On the saved consumer native
37,210-vertex GND/In1.Cu region, controls224/223 took 6.83 s and 3.72 s.
Input hashes, area and normalized WKB hashes match exactly in each comparison.
These are isolated decode timings under concurrent workloads, not a claim
about complete routing or native feedback duration. A one-second sample of
the earlier full consumer verification was inside the former all-edge scan;
repeated complete verification still needs its own measurement.

The 76-case parity, qualification and filled-feedback test set passes. Boundary
tests compare active edge multisets and parity at vertices and neighboring
floating-point values in millimetre and nanometre scales; a 400-hole walk
retains its exact area and every void. Native connectivity, electrical/RF/PDN,
assembly and CAM qualification remain separate.

The matching unified nRF52 ladder227 retains all 35 signal connections, zero
exact geometry errors, 40 vias and 346.483 mm. Its 93.7 s versus control214's
19.7 s triggers the timing warning under concurrent full-board jobs; no whole
route speedup or clean timing gate is claimed. The unchanged active compiler
control228 also takes 71.6 s and flags the same earlier baseline; this timing
comparison remains inconclusive. Run225 omitted the unified
override and is an extra legacy control, not a comparable unified result.

## Coupled groups before equal-priority singles (2026-10-23)

Critical job ordering preserves explicit source priority and places coupled
groups before singles at an equal priority. The old alphabetical kind ordering
put clocks first; a legal single trace could wall off the same package's paired
exit. Coupled lanes require one contiguous joint channel and cannot use the
independent single fallback. The new tie-break is forward ordering, with no
additional portal, search, state, retry or rip-up allowance.

A real crossing fixture connects both a surface-only pair and an equal-priority
clock, using the clock's permitted layer transition. Giving the clock a higher
explicit priority preserves that choice and the blocked pair remains failed.
Both cases preserve all inputs and complete geometry checks. The 62-case
routing/flow set passes. The nRF52 ladder214 retains35/35connections, zero exact
geometry errors,40vias and346.483mm, with no regression.

In full consumer controls198/205, the clock's13-segment surface path runs
between paired package lands and termination supports; both pairs fail before
main search. Replay212 of the actual prepared input, including all344bypass
tracks,108vias and ordinary reservations, connects both pairs and all five
clocks with the new order. TX uses470states and RX3070, each under6000. The
two clock branches tune to23.106233mm each with two legal bumps. Fresh
KiCad10.0.6 refill has zero geometry errors and no selected-net opens;499
unrelated whole-board open items and12dangling-track warnings remain. This
is a prepared-critical subset, not a full ordinary-routing or SI result.

## Exact bypass reservations at the critical boundary (2026-10-23)

The standard flow can hold its complete planned bypass contribution as actual
copper through the existing validated critical-prefix interface. This preserves
net identity, real track/via shapes and holes, permits legal same-net joins, and
locks the complete accepted contribution through critical routing. The previous
all-net rectangular keepouts can forbid such joins and approximate round
geometry conservatively. An unsupported or ambiguous semantic ownership claim
retains that conservative protocol with an explicit reason; no proposed piece
is filtered out to manufacture a passing claim.

Exact source fields, prefix occurrence multiplicity, original admitted return
paths, every declared association and hard geometry are checked afresh. Changed
temporary reservation context can therefore refuse reuse too; that adaptation
is not silently authorized. The existing initial pair-state option now also
bounds the standard critical path, rather than only package-access preflight.
No maze search or retry allowance is added.

Checkpoint-directory builds save first and retry critical inputs. Bypass-only
replay restores each decoded edge to a unique equal original occurrence, then
runs all normal admission checks. Missing/duplicate occurrences, changed
values, context digests and stale source remain rejected. Restoration changes
no routing value and grants no acceptance authority. Workbench views show the
reserved proposal as diagnostic geometry.

Six new real-geometry/flow tests cover a ground join rejected by the old proxy,
foreign-net collisions, complete cross-face supply/return preservation,
serialized replay, ambiguous/stale ownership refusal and the aggregate pair
bound. A 64-case flow/admission/replay set passes. The nRF52 ladder206 retains
35/35 connections, zero exact geometry errors,40vias and346.483mm, with no
regression. Full-consumer205 admits all344planned tracks and108vias as copper;
clock-first ordering still obstructs its paired exits. Removing ordinary
reservations changes an original witness context and correctly retains the
conservative fallback rather than rebinding it without authority. Prepared
replay212, using the changed coupled-first order, needs no such release and
passes selected-native verification. No improved full-board result is claimed.

## Joint separate-support completion (2026-10-23)

Pairs with exactly two shared endpoint packages and one source-bounded separate
support contact per member keep those support contacts as obligations while
forming each escape. A proposed signal collar and its required ground return
are reserved before both local ties are completed against fresh exact geometry.
Changing the return site can therefore change the temporary support topology.
Only the ordinary complete-pair gates may adopt the resulting group; failure
rolls back all generated copper and retains every pending source contact.
Macro-owned, ambiguous, remote and unsupported contact trees keep their existing
preparation. Fixed copper and required via-array multiplicity remain immutable.

Eight distinct surface and eight transition proposals share the former
600-state local allowance per member: 300 states per family/member across all
local calls, at most 1200 total. Each support samples at most eight attachment
points on actual connected collar copper within its original breakout region.
An exact repeated local proposal can reuse geometric work across pitches only
with identical non-copper source fields, tracks/vias including occurrence
multiplicity, and exact caller prefixes. This transaction-local cache holds at
most 16 outcomes; it never certifies whole-pair acceptance or reuses a clearance
index. Every final candidate receives fresh complete validation.

Transition-pair ranking also checks each endpoint's one-pitch straight
continuation against its own complete local proposal. A clear continuation is
a preference inside the existing 128 combinations and search/state allowances,
not an admission gate: a shorter goal tail may reach another legal collar.
Compact attempted-endpoint telemetry reports at most 16 orientations, expanded
states, legal advances and closest approaches, excluding private frontiers and
routing identity.

On one unchanged 389-component consumer, serial preparation produced no PHY
transitions and failed TX. Joint completion produces 23 transitions, but its
first channel ranking still fails at 6000 and 60000 states. Endpoint approach
ranking then connects all six TX contacts in 4032 of 6000 states, with one signal
transition pair and its required ground return. Independent KiCad 10.0.6 refill
has zero geometry errors and no TX opens; unrelated board opens remain. RX also
retains all six contacts. Copper-total skew is about 0.5 mm and is not an
endpoint-delay or SI qualification. No capacitor or source-rule change is needed.

The nRF52 ladder controls188/194 retain 35/35 signal connections, zero exact
geometry errors, 40 vias and 346.483 mm, with no regression. Concurrent runtimes
19.3/18.9 seconds are observations, not a speedup claim. Full-board, native plane
and engineering qualification remain separate from these selected-net results.

## Terminal-transition rejection diagnostics (2026-10-23)

Critical replay now reports finite transition-stage counters and at most 16
rejection examples per terminal/layer/pitch, with two examples per stage and
eight exact blocker identities per example. Local access distinguishes an
exhausted frontier from its separate state limit. These observations do not
change candidate order, state allowances, copper ownership or acceptance.

One consumer's baseline has three short collars: one violates signal
via-in-pad policy and two collide with opposite-face bypass lands. Its local
fallback samples 155 sites, finds 12 legal via pairs and spends 32 local path
calls without a complete collar. A retained capacitor-move trial generates
six signal collars but rejects all 2472 sampled return sites, including sites
blocked by its own disposable termination branches. This identifies serial
support/escape/return preparation as a limitation; it is not a proof that the
physical placement has no valid route. Both diagnostics export byte-identical
PCBs and routing fingerprints to their corresponding pre-telemetry controls.

The 41-case pair/replay set, 51-case local/IPC/telemetry set and 58-case
transition/return set pass (overlapping sets). The nRF52 ladder case180 retains
35/35 signal connections, zero exact geometry errors, 40 vias and 346.483 mm;
38.3 seconds versus the prior 40.1-second control, with no ladder regression.
Native/plane and engineering gates remain open; no speedup claim is made.

The reference IPC reader also accepts an explicit empty-pad separator on a
plated via record. A named manufacturer via is classified as a via, not a
component with an empty pad. Units, drill dimensions and net identities remain
explicit; unsupported records still fail closed.

## Offset collars for staggered plated rows (2026-10-23)

Round plated terminals with a source breakout contract can now propose lateral
collars derived from neighboring land edges. At most two proposals per cardinal
heading are retained, with one opportunity on each lateral side. Lead-ins and
oriented tails use the existing exact source-width and whole-board clearance
checks; main search/state limits and complete-pair acceptance are unchanged.
The change is generic, rotation/mirror independent and has no board selectors.

On one consumer's unchanged revised placement, baseline161 failed both MDI
pairs after ten states each. Candidate160 connected RX within the same 6000-state
bound (3070 states, one complete candidate); TX still failed. Reported RX total
copper skew is zero, but includes termination branches and is not endpoint-delay
qualification. Independent KiCad refill of the RX diagnostic has zero geometry
errors and no RX opens; unrelated board opens remain. Transition diagnostics
then locate TX's missing generated launches at its surface-mounted terminal,
while its plated terminal offers twelve direct opposite-face contacts.

The 122-case routing set passed before the final lateral-slot diversification.
After that change, 58 targeted cases passed; 38 cases including new directional
and transition-inventory assertions also passed (sets overlap). The early nRF52
run157 retains all 35 signal connections, zero exact geometry errors, 40 vias,
346.483 mm and byte-identical PCB SHA256
`9f05741a655c1d478e1d53beeec7d1aaef80c5e75ea8e14ef6a539680b6014d3`.
Its concurrent runtime 16.1 to 67.5 seconds triggers the ladder timing warning;
this is retained without a speed claim. A first control launch167 accidentally
used the original editable install and failed CLI parsing; it is not a routing
comparison. Explicit-PYTHONPATH control168 and candidate169 repeat the same case.
Both retain byte-identical copper and all signal connections, with 75.4 and
40.1 seconds respectively and no ladder regression. External workloads differed;
this is not a whole-board speedup claim.
Native plane and engineering gates remain incomplete in this example subset.

## Direct unified pipeline subset (2026-10-23)

| Ladder case | Flow | Signal opens / connections | Exact DRC errors | Vias | Length mm | Time s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| round-led-ring | retained legacy baseline | 2 / 51 | 0 | 33 | 523.802 | 178.2 |
| round-led-ring | direct unified | 0 / 51 | 0 | 39 | 591.697 | 77.2 |
| nrf52 | unified with bootstrap | 0 / 35 | 0 | 40 | 346.483 | 27.9 |
| nrf52 | direct unified, older export contacts | 0 / 35 | 0 | 37 | 360.121 | 21.8 |
| nrf52 | strict unified only | 0 / 35 | 0 | 35 | 359.121 | 11.7 |
| nrf52 | standard unified with exact neck emission | 0 / 35 | 0 | 40 | 346.483 | 74.5 |

Runs: `build/bench-main-baseline109`, `build/bench-unified-direct115`,
`build/bench-unified-nrf-baseline118`, `build/bench-unified-nrf-direct119`,
`build/bench-unified-only-nrf123`, `build/bench-unified-standard131`.
Native verification was not run; route status remains failed with deferred
plane/evidence gates. The nRF52 timings were concurrent with each other and a
consumer run. This is a measured subset, not full-ladder or release acceptance. The standard
neck-emission run reproduced the standard baseline completion, DRC, via count
and track length. The ladder flags its total wall time (27.9 to 74.5 s); loads
were not controlled. Instrumented unified-stage times were 7.69 and 7.92 s,
and export finished at 22.57 and 22.29 s respectively. The larger
process-level timing difference is retained without a cause or speed claim.


## Fine-grid keepout windows (2026-10-23)

A read-only stack sample of the consumer's standard unified pipeline caught its
critical-clock prepass in `detailed._blocked_nodes`: every fine-grid point
scanned all keepouts on every layer. The new broad phase visits each keepout's
inclusive axis window and retains the exact polygon and board-material
predicates. Source keepouts, layer membership, node coordinates and the result
set are unchanged; no geometry or routing budget is relaxed.

The existing hotspot runner now accepts `--only grid`. On its synthetic
six-layer fixture (120 small reservations, irregular axis coordinates), five
alternating samples gave median 4.280 s before and 0.311 s after (13.78x).
Every blocked-node output matched exactly. This measures grid construction,
not a full-board speedup or qualification. Working results:
`build/grid-hotspot132.json`.

The 64-test detailed/reuse/grid set passed 63 tests and initially rejected a
new invalid test fixture that named a layer outside its board stackup. The
corrected fixture and both existing grid-equivalence checks then passed.
All 32 critical/pipeline tests also passed. A warm same-checkout nRF52 ladder comparison used the exact original
`detailed.py` for the baseline and restored the committed candidate for the
second run. Both completed 35/35 signal connections with zero exact DRC errors,
40 vias and 346.483 mm; the emitted KiCad PCB bytes were identical
(SHA-256 `9f05741a655c1d478e1d53beeec7d1aaef80c5e75ea8e14ef6a539680b6014d3`).
Total times were 45.4 and 44.1 s and the ladder reports no regression.
The earlier cold-copy/concurrent attempt (108.6 s) and its warning remain in
`build/bench-grid133`; the warm baseline/candidate are
`build/bench-grid-baseline135` and `build/bench-grid-candidate136`.
Native verification was not run on these ladder cases. Full-board routing and
native verification remain separate pending checks.


## Isolated hotspot experiments (2026-10-12)

The fresh standard full-vertical profile in
`build/full-vertical/runs/20261012T054251927652Z` took 1,391.918 s with
`cProfile`. It identified four targets: redundant physical-board construction,
macro via-permission geometry, repeated maze-edge work and placement legality.
The pre-change sources used by the experiments match that profile's source
hashes, including its uncommitted routing work.

The repeatable fixture runner is
[`scripts/benchmark_routing_hotspots.py`](../scripts/benchmark_routing_hotspots.py).
Snapshot the working `pcbir` sources before an intervention, retaining the
`backends/kicad_pcb.py` subdirectory, then run:

```powershell
.venv/Scripts/python.exe -m scripts.benchmark_routing_hotspots `
  --baseline-dir build/performance-research-20261012/baseline `
  --output build/performance-research-20261012/isolated-results.json
```

`--only macro|construction|placement|maze` isolates a research area. The runner
loads selected baseline functions against the current unchanged helpers/types,
so each comparison isolates that intervention. It checks exact return values,
alternates method order, warms up once, and records five unprofiled samples,
medians, Python/platform information and source hashes. It also checks 64 maze
equivalence cases with different grids, congestion, guide-layer projection,
movable obstacles, forbidden vias and budget exhaustion. This is a developer
diagnostic, not a timing threshold or manufacturing qualification.

| Target | Methods tried | Selected method and correctness boundary |
| --- | --- | --- |
| Board construction | Whole-board replacement with linear validation; replacement with indexed validation; explicit rendering/track context | Pass the complete committed track prefix into chamfering and a UUID namespace into footprint rendering. No physical board is constructed just to change that context. Real board construction still validates its existing reference and macro invariants. |
| Macro permissions | Linear scan; linear scan with an LRU pad-shape cache; indexed land snapshot, cold and warm | An eight-entry memo retains exact immutable placement/footprint identities and indexes their SMD shapes. Every via still checks current process, binding, permission, exact contact, containment and multiplicity. |
| Maze | Existing resource/guide caches; congestion-only cache; symmetric edge-cost cache; edge costs plus integer keys | Cache only search-local symmetric costs and use grid-derived integer keys for best costs and edges. Arrival-direction bends and directional via guide transitions remain separate. Queue ordering, nodes, traceback and state budgets stay unchanged. |
| Placement | Rebuilt shapes; cached macro copper; cached local land/copper envelopes; those envelopes plus exact posed courtyards | Reject distant candidates using conservative aggregate bounds, then retain the original exact net/layer/copper/drill checks. Geometry caches retain immutable inputs, with 256-entry land/copper and 8,192-entry courtyard limits. Current rules and net ownership are read for every check. |

On this Windows/Python 3.13.1 run, representative median fixture batches were:

| Fixture batch | Baseline | Selected method |
| --- | ---: | ---: |
| 12 warm permission checks, 1,538 lands / two vias | 253.0 ms | 0.304 ms |
| One cold permission check | 20.75 ms | 12.60 ms |
| 30 surface paths with macro-bearing board | 640.0 ms | 0.650 ms |
| 30 library renders, 16-pad footprint | 657.3 ms | 7.386 ms |
| 240 macro-copper placement checks | 159.7 ms | 4.489 ms |
| 240 complete placement legality checks | 40.50 ms | 21.13 ms |
| Orthogonal maze with congestion | 177.7 ms | 143.9 ms |
| Octilinear maze with congestion | 624.2 ms | 413.4 ms |

All compared outputs matched. The large construction and warm-index ratios are
specific to deliberately dense, repeated-validation fixtures; they are not
whole-board speedup estimates. Congestion-only caching barely helped, so the
larger symmetric cost cache was selected. Retained JSON includes every method,
individual sample and exact-output digest. Bounded invalidation, rotated/odd-nm
geometry, permissions, drills, branches and renderer namespace regressions are
covered by the routing/cache/backend tests.
The cold-index case clears the land snapshot before each call; both methods
retain the existing coordinate-transform caches. It is not a cold-process run.

### Matched full-vertical profile

The optimized standard run is
`build/full-vertical/runs/20261012T071852682556Z`. Both runs used Python 3.13.1,
KiCad 10.0.6, the same locked/offline inputs, six-layer fabrication profile,
both immutable macros, `candidate-00`, 20,000 search states, two detailed passes
and the standard bounded pipeline. Router commands and all recorded input hashes
match. Source hashes did not change during either run.

| Observed cProfile run / phase | Before | After |
| --- | ---: | ---: |
| Full build elapsed | 1,391.918 s | 724.974 s |
| Placement/global planning | 312.686 s | 166.158 s |
| Initial package plane contacts | 129.998 s | 1.525 s |
| Detailed pass 1 | 308.930 s | 167.251 s |
| Detailed pass 2 | 316.699 s | 220.029 s |
| Ordinary area phase | 643.494 s | 398.753 s |
| Export phase | 5.761 s | 0.638 s |
| Independent KiCad phase | 33.543 s | 40.886 s |

The observed full profiled run used **47.9% less elapsed time (1.92x ratio)**.
This compares two instrumented runs, not repeated unprofiled whole-board runs.
Profiler overhead and external machine load affect the ratio. Function/phase
times below overlap and must not be summed.

| Hotspot, cumulative cProfile time | Before | After | Work/output check |
| --- | ---: | ---: | --- |
| Physical-board reference validation | 280.788 s | 11.67 s | Constructors reduced from 1,863 to 192; real constructors still validate. |
| Macro via permissions | 134.271 s | 0.18 s | 324 fresh permission calls; three cold land snapshots. |
| Surface paths | 176.955 s | 1.36 s | Same 1,356 calls. |
| Canonical footprint rendering | 82.437 s | 0.51 s | Same 476 renders. |
| Placement legality | 167.737 s | 62.75 s | Same 213,465 probes. |
| Placement versus macro copper | 64.970 s | 1.68 s | Same 58,653 checks. |
| Maze search | 603.149 s | 368.56 s | Same 393 searches and 1,170,524 neighbor-generation calls. |
| All plane stitching | 179.091 s | 2.86 s | Same two calls. |

Total profiled calls fell from 2.265 billion to 1.282 billion. The initial
critical tier still expanded 27 states; package exits remained 76 accepted / 0
pending. Both runs made 100 detailed net attempts, with the same two first-pass
failures and all 50 ordinary nets connected after the second pass. No additional
repair searches were needed.

The draft board SHA-256 and editor-intent SHA-256 match exactly. Saved native
boards differ only in **110 UUIDs for KiCad-generated empty Datasheet/Description
properties**; every other line, including routed and filled copper, is identical.
Both have 680 tracks, 223 vias, 1,981.243638 mm of track, and fresh native DRC with
zero violations / zero unconnected items. Native fill resolves the same three
deferred explicit-copper observations. The same eight engineering requirements
remain blocked; fabrication readiness is false.

Regression coverage: 2,673 passed / 18 skipped in the initial full-suite run,
plus 13 environment failures. All affected tests passed in a 107-test rerun
with localhost access and a temporary directory outside the repository. That
rerun also covers one regression added after initial collection, giving 2,687
distinct passing tests / 18 skipped / no unresolved failures. Localhost was
sandbox-blocked, and a repository-local temp directory changed the module root
seen by the import-escape test. Optional skipped capabilities remain unverified.

The optimized router process measured 110.6 MiB peak Windows working set and
457.1 MiB peak pagefile usage, including cProfile and excluding child KiCad
processes. A small exit hook captured these counters; its hash and scope are
retained in `peak-memory.json`. The baseline has no peak-memory measurement.

Retained research evidence is in `build/performance-research-20261012/`:
`isolated-results.json`, baseline source snapshots/hashes,
`full-profile-comparison.json`, `test-results.json` and the original test logs.
Each full run retains raw `routing.prof`, summaries, phase events, routing/native
reports and provenance. The remaining dominant consumer is maze search:
neighbor generation and grid/material queries account for much of its 368.6 s;
physical DRC is now 97.0 s and placement legality 62.8 s, with overlap.

Exact profiled sources are retained in
`build/performance-research-20261012/profiled-sources/`, verified against every
launch-time source hash. Subsequent shared-checkout edits to KiCad clearance-rule
export arrived after the profile finished and are preserved. These timing,
regression and native-verification results apply to the retained profiled
revision; they do not verify those later export changes. See
`post-profile-edits.json` for the recorded boundary.

## Branch comparison runs

`.github/workflows/routing-comparison.yml` runs on pushes to `claude/plan-*`
branches (and on manual dispatch). It routes full-vertical twice on matched
GitHub runners, unprofiled and with the release route job's settings: once at
the commit named by `baseline_ref` in `.github/routing-comparison.json` and
once at the pushed commit. Each run directory is kept as an artifact; the
compare job writes `python -m pcbir.routing_benchmark table` to the run
summary and keeps `summarize`/`compare` JSON. A plan step sets
`baseline_ref` to the commit it builds on.

Expanded states, outcomes and copper are deterministic and compare exactly.
Wall times on shared runners vary between jobs, so a timing claim needs the
same direction on repeated runs. Results are measurements, never signoff or
an automatic acceptance gate. The 2026-10-12 profile above used KiCad 10.0.6
on Windows; branch comparisons use KiCad 10.0.7 on Ubuntu and are not
comparable with it.

### Phase 1 baseline (2026-10-20)

Run 39459011927 routed full-vertical at 15f94b4 (KiCad 10.0.7 pin plus the
terminal-capacity fix) and at the search-telemetry commit on matched Ubuntu
runners. Global, critical and detailed fingerprints were identical, so the
telemetry does not change copper. Both runs: global success, package access
ready, 0 failed signal nets, 2 zone nets awaiting fill, 366 tracks, 46 vias,
1,572.318 mm of track, 0 KiCad unconnected items and 4 other independent
findings (2 silkscreen clipped by mask, 1 silkscreen clearance, 1 dangling
track).

| Area search work (telemetry run) | Value |
| --- | ---: |
| Executed `detailed_net` attempts | 113 |
| A* calls | 453 |
| Calls that found a path | 132 |
| Calls with no path | 245 |
| Calls out of state budget | 76 |
| States expanded | 1,412,850 |
| States expanded by calls that found nothing | 943,086 (67 %) |
| Search seconds | 135.7 |

Elapsed times were 370.3 s and 267.9 s for identical copper, so runner
variance alone exceeds 30 %. Phase 1 changes are judged on expanded states
and on routed outcome; timing claims need repeated matched runs.

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

### Placement and global progress

Normal `route-board --progress` forwards the existing placement seed stages
through `on_progress`, using zero-based seed indices and the configured
seed count. A bounded legalization failure is reported as a failed seed stage;
the final `placement_candidates` event reports the total, not another seed.
The existing refinement stage includes decoupling work and does not expose its
internal steps.

Global routing emits sparse graph/access setup and iteration start/finish
events, inside candidate, feedback-trial and final-certification boundaries.
Iterations report existing attempt overflow/unrouted counts and best-score
improvement; final route status describes global guides. These operational
events are not exact-copper closure or engineering qualification. They add no
heartbeat, inner-search events, solver work or timing code. Interrupted work
has no invented finish event. With no callback, results, ordering and budgets
are unchanged; existing raw placement/editor callbacks remain compatible.

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


## Global capacity-graph static calculations

Each global graph owns an immutable snapshot of its axes, legal nodes,
resources/capacities, terminal alias edges, via sites and layer preferences.
Construction copies caller-owned maps. Terminal registration and via-capacity
refinement create replacement snapshots; all derived data starts empty on the
replacement. Private derived fields do not change the constructor, equality,
representation, route reports or fingerprints. The graph is internal and is
not a serialized public routing artifact.

Cartesian Points are constructed once per graph, with the original tuple-index
semantics. Lazy outgoing-step entries retain exact original neighbor order,
resource identity, direction and planar/via plus layer/direction preference
cost. Their key includes the node and all three static cost options. At most
`len(legal_nodes)` entries are retained across every cost variant; when full,
new variants compute normally. Point storage is bounded by the finite Cartesian
axes, and step storage by that entry cap and the graph's finite degree.

Every search freshly filters allowed layers and blocked resources and reads
current demand, usage, congestion history, present/history penalties, bends and
remaining via allowance. Heap keys, deterministic serials, comparisons, state
space, ordering and iteration/search limits are unchanged. No path, failure,
mutable congestion answer or acceptance decision is memoized.

The differential suite retains the original A* function and checks exact paths,
heap events, complete route JSON/fingerprints, changed congestion/cost options,
layer/via constraints, terminal aliases and refined/replaced graphs. Separate
synthetic uninstrumented timings can measure recomputation savings; they do
not establish a source-board speedup, electrical/routing closure or native
qualification. Cache container sizes are not process peak-memory measurements.


## Immutable pad and breakout-owner templates

Clearance indexes share only immutable original pad geometry and source-land
membership. An eight-entry template cache retains strong identity keys for all
placements, named footprints, nets, stack, board rules, routing rules and bin
size. A separate 512-entry cache retains transformed footprint geometry. New
source epochs rebuild template ownership, including effects of a moved other
land. The pure nearest-region predicate stops at the first exact zero squared
distance, preserving the original nearest-distance and first-tie result.

Each index owns fresh object, hole and bin containers. Current breakout query
caches, zones, keepouts, macro reservations, NPTHs, tracks, vias and mutable
additions remain local and are checked under current policies. Neither a
clearance answer nor an accepted routing witness is cached. Original order,
unconnected lands and plated drill geometry remain intact.

The constructor-only comparison used actual 159-component geometry with 615
copper pads, four copper-only snapshots per batch and five alternating pairs.
It compared the original constructor AND original breakout implementation with
the candidate, checking exact index state, pad owners and clearance queries.
The measured medians were 1.005159 s and 0.034161 s. This repeated-source fixture
measures construction reuse; concurrent load was recorded, and it is not a
whole-board speedup or a routing, native, RF, power or manufacturing result.


## Ordinary breakout capacity

Global search, target selection, negotiated resource use and final overflow
metrics use the same source-declared ordinary breakout width for an edge whose
entire centreline is inside one shared terminal-land breakout region. An edge
crossing that boundary retains its normal full-width demand. A narrower width
on another net, or separate regions containing only the two endpoints, does
not grant a reduced demand. Declared clearance remains unchanged; specialized
profiles keep their existing capacity policy. Distinct physical via-site
multiplicity is unaffected.

This avoids a false capacity overflow or needless detour when a source already
allows an ordinary neck-down beside a fine-pitch package. It does not emit a
trace, relax any source width, bypass detailed/native checks, or certify a
whole-board route. Only immutable source-region membership is cached within
the net's demand evaluator, with a 16,384-edge bound; mutable usage and history
are read on every search step. Nets without an effective width reduction keep
the existing fast path. Regressions include renamed fixtures, boundary-crossing
edges, declared clearance and unchanged physical-copper rules.


## Lazy breakout measurements and bypass baseline DRC

Copper-spacing checks first test the normal, unrelaxed spacing before
measuring a rational distance for a breakout-relaxation record. Pairs that
already meet normal spacing cannot produce such a record. Near pairs retain
the same exact predicates, measured values, ordering and report fields,
including half-nanometre width thresholds.

Cross-layer bypass proposals defer their immutable input's full DRC until a
complete supply/return witness passes exact geometry checks. An unsuccessful
search adopts no copper and needs no baseline comparison. Every potentially
accepted proposal still compares fresh full candidate DRC with fresh full
input DRC; no report cache or stale-board evidence is introduced. The
baseline is evaluated at most once inside that immutable transaction.

On an 892-track/79-via CopperProbe diagnostic snapshot, the complete DRC JSON
was byte-identical (77 findings and 347 breakout records). One paired timing
measured 5.36 s before and 3.35 s after the spacing optimization. This is a
snapshot measurement, not a whole-route speed guarantee. Existing running
jobs remain pinned to their original compiler.


## Compact fresh bypass launches

Fresh bypass launches now compare both existing bounded geometric sampling
tiers before spending the finite launch slots. Their score is new launch
Manhattan length plus remaining distance to the destination land, followed
by new launch length and deterministic coordinates. Existing contained
source tracks add no new length to that score. This is a local ranking
heuristic, not a shortest-path or electrical loop-quality proof.

The ranked list is interleaved with the original outward candidate inventory,
deduplicating exact choices before spending a launch slot. Compact choices keep
the first opportunity, while outward alternatives remain represented when
shorter launches cannot reach the destination surface. Neither ordering receives
an extra launch allowance.

Exact verified existing launches still consume their original slots first.
Each radial/two-leg sampling pass retains the site cap, and launch,
complete-proposal, shared local-path and inner-bridge allowances are unchanged.
The fine tier is considered even when coarse rays already fill the slots;
this may do more geometric sampling within the established maximum bounds.
All complete-path, source-prefix, whole-net budget and fresh DRC gates remain.


### Breakout membership across copper-only trial indexes

Breakout-region membership now shares a bounded memo across clearance indexes
with the same complete immutable source epoch: placement tuple, exact footprint
objects, net assignments, board rules, routing rules and stackup. The cache keeps
four source epochs with at most 8192 queries each. Strong identity keys prevent
identity reuse. A changed source field starts a new epoch; added/removed copper
still receives fresh spatial indexing and exact clearance checks. No board,
generated copper, fill, clearance result or routing acceptance is cached.

On one 414-track/69-via consumer control, eight repeated index builds and region
queries took 1.65 s before and 0.15 s after. The membership result digest and
complete physical DRC report were identical. This isolates repeated source
membership work, not complete-route performance or any engineering acceptance.

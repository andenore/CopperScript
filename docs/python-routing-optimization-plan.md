# Python-first routing optimization

Continue optimization in Python before considering native extensions or another
language. The parser, IR, CLI and editor remain unchanged. Compiled alternatives
are deferred until remaining Python opportunities have been measured and used.

## Evidence and constraints

The retained staged preflight profile contains about 700 million calls, including
31 million point/segment distances, 32 million `Fraction` constructions and
31 million rational comparisons. Native copper-spacing DRC accounts for about
257 inclusive instrumented seconds; that time overlaps the geometry functions.
Profiles identify work, not uninstrumented speed ratios.

Preserve integer-nanometre and exact rational semantics, inclusive tangency,
full via spans, pad-overlap policy, deterministic selection and diagnostics.
Do not reduce budgets, relax clearance, replace exact arithmetic with floating
point, or claim faster failures as success. Retain the rational distance APIs as
an oracle and for measured violation distances. Python integers remain unbounded.

An already-running process retains its loaded implementation. Its pinned Git
revision identifies that code; later working-copy changes are not evidence that
the running board was produced by the new implementation.

The full-board baseline has now finished at `7d7673f`: retained under
`build/full-vertical-staged-indexed-20261005/`, profile saved, build exit 1,
`status=unmet_gates`. Ordinary failures are `GNSS_TX`, `I2C_SDA` and `V3V3`;
independent KiCad reports 34 unconnected items and one other violation. This is
a diagnostic draft, not a complete or fabrication-ready board.

Its profile contains 9.81 billion calls, 64.46 million Fraction constructions,
61.77 million point/segment distance calls, and 56 copper-spacing DRC calls
(457.64 inclusive instrumented seconds). `_grid_query_context` is called 47.97
million times. Dictionary lookups, generated hashes and polygon containment
also dominate. Generated hash entries aggregate multiple dataclass types;
attribute them before choosing a cached-hash or packed-key implementation.

## Implementation sequence

1. [x] Fraction-free exact clearance predicates. Compare squared cross products
   and integer products instead of constructing/reducing fractions. Short-circuit
   the first failing edge, retain containment and zero-distance behavior, and
   avoid temporary bounding objects in hot Boolean checks.
2. [x] Native DRC fast rejection. Use exact Boolean checks on passing pairs and
   compute the existing rational distance only for actual findings. Preserve
   odd-width half-nanometre thresholds, finding order, codes and measurements.
3. [x] Validate each increment against the rational oracle: seeded shapes,
   rotated/degenerate geometry, tangency/one-nanometre gaps, very large integers,
   ordinary and failed routing, drills, physical DRC and installed KiCad.
4. [x] Measure allocation/work reduction and alternating uninstrumented fixture
   timings. Keep actual work/result comparisons separate from concurrent-load
   timing observations. Commit verified increments.
5. [ ] Continue the completed full-board profile's remaining hot spots:

   - [x] Reuse immutable placed track-keepout shapes per board snapshot and grid
     identity tuples per grid. Hold inputs against ID reuse. Replaced boards,
     axes and blocked-node sets get independent keys even with shared caches.
   - [x] Construct each physical query ray once; skip allocating ignored endpoint
     nodes/sets in its blocked-node scan. Still check every interior on-ray node.
   - [ ] Measure repeated coordinate/placed-pad/port construction and bounded reuse.
   - [ ] Attribute hash/key work and evaluate tighter internal representations
     without changing heap tie-breaking, costs, budgets or target eligibility.
   - [x] Inspect repeated repair/search work against exact snapshot identities.
     Exact repeats are 16 of 201 full-vertical detailed attempts, all in repair;
     they and repeated blocked-node meshes are reused within one repair stage,
     and blocking tests material once per point. See
     [exact repair-search reuse](repair-search-reuse.md).

6. [ ] Measure full-board closure and matched uninstrumented timing before claiming
   an end-to-end gain. Continue the shared optimization checklist, including
   bounded Python process workers only for independent trials and with memory
   limits. No native backend is introduced by this plan.

## Acceptance

Boolean answers must match the retained distance oracle; DRC findings and selected
route geometry must stay identical. Tests must distinguish reduced Fraction work
from reduced geometric work. Faster fixture timings are not board completion,
manufacturing signoff or a full-board speedup estimate.

## Measurements and retained tests

CPython 3.13.1, Windows; reference code loaded read-only from local Git commit
`7d7673f`. Seven alternating unprofiled reference/optimized repetitions; output
equality checked on every run. Work counts come from separate cProfile runs,
not from instrumented timings. The routing regression suite was active during
measurement, so timings are observations under concurrent load.

| Fixture | Reference median | Optimized median | Fraction constructions |
| --- | ---: | ---: | ---: |
| 12,000 mixed point/segment/diamond shape checks (seed 581) | 140.78 ms | 43.16 ms | 59,945 -> 0 |
| Copper spacing of 160 separated passing tracks | 44.22 ms | 4.90 ms | 32,000 -> 0 |

These are approximately 3.26x and 9.03x kernel speed ratios, **not** full-board
speed ratios. The passing-track fixture emphasizes rejection, not crowded-board
violations. Failed pairs still use the retained rational measurement API.

With the immutable-grid cache increment, seven alternating batches of five
routes gave medians of 36.89 -> 33.25 ms on the open fixture and
297.76 -> 271.95 ms on the locked-obstacle fixture. Shape constructions fell
from 1,218 to 777 and from 5,751 to 2,521 respectively. Those measurements
precede the final interior-node-only scan optimization. Small-route timings
vary with load; the earlier single-route comparison did not establish an
overall improvement. No full-board gain is claimed.

`test_geometry_predicates.py` retains 4,000 segment/capsule oracle cases,
2,500 shape pairs in both orders, one-nanometre tangency, half-nanometre
thresholds, degenerate/concave shapes, unbounded coordinates and construction
guards. Open, detoured and blocked routes must retain identical geometry,
route JSON and DRC JSON. `test_drc_lazy_distances.py` compares complete reports
against eager rational decisions and forbids measurements on passing pairs.
`test_grid_snapshots.py` checks replaced-board/axis/block invalidation, counts
100 rays plus one reused region (formerly 300 shapes), forbids adjacent-ray
endpoint allocation and compares 1,200 split-axis/layer/blocked-node rays with
the uncached original predicate. Existing escape, via-span, hole, native KiCad
and incremental-repair suites remain required.

Validation completed: 505 distinct focused tests passed, with one Linux-only
Makefile test skipped on Windows. The final-code broad run passed 430 tests;
GNU Make integration was then run explicitly with the installed Scoop binary,
the expensive unavoidable-locked-route case passed separately, and 65 additional
hard-macro/KiCad-backend/manufacturing tests passed. No test timing is used as a
full-board speed claim. `compileall` and `git diff --check` also passed.

Next gates: an unprofiled matched full-board run, no worse named failures/DRC,
then a new diagnostic profile before further algorithm or representation work.
No worker-count, routing-budget, fabrication-rule or native-backend change is
part of this increment.

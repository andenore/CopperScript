# Early pattern negotiation and indexed immutable geometry

## Evidence and scope

The profiled 2026-10-05 full-vertical run spent 1,883.553 seconds and 720,000
paired-search states on its first modem USB pattern, without an accepted route.
The revalidated alternate pattern needed 67 states and 22.712 seconds for that
pair. The earlier matching preflight profile recorded about 148 million calls
to `geometry.bounds`; keepout checking consumed substantial cumulative time.
These are instrumented measurements, not promised uninstrumented speedups.

Implement two generic improvements. Do not modify board placement, pin mapping,
clearance, layer policy, pair coupling, via limits or production gates. Do not
change the currently running process. Parallel workers and guided pair-search
corridors remain separate follow-ups.

Primary architectural references: [TritonRoute's staged access/search/repair
flow](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html) and
[spatial-index broad phases](https://www.boost.org/doc/libs/latest/libs/geometry/doc/html/geometry/spatial_indexes/introduction.html).
The budget scheduler below is our proposal based on measured owner conflicts,
not a claim to reproduce a published PCB solver.

## S1 — Bounded initial search and early negotiation

- [x] Add an optional aggregate expanded-state limit per differential pair,
  shared across surface/via searches, port candidates and all existing pitches.
  Keep the standalone router's historical full-search defaults.
- [x] At package preflight, use an initial 6,000-state limit when paired nets
  and pattern negotiation are enabled. Divide work into bounded candidate
  slices so one candidate cannot silently exceed the aggregate limit.
- [x] Apply the same initial limit to clean-owner probes and alternate-pattern
  revalidation. A ready result still requires actual connectivity, unchanged
  critical profiles, every required access identity and fresh native geometry.
- [x] If limited search/negotiation is not ready, retry the original ordinary
  pattern with historical full budgets and the existing negotiation controller.
  Preserve identity-safe limited improvements if full fallback does not improve
  them. A smaller budget alone must never be a claimed speed improvement.
- [x] Make tiers, limits, expanded states, budget exhaustion, fallback and
  selection explicit in reports/progress. Pattern limits apply per tier; full
  fallback may retry up to the same bounded number of proposals. No unbounded
  search or repeated same-budget loop. Zero initial limit disables staging;
  zero pattern-trial budget retains historical full-budget behavior.
- [x] Test exact aggregate caps, no discarded candidate copper, easy success
  without fallback, early alternate success, success only at full budget,
  unavoidable failure, rollback/identity preservation and deterministic replay.

## S2 — Cached shape bounds and indexed static obstacles

- [x] Compute rounded-shape bounds once per immutable shape. Exclude the cache
  from equality/hash/repr; `replace`, copy and pickle must remain correct.
  Do not change exact integer/Fraction narrow-phase predicates.
- [x] Index keepouts and macro access reservations by layer and coarse bins,
  using the existing clearance-index scale. Deduplicate queried identities and
  retain deterministic original order. Limit bin expansion for huge regions
  and queries, with a conservative per-layer fallback.
- [x] Include the macro's one-nanometre exclusion margin in broad-phase queries.
  Preserve track/via flags, full via spans, rotated/mirrored footprint keepouts,
  mechanical checks and the existing globally fail-closed unsupported-hole policy.
- [x] Keep caches per shape/index, never globally keyed only by XY coordinates.
  New placement/rule/outline snapshots construct fresh indexes; copper insertion
  continues to update the separate mutable copper index. No stale legal-result cache.
- [x] Compare indexed answers against the original linear oracle across exact
  tangencies, negative coordinates, bin boundaries, layers/flags, large shapes,
  unsupported holes, macros, rotated geometry and copper insertion.
- [x] Record deterministic bound/predicate work reduction and profiled fixture
  measurements. Run routing, via, mechanical, native/KiCad and build regressions.

## Integration and acceptance

- [x] Document the knobs and add decisions to the design specification.
- [x] Run a profiled matched-placement preflight on current pinned sources;
  compare required identities, pair profiles, hard findings and selected paths.
- [x] Commit each verified major increment. Preserve unrelated user artifacts.
- [ ] Run the full area/fill/DRC workflow on the final implementation separately.
  No complete-board, manufacturing-ready or wall-time-ratio claim until its
  exact saved output passes the relevant gates and matched timing protocol.

## Recorded validation — 2026-10-05

The profiled working-copy preflight at saved placement
`5d1b87bb622bd415def17b3c54bd23db266dee9b4a9a08d8c74632bc9551163e`
passed the initial tier after one accepted `critical_first` proposal: 76 ordinary
exits/boundary paths, 44 requested dense-package plane contacts, six critical
groups (four RF and two USB pairs), no pending identities and zero native hard
findings. Three actual critical passes consumed 6,397 total pair-search states,
including the discarded initial pattern and clean-owner probe; no full-budget
fallback was required. The earlier full-budget log at the same placement
recorded 728,263 states over the corresponding passes.

All selected critical geometry/profile measurements match the previous accepted
owner preflight: connectivity, lengths, skew, coupled/uncoupled lengths,
transitions, return-via counts and track/via counts. All **329 saved segment/via
occurrences** also match after removing UUIDs; no manual placement/routing change
was used. Artifacts/profile/logs are ignored under
`build/package-pattern-validation/staged-indexed-20261005/`.

The retained geometry fixture checks 1,000 repeated bound reads with one bound
calculation, 2,400 seeded indexed-versus-linear flag/layer queries, exact
tangencies and one-nanometre macro contact, huge-envelope fallback, hole policy,
rotated/mirrored keepouts and mutable copper insertion. A local query reduces
1,000 linear bounding-box checks to fewer than 10, with the same exact answer.
Its function profile is retained as `geometry-fixture.prof` in that directory.

Final affected routing/geometry/native-KiCad/CLI/build regressions: **337 passed,
10 skipped** (GNU Make unavailable). A further 74 incremental-repair,
profiling and comparison tests passed, and four new benchmark tests check
staged-search comparison settings and total discarded-search work. The comparison
helper permits the staging knob as an explicit intervention but still rejects
changed area-search budgets, missing quality evidence and instrumented timing
ratios. Pattern progress events identify their tier explicitly.

These are deterministic work-count and instrumented preflight results, **not**
an uninstrumented wall-time ratio, complete area routing or manufacturing
signoff. Other routing/tests were running concurrently. Final micro-adjustments
after this profile avoid retaining discarded board snapshots for counters and
skip empty/unrelated-layer static bins; regression tests cover those separately.
The older full-area run does not contain these optimizations. Final full-board
closure and sequential uninstrumented comparison remain unchecked above.

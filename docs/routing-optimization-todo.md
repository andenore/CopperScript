# Routing optimization implementation work list

Implement in this order. Correctness gates are shared across every item: preserve
critical ownership and exact clearance/via-span checks, reject connectivity/DRC
regressions, keep deterministic selection, and retain a conservative fallback.
See [the assessment](routing-performance.md) for evidence, limitations and sources.

## O0 — Reproducible measurements

- [x] Enable full-run function profiles, phase timings, provenance and exit-status checks.
- [x] Add read-only run summaries and comparison gates that reject new named-net,
  pad, hard-DRC, coverage and independent-verification failures. Require matching
  recorded inputs/tools/settings and two uninstrumented runs for a timing ratio.
  `tests/test_routing_benchmark.py` covers missing/stale data, mixed profiling,
  dropped or exchanged failed nets, swapped finding categories and legacy logs.
- [ ] Capture a current full-board profile and an uninstrumented comparison run.
- [ ] Record expanded states, affected-net count, local/probe searches, full-pipeline
  evaluations and peak memory; distinguish selected work from rejected trials.
- [ ] Retain representative dense-package, power-tree and repair fixtures so
  optimization work does not depend on a 66-minute board run alone.

## O1 — Reduce whole-board reroutes (first implementation)

- [x] O1a: Extend existing same-placement local rip-up with a bounded dependency
  cone. If subset routing fails, route a temporary probe without unrelated ordinary
  area copper, use exact clearance queries to identify further blockers, then
  retry the enlarged subset against all preserved copper. Never commit probe copper.
  Cap net count/expansion rounds; reject locked blockers and fallback on failure.
- [x] O1a tests: secondary blockers, no-progress/cap failures, rollback, locked
  copper, immutable fanout, all-layer via blockers and deterministic repeated results.
- [x] O1a telemetry: expose subset/probe work and accepted affected nets in progress
  and reports; add an explicit zero-expansion comparison mode.
  `tests/test_local_dependencies.py` uses the real detailed router and native DRC
  on a retained corridor fixture: A-only repair fails; the exact probe discovers
  B; A+B repair succeeds after one expansion while C and the fixed reservation
  survive unchanged. Further controller tests check inner-layer via blockers and
  noncommitting probes. This proves bounded repair behavior, not full-board speedup.
- [x] O1b: Extend the transaction to legal small placement moves/rotations. Invalidate
  incident ordinary nets and detect moved-pad/keepout or early-contact collisions;
  preserve unrelated copper and recompute global guides/fingerprints. Rebase access
  evidence, clearing obsolete domain statistics rather than treating them as fresh.
  Rebuild contacts on affected zone nets, retaining critical return vias. Initially
  fallback for moved critical endpoints, rigid clusters, crowded package-access
  owners, unsupported geometry changes or excessive cones. Package exits belonging
  to a moved crowded owner are not reused: that candidate takes the full pipeline.
- [x] O1b tests: real incident-net reroute, unchanged critical copper, deterministic
  replay, explicit 45-degree rotation, refreshed stage evidence, collision/ownership
  and dependency-limit rollback. A controlled real-router comparison performs zero
  full-pipeline evaluations with incremental repair enabled and one when disabled.
  `tests/test_incremental_placement.py` and `tests/test_escape_feedback.py` retain
  these fixtures. This is a work-count measurement, not a full-board timing claim.
- [x] O1b telemetry/comparison: report changed references and rebuilt zone nets;
  `--no-incremental-placement-repair` retains full-pipeline placement trials.
- [ ] Extend incremental ownership to moved crowded packages/rigid macros only after
  explicit pad-level exit ownership and joint access revalidation are available.
- [ ] O1c: Reserve failing package ground groups jointly with required access and
  retain already-accepted contacts. Evaluate whether this avoids late feedback.
- [ ] O1d: Benchmark successful and failed repair cases, then the full board. Require
  fewer full evaluations or lower uninstrumented time without worse closure/DRC.
  The read-only comparison harness is implemented. Current-board profile launched
  from routing commit `39a44d7` on 2026-10-02; completion and uninstrumented paired
  measurements must be recorded before this benchmark item can be checked off.

## O2 — Reduce geometry/allocation overhead

Current measured work: [early negotiation and indexed geometry plan](routing-search-speed-plan.md).

- [x] Use full profiles to select significant self-time/call-count hot paths.
- [x] Cache immutable rounded-shape bounds; spatially index static keepouts/macro
  regions separately from mutable copper. Fresh board snapshots build fresh
  indexes; huge envelopes fall back conservatively. Linear-oracle tests retain
  exact tangency, layer/flag, hole-policy and transformed-geometry semantics.
- [x] Negotiate package patterns after bounded initial pair-search slices,
  retaining original-pattern full-budget fallback and strict owner gates.
  The profiled matched-placement preflight passes with unchanged selected
  copper and 6,397 actual states across all passes versus 728,263 in the earlier
  full-budget log. This is a work-count result, not a full-route timing ratio.
- [ ] Extend caching beyond bounds to coordinate/placed-pad/port construction where profiles justify it.
- [ ] Reuse placed-pad/keepout snapshots across search indexes separately from mutable copper;
  define placement, clearance and copper-revision invalidation keys before reuse.
- [ ] Tune spatial-bin sizes on realistic footprints, retaining exact narrow-phase checks.
- [ ] Benchmark each change and run tangency, rotated-pad, drill and full-span-via tests.

## O3 — Reduce maze states

- [ ] Record guide expansions, budget exhaustion, no-path failures and target-set sizes.
- [ ] Add cheap feasible-port/layer reachability screening before costly repairs.
- [ ] Evaluate tighter adaptive/coarse-to-fine corridors with wide/neutral fallback.
- [ ] Index large tree-target sets only if profiles justify the added machinery.
- [ ] Compare expanded states, routability, quality and uninstrumented time; do not
  count lowered search budgets or newly failed routes as performance improvements.

## O4 — Process-level candidate parallelism

- [ ] Define portable immutable worker inputs (existing metadata uses mapping proxies).
- [ ] Implement bounded independent placement/global candidate workers with isolated
  routing state and temporary paths; begin with 1/2 workers and measure memory/IPC.
- [ ] Select in deterministic original candidate order, not completion order.
- [ ] Add interruption/error cleanup and reject stale-incumbent speculative results.
- [ ] Compare 1/2/4 workers before enabling a default. Do not parallelize dependent
  ground trials or per-net shared-copper searches without conflict reconciliation.

## O5 — Native kernels, only if still justified

- [ ] Re-profile after O1–O4; select a narrowly bounded remaining hot kernel.
- [ ] Retain the Python oracle and prove integer-overflow/exact-predicate behavior.
- [ ] Verify deterministic tie-breaking and cross-platform deployment before adoption.

Completed implementation entries must name tests and measurements. A benchmark or
prototype is not full-board signoff; remaining board-production gates stay open.

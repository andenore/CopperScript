# Routing optimization backlog

Use [performance guidance](routing-performance.md) for commands and acceptance.
Require current profiles and exact-output/quality comparisons before optimization.

- [ ] Capture a matched uninstrumented comparison run.

- [ ] Record affected-net count, full-pipeline evaluations and peak memory. Expanded
  states, including rejected and probe searches, are now in `search_work`; per-net
  `search_stats` covers only the selected attempt.

- [ ] Retain representative dense-package, power-tree and repair fixtures so
  optimization work does not depend on a single large consumer board.

- [ ] Extend incremental ownership to moved crowded packages/rigid macros only after
  explicit pad-level exit ownership and joint access revalidation are available.

- [ ] O1c: Reserve failing package ground groups jointly with required access and
  retain already-accepted contacts. Evaluate whether this avoids late feedback.

- [ ] O1d: Benchmark successful and failed repair cases, then the full board. Require
  fewer full evaluations or lower uninstrumented time without worse closure/DRC.
  Use repeated uninstrumented matched-input runs; the comparison harness is implemented.

- [ ] Extend caching beyond bounds to coordinate/placed-pad/port construction where profiles justify it.

- [ ] Reuse placed-pad/keepout snapshots across search indexes separately from mutable copper;
  define placement, clearance and copper-revision invalidation keys before reuse.

- [ ] Tune spatial-bin sizes on realistic footprints, retaining exact narrow-phase checks.

- [ ] Benchmark each change and run tangency, rotated-pad, drill and full-span-via tests.

- [x] Record guide expansions, budget exhaustion, no-path failures and target-set sizes
  (`DetailedSearchStats`; see [search policy](routing-search-policy.md#search-work-counters)).

- [ ] Add cheap feasible-port/layer reachability screening before costly repairs.

- [ ] Evaluate tighter adaptive/coarse-to-fine corridors with wide/neutral fallback.

- [ ] Index large tree-target sets only if profiles justify the added machinery.

- [ ] Compare expanded states, routability, quality and uninstrumented time; do not
  count lowered search budgets or newly failed routes as performance improvements.

- [ ] Define portable immutable worker inputs (existing metadata uses mapping proxies).

- [ ] Implement bounded independent placement/global candidate workers with isolated
  routing state and temporary paths; begin with 1/2 workers and measure memory/IPC.

- [ ] Select in deterministic original candidate order, not completion order.

- [ ] Add interruption/error cleanup and reject stale-incumbent speculative results.

- [ ] Compare 1/2/4 workers before enabling a default. Do not parallelize dependent
  ground trials or per-net shared-copper searches without conflict reconciliation.

- [ ] Re-profile after O1–O4; select a narrowly bounded remaining hot kernel.

- [ ] Retain the Python oracle and prove integer-overflow/exact-predicate behavior.

- [ ] Verify deterministic tie-breaking and cross-platform deployment before adoption.

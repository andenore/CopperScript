# Routing completion research tasks

These are separate, testable tasks for the full-vertical prototype. The common
acceptance gate is exact native DRC followed by KiCad refill-aware DRC; a higher
connected-net count never excuses a new manufacturing violation. The provisional
100 x 80 mm, four-layer board is the benchmark, not evidence of producibility.

## R1. Component pin fanout before general routing

**Question.** Which dense SMD pads need an escape trace and/or via, and which
candidate escapes can coexist under actual width, spacing, via, hole, and layer
rules?

**Recommendation.** Add a deterministic component-level fanout stage between
placement/global guidance and ordinary detailed routing. Rank constrained pads
first; enumerate short outward/axis-aligned and staggered via candidates; check
each complete candidate against exact geometry; commit only a conflict-free set.
Return explicit failed-pad diagnostics. Keep fanout copper separate from the
electrical IR and pass it as locked copper into subsequent routing. Do not place
via-in-pad without an explicitly qualified process.

**Acceptance.** Unit tests cover pad orientation, neighboring pads, via legality,
order independence, rollback, and failure diagnostics. Benchmark against the
STM32 and Nordic pad fields and confirm exported geometry with KiCad DRC.

**Primary references.** [Altium fanout control](https://www.altium.com/documentation/altium-designer/pcb/design-rule-types/routing),
[Freerouting's staged pipeline](https://github.com/freerouting/freerouting/blob/master/docs/architecture.md).

## R2. Detailed-routing failures fed back into placement

**Question.** When global routing predicts success but exact routing fails,
which movable components and orientations relieve the specific blocked escapes
without breaking mechanical, proximity, RF, or electrical constraints?

**Recommendation.** Keep the current global congestion feedback, then add a
bounded outer loop: route candidate placements, collect failed pad references
and local obstruction pressure, propose only legal small moves/rotations of
those components and nearby movable partners, rerun fanout plus detailed
routing, and accept a candidate only on a lexicographic completion/DRC score.
Cache and cap trials; never move components under already accepted copper.

**Acceptance.** A synthetic placement where the global guide succeeds but
detailed pin access fails is repaired; fixed parts remain fixed; a failed trial
leaves the original board unchanged. Report selected candidate and trial score.

**Primary references.** [Altium autorouting placement guidance](https://www.altium.com/documentation/altium-designer/pcb/routing/situs-topological-autorouter),
[Altium routing/placement feedback](https://www.altium.com/documentation/altium-designer/pcb/routing).

## R3. Multi-net transactional rip-up and shove

**Question.** Which already routed connections block a failed net, and can they
be displaced and rerouted as a legal group rather than limiting repair to two
blockers?

**Recommendation.** Build a geometric blocker set from a candidate route,
protect locked critical/fanout copper, remove a bounded group of ordinary
routes transactionally, insert the candidate, and reroute the displaced nets
with varied order/costs. Commit only if all displaced nets reconnect and exact
DRC passes; otherwise restore the original solution. Add local trace/via shove
as another candidate generator, not an unchecked mutation.

**Acceptance.** Tests cover 3+ blockers, locked-copper refusal, complete
rollback, preserved connectivity, and DRC. Benchmark completion and runtime
against the 46/58 baseline.

**Primary references.** [Freerouting autorouter/optimizer stages](https://github.com/freerouting/freerouting/blob/master/docs/architecture.md),
[KiCad shove and walk-around modes](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html).

## R4. Authoritative plane connectivity

**Question.** After KiCad fills the declared plane, which pads and vias are
electrically connected to the same-net zone, and are any copper islands or
unconnected items left?

**Recommendation.** Keep provisional pad stitching separate. Export a
same-stem PCB/project, run a pinned KiCad zone refill and JSON DRC, parse the
unconnected-item and island findings, and bind the result to hashes of the
exact board, project, tool version, and report. A zone outline or pad escape is
not connectivity evidence. Never mark the route complete or release Gerbers
while any pad, via, or island remains unresolved.

**Acceptance.** Tests cover connected and disconnected plane fixtures,
island findings, stale evidence, and KiCad process failure. The full-vertical
board has zero refill-aware KiCad unconnected items and no unwaived violations
before manufacturing export.

**Primary reference.** [KiCad PCB editor zone filling and DRC](https://docs.kicad.org/master/en/pcbnew/pcbnew.pdf).

## Prototype implementation and limits

- R1: `pcbir.fanout.route_fanout` now creates DRC-checked, non-via-in-pad
  escapes for crowded ordinary SMD pads. The stage is opt-in (`--fanout`),
  returns explicit pending pads, and supplies via anchors to the detailed
  router. It is greedy and does not yet solve component-wide escape assignment.
- R2: `detailed_failure_trials` proposes bounded legal moves for components
  attached to failed detailed nets. The pipeline reruns global, critical,
  fanout, detailed routing, and native DRC, committing only an improved score.
  It is opt-in (`--detailed-feedback-trials`) because full reroutes are costly.
  The first prototype uses cardinal moves, not rotations or analytic gradients.
- R3: repair now accepts a configurable 1–8 ordinary-net blocker group and
  tries bounded deterministic reroute orders. Locked critical/fanout copper is
  never evicted. Each order builds a fresh clearance index, so failed partial
  reroutes roll back. `--soft-ripup` enables candidate paths across removable
  copper; `--maximum-ripup-blockers` bounds displacement. Standalone shove is
  still not integrated as a topology-preserving candidate generator.
- R4: `pcbir.plane_verify.verify_filled_planes` exports a disposable PCB and
  same-stem project, runs a version-qualified KiCad refill plus JSON DRC,
  counts open connections and isolated islands, and binds evidence to source,
  export, filled-board, and report digests. `--verify-plane-fill` emits this
  evidence in the routing report. Any open connection or violation fails the
  gate; Gerbers are not generated. This is independent KiCad evidence, not a
  normalized polygon-fill implementation in the physical IR.

The full-vertical board remains a draft until the measured route has zero
unrouted nets and passes native plus refill-aware KiCad DRC. An installed
algorithm is not itself evidence that this acceptance condition was met.

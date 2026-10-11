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

## R5. Automatic fragmented-fill repair and routing-guide coordination

**Question.** When native refill splits one source zone into disconnected copper
fragments, how should the shared pipeline identify affected terminals, choose
legal repair actions and obtain routing guides for a normally deferred zone net?

**Reproduction.** `tests/test_autonomous_routing_quality.py` has two renamed
real-KiCad cases: a fill-only keepout divides a single pour even though both pads
have prospective surface contact. Native opens correctly prevent completion.
A candidate from the shared exact-clearance surface search joins them and passes
fresh refill without changing zone/keepout intent. Merely setting detailed
`defer_zone_nets=False` is insufficient: global routing still defers the net,
leaving no connected guide. This is captured behavior, not an automatic repair.

**Investigation checklist.** Generic implementation belongs in CopperScript;
reusable electrical/layout contracts belong in CopperLib, not board scripts.

- [x] Reproduce disconnected fill with real native evidence and a legal bridge
  control, including renamed pads/nets and stale-evidence rejection.
- [x] Retain structured native finding/object identities and map opens to
  physical pads, layers and filled fragments; preserve exact artifact hashes.
- [x] Investigate explicit, bounded zone-net guide generation or guide-free
  local bridging, preserving authored distribution reservations and widths.
- [x] Choose bounded small mutable blockers/actions from geometry, never net-name
  lists; protect macros, required via arrays, critical copper and successful exits.
- [x] Revalidate ordinary/critical/boundary access and displaced-net continuity;
  commit only after fresh native refill proves improvement without new violations.
- [x] Roll back rejected trials, cap work, detect repeated states and report
  exhaustion/tool limitations separately from impossible or inadequate zone intent.
- [x] Integrate into ordinary CLI feedback and validate consumer runs without
  project-specific routing fixes, preserving unresolved failures.
- [ ] Demonstrate equivalent shared tracker closure before retiring historical
  project repair scripts. LED shared closure is already verified.

**Acceptance.** A standard bounded pipeline repairs the fragmented-fill fixture
without targeted pad/net selectors; constrained/unrepairable variants remain
truthfully unresolved with their original copper intact. Stale evidence cannot
approve a candidate. Native routing closure is not CAM/RF/power or release approval.

See [autonomous routing plan](autonomous-routing-plan.md), especially its filled-
copper feedback and CLI integration milestones. Initial evidence: 14 new quality
cases passed; the wider focused run was 210 passed, one optional skip, with both
new fragmented-fill cases executed using installed KiCad.

2026-10-11 implementation: [bounded native feedback](filled-copper-feedback.md)
now repairs both renamed fragmented-fill fixtures through the standard CLI,
including mutable ordinary dependencies and immutable macro/array rollback.
Actual filled polygon membership handles source zones whose disconnected
fragments share one UUID. Retained evidence binds native version and exact
input/output/report hashes. The bounded surface path is intentionally guide-free;
it does not authorize source-zone edits or claim globally minimal displacement.
CopperLedRing's shared balanced run has zero native opens/violations; the initial
tracker run remains blocked and does not justify deleting its historical helpers.

## R6. Narrow package-owner coordination

2026-10-11 consumer evidence: ordinary-first keeps every ordinary exit but blocks
one constrained RF connection and one selected plane contact. A clean critical
probe connects the RF feed within its original limit; critical-first and
critical/plane-first proposals then lose other ordinary escape identities.
The shared gates correctly reject those candidates. This is not evidence that
the fixed source design is impossible, nor authority to change its rules.

- [x] Record the all-owner ordering conflict and preserve failed candidates as
  diagnostic evidence; do not waive lost ordinary access to force area routing.
- [x] Reduce a partial-owner negotiation fixture, including renamed owners and
  a rollback case with immutable macros/required arrays.
- [ ] Investigate retaining unaffected ordinary escapes during the critical
  probe, releasing only geometry-derived conflicting generated ownership.
  Rebase retained and rebuilt ownership with occurrence-counted geometry.
- [x] Use the existing disposable probe to propose a partial ordinary rebuild:
  keep nonintersecting escape contributions, release intersecting nets only,
  then rebuild all critical/contact/boundary owners against the combined result.
- [x] Feed measured lost ordinary identities from rejected ordering trials into
  legal placement hints, even when the original failed endpoints are fixed.
  Hints never install rejected copper or unlock source/session constraints.
- [x] Bound partial proposal/dependency rebuilding explicitly; preserve the same
  acceptance gates, successful identities and fresh all-owner verification.
- [x] Demonstrate equivalent source-matched tracker closure through the standard
  shared CLI before removing historical selected-net repair helpers.

Placement-hint verification passed 65 package/access ranking tests, including
fixed-owner guards and failed-proposal rollback. A smaller partial-ordinary
proposal passed focused verification: reuse the existing disposable critical
probe, retain all nonintersecting generated ordinary ownership exactly, and
rebuild only intersecting ordinary nets. Final critical, plane and boundary
owners are rebuilt against the combined contribution. This does not yet retain
ordinary prefixes during the *initial* critical probe; that remains research.
Twenty-three partial-owner cases cover real scarce-neighbor fallback, renamed
identities, protected macro/required-array success and rollback, strict source
length limits, release/trial budgets and CLI controls. The new two-case scarce-
neighbor fixture runs without patched routing/candidate/connected flags. The
dispatch budget test separately injects first-search exhaustion; it is not used
as proof of geometry feasibility. Fresh full-board Tracker validation is pending.
Actual feed geometry is now protected separately from unrelated escapes on the
same net. Cheap partial eligibility rejection retains the established second
critical-only ordering, within the existing pattern allowance. Four renamed
clearance cases exercise same-net release and actual-feed rejection.

The Tracker contact maps to `V3V3`, not GND. Its supply decoupler is
`C_LEVEL_A`; both translator pairs remained distant in the failed review.
Read-only source/native pose diagnostics found legal local placements with the
existing full-link optimizer. A reduced planner reproduction found stale phase
locks after rigid legalization; its correction and fresh consumer routing are
being verified. No component, net, source pose or macro geometry was edited.

## R7. Budget-exhausted ordinary final retries

The post-placement Tracker benchmark now clears all package owners, but ordinary
area searches still hit their computational limits. Repeating the same bounded
search policy is not a proof that a route is impossible.

- [x] Reduce a real four-layer surface-wall case: four renamed/reversed variants
  exhaust the normal search while a weighted search finds legal copper at the
  same 200-state-per-search limit. No candidate/search/connected flags are mocked.
  The coarse guide is deliberately surface-biased; exact searches see the wall.
- [x] Record actual failed search-budget exceptions separately from diagnostic
  strings, missing guides and completed routes. Preserve both baseline passes.
- [x] Diversify only the existing final strict retry after actual exhaustion,
  to at least 200 percent heuristic weight. Retain a larger requested weight,
  existing state/pitch bounds, source width/length/layer rules and ownership.
  No additional search attempt or net/component selector is introduced.
- [x] Verify affected routing, report, flow, native and ownership regressions:
  94 detailed/report/benchmark tests, 177 pipeline/ownership/native tests and
  28 retry/checkpoint/reuse/native-CLI tests passed. Overlapping cases are not
  counted as distinct: a collection audit confirms 285 unique affected cases.
  Replay preserves geometry and reports with reuse off.
- [x] Validate a fresh source-matched Tracker run before retiring historical
  helpers; previous balanced runs predate this retry policy.

Weighted search can choose a longer legal path; it does not promise an optimal
route or authorize violating authored length limits. `search_budget_exhausted`
identifies an incomplete attempt stopped at its allowance, and
`retry_heuristic_weight_percent` discloses an automatically changed final retry.

2026-10-11 closure: fresh balanced and thorough Tracker profiles each connect
60/60 ordinary nets, with actual KiCad 10.0.6 refill reporting zero opens,
violations and islands. The source/lock hashes are unchanged. The new shared
native reader checks actual lands, macro copper, polygons and required arrays;
it never reinserts missing copper. The frozen solver suite passed 2,436 tests
with 21 skips; the reader was added afterwards and tested separately.

## R8. Endpoint-only surface-chain readback

- [x] Record the Tracker C_STATUS_B false negative: a legitimate T junction
  contacts the interior of a straight track rather than its endpoint. The
  existing exact copper-contact graph, with all vias/fills/internal component
  joins excluded, confirms top-copper connectivity for all 14 associations.
- [ ] Reduce renamed/rotated real-geometry interior-junction cases and decide
  whether the path extractor should node tracks or use exact contact witnesses.
  Preserve width, clearance, layer, ownership and via-free checks. Do not invent
  a path length from mere electrical connectivity or add consumer-specific code.

## Prototype implementation and limits (historical R1–R4 checkpoint)

- R1: `pcbir.fanout.route_fanout` now creates DRC-checked, non-via-in-pad
  escapes for crowded ordinary SMD pads. The stage is opt-in (`--fanout`),
  returns explicit pending pads, and supplies via anchors to the detailed
  router. It is greedy and does not yet solve component-wide escape assignment.
- R2: `detailed_failure_trials` proposes bounded legal moves for components
  attached to failed detailed nets. The pipeline reruns global, critical,
  fanout, detailed routing, and native DRC, committing only an improved score.
  It is opt-in (`--detailed-feedback-trials`) because full reroutes are costly.
  The first prototype uses cardinal moves, not rotations or analytic gradients.
  A synthetic pin trap now verifies that the global guide can succeed, exact
  detailed access can fail, and a legal placement retry closes the route while
  a fixed component remains fixed.
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

## Prototype benchmark and independent findings

An eight-pass, 0.5 mm-grid, 10,000-state full-vertical run with fanout and
four-blocker soft repair connected **50/58 nets**, versus the prior **46/58**
baseline. Native DRC reported the eight open nets and incomplete routing,
without shorts or copper-clearance findings. The open ordinary nets were
`GNSS_TX`, `MCU_RESET`, `MCU_SWDCLK`, `MCU_SWDIO`, `NRF_SWDCLK`,
`USB_DP_MODEM`, and `V3V3`; `GND` was deliberately deferred pending a verified
plane fill. This did not produce a manufacturable board.

KiCad 10.0.6 refill-aware DRC of that first draft found 121 unconnected items,
45 dangling fanout vias, six drill-spacing warnings, three isolated copper
findings, and eight library-footprint findings. The dangling-via and same-net
drill-spacing findings led to the post-route pruning and shared drill checks
in CS-089/CS-090. These fixes have targeted regression tests, including a
real KiCad connected/disconnected plane fixture. They do not close the seven
ordinary failed nets or verify the full board's GND plane.

A bounded post-fix regression (two passes, 2,000 search states) connected only
22/58 nets, as expected from its much smaller search budget. Its refill-aware
KiCad DRC found **zero dangling vias and zero drill-spacing violations**;
the eight remaining rule findings were six footprint-library mismatches and
two footprint-library issues. It still had 172 unconnected items.

The corrected eight-pass full rerun connected **51/58 nets**. Native DRC
reported seven open nets and incomplete routing, with no copper-geometry
findings. The remaining nets are `CAN_RX`, `MCU_SWDCLK`, `MCU_SWDIO`,
`NRF_SWDCLK`, `USB_C_CC2`, `V3V3`, and the intentionally deferred `GND`.
KiCad 10.0.6 refill-aware DRC found 125 unconnected items and 13 rule
findings: five isolated copper islands plus the same six library-footprint
mismatches and two library-footprint issues. There were no dangling-via,
drill-spacing, short, or copper-clearance findings. The change from 50 to
51 routed nets is a measured improvement, not signoff; the different route
also increased KiCad's unconnected-item count from 121 to 125.

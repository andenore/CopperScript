# Power-domain planning and terminal-access closure

## Evidence and scope

The unprofiled full-vertical run at `f850ba0` retained 48/49 ordinary nets.
The eight-pad, 0.65 mm pitch level shifter was excluded by the twelve-pad
fanout cutoff; nearby completed routes then blocked its 3V3 access. The last
ground contact was blocked by a regulator-feedback route. Finer ground-via
sampling did not solve it, but removing that route did. These are access and
ownership problems, not evidence that a larger global search budget is needed.
Local diagnostic successes are not proof that displaced routes can be restored.

This work keeps electrical IR coordinate-free. Physical domain membership is a
derived supply-net view, not a second connectivity definition. Power planning
is a soft objective, subordinate to locks, mechanical legality, proximity rules
and package escape spacing. No clearance reduction, implicit via-in-pad,
ground-plane splitting, firmware guarantee or power-integrity certification.

## Implementation sequence

- [x] 1. Replace the fanout footprint-count exclusion with per-pad geometric
  eligibility for small multi-terminal SMD packages. Continue excluding simple
  two-terminal passives, critical-owner pins, macro-owned pins and plane pads.
  Preserve bounded candidate sampling, assignment, ownership and exact DRC.
- [x] 2. Default package-access preflight to reserve declared plane contacts
  before ordinary area routing. Use the existing critical/plane/ordinary
  pattern negotiation and full revalidation, not an unconditional GND-first
  lock. Expose an explicit opt-out and preserve selected-pad overrides.
- [x] 3. Keep successful branches of failed multi-terminal attempts as private
  checkpoints. Resume compatible soft-repair proposals from those branches;
  commit only a complete route after restoring every displaced net. Checkpoint
  copper must never enter the output board or congestion map on failure.
- [x] 4. Lower supply-net membership and source-pad identity into a typed
  physical planning view. Treat nets, not voltage labels, as domain identities;
  switched/filtered same-voltage nets remain distinct. Components may belong
  to several domains, including local decouplers and passive rail terminals.
- [x] 5. Add an adjustable soft domain compactness/distribution objective to
  analytical and discrete placement. Multi-domain parts participate in each
  group; orientation and actual supply-pad positions matter. Do not force
  every multi-supply IC to a geometric boundary. Report the objective and allow
  disabling it for comparisons. Plane-backed nets use reduced generic
  wirelength attraction rather than pulling all their consumers together.
- [x] 6. Extend `copper_zone` with an explicit polygon or named placement-region
  boundary and priority. Keep existing whole-outline zones unchanged. Validate
  geometry, net/layer references and ambiguous parameters; native refill and
  connected-copper signoff remain authoritative. Do not automatically invent
  a layer assignment or pour from a component bounding box.
- [x] 7. Document source syntax/options and verify focused regressions, complete
  existing tests, and unprofiled full-vertical physical/domain/access smoke
  checks. Record observed limitations without claiming production closure.

## Algorithm choices

Fanout precedes area routing. Critical pairs retain specialized routing and
ground/power contacts participate in the same negotiated package-access gate.
Branch checkpoints retain a legal tentative tree only as repair input; local
blocker removal stays transactional, with the safe incumbent kept on failure.
These follow the fanout/per-connection/local-repair sequencing documented by
[Freerouting](https://github.com/freerouting/freerouting/blob/master/docs/architecture.md#routing-algorithm).

Domain placement uses normalized supply-terminal distance to each domain's
sources (or centroid when no physical source exists). Normalization prevents a
large shared rail overwhelming signal relationships. Source-to-consumer
distribution and overlapping membership are heuristics, not DC drop models.
Local bypass/proximity and escape requirements outrank grouping. Manufacturer
guidance prioritizes functional blocks, short supply paths, close decoupling,
and deliberate power/reference layer planning:
[Analog Devices](https://www.analog.com/en/resources/analog-dialogue/articles/what-are-the-basic-guidelines-for-layout-design-of-mixed-signal-pcbs.html).

## Follow-up outside this increment

Automatic multi-rail polygon partitioning needs joint layer/corridor planning,
minimum neck/current checks, fill feedback and reference-plane analysis. It is
not equivalent to filling each domain's bounding box. Likewise, retaining a
Pareto archive of whole-board feedback candidates and connection-level ripping
of already-complete blocker trees are subsequent improvements; this increment
resumes the failing net's successful branches and preserves existing blocker
restoration and strict acceptance gates. The existing six-layer full-vertical
stackup must not silently lose a signal layer to a new full-board power pour.

## Validation — 2026-10-06

Current-source focused runs passed: 58 new-feature/CLI tests, 90 existing
detailed-routing/pipeline/package-access/feedback tests, and 64
placement/template/region/checkpoint tests (these batches overlap; do not add
them). Two additional opt-out/ground-owner-preservation cases passed, including
a late feedback trial which retains another package's accepted ground contact.
Checkpoint tests prove a three-terminal repair searches only the remaining
branch, binds source/mesh/rules/anchors, rejects locked obstacles, does not
retain large search-query caches, and never publishes a failed partial tree.
Python compilation and the normal repository `git diff --check` passed.

The full-suite run completed with 1,537 passes and 21 skips, but is **not green**:
two CAM failures come from the incomplete local PyGerber installation (its
module file and distribution version are both unavailable); two CM4 fixture
setup errors come from comparing an unqualified profile footprint path with a
qualified package URI. The CM4 error reproduces using committed `physicalize`
code without modifying the checkout. That suite also captured an outdated CLI
fanout-field assertion before it was corrected; the current complete CLI batch
passes. Do not present this as an entirely passing repository test suite.

Unprofiled, locked/offline full-vertical planning and package preflight used the
unchanged source, pinned CopperLib data, actual KiCad 10.0 footprints,
six-layer JLCPCB profile and rigid placement templates. The first complete
smoke took 162.43 s wall-clock time; this is not a router speedup benchmark.
Placement has zero represented relative-constraint penalty, zero coarse
congestion overflow and zero estimated escape-channel deficit. Its raw HPWL
is 1,983.75 mm, weighted wirelength 1,868.90 mm and soft domain penalty 36.94 mm.
Identified six rail views; source-free rails use the documented centroid
fallback. No source constraints, clearances, via-in-pad permissions or layer
assignments were changed.

Package preflight reserves 104 ordinary exits (previous run: 76), including
both `U_MODEM_LEVEL.1` and `.7`, and establishes prospective contacts for all
80 requested GND pads, including `PWR/R_MODEM_EN_PD.2`. All four RF groups and
both USB pairs connect, with no pending access and no hard internal finding.
The initial modem USB conflict is resolved by cross-owner pattern negotiation,
not a component-specific override or relaxed production rule. A repeated run
produced the same ready preflight and reservation identities.

Ignored outputs are under `build/power-domain-access-20261006/`. The repeated
run's complete generated project is in `native-preflight/`; its
`access-smoke.json` explicitly records `ordinary_area_routing_run: false` and
`fabrication_ready: false`. Independent KiCad 10.0.6 refill/DRC of that exact
project finds **zero GND opens and no clearance/short violations**. It still
reports 95 ordinary-net opens and 104 `via_dangling` warnings: 103 ordinary
fanout vias and one GND return via. These are unfinished preflight findings,
not waived for a completed board: the native result remains a failure and no
manufacturing release exists.

The shared full-vertical build has subsequently completed with `--profile none`.
See [the full rerun and layer review](routing-domain-access-review.md): all 49
ordinary nets and eight critical nets connect, and independent KiCad refill has
zero unconnected items. Completion/signoff still fails on one dangling GND
return via and one reported detailed-resource conflict. Neither closure finding
is waived and no manufacturing output is released.

### Next closure work

- [x] Complete the shared unprofiled full routing run and independently refill,
  check and plot every copper layer. Preserve raw failed-gate evidence.
- [x] Bind detailed resource accounting to final emitted copper after stub
  pruning and corner chamfering. Add a regression in which a legal later route
  occupies a removed corner; retain live resource conflicts and exact DRC.
  Applies per tentative net, before installing occupancy for the next search;
  the complete board still needs a rerun after the correction.
- [ ] Resolve the unconnected-surface USB return via generically. Check actual
  reference-layer/contact requirements and preserve explicit source requirements;
  do not delete a required via, invent copper solely to silence DRC, or waive the
  warning. A shared reference-plane case needs an explicit modeled policy.
  Implementation/verification is tracked in the
  [shared-reference return plan](shared-reference-return-plan.md).
- [ ] Repeat the complete flow after those fixes, requiring zero native opens
  and violations and no stale/real resource overflow before manufacturing export.

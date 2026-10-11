# Autonomous placement and routing

Objective: the standard CopperScript pipeline routes and verifies a board, or
reports actionable unresolved failures, without board-local repair code or
hard-coded component/net selectors. CopperLib owns reusable electrical/layout
contracts; consuming projects own design intent, mechanics and operating inputs.

## Checklist

### 1. Baseline and reproductions

- [x] Audit the current pipeline and tracker workflow: early plane reservations
  already exist, while the review workflow disables them and freezes all poses.
- [x] Add a small real-geometry blocked-plane-contact regression, including
  renamed nets/components and rotated variants, without tracker dependencies.
- [x] Cover impossible access, rejected trials and deterministic replay. Keep
  provisional plane contact distinct from filled connectivity.
- [x] Add protected-macro / required-via-array variants to the new scarce-contact
  fixture, beyond the existing separate ownership regressions.
- [x] Capture additional generic cases: fragmented fills, redundant escapes,
  decoupling access and same-net crossing/escape quality.

### 2. Access planning before area routing (first implementation milestone)

- [x] Enable the existing package-access planner in the standard `route-board`
  command by default; retain `--fanout` compatibility and an explicit
  `--no-fanout` diagnostic opt-out. The Python no-fanout API is unchanged.
- [x] Extend existing package-pattern negotiation to establish scarce plane
  access before conflicting ordinary exits when needed, then revalidate all
  ordinary, critical and boundary access. No blanket power-first rule.
- [x] Verify accepted contact geometry survives area routing in the small fixture;
  run the existing boundary, ownership and cleanup guard regressions.
- [x] Verify existing shared reports/progress expose automatic pattern decisions
  and pending contacts; make the default CLI exercise that reporting path.
- [x] Verify the standard pipeline solves the small regression without net/pad
  repair flags; run package/access/plane/flow/routing regressions.

### 3. Placement feedback

- [x] Rank legal placements by actual escape/contact feasibility as well as
  congestion and wire length; feed scarce/blocked access into placement trials.
- [x] Move/rotate eligible placement units and rigid macro groups transactionally;
  respect fixed intent and verify regenerated owner geometry at accepted poses.
- [x] Explain specific fixed companions, regions, orientations and other rules
  preventing a bounded placement solution, beyond the current stop reason.

### 4. Automatic bounded routing repair

- [x] Record blocker identities/reasons from failed connection searches.
- [x] Select small geometry-derived eligible mutable blocker sets, not board-specific net names.
- [x] Trial contact reservation and displaced-net rerouting with exact ownership,
  preserved successful access and no new hard violations.
- [x] Roll back failures; bound search, record work, detect repeated states and
  report stagnation/exhaustion without claiming impossibility.

### 5. Filled-copper feedback

Investigation detail: [R5 fragmented-fill repair and guide coordination](routing-research-tasks.md#r5-automatic-fragmented-fill-repair-and-routing-guide-coordination).

- [x] Integrate the existing generic native refill/DRC adapter into the repair
  feedback loop, retaining version/input/output hashes,
  recoverable candidates and explicit missing-tool / unsupported-result status.
- [x] Map native filled connectivity failures back to physical pads and regions.
- [x] Feed geometry failures into bounded repair. Never silently change electrical
  topology, minimum widths, copper weights, keepouts or authored macro geometry.
- [x] Report source-zone review requirements separately from a routing failure.

### 6. Standard CLI and board-project migration

- [x] Integrate placement, access, route, refill, repair and final verification in
  the ordinary CLI, using bounded profiles instead of project-local orchestration.
- [x] Publish deterministic reports, inputs and small generic regression fixtures.
- [x] Validate CopperAssetTracker and CopperLedRing with no hard-coded repair
  selectors; preserve historical boards and locked production inputs.
- [x] Replace/retire consumer repair scripts only after equivalent shared behavior
  is demonstrated. Ordinary build wrappers and design intent may remain.

## Acceptance and safety

Each box is checked only after implementation and recorded verification. Native
fill evidence is required for zone-net connectivity; a prospective contact is
not a connected plane. A successful routing result is not production, RF, power
or battery qualification. Failure never licenses weaker clearance/width rules,
unapproved via-in-pad or changes to fixed placements and immutable macros.

## Work log

### Current checkpoint (2026-10-11, Tracker closure and script retirement)

All six implementation/migration milestones are now checked. Fresh standard
balanced and thorough Tracker profiles each close 60/60 ordinary nets, with
actual KiCad 10.0.6 refill reporting zero opens, other violations and islands.
The two distribution nets are fill-verified, not assumed connected from source
outlines. CopperLedRing's latest balanced run likewise closes all 31 ordinary
nets and has zero native opens, violations and islands. No board source, source
rules, macro assets or release locks were changed for these solver benchmarks.

The frozen solver snapshot passed 2,436 tests with 21 skips. The new shared
`pcbir.native_checkpoint` was added afterwards: 40 real-geometry reader cases
pass, and its preceding 36-case version passed with 113 existing ownership,
array, branch and cleanup cases (149 passed, one optional skip). Counts overlap;
the reader is not retroactively included in the frozen full-suite result.
Tracker's remaining consumer suite passes 51 tests with the explicit CopperLib
review root and no skips. Tests tied to removed mutation helpers were retired;
their read-only land/pose safety checks now live in shared real-geometry tests.

Removed seven Tracker routing/probe/migration entry points after equivalent
shared closure. Rendering and modem-power checks now use a composition-only
wrapper delegating native pose/land/actual-copper validation to CopperScript.
The reader counts observed owner tracks, vias and polygons; unlike the old
reader, it never reinserts absent native copper. Both exported and filled
Tracker PCBs pass its guard without byte changes. Every explicit decoupling
association has top-copper connectivity with vias, fills and internal joins
excluded; endpoint-only chain extraction's T-junction false negative remains
[R8 research](routing-research-tasks.md#r8-endpoint-only-surface-chain-readback).

Retained evidence is in the chat workspace's `autonomous-routing-20261011/`;
Tracker documents exact input/output hashes in `docs/autonomous-routing-review.md`.
Historical boards/reports, read-only tools and locked production inputs remain.
This completes the autonomous-routing implementation plan, not the separate
production revision or CAM, electrical, assembly and prototype approval gates.

### Detailed history

- Initial audit: `flow.py` / `package_access.py` already have alternate-order
  package negotiation and placement feedback. The tracker review wrapper uses
  `reserve_plane_contacts=False`, fixed references and no placement trials.
  Start by reproducing a remaining generic access failure and extending those
  existing mechanisms, not by adding another board-specific repair controller.
- First real-geometry reproduction: a narrow inner-zone contact window conflicts
  with a normal signal escape. Existing package-pattern negotiation already
  solves it without named repair selectors. Eight renamed/cardinal-rotation
  variants and deterministic replay exercise real clearance and boundary checks,
  with no mocked candidate generator or connected flag. The standard pipeline
  also preserves the resulting rail contact while routing the ordinary signal.
- CLI policy gap: package preflight was opt-in even though early plane contacts
  were enabled by default. `route-board` now enables the complete package-access
  stage by default. This activates existing negotiation rather than claiming a
  newly invented solver. Full-board completion is not inferred from this fixture.
- Further audit: `escape_feedback.py` already selects exact mutable blockers,
  reroutes displaced ordinary nets and supports placement dependency repair.
  `plane_verify.py` already supplies generic native fill evidence. Later milestones
  must extend/integrate those implementations, especially ownership of conflicting
  fanout and feedback from filled-zone fragments, rather than duplicate them.
- First-increment verification: 85 tests passed across autonomous-contact, CLI,
  package-access and physical-flow suites; 187 across plane, boundary, ownership,
  cleanup, small-package and routing-quality suites; 37 across zone-feedback,
  incremental-placement and routing-review suites. These are 309 focused tests,
  not a claim of full-suite success or complete tracker routing. The finalized
  default CLI export test and 11 new cases were additionally rerun together.
- Ownership increment: five additional real-geometry cases exercise scarce
  contact negotiation with immutable macro tracks/vias, with and without a
  required four-via exposed-land array, including renamed signal/supply nets.
  Successful trials preserve owner copper, exact array multiplicity/finish and
  fixed poses while the ordinary signal connects. A legal macro occupying the
  sole contact window makes trials fail without evicting either owner; area
  routing stays stopped and unresolved contact evidence is retained. These are
  synthetic unqualified fixtures, not reusable component/regulator designs.
  Existing shared algorithms pass; no production solver change was needed for
  these variants, and no filled-plane connectivity is inferred.
- Ownership-increment verification: 289 passed and one optional test skipped
  across autonomous plane access, hard macros, pad via arrays, package access,
  physical flow, plane stitching, boundary access/routing, escape feedback,
  route/bend cleanup and CLI tests (290 collected). This is focused regression
  coverage, not full-suite or full-board routing/qualification evidence.
- Quality-reproduction increment: 14 new integration cases use synthetic lands,
  real clearance/search and the standard pipeline, without tracker imports or
  mocked routing. Eight renamed/cardinal-rotation cases verify straight initial
  exits, no same-net X crossings and removal of genuinely redundant launch vias
  after surface connectivity; two blocked-area cases preserve reserved exits.
  Two semantic-bypass cases relocate an unfixed capacitor using only its typed
  pin association, preserve the front-layer feed through routing/cleanup and
  permit real ground-return vias. No universal electrical distance is invented.
- Fragmented-fill reproduction: two renamed variants run actual native refill.
  A fill-only keepout splits one source pour although both lands have prospective
  surface contact. Native opens correctly prevent reconciliation/completion.
  A legal bridge from the shared surface search passes fresh native refill with
  authored zones/exclusions unchanged; old evidence becomes stale. This is a
  feasible-candidate control, not automatic repair. The global router still
  defers that net, so merely setting detailed `defer_zone_nets=False` reports a
  missing connected guide. Native-driven repair/action selection and zone-guide
  coordination remain unchecked work in the feedback/integration milestones.
- Quality-increment verification: 210 passed, one optional test skipped across
  autonomous quality/plane access, routing quality, native plane verification,
  route cleanup, escape terminals, routing review, regional zone access, package
  access, escape assignment and surface package escape. Both new fragmented-fill
  cases ran successfully against the installed KiCad; the skip was an older
  regional-zone test requiring `kicad-cli` on PATH. No full-board reroute or
  hardware/production qualification is claimed.
- Research-list update: R5 now tracks native fragmented-fill repair and the
  missing-guide interaction for normally deferred zone nets. The feasible
  native-refilled bridge control is recorded, not relabeled as automatic repair.
- Contact-order increment: a reduced real-geometry case exposes a critical route
  consuming the only plane-contact window. The existing two-pattern budget now
  spends its second trial on `plane_first` when the probe lacks contacts or the
  final critical rebuild loses them. Ordinary geometry is proposed around early
  contacts, rebased to the clean owner source, and freshly revalidated with
  internally regenerated contacts locked before critical search. No default
  power-first policy, named repair selector or placement change is added.
- Ownership and rollback: early plane tracks/vias have separate report counts,
  never ordinary fanout ownership. Fresh critical, contact, hard-DRC and boundary
  checks retain successful access by identity. A seven-millimetre critical limit
  prevents the needed detour in the negative fixture; rejected copper is fully
  discarded rather than weakening that limit. Immutable macro copper and exact
  required four-via array multiplicities remain intact in renamed variants.
  Critical-only placement feedback refuses this mixed ownership; incremental
  placement safely falls back to full package preflight until explicit mixed-
  owner dependency rebuilding is implemented.
- Contact-order verification: all seven new success/rollback/area-routing cases
  passed. Two additional cases verify feedback/stale-guide guards and deterministic
  specialized coupled-pair routing around separate early contacts. Two incremental
  fallback guard cases passed with 113 critical, feedback, placement, CLI and
  flow tests. Another 196 boundary, plane, macro, via-array and critical-bundle
  tests passed; one optional via-array native test skipped because `kicad-cli`
  was not on PATH. The finalized report assertions and all 14 quality cases
  passed in an 18-test rerun, including both actual native fragmented-fill checks.
  The complete package/access rerun passed 69 tests; the additional coupled-pair
  case passed separately. Across these suites, 393 distinct focused tests passed
  and one optional test skipped; repeated cases are not counted again.
  These synthetic fixtures are not full-board closure or hardware qualification.
- Placement-ranking increment: shared package feedback now probes a bounded
  look-ahead of legal poses using real ordinary escapes, selected plane contacts,
  boundary allocation and hard native geometry. Access loss/new pending identities
  rank ahead of the existing congestion/escape/weighted-wire-length metrics.
  The default 16 probes are disposable hints, shared across accepted moves,
  independent of the eight full-trial budget; zero restores generator order.
  Generated critical copper is excluded from probes and freshly rebuilt during
  full acceptance. Reports/progress expose probe scope, rank, trial linkage and
  stop reason. No consumer-specific selectors, rules or repair code are added.
- Real-geometry placement coverage: a shorter-weighted-wire eastward move cannot
  reach the contact; the ranked westward move succeeds within one full trial in
  renamed variants and the standard placement/access/area pipeline. Negative
  variants reject locally successful contacts that break a critical length limit.
  A rigid unit rotates as a whole, a fixed companion prevents every pose trial,
  and renamed hard-macro cases translate both members and regenerate their exact
  owned tracks/vias without retaining copper from the old pose. Budget tests
  cover disabled/limited probing and multiple accepted moves without resetting
  the total probe allowance. These synthetic cases do not prove plane refill
  connectivity or hardware qualification.
- Placement-increment verification: all 15 new geometry/budget cases passed;
  92 package, CLI, flow and incremental-placement tests passed; 190 escape
  placement, rigid/template/macro, escape-feedback, incremental-placement and
  critical-feedback tests passed; 53 layout, global-routing, power-domain and
  small-package tests passed; 39 autonomous-contact/quality cases passed,
  including both actual native fragmented-fill checks. A collection audit
  confirms 371 distinct focused tests across these suites (shared incremental
  cases counted once), all passed without skips. The finalized CLI report
  assertions passed again separately. This is not full-suite or full-board
  routing/production evidence; no consumer source or release package changed.
- Next: add specific placement-constraint explanations, then extend shared
  blocker/dependency repair and native-filled feedback using the existing APIs.
  Do not replace consumer scripts or change locked production inputs until the
  standard pipeline demonstrates equivalent verified behavior.

- Native-feedback implementation (2026-10-11): retained input/project copies,
  refilled boards and raw native reports bind exact hashes and qualified version.
  Stable backend UUIDs map physical pads, tracks, vias and source regions; actual
  filled polygons distinguish fragments sharing one source-zone UUID. Guide-free
  surface bridges use exact clearance and source reservations. Zero-blocker
  search precedes a bounded ordinary-net dependency transaction; source/macro,
  critical/escape/plane ownership and required arrays remain immutable. Fresh
  native checks must reduce opens without introducing newly open nets or native
  non-library violations. Rejected trials roll back; repeated states, budgets,
  unknown objects and tool failures remain explicit failures. Isolated copper
  requests source-zone review, not automatic source redesign.
- Implementation verification: real native success/rollback, renamed inputs,
  fragmented-zone UUIDs, displaced ordinary dependencies, immutable macro/array
  variants, missing/unsupported tools, repeated state and CLI export fixtures
  passed targeted tests. Placement tests cover fixed companions, tight source
  regions and permitted orientation domains; reports cap sampled explanations.
  Area blocker reports include exact geometry identities, locks, reasons and
  work counts. This selects small bounded sets, not a proof of globally minimal
  disruption. See [implementation and limits](filled-copper-feedback.md).
- Consumer review: locked/offline checks passed for both boards using the current
  local compiler. CopperLedRing's balanced shared command connected all 31
  ordinary nets and passed actual KiCad 10.0.6 refill/DRC with zero opens and
  zero violations. Tracker's initial quick run correctly stopped ordinary
  routing: fixed `CELL_RF` endpoints exhausted exact search, and fixed
  `U_MODEM_LEVEL.7` lacked selected plane contact. Fixed-owner explanations were
  emitted; no source, macro, library or lockfile was weakened. Validation includes
  this unresolved outcome, not tracker closure or production approval.
- Migration: both projects now offer a fresh-directory `route-review` wrapper
  around the standard current CLI. Make dry runs verified its arguments. The
  explicitly selected compiler is a review input; pinned production dependencies
  and historical candidate files are unchanged. Tracker helpers are labeled
  historical and must not accumulate future selected-net repairs. Their removal
  remains unchecked until equivalent shared tracker closure is demonstrated.
- Final verification in progress: an isolated 2,403-test compiler snapshot is
  running, alongside a new balanced tracker run and final LED replay. Snapshot
  isolation preserves existing generated root files; full-suite success is not
  yet claimed. A subsequent bounded local critical-surface fallback is covered
  by four new renamed/length-limit cases and the existing local-critical suite;
  its full-suite snapshot replay remains to be run.
- Subsequent verification: that full run completed with 2,377 passes, 21 optional
  skips and five failures in legacy dogbone-only expectations. The tests now
  explicitly isolate dogbone-only policy, while default-surface cases verify
  that installed input copper remains intact. A 69-test rerun passed. A further
  123-test escape/package/quality rerun passed after adding a real mixed
  surface/dogbone domain case. Delayed surface expansion shares the existing
  assignment work budgets and preserves prior successful domain behavior.
- Native-anchor coverage: all 28 filled-feedback/CLI cases passed, including a
  real 65-land native fixture. Round-robin bounded anchor sampling represents
  both disconnected fragments rather than exhausting the allowance on the busy
  fragment. The test uses exact integer nanometre coordinates.
- Tracker probe outcome: the clean-source critical probe now connects `CELL_RF`
  using the shared local surface maze within its original rules. Rebuilding
  ordinary exits around critical/plane reservations still loses access to MCU
  SWD pins (and additional modem pins with planes first), so that proposal is
  correctly rejected. Mixed-domain assignment is now being tested on a fresh
  full tracker run. This is owner-coordination work, not an RF-rule waiver.
- Verification pending: a new full-suite replay is running against a refreshed
  isolated source/test snapshot. Existing generated files, locks and source
  constraints remain unchanged. Historical-script retirement is still gated on
  equivalent tracker closure, not merely installed algorithms or clean fixtures.
- Placement-hint verification passed 65 package/access-ranking tests. The standard
  CLI now considers authored surface-zone contacts by default, with an explicit
  opt-out and unchanged native-connectivity gate; 95 CLI/package/flow/incremental
  tests passed. Tracker review runs are pending, so no closure is claimed.
- Narrow-owner increment: a partial-ordinary fallback retains unaffected generated
  escapes and rebuilds intersecting ordinary nets only. It shares the existing
  pattern allowance, caps release at eight nets and protects source/macro/array
  copper and semantic bypass feeds. Ten new real-geometry success/budget cases
  passed with the 45 existing package-access tests (55 total). Final-gate rollback,
  dispatch and CLI cases plus placement/incremental regressions are now running.
  The separate full-suite snapshot predates this increment and the placement-hint
  and surface-default edits; its eventual result must not be attributed to them.
- Full snapshot replay completed: 2,388 passed and 21 optional tests skipped.
  Latest affected-source verification is separate: 56 partial-owner, CLI/native,
  placement-ranking and incremental tests passed; both newly added unmocked
  scarce-neighbor cases also passed. The all-owner ordering loses an ordinary
  exit in these renamed geometries, while the shared second partial-ordinary
  pattern preserves its original copper and reaches full package readiness.
  An isolated affected-suite replay and latest LED/Tracker board runs are pending.
- Latest affected-source replay passed 130 CLI, flow, package, placement, escape-
  feedback and critical-bundle tests in the isolated snapshot. All 17 finalized
  partial-owner cases passed together. Latest CopperLedRing replay again connected
  all 31 ordinary nets and passed actual KiCad refill/DRC with zero opens and zero
  violations. Tracker source and both consumer lock hashes remain unchanged.
- Pin-map clarification: the retained Tracker export maps `U_MODEM_LEVEL.7` to
  `V3V3`, not GND. The authored top GND pour/default surface policy cannot resolve
  this 3.3 V contact. Treat the selected supply-plane failure separately from
  ground-return intent and RF/ordinary owner ordering; use actual physical maps.
- Exact feed ownership: the first partial Tracker trial exposed an over-broad
  protection of every escape on a net having a decoupler. Fanout now attributes
  actual generated semantic-feed tracks separately. Partial rebuilding retains
  those occurrence-counted tracks while unrelated same-net escapes remain
  eligible; a probe intersecting the actual feed is rejected. Incremental
  ownership propagates surviving feeds. Cheap ineligible partial proposals do
  not suppress the established critical-only second ordering. All 68 focused
  ownership/assignment/incremental cases and 159 isolated CLI/package/quality/
  native cases passed after this refinement. These include 23 partial-owner
  cases; dispatch exhaustion/rejection injections are distinguished from the
  real geometry controls.
- Consumer results before lock correction: placement hints removed all Tracker
  critical failures via one legal MCU move, but the 3.3 V contact remained pending
  and ordinary area routing stayed stopped. The earlier partial run retained
  the protected-owner failure and failed; it predates exact semantic-feed
  attribution. No failed export became a production candidate or source waiver.
- Placement root-cause reproduction: temporary locks added by rigid legalization
  were checked against pre-legalization source poses during later decoupler
  validation. Two renamed real-geometry regressions failed before the fix; three
  explicit source/session-lock controls passed. The planner now binds only that
  phase to the current pose snapshot, preserving original rules and the final
  original-source/caller-session legality gate. All five regressions passed with
  the rigid, macro and routing-quality suites (114 total). A fresh standard
  Tracker run is underway; closure and historical-script retirement are unclaimed.
- Post-lock consumer replay: unchanged locked Tracker inputs now reach package
  readiness in the initial all-owner ordering: no ordinary/boundary or selected
  plane pads pending, no critical failures and no hard geometry findings. No
  package placement retry was needed. Ordinary area routing now runs; the quick
  profile still reports bounded-search exhaustion on several ordinary nets, so
  a fresh standard balanced-profile comparison is running. Package readiness
  alone is not filled-board closure or justification for helper retirement.
- Latest CopperLedRing replay after the lock correction connected all 31 ordinary
  nets and passed actual KiCad 10.0.6 refill/DRC with zero opens, islands or other
  violations. Physical and export digests match the preceding shared replay;
  native artifact bytes are hashed independently. The latest isolated full-suite
  replay remains in progress; no result from an earlier snapshot is attributed
  to newer code.
- Quick Tracker replay completed at 46/60 ordinary nets. Actual native refill
  found 19 opens, 11 dangling-track findings and zero islands; the result remains
  failed. Semantic bypass pending is empty and fanout records 25 protected feed
  tracks. Both ground/supply pours have no reported native open in this draft;
  the remaining native open nets are ordinary routing work, not plane contacts.
- Reporting correction: four real bounded-search/native cases reproduced known
  non-zone opens being mislabeled as unsupported native objects. The adapter now
  distinguishes `no_eligible_zone_opens` and `no_eligible_surface_bridge` from
  unknown UUID/schema evidence. Two opposite-surface variants preserve unsupported
  layer transitions as unresolved. All 57 affected native-feedback/CLI/plane tests
  passed. The running full-suite snapshot predates this reporting-only correction;
  its eventual result will be recorded separately.
- Read-only native checkpoint validation of the quick Tracker export passed the
  existing strict current-source pose/land/net and macro guards. Rechecking all
  typed bypass associations found 26 already surface-connected terminals (13
  feeds), zero additional feed tracks needed and no pending generic bypasses.
  The fourteenth association remains under specialized routing ownership. This
  confirms existing bypass copper, not whole-board closure or power integrity.
- Latest full frozen replay finished: 2,419 passed, 21 optional skips and one
  stale mechanical-language assertion expecting surface-zone CLI handling off
  by default. The intentional new default remains on, with an explicit generic
  opt-out; the corrected expectation passed with all 160 mechanical/editor
  cases. The separate 57-test native-feedback run covers the reporting change
  made after the snapshot. This is full-snapshot plus affected-source evidence,
  not a claim of a single fresh all-green full run. Tracker's consumer suite ran
  64 tests successfully, with three explicit-library-review cases skipped.
- Next generic area-search reproduction: four renamed/reversed real geometries
  fail the existing final retry while a 200-percent weighted control succeeds at
  the same per-search state cap; both strict length-limit controls stay open.
  The shared final retry now diversifies only after an actual budget exception,
  with structured failure/retry-weight reports and no additional attempt or rule
  waiver. Affected regressions and a fresh unchanged-source Tracker replay are
  running. [R7](routing-research-tasks.md#r7-budget-exhausted-ordinary-final-retries)
  tracks this verification separately; the completed frozen full suite predates
  this latest algorithm change.
- Retry-increment verification completed: 94 detailed/report/benchmark tests,
  177 pipeline/placement/ownership/native tests and 28 finalized retry/checkpoint/
  exact-reuse/native-CLI tests passed. These suites overlap; they are not summed
  as distinct cases. All ten finalized bounded-search cases passed, including
  deterministic replay with memoization disabled, truthful JSON retry metadata,
  requested-weight preservation, missing guides and immutable length limits.
  The existing injected branch-failure test additionally confirms that exhausted
  private checkpoints retain the budget bit and successful resumes clear it.
  Tracker's fresh balanced run and latest CopperLedRing native check are pending;
  historical script retirement remains unchecked. Read-only checkpoint and
  rendering tools still import those helpers, so retirement must preserve their
  validation dependencies rather than deleting them indiscriminately.
- Latest retry-source CopperLedRing replay passed: all 31 ordinary nets connected;
  actual KiCad 10.0.6 refill reported zero opens, other violations and islands.
  Physical digest `6679df65256b8f19cfe34ab1c3f961862cc9dd3a5df6f9f107a1da355a862992`
  and export digest `236970d09e42c8758daae2388b9befe30583ec45d6245662a9ca53be0898b791`
  match the prior successful geometry; this run's native filled-board digest is
  `a5a26150bfafe8b1f0648bbdf71ffd5c7565563e5d58df88f0157d7fedf6bfd5`.
  Inputs/lock hashes remain unchanged. This is routing evidence, not approval.
- The older balanced Tracker comparison completed at 58/60 ordinary nets, with
  `I2C_SDA` and `V5` unresolved. Native refill reported seven open connections on
  those two nets, zero other violations and zero islands. Its old native stop
  reason predates the corrected scope reporting, and it did not execute the new
  retry policy. The fresh retry-source balanced run again reached full package
  readiness (all critical routes, plane contacts and 82 ordinary/boundary exits),
  with no package placement retry or source/constraint edits. Area routing is
  running; final closure remains unclaimed.
- Consumer integration replay with `COPPERLIB_REVIEW_ROOT` explicitly selecting
  the unchanged local CopperLib passed all 64 Tracker tests without skips. This
  includes the preliminary battery-revision incomplete-evidence guards; their
  expected incomplete statuses are not hardware or release approvals. No library,
  consumer source, procurement selection or lockfile changed.
- Actual retry-source Tracker area evidence: the automatically diversified
  200-percent final retry reconnected `MCU_MODEM_RX`, `MCU_MODEM_TX` and
  `MCU_RESET` at the unchanged 30,000-state search cap. The selected merged
  candidate reached 59/60 ordinary nets; only `V5` remained budget-exhausted.
  The existing neutral-cost fallback is still running. This is an intermediate
  area result, not final native closure or proof that `V5` is impossible.
- A collection audit confirms 285 distinct affected tests across the three
  completed suites; all were exercised successfully, with the ten finalized
  retry cases covered by the separate replay. The fresh isolated full run is
  still pending. An additional unchanged-source Tracker `thorough` replay uses
  the existing generic profile (four passes, 100,000 states per search), without
  named repair selectors or source/rule changes. Neither running replay is yet
  source-matched final native closure; historical helpers remain retained.

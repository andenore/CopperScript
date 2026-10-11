# Bounded filled-copper feedback

`route-board` owns placement, package access, ordinary routing, late contacts,
native refill and bounded native-open repair. No board-specific net/pad repair
list is needed. CopperLib continues to own reusable parts and physical contracts;
board projects retain electrical/mechanical intent and qualification evidence.

## Run it

```console
python -m copperscript route-board board.copper --locked --offline --routing-profile balanced --verify-plane-fill /path/to/kicad-cli --report new-review/route.json -o new-review/candidate.kicad_pcb
```

Supply the normal footprint roots, fabrication profile, layers and explicitly
bound macro assets for the board. A profile never changes those design inputs.
Authored surface-zone contacts are considered by default; use
`--no-stitch-surface-zones` for an explicit diagnostic opt-out. These remain
provisional until native refill verifies actual connectivity. The low-level
`PlaneStitchOptions` API default is unchanged.
On Windows use the installed absolute executable path. KiCad major 10 is the
qualified refill adapter; other majors are reported as unsupported, not approved.

The profiles set bounded **computational** work, not electrical constraints.
Explicit numerical options override the profile, including zero trial budgets.
No profile forces a fixed component to move or changes RF/power requirements.

| Profile | Area passes | Area search states | Placement trials / probes | Native trials / states per trial |
| --- | ---: | ---: | ---: | ---: |
| quick | 1 | 10000 | 2 / 4 | 2 / 2000 |
| balanced | 2 | 30000 | 8 / 16 | 8 / 4000 |
| thorough | 4 | 100000 | 16 / 32 | 16 / 12000 |

Without a profile, the established individual CLI defaults remain, with eight
native-driven repair trials. `--filled-copper-trials 0` runs verification only.
`--filled-copper-search-states` caps each bridge trial. The shared blocker limit
and dependency-expansion limit also apply. Critical, package-pattern, local
contact and placement stages retain their separately reported bounds; the
profile is not a total wall-clock timeout or an exhaustive routing search.

## Evidence before action

The native adapter binds source/export digests, KiCad version, exact filled-board
and report hashes, mapped UUID objects and filled polygon fragments. A zone
outline is never fill evidence. KiCad may report two fragments with the **same
zone UUID and nominal origin**: the controller measures exact physical pad/track/
via membership in the freshly filled polygons instead of guessing from text.
Fractured hole/slit polygon representations are retained as native vertices.

CLI native runs retain a fresh evidence directory beside the report, or the new
directory specified by `--native-evidence-directory`. Each native invocation
retains the original same-stem PCB/project under `input/`, the refilled candidate,
same-stem project and raw `drc.json`. Rejected candidates remain inspectable but
never replace the incumbent. Existing evidence directories are never overwritten.
Exact artifact hashes may change with native timestamps; replay compares routing
geometry, decisions and work bounds, not a fabricated constant native report hash.

The standalone API is `improve_filled_copper(pipeline, ..., kicad_cli=...)` with
`FilledCopperFeedbackOptions`. It verifies fresh inputs itself; it does not accept
a caller-supplied connected flag or a stale native certificate.

## Repair transaction

Only mapped opens on general distribution-zone nets are eligible. Specialized
RF/USB/critical nets do not become ordinary bridge candidates. Unknown objects,
unsupported findings and absent fragment anchors remain explicit unresolved
statuses. Endpoint sampling is bounded to 64 anchors per reported zone item;
failure of this sampled search does not prove there is no physical solution.
Anchors are sampled round-robin across eligible fragments, so a busy first
fragment cannot consume the entire allowance before another is represented.

1. Prefer a legal surface bridge with **no displaced owner**. With mutable area
   copper present, spend at most one quarter of the per-trial bridge state budget
   on this search; the relaxed proposal receives only the remaining states.
2. If necessary, propose an octilinear surface path with only completed ordinary
   area copper removed. The source width/neckdown, allowed layers, keepouts,
   board-material limits and macro access reservations remain authoritative.
3. Query the exact identities/reasons of blockers. Select only the intersected
   eligible mutable nets, then use the existing bounded dependency-cone router.
   This is minimal for the chosen path, not a globally optimal blocker-set proof.
4. Restore all unaffected copper exactly, preserve the immutable prefix and all
   accepted contacts/bridges, reroute displaced nets and check hard physical DRC.
5. Refill independently. Accept only a strict reduction in native opens, no newly
   open net, no island and no native non-library violation. Unknown new objects
   fail closed. Geometry-changing closure after verification rejects the trial.

Source, macro, required via-array, critical, package/boundary escape and accepted
plane/bridge ownership is protected using occurrence-counted geometry, not a
designator blacklist. Electrical topology, minimum widths, copper weights,
authored keepouts and fixed poses are never silently edited. Successful boundary
geometry stays reserved; native connectivity rechecks displaced signals.

This implementation generates surface bridges only, not new layer transitions,
automatic pour redesign or a general RF/power solver. It can make partial
progress and still report failure. A clean native route is **not production,
independent CAM, RF, power, thermal, battery or prototype qualification**.

## Reports and stop reasons

`package_access.placement_constraints` contains at most 32 explanations, separating
sampled rejected poses, fixed owners and permitted orientation-domain context.
Rejected ordering trials can also supply `pattern_hint` context: measured lost
ordinary identities identify eligible placement owners when original endpoints
are fixed. Discarded candidate copper is never adopted, and every hint still
passes source/session placement legality and fresh all-owner acceptance.
These reuse the actual placement legality predicates, including fixed companions,
named regions, keepouts, mechanics, rigid groups and relative constraints. They
do not claim an orientation domain or a sampled region is an impossibility proof.

When an all-owner ordering loses ordinary access, the second pattern allowance
can try `partial_ordinary`: keep generated escape copper that remains legal
against the disposable critical probe, and rebuild only intersecting ordinary
nets around it. Release is net-granular so shared same-net stubs are not assigned
to guessed pad owners. Source/macro copper, required arrays and semantic
decoupling feed tracks cannot be released. Unrelated escapes on the same supply
net remain eligible; protection follows actual generated copper, not a blanket
net label. Occurrence-counted ownership and the same
fresh critical/contact/boundary acceptance gates protect the combined result.
`--package-partial-owner-nets` bounds release to at most eight nets; zero disables
this proposal. It shares the existing two-pattern allowance, never adds a third
trial, and is not a globally minimal repair or an exhaustive search. Trial reports
include displaced net identities and retained track/via counts. Consumer closure
must still be demonstrated independently; installing this fallback is not proof
that Tracker routes.
Cheap eligibility rejection does not suppress the established critical-only
second ordering after a plane/critical proposal; `partial_skipped_reason` records
why that bounded fallback was selected. `fanout.protected_decoupling_track_count`
describes semantic-feed ownership at that stage, not final filled connectivity.

The placer validates later bypass moves with rigid phase locks bound to their
current legalized/refined poses. Source-fixed constraints and the final legality
gate against the original source and actual caller session locks remain intact.
This prevents temporary macro locks from silently blocking semantic capacitor
placement; it does not unlock a fixed capacitor or rigid companion.

`detailed.repair_trials` records exact blockers, immutable flags, snapshot digest,
displaced nets, rerouting order/search counts and whether the merged pass was
selected. At most 256 entries are retained; progress reports each transaction.
Failed detailed searches record `search_budget_exhausted` from actual exceptions,
not text matching. After both baseline passes, the existing final strict retry
may use at least 200 percent heuristic weight on those still-open nets. It keeps
the same state/pitch allowances and source/ownership checks, and reports the
automatic choice as `retry_heuristic_weight_percent`. There is no extra attempt,
net selector or source-length waiver; weighted paths need not be optimal.
`filled_copper_feedback` records input/candidate hashes, bridge expanded-state
counts, dependency searches, displaced nets and fresh native evidence per trial.
Failed bridge searches retain at most 32 blockers sampled from 16 rejected edges.

Native stop reasons distinguish `verified`, `disabled`, `trial_budget_exhausted`,
`stagnation`, `package_access_blocked`, `physical_violations`, `native_violations`,
`source_zone_design`, `unsupported_native_objects`, `missing_tool`,
`no_eligible_zone_opens`, `no_eligible_surface_bridge`, `unsupported_tool` and
`invalid_native_result`. Known non-zone/specialized opens remain routing work,
not an unknown-object failure. Known zone opens without a supported same-surface
terminal/fragment pair do not authorize an invented layer transition.
Source-zone design is a review
requirement for native isolated copper, not automatic permission to alter a pour.
Repeated candidates are not reverified; failed candidates roll back completely.
Missing tools save a failed review report/draft, never a successful empty check.

When dogbone sites are unavailable, crowded ordinary pins can use a directly
validated via-free surface escape to their package collar. The same assignment,
boundary and physical DRC gates apply; this fallback does not waive via-in-pad
or clearance rules. `FanoutOptions(surface_fallback=False)` isolates dogbone-only
experiments; no board-specific surface selector is necessary.

When dogbones exist individually but conflict jointly, surface alternatives are
added only after the original assignment search fails. Both phases share the
same state/pair/query budgets; exhaustion does not reset an allowance. Selected
dogbones, reused input vias and surface handoffs retain distinct ownership.

An explicitly via-free two-terminal critical route that exhausts the board-wide
fine-grid attempt can also try the shared bounded local surface maze: 0.1 mm
steps, a 2 mm search-envelope detour and at most 20,000 states per permitted
layer. These are computational limits, not permission to exceed electrical
length limits or bypass reservations. Original specialized rules and hard DRC
gate every candidate; missing global guides remain failures. Existing critical
`search_states` counts primarily paired search and is not a total single-ended
fine-grid work counter; do not infer zero computation from its zero value.

For fixed input/evidence snapshots, proposal ordering and physical/export
digests are deterministic. Native tools
can produce different refilled file bytes/report metadata between runs; each
actual output is hashed independently rather than promised byte-identical.

## Regression inputs

`tests/test_filled_feedback_cli.py` builds a synthetic source board and explicit
synthetic footprint keepout, then executes the real standard CLI/native adapter.
`tests/test_autonomous_routing_quality.py` covers renamed fragmented pours,
surface-fallback rotations, package crossing quality and semantic decouplers.
`tests/test_filled_feedback.py` covers bounded failure, exact mappings/artifacts,
real displaced-signal rerouting, immutable critical routes/macros/required arrays,
native-rejected candidates, repeated states and tool/schema limitations.
These are algorithm fixtures, not reusable production component definitions.

See the [checked implementation plan](autonomous-routing-plan.md) and
[R5 investigation](routing-research-tasks.md#r5-automatic-fragmented-fill-repair-and-routing-guide-coordination).

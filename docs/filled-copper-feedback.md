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
Anchors are sampled round-robin across electrically distinct components, then
their accessible layers. Separate polygons already joined by explicit copper,
plated transitions or actual native fill do not consume separate component slots
or receive redundant bridges. The grouping filter shares DRC's bare-copper
contacts, excludes installed-component internal edges, and requires fresh
source/export-bound fill. The optional `qualification` geometry dependency
decodes exact reversible native hole slits without healing arbitrary polygons.
Positive-area contacts use conservative inscribed copper/circumscribed drills;
boundary-only contact, drilled voids and unsupported/disconnected polygon walks
do not establish a join. Without that dependency, native-fill component grouping
is unavailable; non-zone explicit-copper anchor expansion remains available and
unresolved searches still cannot claim native closure.
Repair proposals also share the trial allowance round-robin across distinct
open nets. Equivalent net/layer/endpoint proposals are deduplicated before
spending a trial; source names do not select the repair policy.
For a zone or non-zone native item, anchors may also come from its actual connected
bare-copper/native-fill component. Existing plated transitions can therefore supply a
same-layer bridge endpoint even when KiCad names two opposite-layer tracks.
Same-net labels, installed-component internal joins and source-zone outlines
do not join this graph. Sampling stays capped at 64 nearby explicit anchors;
no new transition is invented and fresh native acceptance remains mandatory.
Native-rejected endpoint pairs are skipped only within the current transaction
when involved-net fill/geometry, every via and all copper on involved layers
remain unchanged. Unrelated other-layer bridges do not trigger identical failed
trials; relevant geometry changes invalidate the rejection cache. This does not
increase the configured trial or search budgets.

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
   Dependency routes with retained package exits must use those ports: flexible
   pad alternatives cannot abandon a preserved launch via's other-layer witness.
   Main routing may have replaced a collar witness with ordinary area copper.
   If its owned launch via remains, the original witness can be regenerated
   only after actual land-path and exact clearance verification against the
   reserved prefix/bridge, then rerouted through that port. The shared cone
   allowance is 64 regenerated witnesses. Released vias are never recreated;
   blocked witness regeneration rolls back rather than evicting a fixed owner.
5. Refill independently. Accept only a strict reduction in native opens, no newly
   open net, no island and no native non-library violation. Unknown new objects
   fail closed. Geometry-changing closure after verification rejects the trial.

Inside a trial, up to 64 exact native dangling tracks/vias on displaced ordinary
nets may be removed transactionally. Macro, required-array and caller prefix
ownership is occurrence-counted even when the caller supplies empty guards.
Only already explicitly connected ordinary nets are eligible; no supply-zone or
specialized copper is cleanup authority. Both before/after explicit connectivity
and one further fresh native refill are required. Unrelated zone opens may remain
during cleanup, but no new open net, increased open count, violation or island may
be accepted. This extra refill does not grant another routing trial. Protected,
unknown, mixed unsupported or stale findings leave the entire candidate intact.

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
Placement look-ahead alternates legal proposals between failing source
components. Bypass capacitors associated with the same target share one
cohort, so many dependencies of one package do not consume every early
probe. Existing owner ranking and move choices are retained within each
cohort; probe, trial and legality bounds are unchanged. Exhausted cohorts
drop out without granting additional moves or acceptance authority.

These reuse the actual placement legality predicates, including fixed companions,
named regions, keepouts, mechanics, rigid groups and relative constraints. They
do not claim an orientation domain or a sampled region is an impossibility proof.

When an all-owner ordering loses ordinary access, the second pattern allowance
can try `partial_ordinary`: keep generated escape copper that remains legal
against the disposable critical probe, and rebuild only intersecting ordinary
nets around it. Release is net-granular so shared same-net stubs are not assigned
to guessed pad owners. Source/macro copper, required arrays and semantic
decoupling supply/return tracks and vias stay protected by default. Explicit
bypass regeneration may release only freshly witnessed generated contributions
within the measured owner budget; every prior association must be independently
verified again. A return-net conflict cannot release an unselected supply cohort.
Shared, source and immutable contributions remain held. Unrelated escapes on the same supply
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
and `fanout.protected_decoupling_via_count` describe generated semantic ownership
at that stage, not final filled connectivity.

The placer validates later bypass moves with rigid phase locks bound to their
current legalized/refined poses. Source-fixed constraints and the final legality
gate against the original source and actual caller session locks remain intact.
This prevents temporary macro locks from silently blocking semantic capacitor
placement; it does not unlock a fixed capacitor or rigid companion.

Opposite-surface bypasses require complete explicit supply and declared return-net
joins to physical terminals on the target component. Only current original tracks
and permitted ordinary through-vias witness these paths; zone intent, pad groups,
via labels and connected flags do not. The witness establishes physical joins,
not component-internal return behavior, decoupling impedance or electrical loop
qualification. Unsupported source/process/length/via budgets stay pending.
The generated contribution retains its original complete path witnesses in
`FanoutResult.decoupling_witnesses`. Admission freshly checks those exact edges,
both terminals, source context, current clearances and whole-net budgets; a later
canonical path choice cannot discard ownership of a still-valid adopted path.
Paths must be node-simple, although consecutive subsegments may use the same
original track. New proposal copper must occur on its complete supply/return
paths before adoption, within the existing proposal allowance; unsupported
contributions remain pending. Legacy contributions without records still require
fresh canonical-path coverage. A freshly verified complete return half keeps its
explicit target while the missing supply half is generated; distance-based target
truncation applies only when no return half exists. An explicitly selected target
remains binding, and the combined proposal still requires fresh validation.
Connection generation omits a new straight segment only when both endpoints lie
exactly on one held segment of the same net, layer and width. It preserves the held
segment and its ownership; partial overlap or a union of separate segments does
not satisfy this narrow containment check. Proposal and path-search bounds remain
unchanged. Partial/contact regeneration includes original
paths when protecting shared owners and freshly validates its merged ledger.
Disposable partial probes retain their actual copper/fills but generate new
witnesses under the owner's source metadata from the start. All other source
fields and the exact ordered source copper prefix must match before generation;
this does not rebind a witness created under another context.
Changed placement context requires full preflight rather than relabelling an old
path. `DecouplingRouteResult.supply_return_witnesses` remains the final canonical
snapshot; `admitted_path_witnesses` separately records adopted original paths.
Placement retains exact shared/generated/source copper while testing disposable
poses, freshly rechecks all prior association identities at the new poses, and
rolls back if normal regeneration loses any identity. Public placement output
remains unrouted; it does not promote generated bypasses into immutable macros.


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

For unresolved work, use the [current backlog](../TODO.md). Dated board closure
and test evidence are recoverable from Git; use current contracts and input-bound reports.

## Contributor invariants

Generic repairs use the standard bounded pipeline, never board-specific net/pad
selectors or historical pose seeds. Reproduce failures as small real-geometry
cases with renamed nets/components. Preserve electrical topology, declared
widths/clearances/keepouts, fixed/rigid placements, immutable macro copper and
required via-array multiplicity; native warnings cannot be waived for closure.

Keep verified package exits reserved while ordinary routes remain partial.
Only remove disposable unused escapes after the net connects. Distinguish
ordinary mutable escape copper from source, macro, semantic-feed, required-array,
critical and accepted bridge/contact ownership. Mutability is explicit and
occurrence-counted. Dependency eviction, rerouting, ownership-checked cleanup
and fresh native refill form one transaction with complete rollback.

Follow actual explicit copper components from native findings to retained plated
transition anchors; source-zone outlines and installed-component internal joins
are not repair witnesses. Regenerate a trimmed owned boundary witness only with
its retained launch via and fresh exact land/clearance checks. Deduplicate
equivalent proposals across opens within the shared search budget. Cleanup needs
exact native object mapping and fresh connectivity, not inferred object identity.

Missing tools and exhausted budgets remain unresolved evidence, not proof of
impossibility. A closing benchmark establishes routing only; CAM, electrical,
RF/power, assembly and prototype approval retain their separate gates.

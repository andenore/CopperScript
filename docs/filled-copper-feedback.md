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
cohort; probe, trial and legality bounds are unchanged. Inside an otherwise
tied dependency tier, exact current source capacitor-terminal positions may
break the reference-name tie by distance to existing failed pins. This is a
proximity hint, never measured blocker or clearance authority. A tier with an
unsupported terminal identity retains its original order. Exhausted cohorts
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

A no-pose surface/bypass proposal may spend one existing pattern slot when a
freshly measured generated supply feed blocks an ordinary surface-only package
cohort. It builds one bounded domain batch, releases only one declared
association's exclusive original supply-track occurrences, jointly allocates
all cohort exits and regenerates that exact feed. Shared paths, all vias,
unselected associations, source copper, macros and required arrays remain held.
Every original cross-layer witness and established exit is freshly checked;
the complete critical/contact/boundary gates and exact unrelated-owner
retention still decide adoption. Unsupported source/fill/ownership contexts
decline without contribution. Same-face supply continuity does not establish
return-loop qualification or complete end-to-end signal routing.

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

Global terminal entrance selection screens legal direct stems for incident graph
resources on allowed layers before deciding whether its existing bounded local
fallback is needed. The fallback targets existing nodes with allowed resources;
its candidate/state/step/detour bounds and all exact copper predicates stay the
same. If no better checked path is found, original isolated choices remain
available for later checked terminal-via alias registration. Incident degree
alone proves neither target connectivity nor physical route feasibility; this
does not refine the coarse graph's lane-capacity model.

Differential tuning retains the legacy bounded attempt first. Only a zero-addition,
overskew result with an explicit complete held context may try one additional
call to that same bounded tuner using private virtual spans. Only adjacent,
forward, axis-aligned segments of equal net/layer/width in one source breakout
region may merge. Source/held occurrences, finite-width copper contacts, pads,
vias, branches and explicit access nodes retain their authority; unsupported
or native-fill contexts decline. An unsuccessful attempt preserves the original
candidate objects. A successful private proposal restores untouched fragments,
freshly checks source widths, anchors, skew and full held geometry, and still
passes the normal outer candidate gates. This adds no via or electrical allowance
and does not establish consumer USB routing or signal-integrity qualification.

Package-placement feedback may borrow one existing probe slot for one coupled
placement proposal before ordinary legal-move lookahead. Its bounded source-pad
projection hint uses exact transformed land identities and side labels; at the
default 28-owner/28-root limit it evaluates at most 1,568 shape pairs and 3,136
shape predicates. Projection overlap/proximity is a selection heuristic, not
copper clearance, a verified via blocker or exit feasibility. Ambiguous lands
or associations decline. Only mutable singleton named-region owners/neighbors
are supported; fixed/session locks, rigid/macro membership, relative rules and
via-array ownership remain protected.

The requested existing move stays anchored in BASE. One unchanged local pack
call may move its directly colliding eligible neighbors, with at most 28 shared
neighbor probes across requests and the existing six-member limit. A single
irreversible token persists across accepted rounds; unsupported/failed claims
spend their borrowed probe slot too. Successful geometry uses that same slot
for the ordinary access probe. This adds no probe/trial slot or budget reset.
Reports distinguish spent slots, normal probes, source-hint predicates and
actual pack domain/search work. Domain-probe counters do not count every inner
geometry predicate. Default local limits remain 6,144 raw proposals/384 retained
poses per neighbor, 172,032 search operations and the separate 57,344 heuristic
checks; named-region continuation uses only residual resources.

Every positive retains original source fields, the requested pose and all
unselected poses, then passes complete original-source placement legality and
the normal fresh global/preflight/ownership gates with full rollback on failure.
Numeric options, regions, courtyards, copper rules and electrical requirements
are not relaxed. This generic placement adapter has no normal CopperProbe
adoption or whole-board/electrical qualification claim.

Global terminal access retention uses the same finite generated pool and
configured candidate count. Physical through-via launches receive one retained
slot each before additional signal-layer aliases of the same hole; available
unrepresented layers are preferred within each launch. Distinct surface graph
entrances remain separate. This selection performs no geometry queries, adds no
candidate or capacity, and is not an exhaustive escape search. Fresh terminal
capacity registration, negotiated routing and all physical gates still apply.

Opposite-face bypass proposals may reuse existing normal through-vias only after
recognizing an exact, legal copper path from the actual declared source pad.
Installed-component internal connections do not establish this path. Existing
sites are examined within the finite fanout radius and site count, and consume
the same launch allowance before fresh launches. One proposal transaction
shares at most four local tail searches (100 um steps, 1 mm detour, 4000 states
each) across supply, return and return targets. These require an already
verified launch, no legal cheap tail, and the fanout maze option. Exact complete
paths, protected owners, created-edge coverage and final physical gates remain
mandatory. This helper requires existing source copper; it alone does not prove
the standard fanout sequence or a consumer board has closed.


After ordinary fanout passes its materialization gate, one explicit late bypass
completion pass may retry only its original pending opposite-face source links,
intersected with the original net filter. It requires newly generated ordinary
copper and never repeats a completion round or reroutes ordinary/critical/area
copper. Existing per-link proposer bounds remain authoritative. The pass and
selected/completed/remaining link counts, generated counts and rollback reason
are recorded; compact reports expose the result separately from routing closure.

Every late candidate retains the full early-semantic and ordinary occurrence
prefix and source fields. Original via/surface handoffs, verified associations,
admitted path witnesses, source budgets and physical gates are freshly checked.
A rejected late candidate alone is rolled back. Only newly appended late objects
join the semantic owner ledger; reused ordinary source paths/vias retain their
original owners. Pending obligations use complete declared link identities;
source validation still permits only one explicit association per capacitor.
Existing handoffs are not replaced and a newly measured semantic port never
invents an ordinary-domain candidate index. These physical joins and bounded
completion reports do not establish electrical, RF, power or fabrication release
qualification.


Local pin-access routing retains the eight-direction geometry contract even when
exact endpoint coordinates divide computational grid intervals unequally. The
direct shortcut and each maze edge reject non-45-degree diagonals before any
clearance query. Horizontal/vertical edges, original grid/domain, state bounds
and source clearances remain unchanged; rejected edges do not purchase extra
search allowance. Exact existing package-exit verification remains authoritative.

### Geometry-derived short package escapes

Ordinary fanout and opposite-side bypass search try up to four adjacent square
cell centres for circular SMD lands before long outward rays. Cells come from
actual footprint pad positions, including rotation and back-side mirroring;
there is no package-name or board-name selector and no assumed ball pitch.
Bypass sites consume the existing candidate budget. These are ordinary through-
via proposals: exact pad/annulus/drill exclusions, layer rules, fixed ownership,
verified feeds and whole-candidate acceptance still apply. Reference-board
measurements belong to the independently versioned part library.

At equal feed length, the decoupler placer prefers its supply-to-return axis
pointing outward from the target pin. This tie break avoids tangential bodies
occupying adjacent package exits; explicit orientations, placement domains and
fixed poses remain authoritative. It is not a decoupling impedance qualification.

Full ordinary pattern proposals use the same authoritative source frame as
partial proposals: disposable routing metadata is normalized before generating
bypass witnesses, while actual probe copper/fills remain obstacles. After probe
copper is stripped, original witness paths and exact generated ownership are
freshly admitted. Borrowed disposable return copper rejects the proposal within
its existing trial slot; it does not abort routing or become ordinary ownership.

## Per-association post-fill observations

Native plane verification reports `decoupling_assessments` for each declared bypass.
It uses the existing positive-area filled-fragment contact grouping and exact
source/export binding to observe supply and target-component return continuity.
Split fragments, missing return terminals, ambiguous pads, stale exports, unknown
native objects and unavailable geometry remain open or explicitly unsupported.
These observations do not replace local bypass path witnesses, waive routing gates,
or qualify high-frequency loop impedance. They are a diagnostic foundation for
plane-assisted routing; the preliminary package-access stage still requires its
existing explicit local supply/return paths.


Opposite-surface bypass proposals can use a bounded inner-layer bridge with
two ordinary through-vias when direct surface tails are blocked and the source
permits those layers and via counts. All eight destination-launch trials share
the existing complete-transaction allowance; four bounded local maze tails are
shared by supply and return. Both explicit terminal paths must still validate.
Before admission, an unused branch may be removed only from the current
proposal's generated suffix. Every input track/via object, including immutable
macros, required arrays and previously admitted partial exits, stays intact.
The trimmed candidate receives a fresh clearance index, both complete path
witnesses, protected-witness checks and physical DRC. A failed trim rolls back.
Native fill observations remain separate from this explicit-path admission.

Paired through-via preparation first tries the established short collars. If
none produces a legal complete surface/via/return/other-layer transition, it
tries four longer collar distances (4 mm, 4.5 mm, 5 mm and 6 mm) to reach beyond
opposite-side component banks. This fallback adds no local maze allowance and
keeps the existing retained-port and joint-search bounds. Breakout regions,
normal trace profiles, source keepouts, ordinary drill clearance, return-via
requirements and final length/skew gates remain unchanged. Legal launch sites
alone do not establish simultaneous or end-to-end pair connectivity.

Inner bypass bridges consider verified existing destination feeds before new
via sites, within the same shared eight-launch budget. Both terminal-to-via
paths must use original copper edges; unconnected same-net vias are not feeds.
Existing objects survive both admission and rollback, and repeated sites do
not consume additional bridge trials.

Bypass preparation honors descending source supply-net routing priority, with
stable source order for ties. Exact link/net filters retain the full source
inventory and all protected paths. This permits an authored constrained local
regulator output to reserve its explicit path before broader power distribution;
priority does not relax geometry, ownership, path or native verification gates.

Explicit bypass reuse may follow existing legal straight plane-contact edges
at arbitrary angles. This is an opt-in to the exact land-path observer; ordinary
fanout keeps its orthogonal/45-degree policy. Width, clearance, through-via
geometry, exact source association and complete supply/return witnesses remain
mandatory. No new arbitrary-angle search or component-internal join is inferred.

Repeated edge observations use a bounded 32-entry cache keyed by exact board
and clearance-index identities, the complete ordered insertion ledger and all
index routing rules. Added obstacles or changed rules invalidate reuse. Path
graphs, endpoint selection, state bounds and complete witnesses are rebuilt;
no changed trial inherits a previous board's clearance proof.

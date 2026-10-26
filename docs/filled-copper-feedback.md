# Bounded filled-copper feedback

When a critical retry removes temporary ordinary-escape keepouts, an already
admitted bypass reservation can be rebound to that exact removal. The original
source and reservation must pass admission first; every non-keepout source field,
ordered copper occurrence and generated ownership claim stays unchanged. Supply
and return paths are derived again from the new geometry and pass full admission.
Changed poses, rules, copper, added/reordered keepouts or incomplete paths reject
the rebind. The previous reservation remains intact on failure. This does not
waive a saved witness's context digest or remove authored constraints.

`route-board` owns placement, package escapes, critical routing, unified ordinary
routing, late contacts, native refill and bounded native-open repair. No
board-specific net/pad repair list is needed. CopperLib continues to own reusable parts and physical contracts;
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

| Profile | Native trials / states per trial | Decoupling placement trials |
| --- | ---: | ---: |
| quick | 2 / 2000 | 4 |
| balanced | 8 / 4000 | 16 |
| thorough | 16 / 12000 | 64 |

Without a profile, the established individual CLI defaults remain, with eight
native-driven repair trials. `--filled-copper-trials 0` runs verification only.
`--filled-copper-search-states` caps each bridge trial. The shared blocker limit
and dependency-expansion limit also apply. Critical, local contact, placement
and unified-router stages retain their separate bounds; the profile is not a
total wall-clock timeout or an exhaustive routing search.
`--package-initial-pair-states N` caps each coupled pair's search in the critical
stage (zero: no cap); `--unified-pair-states` takes precedence. Exhaustion remains
a routing failure and never relaxes geometry or acceptance gates.

## Evidence before action

Final routing closure may reconcile failed coarse global guides when later
detailed or critical routing reports those exact nets connected, or the nets
belong to declared zones, and matched clean native refill proves the final
board. Global overflow, unresolved downstream signal searches and physical
geometry findings still reject closure. The original guide report and physical
DRC token are retained; this does not confer manufacturing or electrical
qualification.

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

The public `route_global(source, fixed_prefix=board, reserved_accesses=ports)`
interface admits a fixed prefix only when its clean source contracts, complete
macro/required-array prefix and a fresh physical DRC match, and records a
`fixed_prefix_digest`. Unknown ports fail closed; no input copper is moved or
released. Retained launch vias are reported separately from new via proposals
and do not create free transition capacity for other nets. A checked port may
project into a coarse cell using the existing `region_only` representation;
detailed routing must still reach the actual port. Native whole-board
connectivity and all engineering gates remain mandatory.

An empty global terminal now reports how many local candidates were generated
and how many survived shared transition-capacity admission. Zero generated
candidates identifies a bounded local search miss; nonzero generated candidates
with zero registered candidates identifies capacity rejection. These diagnostics
do not establish geometric impossibility or change the admission rules.

Global keepouts use their actual polygon for node exclusion and conservative
polygon projections over each complete coarse edge for lane capacity. Concave
bays remain available; a thin barrier between grid centers still blocks the
crossing. Keepout holes remain unsupported by the exact router and do not gain
an approval from a coarse guide.

Checked terminal via proposals receive capacity before uncommitted sampled
slots. The sampler then refills only coexisting sites and rebuilds transition
capacities from the surviving pool. Actual input copper is always present in
the clearance index and cannot be replaced. Candidate rounds are deterministic
and bounded; remaining access rejection and overflow stay explicit failures.

Remote pads of explicitly reserved regional pours are reported separately as
`plane_stitch.explicit_feeder_pads` when they are beyond the conservative local
contact envelope. They are neither stitched nor connected by this classification.
Nearby contacts retain their original search and pending failures. Ordinary
feeder routing and final full-net/native checks must connect the remote pads.
Unreserved distribution planes and immutable macro contacts keep their prior
contact obligations.

The standalone API is `improve_filled_copper(pipeline, ..., kicad_cli=...)` with
`FilledCopperFeedbackOptions`. It verifies fresh inputs itself; it does not accept
a caller-supplied connected flag or a stale native certificate.

## Final native dead-end cleanup

The final CLI pass can trim a mixed set of ordinary dangling tracks and vias.
The source, prepared prefix, macros, critical nets and required via arrays stay
protected. Existing track-only API callers cannot authorize
via removal without explicitly supplying via ownership guards. Via cleanup on
zone nets is unsupported.

Every proposal preserves exact explicit pad partitions. Off-centre land entries
may anchor a trim only when the complete existing endpoint disk lies inside the
pad. Grazing or width-only contacts permit a whole-segment removal trial, never
an invented cut point; any lost pad connection rejects that trial.
Removing a one-layer via may expose its old launch leg. At most four fresh native
rounds form one transaction, and only a completely clean matched refill commits.
Stale evidence, protected exposed legs, unsupported findings or an exhausted bound
restore the original board and evidence. The route report records cleanup
digests, removed/replaced occurrence counts, artifact paths and rejection reasons.

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
   Dependency rerouting uses the pipeline's unified router options. Every prefix
   occurrence, including changed-net critical contacts, must survive the reroute
   exactly or the transaction fails. A dependency transaction cannot overwrite
   the main run's saved replay inputs.
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

Source, macro, required via-array, critical, held bypass, plane-contact and
accepted bridge ownership is protected using occurrence-counted geometry, not a
designator blacklist. Electrical topology, minimum widths, copper weights,
authored keepouts and fixed poses are never silently edited. Native connectivity
rechecks displaced signals.

This implementation generates surface bridges only, not new layer transitions,
automatic pour redesign or a general RF/power solver. It can make partial
progress and still report failure. A clean native route is **not production,
independent CAM, RF, power, thermal, battery or prototype qualification**.

## Reports and stop reasons

The placer validates later bypass moves with rigid phase locks bound to their
current legalized/refined poses. Source-fixed constraints and the final legality
gate against the original source and actual caller session locks remain intact.
This prevents temporary macro locks from silently blocking semantic capacitor
placement; it does not unlock a fixed capacitor or rigid companion.

The default opposite-surface bypass policy requires complete explicit supply and declared return-net
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
unchanged. `DecouplingRouteResult.supply_return_witnesses` remains the final canonical
snapshot; `admitted_path_witnesses` separately records adopted original paths.
Placement retains exact shared/generated/source copper while testing disposable
poses, freshly rechecks all prior association identities at the new poses, and
rolls back if normal regeneration loses any identity. Public placement output
remains unrouted; it does not promote generated bypasses into immutable macros.

`filled_copper_feedback` records input/candidate hashes, bridge expanded-state
counts, dependency searches, displaced nets and fresh native evidence per trial.
Failed bridge searches retain at most 32 blockers sampled from 16 rejected edges.

Native stop reasons distinguish `verified`, `disabled`, `trial_budget_exhausted`,
`stagnation`, `physical_violations`, `native_violations`,
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
validated via-free surface escape to their package collar. The same assignment
and physical DRC gates apply; this fallback does not waive via-in-pad
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
are not repair witnesses. Deduplicate
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

Differential tuning retains the original bounded attempt first. Only a zero-addition,
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
shares at most four local searches (100 um steps, 1 mm detour, 4000 states
each) across supply, return and return targets. Existing-launch tails and
bounded inner bridges share this allowance. When every ordinary source launch
is absent and the fanout maze option is enabled, a circular array may use
remaining slots to reach farther actual four-land cells. At most the site cap
of nearest land origins contributes four cells each; at most the site cap of
nearby cells receives exact via checks, and accepted launches remain within
the original launch cap. Incomplete cells confer no access. Original source
tracks/vias are preserved; every complete supply/return path, protected owner,
created-edge coverage and physical gate is checked before adoption. A legal
local candidate alone does not establish standard-pipeline or board closure.

After ordinary fanout passes its materialization gate, one explicit late bypass
completion pass may retry only its original pending opposite-face source links,
intersected with the original net filter. It requires newly generated ordinary
copper and never repeats a completion round or reroutes ordinary/critical/area
copper. Existing per-link proposer bounds remain authoritative. The pass and
selected/completed/remaining links, generated counts and rollback reason are
recorded in `FanoutResult.late_bypass`, separately from routing closure.

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

## Per-association post-fill observations

Native plane verification reports `decoupling_assessments` for each declared bypass.
It uses the existing positive-area filled-fragment contact grouping and exact
source/export binding to observe supply and target-component return continuity.
Split fragments, missing return terminals, ambiguous pads, stale exports, unknown
native objects and unavailable geometry remain open or explicitly unsupported.
These observations do not replace local bypass path witnesses, waive routing gates,
or qualify high-frequency loop impedance. They are a diagnostic foundation for
plane-assisted routing; bypass reservation and admission still require explicit
local supply/return paths.

Opposite-surface bypass proposals can use a bounded inner-layer bridge with
two ordinary through-vias when direct surface tails are blocked and the source
permits those layers and via counts. All eight destination-launch trials share
the existing complete-transaction allowance; four bounded local maze tails are
shared by supply and return. Both explicit terminal paths must still validate.
Before admission, an unused branch may be removed only from the current
proposal's generated suffix. Every input track/via object, including immutable
macros, required arrays and previously admitted exits, stays intact.
The trimmed candidate receives a fresh clearance index, both complete path
witnesses, protected-witness checks and physical DRC. A failed trim rolls back.
Native fill observations remain separate from this explicit-path admission.

Local paired through-via preparation first tries the established short collars.
If none produces a legal complete surface/via/return/other-layer transition, it
tries four longer collar distances (4 mm, 4.5 mm, 5 mm and 6 mm) to reach beyond
opposite-side component banks. Joint channel search samples both families, as
described below. This preparation adds no local maze allowance and
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

Paired surface and via searches check the exact mandatory first spine step
against the fully escaped board before spending a maze slot. A legal collar
whose departure is obstructed cannot start that search at that pitch. The
existing 128-combination, search-count and expanded-state limits remain; this
check adds no lookahead search or geometry exemption. Endpoint approaches and
complete candidates still pass the ordinary profile and DRC gates.

Collinear paired lands may use either transverse lane order; both complete
collars are checked against exact geometry. Joint paired-via search retains
short and outer collar families inside the same 16-port heading/sign domains,
128 candidate combinations and per-tier maze limits. Short/short, outer/outer
and mixed endpoint families share those slots. Within each family, source site
rank and target-facing headings precede collar preference and distance. This
allows an outer escape when a locally legal short collar is trapped behind
opposite-layer copper. Local terminal preparation keeps its short-first fallback.
No signal polarity, layer permission, width, skew, return or via gate changes.

For actual round breakout lands aligned with a cardinal outward heading, one
existing heading/order/length slot first tries an inward-converging departure.
Both lands approach their midpoint, turn to the same side as a narrow pair,
then widen to the requested port pitch beyond the farther land. The actual
source breakout widths and pair gap determine the stems; the normal joint
clearance checks still include every other land, keepout and held conductor.
A legal proposal replaces that slot's direct/wide fork rather than adding a
port or maze allowance. Non-round contacts, arbitrary via endpoints, absent
breakout profiles and too-short slots retain their existing behavior. This
is a geometric proposal family, not a manufacturer impedance certificate or
permission to transplant a reference board's widths or layer stack.

When neither analytic collar family yields a complete paired transition,
the recessed-land fallback checks via sites before searching their surface
lead-ins. Cardinal headings, the existing 0.5–6 mm local extent and transverse
offsets derived from terminal pitch cover staggered banks beside opposite-face
bypass pads. At most 16 legal sites, 32 local path calls of 2400 states and
eight complete collars are considered. Both lane orders remain explicit,
and vias must clear every spanned copper layer, pads and drill envelopes.
The existing 16-port heading/sign domains, 128 pair combinations and joint
search state limits still bound the middle route. All new collars pass the
normal profile, return-via and atomic physical acceptance gates. This fallback
does not release an existing bypass or power owner to make room.

Local pin-access A* breaks equal estimated-cost ties by remaining distance
to the goal. This avoids flooding an entire shortest-path rectangle before
reaching a recessed land; the metric, exact geometry checks and state cap are
unchanged.

Cross-surface bypass refinement ranks the existing analytic launch sites by
local distance before its unchanged per-terminal site cap. Coarse outward
launch order remains unchanged. Inner-bridge destination refinement uses the
same local ranking. This prevents a long blocked outward ray consuming the
entire local allowance; it introduces no new site family, launch slots,
maze states or complete-proposal budget. Analytic inventory materialization
is bounded to eight times the site cap plus four package-derived sites.
All old paths, original-edge ownership and complete supply/return gates remain
mandatory; a rejected proposal contributes no copper.

### New bypass proposal tails

A complete bypass witness can traverse only a subsegment of a generated track.
Before adopting a cross-layer proposal, discard its wholly unused new tracks
and vias and clip new track endpoints to their witnessed outer step span.
Otherwise an unused extension can survive the ownership check and become a
native dangling-track finding. This operation is confined to the uncommitted
proposal suffix; every input/source occurrence, including a partially used
shared feed, remains identical. Rebuild the candidate and freshly validate
the complete supply/return paths, all protected original witnesses, full
new-edge coverage and whole-board DRC. A failed revalidation rejects the
proposal. No accepted owner is trimmed retroactively.

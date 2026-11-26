# Placement contracts

These are implemented planner rules. Public commands and report interpretation
are in [layout workflow](layout-workflow.md); pending improvements are in the
[routing open work](routing.md#open-work). No board-specific coordinates belong in
the shared placer.

## Directional escape spacing

1. Build orientation/side-aware access profiles from actual physical pad shapes
   and package/courtyard envelopes. Count connected SMD pads, including power
   and ground. Through-hole pads already have a layer transition. Unconnected
   pads remain obstacles but do not demand an escape. No 32-pad cutoff.
2. Assign demand to the nearest facing package side. Estimate via-bank rows from
   tangential pad span and the stricter of copper-via and drill pitches. Permit
   bounded lateral staggering by one via pitch at each end of that span, capped
   by the package-side envelope; do not assume unlimited spreading. Use
   actual net widths/clearances, via dimensions, allowed layers, dedicated-plane
   policy and zero-via restrictions. Do not divide surface escape demand by the
   number of copper layers; those vias still need physical landing space.
3. Score only overlapping, facing same-side envelopes. Budget the two sides'
   demanded access depth, a configurable transit lane and placement margin.
   A blocker need not itself be a dense IC. Two-terminal passives do not demand
   their own via banks but still obstruct other packages. Close companions are
   exempt within their proximity unit; rigid macro interiors are exempt because
   they have their own fixed geometry. Other constrained IC pairs still report
   an unmet channel if their constraint prevents relief.
4. Prioritize channel deficit ahead of HPWL in local and final candidate scores.
   Preserve hard legality and relative constraints. Spacing is a soft heuristic:
   fixed or crowded designs may retain a deficit, but must expose it as a
   warning with component pair, direction and measured/target gap.
5. Translate whole rigid/proximity units, including decouplers, in bounded
   deficit-derived grid steps into unused material. Recheck complete legality
   and non-increasing coarse congestion before accepting a move. Preserve fixed
   poses and macro rotations. No example references or coordinates in the compiler.
   Score proposals before complete geometric validation, so non-improving
   trials do not repeat the full-board checks. Every selected move still
   receives complete legality validation. Progress identifies the active
   escape unit during this bounded stage.
6. Keep joint ordinary/critical/GND package-access preflight before area routing.
   A zero heuristic deficit is not proof that both packages can escape: only
   actual reserved geometry and existing native acceptance gates establish that.

Default margin is 0.5 mm per demanding side and one transit lane. Dimensions,
lane count and bounded spreading passes/movement are planner options, not
global fabrication limits. Rotate/mirror profiles with the actual footprint,
including already supported 45-degree poses. Profiles are cached within a
board-bound model; new board/rule snapshots get new models. Keep only the latest
XY pose/pair result in incremental caches so placement trials do not accumulate
an unbounded position history.

Cross-component drill checks first compare cached geometric envelopes containing
the actual lands, drills and courtyard. Distant envelopes skip narrow geometry;
nearby pairs retain the same checks on both faces. The cache contains geometry,
not a placement acceptance decision, and includes the exact footprint and pose.

## Basis and limits

The floorplanning-before-escape principle is consistent with
[TI's PCB escape-routing guide](https://www.ti.com/lit/an/sprad13a/sprad13a.pdf).
Its BGA-specific patterns are not copied into LQFP/LGA algorithms. Existing
clearance, mechanical and package-access checks remain authoritative. The
axis-projected model is deliberately conservative at diagonal/corner channels;
it does not solve exact pin-access allocation, SI, impedance or power integrity.

## Supply-domain placement

Supply-net membership and source-pad identity form a derived physical planning
view. Nets, rather than voltage labels, identify domains: switched/filtered
same-voltage nets remain distinct. Multi-domain components participate in each
group. Domain compactness/distribution is a configurable soft objective; locks,
mechanical legality, proximity and escape access remain authoritative.

## Algorithm choices

Ordinary legalization orders components by authored placement priority, weighted
connection degree, then body area. Automatically derived hierarchy groups do
not add their priority or all-to-all clique weight to those ordering keys:
placing a component inside a logical module must not alone give it first claim
on board space over a larger root component. Authored group/component priorities
and other physical grouping hints retain their existing effect. Hierarchy still
contributes unchanged to analytical seeds, group forces, pose-ranking adjacency
and spread metrics. Relative units remain scheduled together, and fixed/rigid
geometry, legal poses and bounded repair allowances are unchanged.

For a pin-relative member whose companions are not placed yet, ordinary-grid
legalization retains nearest-first search for three quarters of its existing
raw probe allowance. If that prefix has no legal pose, the remaining allowance
samples the unused position domain evenly, retaining every allowed orientation
at each sampled site. Small domains keep their complete original ordering.
If a complete orientation cycle exceeds the raw allowance, retain the original
bounded ordinary prefix instead of dropping all candidates.
Nearby successes skip the distant samples; anchored companion searches keep
their existing relative neighbourhoods. The `legalize_wide:REFERENCE` progress
event identifies this bounded coverage. This changes proposals only: exact
geometry, subsequent companion placement and complete relative checks still
decide acceptance. A free site for an IC alone does not certify its bypass unit.

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

## Joint support-bank patterns

**Rust implementation status (checked 2026-11-05):** the current planner accepts
zero support-pattern/package-support trials. Nonzero values return
`not implemented`; the joint support-bank machinery described below is a design
and legacy implementation reference, not an available Rust optimizer. Explicit
source placement trials must still pass complete placement legality and fresh
escape/routing checks. Do not infer preserved dogbones from a legal cap pack.

`PlacementPlannerOptions.support_pattern_trials` opts into joint support
placement after ordinary capacitor refinement and before placement candidate
ranking. It defaults to zero, and the Rust planner reports any other value as
not implemented (the stage was not ported from Python). `plan-layout` and
`route-global` accept `--support-pattern-trials N`; `route-board` does not.
The stage does not move accepted routed/frozen copper. Public output remains
unrouted source poses; proof copper is separate.

The `support_patterns` module groups explicit pin-associated capacitor roles by target
pad, supply and return, without swapping same-rail components or values. A rail
feeding different package balls retains separate local banks, jointly selected
against each other. The physical IR requires one explicit target per capacitor.
It derives finite end-on/paired, staggered, underside-pocket, land-rim and split-bank
instances from actual transformed pad/courtyard dimensions. Legal source sides,
rotations, fixed/rigid membership, macro and required-array owners remain
binding. Opposite-face land-rim banks use actual outer land bounds rather than
requiring clearance outside the front package courtyard; complete placement,
hole and reserved-copper checks still apply. Source-forbidden sides are filtered
before spending raw geometry probes. A fixed bank returns `fixed_or_unsupported` with no routing trials;
unobserved association counts remain unknown rather than an invented zero pass.

Whole-bank offsets use a bounded configuration-space sweep. Each held
same-side courtyard expands into forbidden bank translations; source regions
and board boundaries bound the translation interval. At most 64 x support
events per template expose up to eight free y events each, retaining eight
nearest offsets. Exact polygon, hole, macro and complete-placement checks still
decide legality. Up to four seeds per family receive offsets, interleaved
across families within the same cumulative 512 pose-probe limit. Roughly half
the available probes go to nominal templates and the rest to offsets; no extra
placement or routing budget is created. Bounding boxes are conservative
proposal geometry and never establish infeasibility.

Source-frame templates also translate/rotate the complete existing bank at
0/90/180/270 degrees, subject to each member's authored orientations. They keep
the original per-component roles and relative geometry when fixed neighbors
prevent a compact bank. Legal source frames can receive offsets to free sites
for another mutable bank. The capacitor-to-capacitor pitch uses their actual
pair gap; the front package's courtyard gap does not inflate an opposite-face
bank. Every generated family rejects fixed, rigid, macro and array owners.

Joint selection uses the shared most-constrained-first/forward-checking kernel.
It can move groups simultaneously where neither move is legal alone. Raw bank
pose probes, retained family alternatives, compatibility checks, search states
and complete checks have separate cumulative bounds. By default at most four
groups of four members receive twelve retained alternatives each, with 512 raw
bank probes, 2,048 search states and 8,192 compatibility checks. Group priority
favours unresolved associations. Alternative families share the retained cap.

Each full combination is checked against all held geometry, complete placement
and relative rules. Ordinary exits are freshly reserved before local feeds, or
the caller can supply exact tracks/vias and terminal ports. These objects are
temporary fixed obstacles; final power-only proof copper excludes them. Existing
verified associations and ordinary ports cannot be lost. Both supply and
explicit target-package return paths must exist for every moved association.
Same-face local returns use checked paths on source-permitted layers; opposite
faces retain the standard bounded normal-via proposer. Missing return targets,
forbidden transitions and blocked combinations cannot win through compactness.

The search retains a small Pareto frontier over complete-association count,
actual local path length, via count and the support/contact surface envelope.
Selection prioritizes complete obligations and shorter local paths, then via
count and occupied envelope. These are geometry proxies, not an extracted loop
inductance or PDN model. A final clean-source regeneration must independently
restore every retained witness before returning changed poses. Baseline and
final regeneration add at most two routing passes beyond the complete-check
allowance; each pass retains the existing per-link proposer bounds. A rejected
trial retains no generated copper or moved pose. Reports expose work, family
parameters, unobserved scope/phases and `native_acceptance: false`.

Routing and fresh native gates still follow. This local placement result does
not prove onward area access, global routability, high-frequency loop quality or
board qualification.

## Inline differential-pair components

Recognize an inline component only from explicit differential-pair topology:
two two-terminal nets per group, one terminal on the inline component and one
shared external component per group. Generate deterministic bounded poses near
the external pad-pair centroid segment, with permitted rotations and
perpendicular offsets. Detailed refinement accepts a proposal only when the
complete legality, escape-spacing and placement score gates improve the
incumbent. Package/critical preflight remains required; a shorter span does not
establish impedance or EMC performance.

## Origin domains and bounded seeds

Candidate domains use the signed courtyard bounds at each allowed orientation.
The footprint origin may be at a connector's first pin rather than its centre;
symmetric half extents must not discard valid placements of such footprints.
Sampling unions the orientation-specific domains, and analytical projection uses
the same signed bounds. Board-edge proposals honor the component's declared edge
allowance. Region boundaries contain the courtyard without adding a second board
margin; exact legality still checks the independent physical board edge, actual
polygons, keepouts and all other source requirements.

Exhausting a mutable rigid-group or companion search rejects that placement seed
and records its failure before trying the remaining bounded seeds. Conflicting
fixed geometry remains fatal. If every seed exhausts its search, no partial or
invalid placement is accepted.

When every candidate fails during placement, the route run can save
the deepest failed seed (most held components, earliest seed on a tie) beside
its existing error report. `diagnostic_state` is an inspection-only physical
board consumed by `copper issues` and `copper view`. It contains held poses and
the failed component's explicitly unaccepted analytical target; remaining
components are omitted. The report and state list all three sets. Held poses
may still violate relative constraints and are not certified placement.
If any seed completes legalization but fails the final constraints, the snapshot
instead preserves the complete rejected layout with the lowest relative penalty
(earliest seed on a tie), matching the error details. All components are held,
none is an unaccepted analytical proposal, and the issue locates a violated rule
target or an illegal component when available. This complete snapshot takes
precedence over shallower bounded failures and remains uncertified.
Ordinary bounded-failure snapshots are emitted only for failed searches. A
typed physical hard-macro rejection also retains the exact rejected tracks,
vias and DRC findings, including when a different seed succeeds. Native issues
locate those copper objects, not an arbitrary macro anchor. Its metadata states
whether the overall placement search succeeded; such a discarded-seed artifact
cannot be attached to a later unrelated run error. Neither kind is a
placement/global routing result or a reusable placement checkpoint.

Rigid groups first retain their nearest-first candidate search and ranking. If
that search finds no legal pose, a movable group samples the remaining origin
domain deterministically across its extent. Local and wider trials share the
existing raw probe allowance, including rejected transforms and rotations.
Successful local choices are unchanged when the former local domain fits this
raw allowance (including the usual one-to-four rotation groups). Larger
orientation sets are now capped too; more than 64 rotations can truncate the
former local domain. Fixed groups never enter the wider search. Every proposal
still checks the complete group and held geometry with
the same exact legality predicate. The `cluster_wide:NAME` progress stage
identifies this bounded search, without implying that a legal pose exists.

Each candidate checks its moving members first, then every held pose, against
the same complete trial and resolved group keepouts. This rejects ordinary
collisions sooner without omitting the held-pose checks: a translated keepout
can block an outsider even when every moving member is otherwise legal.

Group ordering keeps all groups with fixed members first, in their existing
name order. Movable groups with any member in a named placement region come
next, before unrestricted movable groups; name remains the tie-break within
each class. This reserves constrained placement areas before unrestricted
groups consume them. It is an ordering heuristic, not a feasibility guarantee
or permission to move fixed members or ignore any region geometry.

Region-only mutable singletons also reserve their scarce domains after fixed
groups and before movable groups. Eligibility requires no relative rule or
rigid membership, and the complete ordinary origin/orientation domain must fit
the existing legalization-candidate allowance. Order is domain size then
reference. Each unrelated scarce singleton uses its normal bounded choice once,
including on boards without rigid groups. A scarce domain whose authored region
bounds overlap a broad regional domain (including transitive overlaps and the
pair's courtyard clearance) instead joins the existing broad reservation search.
Otherwise its greedy central pose can erase every broad candidate before the
joint packer can move it. Fixed owners and rigid members are never promoted.
The broad list keeps its domain-size/name order and existing batching; disjoint
authored regions are not split into separate searches because their access
envelopes can extend beyond those regions. Bounds only select search members;
all poses still pass the same exact mechanical and electrical gates.

After the regional rigid groups, remaining region-only singletons are jointly
packed before unrestricted rigid groups and ordinary components. A broad authored
region can contain only a small usable landing after mechanical obstacles are
applied; postponing it by raw rectangle size lets unrelated components fragment
that landing. The existing local packer filters exact legal domains and checks
the selected combination. References retain deterministic domain-size/name order;
each batch has at most `exact_repair_limit` members and uses the existing domain,
search and heuristic budgets. `region_pack:REF,...` identifies the work. Regional
rigid groups keep precedence, so these reservations cannot displace their
accepted geometry. This also runs when there are no rigid groups. Separate
batches are greedy with respect to earlier batches; this is bounded scheduling,
not a complete global packing search.

A failed joint regional pack reports each observed initial sampled domain's
raw-probe and individually legal-pose counts. An empty domain identifies its
actual component; otherwise the report distinguishes exhausted joint-search
work from no compatible sampled combination with work remaining. These are
observations of the same search, not a second sampling pass or a proof about
unsampled continuous poses. Failure snapshots remain inspection-only.

These reservations are candidate-owned mutable poses, never additions to source
or phase locks. Ordinary legalization revalidates them against the final held
fixed/rigid geometry, then carries them in its accepted map and mutable repair
order instead of repeating their greedy turn. Later exact repair and refinement
can move them within their original region, side, orientation and clearance
constraints. Revalidation failure rejects that seed; it cannot relax geometry.

## Ordinary companions of movable macros

A movable rigid macro also accounts for ordinary parts reachable through
authored pin-to-pin maximum-distance links. Other rigid groups and source-fixed
parts are boundaries. A small driver and bypass can fit a pocket that strands
its larger external switch; legal macro geometry alone is insufficient in that
case. The selected macro pose must have a sampled exact joint placement witness
for a bounded set of its currently unplaced ordinary companions. At most
`exact_repair_limit` members are selected by shortest pin-link hop, then larger
footprint area and reference name. Earlier hops retain the selected dependencies;
members beyond that bound keep their ordinary placement path and every final
constraint. `cluster_companions_deferred:NAME:COUNT` exposes that remaining work.

Each existing nearest/wide macro pass retains the original legal-pose ranking.
At most `exact_repair_candidates` poses are checked with the existing local
packer: half retain the nearest ranked choices, while the rest span the remaining
ranked legal domain. Thus one blocked pocket does not consume the complete
shortlist. No companion packing occurs for every raw macro probe. There are at
most two passes, or 56 local packs at default options; each pack retains its
existing domain/search limits and exact gates. Repeated local packs can still
make this materially more expensive than macro-only placement. This is bounded
sampling, not a complete search of all coupled poses.

Trials operate on local placement maps. Failed trials retain no accepted macro
or companion poses. Successful companions become mutable reservations, not rigid
members or source locks, and later macro candidates preserve their geometry and
active relative constraints. Ordinary legalization revalidates and consumes
those witnesses once; its support pack skips already-held components. Failure
reports the sampled legal macro count and bounded witness trials, explicitly
distinguishing missing sampled witnesses from geometric impossibility. Complete
placement, macro copper and routing verification remain mandatory.

## Complete small relative repair units

Within a proximity unit, anchored companions are scheduled by their smallest
active maximum-distance radius squared divided by courtyard area (body area
when no courtyard is supplied). This favors a larger part with slightly more
reach over a tiny part with a tighter raw radius, preventing small supports
from taking the only pocket that can hold a large companion. The ratio uses
exact rational arithmetic; equal ratios retain the earlier distance, link-count,
body-area and reference-name tie breaks. Unit order and anchor choice are
unchanged. This is a scheduling estimate, not a legal-domain count: region,
height, side, rotation, material and clearance checks still apply to every pose.
It adds no candidate allowance, packing stage or repair members.

When greedy placement requests exact repair, reserve the failed component's
complete maximum-distance unit before filling the remaining repair slots with
the existing ranked blockers. This applies only when the complete unit fits
the current repair-member limit, every member has a target and is movable,
and no member belongs to a rigid group. The unit also includes supports whose
ordinary turn has not arrived yet; a partial repair can otherwise move its
anchor into another pocket that strands those supports. Newly accepted members
enter mutable placement order and are skipped on their later ordinary turn.
Fixed, rigid, missing-target or oversized units retain the existing blocker selection. A proximity anchor can
need relocation even when it is not a physical blocker, for example when an
IC and its decoupler occupy opposite faces.

This changes only membership of the existing bounded local search. It adds no
repair stage or budget, and leaves successful greedy choices unchanged. The
existing exact geometry and relative-constraint checks still decide every
trial; failure leaves the held base untouched. Reserving a complete unit can
leave fewer slots for unrelated blockers, so it is not a feasibility guarantee.

Ordinary repair emits `repair_pack_started` with the bounded selected references,
then `repair_pack_finished` with sampled/legal domain counts, the first empty
domain, remaining combination-search budget, and coarse/fine pass information.
Counts describe the final pass; a region-refinement pass can retain earlier
legal poses in addition to its newly sampled poses. A null remaining budget
means combination search was not reached. No candidate poses are dumped.
Failed ordinary repairs persist the same `bounded_packing` record in native
failure metadata; `copper issues RUN --issue E1 --json` exposes it as bounded
structured details independently of the shortened issue title. This is
observation of the existing search, not another placement trial or proof of
geometric impossibility.

Within the existing local pack, variable ordering prioritizes the largest
connected component of its internal pair-relative rules before independent
neighbours, retaining minimum-remaining-domain ordering within that component.
This prevents a narrow independent neighbour from repeating a coupled support
search first. Independent blockers remain movable in the same joint search and
participate in exact forward checking; no new pass, candidates or budget are
added. The ordering is a bounded-search heuristic, not a completeness guarantee.

A held peer bounds the local candidate neighbourhood only through a maximum
distance. Minimum distances are exclusions, and an alignment still leaves an
unbounded axis. Pin-relative greedy and joint searches therefore retain their
existing bounded wide-coverage samples when no held maximum-distance peer
exists, including when nearby supports have not been placed yet. All minimum,
maximum and alignment checks still apply to every accepted pose; no additional
raw probes are allowed.

When this repair removes a mutable pin-distance anchor from the held base,
candidate generation must not forget its companions. Validated removed poses
remain candidates; related domains are constructed anchor-first, with bounded
ordinary-grid coverage when no peer is held. A companion can use up to half its
existing raw probe allowance for relative proposals around a spread sample of
already constructed peer-domain poses (at most `exact_repair_candidates`
anchors). Unused allowance stays with ordinary proposals. Domains still retain
at most `4 * legalization_candidates` poses, and the existing search/heuristic
allowances are unchanged. The caller's original reference order is restored
before search, preserving its tie breaks. This is finite domain propagation,
not a new recovery pass or a completeness guarantee. Fixed/rigid references are
excluded; all original base constraints and exact pair/geometry gates apply.

## Named-region local packing

Initial local packing can supplement ordinary candidates for a mutable component
with a named region and no relative rules. Support-edge proposals use its actual
transformed courtyard, the ordinary component gap, and same-side held or member
target courtyards. They include edge alignments and conservative corner sites;
the region bounding box is only a proposal filter. The complete legality gate
still checks the actual region polygon, board edge, side, allowed rotation,
keepouts, macro copper and held geometry. Fixed and rigid members gain no new
motion. Components without a named region retain their existing candidate path.

Legal ordinary sites remain in their original order. Contact sites fill only
unused retained-domain slots; no contacts are added when the ordinary domain
already fills that cap. Each probe, including duplicate ordinary proposals,
spends the original per-member raw allowance. Default options provide 6,144 raw
probes and 384 retained sites per member, 172,032 local search checks and the
existing one-third heuristic allowance. These derive from planner options and
are computational limits, not fabrication or electrical requirements. Source
clearance, region size and repair-member limits are unchanged.

A repair may supply a finite tuple of its removed members' current held poses
as proposal seeds. Validate source reference, footprint, side, allowed rotation,
mutable membership and legality against the current held base. Such poses help
suggest sites; they do not become immutable obstacles or authorize releasing
any held-base geometry. Domains are freshly built for the current source,
targets and held base. Private regional fine continuation binds that exact
context, including the proposal tuple, and uses only residual search/heuristic
checks and unused original member probes. Zero residual prevents continuation.

The existing pad-associated maximum-distance fine path retains its prior
behavior and allowances. A mixed pack entering that legacy recursion rebuilds
its domains; initial contact sites of other region-only members are not carried
into that path. Mixed-pack closure is therefore not established by this change.
A bounded placement failure is not evidence of continuous-domain infeasibility,
and legal packing does not establish escape, routing, SI or manufacturing
qualification.

## Exhausted fixed-anchor packs

Keep whole-group packing when it succeeds. Only after an oversized group joined
through a fixed anchor exhausts its bounded search, split independently
constrained movable groups and retry the existing local repacker. Preserve fixed
anchors, rigid geometry and every relative rule. A failed search never permits a
constraint relaxation.

## Authored additional courtyard gaps

The ordinary fallback is 0.5 mm beyond the footprint courtyard. A source
`courtyard_gap` hard constraint can specify a nonnegative additional minimum
for selected members. Pair clearance is the larger of each member's authored
minimum or its fallback when unannotated. An unannotated neighbor therefore
keeps the default. Inside a shared audited rigid cluster, its internal contract
replaces the fallback; explicit member minima can tighten that contract.

The shared physical resolver controls legality, analytical separation, fine
search support edges, region contact proposals, pair forward checking and
decoupling search extents. Search and repair allowances remain unchanged.
Default source checking, physical DRC and native export use the same member
policy. Local helpers may still accept a caller-specific fallback; a pose-only
helper result does not alter the original board's default DRC policy.

Zero additional clearance allows boundary-only contact while requiring disjoint
courtyard interiors. Nonrectangular simple rings use exact integer triangulation
and strict projection tests; no epsilon or negative clearance is used. This
pair-only test leaves existing macro, keepout and assembly boundary predicates
unchanged. Copper,
pad, mask, paste and assembly/process requirements remain independent.

Native courtyard rules reproduce the resolved pair floors, but native geometry
is a complementary gate. KiCad 10.0.6 sampled controls matched the expected
policy cases, while a 1 um courtyard-interior overlap at zero additional gap
produced no native finding, with both 10 um and zero drawing strokes. Native
reported gaps also depend on courtyard drawing stroke. Retain the exact
Physical-IR placement/interior check; a native-only pass cannot waive it.
No epsilon, negative clearance or drawing-geometry modification is introduced.

## Footprint-owned through-drill obstacles

Incremental placement treats accepted footprint NPTH/PTH drills as physical
obstacles on both faces. Candidate and held drill owners are checked in both
move directions against foreign courtyards and copper, using the same oval/
round drill envelopes and board hole clearance as physical DRC. Courtyard
nonpenetration retains the existing positive guard used for mechanical holes.
Only accepted held poses supply peer geometry; unaccepted source seeds do not
become obstacles. Fixed/rigid constraints and search allowances remain unchanged.

Own NPTH copper checks honor the existing component-scoped hole rule. That rule,
net labels and solder mask never waive a foreign component's drill clearance.
Own PTH annuli and own courtyard are exempt from the foreign-obstacle test;
other own-PTH land checks remain unchanged. Slot-process, native quantization,
assembly and CAM qualification remain independent. Bounded caches retain only
immutable footprint and exact transformed pose geometry, never legality answers.


## Discarding an exhausted seed

The default deterministic seed count remains `max(2, candidate_count)`. An
explicit positive `seed_count` (`--placement-seeds` on `plan-layout` and
`route-global`) bounds attempts instead.
The private typed failure from exhausted mutable legalization/local repair,
or exact physical DRC rejection of a resolved hard-macro trial at the
decoupling-access preflight, discards one attempt and permits the next already
planned seed. A rejected later seed cannot erase a prior accepted candidate.
The strict public macro materialization API still returns an error for that
same copper; only the planner's typed trial API distinguishes it from malformed
source and structural errors. No rejected copper enters routing. Each attempt
builds its own tentative placed map from source/fixed intent and keeps the
existing per-member, local-repair and search bounds. No partial attempt is
promoted, no budget is restarted within an attempt, and no extra seed is added.

Unrouted-source/unknown-lock/corridor prechecks, invalid fixed geometry, rigid
contracts, malformed source and unexpected `PlacementAlgorithmError` failures
remain fatal. Retry classification does not inspect error text. Retained
candidates carry an immutable `failed_seed_attempts` statistics tuple with the
seed, failed reference, accepted count and reason, including failures after a
successful seed. The report omits this field when empty. When all attempts fail,
the final error retains all bounded reasons and the deepest accepted-count
message (earliest seed on a tie). Finite attempts never prove infeasibility.


## Unrestricted bypass sides

For a pending movable bypass with no authored side restriction, refinement
enumerates both board faces even when the target and incumbent are on the
same face. Ranked side domains are interleaved within the configured legal
pose checks, so one face cannot consume every trial. An already accessible
incumbent keeps the ordinary ranking preference. Explicit side, fixed pose,
assembly and macro constraints remain hard; opposite-face admission requires
fresh supply and return paths and full regeneration before adoption.


Bypass pose refinement skips its disposable baseline routing when every
associated capacitor is fixed by source/session constraints, rigid membership
or a required via array. Unrouted-source and macro materialization checks still
run first. This is a no-move optimization, not bypass-connectivity evidence;
the standard package-access and native routing gates remain unchanged.

Required exposed-pad via arrays reserve their actual through-via annuli during
placement, including against opposite-side pads. Moving an array owner checks
all held neighbors; moving a neighbor checks every held array. Only the declared
owner land may overlap an annulus, including on the same net. Foreign copper
and drill clearances also remain binding. This reservation does not place or
reduce the array: full materialization and native checks still validate every
site. A failed materialization retains its posed board for `copper issues` and
`copper view`.

Placement progress identifies decoupler baseline routing, each capacitor's
refinement and bounded legal-pose trials, and final regeneration. These events
are telemetry only; witness ownership, rollback and the trial budget are
unchanged.

`decoupling_placement_trials` bounds copper-access pose trials per capacitor
(API/default CLI: 64). `--decoupling-placement-trials` exposes this budget;
the quick, balanced and thorough routing profiles select 4, 16 and 64 unless
explicitly overridden. Zero disables the optional relocation heuristic after
fixed macro/array validation. It does not remove source associations or waive
package access, source distance, routing, DRC or native-fill checks. The budget
is part of placement-checkpoint identity and appears in routing work limits.

`--placement-refinement-passes` and `--escape-spacing-passes` expose the existing
optional improvement passes (both default to 2). Zero retains the legalized
poses without those optimization passes. An explicit seed budget of one with
both passes zero is a quick feasibility attempt, with fewer opportunities to
improve routability. Fixed/rigid constraints, legal geometry, relative intent,
macro/array materialization and later package/routing gates remain binding.
These settings are part of checkpoint identity and are recorded in work limits;
use matching settings for checkpoint reuse. A failed small budget is not proof
that the board is infeasible.

Pad-only copper keepouts use exact closed-shape contact in placement and
physical DRC. A zero-distance clearance query is not an overlap predicate:
rectangular corner crossings must be detected even when neither first vertex
is contained. Concave notches remain usable; no convex-hull substitution or
epsilon clearance is used.

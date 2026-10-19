# Placement contracts

These are implemented planner rules. Public commands and report interpretation
are in [layout workflow](layout-workflow.md); pending improvements are in the
[routing backlog](routing-backlog.md). No board-specific coordinates belong in
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
   poses and macro rotations. No example references or coordinates in pcbir.
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

`PlacementPlannerOptions.support_pattern_trials` opts into joint support
placement after ordinary capacitor refinement and before placement candidate
ranking. It defaults to zero. The normal `plan-layout`, `route-global` and
`route-board` commands expose `--support-pattern-trials N`; N is the complete
combination-check allowance per placement seed, not a fabrication constraint.
The stage does not move accepted routed/frozen copper. Public output remains
unrouted source poses; proof copper is separate.

`support_patterns.py` groups explicit pin-associated capacitor roles by target
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

Normal package/contact preparation, distribution-area planning and fresh native
gates still follow. This local placement result does not prove onward area
access, global routability, high-frequency loop quality or board qualification.
Reference rationale and remaining consumer gates are in the
[pattern research and implementation plan](power-area-planning-plan.md#pattern-based-support-placement-research-2026-10-17).

### Joint support, contact and power-area trials

When `route-board` enables both support-pattern trials and power-area planning,
the placement search now runs the normal native-backed power preparation on
each complete pose assignment. The caller's global routing settings, fanout
settings and pair-state bound are forwarded. Critical routes, ordinary exits,
local contacts, returns and area alternatives are regenerated from that trial's
clean source; previous filled evidence and generated contacts are not reused.
The source inventory, pin associations, fixed/rigid poses, macros and required
arrays remain binding. A missing native verifier is reported as
`native_tool_required`, with no moved pose.

This mode starts before contact/ordinary ownership freezes. Caller-supplied
pre-routed reservations are currently reported as
`external_support_reservations_unsupported`; they are neither discarded nor
passed through the clean-source macro boundary. The normal preparation instead
creates its own early ordinary reservations when configured. Existing local
supply/return witnesses must survive; plane-assisted continuity additionally
requires the ordinary preparation's fresh native gates. Native continuity
does not replace high-frequency loop qualification.

All group preferences vary together within the existing complete-check cap;
the expensive native checks are not all spent changing only the final DFS
group. Rejected assignments, pair checks and search-state counters survive
between samples. There are at most `maximum_complete_checks + 2` full power
preparations, including baseline and mandatory final regeneration. Each uses
the configured preparation bounds; these are nested bounds, not one shared
maze allowance across different placements. Failure of the final regeneration
rolls back every moved pose.

The result still returns unrouted source poses. `power_trials` records the
source digest, critical results, area/contact plan and native outcome of each
preparation. `power_preparation` retains the final scoped native evidence and
`signal_scene` its exact board; `proposal` is absent in this mode. Whole-board
`native_acceptance` remains false. `power_native_acceptance` describes only
the selected power scope. Standard downstream preparation runs again on the
selected placement before routing admission.

## Inline differential-pair components

Recognize an inline component only from explicit differential-pair topology:
two two-terminal nets per group, one terminal on the inline component and one
shared external component per group. Generate deterministic bounded poses near
the external pad-pair centroid segment, with permitted rotations and
perpendicular offsets. Detailed refinement accepts a proposal only when the
complete legality, escape-spacing and placement score gates improve the
incumbent. Package/critical preflight remains required; a shorter span does not
establish impedance or EMC performance.

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

The existing deterministic seed count remains `max(2, candidate_count)`. Only
the private typed failure from exhausted mutable legalization/local repair
discards one attempt and permits the next already planned seed. Each attempt
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
same face. Ranked side domains are interleaved within the existing 64 legal
pose checks, so one face cannot consume every trial. An already accessible
incumbent keeps the ordinary ranking preference. Explicit side, fixed pose,
assembly and macro constraints remain hard; opposite-face admission requires
fresh supply and return paths and full regeneration before adoption.


Bypass pose refinement skips its disposable baseline routing when every
associated capacitor is fixed by source/session constraints, rigid membership
or a required via array. Unrouted-source and macro materialization checks still
run first. This is a no-move optimization, not bypass-connectivity evidence;
the standard package-access and native routing gates remain unchanged.

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

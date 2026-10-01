# Detailed-search policy reporting

`DetailedNetResult.search_policy` records requested and effective **detailed**
layer/direction costs, whether neutral-cost fallback was attempted/selected,
the compared search failure/overflow pairs and their `scored_nets` scope.
The route-board JSON exposes it on each entry under `detailed.nets`.
It does not describe global-guide costs, change any routing cost, or certify
the resulting copper.

The existing fallback reruns the invocation's whole ordinary-net scope with
zero detailed layer/direction costs when a preferred-cost search leaves an
ordinary net open. It selects neutral copper only when the failure/overflow
pair improves. An unsuccessful or equal-score fallback retains the preferred
route and reports that neutral was attempted but not selected. Explicitly
requesting zero costs is not itself a fallback. Localizing this mechanism
remains R6b work; this change only makes its behavior observable.

Failure/overflow pairs describe the search **before** physical-land closure
and native checking. They can include a deferred zone net in `scored_nets`;
the zone does not trigger fallback and its own `search_policy` is null.
Final connectivity comes from normal closure/native/independent checking,
not these telemetry values.

Policy belongs to each net result so a committed subset repair retains the
new policy on repaired nets and the previous policy on untouched nets.
There may be no single effective policy for the whole board. Subset telemetry
names only its scored scope; it does not retroactively relabel unrelated nets.
Physical-land closure retains this provenance when remeasuring final metrics.

Telemetry does not enter physical metadata, candidate comparisons, geometry
digests or route fingerprints. Reports can have different bytes without any
copper change. Tests check exact board/metric/fingerprint equality against a
direct neutral control, preferred/rejected/zone cases and mixed-policy local
repair. A null or absent policy in historical results means **unavailable**,
not an inferred zero-cost policy. The pass-13 full rerun started before this
report-only addition and therefore cannot report which fallback won.

## Physical guide-deviation cost

Guide exposure is measured along the entire planar edge against the union of
same-layer segment capsules (guide half-width plus search margin) and access
squares (search margin). The cost is 50 units/mm outside that union, in addition
to the baseline 10 units/mm and layer/direction preferences. Partly covered
edges pay only for their uncovered length, even when the destination is inside.
Overlaps do not double-count coverage. Straight-edge subdivision or routing-pitch
changes no longer add fixed 50-unit charges. Intersection parameters use floats;
rounding once per edge can change the sum by at most a nanometre per additional
edge (50 integer micro-cost units), not 50 whole cost units.

A via has no planar run length. Only a transition from guided to unguided
membership pays a separate 50-unit guide-exit event; transitions already outside
do not repeat it. Existing via/bend/congestion events remain separate. Projected
guide search projects both membership and exposure across **allowed** layers;
it does not open the reserved plane or override explicit net-layer restrictions.

`guide_deviation_count` remains an endpoint/edge diagnostic. It is not the new
wirelength-based cost, a grid-invariant quantity or a physical-rule finding.
The first-feasible guided-stage policy and whole-scope neutral fallback still
have their separately tracked R5/R6b limitations.

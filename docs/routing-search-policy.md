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

## Search-work counters

`DetailedSearchStats` counts bounded A* searches. One *call* searches for one
terminal connection in one stage; stages run in order: the guide corridor,
wider corridors (`progressive_guides`), the corridor projected onto other
allowed layers, then the whole board. Each call ends as `found` (counted per
stage), `budget` (it reached its state budget) or `no_path` (its queue
emptied). `failed_expanded_states` totals the states expanded by calls that
found nothing: work that produced no copper.

Two scopes are reported in the route-board JSON under `detailed`:

- `search_work` totals every search of one `route_detailed` call, including
  rejected passes, repair trials, probes and the neutral-cost fallback rerun.
  A nested `route_detailed` call is also counted by its caller.
- `nets[].search_stats` describes the attempt that produced that net's
  reported result. A memo hit (see `search_reuse`) returns the original
  attempt, with its original counts.

Each executed `detailed_net` progress event adds `search_seconds` and
`search_stats`; reuse hits carry neither, so summing these events over a
`routing.log` counts every executed search once; `python -m
pcbir.routing_benchmark summarize RUN` reports that sum as
`work.detailed_search`. Searches outside `_search_detailed_net` appear only
in `search_work`.

Counts are deterministic for fixed inputs. Like the policy telemetry above,
they never enter equality, physical metadata, candidate comparisons, route
fingerprints or acceptance. The search memo lowers `search_work` without
changing copper, so report bytes may differ between otherwise identical runs.

## Guide-distance heuristic (opt-in, off by default)

`guide_distance_heuristic` (`--guide-distance-heuristic`) adds the guide
deviation that any path to the target must pay to the maze search's lower
bound: 50 units per nanometre of `GuideDistance.exposed_length_bound_nm`, the
smaller of the straight distance and both endpoints' distances to the union of
every layer's guide capsules and access squares. `_queue_estimate` inflates
only the octile term in weighted passes; the guide bound is never inflated.

Measured on 2026-10-20 (local, unprofiled, same inputs and settings as the
make recipes):

- Paired replays of 38 whole-board searches on identical inputs: identical
  optimal cost at 100 % weight, median 0.77 of the states (0.50–0.94), and 4
  of 21 searches that exhausted 20,000 states completed within that budget.
- Whole runs:

| Case | Vias | Track mm | States expanded |
| --- | --- | --- | --- |
| full-vertical candidate-00 | 46 → 62 | 1,572.3 → 1,587.8 | +10.0 % |
| full-vertical candidate-01 | 51 → 51 | 1,640.7 → 1,640.3 | −9.9 % |
| full-vertical candidate-02 | 56 → 65 | 1,506.7 → 1,599.2 | −58.8 % |
| cm4 | 23 → 23 | 587.6 → 587.6 | −5.3 % |
| nrf52 | 10 → 10 | 317.5 → 317.5 | 0.0 % |

Unrouted nets were unchanged in every case. Total work fell 17 %, but routed
quality never improved and twice became worse, so the option stays off. The
search budget also filters quality: a net whose detour exceeds it is retried
by rip-up and repair, which can find better copper than a long whole-board
detour accepted in a normal pass. A faster search accepts more such detours.

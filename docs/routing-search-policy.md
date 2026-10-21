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

## Global tiles at the board edge

The global router places tile centres half a tile (2.5 mm) in from the low
edge of each axis and steps by whole tiles. A board whose size is not a
multiple of the tile therefore ends with a partial tile, and its last
centre can sit up to a full tile from the far edge. On the 11.9 mm-tall
MySRaspiGW boards of the D3 benchmark, a module pad in that 4.4 mm strip
was beyond pin-access reach of every node, so its net had no global route.
When a pin has no global access, the graph is rebuilt once with edge
strips: where the last centre is more than three quarters of a tile from
the edge, one more row (or column) of centres sits in the middle of the
uncovered strip. The strip graph is kept only if fewer pins lack access.
Edges along that row cross only the strip, so their capacity counts the
strip's width rather than a whole tile.

Boards whose pins all have access keep their grid, and so their routes.
Adding the strip row everywhere changed the grid of 45 of the 99 D3-A
boards, and it made one of them (0054) lose a clean route.

## Enclosure-window repair

A terminal can be walled in by several routed nets at once: the pins of a
crowded header, where each neighbour's escape blocks the next. Blocker
transactions evict only the nets that one candidate path crosses, and fail
when an evicted net cannot return. The enclosure window
(`enclosure_repair_nets`, off by default; the D3 benchmark turns it on)
moves the neighbours together:

1. The open net is routed with no other routed copper. If fixed geometry
   alone blocks it, moving routed nets cannot help and the net is skipped.
2. Routed ordinary nets whose copper conflicts with that free path are the
   victims, then nets with copper within `enclosure_repair_radius_nm` of the
   unreached terminals, up to `enclosure_repair_nets`.
3. The open net and the victims are ripped up and rerouted, open net first.
   A net that fails goes first in the next order (at most
   `enclosure_repair_orders` orders), so the nets hardest to place claim
   their escape before the easy ones.
4. The window is kept only if every net in it connects; otherwise nothing
   changes.

Other open nets with a pad in the window join it as optional companions: a
companion that fails stays open without rejecting the window. Open nets are
taken smallest first, so ground can connect as a signal net's companion.

The repair is the last stage, after the final retry, so boards that close
earlier never pay for it. It is still expensive: a failing search spends its
whole budget (10 to 75 s on the D3 boards), and a window can try many
orders. Window searches therefore use the ordinary state budget, the repair
runs only when at most `enclosure_repair_open_nets` (2) ordinary nets are
open, and `enclosure_repair_searches` (64) bounds all its windows. On 0050,
with three open nets, windows took 650 s and closed nothing, so boards like
it are skipped. On D3-A board 0049 (MySRaspiGW PA/LNA, a 2×4 radio header on an
11.5 × 11.9 mm board), CSN routes alone but is enclosed by +3V3, CE, MISO and
SCK; fixed orders of those four kept trading the failure among them. With
all seven signal nets in the window, promotion settles it.
A negotiated-congestion window (the unified router on the window's nets,
everything else fixed) was tried and did not connect 0049; it is not used.

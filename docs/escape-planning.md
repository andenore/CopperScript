# Escape planning

The implemented contract of `pcbir/escape_plan.py`. The steps, research and
open questions are in the [escape planning plan](escape-planning-plan.md);
this page describes only what exists.

Status: steps E0 (diagnostics), E1 (pad fields and channel model), E2
(ball-grid escape by min-cost flow), E3 (two-end ordering and layer
assignment) and E4 (the unified router and `route-board`). Diagnostics never change routing;
the E3 plan does only when `EscapePlanOptions(two_end_order=True)` is passed
(off by default).

## Escape diagnostics

`escape_diagnostics(board, EscapePlanOptions())` reads the placed board and
returns an `EscapeDiagnostics` with one `ClusterEscape` per component and one
`ClusterPairEscape` per cluster pair. It is deterministic, integer-based where
it measures copper, and never raises for a valid board.

### Clusters and channel load

A component's cluster is its copper pads (non-plated holes and apertures
excluded). A pin is **outer** when it lies on the convex hull of the pad
centres, otherwise **inner**. Only inner pins on signal nets (nets with at
least two pads that are not distribution-zone nets) create demand: their
tracks must pass between outer pins. A surface land at least as wide as the
board's default via (an exposed pad) is not demand: it leaves through a via
in its own land.

For each copper layer an inner pin reaches (its surface for SMD pads, every
layer for through-hole pads), the channel load compares:

- **demand**: inner signal pins on that layer;
- **capacity**: the sum over consecutive outer pins on that layer, in order
  around the cluster, of ⌊(gap − s)/(w + s)⌋, where gap is the copper edge
  distance between the two pads, w the widest track and s the largest
  clearance among the inner pins' escape rules (`channel_rules`, below).

`overflow` counts the inner pins the ring cannot pass: surface pins against
their own layer's capacity, through-hole pins against what is left on any
layer. Example: 2.54 mm header pitch with
1.7272 mm lands leaves 0.8128 mm gaps, which hold one 0.254 mm track at
0.254 mm clearance.

The channel load is a whole-ring estimate; the pad-field channel model below
holds the gap-by-gap geometry the escape solvers use.

### Cluster pairs and two-end crossings

A **bundle** is the signal nets whose pads all lie on the same two
components. Two components with at least `cluster_pair_minimum_nets` (2)
bundle nets form a cluster pair; nets that also reach a third part are trees
and are left out. Pairs with more than `maximum_pair_nets` (256) nets are
listed with `analysed: false`.

Each net is placed at its pin nearest the other component and ordered along
its component's boundary: by angle around the cluster's mean pin position,
starting from the direction away from the other component. The second
component is read in the opposite sense, so a bundle that does not cross has
the same order at both ends. A single row of pins has no inside and is seen
from 1 mm behind its centre, so the row reads as the face toward the other
part.

- `crossing_pairs`: net pairs whose orders disagree, so they cross between
  the two parts on one layer.
- `crossing_nets`: every net in such a pair.
- `layer_moves`: the nets outside one longest non-crossing subset (a longest
  non-decreasing subsequence, as in Ozdal & Wong). They need another layer
  or a detour around a part.

### Net causes

`EscapeDiagnostics.net_causes()` maps a net to `two_end_crossing` (it belongs
to a bundle with at least one crossing pair: whichever nets route first take
the gaps the others need) and `channel_overflow` (it has an inner pin of a
cluster that overflows).

## Pad fields and the channel model

`pad_fields(board, component)` splits a placed component's copper pads into
**pad fields**, the pads that escape together, largest first. Pads join a field
when their footprint-frame centres are within the larger of
`field_spacing_factor` (3) times the component's median nearest-neighbour
distance and `field_spacing_minimum_nm` (3 mm), so a module's opposite edges
are separate fields and a package's rows are one.

Each field has a kind and its gaps:

| Kind | Detected when | Gaps |
|---|---|---|
| `lattice` | Pad centres lie on uniform columns and rows (in the footprint frame, within 1 µm) filling at least half the positions: BGAs, 2×N headers, pin grids, with missing pins allowed | `row` and `column` gaps between orthogonal neighbours, `diagonal` gaps between diagonal neighbours |
| `row` | All centres on one line | `ring` gaps between neighbours along the line |
| `perimeter` | Every pad on the convex hull: QFN, QFP, SOIC | `ring` gaps between neighbours around the outline |
| `irregular` | Anything else | `ring` gaps around the outline |

A `Gap` holds the copper edge distance between its two lands and the layers
both occupy (a surface land is on its side's outer layer, a through-hole land
on every layer). `Gap.capacity(w, s)` is ⌊(gap − s)/(w + s)⌋. Example, a
2.54 mm header with 1.7272 mm square lands at 0.254 mm track and clearance:
orthogonal gaps of 0.8128 mm hold 1 track, and the corner-to-corner diagonal
gap of 1.1495 mm also holds 1; round lands leave a 1.8649 mm diagonal that
holds 3.

### Tiles

A lattice field has a `Tile` for every cell between four neighbouring
positions, in the footprint's own frame (x east, y south): its corner pins
(NW, NE, SE, SW; `None` where a pin is missing), its side gaps (W, N, E, S)
and its two diagonal gaps (NW–SE, NE–SW). A side or diagonal through a
missing pin is unbounded. `via_site` says whether the board's default via
fits at the tile centre with the board's minimum clearance to every corner
land.

`Tile.admits(flows, layer, w, s)` checks tracks through the tile on one
layer, given as counts per pair of terminals (sides and corner pins):

- each side gap bounds the tracks through it;
- each pin starts at most one track, a missing pin none;
- each diagonal gap bounds the tracks that join its two halves. The NW–SE
  diagonal separates {N, E, NE} from {W, S, SW}; the NE–SW diagonal separates
  {W, N, NW} from {E, S, SE}; a corner pin on a diagonal belongs to neither
  of its halves.

This is the diagonal-capacity condition of Yan & Wong. A model that ignores
diagonal gaps admits tracks that cannot pass: W→N and S→E both squeeze
between the NW and SE pins. A model that caps the whole tile at one
diagonal's capacity refuses tracks that can: W→N and W→S use different
diagonals.

### Escape rules

`channel_rules(board, net, rule)` is a net's track width and clearance
between a field's pins. A net with breakout properties (`breakout_length`)
uses its breakout width and breakout clearance there, as the routers do
inside its terminal regions; any other net uses its own width and clearance,
else the board defaults. Widths never fall below the board minimum unless a
breakout width says so; clearances never fall below the board minimum.

Cluster reports in `escape` carry a summary per field: kind, lattice size
and pitch, gap counts and the narrowest gap of each kind in tracks at the
cluster's widest escape rule, tiles and via sites.

## Array escape (E2)

`plan_array_escapes(board, options)` decides, for every surface-mount ball
grid (a `lattice` pad field on one surface layer), which signal balls escape
on that layer and which need a via. It solves an integer min-cost max-flow on
the field's channel graph:

- every signal ball is a source of one track; plane (zone) balls do not
  count, they take a via to their plane;
- an edge ball (on the lattice's outer columns or rows) leaves directly;
- an inner ball enters one of its four tiles; a track crosses from tile to
  tile through a side gap, at most the gap's capacity, and leaves the field
  through an outer side gap;
- a tile passes at most as many tracks as its narrower diagonal holds. That
  is exact for straight tracks and conservative for turning ones: Yan & Wong
  couple the two diagonals exactly, which a plain flow cannot;
- each tile step costs one, so the flow keeps the most balls on the surface
  and, among those, the shortest escapes.

The solver is successive shortest paths with potentials, deterministic
(edges in insertion order, ties to the lower node), with no solver
dependency. Example: an 8×8 grid at one track per gap has 28 outer gaps, so
28 of its 36 inner balls escape on the surface and exactly 8 need vias; the
ring rule below fans out 16.

`ArrayEscape` lists `surface` and `vias` balls and `tile_load`, the planned
surface tracks per tile. With `EscapePlanOptions(array_flow=True)`
(`route-board --array-flow`, `bench route --array-flow`):

- the unified path's BGA fanout (`plan_bga_fanouts(..., array_plans=...)`)
  fans out exactly the plan's `vias` balls (and plane balls) instead of every
  ball deeper than two rings, and puts each via preferably in a cell no
  planned surface track crosses;
- the ordinary path's fanout tries a checked surface escape first for the
  plan's `surface` balls (`FanoutOptions.surface_pads`).

## Two-end plan

`plan_two_end_escapes(board, options)` plans every cluster pair whose bundle
crosses (`crossing_pairs > 0`). It covers every signal net joining the two
parts, rails and other trees included: they cross the bundle too, and an
unplanned net routed last finds its lane taken.

- **Planar layer**: the surface of the part whose bundle pins are all
  surface lands (the end that needs a via to change layer), else the first
  layer all the nets may use. A bundle with surface pins on both outer
  layers, or with no common layer, is not planned.
- **Kept nets**: one largest non-crossing subset (the LIS of the boundary
  orders) stays on the planar layer.
- **Moved nets**: the rest move to the legal layer nearest the planar layer
  in the stackup; a net with no other legal layer is listed as `unmoved`.
- **Corridor**: the box of both parts' bundle pins, grown by
  `corridor_margin_nm` (1 mm).

An `EscapePlan` carries what the routers use:

| Field | Used by | Effect |
|---|---|---|
| `surface_pads` | Fanout (`FanoutOptions.surface_pads`), for the ordinary path and the unified router's escape reservations | A kept net's surface pin tries a checked surface escape before a dogbone via, so the bundle's channel is not walled by vias; a moved net keeps its dogbone, which is its layer change |
| `net_order` | Detailed router (`DetailedRouterOptions.escape_net_order`); unified router (`UnifiedRouterOptions.escape_net_order`) | Planned nets route first: in passes 1 and 2 of the detailed router, and in the unified router's negotiation order. Moved nets, then kept nets from the outside of the bundle in, then unmoved nets |
| `layer_biases` | Detailed router (`DetailedRouterOptions.escape_layer_biases`); unified router (`UnifiedRouterOptions.escape_layer_biases`) | Inside the corridor a moved net pays `layer_bias_rate` per mm on any layer but its own (detailed router: added to the planar edge cost; unified router: `rate/10` added to those nodes' history while that net searches). In the detailed router a moved net's corridor searches also cover every allowed layer, not only its global guide's |

`layer_bias_rate` is 20 by default. A weaker bias leaves a moved net on the
planar layer, where it takes the gap a kept net needs: with rates 0, 5 and
10, D3 board 0049 loses 3 nets and led-ring 1 (rate 0), while at 20 both
route completely. The price is a via where a planar detour existed: on D3,
rate 20 added 33 vias to boards that were already clean (0035: 0 → 9,
0060: 0 → 6).

### Bus layers (E5)

With `EscapePlanOptions(bus_layers=True)` (`route-board --bus-layers`,
`bench route --bus-layers`; it implies the two-end plan) the nets outside the
planar subset are not all moved to one layer, where they may still cross each
other. Following Ozdal & Wong, each further layer takes a largest
non-crossing subset of the nets still left, nearest the planar layer first:

- a net joins a layer only if its rule allows the layer and its `max_vias`
  covers the layer changes at its surface lands (a through-hole land changes
  layer for free);
- nets that fit no layer stay `unmoved`;
- each layer's group routes together, outside-in, before the kept nets.

Every layer then carries a crossing-free part of the bundle. Example: a fully
reversed bundle of four on four layers keeps one net on F.Cu and puts one on
each of In1.Cu, In2.Cu and B.Cu; the single-layer move puts three crossing
nets on In1.Cu.

### Bus skew (E5, unified router)

A length-match group (a bus) routed by the unified router (`buses_first`,
on by default) negotiates among its own members first, so it keeps a shared corridor
instead of losing it piece by piece to other nets. On CopperProbe this
leaves 3 of 12 ADC nets open instead of 12. Its members then still differ in
length: routed alone on their shortest paths, the ADC members span 26.5 to
32.4 mm against a 5 mm `max_skew`, so no route order or detour control can
meet the limit; the short members must be lengthened.

With `bus_skew_tuning` (on by default; `route-board --unified-set
bus_skew_tuning=false` turns it off), once every member of a group has copper, each
member shorter than the longest minus `max_skew` is lengthened with checked
bumps or serpentines (`critical_tuning.UnitTuner`) against all other copper:
to the longest when that fits, else by the least that meets the limit,
never past its own `max_length`. As in the critical router, only a net that
declares `tuning_amplitude_limit` is tuned: the amplitude is a design
contract, not a router choice. Every member left over the limit is listed,
with its reason, in the routed board's `bus_skew` metadata; DRC reports the
skew itself.

CopperProbe's ADC rules declare no amplitude, so its bus stays 8.0 mm over
5 mm and `bus_skew` names each short member. With a hypothetical 1 mm
amplitude (an experiment, not a design fact), every short member was tuned
and the skew came to 5.0 mm (limit 5.0). A board without length-match
groups is unaffected by either option: D3 and the small ladder boards route
identical copper with both on.

Kept nets carry no layer bias: they start on the planar layer anyway, and at
a through-hole end every layer is theirs. Holding them to one layer there
squeezes the bundle through that layer's gaps. Biases are penalties, never
discounts, so the search heuristic stays admissible.

`run_routing_pipeline(..., escape_plan_options=...)` plans once on the
placed board before package access, then gives escape generation and the
ordinary or unified router the plan; detailed-feedback trials re-plan their
moved board. `copper route-board --escape-plan` turns it on and adds the plan
to the report as `escape_plan`. The global router does not read the plan:
global routing runs inside placement, before the placed board exists to plan
from. `python -m pcbir.bench route --escape-plan` routes a board
with the plan and records it under `escape_plan`, with `moved_on_layer`: for
each moved net, whether its routed copper used the planned layer inside the
corridor.

## Where diagnostics appear

| Consumer | What it shows |
|---|---|
| `copper route-board` report | `escape`: clusters with inner signal pins, every cluster pair, `overflow_clusters`, total `crossing_pairs` |
| `copper issues RUN` | Advisory `X` ids: `ESCAPE-CROSSING` per crossing pair (names the nets to move and the unrouted ones among them), `ESCAPE-CHANNEL-OVERFLOW` per overflowing cluster |
| `python -m pcbir.bench route` record | `escape` (crossing pairs and overflowing clusters only), `escape_causes` and `unrouted_causes` for unrouted nets |
| `python -m pcbir.bench causes RESULTS` | Unrouted nets per primary cause class, per unclean board |

## Cause classes of unrouted nets

`pcbir.bench.unrouted_causes` gives each unrouted net its classes, primary
first. Escape classes come first: a pending package exit or a failed search is
usually their symptom.

| Class | Source |
|---|---|
| `two_end_crossing` | Escape diagnostics: the net is in a bundle whose two ends' orders disagree |
| `channel_overflow` | Escape diagnostics: the net's inner pin is in an overflowing pad field |
| `package_access` | A pad of the net has a pending package exit, or the router blames package access |
| `search_budget` | The detailed search ran out of budget |
| `no_path` | The detailed search found no path |
| `other` | A router diagnostic that fits none of the above |
| `unlocated` | No router diagnostic |

A scored board that fails KiCad's check with no unrouted net is listed as
"KiCad only"; a completely routed board that is not scored yet is left out.

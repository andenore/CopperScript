# Escape planning

The implemented contract of `pcbir/escape_plan.py`. The steps, research and
open questions are in the [escape planning plan](escape-planning-plan.md);
this page describes only what exists.

Status: steps E0 (diagnostics) and E1 (pad fields and channel model).
Nothing here changes placement or routing.

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

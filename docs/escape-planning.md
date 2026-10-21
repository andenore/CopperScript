# Escape planning

The implemented contract of `pcbir/escape_plan.py`. The steps, research and
open questions are in the [escape planning plan](escape-planning-plan.md);
this page describes only what exists.

Status: step E0, diagnostics. Nothing here changes placement or routing.

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
  clearance among the inner pins' nets (each net's own rule, else the board
  defaults, never below the board minimums).

`overflow` counts the inner pins the ring cannot pass: surface pins against
their own layer's capacity, through-hole pins against what is left on any
layer. Example: 2.54 mm header pitch with
1.7272 mm lands leaves 0.8128 mm gaps, which hold one 0.254 mm track at
0.254 mm clearance.

Not modelled yet (step E1): diagonal gaps, via escapes to other layers,
neck-down widths, and the outer pins' own exits.

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

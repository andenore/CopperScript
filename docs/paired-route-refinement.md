# Bounded paired-route refinement

Joint package-escape search keeps its accepted raw candidate as a fallback.
Before locking it, `pcbir.pair_refine.paired_shortcuts` proposes bounded local
alternatives on the **shared spine**, not independent lane cleanup or arbitrary
track deletion. This extends the critical routing quality work in review R5.

## Geometry ownership

The search candidate carries its spine and original start/end ports. Missing or
inconsistent spine/escape provenance yields no refinement. Only exact forward
collinear spine vertices are compacted; reversal vertices and both endpoint
positions remain. Candidate bridges use deterministic straight/45-degree,
one-corner alternatives between existing spine vertices. They never invent
Steiner branches, change pin polarity or create new vias/layers.

Both lane paths are rebuilt together using offset-line miter intersections.
Before proposing copper, construction verifies that every offset edge has
the same forward heading as its corresponding spine edge. Adjacent miters
may consume a short spine edge, making an inner lane collapse or backtrack
even when all spine turns are legal. Such a construction is rejected for
both members; the existing bounded search/refinement must choose another
topology. No member is independently trimmed and no escape endpoint is moved.
This predicate applies to original joint search and shared-spine refinement.
Joint search retains its original cheap goal bridges, then tries bounded
terminal-collar alternatives: advance one/two pitches along the current or
adjacent allowed heading and join a fixed one-pitch end collar. Two-leg
half/full-pitch advances distribute small sub-grid offsets across an S-turn.
At most 24
tails are considered per near-goal state. This lets an off-grid terminal use
a forward S-turn instead of requiring a very short, consumed inner miter.
Both original port headings must match exactly, and all full-lane/escape
clearance and original profile checks still apply. Maze pitches, port-pair
limits and expanded-state budgets are unchanged; a larger tail menu is not
evidence of optimal routing or a new unbounded search.
The first and last spine headings must match the original ports exactly, and
every turn must remain at most 45 degrees. Original package fanout tracks are
retained byte-for-byte at both ends. All lane segments and cross-lane distances
are checked against actual pads, board edges, keepouts, prior critical copper
and the original escape reservations. Up to 256 geometric attempts are permitted
per accepted search candidate; the original paired maze bounds are unchanged.

## Profile and transaction ownership

The generator proposes geometry; `critical._improve_pair_spine` owns acceptance.
Every complete proposal is remeasured by `_route_pair` under the original width,
gap, skew, length, uncoupled and via rules, then checked by complete native DRC
with earlier reservations. Resizing/relabeling and forbidden-layer proposals
are rejected explicitly. Failed proposals cannot replace the accepted raw pair.

Neither member's measured length may increase. Total length must decrease, or
equal-length geometry must use fewer segments. Accepted result metrics are
recomputed from emitted copper and marked `joint_pair_refined`; original search
counts/states remain. `pair_refinement_attempts` and
`pair_refinement_candidates` are reported and included in the critical fingerprint.
The latter counts complete geometric proposals, not accepted/profile-qualified
ones. Collinear compaction alone is **not wirelength or bend improvement**;
per-segment integer length rounding can change a few nanometres without changing
the physical curve.

This is deterministic bounded local improvement, not an optimal octilinear
visibility router or placement optimizer. It currently handles only joint-search
spines: aligned/global-guide candidates retain their existing owner, as do
transition routes. Earlier/later critical groups and ordinary tracks are not
moved or globally ripped up. External impedance and USB/RF qualification remain
separate requirements even when native geometry and measured budgets pass.

Tests exercise actual detour shortening, exact escape/endpoint preservation,
reversal/collinearity, keepout/reserved-route rejection, deterministic bounds,
missing provenance, original profile/native rejection, and malformed width/net/
layer proposals. Real-board results and unresolved topology limitations are in
[pass 10](routing-review-pass10.md).
The corrected-part rerun and short-miter/access fixes are reviewed in
[pass 12](routing-review-pass12.md).

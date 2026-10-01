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

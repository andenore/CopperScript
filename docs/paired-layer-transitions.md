# Paired USB layer transitions

USB does not inherently require top-layer-only routing. A short pad escape
into matched signal vias, followed by coupled routing on another signal layer,
is a valid topology to investigate. TI recommends minimizing transitions,
matching their count/geometry and maintaining the reference return path:
[USB layout basics](https://www.ti.com/video/6087491555001),
[differential-pair via discontinuities](https://www.ti.com/document-viewer/lit/html/SSZTCM4).
These guides are design guidance, not qualification of our provisional board.

## Implemented bounded topology

After the coarse-guide pair candidate fails its atomic gate, package geometry
orders searches. Internal SMD pairs (for example internal LGA rows) try legal
matched via escapes before consuming every surface maze resolution. A pair is
classified internal only when both members and every matching same-number land
at one endpoint lie beyond a margin of one default via diameter plus twice
minimum clearance inside the local copper-pad-center envelope on all sides.
Perimeter and ambiguous pairs retain surface-first search. This is an ordering
heuristic, not proof that a transition is necessary or via-count optimal. It
uses no component-name special cases and is independent of placement rotation.
The report's `pair_search_order` records the choice. All search bounds and atomic
acceptance gates are unchanged; the other topology remains fallback.

`pcbir/pair_vias.py` proposes complete paired candidates after the surface-only
joint search fails. It never modifies locked input copper or routes members
independently. The sequence is:

1. Enumerate straight/45-degree terminal collars on the actual pad-side layer.
2. Widen locally to a matched signal-via pitch large enough for copper and
   drill spacing. Trace pitch is not incorrectly reused as via-pad pitch.
3. Select a physically supported via span; absent an explicit technology table,
   use through-vias across the entire stackup, not fictional blind vias.
   Signal drills must stay outside solderable SMD lands; same-net clearance
   exemptions cannot silently introduce an unqualified signal via-in-pad.
4. Where requested, find a legal nearby reference-net via at each transition.
   After axis-aligned slots, a bounded 0.125 mm local lattice within 2 mm
   tries lateral sites; all original distance/copper/drill constraints apply.
5. Taper back to the declared trace pitch on another jointly allowed signal
   layer and search both lanes along one heading-aware octilinear spine.
6. Repeat the matched transition at the other end. Check the complete pair,
   returns, native contacts, every physical via layer, original budgets and
   prior reservations atomically before accepting any copper.

Two signal vias per member are required for this topology. Zero/one-via
budgets and surface-only profiles fail closed. Dedicated plane layers are
excluded by the existing routing policy. Target layers are tried in the
existing plane-adjacency preference order. Terminal domains retain at most
16 alternatives per heading/member-order combination to avoid eliminating
all reverse-heading alternatives with a single sorted truncation. At most
eight compatible terminal-pattern combinations per target layer are searched
at each 1/0.5/0.25 mm pitch, with 30,000 expanded states per search.

Search/report provenance records both signal-via pairs explicitly. The
critical owner verifies matched dimensions, spans and widened spacing, equal
member via counts, return-via distance, measured coupling/skew/uncoupled length
and fresh native DRC. Ordinary routing cannot replace this copper. Existing
surface-only shared-spine refinement does not own these multilayer transitions.

## Example profile

All four USB member profiles in `examples/full_vertical_board.copper` now
allow `F.Cu,In2.Cu`, with `max_vias = 2`, required `GND` return vias and a
2 mm maximum distance to both signal vias. This is an explicit provisional
geometric bound, not a vendor-qualified SI limit. The initial 1.5 mm bound
introduced with this feature left no legal reference-via site in the saved
terminal pattern; copper/drill fabrication limits were not changed. In2 is adjacent to the declared
In1 ground plane in the four- and six-layer prototypes. RF profiles are
unchanged. A legal surface-only result remains preferred and needs no return
via because it contains no transition.

## Explicit limitations

- Only terminal transitions plus one other-layer middle spine are supported;
  this is not a general 3-D paired maze or interior-transition optimizer.
- Collars use the same nominal width/gap on both layers. Real controlled
  impedance requires actual dielectric/copper geometry and layer-specific
  width/gap qualification; the current prototype does not supply that evidence.
- Nearby return vias and zone intent do not prove refilled reference-plane
  continuity. Ground connectivity, plane voids/antipads, via stubs and signal
  integrity remain independent signoff concerns. No production readiness is
  inferred from pair connectivity or a photo of a reference layout.
- Search failure under finite pattern/state bounds is not proof that no route
  exists. Joint ordinary/critical escape allocation remains a separate task.

Regression tests cover a top-layer wall crossed on In2, required return vias,
determinism, through-span emission, exact native connectivity, constrained
budgets/layers, blocked transitions, 45-degree terminal rows, own-land drill
exclusion and atomic rejection of corrupted proposals.

The matched saved-checkpoint result is recorded in
[routing-review-pass19](routing-review-pass19.md).

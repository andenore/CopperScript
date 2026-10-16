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
   Members of a pair with breakout properties are spaced by the pair gap
   (D-PHY plan R1), so its via pitch uses the pair gap when that is larger.
3. Select a physically supported via span; absent an explicit technology table,
   use through-vias across the entire stackup, not fictional blind vias.
   Signal drills must stay outside solderable SMD lands; same-net clearance
   exemptions cannot silently introduce an unqualified signal via-in-pad.
4. Where requested, find a legal nearby reference-net via at each transition.
   Default `always` behavior is unchanged. Explicit `reference_change` rules
   may omit it only for two actual signal layers adjacent to the same declared
   `shared_reference_layer`; both member policies/return nets must agree.
   Owning validation checks actual contact layers and the complete pair's
   declared reference-zone coverage. Unknown/different references still need
   return vias; intent never proves refilled copper or SI.
   After axis-aligned slots, a bounded 0.125 mm local lattice within 2 mm
   tries lateral sites; all original distance/copper/drill constraints apply.
5. Taper back to the declared trace pitch on another jointly allowed signal
   layer and search both lanes along one heading-aware octilinear spine.
6. Repeat the matched transition at the other end when it is on another layer. Check the complete pair,
   returns, native contacts, every physical via layer, original budgets and
   prior reservations atomically before accepting any copper.

Same-face terminals require two signal transitions per member for an other-layer
middle route; a zero/one-via budget still rejects that topology. Opposite-face
terminals may instead use either terminal surface as their common middle layer:
one end has a legal direct collar and the other has one matched transition.
One-via budgets then work without inventing a second via pair. A third signal
layer still requires two transitions. Only emitted transitions receive return
vias and site ranking; direct collars never claim a layer change. Dedicated
plane layers remain excluded. Fewer required transitions are preferred, then
the existing plane-adjacency order. Terminal domains retain at most
16 alternatives per heading/member-order combination to avoid eliminating
all reverse-heading alternatives with a single sorted truncation. At most
eight compatible terminal-pattern combinations per target layer are searched
at each 1/0.5/0.25 mm pitch, with 30,000 expanded states per search.

A planned crossing of a bundle (D-PHY plan R5, CS-165) uses this proposer
alone, restricted to its planned layer (`layers`), with a site rank that drops
transitions whose vias lie in a breakout region and tries the pair's reserved
corridor first (`site_rank`). Without those arguments the proposer is
unchanged.

Search/report provenance records every emitted signal-via pair explicitly. The
critical owner verifies matched dimensions, spans and widened spacing, equal
member via counts, return-via distance, measured coupling/skew/uncoupled length
and fresh native DRC. Ordinary routing cannot replace this copper. Existing
surface-only shared-spine refinement does not own these multilayer transitions.

## Example profile

All four USB member profiles in `examples/full_vertical/board.copper` now
allow `F.Cu,In2.Cu`, with `max_vias = 2`, required `GND` return vias and a
2 mm maximum distance to both signal vias when the reference changes. The four
members explicitly select `reference_change` and `shared_reference_layer =
"In1.Cu"`: F.Cu and In2.Cu are on opposite sides of this same declared plane,
so no single-plane-only GND via is generated for that topology. This is an
explicit topology inference, not evidence of filled continuity or impedance.
The default `always` profile still requires vias. This is an explicit provisional
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


Opposite-face regression fixtures cover both terminal orders, required returns,
one-via budget acceptance, zero-via/absent-layer/blocked-return rejection,
renamed endpoints, exact resumed transition ownership and the aggregate state
limit. This topology does not qualify layer-specific impedance or via stubs.


For end-on cardinal terminal rows, a failed direct collar slot can try three
analytic wide-and-tapered departures. Each starts transversely to clear the
companion land, turns outward, then tapers to the declared pair pitch. Only
one exact legal collar may occupy the original heading/lane/length slot;
port counts, search budgets and acceptance gates are unchanged. Rotation,
foreign-obstacle and insufficient-length fixtures cover this fallback.


### Plated terminal contacts

Exact pair search derives access from the unique physical pad rather than its
coarse guide layer or component mounting side. A plated through-hole contact
can join any permitted physical copper layer directly; a surface land can join
only its actual face. The same-layer family uses the first common contact
layer. Other bounded transition families charge vias only at ends that lack
direct copper on the selected middle layer. Existing per-layer search and
aggregate state budgets, return-via obligations and fresh owner validation
remain unchanged. Missing, ambiguous, aperture and unplated contacts grant no
electrical access. This is geometric contact support, not connector-launch or
plated-barrel signal-integrity qualification.


Joined duplicate contacts prefer the terminal with the largest intersection
of actual copper contact layers and source-permitted routing layers. All
contacts still require fresh local branch connectivity; failed main searches
roll back those branches. Equivalent single-layer contacts keep deterministic
identity ordering. No source pad is omitted.

The same-layer pair family ranks both endpoint collars by a checked forward
pitch before existing geometric preferences. This avoids exhausting the
bounded maze allowance on a goal still buried among package pads. The goal
may have a shorter final approach, so a negative hint never deletes a port.
The original port inventory, combinations cap, search slots and aggregate
state budgets remain unchanged; complete escaped-board checks still gate
every candidate.


When legacy skew tuning creates a spacing violation, exact pair admission can
try at most two existing clearance-aware tuning proposals. These operate on
the complete disposable group, including duplicate-contact branches, with
all earlier owners retained as obstacles. Source contacts, reserved layers,
length/skew, via counts and physical clearance are validated afresh. Rejected
proposals leave no copper behind; this adds no maze searches or search states.


Virtual tuning spans validate compatible net/layer/width, forward direction,
shared unprotected endpoint and collinearity before constructing a merged
track. Candidate list order is not electrical traversal order; a later edge
can end at an earlier edge's start without forming a legal forward span.
Rejected joins preserve the exact original occurrences and all outer gates.

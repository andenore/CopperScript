# Rigid physical placement clusters

This implements the placement mechanism for R8c, not the vendor reference
template or RF qualification. The electrical IR and `.copper` connectivity stay
unchanged. Initial integration is through `PhysicalBoard.rigid_clusters`; there
is no new language syntax or CLI template loader yet.

## Contract

`RigidPlacementCluster` is a hard macro. `PlacementGroup` remains a soft
proximity/priority hint. Each `RigidPlacementMember` binds an instance reference,
resolved footprint name, complete footprint SHA-256 and local center pose. The
hash includes source library ID, pads, courtyard, graphics, keepouts and metadata;
changed assets require deliberate template revalidation, not name-only reuse.
An evidence `source` locator is mandatory for this physical template but does
not change the optional provenance policy for electrical parts/devices.

The local frame is the anchor's unique physical pad, or its footprint origin
when `anchor.pad` is omitted. The local anchor rotation is zero. Its local
center is the negative of that pad's footprint coordinate. Member rotations
are relative to the anchor. Angles use the existing physical IR/KiCad convention,
not an independently invented Cartesian convention. Integer-nanometre rounding
is shared with ordinary footprint transforms.

The initial implementation supports front-side units without mirroring, and
explicit allowed rotation sets, including 45 degrees. Per-component allowed
orientations, regions, courtyards, edge clearance, keepouts and relative rules
still apply. Member sets cannot overlap between macros. A failed bounded search
reports failure; it does not relax geometry or split the unit.

## Python integration

Attach the template after footprints have been resolved, before `plan_placement`
or `run_routing_pipeline`. For example, using **synthetic**, origin-anchored
reference coordinates (not a usable RF layout):

```python
from dataclasses import replace
from pcbir import (
    ComponentPlacementRule, PlacementTarget, Point, RigidPlacementCluster,
    RigidPlacementMember, plan_placement,
)
from pcbir.clusters import footprint_geometry_digest

poses = {p.reference: p for p in physical.placements}

def member(reference, position, rotation=0):
    footprint = physical.footprints[poses[reference].footprint]
    return RigidPlacementMember(
        reference, footprint.name, footprint_geometry_digest(footprint),
        position, rotation,
    )

template = RigidPlacementCluster(
    name="test-matching",
    anchor=PlacementTarget("U_RF"),
    members=(member("U_RF", Point.mm(0, 0)),
             member("L_MATCH", Point.mm(3, 0), 90)),
    source="synthetic test fixture; replace with audited vendor evidence",
    allowed_rotations=(0, 45, 90, 180, 270),
)
# Merge these into existing component rules rather than duplicating references.
rules = {r.reference: r for r in physical.placement_rules}
rules["U_RF"] = replace(rules.get("U_RF", ComponentPlacementRule("U_RF")),
                        allowed_orientations=(0, 45, 90, 180, 270))
rules["L_MATCH"] = replace(rules.get("L_MATCH", ComponentPlacementRule("L_MATCH")),
                           allowed_orientations=(0, 90, 135, 180, 270))
physical = replace(physical, rigid_clusters=(template,),
                   placement_rules=tuple(rules.values()))
planned = plan_placement(physical)
```

For a pad-anchored template, use `PlacementTarget("U_RF", "30")` and set the
anchor member's local center to negative pad 30's actual position. Never infer
the physical pad from a logical pin name: use resolved package bindings first.
Preserve source-specific matching ground topology separately; relative poses
do not create tracks, return vias or ground connectivity.

## Placement and routing sequence

1. Analytical placement estimates an anchor target. Macros with fixed members
   legalize first, followed by named movable macros. A fixed non-anchor member
   determines the anchor pose; all fixed poses must agree.
2. The planner checks at most `legalization_candidates` distinct anchor locations
   per macro, times its explicit rotations. All members and already placed fixed
   obstacles must be legal together, including internal courtyard clearance.
   There is no macro backtracking/repacking if greedy bounded placement fails.
3. Ordinary legalization and detailed refinement freeze macro members. Subsequent
   bounded whole-macro refinement accepts only legal lower-cost trials without
   increased relative-rule penalty or estimated congestion overflow. Dense-package
   spreading includes every rigid companion and retains complete legality checks.
4. Global, detailed and zone-escape placement feedback expand a proposed member
   move to its complete unit, then apply fixed-member and whole-board legality.
   Existing pipeline route/DRC/connectivity rollback gates still apply. Nothing
   moves beneath accepted copper; copper is rebuilt for placement trials.
5. Final placement acceptance/native DRC rejects a split macro. Template rules
   participate in global-route and signoff fingerprints. Existing candidate
   statistics count accepted whole-macro refinement as refinement moves.

Local `CopperKeepout` polygons/holes transform around the same anchor and retain
their explicitly declared physical layers/blocking flags. Complete placement
checks footprint-blocking keepouts against all members/components; there is no
implicit exemption for a member. The shared clearance resolver supplies track
and via obstacles to routing and all blocking flags to native DRC. KiCad exports
cluster keepouts at board level with stable `cluster/<name>/<id>` identities.
Existing unsupported keepout-hole geometry fails at KiCad export; it is not
silently removed. Pad/copper clearance is still checked by native DRC rather
than inferred solely from courtyard legality.

## Remaining full-vertical work

Verification: 352 CopperScript tests pass, including 19 new cluster cases and
two installed-KiCad 10.0.6 differential keepout checks. The F.Cu obstacle is
reported as `items_not_allowed`; the same segment on B.Cu is not blocked.
The suite retains 459 upstream CAM-library warnings. This increment does not
move the full-vertical example, transplant vendor copper, or claim a full-board
reroute or production signoff.

Nordic support circuit completion, vendor placement/pad extraction, local
matching-ground reservations and Johanson corner/ground-clearance qualification
remain open. The cached Nordic v1.1 archive includes a QFAA LDO pick-and-place
file and Gerber/Altium assets, so offsets need not be guessed from a PDF image.
Cross-check their package/pad orientation and schematic population before
binding the template to our resolved footprints. No vendor macro was fabricated
from the synthetic test coordinates above. GNSS noise-source separation and
actual six-layer return-path/impedance evidence also remain separate gates.

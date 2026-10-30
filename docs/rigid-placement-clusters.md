# Rigid physical placement clusters

This implements the placement mechanism for R8c, not the vendor reference
template qualification. Integration is through `PhysicalBoard.rigid_clusters`
or an explicit `--placement-templates` JSON scene. Electrical connectivity stays
in `.copper`; the loader verifies pad/net bindings and never rewires a design.

This original template path remains placement-only. The experimental
[physical hard-macro extension](physical-hard-macros.md) adds separately bound
immutable copper, ports and private access regions; it must be explicitly
materialized and is not yet consumed by the full routing pipeline.

## Contract

`RigidPlacementCluster` is a hard macro. `PlacementGroup` remains a soft
proximity/priority hint. Each `RigidPlacementMember` binds an instance reference,
resolved footprint name, complete footprint SHA-256 and local center pose. The
hash includes source library ID, pads, courtyard, graphics, keepouts and metadata
except the machine-local `source_path` locator;
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

## Library integration

In the Rust crates a template is a `copper_pcb::board::RigidPlacementCluster`
of `RigidPlacementMember`s, each bound to its footprint by
`copper_physical::footprint::footprint_geometry_digest`. Attach it to the
physical board after footprints have been resolved, before `plan_placement` or
`run_routing_pipeline`, and merge the members' allowed orientations into the
existing component rules rather than duplicating references. Users normally
supply templates through the CLI scene below.

For a pad-anchored template, anchor on a pad target (component `U_RF`, pad `30`) and set the
anchor member's local center to negative pad 30's actual position. Never infer
the physical pad from a logical pin name: use resolved package bindings first.
Preserve source-specific matching ground topology separately; relative poses
do not create tracks, return vias or ground connectivity.

## Source-backed CLI scene

`examples/full_vertical/placement_templates.json` binds CopperLib's extracted
Nordic QFAA LDO U1/C3/L1 midpoint/rotation data to U_NRF/C_BT_MATCH/L_BT_MATCH.
The scene pins both reference bytes and complete resolved footprint digests,
then checks the declared physical pad/net roles. Changed source/footprints,
missing members, mismatched nets, or unknown binding fields fail rather than
being ignored. No scripts are loaded or executed by the scene loader. Paths
are relative to the scene. Explicit component orientation restrictions are
retained, not broadened. The scene allows 45-degree macro rotations when all
members' component rules permit them.

Example, from the CopperScript root:

```powershell
copper plan-layout examples/full_vertical/board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --placement-templates examples/full_vertical/placement_templates.json `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints" `
  --report build/rf-layout.json -o build/rf-placed.kicad_pcb
```

The same option is accepted by `route-global`, `route-board` and
`export-kicad-pcb`. Proxy/different footprints fail the pinned identity check.
Templates are opt-in; there is no implicit filename-based discovery.

This is an explicitly **provisional footprint adaptation**, not the vendor PCB:
the installed QFN land centers differ by about 75 um, and C/L end-pad centers
also differ from the reference. The macro preserves source component centers,
top-view orientation and physical C3 numbering (pad 1 GND, pad 2 ANT). It does
not transplant matching-ground copper, vias, crystal/DEC support or antenna
keepouts. Those remain required work before RF qualification.

`internal_clearance_nm` is an explicit additional courtyard gap inside a macro;
`None` keeps the ordinary planner gap. The example uses zero additional gap
because its source-backed C3/L1 positions leave only about 32 um between the
installed courtyards. Courtyards must still not overlap; outside components
retain the ordinary 0.5 mm additional gap. All pad/track/via/fabrication rules
and native DRC are unchanged. This setting is not a copper-clearance waiver or
manufacturer assembly approval.

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
4. Global and congestion-driven placement feedback expand a proposed member
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

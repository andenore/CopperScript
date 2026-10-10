# Routed-board quality improvements

Requested 2026-10-10 after inspection of CopperAssetTracker. Generic semantics,
placement/routing algorithms and regression fixtures belong in CopperScript;
reusable part/circuit metadata and macro assets belong in CopperLib. The tracker
owns only its composition, placement choices and validation artifacts.

- [x] Diagnose and enable compatible same-net host/macro ground-zone overlap;
  preserve macro tracks, switching-node exclusions and prescribed RF returns.
  Verify actual native filled copper, not just touching outlines or net names.
- [x] Add reusable decoupling intent with an explicit protected pin/pad association.
  Prefer nearby, same-side pad-to-pad placement and direct surface routing before
  package fanout or plane stitching. Do not guess RF/load/bulk capacitors from a
  reference designator or ban ground-return vias needed to reach a plane.
- [x] Avoid gratuitous same-layer same-net X crossings in package escapes while
  permitting shared endpoints, T junctions and deliberate common trunks. Apply
  the same quality rule to greedy reservations and joint escape assignment.
- [x] Prefer straight outward package exits when legal; retain diagonal/two-leg
  fallback for obstructed escapes, rotated packages and dense-pin access.
- [x] Add regression coverage for ground-zone compatibility and keepouts,
  decoupling placement/routing, adjacent common-net escapes and fallback paths.
- [x] Validate changes on a new tracker candidate; retain prior fabrication files
  and qualification reports as historical artifacts, never overwrite evidence.

Completion and remaining limitations must be recorded below. Geometric routing
quality is not RF/power qualification or production authorization.

## First implementation and tracker evidence

The host-zone overlap gate now scopes blocking keepouts to the owner's protected
region: a remote RF exclusion intersecting a whole-board host pour must not block
an otherwise compatible regulator overlap. Explicit overlap permission is still
required; foreign nets, local exclusions and immutable owner copper remain guarded.
The tracker replaces the four segmented top zones (which left a central hole)
with one same-net-overlap-enabled top GND pour.

Separate `build/routing-quality-20261010/ground-merged.kicad_pcb` passes native
KiCad 10.0.6 refill: zero violations and zero unconnected items. Comparing actual
filled polygons gives zero gap and 58.638 mm of shared boundary for EACH regulator
against the host zone. No positive area overlap is necessary: they abut as
continuous copper. Separate zone objects/settings and owner asset bytes survive.
Previously exported production packages are unchanged.

`role = decoupling; decouples = "U.VDD";` on a capacitor lowers to typed physical
pad/net intent and follows module hierarchy. Placement prefers the same side and
nearby actual feed land, without a guessed universal electrical distance limit.
Ordinary local surface paths are reserved before package exits/plane access;
ground vias remain permitted. Explicit RF/critical routing and macros retain
ownership. The existing `max_distance` and fixed-placement rules are not weakened.

The tracker annotates 14 capacitor/pin associations, not RF/crystal load caps or
the modem bulk reservoir. A read-only probe against the EXISTING routing found
eight associations with blocked direct candidates: C_CC, C_ACCEL, C_LEVEL_A/B,
C_STATUS_A/B, C_DEC4 and C_NRF_VDD3. Its ten proposed surface segments were NOT
committed. Existing fixed placements/obstacles need a fresh routing iteration.
C_DEC3 keeps its explicit critical-router policy.

Greedy and joint package escape selection use the same exact interior-crossing
predicate. Collinear trunks, shared endpoints, T junctions and crossings on
different layers remain allowed. Straight outward rays are tried before diagonal
fallback; a package rotated 45 degrees keeps its diagonal outward normal.
This does not rewrite old copper or prohibit all crossing shapes in area routing.

Verified routing/compiler suite: 210 passed, 2 optional tests skipped; tracker:
27 passed. Retained ground-only native PCB SHA-256:
`f9f4533bc6e1137bb92ed330b2bf49c743d0389f44e42d99ff72cea5fbc7f3a4`;
its clean native DRC JSON SHA-256:
`15dfc2e08a01f3e3a8f73ae2b60873fbdb30406f999d91c6269e9de7cdbe0848`.
No claim is made that the broader full suite or RF/power qualification passes.

- [x] Revisit the eight blocked tracker bypasses using real pad geometry; remove
  obsolete project-fixed poses only where replacement placement is verified.
- [x] Produce and inspect a fresh complete tracker route with the new defaults;
  compare local path lengths, via counts and common-pad escape quality.
- [x] Rebuild candidate manufacturing/qualification evidence after that reroute.
  Earlier qualification reports are historical after source/algorithm changes.
- [x] Add bounded obstacle-aware local bypass search and placement access ranking.
  Preserve verified existing surface chains and earlier bypass reservations.
- [ ] Extend placement feedback to joint repair of mutually blocked decouplers;
  the current bounded greedy ranking reports unresolved cases rather than
  relaxing fixed placement, copper ownership or electrical constraints.
- [ ] Extend same-net crossing preferences beyond package escape assignment to
  ordinary area-route topology, without banning legitimate intentional junctions.

## Pad-access follow-up

On the unrouted tracker geometry, four of the eight original failures were old
copper obstacles. The remaining four had legal straight exits but the simple
elbow candidates turned into adjacent lands too early. The generic router now
tries land-sized straight lead-ins and bounded exact-clearance octilinear A*,
attaches off-grid terminals exactly, rechecks compressed route pieces, honors
breakout widths and reuses verified multi-leg surface chains. No vias or
movable-obstacle exemptions are introduced by the local bypass search.

Placement ranks the outward pin neighborhood and actual surface access, not
just Euclidean distance. Earlier fixed/movable bypasses reserve their corridors;
a candidate's ground land cannot sever one of those earlier paths. The search
is bounded to 64 locally legal copper-access trials per movable capacitor.

The tracker's translator was moved from (20.5,39) to (20.5,42) mm to free room
below the modem. Four translator bypass fixed poses were removed. A separate
`build/decoupling-review-20261010/review.json` records legal placement and 13/13
ordinary associations connected by verified F.Cu chains, zero local-path vias,
27 created surface segments and no pending associations. Their paths measure
1.640–3.321 mm; these are measurements, not electrical qualification limits.
DEC3 retains its specialized policy. Independent native refill of this LOCAL
review has no clearance errors, one expected dangling immutable MODEM_EN port
warning and 106 opens elsewhere on the intentionally unrouted board.
Reading the saved native PCB back through the exact current-source pad/pose
guard confirms all 26 target/feed terminals, no pending links and zero newly
proposed tracks: the thirteen surface chains survive native export and refill.

Current regression coverage: 214 passed / 2 optional skips in the routing and
compiler suite, 65 surface/fanout/plane checks passed, 88 interface/qualification
checks passed; tracker 28 passed. Straight common-net launches legitimately share
one verified via in the adjacent-pad fixture. A separate bounded-radius fixture
retains two drills and checks their actual hole spacing; neither fixture permits
crossed launch paths or unresolved pads.

A fresh all-source reroute passed ordinary exits and the RF/clock groups, but
the modem USB differential pair failed the first bounded package-access search.
Ordinary area routing correctly remained gated. Full routing and new fabrication
qualification are still unchecked; previous production packages are untouched.

A second preflight using the legally reposed review seed (NO historical ordinary
copper) passes all 82 ordinary exits, every critical group including both USB
pairs, and all bypass reservations. This isolates the earlier USB failure to the
new placement candidate/search, not an impossible source connection. The
controlled full reroute is preserved separately with its area/critical/native
reports; it must still pass complete connectivity and final-native checks.
The CLI now creates a fresh report parent directory before writing, with a
nested-output regression: a long routing attempt must not lose its report simply
because its requested output directory did not already exist.

## Failed-net reservations and final review checkpoint

The pose-preserving full attempt connected all critical groups and 58 ordinary
nets; GNSS_TIMEPULSE and V5 needed bounded repairs. GND/V3V3 were deferred fill
nets, not ordinary search failures. Native refill initially showed eight opens.

The general cleanup stage discarded V5's owned escapes when its area search
failed. Later plane stitching then occupied the charger's input-pad exit. A
controlled comparison proves all five dense V5 exits legal BEFORE stitching,
but the charger pin 13 blocked afterwards. CopperScript now retains verified
escapes for failed ordinary nets, as it already did for failed subset repairs.
Successful nets still release unused tails/vias; a preserved exit does not
change partial status or constitute a connected route. Two-leg, mixed-net and
regional-pour regressions cover this policy, without changing width/clearance.

The tracker repair workflow rebuilt these escapes at the saved current-source
area checkpoint, routed V5/GNSS_TIMEPULSE, then stitched planes. The debug-header
contact needed an 8 mm computational search radius rather than 3 mm; no electrical
limit, footprint, stackup or via-in-pad permission was changed. Conservative
native-warning cleanup uses the existing generic stub/unused-escape algorithms,
exact ordinary copper extents and actual connectivity checks. Macro/critical
copper and required via arrays remain protected; the inputs remain recoverable.

`build/fabrication-routing-quality-20261010/board.kicad_pcb` is a new six-layer
tracker review export. Native refill/DRC: zero violations, zero opens. Exact-source
readback confirms all 13 ordinary bypass chains (26 terminals), no proposed repair
tracks and zero local-path vias. Both regulator ground fills meet the host fill
with zero gap and 58.638 mm shared boundary each. DEC3 remains specialized.

This does NOT resolve every visually crossed junction. The exported PCB contains
11 strict same-net X intersections: two immutable macro GND intersections, four
V3V3 stitching/area intersections, two V1V8_MODEM and one each GNSS_TX, MCU_SWDCLK,
NRF_RESET. The nine ordinary intersections remain the next area-topology review
task; do not claim the package-escape preference fixes all copper junctions.

Post-reservation-change regression runs: 274 passed, 3 optional skips across
three focused suites (101/78/98 collected); tracker 34 passed plus 28 subtests.
Broader full-suite success is not claimed. Fresh CAM: 10 pass/3 incomplete;
RF/power: 7 pass/36 incomplete; checkpoint: 19 pass/3 incomplete. Independent
vector CAM, reviewed stackup/operating inputs, RF/power evidence and all 83
assembly approvals remain outstanding. New review archives have
`qualified_release=false`; older fabrication packages are unchanged.

# Pass 19: modem USB through paired terminal vias

## Scope and method

This is a **partial critical-pair experiment**, not a full ordinary-board rerun.
Restore all 51 pass-18 poses and its 137 accepted tracks / 75 physical vias.
Change the source USB profiles to allow F.Cu/In2.Cu and two signal vias/member,
with required nearby GND return vias; keep footprint identities, stackup and
all fabrication copper/drill limits unchanged. Regenerate global guides under
the revised profiles. Invoke the new joint terminal-via proposer and the normal
critical profile/native-DRC acceptance functions against the saved copper.
No pad or via coordinates are hard-coded in the algorithm.

The user-provided reference photo motivated the topology (short surface
escapes into two signal vias), not a transfer of geometry or proof of SI.

## Result

- First via-assisted candidate accepted; one search, 25 expanded states.
- Both modem USB nets connected. No displacement of prior copper or poses.
- 40 added segments, four signal through-vias (two/member), two provisional
  GND return through-vias. Total checkpoint: 177 tracks, 81 vias, 471 lands.
- 20.291 / 20.920 mm member lengths; 0.630 mm measured trace skew.
- 17.870 mm coupled length; 2.421 / 3.050 mm uncoupled length. These are
  geometric metrics, not electrical delay/impedance qualification.
- Width/gap remain provisional 0.18 / 0.20 mm. Signal via diameter/drill remain
  0.8 / 0.4 mm; widened matched via pitch is 0.890004 mm.
- Signal drills are outside SMD lands; standard open vias are not placed
  through solderable USB pads. Their same-net annular copper may contact
  same-net pad/escape copper.
- Fresh native checking finds no hard geometry errors, excluding expected
  unrelated opens and full-route incompleteness in this early checkpoint.

The initial newly introduced 1.5 mm reference-via search limit left no legal
reference site in the saved terminal pattern. The example now explicitly uses
a provisional 2 mm distance bound and tries bounded lateral reference sites.
No minimum copper/drill spacing, via dimension or physical DRC check was
relaxed. This distance is not asserted to be a manufacturer-qualified SI bound.

## Independent KiCad 10.0.6 check

Export, refill in memory, then run KiCad DRC on the actual generated board:

- Zero modem USB unconnected items.
- Zero shorts, clearance, hole-spacing or dangling-track findings.
- Eight unchanged footprint-library findings (four issues, four mismatches).
- 77 dangling-via findings and 184 unrelated unconnected items remain because
  ordinary area routing and ground completion are absent from this checkpoint.
  In particular, the two proposed GND reference vias are not yet certified
  connected to a refilled reference plane. Neither their placement nor native
  zone intent establishes that continuity.

The accepted export explicitly remains non-fabrication-ready. Existing complete
board artifacts are not replaced by this partial experiment. Actual stackup
geometry, per-layer impedance, reference-plane voids/continuity, via stubs,
USB skew/delay limits and final full-board DRC/CAM review remain open.

## Regressions and next work

Regression coverage includes complete top-wall bypass on In2, matched return
vias, deterministic joint construction, correct full physical spans, via/layer
budgets, all-layer walls, missing returns, asymmetric spans, disconnected
lanes, colliding vias, allowed 45-degree placements and own-land drill exclusion.
The complete suite passed 540 tests (459 upstream warnings). A subsequent
60-test focused run passed after adding the actual-pad-layer versus tentative
guide-access regression; the current suite collects 541 tests. Surface-only
alternatives now start on actual pad-side copper even when a global guide
proposes an inner-layer access reached through a tentative via.
The previous full-vertical profile regression is updated to require the new USB
policy while retaining zero-via/top-only RF profiles and absent qualification
evidence. Existing surface-only pair search/refinement remains covered.

The normal critical owner now tries this topology after bounded surface-only
joint search. General 3-D pair routing, jointly optimized ordinary/critical
escape assignment and the final ordinary/GND/full-board rerun remain separate
work; successful local or critical routing does not complete those tasks.

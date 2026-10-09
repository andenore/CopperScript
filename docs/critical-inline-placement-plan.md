# Critical inline component placement

The full-vertical USB filter illustrates a search gap: its routed pose is
`(33, 27) mm, 0°`, although a legal front-side pose at `(34, 12) mm, 270°`
reduces the four USB pad-to-pad Manhattan spans from 81.0 to 51.9 mm. The
current detailed refinement searches only two 1 mm grid steps around each
component, so it cannot cross the intervening U_MODEM_LEVEL courtyard to
reach the northern channel.

1. Recognize an inline part joining two explicit differential-pair groups.
   Require two two-terminal nets per group, one pad of each net on the inline
   part, and one shared external component per group. Do not infer criticality
   from names or attach the heuristic to unrelated multi-pin parts.
2. Generate bounded, deterministic poses along the segment between the two
   external pad-pair centroids. Try legal rotations at the midpoint and two
   flanking points, with perpendicular offsets so a blocked direct corridor
   does not hide a route around another component.
3. Insert these proposals into detailed refinement. Keep the existing full
   physical legality, escape-spacing, and placement score gates; a candidate
   must improve the score to displace the incumbent. The existing package
   access and critical-router preflight remains the final routing gate.
4. Test a synthetic inline pair, the full-vertical FL_USB pose, and a board
   with no qualifying pair. Run the current six-layer route and compare USB
   geometry, connectivity and native KiCad DRC with the prior board.

The proposal creates route-aware candidates without fixing FL_USB's coordinates
in the source. A passing geometric route still needs impedance and EMC review.

## Result

Implemented in the placement refinement and verified with a synthetic obstacle
case. The new full-vertical run is
`build/full-vertical/runs/20261009T081529667065Z`: FL_USB moved to
`(34, 15) mm, 270°`, between the modem and MCU. The four USB tracks total
54.82 mm, down from 113.26 mm in the previous routed run. KiCad's filled-board
DRC has zero violations and zero unconnected items. The modem-side pair now
uses two vias per member to reach In2.Cu; both USB pair skews are about 0.63 mm.
Those are routing tradeoffs to review with the stackup and USB requirements.

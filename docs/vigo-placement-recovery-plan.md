# Vigo placement recovery for the revised TPS62130A macro

## Evidence

With the same CopperScript revision and Vigo source, the original locked
TPS62130A asset gives a legal one-candidate placement. The revised CopperLib
asset fails with one or three candidates: `R_VDUT_BOT.P1` ends up 12.58 mm from
`U_MCU.PA3` against its 4 mm limit. Replaying all 142 poses from the original
placement with the revised rigid-macro members is legal, including every
relative constraint. The revised macro therefore has a feasible host-board
placement; this is a bounded-search regression, not grounds to relax the sense
resistor constraint.

## Repair

1. Instrument the rejected seeds at legalization, relative repair, refinement
   and final selection. Identify the first stage that loses the last legal pose
   or fails to find one. Keep the instrumentation out of production code.
2. Fix that stage generically. Keep the existing whole-group pack where it
   succeeds. When an oversized group connected through a fixed anchor exhausts
   its bounded search, split it into independently constrained movable groups
   and retry the existing local repacker on each close-coupled group. Never
   accept a relative-rule violation, move a fixed IC, or add Vigo coordinates
   to the generic placer.
3. Add a focused synthetic regression for the failing mechanism, then rerun
   placement tests. Test deterministic one-candidate Vigo placement with a
   temporary local CopperLib lock and all three revised buck scenes. Verify
   fixed anchors, macro geometry and zero relative-rule violations.
4. Only after legal placement, run Vigo preflight and independent native KiCad
   checks. Full detailed routing and CopperVigo lock update remain separate
   integration gates for the physical macro revision.

Do not weaken the 4 mm `R_VDUT_BOT`–`U_MCU.PA3` rule or pin unrelated support
parts merely to hide the search failure.

## Result, 2026-10-08

Instrumentation showed the rejected first seed was already illegal immediately
after legalization. Its 17 MCU-associated support parts formed one transitive
unit through the fixed MCU, and the whole-unit bounded pack did not find a pose.
Single-part relative repair reduced the divider error only slightly. Splitting
every large unit up front disrupted previously useful source/sink packing, so
the implemented fallback splits only after a whole-unit pack fails.

The revised three-macro Vigo trial now produces a legal one-candidate layout
with zero relative-constraint penalty and zero coarse congestion overflow.
Critical preflight with Vigo's package-access and preferred-ground-pad options
passes, including all ten CSI-2 differential pairs. The new synthetic regression
exercises an exhausted large-anchor pack with a close-coupled pair; the focused
placement suites pass. The original locked Vigo macro still produces byte-identical
one-candidate PCB and layout-report files with the updated placer. The complete
CopperScript test command stops during CM4 fixture setup because that example's
cached KiCad-footprint inventory differs from its committed lock; it does not
reach placement. A complete ordinary-net route and native filled-board DRC
remain for the later CopperLib/Vigo integration step.

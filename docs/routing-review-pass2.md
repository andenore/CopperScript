# Routing review after the second implementation pass

Reviewed 2026-10-01. Implementation commit: `4839b90`.
Work list: [routing-review-todo.md](routing-review-todo.md).
Previous review: [routing-review-pass1.md](routing-review-pass1.md).

## Outcome

Independent KiCad 10.0.6 refill and DRC reports **zero unconnected items**,
including the three previously open U_CC ground pins. There are no dangling
track, short, or clearance findings. The only eight violations are unchanged
footprint-library findings: four missing-library/footprint issues and four
library-copy mismatches. All 297 tests pass, including five installed-KiCad
differential connectivity cases and 20 new tests since pass 1.

This is verified copper connectivity, not a production-ready board. USB/RF
critical profiles are still absent; footprint, return-path, fabrication and CAM
qualification remain separate requirements. The CLI correctly retains a failing
draft status. No DRC check was waived and no routes were manually drawn.

## Implemented

- R10: Layer-aware physical copper-contact connectivity replaces endpoint-only
  unions. Exact rounded-shape contacts include interior T/cross junctions,
  pad edges and via annuli. Actual layer spans, drill voids and real gaps remain
  significant. Repeated pad numbers do not create virtual physical connections;
  zone intent is not treated as already filled copper.
- R3b: Search edges and shortcuts check their physical rays against actual
  outlines and track keepouts, including narrow concavities and nonuniform
  coordinates. Both diagonal and orthogonal edges are checked. Via-only keepouts
  do not unnecessarily block track movement. Mutable-copper clearance checks
  remain mandatory.
- R11: When a package has pending zone-connected pins, feedback reserves its
  whole same-zone-net pin group before ordinary routing, including neighbors
  already escaped on the previous board. The accepted current-placement trial
  closes U_CC.3/.10/.11 without moving any component.
- R9, partial: Equivalent physical-span via legality queries are memoized
  within one immutable single-net search. Static ray checks have geometry/axes
  identity; mutable copper results are not shared across searches. Operational
  timing/checkpoint support remains open.

Replaying pass-1 copper against the new native checker removes its four false
ordinary-net opens (V3V3, V3V8, MODEM_EN and PWR/MODEM_FB). GND remains explicitly
deferred to independent fill verification, rather than silently waived.

## Matched rerun and provenance

Board, placement inputs and CLI options match pass 1: 100 x 80 mm,
candidate-01, six-layer JLCPCB profile, 1 mm pitch, two passes, 20,000-state base
search budget, 10x failed-net repair, fanout, soft rip-up, constrained-pins-first,
progressive guides, 5 mm plane-contact radius and qualified GND via-in-pad.
The four-full-trial and six-local-transaction limits are unchanged. All six
local subset attempts were rejected; the first full current-placement/nearest
escape trial was accepted. Every exported component position and rotation
matches pass 1 exactly. No critical profiles or electrical inputs changed.

The final process used the exact implementation source committed as `4839b90`.
An external diagnostic wrapper recorded elapsed times without changing function
inputs, outputs or acceptance decisions. Initial placement/global took 164.6 s,
initial detailed routing 338.8 s, and the accepted feedback pipeline 572.8 s
(514.8 s detailed). The measured pipeline/zone-feedback interval was 1,148.6 s,
about 19 minutes, before final export/verification overhead. Pass 1 took about
45 minutes but tried four rejected full trials; this is not an isolated cache
speedup comparison. Tests ran concurrently during the initial stage.

Artifacts are retained outside the repository at:
`C:/Users/anden/Documents/Codex/2026-09-17/referenced-chatgpt-conversation-this-is-an/outputs/routing-review-pass2-2026-10-01/`.
They include the original draft, a disposable refilled `latest.kicad_pcb` and
project, `latest-drc.json`, extracted actual geometry, `layer-analysis.json`,
six layer images, central zooms, the U_CC access zoom, provenance and phase times.
The original user-owned repository drafts were not modified.

## Repeated six-layer review

Lengths exclude GND and come from independently extracted exported geometry.
Red tiles in the renders mark clipped centreline length in 5 mm squares: they
are density proxies, not verified routing-capacity overflow. Rendered pads are
rectangular approximations; KiCad, not the pictures, establishes connectivity.

| Layer | Pass 1 | Pass 2 | Review |
| --- | ---: | ---: | --- |
| F.Cu | 645.57 mm | 643.95 mm | Dense modem-level and MCU exit channels persist. The busiest tile is x35–40/y30–35: 22.83 mm across six nets. The MCU tile x45–50/y40–45 contains nine nets. Joint U_CC ground exits are now visible; long rectangular branches elsewhere still need protected cleanup. |
| In1.Cu | 0 | 0 | Dedicated GND plane remains free of ordinary tracks. Refilled connectivity now reaches all ground lands, including U_CC and J_SIM shields. |
| In2.Cu | 551.32 mm | 516.95 mm | Still the dominant signal layer, with broad perimeter detours. The densest tile x55–60/y25–30 contains MODEM_EN, PWR/MODEM_SS, V3V3 and V3V8 (17.36 mm). Local clearance-expanded demand should guide redistribution, not equal layer usage. |
| In3.Cu | 210.13 mm | 223.57 mm | Mostly sparse; the largest density is near Nordic access (9.96 mm, NRF_RF_RAW/NRF_SWDCLK). Horizontal length 123.39 mm exceeds vertical 54.93 mm. Preferences remain soft, and neutral fallback policy still needs explicit/local reporting. |
| In4.Cu | 143.74 mm | 148.72 mm | Sparse, with CAN_RX/GNSS_RF sharing the horizontal corridor near x50–60/y45–50. Long orthogonal trunks remain; RF cannot be moved merely to balance density. |
| B.Cu | 96.84 mm | 128.79 mm | More ordinary routing; hottest tile x40–45/y40–45 has ACCEL_INT, MODEM_PWRKEY_CTL and USER_BUTTON (12.16 mm). Useful diagonals coexist with right-angle chains requiring branch-safe cleanup. |

Signal length is about 1,661.98 mm versus 1,647.60 mm in pass 1; all-net vias
increase from 161 to 171. Ground closure costs approximately 14.38 mm of signal
copper and ten vias; this is not presented as a wirelength optimization.
Compared with the earlier substantially connected board (one ground open),
signal length remains below 1,684.75 mm and vias below 180, but its U_CC placement
differs by 0.5 mm, so this is not a controlled algorithm-only comparison.

Degree-two 90-degree turns fall from 99 to 95, and 45-degree turns rise from
135 to 136; junctions and interlayer turns are excluded. Front GND copper is
194.19 mm versus 191.24 mm and includes shield bridges and short escapes.
Reported escaped pads missing an F.Cu pad-centre endpoint remain zero. None of
these proxies replaces independent connectivity or electrical qualification.

## Remaining findings

R5/R7: MCU_MODEM_TX remains 29.51 mm for 8.08 mm pad separation (3.65x).
MODEM_STATUS is 66.64 mm against an 18.50 mm Euclidean pad-MST baseline (3.60x).
USER_LED_2_A is 57.53 mm against 21.60 mm (2.66x). These are bounded alternate-guide
and branch-safe cleanup candidates, not proof a shorter legal path exists.
USB_C_VBUS_DET improves from 12.40 to 7.11 mm, but other rails grow: V3V8 from
69.64 to 81.79 mm and V5 from 54.43 to 69.32 mm. Feasible-first guide acceptance
still does not optimize expensive incumbents. Orthogonal routing remains for
GNSS_RF, USER_LED_2 and V3V3.

R6: Global capacity overflow is still zero despite locally busy escape channels.
Clearance-aware local demand and failure feedback remain needed. This run also
used whole-board neutral fallback inside the accepted feedback pipeline;
localizing and reporting that policy remains unfinished.

R8: The critical list is empty. USB pairs and RF are ordinary independent nets;
neither zero opens nor visually straight tracks establish impedance, pairing,
reference continuity or RF suitability. Integrate explicit critical profiles
and protect their geometry before aggressively optimizing routes.

R12, newly recorded: Feedback's pre-final native checker reports USER_BUTTON
because SW_USER has disconnected same-number lands. Final duplicate-pad stitching
adds 16 tracks across SW_USER.1/.2 and J_CELL.2/J_GNSS.2; final native checking
has no ordinary-net opens. Acceptance did not introduce a new open: it requires
failures to be a subset of existing baseline failures. However, evaluating and
reporting closure at different stages can obstruct future trials. The same
surface-stitch helper lists J_POWER.SH/J_SIM.SH pending even though independent
filled-zone KiCad connectivity is complete. Move consistent physical-land
closure into candidate evaluation and reconcile pending evidence explicitly;
never infer same-number virtual contacts or waive genuine opens.

Native final status still has GND explicit-copper deferral and route-incomplete;
independent plane verification has zero opens/islands but fails its overall
gate on eight library findings. Keep those reasons separate from actual
unconnected copper. Production-ready status remains false.

## Next implementation order

1. R12 consistent physical-land closure/evidence through scoring and reporting.
2. R8 explicit USB/RF profiles and protected repair integration.
3. R6 local demand/fallback and R5 bounded feasible-route improvement.
4. R7 protected tree cleanup and remaining R9 timing/checkpoint support.
5. Repeat the same independent review, qualify the eight footprint findings,
   then complete fabrication, return-path and CAM gates.

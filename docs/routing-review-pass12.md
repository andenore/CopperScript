# Routing review pass 12: octilinear access and forward paired miters

This follows the source-backed USB winding correction in pass 11. The netlist,
library lock, original critical profiles, actual footprints and pinned Nordic
scene are retained. It is a critical preflight, not ordinary routing or a
fabrication-ready board.

## Implemented findings

R14: detailed pin access used a single pad-to-grid chord, irrespective of
its angle. Both selection and emission now construct the same bounded exact
axis/45-degree path, checking each leg with original width/layer, board-edge,
foreign-copper and keepout predicates. There is no oblique fallback or endpoint
snapping. Blocker-aware tentative routing can cross removable tracks only;
existing transactional checks remain required. See
[the access contract](octilinear-pin-access.md) and CS-124.

R15: adjacent offset miters can consume a short spine segment and reverse
an inner lane. The construction now requires both lane edges to retain
the corresponding spine edge's forward heading. A collapsed/reversed edge
rejects the whole joint candidate, letting the bounded search/refinement owner
try another topology. Original package escapes and paired profile/native
acceptance remain intact; no one-lane cleanup is used. See
[the paired contract](paired-route-refinement.md) and CS-125.

The initial forward-miter guard alone exposes a real goal-connector gap:
the old terminal bridge menu produces no replacement for the modem pair at
this placement (24 searches, 90,035 states, zero candidates). It is not treated
as success. Its partial checkpoint, including an interrupted first placement
trial, is retained separately under `routing-review-pass12-access-miters-2026-10-01`.
An initial 16-tail collar menu also failed the real board. The diagnostic
capture reproduced the original 23-search/63,106-state candidate without
accepting/exporting its unsafe geometry. Its last 0.045007 mm spine edge was
consumed by the inner miter. The final construction adds at most 24
heading-aware direct/collar tails per near-goal state, including two-leg
half/full-pitch advances that distribute sub-grid offsets across an S-turn.
Expanded-state, pitch and port-pair limits are unchanged. A translated/rotated
synthetic case and the real displacement regression distinguish the two-leg
menu from the insufficient single-advance version. An isolated fit against
the captured prefix and actual source pads/reservations passes original paired
profiles and full native acceptance; the final fresh search must still find
that geometry itself. No diagnostic candidate is used as production copper.

## Matched verification

Fresh locked/offline candidate-01 placement with the pinned RF scene, one
placement/global feedback iteration and five global iterations is rerun on
the same provisional 100 x 80 mm six-layer JLCPCB profile. All critical nets
retain their F.Cu/no-via profiles. The final run explicitly disables placement
repair, to measure the geometry changes at the original placement rather than
credit a pose change. Pass-11 copper/signoff tokens are not imported.

The final fresh run connects all eight critical nets with all 51 poses and the
global-routing fingerprint identical to pass 11. No placement-repair trial is
run. Critical status remains `warning` for external USB/RF qualification, not
production acceptance.

| Geometry | Pass 11 | Pass 12 |
| --- | ---: | ---: |
| Oblique RF access segments | 4 | 0 |
| Modem DP collinear reversal vertices | 1 | 0 |
| Nordic antenna feed length | 18.561 mm | 18.582 mm |
| GNSS feed length | 33.460 mm | 33.479 mm |
| Modem USB DM / DP length | 22.761 / 23.458 mm | 22.984 / 23.614 mm |
| Modem USB DM / DP bends | 16 / 17 | 16 / 16 |
| Total F.Cu segments | 80 | 83 |
| Critical vias | 0 | 0 |

Every emitted critical segment is axis-aligned or 45-degree (two-nanometre
integer heading tolerance), and the measured track-vertex graph has no
collinear reversals. MCU USB, cellular feed and raw Nordic matching-tree
copper are unchanged. RF access now has extra conventional corners and the
modem takes a slightly longer legal S-turn. This is a geometry-correctness
improvement, **not wirelength optimization or fewer segments overall**.
The antenna/GNSS feeds remain long and unqualified; crowded reference labels
remain draft F.Fab text, not a completed silkscreen layout.

The modem uses 23 searches and 63,033 states, versus 23/63,106 in pass 11.
Two geometric refinement proposals are emitted; the owner still remeasures
and atomically validates each complete pair. Original widths, gaps, layer/via
limits, port-pair/state budgets, pin mapping and RF template are unchanged.

Native DRC reports 50 ordinary-net opens plus incomplete routing, no hard
geometry findings. Independent KiCad 10.0.6 on disposable same-stem PCB/project
copies, with zone refill and all-severity DRC, reports 194 partial-board
unconnected items, **zero critical-net opens**, no short/clearance/dangling
findings and eight unchanged library findings (four issues/four mismatches).
No finding or rule is waived. Ordinary routing has not run in this preflight.

All 432 CopperScript tests pass (459 upstream CAM warnings), including 19 new
regression cases. The shared-access golden now expects three octilinear
segments rather than two including an oblique chord, and additionally checks
fresh native connectivity/clearance. All 30 CopperLib tests pass; its
compatibility reports are unchanged. Placement/global time is 194.84 s and
critical time 103.76 s with concurrent tests; these are operational timings,
not controlled speed benchmarks.

Final task artifacts: `outputs/routing-review-pass12-final-2026-10-01` contains
preflight JSON, original PCB/project, disposable KiCad refill/DRC, matched
exported-copper measurements and KiCad front-layer SVG/PNG. The original
pass-11 PCB SHA is verified before comparing geometry/poses. Recorded identities:

- Unchanged global route: `c5668ea3f5bb102e68f3becf6cb6eace033ba502d8b6a5af617c2577dd7cb86f`.
- Critical route: `d4abd396efd4f0c21a20733d1ff2dca2d06a8236ad02f9facb66cd18a7f89e0f`.
- Preflight bytes: `b7692e3df3ea7376842386da66252b95452dd1cbaa9ee336660d952e7f8f4f52`.
- Original PCB bytes: `01d4ef12eea0646b4db60be0620437427b1b962c84fcd203f321393d30e1ad97`.
- Independent KiCad report: `08f86a4a72e201d1fafa26a50c9a3b75d565f3eb915105b82bf49c8a7336e5a4`.

Reproduce with the [pass-9 preflight command](routing-review-pass9.md#reproduction-and-next-gates)
against corrected CopperLib `5bcbaa4`, setting `--critical-feedback-trials 0`.
R14/R15's two shape defects are resolved on the unchanged source placement.
Full ordinary rerouting, eight library findings, RF support,
antenna/reference-ground layout, actual impedance/return-path evidence and
manufacturing/CAM signoff remain separate gates.

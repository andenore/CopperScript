# Routing review pass 9: critical placement feedback

Electrical evidence superseded: the USB choke mapping used here was incorrect.
Zero critical copper opens did not validate the internal winding connections.
See [pass 11](routing-review-pass11.md); do not replay this rotation as a solution
for the corrected component.

The bounded controller repairs the source-backed RF scene's modem USB failure
without manual example coordinates, independent member routing, extra vias or
larger search budgets. Its first legal candidate rotates FL_USB from 90 to 270
degrees at the unchanged (45, 30) mm center. Every other component pose remains
unchanged. Global guides and all six critical groups are rebuilt before acceptance.
See the [controller contract](critical-placement-feedback.md) and decision CS-121.

## Control and trial

This experiment replays the [pass-8 control](routing-review-pass8.md), rather than
rerunning its expensive failed critical search. Exact control report/PCB SHA-256
values and current locked source bytes are checked. Current real footprints and
the pinned scene are resolved, initial placement/global routing are freshly
recomputed, and every pose and both placement/global fingerprints must match.
Control tracks/results are reconstructed and their critical fingerprint is
recomputed. The controller also verifies the full physical baseline and fresh
native connectivity. No cached signoff token is accepted.

The trial itself rebuilds **all** global and critical routing from empty copper.
The 100 x 80 mm, six-layer JLCPCB prototype, original no-via F.Cu critical profiles,
candidate-01, five global iterations, and provisional Nordic template are retained.
CopperLib is at `1702f54`. Only one placement trial is permitted; it is accepted
with zero failed critical nets and no hard native geometry findings.

| Group | Trial committed lengths | Tracks | Strategy |
| --- | --- | ---: | --- |
| NRF_RF_RAW | 2.306 mm | 4 | Local surface tree |
| NRF_RF_ANT | 22.726 mm | 2 | Local surface tree |
| CELL_RF | 62.651 mm | 11 | Exact single-net search |
| GNSS_RF | 33.498 mm | 9 | Exact single-net search |
| MCU USB DM / DP | 17.172 / 15.340 mm | 36 | Joint paired search |
| Modem USB DM / DP | 60.367 / 62.885 mm | 214 | Joint paired search |

All 276 tracks are on F.Cu, with no vias. RF lengths are unchanged from the
control. MCU USB becomes longer than its previous 7.950 / 7.321 mm route; the
acceptance objective is fewer critical failures, not global minimum length.
Modem USB uses nine searches and 258,851 states instead of exhausting 24 searches
and 720,000 states without a candidate. The 412 s feedback observation includes
trial global/critical routing and checking, not initial control reconstruction;
concurrent tests make it an operational observation, not a speed benchmark.

Native DRC finds 50 ordinary-net opens plus incomplete routing, no hard errors.
KiCad 10.0.6 refills a disposable same-stem PCB/project and finds 194 expected
partial-board unconnected items, **zero on the eight critical nets**, no short,
clearance or dangling-track findings, and the unchanged eight library findings
(four issues and four mismatches). Neither count means a fully routed board.
Critical status is `warning`, retaining external USB/RF qualification assumptions;
fabrication readiness remains false.

Recorded identities:

- Global route: `e72e5f294f296682a35dfd41930e03f1350128ce9d972a4e9ae779e6e64ff627`.
- Critical route: `8e9fcffb85cc44ff06b1de90665b2ca68166e791878cb7f14f6928f4d8742e91`.
- Feedback report bytes: `62b26def637fb9f3788160b748a99138e86200356516118445b5cd335bd61849`.
- Original PCB bytes: `c4ed6f45c6c0012114530ef95ebc6dbe09142df952f76e1ba6a9b4096fd9f79c`.
- KiCad report bytes: `c7044f0981e47cc2fd12da275579841819511e562d565e7cfff3bfb8aa38c9fd`.

## Reproduction and next gates

For a fresh run (no cached control), from the installed CopperScript checkout:

```powershell
uv run --no-sync python -m pcbir.critical_preflight examples/full_vertical_board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --placement-templates examples/full_vertical_placement_templates.json `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints" `
  --footprint-root "..\CopperLib\footprints" `
  --candidates 1 --placement-candidate candidate-01 --feedback-iterations 1 `
  --router-iterations 5 --critical-feedback-trials 1 `
  --report build/critical-feedback.json -o build/critical-feedback.kicad_pcb
```

404 CopperScript tests pass (459 upstream CAM warnings), as do 29 CopperLib
tests; compatibility output remains unchanged. Coverage includes stale inputs,
fixed/rigid and 45-degree placement rules, strict failed-net identity preservation,
missing/stale success flags, new shorts, deterministic rollback, CLI/pipeline
integration and interruption checkpoints. No rule, profile or DRC gate is waived.

Long segmented USB paths are a route-quality issue, not evidence of electrical
qualification. Bounded paired geometry improvement, Nordic support/reference-ground
copper, antenna corner/keepout rules, shorter GNSS/cellular placement, wider-guide
and layer-cost improvements, and the complete ordinary-net rerun remain open.
Actual stackup/return-path and independent manufacturing signoff remain mandatory.

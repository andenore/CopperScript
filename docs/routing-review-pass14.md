# Pass 14: physical guide costs and failed-terminal replay

Baseline: committed `38c69a0`, corrected pass-13 six-layer board. This pass
implements R16; it is not a full pipeline rerun or route-completion claim.

## Implemented and tested

Detailed search charges 50 units/mm of actual uncovered planar edge length,
not 50 units for each grid edge whose destination is outside a guide. Analytic
capsule/square clipping unions overlapping regions and includes partial boundary
crossings. Via guide exits are separate events. Explicit layer restrictions,
reserved GND plane, physical clearance and search budgets are unchanged.

The full suite passes 450 tests (459 upstream dependency warnings); two further
search-level regressions pass separately, bringing the tested inventory to 452.
Thirteen guide-cost tests cover pitches, nonuniform coordinate insertion,
boundary splits, overlap, diagonal round caps, large board offsets, access
squares, layer projection and via transitions. The search-level checks observe
the same final A* cost for whole versus subdivided edges whose target is inside.
Clipping rounds once per edge to integer nanometres; diagonal subdivision has
bounded nanometre rounding, not fixed per-edge deviation events.

## Matched saved-board experiment

Rebuild the current locked/offline electrical/physical inputs and pinned scene,
restore all 51 poses and actual pass-13 track/via geometry, and recompute global
guides. Serialized global routes match the original exactly. Run each of the
three failed nets independently with all existing copper immutable. Compare the
committed pre-change implementation with this implementation, using the same
1 mm pitch, two passes, progressive guides, 20,000-state budget and 10x existing
repair budget. Input copper and every placement remain identical.

Neither implementation connects a complete failed net; no partial tree is
committed. Both retain the preferred detailed policy after unsuccessful neutral
fallback. Observational search tracing records 380 calls across the six cases:

| Net | Old and new outcome on locked saved copper |
| --- | --- |
| MCU_NRF_TX | Exhaustive no-path, including half-pitch retry; U_NRF.10 target |
| MODEM_EN | Exhaustive no-path, including half-pitch retry; U_MCU.64 target |
| V3V3 | Coarse access no-path; refined tree searches also encounter budget exhaustion; final retained failure at U_NRF.36 |

Refined V3V3 search finds a temporary branch in both versions, with different
tree geometry, but cannot finish the complete net. Timing is diagnostic only:
concurrent tests/other work preclude a speed comparison. These saved-board
results are not the original full-run order, which exhausted MODEM_EN/V3V3
search budgets at different terminals.

Fresh KiCad 10.0.6 refill/DRC of the new replay export still reports 34 open
items on the same three nets and eight unchanged library findings. No new hard
copper violations, critical opens or GND opens appear. No committed copper or
layer-use improvement is claimed; the prior six-layer shape/density review
continues to apply to this unchanged geometry.
Fresh installed-KiCad extraction also matches the original tracks, vias, pads,
component poses, fills and board edges as complete multisets: 646 tracks,
153 vias, 471 pads and one filled polygon. Recomputed per-layer lengths retain
F.Cu 526.4714, In2.Cu 429.8056, In3.Cu 264.2810, In4.Cu 122.3036 and B.Cu
37.5895 mm of signal copper; the reserved In1.Cu plane has no signal tracks.

## Access findings and next implementation

Read-only exact-clearance queries distinguish ordinary detailed/fanout copper
from pads, critical reservations and ground escapes. On the saved board:

- U_MCU.62's local exits intersect MCU_MODEM_RX, MCU_MODEM_TX and MCU_NRF_RX.
- U_MCU.64's local exits intersect MCU_MODEM_RX, MCU_MODEM_TX and MCU_NRF_RX.
- U_NRF.36's local exits encounter NRF_RESET and adjacent package/ground geometry.
- No tested straight/45-degree radial escape to a legal non-via-in-pad site
  exists for those three terminals with all current copper locked, at 0.5,
  0.25 or 0.1 mm candidate spacing within 3 mm. This is a bounded candidate
  result, not proof that no arbitrary maze exit exists.
- Other terminals, including U_NRF.10 and PWR/U_MODEM.13, have legal radial
  escape/via candidates not necessarily represented by current grid access.

The original fanout report already marks U_MCU.62/.64 pending; U_NRF.36 had
an early escape that failed-net cleanup subsequently removed. Therefore
cost normalization and inner-layer balance alone cannot resolve this replay.
Next, evaluate package escape allocation against the pre-detail snapshot:
prioritize low-slack exits before easy neighbors and transactionally reconsider
only explicitly owned ordinary escape/detail copper. Preserve critical/GND
geometry and require every displaced signal to reconnect. Local neutral
fallback and search telemetry remain separately tracked; increasing the budget
cannot repair a disconnected terminal-access graph.

Evidence is in task outputs `routing-review-pass14-guide-costs/`: `retry.py`,
`global.json`, six per-case route/native/export artifacts, `search-events.json`,
`diagnose_access.py`, `access-diagnosis.json`, independent `physical-V3V3-drc.json`
and source/baseline `provenance.json`. Task outputs are not library signoff.
`verified-geometry.json` and `verified-layer-analysis.json` contain the fresh
installed-KiCad re-extraction and repeated layer measurements.
RF/support/stackup qualification, library findings and independent CAM remain
production gates.

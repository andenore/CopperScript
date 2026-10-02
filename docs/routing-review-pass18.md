# Pass 18: escape-first pipeline and package-access gate

This is a partial-stage experiment, not a completed routing run. Source
connectivity, CopperLib, physical rules, stackup, footprints and all 51 saved
pass-13 placements are unchanged. Unlike pass 17, long critical routes and GND
contacts are deliberately **not** imported before ordinary fanout.

## Implementation

See [package-access-first](package-access-first.md) for primary research,
contracts, options and remaining steps. `--fanout` now reserves compatible
ordinary crowded-pin exits first. Critical routing honors their exact tracks
and all-layer vias. Explicit early plane contacts follow. A native access /
critical compatibility gate prevents ordinary area search when incomplete.

The new bounded placement controller rebuilds from an unrouted placement;
accepted trials must reduce pending-pad or critical-failure identities while
preserving previous exits/critical connectivity and every hard constraint.
It supports explicitly permitted 45-degree rotations and rigid units. This
is exact placement feedback, not yet a demand-derived margin model.

## Unchanged-placement experiment

The saved KiCad angles require `(angle % 360 + 360) % 360` when reconstructing
Python Decimal poses. An initial harness used only `% 360`, retained `-90`
instead of allowed `270` and was correctly rejected by the placement gate.
The corrected experiment normalizes representation only; it does not move
components or change geometric rules. No such failure is waived in the code.

| Partial measurement | Pass 17, critical/GND-first | Pass 18, ordinary-access-first |
| --- | --- | --- |
| Crowded ordinary exits | 74 | 76 |
| Pending ordinary exits | MCU.62 / MCU.64 | None |
| Accepted critical signal nets | 8, preserved input | 6, freshly routed |
| Failed critical signal nets | None | USB_DM_MODEM / USB_DP_MODEM |

All 76 ordinary access identities survive the critical stage, including
`U_MCU.62` (`MCU_NRF_TX`) and `.64` (`MODEM_EN`). Four RF nets and the MCU-side
USB pair reconnect. The modem-side USB pair does not: its coarse candidate
conflicts with pad/reserved geometry and the bounded paired search produces
no accepted alternative (24 searches, 1,808 states, zero candidates).
Rejected pair copper is not committed. The resulting board has zero hard
native findings and keeps the access prefix unchanged.

Thus ordering alone does **not** establish compatible complete-board routing.
It solves the two local exit failures but transfers the bottleneck to a
critical interface. The new pipeline would stop with zero ordinary area
passes rather than spend another full routing run on this incompatible pattern.
No real-board placement-feedback trial or ordinary area rerun is claimed here.

Independent KiCad 10.0.6 DRC on the disposable partial export finds four
library issues, four library mismatches and 75 dangling-via findings, with
187 unconnected items. No short, clearance or drill-spacing finding is present.
Dangling vias and opens are expected in an unfinished package-access checkpoint;
they are not waived signoff findings. These raw counts are not comparable to
pass 13's full-route 34 open items. No filled-GND continuity is claimed.

## Next acceptance

Co-allocate/refine critical and ordinary package patterns instead of freezing
the first ordinary pattern. Include required plane contacts, usable boundary
ports and onward layer capacity, and use bounded legal placement feedback
when the package pattern is incompatible. Derive directional placement margin
from actual pin-bank/via/channel demand, not a uniform gap. Only after the
combined access/critical gate passes should the full routing, filled-zone and
all-layer review be repeated.

Evidence outside the checkout: `routing-review-pass18-escape-first/experiment.py`,
`preflight.json`, `exits-native.json`, `critical.json`, `native.json`,
`escape-first.kicad_pcb` / `.kicad_pro` and `kicad-drc.json`.

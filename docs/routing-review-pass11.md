# Routing review pass 11: USB choke winding correction

## Source finding

The official [Coilcraft 0603USB datasheet](https://www.coilcraft.com/getmedia/4e4ea8c1-6d24-4c0b-815c-9af87a76485b/0603usb.pdf),
Document 406-1, revised 09/10/24, page 1 schematic, specifies windings **1-2
and 4-3**, dotted ends **1 and 4**. The complete rendered page was inspected;
text extraction order alone does not establish winding topology. Source SHA-256:
`a515a5af63a3098746b375c726cd5873390da8606ec9a9abbae982bb59b21c09`.

CopperLib previously assigned DP_IN/DP_OUT to 1/4 and DM_IN/DM_OUT to 2/3.
Consequently MCU D+/D- landed across one winding (1/2), and modem D+/D-
across the other (4/3), rather than each signal traversing its own winding.
No routing or clearance adjustment can repair that library error.

| Signal | Correct physical pad |
| --- | --- |
| DP_IN (MCU D+) | 1 |
| DM_IN (MCU D-) | 4 |
| DP_OUT (modem D+) | 2 |
| DM_OUT (modem D-) | 3 |

Both inputs use the dotted side; logical signal polarity/net names are unchanged.
The installed KiCad footprint already has the appropriate numbered land geometry
and is not replaced. The correction is factual, not a routing optimization.

## Implementation and invalidation

CopperLib records the source revision/location/hash on the part and in
`packages/parts/coilcraft/0603usb/evidence/usb-choke-audit.json`. Its regression verifies that each
signal traverses a distinct winding and that inputs share dot polarity. The
CopperScript integration regression verifies the lowered physical nets:
MCU DP/DM on pads 1/4, modem DP/DM on pads 2/3. The local dependency lock is
refreshed; no compatibility shim or schema version increment is introduced.

Earlier PCB/DRC measurements remain historical geometric evidence only.
Their USB electrical acceptance is invalidated, including passes 5 and 7-10.
ERC/passive-pin compatibility and copper DRC do not model internal coil paths;
the topology-specific source regressions catch this case. No claim of a generic
internal conductive-path ERC or SPICE model is made. See CS-123.

## Fresh verification

A new locked/offline critical preflight uses actual installed KiCad/CopperLib
footprints, the existing pinned Nordic scene, fresh candidate-01 placement,
one placement/global feedback iteration and five global-routing iterations.
The provisional 100 x 80 mm, six-layer JLCPCB profile and original F.Cu/no-via
USB/RF rules are unchanged. Up to four legal critical-placement repair trials
are allowed if needed; **the historical 270-degree choke rotation is not replayed**.
Ordinary routing is not part of this preflight, and fabrication readiness is false.

The fresh run completes with all eight critical nets connected, zero placement
repair trials and zero vias. Choke pose is (48, 31) mm, 90 degrees, selected
by ordinary placement rather than forced to the historical 270-degree repair.
MCU is now (39, 38) mm / 0 degrees, modem (43, 20) mm / 90 degrees,
and Nordic (50, 37) mm / 90 degrees. The corrected physical nets change
placement/global scoring, so this is **not a matched algorithm benchmark**.

| Critical net/group | Committed length | Segments |
| --- | --- | ---: |
| MCU USB DM / DP | 28.298 / 27.039 mm | 26 |
| Modem USB DM / DP | 22.761 / 23.458 mm | 38 |
| Nordic raw matching tree | 2.306 mm | 4 |
| Nordic antenna feed | 18.561 mm | 4 |
| Cellular feed | 13.325 mm | 2 |
| GNSS feed | 33.460 mm | 6 |

There are 80 F.Cu tracks total. Native checking reports 50 ordinary-net opens
plus incomplete routing, no hard geometry findings. Independent KiCad 10.0.6
refill/DRC on disposable PCB/project copies reports 194 partial-board unconnected
items, **zero critical-net opens**, no short/clearance/dangling findings, and the
same eight library findings (four issues and four mismatches). Those findings
remain visible. This is a partial critical PCB, not a fully routed board.

Placement/global time: 146.87 s; all critical routing/checks: 161.77 s.
Tests ran concurrently; these are operational timings, not speed benchmarks.
All 413 CopperScript tests pass (459 upstream CAM warnings), all 30 CopperLib
tests pass, compatibility outputs are unchanged, and locked/offline ERC passes.
CopperLib correction commit: `5bcbaa40504515e758f1bc9dba18c00f89b26939`.

Task artifacts are in `outputs/routing-review-pass11-usb-pinout-2026-10-01`:
preflight report, original critical PCB/project, independent disposable KiCad
refill/DRC, measured copper/poses, and a KiCad front-layer SVG/PNG. Source and
artifact identities:

- Dependency lock bytes: `aef8c8dfa80c5481456e4c19dfd1fc2da6a1ef9a35bbeea51d5a90dc5f6ad744`.
- Global route: `c5668ea3f5bb102e68f3becf6cb6eace033ba502d8b6a5af617c2577dd7cb86f`.
- Critical route: `8371e46311fa222fa19b34abfc30da4af37eb9f08829668c52e6ffb5ce7c9d61`.
- Preflight bytes: `fd115796286ea6959952a0d9ddf29e60b91911ceceb7b55763dc595530292df5`.
- Original PCB bytes: `7e3d75b379ddf0bc3056600ac2d9e6fe1697796c6b7263707cce5f3bbc6cdb5e`.
- Independent KiCad report: `1c3742533563b091419df55603984bf2b0c6c7e39fec13dd0bc830a24833db5c`.

## Repeated shape review and next work

All USB segments are axis-aligned or 45-degree. The modem DP chain nevertheless
contains a short collinear reversal at its package taper/spine transition;
its raw emitted sequence backtracks about 0.034 mm. This should be repaired
by the paired construction/refinement owner, not independently pruned from one
lane or hidden by segment-count metrics. Atomic profile/native checks remain
mandatory. It does not prevent current copper connectivity.

The exact single-ended RF paths still have oblique package/grid access links:
Nordic antenna vectors (-0.500, 0.050) and (-0.008, -0.4016) mm, and GNSS
(-0.500, 0.475) and (0.250, -0.200) mm. These are real geometry, not nanometre
rounding. Clearance-safe octilinear access construction is needed before
claiming the entire critical stage obeys the straight/45-degree guideline.
Long antenna/GNSS feeds and crowded central references remain review issues;
F.Fab reference overlap is not a copper short or a finished silkscreen plan.

Next: octilinear critical access and paired reversal-safe construction, then
the full ordinary-net rerun on the corrected source with independent layer review.
USB SI/EMC, actual stackup/return-path evidence, RF support/antenna qualification,
library findings and full-board/manufacturing signoff remain mandatory gates.

Reproduce the fresh run with the [pass-9 preflight command](routing-review-pass9.md#reproduction-and-next-gates)
against the corrected library/lock, changing `--critical-feedback-trials` to 4.
The option bounds available repair trials; this run needed none. Do not replay
the historical selected placement/copper or reuse its electrical acceptance.

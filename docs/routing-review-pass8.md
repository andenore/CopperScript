# Pass 8: shorten critical matching-tree guides

The bounded local single-ended improvement is integrated before critical copper
is locked. See [algorithm/acceptance contract](local-critical-routing.md). Every
physical terminal remains represented. Failed/equal-length proposals preserve
a connected incumbent; original profile budgets, exact clearance and fresh
native connectivity remain mandatory. Paired nets retain joint search.

## Matched-placement review

Task artifacts: `routing-review-pass8-local-critical-2026-10-01/preflight.json`,
`critical-board.kicad_pcb`/project and `kicad-check/drc.json`/refilled copy.
Settings match pass 7: locked/offline current full-vertical example, explicit
Nordic scene, installed KiCad/CopperLib footprints, JLCPCB six-layer prototype,
100 x 80 mm, candidate-01, one placement-feedback iteration, five global
iterations and 5 mm tiles. Fresh placement is recomputed; exported-board
comparison confirms all 51 component poses are unchanged. The reference packet
uses its canonical LF identity recorded in pass 7; historical signoff tokens
are not reused.

| Group | Pass 7 length | Pass 8 length | Result |
| --- | --- | --- | --- |
| Nordic raw matching tree | 17.19 mm | 2.31 mm | Local tree, four segments, no vias |
| Nordic antenna feed | 23.03 mm | 22.73 mm | Local path, two segments, no vias |
| Cellular feed | 62.65 mm | 62.65 mm | Exact maze fallback; placement still unsuitable |
| GNSS feed | 33.50 mm | 33.50 mm | Exact maze fallback; placement still open |
| MCU USB pair | 7.95 / 7.32 mm | 7.95 / 7.32 mm | Joint search connects |
| Modem USB pair | None | None | 24 searches / 720,000 states / zero candidates |

Raw matching copper is 86.6% shorter, through an algorithmic alternative rather
than a manual component/copper edit. The feed's smaller improvement does not
solve antenna placement. Both complete terminal sets use unchanged F.Cu/no-via
profiles and straight/45-degree segments. All six accepted critical nets connect.
Cellular/GNSS lengths remain unchanged, but emitted segmentation can differ
because exact fallback sees new Nordic reservations. Other copper bytes are not
claimed identical.

Native: 52 open nets plus incomplete routing, no hard geometry errors. KiCad
10.0.6 independently refills a disposable same-stem board/project and finds
196 expected partial-board unconnected items, including both modem pair members;
none are on the six accepted critical nets. Short, clearance and dangling
findings remain zero. Eight library findings remain (four issues/mismatches
each). Modem guide errors are rejected-proposal diagnostics, not installed
shorts. Ordinary nets have not been rerouted; this is not a completed-board
regression comparison with pass 3.

Observed phases: resolve 0.29 s, placement/global 160.74 s, critical 860.02 s.
Concurrent tests and changed reservations make these uncontrolled operational
measurements, not a speed benchmark against pass 7's 1185.40 s critical phase.
The rectangular edge fast path is active. Critical status remains failed and
fabrication readiness false; `complete` means preflight execution finished.

Recorded identities:

- Global route: `40db2f758af521034905c416ae2e16625ce41c3548e27b094c5d89d473b2071d`.
- Critical route: `3f70a0e443545f1b4cd695dc21ee7caacd8cd411a29e4619d9eaad6a07123314`.
- Preflight bytes: `eb54b5c7c599b83f7b46174a456fbedb770eea9b06da975da307f0a137c86523`.
- Original PCB bytes: `e9d4ab3dce15f194543c83a512f174b0b60eadda1e3f2d78fa553f0454207a79`.
- KiCad report bytes: `219ff5266cfe055fac75dcbbd56263d4fd15ce812cd7a4c4e40421009399b2ce`.

## Checkpoints and regressions

Preflight now saves group started/finished notifications, completed results and
elapsed time, and records the selected scene byte identity. Running/finished-group
interruption tests retain incomplete status without final signoff; observational
callbacks preserve route identity. The above full-board run launched before the
checkpoint callback was added: its saved report has whole-phase timing, not
retroactively invented per-group data. The final duplicate-land RF/clock guard
does not affect its two-land feeds or three-land generic matching tree.

386 CopperScript tests pass (459 upstream CAM-library warnings); 29 CopperLib
tests pass and compatibility outputs remain unchanged. New cases cover branch
and repeated-land preservation, disconnected terminals, original budgets,
reservations/keepouts, equal-length rollback, common layers/bounded size,
malformed-shorter-proposal rejection, RF/clock limits, deterministic output
and progress/interruption serialization. Locked/offline full-vertical ERC passes.
No fabrication rule or DRC gate is waived.

## Next blocker

Modem USB needs package-access/placement feedback. The control has U_MODEM at
(39, 13) mm / 90 degrees and FL_USB at (45, 30) mm / 90 degrees; the modem pair's
choke lands are on its lower edge while the modem lies above. This is an
orientation/access issue to test, not a proven root cause or permission for a
manual 180-degree fix. Evaluate permitted choke rotations/legal moves
algorithmically, rebuilding both USB pairs and RF reservations transactionally.
Reject any new critical open or hard DRC; do not inflate search budgets blindly.

Nordic support/reference-ground geometry, antenna corner keepouts, shorter
cellular/GNSS placement, general R5/R6/R7 improvements and complete R8d rerouting
remain open. Short raw matching copper is not RF performance, actual-stackup/
return-path or manufacturing qualification.
